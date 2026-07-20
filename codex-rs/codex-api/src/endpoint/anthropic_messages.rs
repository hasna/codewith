//! Native Anthropic Messages API runtime adapter.
//!
//! This endpoint translates Codewith's provider-agnostic [`ResponsesApiRequest`]
//! (the same request shape the Responses and Chat Completions adapters consume)
//! into Anthropic's native `/v1/messages` request, streams the resulting
//! server-sent events, and converts Anthropic's message/content-block event
//! shapes back into Codewith [`ResponseEvent`]s.
//!
//! Unlike the OpenAI-compatible Chat Completions surface Anthropic also exposes,
//! the native Messages API preserves Anthropic-specific behavior end to end:
//!   * extended thinking (reasoning) blocks with `signature` continuity across
//!     tool turns,
//!   * `tool_use` / `tool_result` content blocks for function calling,
//!   * incremental `input_json_delta` tool argument streaming, and
//!   * Anthropic-native token accounting (including prompt cache reads).
//!
//! Auth (`x-api-key`) is applied by the caller's [`AuthProvider`]; the required
//! `anthropic-version` header is attached here so the adapter is self-contained.
//!
//! [`AuthProvider`]: crate::auth::AuthProvider

use crate::auth::SharedAuthProvider;
use crate::common::Reasoning;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::common::ResponsesApiRequest;
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
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::instrument;
use tracing::trace;

/// Header carrying the Anthropic API version. Required on every request.
const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
/// Anthropic API version this adapter targets. The Messages API and its
/// streaming event shapes are stable under this version string.
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_ID_HEADER: &str = "request-id";

/// Default `max_tokens` for a turn. Anthropic requires this field, but the
/// Responses request shape has no equivalent. This default is safe for the
/// large-output Claude models Codewith defaults to; per-model output caps are
/// tracked as a follow-up so lower-limit models (e.g. Haiku) can lower it.
const DEFAULT_MAX_TOKENS: i64 = 32_000;

/// Anthropic tool names must match `^[a-zA-Z0-9_-]{1,128}$`.
const ANTHROPIC_TOOL_NAME_MAX_LEN: usize = 128;

/// Minimum extended-thinking budget accepted by Anthropic.
const MIN_THINKING_BUDGET: i64 = 1_024;

pub struct AnthropicMessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport> AnthropicMessagesClient<T> {
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
        name = "anthropic_messages.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_messages_http",
            http.method = "POST",
            api.path = "messages"
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
            turn_state: _turn_state,
        } = options;

        let AnthropicRequestParts {
            body,
            tool_name_map,
        } = anthropic_request_from_responses(request)?;

        let mut headers = extra_headers;
        insert_header(&mut headers, ANTHROPIC_VERSION_HEADER, ANTHROPIC_VERSION);
        if let Some(ref thread_id) = thread_id {
            insert_header(&mut headers, "x-client-request-id", thread_id);
        }
        headers.extend(build_session_headers(session_id, thread_id));
        if let Some(subagent) = subagent_header(&session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }

        self.stream(body, headers, compression, tool_name_map).await
    }

    fn path() -> &'static str {
        "messages"
    }

    #[instrument(
        name = "anthropic_messages.stream",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_messages_http",
            http.method = "POST",
            api.path = "messages"
        )
    )]
    async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        compression: Compression,
        tool_name_map: HashMap<String, AnthropicToolName>,
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

        Ok(spawn_anthropic_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            tool_name_map,
        ))
    }
}

// ---------------------------------------------------------------------------
// Request translation: ResponsesApiRequest -> Anthropic Messages request body
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AnthropicRequestParts {
    body: Value,
    tool_name_map: HashMap<String, AnthropicToolName>,
}

/// Records how an encoded Anthropic tool name maps back to Codewith's
/// `(namespace, name)` pair, mirroring the Chat Completions adapter so
/// namespaced/overlong tool names round-trip losslessly.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AnthropicToolName {
    namespace: Option<String>,
    name: String,
}

fn anthropic_request_from_responses(
    request: ResponsesApiRequest,
) -> Result<AnthropicRequestParts, ApiError> {
    let mut tool_name_map = HashMap::new();
    let tools = anthropic_tools_from_responses(&request.tools, &mut tool_name_map)?;

    let mut system = request.instructions.clone();
    let messages = anthropic_messages_from_responses(&request.input, &mut system)?;

    let max_tokens = DEFAULT_MAX_TOKENS;
    let thinking = anthropic_thinking(request.reasoning.as_ref(), max_tokens);

    let mut body = serde_json::json!({
        "model": request.model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": request.stream,
    });

    if !system.trim().is_empty() {
        body["system"] = Value::String(system);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        // When extended thinking is enabled Anthropic rejects forced tool use
        // (`any`/`tool`), so downgrade to `auto` in that case.
        let force_disabled = thinking.is_some();
        body["tool_choice"] = anthropic_tool_choice(
            &request.tool_choice,
            request.parallel_tool_calls,
            force_disabled,
        );
    }
    if let Some(thinking) = thinking {
        body["thinking"] = thinking;
    }

    Ok(AnthropicRequestParts {
        body,
        tool_name_map,
    })
}

/// Map Codewith reasoning effort onto an Anthropic extended-thinking config.
///
/// Returns `None` (thinking omitted) when reasoning is absent or explicitly
/// disabled. The budget is clamped so it always stays within
/// `[MIN_THINKING_BUDGET, max_tokens)`, which Anthropic requires.
fn anthropic_thinking(reasoning: Option<&Reasoning>, max_tokens: i64) -> Option<Value> {
    let effort = reasoning.and_then(|reasoning| reasoning.effort.as_ref())?;
    let budget = match effort {
        ReasoningEffort::None => return None,
        ReasoningEffort::Minimal | ReasoningEffort::Low => 4_000,
        ReasoningEffort::Medium => 8_000,
        ReasoningEffort::High => 16_000,
        ReasoningEffort::XHigh => 24_000,
        ReasoningEffort::Custom(_) => 8_000,
    };
    let ceiling = max_tokens.saturating_sub(1);
    if ceiling < MIN_THINKING_BUDGET {
        return None;
    }
    let budget = budget.clamp(MIN_THINKING_BUDGET, ceiling);
    Some(serde_json::json!({
        "type": "enabled",
        "budget_tokens": budget,
    }))
}

fn anthropic_tool_choice(
    tool_choice: &str,
    parallel_tool_calls: bool,
    force_disabled: bool,
) -> Value {
    let mut choice = match tool_choice {
        "none" => serde_json::json!({ "type": "none" }),
        "required" | "any" if !force_disabled => serde_json::json!({ "type": "any" }),
        "auto" | "required" | "any" => serde_json::json!({ "type": "auto" }),
        // A specific function name. Forced tool choice is unsupported alongside
        // thinking, so fall back to `auto` when thinking is enabled.
        name if !force_disabled => serde_json::json!({ "type": "tool", "name": name }),
        _ => serde_json::json!({ "type": "auto" }),
    };
    // `disable_parallel_tool_use` is only valid on `auto`/`any`/`tool`.
    if !parallel_tool_calls
        && let Some(object) = choice.as_object_mut()
        && object.get("type").and_then(Value::as_str) != Some("none")
    {
        object.insert("disable_parallel_tool_use".to_string(), Value::Bool(true));
    }
    choice
}

fn anthropic_tools_from_responses(
    tools: &[Value],
    tool_name_map: &mut HashMap<String, AnthropicToolName>,
) -> Result<Vec<Value>, ApiError> {
    let mut anthropic_tools = Vec::new();
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => {
                if let Some(tool) =
                    anthropic_function_tool(tool, /*namespace*/ None, tool_name_map)?
                {
                    anthropic_tools.push(tool);
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
                    if let Some(tool) =
                        anthropic_function_tool(namespace_tool, Some(namespace), tool_name_map)?
                    {
                        anthropic_tools.push(tool);
                    }
                }
            }
            // Anthropic-native server tools (e.g. `web_search_20250305`) are
            // already in the correct shape, so pass them through untouched.
            Some(kind) if kind.starts_with("web_search_") => {
                anthropic_tools.push(tool.clone());
            }
            // Tool types Codewith cannot faithfully translate to Anthropic yet
            // (generic web_search, custom, tool_search, image_generation) are
            // dropped rather than sent in an invalid shape.
            Some(_) | None => {}
        }
    }
    Ok(anthropic_tools)
}

fn anthropic_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    tool_name_map: &mut HashMap<String, AnthropicToolName>,
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
    let encoded_name = encode_tool_name(namespace, name);
    tool_name_map.insert(
        encoded_name.clone(),
        AnthropicToolName {
            namespace: namespace.map(str::to_string),
            name: name.to_string(),
        },
    );

    let input_schema = tool
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));

    Ok(Some(serde_json::json!({
        "name": encoded_name,
        "description": tool
            .get("description")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
        "input_schema": input_schema,
    })))
}

/// Encode a `(namespace, name)` tool identity into a single Anthropic-legal
/// tool name. Uses the same `namespace__name` scheme as the Chat adapter and a
/// deterministic hash fold when the result would exceed Anthropic's limit.
fn encode_tool_name(namespace: Option<&str>, name: &str) -> String {
    let encoded = namespace.map_or_else(
        || name.to_string(),
        |namespace| format!("{namespace}__{name}"),
    );
    truncate_tool_name(encoded)
}

fn truncate_tool_name(encoded: String) -> String {
    if encoded.chars().count() <= ANTHROPIC_TOOL_NAME_MAX_LEN {
        return encoded;
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut hasher = DefaultHasher::new();
    encoded.hash(&mut hasher);
    let suffix = format!("_{:016x}", hasher.finish());
    let prefix_len = ANTHROPIC_TOOL_NAME_MAX_LEN.saturating_sub(suffix.chars().count());
    let prefix: String = encoded.chars().take(prefix_len).collect();
    format!("{prefix}{suffix}")
}

/// A single Anthropic message under construction: a role plus its content blocks.
struct AnthropicMessage {
    role: &'static str,
    content: Vec<Value>,
}

/// Append a content block to `messages`, coalescing with the trailing message
/// when the role matches. Anthropic rejects two consecutive same-role messages,
/// so parallel tool calls and their results must fold into single messages.
fn push_content_block(messages: &mut Vec<AnthropicMessage>, role: &'static str, block: Value) {
    if let Some(last) = messages.last_mut()
        && last.role == role
    {
        last.content.push(block);
        return;
    }
    messages.push(AnthropicMessage {
        role,
        content: vec![block],
    });
}

fn anthropic_messages_from_responses(
    input: &[ResponseItem],
    system: &mut String,
) -> Result<Vec<Value>, ApiError> {
    let mut messages: Vec<AnthropicMessage> = Vec::new();

    for item in input {
        match item {
            ResponseItem::Message { role, content, .. } => match role.as_str() {
                // developer/system messages have no Anthropic message role; fold
                // them into the top-level system prompt.
                "system" | "developer" => {
                    let text = text_from_content_items(content);
                    if !text.is_empty() {
                        if !system.is_empty() {
                            system.push_str("\n\n");
                        }
                        system.push_str(&text);
                    }
                }
                "assistant" => {
                    for block in assistant_content_blocks(content) {
                        push_content_block(&mut messages, "assistant", block);
                    }
                }
                _ => {
                    for block in user_content_blocks(content)? {
                        push_content_block(&mut messages, "user", block);
                    }
                }
            },
            ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            } => {
                let block = serde_json::json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": encode_tool_name(namespace.as_deref(), name),
                    "input": parse_tool_input(arguments),
                });
                push_content_block(&mut messages, "assistant", block);
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                let mut block = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": output_text(output),
                });
                if output.success == Some(false)
                    && let Some(object) = block.as_object_mut()
                {
                    object.insert("is_error".to_string(), Value::Bool(true));
                }
                push_content_block(&mut messages, "user", block);
            }
            // Only *signed* thinking is replayed. Anthropic requires the opaque
            // signature to validate a thinking block across tool turns; unsigned
            // reasoning must not be sent back.
            ResponseItem::Reasoning {
                content,
                encrypted_content,
                ..
            } => {
                if let Some(signature) = encrypted_content {
                    let block = serde_json::json!({
                        "type": "thinking",
                        "thinking": reasoning_text_from_content(content),
                        "signature": signature,
                    });
                    push_content_block(&mut messages, "assistant", block);
                }
            }
            // Items with no Anthropic representation are dropped.
            ResponseItem::AgentMessage { .. }
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

    let mut out: Vec<Value> = messages
        .into_iter()
        .map(|message| {
            serde_json::json!({
                "role": message.role,
                "content": message.content,
            })
        })
        .collect();

    // Anthropic requires at least one message.
    if out.is_empty() {
        out.push(serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "" }],
        }));
    }

    Ok(out)
}

fn assistant_content_blocks(content: &[ContentItem]) -> Vec<Value> {
    let mut blocks = Vec::new();
    for item in content {
        if let ContentItem::OutputText { text } | ContentItem::InputText { text } = item {
            blocks.push(serde_json::json!({ "type": "text", "text": text }));
        }
    }
    blocks
}

fn user_content_blocks(content: &[ContentItem]) -> Result<Vec<Value>, ApiError> {
    let mut blocks = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                blocks.push(serde_json::json!({ "type": "text", "text": text }));
            }
            ContentItem::InputImage { image_url, .. } => {
                if let Some(block) = anthropic_image_block(image_url) {
                    blocks.push(block);
                } else {
                    return Err(ApiError::InvalidRequest {
                        message: "unsupported image input for Anthropic messages".to_string(),
                    });
                }
            }
        }
    }
    Ok(blocks)
}

/// Build an Anthropic image content block from a Codewith image URL, supporting
/// both `data:` (base64) URIs and remote `http(s)` URLs.
fn anthropic_image_block(image_url: &str) -> Option<Value> {
    if let Some(rest) = image_url.strip_prefix("data:") {
        let (media_type, data) = rest.split_once(";base64,")?;
        if media_type.is_empty() || data.is_empty() {
            return None;
        }
        return Some(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
            }
        }));
    }
    if image_url.starts_with("http://") || image_url.starts_with("https://") {
        return Some(serde_json::json!({
            "type": "image",
            "source": { "type": "url", "url": image_url }
        }));
    }
    None
}

/// Anthropic `tool_use.input` is an object. Parse the Responses-style argument
/// string; fall back to an empty object when it is blank or not valid JSON.
fn parse_tool_input(arguments: &str) -> Value {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str::<Value>(trimmed).unwrap_or_else(|_| serde_json::json!({}))
}

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

fn text_from_content_items(items: &[ContentItem]) -> String {
    let mut text = Vec::new();
    for item in items {
        if let ContentItem::InputText { text: item_text }
        | ContentItem::OutputText { text: item_text } = item
        {
            text.push(item_text.clone());
        }
    }
    text.join("\n")
}

fn output_text(output: &FunctionCallOutputPayload) -> String {
    output.body.to_text().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Streaming: Anthropic SSE -> ResponseEvent
// ---------------------------------------------------------------------------

fn spawn_anthropic_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_name_map: HashMap<String, AnthropicToolName>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .or_else(|| stream_response.headers.get("x-request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_anthropic_sse(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolUse,
    Other,
}

#[derive(Debug, Default)]
struct BlockState {
    kind: Option<BlockKind>,
    started: bool,
    text: String,
    signature: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_input: String,
}

#[derive(Debug, Default)]
struct AnthropicStreamState {
    response_id: Option<String>,
    model: Option<String>,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    stop_reason: Option<String>,
    blocks: BTreeMap<u64, BlockState>,
    completed: bool,
}

impl AnthropicStreamState {
    fn token_usage(&self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            output_tokens: self.output_tokens,
            reasoning_output_tokens: 0,
            total_tokens: self.input_tokens + self.output_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: Option<i64>,
    #[serde(default)]
    output_tokens: Option<i64>,
    #[serde(default)]
    cache_read_input_tokens: Option<i64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<i64>,
}

async fn process_anthropic_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_name_map: HashMap<String, AnthropicToolName>,
) {
    let mut stream = stream.eventsource();
    let mut state = AnthropicStreamState::default();

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("anthropic messages SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                if state.completed {
                    return;
                }
                // Stream closed without a message_stop. Emit whatever we can so
                // the caller still terminates the turn cleanly.
                emit_completed(&mut state, &tx_event, &tool_name_map).await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("anthropic messages SSE event: {}", &sse.data);
        let data = sse.data.trim();
        if data.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(err) => {
                debug!(error = %err, payload = %data, "failed to parse anthropic SSE event; skipping");
                continue;
            }
        };

        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "message_start" => handle_message_start(&mut state, &value, &tx_event).await,
            "content_block_start" => {
                handle_content_block_start(&mut state, &value, &tx_event).await
            }
            "content_block_delta" => {
                handle_content_block_delta(&mut state, &value, &tx_event).await
            }
            "content_block_stop" => {
                handle_content_block_stop(&mut state, &value, &tx_event, &tool_name_map).await;
            }
            "message_delta" => handle_message_delta(&mut state, &value),
            "message_stop" => {
                emit_completed(&mut state, &tx_event, &tool_name_map).await;
                return;
            }
            "error" => {
                let _ = tx_event.send(Err(anthropic_error_from_event(&value))).await;
                return;
            }
            // `ping` and unknown/future events are ignored per Anthropic's
            // versioning guidance to handle unknown event types gracefully.
            _ => {}
        }
    }
}

async fn handle_message_start(
    state: &mut AnthropicStreamState,
    value: &Value,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) {
    let Some(message) = value.get("message") else {
        return;
    };
    if let Some(id) = message.get("id").and_then(Value::as_str) {
        state.response_id = Some(id.to_string());
    }
    if let Some(model) = message.get("model").and_then(Value::as_str)
        && state.model.as_deref() != Some(model)
    {
        let _ = tx_event
            .send(Ok(ResponseEvent::ServerModel(model.to_string())))
            .await;
        state.model = Some(model.to_string());
    }
    if let Some(usage) = message.get("usage")
        && let Ok(usage) = serde_json::from_value::<AnthropicUsage>(usage.clone())
    {
        apply_usage(state, &usage);
    }
}

async fn handle_content_block_start(
    state: &mut AnthropicStreamState,
    value: &Value,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) {
    let Some(index) = value.get("index").and_then(Value::as_u64) else {
        return;
    };
    let block = value.get("content_block");
    let block_type = block
        .and_then(|block| block.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let mut block_state = BlockState::default();
    match block_type {
        "text" => {
            block_state.kind = Some(BlockKind::Text);
            block_state.started = true;
            let item = ResponseItem::Message {
                id: state.response_id.clone(),
                role: "assistant".to_string(),
                content: Vec::new(),
                phase: Some(MessagePhase::FinalAnswer),
            };
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(item)))
                .await;
        }
        "thinking" => {
            block_state.kind = Some(BlockKind::Thinking);
            block_state.started = true;
            let item = ResponseItem::Reasoning {
                id: state.response_id.clone().unwrap_or_default(),
                summary: Vec::new(),
                content: None,
                encrypted_content: None,
            };
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(item)))
                .await;
        }
        "tool_use" => {
            block_state.kind = Some(BlockKind::ToolUse);
            block_state.tool_call_id = block
                .and_then(|block| block.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            block_state.tool_name = block
                .and_then(|block| block.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        _ => {
            block_state.kind = Some(BlockKind::Other);
        }
    }
    state.blocks.insert(index, block_state);
}

async fn handle_content_block_delta(
    state: &mut AnthropicStreamState,
    value: &Value,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) {
    let Some(index) = value.get("index").and_then(Value::as_u64) else {
        return;
    };
    let Some(delta) = value.get("delta") else {
        return;
    };
    let delta_type = delta
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(block) = state.blocks.get_mut(&index) else {
        return;
    };

    match delta_type {
        "text_delta" => {
            let Some(text) = delta.get("text").and_then(Value::as_str) else {
                return;
            };
            block.text.push_str(text);
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputTextDelta(text.to_string())))
                .await;
        }
        "thinking_delta" => {
            let Some(text) = delta.get("thinking").and_then(Value::as_str) else {
                return;
            };
            block.text.push_str(text);
            let _ = tx_event
                .send(Ok(ResponseEvent::ReasoningContentDelta {
                    delta: text.to_string(),
                    content_index: 0,
                }))
                .await;
        }
        "signature_delta" => {
            if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                block.signature = Some(signature.to_string());
            }
        }
        "input_json_delta" => {
            let Some(partial) = delta.get("partial_json").and_then(Value::as_str) else {
                return;
            };
            block.tool_input.push_str(partial);
            let item_id = block
                .tool_call_id
                .clone()
                .unwrap_or_else(|| format!("tool_{index}"));
            let _ = tx_event
                .send(Ok(ResponseEvent::ToolCallInputDelta {
                    item_id: item_id.clone(),
                    call_id: block.tool_call_id.clone(),
                    delta: partial.to_string(),
                }))
                .await;
        }
        _ => {}
    }
}

async fn handle_content_block_stop(
    state: &mut AnthropicStreamState,
    value: &Value,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    tool_name_map: &HashMap<String, AnthropicToolName>,
) {
    let Some(index) = value.get("index").and_then(Value::as_u64) else {
        return;
    };
    let Some(block) = state.blocks.remove(&index) else {
        return;
    };
    finalize_block(state, block, tx_event, tool_name_map).await;
}

async fn finalize_block(
    state: &AnthropicStreamState,
    block: BlockState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    tool_name_map: &HashMap<String, AnthropicToolName>,
) {
    match block.kind {
        Some(BlockKind::Text) => {
            let item = ResponseItem::Message {
                id: state.response_id.clone(),
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text: block.text }],
                phase: Some(MessagePhase::FinalAnswer),
            };
            let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
        }
        Some(BlockKind::Thinking) => {
            let item = ResponseItem::Reasoning {
                id: state.response_id.clone().unwrap_or_default(),
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: block.text,
                }]),
                encrypted_content: block.signature,
            };
            let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
        }
        Some(BlockKind::ToolUse) => {
            let Some(encoded_name) = block.tool_name else {
                return;
            };
            let mapped = tool_name_map
                .get(&encoded_name)
                .cloned()
                .unwrap_or(AnthropicToolName {
                    namespace: None,
                    name: encoded_name.clone(),
                });
            let call_id = block.tool_call_id.unwrap_or_else(|| {
                format!(
                    "{}_{encoded_name}",
                    state.response_id.as_deref().unwrap_or("msg")
                )
            });
            let arguments = if block.tool_input.trim().is_empty() {
                "{}".to_string()
            } else {
                block.tool_input
            };
            let item = ResponseItem::FunctionCall {
                id: None,
                name: mapped.name,
                namespace: mapped.namespace,
                arguments,
                call_id,
            };
            let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
        }
        Some(BlockKind::Other) | None => {}
    }
}

fn handle_message_delta(state: &mut AnthropicStreamState, value: &Value) {
    if let Some(stop_reason) = value
        .get("delta")
        .and_then(|delta| delta.get("stop_reason"))
        .and_then(Value::as_str)
    {
        state.stop_reason = Some(stop_reason.to_string());
    }
    if let Some(usage) = value.get("usage")
        && let Ok(usage) = serde_json::from_value::<AnthropicUsage>(usage.clone())
    {
        apply_usage(state, &usage);
    }
}

/// Fold an Anthropic usage object into cumulative stream state.
///
/// Anthropic reports non-cached prompt tokens (`input_tokens`) separately from
/// prompt-cache reads/creations. Codewith's `input_tokens` is the total prompt
/// size with `cached_input_tokens` as a subset, so cache tokens are summed into
/// the input total while cache reads are also tracked as the cached subset.
fn apply_usage(state: &mut AnthropicStreamState, usage: &AnthropicUsage) {
    if let Some(input) = usage.input_tokens {
        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
        state.input_tokens = input + cache_read + cache_creation;
        state.cached_input_tokens = cache_read;
    } else if let Some(cache_read) = usage.cache_read_input_tokens {
        state.cached_input_tokens = cache_read;
    }
    if let Some(output) = usage.output_tokens {
        state.output_tokens = output;
    }
}

async fn emit_completed(
    state: &mut AnthropicStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    tool_name_map: &HashMap<String, AnthropicToolName>,
) {
    if state.completed {
        return;
    }
    state.completed = true;

    // Finalize any content blocks that never received an explicit stop event
    // (for example if the stream ended abruptly).
    let pending: Vec<BlockState> = std::mem::take(&mut state.blocks).into_values().collect();
    for block in pending {
        finalize_block(state, block, tx_event, tool_name_map).await;
    }

    let response_id = state
        .response_id
        .clone()
        .unwrap_or_else(|| "anthropic".to_string());
    let end_turn = end_turn_from_stop_reason(state.stop_reason.as_deref());
    let _ = tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id,
            token_usage: Some(state.token_usage()),
            end_turn,
        }))
        .await;
}

/// Map an Anthropic `stop_reason` to whether the model affirmatively ended its
/// turn. `tool_use`/`max_tokens`/`pause_turn` mean the turn should continue.
fn end_turn_from_stop_reason(stop_reason: Option<&str>) -> Option<bool> {
    match stop_reason {
        Some("end_turn" | "stop_sequence") => Some(true),
        Some("tool_use" | "max_tokens" | "pause_turn" | "refusal") => Some(false),
        _ => None,
    }
}

fn anthropic_error_from_event(value: &Value) -> ApiError {
    let error = value.get("error");
    let error_type = error
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("anthropic stream error")
        .to_string();
    match error_type {
        "overloaded_error" => ApiError::ServerOverloaded,
        "invalid_request_error" => ApiError::InvalidRequest { message },
        "rate_limit_error" => ApiError::RateLimit(message),
        _ => ApiError::Stream(format!("anthropic error: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_client::TransportError;
    use pretty_assertions::assert_eq;
    use tokio_util::io::ReaderStream;

    fn user_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
        }
    }

    fn base_request(input: Vec<ResponseItem>, tools: Vec<Value>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "claude-opus-4-8".to_string(),
            instructions: "Be terse".to_string(),
            input,
            tools,
            tool_choice: "auto".to_string(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        }
    }

    fn reasoning(effort: ReasoningEffort) -> Reasoning {
        Reasoning {
            effort: Some(effort),
            summary: None,
            context: None,
        }
    }

    async fn collect_events(body: &str) -> Vec<Result<ResponseEvent, ApiError>> {
        collect_events_with_map(body, HashMap::new()).await
    }

    async fn collect_events_with_map(
        body: &str,
        tool_name_map: HashMap<String, AnthropicToolName>,
    ) -> Vec<Result<ResponseEvent, ApiError>> {
        let stream = ReaderStream::new(std::io::Cursor::new(body.to_string()))
            .map(|chunk| chunk.map_err(|err| TransportError::Network(err.to_string())));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
        tokio::spawn(process_anthropic_sse(
            Box::pin(stream),
            tx,
            Duration::from_secs(5),
            /*telemetry*/ None,
            tool_name_map,
        ));

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn request_maps_system_messages_tools_and_stream() {
        let request = base_request(
            vec![user_text("hello")],
            vec![serde_json::json!({
                "type": "function",
                "name": "exec_command",
                "description": "Run a command",
                "parameters": {"type": "object", "properties": {}},
            })],
        );

        let parts = anthropic_request_from_responses(request).expect("request should map");

        assert_eq!(parts.body["model"], "claude-opus-4-8");
        assert_eq!(parts.body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(parts.body["stream"], true);
        assert_eq!(parts.body["system"], "Be terse");
        assert_eq!(
            parts.body["messages"],
            serde_json::json!([
                {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            ])
        );
        assert_eq!(
            parts.body["tools"],
            serde_json::json!([{
                "name": "exec_command",
                "description": "Run a command",
                "input_schema": {"type": "object", "properties": {}},
            }])
        );
        assert_eq!(
            parts.body["tool_choice"],
            serde_json::json!({"type": "auto"})
        );
        assert!(parts.body.get("thinking").is_none());
    }

    #[test]
    fn request_folds_developer_messages_into_system() {
        let mut request = base_request(
            vec![
                ResponseItem::Message {
                    id: None,
                    role: "developer".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "Follow policy".to_string(),
                    }],
                    phase: None,
                },
                user_text("hi"),
            ],
            Vec::new(),
        );
        request.instructions = "Base".to_string();

        let parts = anthropic_request_from_responses(request).expect("request should map");
        assert_eq!(parts.body["system"], "Base\n\nFollow policy");
        assert_eq!(
            parts.body["messages"],
            serde_json::json!([
                {"role": "user", "content": [{"type": "text", "text": "hi"}]}
            ])
        );
    }

    #[test]
    fn request_enables_thinking_and_downgrades_forced_tool_choice() {
        let mut request = base_request(
            vec![user_text("solve it")],
            vec![serde_json::json!({
                "type": "function",
                "name": "calc",
                "parameters": {"type": "object"},
            })],
        );
        request.reasoning = Some(reasoning(ReasoningEffort::High));
        request.tool_choice = "required".to_string();

        let parts = anthropic_request_from_responses(request).expect("request should map");
        assert_eq!(parts.body["thinking"]["type"], "enabled");
        assert_eq!(parts.body["thinking"]["budget_tokens"], 16_000);
        // Forced tool use is incompatible with thinking, so it downgrades to auto.
        assert_eq!(
            parts.body["tool_choice"],
            serde_json::json!({"type": "auto"})
        );
    }

    #[test]
    fn request_disables_parallel_tool_use_when_requested() {
        let mut request = base_request(
            vec![user_text("go")],
            vec![serde_json::json!({
                "type": "function",
                "name": "calc",
                "parameters": {"type": "object"},
            })],
        );
        request.parallel_tool_calls = false;

        let parts = anthropic_request_from_responses(request).expect("request should map");
        assert_eq!(
            parts.body["tool_choice"],
            serde_json::json!({"type": "auto", "disable_parallel_tool_use": true})
        );
    }

    #[test]
    fn request_flattens_namespace_tools_and_records_mapping() {
        let mut map = HashMap::new();
        let tools = anthropic_tools_from_responses(
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

        assert_eq!(tools[0]["name"], "mcp__read");
        assert_eq!(
            tools[0]["input_schema"],
            serde_json::json!({"type": "object"})
        );
        assert_eq!(
            map.get("mcp__read"),
            Some(&AnthropicToolName {
                namespace: Some("mcp".to_string()),
                name: "read".to_string(),
            })
        );
    }

    #[test]
    fn messages_coalesce_tool_use_and_tool_result_blocks() {
        let mut system = String::new();
        let messages = anthropic_messages_from_responses(
            &[
                user_text("What is the weather?"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "get_weather".to_string(),
                    namespace: None,
                    arguments: "{\"city\":\"SF\"}".to_string(),
                    call_id: "toolu_1".to_string(),
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "get_time".to_string(),
                    namespace: None,
                    arguments: String::new(),
                    call_id: "toolu_2".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "toolu_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("sunny".to_string()),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "toolu_2".to_string(),
                    output: FunctionCallOutputPayload::from_text("noon".to_string()),
                },
            ],
            &mut system,
        )
        .expect("messages should map");

        assert_eq!(
            Value::Array(messages),
            serde_json::json!([
                {"role": "user", "content": [{"type": "text", "text": "What is the weather?"}]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "SF"}},
                    {"type": "tool_use", "id": "toolu_2", "name": "get_time", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "sunny"},
                    {"type": "tool_result", "tool_use_id": "toolu_2", "content": "noon"}
                ]}
            ])
        );
    }

    #[test]
    fn messages_replay_only_signed_thinking() {
        let mut system = String::new();
        let messages = anthropic_messages_from_responses(
            &[
                ResponseItem::Reasoning {
                    id: String::new(),
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: "unsigned".to_string(),
                    }]),
                    encrypted_content: None,
                },
                ResponseItem::Reasoning {
                    id: String::new(),
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: "signed thought".to_string(),
                    }]),
                    encrypted_content: Some("sig-abc".to_string()),
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "answer".to_string(),
                    }],
                    phase: None,
                },
            ],
            &mut system,
        )
        .expect("messages should map");

        assert_eq!(
            Value::Array(messages),
            serde_json::json!([
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "signed thought", "signature": "sig-abc"},
                    {"type": "text", "text": "answer"}
                ]}
            ])
        );
    }

    #[test]
    fn messages_map_data_url_images() {
        let mut system = String::new();
        let messages = anthropic_messages_from_responses(
            &[ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputImage {
                    image_url: "data:image/png;base64,QUJD".to_string(),
                    detail: None,
                }],
                phase: None,
            }],
            &mut system,
        )
        .expect("messages should map");

        assert_eq!(
            Value::Array(messages),
            serde_json::json!([
                {"role": "user", "content": [{
                    "type": "image",
                    "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}
                }]}
            ])
        );
    }

    #[tokio::test]
    async fn stream_maps_text_and_usage() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"!\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":15}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events: Vec<ResponseEvent> = collect_events(body)
            .await
            .into_iter()
            .map(|event| event.expect("events should be ok"))
            .collect();

        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["Hello", "!"]);

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. })
                if content == &vec![ContentItem::OutputText { text: "Hello!".to_string() }]
        )));

        match events.last().expect("must complete") {
            ResponseEvent::Completed {
                response_id,
                token_usage,
                end_turn,
            } => {
                assert_eq!(response_id, "msg_1");
                assert_eq!(*end_turn, Some(true));
                let usage = token_usage.as_ref().expect("usage present");
                assert_eq!(usage.input_tokens, 25);
                assert_eq!(usage.output_tokens, 15);
                assert_eq!(usage.total_tokens, 40);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_accumulates_tool_use_arguments() {
        let mut map = HashMap::new();
        map.insert(
            "mcp__read".to_string(),
            AnthropicToolName {
                namespace: Some("mcp".to_string()),
                name: "read".to_string(),
            },
        );
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"mcp__read\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\" \\\"/tmp\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events = collect_events_with_map(body, map).await;
        let function_call = events
            .iter()
            .find_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                    name,
                    namespace,
                    arguments,
                    call_id,
                    ..
                })) => Some((
                    name.clone(),
                    namespace.clone(),
                    arguments.clone(),
                    call_id.clone(),
                )),
                _ => None,
            })
            .expect("function call emitted");

        assert_eq!(function_call.0, "read");
        assert_eq!(function_call.1, Some("mcp".to_string()));
        assert_eq!(function_call.2, "{\"path\": \"/tmp\"}");
        assert_eq!(function_call.3, "toolu_9");

        let end_turn = events.iter().find_map(|event| match event {
            Ok(ResponseEvent::Completed { end_turn, .. }) => Some(*end_turn),
            _ => None,
        });
        assert_eq!(end_turn, Some(Some(false)));
    }

    #[tokio::test]
    async fn stream_maps_thinking_with_signature() {
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_3\",\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"step 1\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-xyz\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events = collect_events(body).await;

        assert!(events.iter().any(|event| matches!(
            event,
            Ok(ResponseEvent::ReasoningContentDelta { delta, .. }) if delta == "step 1"
        )));

        let reasoning = events
            .iter()
            .find_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                    content,
                    encrypted_content,
                    ..
                })) => Some((content.clone(), encrypted_content.clone())),
                _ => None,
            })
            .expect("reasoning item emitted");
        assert_eq!(
            reasoning.0,
            Some(vec![ReasoningItemContent::ReasoningText {
                text: "step 1".to_string()
            }])
        );
        assert_eq!(reasoning.1, Some("sig-xyz".to_string()));
    }

    #[tokio::test]
    async fn stream_maps_error_events() {
        let body = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        );

        let events = collect_events(body).await;
        assert!(matches!(
            events.as_slice(),
            [Err(ApiError::ServerOverloaded)]
        ));
    }
}
