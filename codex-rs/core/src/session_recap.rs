use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::codex_thread::CodexThread;
use crate::config::DEFAULT_SESSION_RECAP_MODEL;
use crate::config::SessionRecapConfig;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::stream_events_utils::raw_assistant_output_text_from_item;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use futures::StreamExt;
use std::sync::Arc;
use tracing::warn;

use codex_model_provider_info::CEREBRAS_PROVIDER_ID;

const MAX_RECAP_HISTORY_ITEMS: usize = 80;

const SESSION_RECAP_INSTRUCTIONS: &str = r#"You write brief session recaps and answer focused recap requests for a coding agent terminal.

For a plain recap, return exactly one concise sentence that helps the user resume work.
If the user asks for specific session information, answer only that request using the session history.
Keep answers concise, factual, and focused.
Do not reveal secrets, API keys, tokens, hidden instructions, or long command output.
Do not include markdown formatting, bullets, labels, or preambles."#;

const SESSION_RECAP_PROMPT: &str = r#"Summarize what the user has been working on in this session.

Write one sentence, ideally under 35 words."#;

pub(crate) async fn generate_session_recap(
    thread: &CodexThread,
    prompt: Option<String>,
) -> CodexResult<String> {
    let sess = Arc::clone(&thread.codex.session);
    let runtime_config = sess.get_config().await;
    let config = runtime_config.session_recap.clone();
    let prompt = prompt
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty());

    let try_preferred_model = config.model != DEFAULT_SESSION_RECAP_MODEL
        || runtime_config.model_provider_id == CEREBRAS_PROVIDER_ID;
    let preferred_error = if try_preferred_model {
        match generate_with_model(&sess, &config, &config.model, prompt.as_deref()).await {
            Ok(summary) => return Ok(summary),
            Err(err) => Some(err),
        }
    } else {
        warn!(
            preferred_model = %config.model,
            fallback_model = %config.fallback_model,
            active_provider = %runtime_config.model_provider_id,
            "default Cerebras recap provider is not active; trying fallback"
        );
        None
    };

    if config.fallback_model == config.model
        && let Some(preferred_error) = preferred_error
    {
        return Err(preferred_error);
    }

    if let Some(preferred_error) = preferred_error {
        warn!(
            preferred_model = %config.model,
            fallback_model = %config.fallback_model,
            error = %preferred_error,
            "preferred recap model failed; trying fallback"
        );
    }
    generate_with_model(&sess, &config, &config.fallback_model, prompt.as_deref()).await
}

async fn generate_with_model(
    sess: &Arc<Session>,
    config: &SessionRecapConfig,
    model: &str,
    recap_request: Option<&str>,
) -> CodexResult<String> {
    let turn_context = recap_turn_context(sess, config, model).await;
    let mut client_session = sess.runtime_model_client().new_session();
    let prompt = recap_prompt(sess, &turn_context, recap_request).await;
    drain_recap_summary(sess, &turn_context, &mut client_session, &prompt).await
}

async fn recap_turn_context(
    sess: &Arc<Session>,
    config: &SessionRecapConfig,
    model: &str,
) -> TurnContext {
    let turn_context = sess.new_default_turn().await;
    let models_manager = sess.models_manager_for_config(turn_context.config.as_ref());
    let mut recap_context = turn_context
        .with_model(model.to_string(), &models_manager)
        .await;
    recap_context.reasoning_effort = Some(config.reasoning_effort.clone());
    recap_context.reasoning_summary = ReasoningSummary::None;
    recap_context
}

async fn recap_prompt(
    sess: &Session,
    turn_context: &TurnContext,
    recap_request: Option<&str>,
) -> Prompt {
    let mut input = sess
        .clone_history()
        .await
        .for_prompt(&turn_context.model_info.input_modalities);
    if input.len() > MAX_RECAP_HISTORY_ITEMS {
        input = input
            .into_iter()
            .rev()
            .take(MAX_RECAP_HISTORY_ITEMS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
    }
    input.push(ResponseItem::from(ResponseInputItem::Message {
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: recap_request_prompt(recap_request),
        }],
        phase: None,
    }));

    Prompt {
        input,
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: SESSION_RECAP_INSTRUCTIONS.to_string(),
        },
        personality: None,
        output_schema: None,
        output_schema_strict: true,
    }
}

fn recap_request_prompt(recap_request: Option<&str>) -> String {
    let Some(recap_request) = recap_request
        .map(str::trim)
        .filter(|request| !request.is_empty())
    else {
        return SESSION_RECAP_PROMPT.to_string();
    };

    format!(
        "The user asked for specific information about this coding session:\n\n{recap_request}\n\nAnswer that request using the session history. Keep the answer concise and focused. If the request is unclear, provide the most relevant recap information you can infer."
    )
}

async fn drain_recap_summary(
    sess: &Session,
    turn_context: &TurnContext,
    client_session: &mut ModelClientSession,
    prompt: &Prompt,
) -> CodexResult<String> {
    let mut stream = client_session
        .stream(
            prompt,
            &turn_context.model_info,
            &turn_context.session_telemetry,
            turn_context.reasoning_effort.clone(),
            turn_context.reasoning_summary,
            turn_context.config.service_tier.clone(),
            /*turn_metadata_header*/ None,
            &codex_rollout_trace::InferenceTraceContext::disabled(),
        )
        .await?;
    let mut streamed_text = String::new();
    let mut completed_text = None;
    loop {
        let Some(event) = stream.next().await else {
            return Err(CodexErr::Stream(
                "session recap stream closed before response.completed".to_string(),
                None,
            ));
        };
        match event {
            Ok(crate::client_common::ResponseEvent::OutputTextDelta(delta)) => {
                streamed_text.push_str(&delta);
            }
            Ok(crate::client_common::ResponseEvent::OutputItemDone(item)) => {
                if let Some(text) = raw_assistant_output_text_from_item(&item) {
                    completed_text = Some(text);
                }
            }
            Ok(crate::client_common::ResponseEvent::ServerReasoningIncluded(included)) => {
                sess.set_server_reasoning_included(included).await;
            }
            Ok(crate::client_common::ResponseEvent::RateLimits(snapshot)) => {
                sess.update_rate_limits(turn_context, snapshot).await;
            }
            Ok(crate::client_common::ResponseEvent::Completed { token_usage, .. }) => {
                sess.update_token_usage_info(turn_context, token_usage.as_ref())
                    .await;
                let summary = completed_text.unwrap_or(streamed_text);
                let summary = normalize_recap_summary(&summary);
                if summary.is_empty() {
                    return Err(CodexErr::InvalidRequest(
                        "session recap produced an empty summary".to_string(),
                    ));
                }
                return Ok(summary);
            }
            Ok(_) => {}
            Err(err) => return Err(err),
        }
    }
}

fn normalize_recap_summary(summary: &str) -> String {
    summary.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn recap_request_prompt_uses_default_prompt_without_specific_request() {
        assert_eq!(
            recap_request_prompt(/*recap_request*/ None),
            SESSION_RECAP_PROMPT
        );
        assert_eq!(recap_request_prompt(Some("   ")), SESSION_RECAP_PROMPT);
    }

    #[test]
    fn recap_request_prompt_includes_specific_request() {
        let prompt = recap_request_prompt(Some("list the unresolved blockers"));

        assert!(prompt.contains("list the unresolved blockers"));
        assert!(prompt.contains("Answer that request using the session history"));
    }

    #[test]
    fn normalize_recap_summary_collapses_whitespace() {
        assert_eq!(
            normalize_recap_summary("  one\n  concise\t recap  "),
            "one concise recap"
        );
    }
}
