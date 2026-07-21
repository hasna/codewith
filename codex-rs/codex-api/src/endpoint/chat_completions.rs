use crate::auth::SharedAuthProvider;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::common::ResponsesApiRequest;
use crate::common::TextControls;
use crate::endpoint::responses::ResponsesOptions;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::rate_limits::parse_all_rate_limits;
use crate::requests::Compression;
use crate::requests::headers::build_session_headers;
use crate::requests::headers::insert_header;
use crate::requests::headers::subagent_header;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::HttpTransport;
use codex_client::RequestCompression;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::instrument;
use tracing::trace;

const REQUEST_ID_HEADER: &str = "x-request-id";

/// Google Gemini (via its OpenAI-compatibility endpoint) returns an encrypted
/// `thought_signature` on each function/tool call under
/// `tool_calls[].extra_content.google.thought_signature`. Gemini 3 models
/// reject multi-step tool loops with a 400 (`Function call is missing a
/// thought_signature`) unless the signature from the previous response is
/// echoed back on the corresponding tool call in every subsequent request.
///
/// The chat adapter is stateless across turns and a fresh client is built per
/// turn, while conversation history round-trips through `ResponseItem`s that
/// have no field to carry a signature. We therefore keep a small
/// process-global, call-id-keyed cache: the signature is recorded when a tool
/// call is received and re-attached when that same call is replayed in a later
/// tool-turn request. Entries are keyed by the (globally unique) tool-call id,
/// so there is no cross-request or cross-provider contamination.
const THOUGHT_SIGNATURE_CACHE_CAP: usize = 4096;

static THOUGHT_SIGNATURE_CACHE: LazyLock<Mutex<ThoughtSignatureCache>> =
    LazyLock::new(|| Mutex::new(ThoughtSignatureCache::default()));

#[derive(Default)]
struct ThoughtSignatureCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

/// Record the Gemini thought signature for `call_id`, evicting the oldest entry
/// once the cache exceeds its capacity.
fn store_thought_signature(call_id: &str, signature: String) {
    if call_id.is_empty() || signature.is_empty() {
        return;
    }
    let Ok(mut cache) = THOUGHT_SIGNATURE_CACHE.lock() else {
        return;
    };
    if cache.map.insert(call_id.to_string(), signature).is_none() {
        cache.order.push_back(call_id.to_string());
        while cache.order.len() > THOUGHT_SIGNATURE_CACHE_CAP {
            if let Some(evicted) = cache.order.pop_front() {
                cache.map.remove(&evicted);
            }
        }
    }
}

/// Look up a previously recorded Gemini thought signature for `call_id`.
fn thought_signature_for(call_id: &str) -> Option<String> {
    let cache = THOUGHT_SIGNATURE_CACHE.lock().ok()?;
    cache.map.get(call_id).cloned()
}

/// Extract `google.thought_signature` from a tool call's `extra_content` object.
fn thought_signature_from_extra_content(extra_content: &Value) -> Option<String> {
    extra_content
        .get("google")?
        .get("thought_signature")?
        .as_str()
        .filter(|signature| !signature.is_empty())
        .map(str::to_string)
}

pub struct ChatCompletionsClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport> ChatCompletionsClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn codex_client::RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    #[instrument(
        name = "chat_completions.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "chat_completions_http",
            http.method = "POST",
            api.path = "chat/completions"
        )
    )]
    pub async fn stream_request(
        &self,
        request: ResponsesApiRequest,
        options: ResponsesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let ResponsesOptions {
            session_id,
            thread_id,
            session_source,
            extra_headers,
            compression,
            turn_state,
        } = options;
        // Some OpenAI-compatible providers (e.g. Cerebras, NVIDIA vLLM) reject
        // `reasoning_effort: "none"` and only accept low/medium/high, which
        // silently breaks tool calling. Map None -> "low" for those providers;
        // OpenAI/OpenRouter tolerate "none" and keep it unchanged.
        let map_none_reasoning_to_low = {
            let provider = self.session.provider();
            provider.is_cerebras_endpoint() || provider.is_nvidia_endpoint()
        };
        let ChatRequestParts {
            body,
            tool_name_map,
        } = chat_request_from_responses(request, map_none_reasoning_to_low)?;

        let mut headers = extra_headers;
        if let Some(ref thread_id) = thread_id {
            insert_header(&mut headers, "x-client-request-id", thread_id);
        }
        headers.extend(build_session_headers(session_id, thread_id));
        if let Some(subagent) = subagent_header(&session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }

        self.stream(body, headers, compression, turn_state, tool_name_map)
            .await
    }

    fn path() -> &'static str {
        "chat/completions"
    }

    #[instrument(
        name = "chat_completions.stream",
        level = "info",
        skip_all,
        fields(
            transport = "chat_completions_http",
            http.method = "POST",
            api.path = "chat/completions",
            turn.has_state = turn_state.is_some()
        )
    )]
    async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
        tool_name_map: HashMap<String, ChatToolName>,
    ) -> Result<ResponseStream, ApiError> {
        let request_compression = match compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_with(
                Method::POST,
                Self::path(),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(spawn_chat_completions_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
            tool_name_map,
        ))
    }
}

#[derive(Debug)]
struct ChatRequestParts {
    body: Value,
    tool_name_map: HashMap<String, ChatToolName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChatToolName {
    namespace: Option<String>,
    name: String,
}

fn chat_request_from_responses(
    request: ResponsesApiRequest,
    map_none_reasoning_to_low: bool,
) -> Result<ChatRequestParts, ApiError> {
    let mut tool_name_map = HashMap::new();
    let tools = chat_tools_from_responses(&request.tools, &mut tool_name_map)?;
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": chat_messages_from_responses(&request.instructions, &request.input)?,
        "stream": request.stream,
        "parallel_tool_calls": request.parallel_tool_calls,
    });

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = Value::String(request.tool_choice);
    }
    if let Some(reasoning_effort) =
        chat_reasoning_effort(request.reasoning.as_ref(), map_none_reasoning_to_low)
    {
        body["reasoning_effort"] = Value::String(reasoning_effort);
    }
    if let Some(service_tier) = request.service_tier {
        body["service_tier"] = Value::String(service_tier);
    }
    if let Some(response_format) = chat_response_format(request.text.as_ref()) {
        body["response_format"] = response_format;
    }

    Ok(ChatRequestParts {
        body,
        tool_name_map,
    })
}

fn chat_reasoning_effort(
    reasoning: Option<&crate::common::Reasoning>,
    map_none_reasoning_to_low: bool,
) -> Option<String> {
    let effort = reasoning.and_then(|reasoning| reasoning.effort.as_ref())?;
    match effort {
        ReasoningEffort::None if map_none_reasoning_to_low => Some("low".to_string()),
        ReasoningEffort::None => Some("none".to_string()),
        ReasoningEffort::Minimal | ReasoningEffort::Low => Some("low".to_string()),
        ReasoningEffort::Medium => Some("medium".to_string()),
        ReasoningEffort::High | ReasoningEffort::XHigh => Some("high".to_string()),
        ReasoningEffort::Custom(effort) => Some(effort.clone()),
    }
}

fn chat_response_format(text: Option<&TextControls>) -> Option<Value> {
    let format = text?.format.as_ref()?;

    Some(serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": &format.name,
            "strict": format.strict,
            "schema": &format.schema,
        }
    }))
}

fn chat_tools_from_responses(
    tools: &[Value],
    tool_name_map: &mut HashMap<String, ChatToolName>,
) -> Result<Vec<Value>, ApiError> {
    let mut chat_tools = Vec::new();
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => {
                if let Some(chat_tool) =
                    chat_function_tool(tool, /*namespace*/ None, tool_name_map)?
                {
                    chat_tools.push(chat_tool);
                }
            }
            Some("namespace") => {
                let namespace = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                    ApiError::InvalidRequest {
                        message: "namespace tool omitted name".to_string(),
                    }
                })?;
                let Some(namespace_tools) = tool.get("tools").and_then(Value::as_array) else {
                    continue;
                };
                for namespace_tool in namespace_tools {
                    if let Some(chat_tool) =
                        chat_function_tool(namespace_tool, Some(namespace), tool_name_map)?
                    {
                        chat_tools.push(chat_tool);
                    }
                }
            }
            Some(
                "web_search"
                | "web_search_20250305"
                | "web_search_20260209"
                | "openrouter:web_search",
            ) => {
                if let Some(native_tool) = chat_native_web_search_tool(tool) {
                    chat_tools.push(native_tool);
                }
            }
            Some("custom" | "tool_search" | "image_generation") | None => {}
            Some(_) => {}
        }
    }
    Ok(chat_tools)
}

fn chat_native_web_search_tool(tool: &Value) -> Option<Value> {
    match tool.get("type").and_then(Value::as_str) {
        Some("web_search_20250305" | "web_search_20260209" | "openrouter:web_search") => {
            Some(tool.clone())
        }
        Some("web_search")
            if ![
                "external_web_access",
                "filters",
                "user_location",
                "search_context_size",
                "search_content_types",
            ]
            .iter()
            .any(|field| tool.get(field).is_some()) =>
        {
            Some(tool.clone())
        }
        Some(_) | None => None,
    }
}

fn chat_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    tool_name_map: &mut HashMap<String, ChatToolName>,
) -> Result<Option<Value>, ApiError> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return Ok(None);
    }

    let name =
        tool.get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::InvalidRequest {
                message: "function tool omitted name".to_string(),
            })?;
    let encoded_name = encode_chat_tool_name(namespace, name)?;
    tool_name_map.insert(
        encoded_name.clone(),
        ChatToolName {
            namespace: namespace.map(str::to_string),
            name: name.to_string(),
        },
    );

    Ok(Some(serde_json::json!({
        "type": "function",
        "function": {
            "name": encoded_name,
            "description": tool
                .get("description")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
            "parameters": tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
        }
    })))
}

/// Maximum tool-name length accepted by the OpenAI chat-completions schema.
const CHAT_TOOL_NAME_MAX_LEN: usize = 64;

fn encode_chat_tool_name(namespace: Option<&str>, name: &str) -> Result<String, ApiError> {
    let encoded = namespace.map_or_else(
        || name.to_string(),
        |namespace| format!("{namespace}__{name}"),
    );
    Ok(truncate_chat_tool_name(encoded))
}

/// Chat-completions providers reject tool names longer than 64 characters, so
/// deterministically fold overlong names into a `<prefix>_<hash>` form that
/// stays within the limit. The mapping is a pure function of the input, so the
/// same tool name always encodes identically (and round-trips via
/// `tool_name_map`) whether it comes from the tool list or from replayed
/// function-call history.
fn truncate_chat_tool_name(encoded: String) -> String {
    if encoded.chars().count() <= CHAT_TOOL_NAME_MAX_LEN {
        return encoded;
    }
    let mut hasher = DefaultHasher::new();
    encoded.hash(&mut hasher);
    let suffix = format!("_{:016x}", hasher.finish());
    let prefix_len = CHAT_TOOL_NAME_MAX_LEN.saturating_sub(suffix.chars().count());
    let prefix: String = encoded.chars().take(prefix_len).collect();
    format!("{prefix}{suffix}")
}

fn chat_messages_from_responses(
    instructions: &str,
    input: &[ResponseItem],
) -> Result<Vec<Value>, ApiError> {
    let mut messages = Vec::new();
    if !instructions.trim().is_empty() {
        messages.push(serde_json::json!({
            "role": "system",
            "content": instructions,
        }));
    }

    // Buffer for consecutive function calls belonging to the same assistant
    // turn. Chat-completions rejects two consecutive assistant messages that
    // both carry `tool_calls` (HTTP 400), so parallel tool calls must be
    // coalesced into a single assistant message with a `tool_calls` array.
    let mut pending_tool_calls: Vec<Value> = Vec::new();

    // Buffer for a reasoning item that must be replayed onto the assistant
    // message that follows it. Only *signed* reasoning is replayed (see
    // `ResponseItem::Reasoning` arm below); it is attached to the next
    // assistant message (either a plain assistant message or the coalesced
    // `tool_calls` message).
    let mut pending_reasoning: Option<PendingReasoning> = None;

    for item in input {
        if let ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } = item
        {
            let mut tool_call = serde_json::json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": encode_chat_tool_name(namespace.as_deref(), name)?,
                    "arguments": chat_tool_arguments(arguments),
                }
            });
            // Re-attach the Gemini thought signature (if any) recorded when this
            // tool call was received, so Gemini 3 multi-step tool loops validate.
            if let Some(signature) = thought_signature_for(call_id) {
                tool_call["extra_content"] = serde_json::json!({
                    "google": { "thought_signature": signature }
                });
            }
            pending_tool_calls.push(tool_call);
            continue;
        }

        // A reasoning item ends the current run of tool calls (flushing them,
        // which consumes any earlier pending reasoning), then records itself so
        // it can be replayed onto the assistant message that follows.
        if let ResponseItem::Reasoning {
            content,
            encrypted_content,
            ..
        } = item
        {
            flush_pending_tool_calls(
                &mut messages,
                &mut pending_tool_calls,
                &mut pending_reasoning,
            );
            // Provider safety: only *signed* reasoning is echoed back. Signed
            // reasoning (e.g. Anthropic-style extended thinking) must be
            // preserved across tool turns for continuity, whereas unsigned
            // chain-of-thought (e.g. DeepSeek's `reasoning_content`) must NOT be
            // sent back — such providers reject `reasoning_content` on input.
            if let Some(signature) = encrypted_content {
                pending_reasoning = Some(PendingReasoning {
                    text: reasoning_text_from_content(content),
                    signature: signature.clone(),
                });
            }
            continue;
        }

        // Any non-function-call item ends the current run of tool calls, so
        // flush them into one assistant message before emitting this item.
        flush_pending_tool_calls(
            &mut messages,
            &mut pending_tool_calls,
            &mut pending_reasoning,
        );

        match item {
            ResponseItem::Message { role, content, .. } => {
                let mut message = serde_json::json!({
                    "role": chat_message_role(role),
                    "content": text_from_content_items(content)?,
                });
                // Attach buffered signed reasoning to this assistant message.
                if chat_message_role(role) == "assistant"
                    && let Some(reasoning) = pending_reasoning.take()
                {
                    apply_reasoning_to_message(&mut message, &reasoning);
                }
                messages.push(message);
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": output_text(output),
                }));
            }
            // Handled above; listed for exhaustiveness.
            ResponseItem::FunctionCall { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::Other => {}
        }
    }

    // Flush any trailing tool calls that ended the input.
    flush_pending_tool_calls(
        &mut messages,
        &mut pending_tool_calls,
        &mut pending_reasoning,
    );

    if messages.is_empty() {
        messages.push(serde_json::json!({
            "role": "user",
            "content": "",
        }));
    }

    Ok(messages)
}

/// Emit the buffered function calls (if any) as a single assistant message
/// carrying a `tool_calls` array, so parallel calls form one valid message.
/// Any buffered signed reasoning is attached to that assistant message so it
/// round-trips ahead of the tool calls it produced.
fn flush_pending_tool_calls(
    messages: &mut Vec<Value>,
    pending_tool_calls: &mut Vec<Value>,
    pending_reasoning: &mut Option<PendingReasoning>,
) {
    if pending_tool_calls.is_empty() {
        return;
    }
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": std::mem::take(pending_tool_calls),
    });
    if let Some(reasoning) = pending_reasoning.take() {
        apply_reasoning_to_message(&mut message, &reasoning);
    }
    messages.push(message);
}

/// Signed reasoning buffered for replay onto the following assistant message.
#[derive(Debug, Clone)]
struct PendingReasoning {
    text: String,
    signature: String,
}

/// Attach signed reasoning to an assistant chat message. The text is echoed
/// under `reasoning_content` and the (opaque) signature under
/// `reasoning_signature` so signature-based reasoning continuity survives the
/// round-trip. Only ever called for signed reasoning (see the
/// `ResponseItem::Reasoning` handling in `chat_messages_from_responses`).
fn apply_reasoning_to_message(message: &mut Value, reasoning: &PendingReasoning) {
    if let Some(object) = message.as_object_mut() {
        if !reasoning.text.is_empty() {
            object.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning.text.clone()),
            );
        }
        object.insert(
            "reasoning_signature".to_string(),
            Value::String(reasoning.signature.clone()),
        );
    }
}

/// Concatenate the textual parts of a reasoning item's `content`.
fn reasoning_text_from_content(content: &Option<Vec<ReasoningItemContent>>) -> String {
    let Some(items) = content.as_ref() else {
        return String::new();
    };
    items
        .iter()
        .map(|item| match item {
            ReasoningItemContent::ReasoningText { text } | ReasoningItemContent::Text { text } => {
                text.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Chat-completions expects `arguments` to be a JSON object string. Zero-arg
/// tool calls otherwise serialize as an empty string, which fails downstream
/// JSON parsing, so normalize empty arguments to `"{}"`.
fn chat_tool_arguments(arguments: &str) -> &str {
    if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    }
}

fn chat_message_role(role: &str) -> &str {
    match role {
        "developer" => "system",
        "system" | "user" | "assistant" | "tool" => role,
        _ => role,
    }
}

fn text_from_content_items(items: &[ContentItem]) -> Result<String, ApiError> {
    let mut text = Vec::new();
    for item in items {
        match item {
            ContentItem::InputText { text: item_text }
            | ContentItem::OutputText { text: item_text } => {
                text.push(item_text.clone());
            }
            ContentItem::InputImage { .. } => {
                return Err(ApiError::InvalidRequest {
                    message: "chat completions providers do not support image input".to_string(),
                });
            }
        }
    }
    Ok(text.join("\n"))
}

fn output_text(output: &FunctionCallOutputPayload) -> String {
    output.body.to_text().unwrap_or_default()
}

fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
    tool_name_map: HashMap<String, ChatToolName>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(turn_state) = turn_state.as_ref()
        && let Some(header_value) = stream_response
            .headers
            .get("x-codex-turn-state")
            .and_then(|v| v.to_str().ok())
    {
        let _ = turn_state.set(header_value.to_string());
    }
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_chat_sse(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            telemetry,
            tool_name_map,
        )
        .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    id: Option<String>,
    model: Option<String>,
    choices: Vec<ChatCompletionChoice>,
    usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    delta: ChatCompletionDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionDelta {
    role: Option<String>,
    content: Option<String>,
    // DeepSeek (and several other OpenAI-compat providers) stream the model's
    // chain-of-thought here, alongside `content`.
    #[serde(default)]
    reasoning_content: Option<String>,
    // Some providers (e.g. OpenRouter) expose reasoning under `reasoning`, which
    // may be a plain string or an object carrying `{text, signature}`. Kept as a
    // raw value so an unexpected shape never fails the whole chunk parse.
    #[serde(default)]
    reasoning: Option<Value>,
    tool_calls: Option<Vec<ChatCompletionToolCallDelta>>,
}

/// A single reasoning delta extracted from a chat-completions chunk.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReasoningDelta {
    text: String,
    signature: Option<String>,
}

/// Pull the reasoning text (and optional signature) out of a streaming delta,
/// preferring the dedicated `reasoning_content` field and falling back to the
/// `reasoning` field (string or `{text, signature}` object).
fn reasoning_delta_from_chunk(delta: &ChatCompletionDelta) -> Option<ReasoningDelta> {
    if let Some(text) = delta
        .reasoning_content
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        return Some(ReasoningDelta {
            text: text.to_string(),
            signature: None,
        });
    }
    match delta.reasoning.as_ref()? {
        Value::String(text) if !text.is_empty() => Some(ReasoningDelta {
            text: text.clone(),
            signature: None,
        }),
        Value::Object(map) => {
            let text = map
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let signature = map
                .get("signature")
                .and_then(Value::as_str)
                .filter(|signature| !signature.is_empty())
                .map(str::to_string);
            if text.is_empty() && signature.is_none() {
                None
            } else {
                Some(ReasoningDelta { text, signature })
            }
        }
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionToolCallDelta {
    // Some compat providers omit `index` on tool-call deltas. Default it so the
    // whole chunk still deserializes (and the delta is not silently dropped)
    // rather than failing the entire chunk parse.
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<ChatCompletionFunctionDelta>,
    // Gemini's OpenAI-compat endpoint attaches the encrypted thought signature
    // for a tool call here as `{"google": {"thought_signature": "..."}}`.
    #[serde(default)]
    extra_content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionUsage {
    #[serde(default, alias = "prompt_tokens")]
    prompt_tokens: i64,
    #[serde(default, alias = "completion_tokens")]
    completion_tokens: i64,
    #[serde(default, alias = "prompt_tokens_details")]
    prompt_tokens_details: Option<ChatCompletionPromptTokensDetails>,
    #[serde(default, alias = "completion_tokens_details")]
    completion_tokens_details: Option<ChatCompletionCompletionTokensDetails>,
    #[serde(default)]
    total_tokens: i64,
}

impl From<ChatCompletionUsage> for TokenUsage {
    fn from(value: ChatCompletionUsage) -> Self {
        Self {
            input_tokens: value.prompt_tokens,
            cached_input_tokens: value
                .prompt_tokens_details
                .map(|details| details.cached_tokens)
                .unwrap_or(0),
            output_tokens: value.completion_tokens,
            reasoning_output_tokens: value
                .completion_tokens_details
                .map(|details| details.reasoning_tokens)
                .unwrap_or(0),
            total_tokens: value.total_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionPromptTokensDetails {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: i64,
}

#[derive(Debug, Default)]
struct ChatStreamState {
    response_id: Option<String>,
    model: Option<String>,
    usage: Option<TokenUsage>,
    assistant_role: Option<String>,
    assistant_text: String,
    assistant_item_started: bool,
    reasoning_text: String,
    reasoning_signature: Option<String>,
    reasoning_item_started: bool,
    reasoning_item_done: bool,
    tool_calls: BTreeMap<usize, AccumulatedToolCall>,
    completed: bool,
}

#[derive(Debug, Default)]
struct AccumulatedToolCall {
    call_id: Option<String>,
    encoded_name: String,
    arguments: String,
    thought_signature: Option<String>,
}

async fn process_chat_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_name_map: HashMap<String, ChatToolName>,
) {
    let mut stream = stream.eventsource();
    let mut state = ChatStreamState::default();

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("chat completions SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                if state.completed {
                    emit_chat_completion_finished(&mut state, &tx_event, &tool_name_map).await;
                    return;
                }
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before chat completion finished".into(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("chat completions SSE event: {}", &sse.data);
        if sse.data.trim() == "[DONE]" {
            emit_chat_completion_finished(&mut state, &tx_event, &tool_name_map).await;
            return;
        }
        if let Some(err) = chat_completion_error_from_sse(&sse.data) {
            let _ = tx_event.send(Err(err)).await;
            return;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_str(&sse.data) {
            Ok(chunk) => chunk,
            Err(err) => {
                debug!(
                    error = %err,
                    payload = %sse.data,
                    "failed to parse chat completions SSE chunk; skipping",
                );
                continue;
            }
        };
        if let Some(id) = chunk.id {
            state.response_id = Some(id);
        }
        if let Some(model) = chunk.model
            && state.model.as_deref() != Some(model.as_str())
        {
            if tx_event
                .send(Ok(ResponseEvent::ServerModel(model.clone())))
                .await
                .is_err()
            {
                return;
            }
            state.model = Some(model);
        }
        if let Some(usage) = chunk.usage {
            state.usage = Some(usage.into());
        }

        for mut choice in chunk.choices {
            if let Some(role) = choice.delta.role.take() {
                state.assistant_role = Some(role);
            }
            // Reasoning is streamed (and buffered into a Reasoning item) before
            // the assistant message content, mirroring the Responses path.
            if !state.reasoning_item_done
                && let Some(reasoning) = reasoning_delta_from_chunk(&choice.delta)
            {
                if !state.reasoning_item_started {
                    state.reasoning_item_started = true;
                    let item = ResponseItem::Reasoning {
                        id: state.response_id.clone().unwrap_or_default(),
                        summary: Vec::new(),
                        content: None,
                        encrypted_content: None,
                    };
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemAdded(item)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if let Some(signature) = reasoning.signature {
                    state.reasoning_signature = Some(signature);
                }
                if !reasoning.text.is_empty() {
                    state.reasoning_text.push_str(&reasoning.text);
                    if tx_event
                        .send(Ok(ResponseEvent::ReasoningContentDelta {
                            delta: reasoning.text,
                            content_index: 0,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if let Some(content) = choice.delta.content {
                // Finalize any in-progress reasoning item before opening the
                // assistant message, so history ordering matches the Responses
                // path (reasoning item precedes the message item).
                if !finish_reasoning_item(&mut state, &tx_event).await {
                    return;
                }
                if !state.assistant_item_started {
                    let item = ResponseItem::Message {
                        id: state.response_id.clone(),
                        role: state
                            .assistant_role
                            .clone()
                            .unwrap_or_else(|| "assistant".to_string()),
                        content: Vec::new(),
                        phase: Some(MessagePhase::FinalAnswer),
                    };
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemAdded(item)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    state.assistant_item_started = true;
                }
                state.assistant_text.push_str(&content);
                if tx_event
                    .send(Ok(ResponseEvent::OutputTextDelta(content)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if let Some(tool_calls) = choice.delta.tool_calls {
                accumulate_tool_call_deltas(&mut state, tool_calls);
            }
            if choice.finish_reason.is_some() {
                state.completed = true;
            }
        }
    }
}

fn chat_completion_error_from_sse(data: &str) -> Option<ApiError> {
    let error = serde_json::from_str::<ChatCompletionErrorEvent>(data)
        .ok()?
        .error;
    Some(match error.error_type.as_deref() {
        Some("invalid_request_error" | "BadRequestError") => ApiError::InvalidRequest {
            message: error.message,
        },
        _ => ApiError::Stream(format!("chat completions error: {}", error.message)),
    })
}

#[derive(Debug, Deserialize)]
struct ChatCompletionErrorEvent {
    error: ChatCompletionErrorBody,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionErrorBody {
    message: String,
    #[serde(rename = "type")]
    error_type: Option<String>,
}

fn accumulate_tool_call_deltas(
    state: &mut ChatStreamState,
    tool_calls: Vec<ChatCompletionToolCallDelta>,
) {
    for delta in tool_calls {
        let tool_call = state.tool_calls.entry(delta.index).or_default();
        if let Some(call_id) = delta.id {
            tool_call.call_id = Some(call_id);
        }
        if let Some(extra_content) = delta.extra_content.as_ref()
            && let Some(signature) = thought_signature_from_extra_content(extra_content)
        {
            tool_call.thought_signature = Some(signature);
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                tool_call.encoded_name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                tool_call.arguments.push_str(&arguments);
            }
        }
    }
}

/// Emit the accumulated reasoning as a completed `ResponseItem::Reasoning` (so
/// it is recorded in history and can be replayed on later turns). Idempotent:
/// once the reasoning item is finalized it is never emitted again. Returns
/// `false` if the event channel has closed and the caller should stop.
async fn finish_reasoning_item(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> bool {
    if !state.reasoning_item_started || state.reasoning_item_done {
        return true;
    }
    state.reasoning_item_done = true;
    let item = ResponseItem::Reasoning {
        id: state.response_id.clone().unwrap_or_default(),
        summary: Vec::new(),
        content: Some(vec![ReasoningItemContent::ReasoningText {
            text: std::mem::take(&mut state.reasoning_text),
        }]),
        encrypted_content: state.reasoning_signature.clone(),
    };
    tx_event
        .send(Ok(ResponseEvent::OutputItemDone(item)))
        .await
        .is_ok()
}

async fn emit_chat_completion_finished(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    tool_name_map: &HashMap<String, ChatToolName>,
) {
    // Reasoning-only or reasoning-then-tool-call responses never hit the content
    // branch, so finalize any outstanding reasoning item first (ahead of the
    // assistant message and tool calls).
    if !finish_reasoning_item(state, tx_event).await {
        return;
    }
    if state.assistant_item_started {
        let item = ResponseItem::Message {
            id: state.response_id.clone(),
            role: state
                .assistant_role
                .clone()
                .unwrap_or_else(|| "assistant".to_string()),
            content: vec![ContentItem::OutputText {
                text: std::mem::take(&mut state.assistant_text),
            }],
            phase: Some(MessagePhase::FinalAnswer),
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return;
        }
    }

    let tool_calls = std::mem::take(&mut state.tool_calls);
    for (index, tool_call) in tool_calls {
        if tool_call.encoded_name.is_empty() {
            continue;
        }
        let mapped_name = tool_name_map
            .get(&tool_call.encoded_name)
            .cloned()
            .unwrap_or(ChatToolName {
                namespace: None,
                name: tool_call.encoded_name.clone(),
            });
        // Include the tool-call index so id-less parallel calls to the same
        // tool get distinct synthetic ids instead of colliding.
        let call_id = tool_call.call_id.unwrap_or_else(|| {
            format!(
                "{}_{}_{index}",
                state.response_id.as_deref().unwrap_or("chatcmpl"),
                tool_call.encoded_name
            )
        });
        // Record Gemini's thought signature so it can be echoed back when this
        // tool call is replayed on the next tool-turn request.
        if let Some(signature) = tool_call.thought_signature {
            store_thought_signature(&call_id, signature);
        }
        let item = ResponseItem::FunctionCall {
            id: None,
            name: mapped_name.name,
            namespace: mapped_name.namespace,
            arguments: chat_tool_arguments(&tool_call.arguments).to_string(),
            call_id,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return;
        }
    }

    let response_id = state
        .response_id
        .clone()
        .unwrap_or_else(|| "chatcmpl".to_string());
    let _ = tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id,
            token_usage: state.usage.take(),
            end_turn: None,
        }))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Reasoning;
    use crate::common::TextFormat;
    use crate::common::TextFormatType;
    use codex_client::TransportError;
    use futures::TryStreamExt;
    use pretty_assertions::assert_eq;
    use tokio_util::io::ReaderStream;

    async fn collect_chat_events(body: &str) -> Vec<Result<ResponseEvent, ApiError>> {
        let stream = ReaderStream::new(std::io::Cursor::new(body.to_string()))
            .map_err(|err| TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(8);
        tokio::spawn(process_chat_sse(
            Box::pin(stream),
            tx,
            Duration::from_secs(1),
            /*telemetry*/ None,
            HashMap::new(),
        ));

        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        events
    }

    #[test]
    fn chat_request_maps_text_messages_and_tools() {
        let request = ResponsesApiRequest {
            model: "gpt-oss-120b".to_string(),
            instructions: "Be terse".to_string(),
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hello".to_string(),
                }],
                phase: None,
            }],
            tools: vec![serde_json::json!({
                "type": "function",
                "name": "exec_command",
                "description": "Run a command",
                "parameters": {"type": "object"}
            })],
            tool_choice: "auto".to_string(),
            parallel_tool_calls: true,
            reasoning: Some(Reasoning {
                effort: Some(ReasoningEffort::Minimal),
                summary: None,
                context: None,
            }),
            store: false,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: Some("ignored".to_string()),
            text: Some(TextControls {
                verbosity: None,
                format: Some(TextFormat {
                    r#type: TextFormatType::JsonSchema,
                    strict: true,
                    schema: serde_json::json!({"type": "object"}),
                    name: "answer".to_string(),
                }),
            }),
            client_metadata: None,
        };

        let parts = chat_request_from_responses(request, false).expect("request should map");

        assert_eq!(
            parts.body["messages"],
            serde_json::json!([
                {"role": "system", "content": "Be terse"},
                {"role": "user", "content": "hello"}
            ])
        );
        assert_eq!(parts.body["reasoning_effort"], "low");
        assert!(parts.body.get("prompt_cache_key").is_none());
        assert_eq!(
            parts.body["tools"],
            serde_json::json!([{
                "type": "function",
                "function": {
                    "name": "exec_command",
                    "description": "Run a command",
                    "parameters": {"type": "object"}
                }
            }])
        );
        assert_eq!(
            parts.body["response_format"],
            serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "answer",
                    "strict": true,
                    "schema": {"type": "object"}
                }
            })
        );
    }

    fn reasoning(effort: ReasoningEffort) -> Reasoning {
        Reasoning {
            effort: Some(effort),
            summary: None,
            context: None,
        }
    }

    #[test]
    fn chat_reasoning_effort_keeps_none_by_default() {
        // OpenAI/OpenRouter tolerate `reasoning_effort: "none"`.
        assert_eq!(
            chat_reasoning_effort(Some(&reasoning(ReasoningEffort::None)), false),
            Some("none".to_string())
        );
    }

    #[test]
    fn chat_reasoning_effort_maps_none_to_low_for_restricted_providers() {
        // Cerebras / NVIDIA vLLM reject "none"; it must be rewritten to "low"
        // so tool calling keeps working.
        assert_eq!(
            chat_reasoning_effort(Some(&reasoning(ReasoningEffort::None)), true),
            Some("low".to_string())
        );
    }

    #[test]
    fn chat_reasoning_effort_gating_only_affects_none() {
        for effort in [
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ] {
            assert_eq!(
                chat_reasoning_effort(Some(&reasoning(effort.clone())), false),
                chat_reasoning_effort(Some(&reasoning(effort)), true),
            );
        }
    }

    #[test]
    fn chat_request_rewrites_none_reasoning_effort_when_gated() {
        let request = ResponsesApiRequest {
            model: "gpt-oss-120b".to_string(),
            instructions: "Be terse".to_string(),
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hello".to_string(),
                }],
                phase: None,
            }],
            tools: Vec::new(),
            tool_choice: "auto".to_string(),
            parallel_tool_calls: true,
            reasoning: Some(reasoning(ReasoningEffort::None)),
            store: false,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let gated = chat_request_from_responses(request.clone(), true).expect("request should map");
        assert_eq!(gated.body["reasoning_effort"], "low");

        let ungated = chat_request_from_responses(request, false).expect("request should map");
        assert_eq!(ungated.body["reasoning_effort"], "none");
    }

    #[test]
    fn chat_request_preserves_provider_native_web_search_tools() {
        let tools = chat_tools_from_responses(
            &[
                serde_json::json!({
                    "type": "web_search_20260209",
                    "name": "web_search",
                    "allowed_domains": ["example.com"],
                }),
                serde_json::json!({
                    "type": "web_search",
                    "web_search": {
                        "enable": true,
                        "search_engine": "search-prime",
                        "search_result": true,
                    },
                }),
                serde_json::json!({
                    "type": "web_search",
                    "external_web_access": true,
                }),
            ],
            &mut HashMap::new(),
        )
        .expect("tools should map");

        assert_eq!(
            tools,
            vec![
                serde_json::json!({
                    "type": "web_search_20260209",
                    "name": "web_search",
                    "allowed_domains": ["example.com"],
                }),
                serde_json::json!({
                    "type": "web_search",
                    "web_search": {
                        "enable": true,
                        "search_engine": "search-prime",
                        "search_result": true,
                    },
                }),
            ]
        );
    }

    #[tokio::test]
    async fn chat_stream_maps_error_payload() {
        let events = collect_chat_events(
            r#"data: {"error":{"message":"bad request","type":"invalid_request_error"}}

"#,
        )
        .await;

        assert!(matches!(
            events.as_slice(),
            [Err(ApiError::InvalidRequest { message })] if message == "bad request"
        ));
    }

    #[tokio::test]
    async fn chat_stream_completes_when_stream_closes_after_finish_reason() {
        let events = collect_chat_events(
            r#"data: {"id":"chatcmpl-1","model":"gpt-test","choices":[{"delta":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}

"#,
        )
        .await;

        assert!(events.iter().all(Result::is_ok));
        let events = events.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        assert!(matches!(
            events.last(),
            Some(ResponseEvent::Completed { .. })
        ));
    }

    #[test]
    fn chat_request_flattens_namespace_tools() {
        let mut map = HashMap::new();
        let tools = chat_tools_from_responses(
            &[serde_json::json!({
                "type": "namespace",
                "name": "mcp",
                "tools": [{
                    "type": "function",
                    "name": "read",
                    "description": "Read",
                    "parameters": {"type": "object"}
                }]
            })],
            &mut map,
        )
        .expect("tools should map");

        assert_eq!(tools[0]["function"]["name"], "mcp__read");
        assert_eq!(
            map.get("mcp__read"),
            Some(&ChatToolName {
                namespace: Some("mcp".to_string()),
                name: "read".to_string(),
            })
        );
    }

    #[test]
    fn chat_request_rejects_image_input() {
        let err = text_from_content_items(&[ContentItem::InputImage {
            image_url: "data:image/png;base64,abc".to_string(),
            detail: None,
        }])
        .expect_err("image input should be rejected");

        assert!(matches!(err, ApiError::InvalidRequest { .. }));
    }

    #[test]
    fn chat_request_omits_custom_tool_history() {
        let messages = chat_messages_from_responses(
            "",
            &[
                ResponseItem::CustomToolCall {
                    id: Some("custom_1".to_string()),
                    status: Some("completed".to_string()),
                    call_id: "call_custom".to_string(),
                    name: "apply_patch".to_string(),
                    namespace: None,
                    input: "*** Begin Patch\n*** End Patch\n".to_string(),
                },
                ResponseItem::CustomToolCallOutput {
                    call_id: "call_custom".to_string(),
                    name: Some("apply_patch".to_string()),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "continue".to_string(),
                    }],
                    phase: None,
                },
            ],
        )
        .expect("messages should map");

        assert_eq!(
            messages,
            vec![serde_json::json!({"role": "user", "content": "continue"})]
        );
    }

    #[test]
    fn chat_request_maps_developer_messages_to_system_messages() {
        let messages = chat_messages_from_responses(
            "",
            &[ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::InputText {
                    text: "Follow policy".to_string(),
                }],
                phase: None,
            }],
        )
        .expect("messages should map");

        assert_eq!(
            messages,
            vec![serde_json::json!({"role": "system", "content": "Follow policy"})]
        );
    }

    fn function_call(name: &str, args: &str, call_id: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: args.to_string(),
            call_id: call_id.to_string(),
        }
    }

    fn emitted_function_calls(
        events: Vec<Result<ResponseEvent, ApiError>>,
    ) -> Vec<(String, String)> {
        events
            .into_iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                    call_id,
                    arguments,
                    ..
                })) => Some((call_id, arguments)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn chat_request_coalesces_parallel_tool_calls_into_one_assistant_message() {
        let messages = chat_messages_from_responses(
            "",
            &[
                function_call("search", "{\"q\":\"a\"}", "call_a"),
                function_call("search", "{\"q\":\"b\"}", "call_b"),
                ResponseItem::FunctionCallOutput {
                    call_id: "call_a".to_string(),
                    output: FunctionCallOutputPayload::from_text("ra".to_string()),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_b".to_string(),
                    output: FunctionCallOutputPayload::from_text("rb".to_string()),
                },
            ],
        )
        .expect("messages should map");

        // Exactly one assistant message, carrying both tool calls, followed by
        // the two tool results.
        assert_eq!(
            messages,
            vec![
                serde_json::json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_a",
                            "type": "function",
                            "function": {"name": "search", "arguments": "{\"q\":\"a\"}"}
                        },
                        {
                            "id": "call_b",
                            "type": "function",
                            "function": {"name": "search", "arguments": "{\"q\":\"b\"}"}
                        }
                    ]
                }),
                serde_json::json!({"role": "tool", "tool_call_id": "call_a", "content": "ra"}),
                serde_json::json!({"role": "tool", "tool_call_id": "call_b", "content": "rb"}),
            ]
        );
    }

    #[test]
    fn chat_request_normalizes_empty_tool_arguments() {
        let messages = chat_messages_from_responses("", &[function_call("noop", "", "call_1")])
            .expect("messages should map");

        assert_eq!(messages[0]["tool_calls"][0]["function"]["arguments"], "{}");
    }

    #[test]
    fn encode_chat_tool_name_truncates_and_hashes_long_names() {
        let long_name = "a".repeat(200);
        let encoded = encode_chat_tool_name(Some("namespace"), &long_name)
            .expect("long names should be truncated, not rejected");

        assert!(encoded.chars().count() <= 64);
        // Deterministic: same input encodes identically.
        assert_eq!(
            encoded,
            encode_chat_tool_name(Some("namespace"), &long_name).unwrap()
        );
        // Distinct long names produce distinct encodings.
        let other = encode_chat_tool_name(Some("namespace"), &"b".repeat(200)).unwrap();
        assert_ne!(encoded, other);
    }

    #[tokio::test]
    async fn chat_stream_tool_call_delta_without_index_is_not_dropped() {
        // The tool-call delta omits the `index` field entirely; it must still
        // parse and surface as a function call.
        let events = collect_chat_events(
            "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-test\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"id\":\"call_a\",\"function\":{\"name\":\"exec\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        )
        .await;

        let calls = emitted_function_calls(events);
        // Present (not dropped) and empty args normalized to "{}".
        assert_eq!(calls, vec![("call_a".to_string(), "{}".to_string())]);
    }

    #[tokio::test]
    async fn chat_stream_synthesizes_unique_ids_for_idless_parallel_calls() {
        // Two parallel calls to the same tool, neither carrying an id.
        let events = collect_chat_events(
            "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-test\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"exec\",\"arguments\":\"{}\"}},{\"index\":1,\"function\":{\"name\":\"exec\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        )
        .await;

        let ids: Vec<String> = emitted_function_calls(events)
            .into_iter()
            .map(|(call_id, _)| call_id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "synthetic call ids must be unique");
    }

    #[test]
    fn accumulates_tool_call_deltas() {
        let mut state = ChatStreamState::default();
        accumulate_tool_call_deltas(
            &mut state,
            vec![
                ChatCompletionToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    function: Some(ChatCompletionFunctionDelta {
                        name: Some("exec_".to_string()),
                        arguments: Some("{\"cmd\"".to_string()),
                    }),
                    extra_content: None,
                },
                ChatCompletionToolCallDelta {
                    index: 0,
                    id: None,
                    function: Some(ChatCompletionFunctionDelta {
                        name: Some("command".to_string()),
                        arguments: Some(":\"date\"}".to_string()),
                    }),
                    extra_content: None,
                },
            ],
        );

        let call = state.tool_calls.get(&0).expect("tool call accumulated");
        assert_eq!(call.call_id.as_deref(), Some("call_1"));
        assert_eq!(call.encoded_name, "exec_command");
        assert_eq!(call.arguments, "{\"cmd\":\"date\"}");
    }

    #[test]
    fn accumulate_captures_gemini_thought_signature_from_extra_content() {
        let mut state = ChatStreamState::default();
        accumulate_tool_call_deltas(
            &mut state,
            vec![ChatCompletionToolCallDelta {
                index: 0,
                id: Some("call_g".to_string()),
                function: Some(ChatCompletionFunctionDelta {
                    name: Some("search".to_string()),
                    arguments: Some("{}".to_string()),
                }),
                extra_content: Some(serde_json::json!({
                    "google": { "thought_signature": "SIG_ABC" }
                })),
            }],
        );

        let call = state.tool_calls.get(&0).expect("tool call accumulated");
        assert_eq!(call.thought_signature.as_deref(), Some("SIG_ABC"));
    }

    #[test]
    fn chat_request_roundtrips_gemini_thought_signature() {
        // A signature recorded for a call id must be echoed back on that tool
        // call under `extra_content.google.thought_signature` when the call is
        // replayed on a subsequent tool-turn request.
        let call_id = "call_roundtrip_signature_unit";
        store_thought_signature(call_id, "SIG_ROUNDTRIP".to_string());

        let messages =
            chat_messages_from_responses("", &[function_call("search", "{\"q\":\"a\"}", call_id)])
                .expect("messages should map");

        assert_eq!(
            messages[0]["tool_calls"][0]["extra_content"],
            serde_json::json!({ "google": { "thought_signature": "SIG_ROUNDTRIP" } })
        );
    }

    #[test]
    fn chat_request_omits_extra_content_without_signature() {
        // Tool calls with no recorded signature (e.g. non-Gemini providers)
        // must not carry an `extra_content` field.
        let messages =
            chat_messages_from_responses("", &[function_call("search", "{}", "call_no_sig_unit")])
                .expect("messages should map");

        assert!(messages[0]["tool_calls"][0].get("extra_content").is_none());
    }

    fn reasoning_item(text: &str, signature: Option<&str>) -> ResponseItem {
        ResponseItem::Reasoning {
            id: String::new(),
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: text.to_string(),
            }]),
            encrypted_content: signature.map(str::to_string),
        }
    }

    fn emitted_reasoning_deltas(events: &[Result<ResponseEvent, ApiError>]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::ReasoningContentDelta { delta, .. }) => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    fn emitted_done_reasoning(events: &[Result<ResponseEvent, ApiError>]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning { content, .. })) => {
                    Some(reasoning_text_from_content(content))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reasoning_delta_prefers_reasoning_content_field() {
        let delta = ChatCompletionDelta {
            reasoning_content: Some("thinking".to_string()),
            ..Default::default()
        };
        assert_eq!(
            reasoning_delta_from_chunk(&delta),
            Some(ReasoningDelta {
                text: "thinking".to_string(),
                signature: None,
            })
        );
    }

    #[test]
    fn reasoning_delta_reads_string_and_object_reasoning() {
        let string_delta = ChatCompletionDelta {
            reasoning: Some(serde_json::json!("plain cot")),
            ..Default::default()
        };
        assert_eq!(
            reasoning_delta_from_chunk(&string_delta),
            Some(ReasoningDelta {
                text: "plain cot".to_string(),
                signature: None,
            })
        );

        let object_delta = ChatCompletionDelta {
            reasoning: Some(serde_json::json!({"text": "signed cot", "signature": "SIG"})),
            ..Default::default()
        };
        assert_eq!(
            reasoning_delta_from_chunk(&object_delta),
            Some(ReasoningDelta {
                text: "signed cot".to_string(),
                signature: Some("SIG".to_string()),
            })
        );

        // No reasoning present -> None (and empty strings are ignored).
        assert_eq!(
            reasoning_delta_from_chunk(&ChatCompletionDelta::default()),
            None
        );
    }

    #[tokio::test]
    async fn chat_stream_emits_reasoning_content_as_event_and_item() {
        // A DeepSeek-style delta carrying `reasoning_content` must surface a
        // ReasoningContentDelta event (live display) and a completed Reasoning
        // item (history), ahead of the assistant message.
        let events = collect_chat_events(
            "data: {\"id\":\"chatcmpl-r\",\"model\":\"deepseek-v4\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"step 1 \"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-r\",\"choices\":[{\"delta\":{\"reasoning_content\":\"step 2\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-r\",\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\n",
        )
        .await;

        assert!(events.iter().all(Result::is_ok), "events: {events:?}");
        assert_eq!(emitted_reasoning_deltas(&events), vec!["step 1 ", "step 2"]);
        assert_eq!(emitted_done_reasoning(&events), vec!["step 1 step 2"]);

        // The reasoning item must be finalized before the assistant message.
        let done_positions: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })) => {
                    Some("reasoning")
                }
                Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { .. })) => Some("message"),
                _ => None,
            })
            .collect();
        assert_eq!(done_positions, vec!["reasoning", "message"]);
    }

    #[test]
    fn chat_request_replays_signed_reasoning_onto_assistant_message() {
        // Signed reasoning in history is replayed (not dropped) onto the
        // following assistant message.
        let messages = chat_messages_from_responses(
            "",
            &[
                reasoning_item("deep thought", Some("SIG_ABC")),
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "the answer".to_string(),
                    }],
                    phase: None,
                },
            ],
        )
        .expect("messages should map");

        assert_eq!(
            messages,
            vec![serde_json::json!({
                "role": "assistant",
                "content": "the answer",
                "reasoning_content": "deep thought",
                "reasoning_signature": "SIG_ABC",
            })]
        );
    }

    #[test]
    fn chat_request_replays_signed_reasoning_onto_tool_call_message() {
        // Signed reasoning ahead of tool calls attaches to the coalesced
        // assistant tool_calls message.
        let messages = chat_messages_from_responses(
            "",
            &[
                reasoning_item("plan the call", Some("SIG_TOOL")),
                function_call("search", "{}", "call_x"),
            ],
        )
        .expect("messages should map");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["reasoning_content"], "plan the call");
        assert_eq!(messages[0]["reasoning_signature"], "SIG_TOOL");
        assert_eq!(messages[0]["tool_calls"][0]["id"], "call_x");
    }

    #[test]
    fn chat_request_drops_unsigned_reasoning_for_provider_safety() {
        // Unsigned reasoning (e.g. DeepSeek `reasoning_content`) must NOT be
        // echoed back: such providers reject `reasoning_content` on input.
        let messages = chat_messages_from_responses(
            "",
            &[
                reasoning_item("private cot", None),
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "the answer".to_string(),
                    }],
                    phase: None,
                },
            ],
        )
        .expect("messages should map");

        assert_eq!(
            messages,
            vec![serde_json::json!({"role": "assistant", "content": "the answer"})]
        );
    }

    #[tokio::test]
    async fn chat_stream_captures_and_roundtrips_thought_signature() {
        // End-to-end: a streamed Gemini tool call carrying a thought signature
        // is recorded, then re-attached when the resulting function call is
        // replayed on the next request.
        let call_id = "call_stream_signature_unit";
        let body = format!(
            "data: {{\"id\":\"chatcmpl-sig\",\"model\":\"gemini-3.5-flash\",\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"tool_calls\":[{{\"index\":0,\"id\":\"{call_id}\",\"function\":{{\"name\":\"search\",\"arguments\":\"{{}}\"}},\"extra_content\":{{\"google\":{{\"thought_signature\":\"SIG_STREAM\"}}}}}}]}},\"finish_reason\":null}}]}}\n\ndata: {{\"id\":\"chatcmpl-sig\",\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
        );
        let events = collect_chat_events(&body).await;

        let calls = emitted_function_calls(events);
        assert_eq!(calls, vec![(call_id.to_string(), "{}".to_string())]);
        assert_eq!(
            thought_signature_for(call_id).as_deref(),
            Some("SIG_STREAM")
        );

        let messages = chat_messages_from_responses("", &[function_call("search", "{}", call_id)])
            .expect("messages should map");
        assert_eq!(
            messages[0]["tool_calls"][0]["extra_content"]["google"]["thought_signature"],
            "SIG_STREAM"
        );
    }
}
