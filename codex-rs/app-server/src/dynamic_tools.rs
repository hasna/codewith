use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallResponse;
use codex_core::CodexThread;
use codex_protocol::dynamic_tools::DynamicToolCallOutputContentItem as CoreDynamicToolCallOutputContentItem;
use codex_protocol::dynamic_tools::DynamicToolResponse as CoreDynamicToolResponse;
use codex_protocol::protocol::Op;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::error;
use tracing::warn;

use crate::outgoing_message::ClientRequestResult;
use crate::server_request_error::is_turn_transition_server_request_error;

pub(crate) async fn on_call_response(
    call_id: String,
    receiver: oneshot::Receiver<ClientRequestResult>,
    conversation: Arc<CodexThread>,
    pending_interaction_id: Option<String>,
) {
    let response = receiver.await;
    let (response, terminal_status) = match response {
        Ok(Ok(value)) => {
            let (response, error) = decode_response(value);
            let terminal_status = if error.is_some() {
                codex_state::PendingInteractionStatus::Cancelled
            } else {
                codex_state::PendingInteractionStatus::Responded
            };
            (response, terminal_status)
        }
        Ok(Err(err)) if is_turn_transition_server_request_error(&err) => {
            terminalize_dynamic_tool_pending_interaction(
                conversation.as_ref(),
                pending_interaction_id,
                json!({
                    "type": "dynamicTool",
                    "reason": "client is no longer waiting",
                }),
                "dynamic tool call is no longer waiting".to_string(),
                Vec::new(),
                codex_state::PendingInteractionStatus::NoLongerWaiting,
            )
            .await;
            return;
        }
        Ok(Err(err)) => {
            error!("request failed with client error: {err:?}");
            let (response, _) = fallback_response("dynamic tool request failed");
            (response, codex_state::PendingInteractionStatus::Cancelled)
        }
        Err(err) => {
            error!("request failed: {err:?}");
            let (response, _) = fallback_response("dynamic tool request failed");
            (response, codex_state::PendingInteractionStatus::Cancelled)
        }
    };
    terminalize_dynamic_tool_pending_interaction(
        conversation.as_ref(),
        pending_interaction_id,
        json!({
            "type": "dynamicTool",
            "success": response.success,
            "contentItemCount": response.content_items.len(),
            "contentItemsRedacted": true,
        }),
        format!("dynamic tool response: success={}", response.success),
        vec!["responsePayload".to_string()],
        terminal_status,
    )
    .await;

    let DynamicToolCallResponse {
        content_items,
        success,
    } = response.clone();
    let core_response = CoreDynamicToolResponse {
        content_items: content_items
            .into_iter()
            .map(CoreDynamicToolCallOutputContentItem::from)
            .collect(),
        success,
    };
    if let Err(err) = conversation
        .submit(Op::DynamicToolResponse {
            id: call_id.clone(),
            response: core_response,
        })
        .await
    {
        error!("failed to submit DynamicToolResponse: {err}");
    }
}

async fn terminalize_dynamic_tool_pending_interaction(
    conversation: &CodexThread,
    interaction_id: Option<String>,
    response_payload_json: serde_json::Value,
    response_payload_preview: String,
    response_redactions: Vec<String>,
    terminal_status: codex_state::PendingInteractionStatus,
) {
    let Some(interaction_id) = interaction_id else {
        return;
    };
    let Some(state_db) = conversation.state_db() else {
        return;
    };
    if let Err(err) = state_db
        .respond_thread_pending_interaction(&codex_state::PendingInteractionRespondParams {
            interaction_id,
            response_payload_json,
            response_payload_preview,
            response_redactions_json: json!(response_redactions),
            terminal_status,
        })
        .await
    {
        warn!("failed to terminalize dynamic tool pending interaction: {err}");
    }
}

fn decode_response(value: serde_json::Value) -> (DynamicToolCallResponse, Option<String>) {
    match serde_json::from_value::<DynamicToolCallResponse>(value) {
        Ok(response) => (response, None),
        Err(err) => {
            error!("failed to deserialize DynamicToolCallResponse: {err}");
            fallback_response("dynamic tool response was invalid")
        }
    }
}

fn fallback_response(message: &str) -> (DynamicToolCallResponse, Option<String>) {
    (
        DynamicToolCallResponse {
            content_items: vec![DynamicToolCallOutputContentItem::InputText {
                text: message.to_string(),
            }],
            success: false,
        },
        Some(message.to_string()),
    )
}
