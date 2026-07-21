use super::*;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct RewriteAgentMessageContributor;

#[async_trait::async_trait]
impl TurnItemContributor for RewriteAgentMessageContributor {
    async fn contribute(
        &self,
        _thread_store: &ExtensionData,
        _turn_store: &ExtensionData,
        item: &mut TurnItem,
    ) -> Result<(), String> {
        if let TurnItem::AgentMessage(agent_message) = item {
            agent_message.content = vec![AgentMessageContent::Text {
                text: "plan contributed assistant text".to_string(),
            }];
        }
        Ok(())
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some("msg-1".to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
    }
}

fn user_input_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some("msg-user".to_string()),
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
    }
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}

#[tokio::test]
async fn headless_prompt_bounding_leaves_stored_history_untouched() {
    let (session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    let large_output = "headless fan-in stdout line\n".repeat(2_500);
    let mut items = Vec::new();
    for index in 0..8 {
        let call_id = format!("call-{index}");
        items.push(ResponseItem::FunctionCall {
            id: None,
            name: "shell".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: call_id.clone(),
        });
        items.push(ResponseItem::FunctionCallOutput {
            call_id,
            output: FunctionCallOutputPayload::from_text(large_output.clone()),
        });
    }
    session
        .replace_history(items.clone(), /*reference_context_item*/ None)
        .await;

    // Autonomous headless-style turns bound the PROMPT clone...
    turn_context.bound_headless_tool_outputs_for_prompt = true;
    let bounded = sampling_prompt_history(&session, &turn_context).await;
    assert_ne!(items, bounded.raw_items());

    // ...while the session's stored history keeps the full tool outputs for
    // later interactive turns and compaction.
    let stored = session.clone_history().await;
    assert_eq!(items, stored.raw_items());

    // Turns without the flag build prompts from the untouched history.
    turn_context.bound_headless_tool_outputs_for_prompt = false;
    let unbounded = sampling_prompt_history(&session, &turn_context).await;
    assert_eq!(items, unbounded.raw_items());
}

#[tokio::test]
async fn oversized_sampling_prompt_is_rejected_before_streaming() {
    let (_session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.model_info.context_window = Some(128);
    turn_context.model_info.effective_context_window_percent = 100;
    turn_context.enforce_context_window_before_sampling = true;

    let prompt = Prompt {
        input: vec![user_input_text(&"large headless context\n".repeat(300))],
        base_instructions: BaseInstructions {
            text: "base".to_string(),
        },
        ..Prompt::default()
    };

    assert!(estimate_sampling_prompt_input_tokens(&prompt) > 128);
    assert!(matches!(
        reject_oversized_sampling_prompt(&prompt, &turn_context),
        Err(CodexErr::ContextWindowExceeded)
    ));
}

#[tokio::test]
async fn auto_compact_status_uses_chatgpt_capped_bundled_gpt_window() {
    let (session, _initial_turn_context, _rx_event) =
        crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
            codex_login::CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            Vec::new(),
            |config| {
                config.model = Some("gpt-5.5".to_string());
            },
        )
        .await;
    let turn_context = session.new_default_turn().await;

    assert_eq!(turn_context.model_context_window(), Some(258_400));

    let over_chatgpt_default_limit = 272_000 * 90 / 100 + 1;
    {
        let mut state = session.state.lock().await;
        state.set_token_info(Some(codex_protocol::protocol::TokenUsageInfo {
            total_token_usage: codex_protocol::protocol::TokenUsage {
                total_tokens: over_chatgpt_default_limit,
                ..Default::default()
            },
            last_token_usage: codex_protocol::protocol::TokenUsage {
                total_tokens: over_chatgpt_default_limit,
                ..Default::default()
            },
            model_context_window: turn_context.model_context_window(),
        }));
    }

    let status = auto_compact_token_status(&session, &turn_context).await;

    assert_eq!(status.active_context_tokens, over_chatgpt_default_limit);
    assert_eq!(status.auto_compact_scope_tokens, over_chatgpt_default_limit);
    assert_eq!(status.auto_compact_scope_limit, 244_800);
    assert!(status.token_limit_reached);
}

#[tokio::test]
async fn body_after_prefix_auto_compact_status_uses_chatgpt_capped_full_window() {
    let (session, _initial_turn_context, _rx_event) =
        crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
            codex_login::CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            Vec::new(),
            |config| {
                config.model = Some("gpt-5.5".to_string());
                config.model_auto_compact_token_limit = Some(999_999);
                config.model_auto_compact_token_limit_scope =
                    codex_protocol::config_types::AutoCompactTokenLimitScope::BodyAfterPrefix;
            },
        )
        .await;
    let turn_context = session.new_default_turn().await;

    assert_eq!(turn_context.model_context_window(), Some(258_400));

    {
        let mut state = session.state.lock().await;
        state.set_token_info(Some(codex_protocol::protocol::TokenUsageInfo {
            total_token_usage: codex_protocol::protocol::TokenUsage {
                total_tokens: 258_400,
                ..Default::default()
            },
            last_token_usage: codex_protocol::protocol::TokenUsage {
                total_tokens: 258_400,
                ..Default::default()
            },
            model_context_window: turn_context.model_context_window(),
        }));
    }

    let status = auto_compact_token_status(&session, &turn_context).await;

    assert_eq!(status.full_context_window_limit, Some(258_400));
    assert!(status.full_context_window_limit_reached);
    assert_eq!(status.auto_compact_scope_limit, 999_999);
    assert!(status.token_limit_reached);
}
