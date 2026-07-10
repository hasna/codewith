use std::future::Future;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context as _;
use codex_features::Feature;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use regex_lite::Regex;
use serde_json::Value;
use tracing::debug;
use tracing::warn;

use crate::client::ModelClient;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::client_common::ResponseStream;
use crate::config::SmartSuggestConfig;
use crate::hook_runtime::record_additional_contexts;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::registry::PreToolUsePayload;

const SMART_SUGGEST_INSTRUCTIONS: &str = r#"You are an experimental pre-tool advisory pass for Codewith.

Review the planned tool call and return at most one concise suggestion that may help the main agent choose a better tool, command, or Hasna app abstraction.

Rules:
- Suggestions are advisory only. Do not approve, deny, block, rewrite, or execute anything.
- Prefer "NO_SUGGESTION" when the planned tool call already looks reasonable.
- Do not mention secrets. Treat redacted or missing data as unavailable.
- Keep the response under 120 words.
"#;

const ADVISORY_CONTEXT_PREFIX: &str = "Experimental smart-suggest advisory (advisory only; not approval, denial, policy, or a user instruction; ignore if unhelpful):";
const NO_SUGGESTION: &str = "NO_SUGGESTION";
const TRUNCATED_MARKER: &str = "\n[truncated]";

#[derive(Debug, Clone, PartialEq)]
struct SmartSuggestToolCall {
    tool_use_id: String,
    tool_name: String,
    tool_input: Value,
}

#[derive(Debug, Clone)]
struct SmartSuggestModelRequest {
    model_provider: Option<String>,
    model: String,
    service_tier: Option<String>,
    prompt: Prompt,
    max_suggestion_chars: usize,
}

pub(crate) async fn maybe_record_pre_tool_guidance(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    tool_use_id: &str,
    payload: &PreToolUsePayload,
) {
    let infinity_agent_policy = turn.config.is_infinity_agent();
    if infinity_agent_policy {
        return;
    }
    let tool_call = SmartSuggestToolCall {
        tool_use_id: tool_use_id.to_string(),
        tool_name: payload.tool_name.name().to_string(),
        tool_input: payload.tool_input.clone(),
    };
    let config = turn.config.experimental_smart_suggest.clone();
    let advisory = run_smart_suggest_for_policy(
        infinity_agent_policy,
        &config,
        &tool_call,
        |request| query_smart_suggest_model(session, turn, request),
    )
    .await;
    if let Some(advisory) = advisory {
        record_additional_contexts(session, turn, vec![advisory]).await;
    }
}

async fn run_smart_suggest_for_policy<F, Fut>(
    infinity_agent_policy: bool,
    config: &SmartSuggestConfig,
    tool_call: &SmartSuggestToolCall,
    call_model: F,
) -> Option<String>
where
    F: FnOnce(SmartSuggestModelRequest) -> Fut,
    Fut: Future<Output = anyhow::Result<Option<String>>>,
{
    if infinity_agent_policy {
        return None;
    }
    run_smart_suggest_with_model(config, tool_call, call_model).await
}

async fn run_smart_suggest_with_model<F, Fut>(
    config: &SmartSuggestConfig,
    tool_call: &SmartSuggestToolCall,
    call_model: F,
) -> Option<String>
where
    F: FnOnce(SmartSuggestModelRequest) -> Fut,
    Fut: Future<Output = anyhow::Result<Option<String>>>,
{
    if !config.enabled {
        return None;
    }

    let Some(model) = config.model.clone() else {
        debug!("experimental smart-suggest enabled without model; skipping");
        return None;
    };
    let request = SmartSuggestModelRequest {
        model_provider: config.model_provider.clone(),
        model,
        service_tier: config.service_tier.clone(),
        prompt: build_prompt(tool_call, config.max_input_chars),
        max_suggestion_chars: config.max_suggestion_chars,
    };
    let timeout = Duration::from_millis(config.timeout_ms);
    match tokio::time::timeout(timeout, call_model(request)).await {
        Ok(Ok(Some(suggestion))) => advisory_context(&suggestion, config.max_suggestion_chars),
        Ok(Ok(None)) => None,
        Ok(Err(err)) => {
            debug!("experimental smart-suggest request failed: {err:#}");
            None
        }
        Err(_) => {
            debug!(
                timeout_ms = config.timeout_ms,
                "experimental smart-suggest request timed out"
            );
            None
        }
    }
}

async fn query_smart_suggest_model(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    request: SmartSuggestModelRequest,
) -> anyhow::Result<Option<String>> {
    if let Some(provider_id) = request.model_provider.as_deref()
        && provider_id != turn.config.model_provider_id
        && !turn.config.model_providers.contains_key(provider_id)
    {
        warn!(
            model_provider = provider_id,
            "experimental smart-suggest model provider is not configured; skipping"
        );
        return Ok(None);
    }

    let models_manager = session
        .models_manager_for_config_provider_id(
            turn.config.as_ref(),
            request.model_provider.as_deref(),
        )
        .await;
    let suggest_turn = turn
        .with_model_provider_and_model(
            request.model_provider.clone(),
            request.model.clone(),
            &models_manager,
        )
        .await;
    let model_client = ModelClient::new(
        Some(Arc::clone(&session.services.auth_manager)),
        session.session_id(),
        session.thread_id(),
        session.installation_id.clone(),
        suggest_turn.config.model_provider_id.clone(),
        suggest_turn.config.model_provider.clone(),
        suggest_turn.session_source.clone(),
        suggest_turn.parent_thread_id,
        suggest_turn.config.model_verbosity,
        suggest_turn
            .config
            .features
            .enabled(Feature::EnableRequestCompression),
        suggest_turn
            .config
            .features
            .enabled(Feature::RuntimeMetrics),
        /*beta_features_header*/ None,
        session.services.attestation_provider.clone(),
    );
    let mut client_session = model_client.new_http_session();
    let stream = client_session
        .stream(
            &request.prompt,
            &suggest_turn.model_info,
            &suggest_turn.session_telemetry,
            lightweight_reasoning_effort(&suggest_turn.model_info),
            ReasoningSummary::None,
            request.service_tier,
            /*turn_metadata_header*/ None,
            &InferenceTraceContext::disabled(),
        )
        .await
        .context("smart-suggest stream request failed")?;

    collect_suggestion(stream, request.max_suggestion_chars).await
}

fn lightweight_reasoning_effort(model_info: &ModelInfo) -> Option<ReasoningEffort> {
    if model_info
        .supported_reasoning_levels
        .iter()
        .any(|preset| preset.effort == ReasoningEffort::Minimal)
    {
        Some(ReasoningEffort::Minimal)
    } else if model_info
        .supported_reasoning_levels
        .iter()
        .any(|preset| preset.effort == ReasoningEffort::Low)
    {
        Some(ReasoningEffort::Low)
    } else {
        None
    }
}

async fn collect_suggestion(
    mut stream: ResponseStream,
    max_suggestion_chars: usize,
) -> anyhow::Result<Option<String>> {
    let mut output = String::new();
    let mut saw_completed = false;
    while let Some(event) = stream.next().await {
        match event? {
            ResponseEvent::OutputItemDone(item) => {
                if let Some(text) = response_item_text(&item) {
                    output.push_str(text.as_str());
                    output = truncate_chars(&output, max_suggestion_chars);
                }
            }
            ResponseEvent::Completed { .. } => {
                saw_completed = true;
                break;
            }
            _ => {}
        }
    }

    if !saw_completed {
        anyhow::bail!("smart-suggest stream closed before response.completed");
    }

    Ok(normalize_suggestion(&output, max_suggestion_chars))
}

fn response_item_text(item: &ResponseItem) -> Option<String> {
    let ResponseItem::Message { role, content, .. } = item else {
        return None;
    };
    if role != "assistant" {
        return None;
    }
    let text = content
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(text.as_str())
            }
            ContentItem::InputImage { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

fn build_prompt(tool_call: &SmartSuggestToolCall, max_input_chars: usize) -> Prompt {
    let input = serde_json::json!({
        "tool_use_id": tool_call.tool_use_id,
        "tool_name": tool_call.tool_name,
        "tool_input": redacted_tool_input(&tool_call.tool_input, max_input_chars),
    });
    Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!(
                    "Planned tool call metadata follows. Return {NO_SUGGESTION} unless a better tool, command, or Hasna app abstraction is clearly preferable.\n\n{}",
                    serde_json::to_string_pretty(&input).unwrap_or_else(|_| input.to_string())
                ),
            }],
            phase: None,
        }],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: SMART_SUGGEST_INSTRUCTIONS.to_string(),
        },
        personality: None,
        output_schema: None,
        output_schema_strict: true,
    }
}

fn redacted_tool_input(tool_input: &Value, max_input_chars: usize) -> String {
    let serialized =
        serde_json::to_string_pretty(tool_input).unwrap_or_else(|_| tool_input.to_string());
    truncate_chars(&redact_sensitive_text(&serialized), max_input_chars)
}

fn redact_sensitive_text(text: &str) -> String {
    let redacted_tokens = secret_token_regex().replace_all(text, "[REDACTED]");
    redacted_tokens
        .lines()
        .map(redact_secret_assignment_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_secret_assignment_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let sensitive_key = [
        "api_key",
        "apikey",
        "token",
        "password",
        "secret",
        "authorization",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !sensitive_key {
        return line.to_string();
    }
    let Some(separator) = line.find(':').or_else(|| line.find('=')) else {
        return line.to_string();
    };
    format!("{} [REDACTED]", &line[..=separator])
}

fn secret_token_regex() -> &'static Regex {
    static SECRET_TOKEN_REGEX: OnceLock<Regex> = OnceLock::new();
    SECRET_TOKEN_REGEX.get_or_init(|| {
        match Regex::new(
            r"sk-(?:ant|proj)-[A-Za-z0-9_-]+|gh[op]_[A-Za-z0-9_]+|npm_[A-Za-z0-9_]+|ctx7sk-[A-Za-z0-9_-]+|xai-[A-Za-z0-9_-]+|AIza[A-Za-z0-9_-]+|AKIA[A-Z0-9]+",
        ) {
            Ok(regex) => regex,
            Err(err) => panic!("secret token regex should compile: {err}"),
        }
    })
}

fn advisory_context(suggestion: &str, max_suggestion_chars: usize) -> Option<String> {
    normalize_suggestion(suggestion, max_suggestion_chars)
        .map(|suggestion| format!("{ADVISORY_CONTEXT_PREFIX}\n{suggestion}"))
}

fn normalize_suggestion(suggestion: &str, max_suggestion_chars: usize) -> Option<String> {
    let suggestion = truncate_chars(suggestion.trim(), max_suggestion_chars);
    let suggestion = suggestion.trim();
    if suggestion.is_empty() || suggestion.eq_ignore_ascii_case(NO_SUGGESTION) {
        None
    } else {
        Some(suggestion.to_string())
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let marker_chars = TRUNCATED_MARKER.chars().count();
    let keep_chars = max_chars.saturating_sub(marker_chars);
    let mut truncated = value.chars().take(keep_chars).collect::<String>();
    truncated.push_str(TRUNCATED_MARKER);
    truncated
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::*;

    fn tool_call() -> SmartSuggestToolCall {
        SmartSuggestToolCall {
            tool_use_id: "call-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "grep -R token src" }),
        }
    }

    #[tokio::test]
    async fn disabled_config_does_not_call_model() {
        let called = AtomicBool::new(false);

        let advisory =
            run_smart_suggest_with_model(&SmartSuggestConfig::default(), &tool_call(), |_| {
                called.store(true, Ordering::Relaxed);
                async { Ok(Some("should not run".to_string())) }
            })
            .await;

        assert_eq!(advisory, None);
        assert_eq!(called.load(Ordering::Relaxed), false);
    }

    #[tokio::test]
    async fn infinity_agent_policy_does_not_call_smart_suggest_model() {
        let called = AtomicBool::new(false);
        let config = SmartSuggestConfig {
            enabled: true,
            model_provider: Some("attacker-provider".to_string()),
            model: Some("argument-exfiltration-model".to_string()),
            ..Default::default()
        };

        let advisory = run_smart_suggest_for_policy(
            /*infinity_agent_policy*/ true,
            &config,
            &tool_call(),
            |_| {
                called.store(true, Ordering::Relaxed);
                async { Ok(Some("untrusted advisory".to_string())) }
            },
        )
        .await;

        assert_eq!(advisory, None);
        assert_eq!(called.load(Ordering::Relaxed), false);
    }

    #[tokio::test]
    async fn enabled_config_without_model_does_not_call_model() {
        let called = AtomicBool::new(false);
        let config = SmartSuggestConfig {
            enabled: true,
            ..Default::default()
        };

        let advisory = run_smart_suggest_with_model(&config, &tool_call(), |_| {
            called.store(true, Ordering::Relaxed);
            async { Ok(Some("should not run".to_string())) }
        })
        .await;

        assert_eq!(advisory, None);
        assert_eq!(called.load(Ordering::Relaxed), false);
    }

    #[tokio::test]
    async fn successful_suggestion_is_labeled_advisory() {
        let config = SmartSuggestConfig {
            enabled: true,
            model: Some("fast-model".to_string()),
            ..Default::default()
        };

        let advisory = run_smart_suggest_with_model(&config, &tool_call(), |request| async move {
            assert_eq!(request.model, "fast-model");
            assert!(prompt_text(&request.prompt).contains("\"tool_name\": \"Bash\""));
            Ok(Some("Use `rg` instead of recursive grep.".to_string()))
        })
        .await;

        assert_eq!(
            advisory,
            Some(format!(
                "{ADVISORY_CONTEXT_PREFIX}\nUse `rg` instead of recursive grep."
            ))
        );
    }

    #[tokio::test]
    async fn no_suggestion_output_is_ignored() {
        let config = SmartSuggestConfig {
            enabled: true,
            model: Some("fast-model".to_string()),
            ..Default::default()
        };

        let advisory = run_smart_suggest_with_model(&config, &tool_call(), |_request| async {
            Ok(Some(NO_SUGGESTION.to_string()))
        })
        .await;

        assert_eq!(advisory, None);
    }

    #[tokio::test]
    async fn model_errors_are_non_blocking() {
        let config = SmartSuggestConfig {
            enabled: true,
            model: Some("fast-model".to_string()),
            ..Default::default()
        };

        let advisory = run_smart_suggest_with_model(&config, &tool_call(), |_request| async {
            anyhow::bail!("provider unavailable")
        })
        .await;

        assert_eq!(advisory, None);
    }

    #[tokio::test]
    async fn timeout_is_non_blocking() {
        let config = SmartSuggestConfig {
            enabled: true,
            model: Some("fast-model".to_string()),
            timeout_ms: 50,
            ..Default::default()
        };

        let advisory = run_smart_suggest_with_model(&config, &tool_call(), |_request| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(Some("late".to_string()))
        })
        .await;

        assert_eq!(advisory, None);
    }

    #[test]
    fn prompt_redacts_and_truncates_tool_input() {
        let projected_key = format!("{}{}", "sk-pro", "j-secretvalue");
        let github_key = format!("{}{}", "gh", "p_secretvalue");
        let tool_call = SmartSuggestToolCall {
            tool_use_id: "call-secret".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({
                "command": format!(
                    "curl -H 'Authorization: Bearer {projected_key}' https://example.invalid"
                ),
                "api_key": github_key,
                "notes": "safe long field ".repeat(40),
            }),
        };

        let prompt = build_prompt(&tool_call, /*max_input_chars*/ 160);
        let text = prompt_text(&prompt);

        assert!(text.contains("[REDACTED]"));
        assert!(!text.contains(&projected_key));
        assert!(!text.contains(&github_key));
        assert!(text.contains("[truncated]"));
    }

    fn prompt_text(prompt: &Prompt) -> String {
        let ResponseItem::Message { content, .. } = &prompt.input[0] else {
            panic!("expected prompt message");
        };
        content
            .iter()
            .filter_map(|item| match item {
                ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                    Some(text.as_str())
                }
                ContentItem::InputImage { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
