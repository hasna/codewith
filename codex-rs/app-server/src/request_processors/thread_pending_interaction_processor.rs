use super::*;
use codex_app_server_protocol::CommandExecutionRequestApprovalResponse;
use codex_app_server_protocol::FileChangeRequestApprovalResponse;
use codex_app_server_protocol::McpServerElicitationRequestResponse;
use codex_app_server_protocol::PermissionsRequestApprovalResponse;
use codex_app_server_protocol::ThreadPendingInteraction;
use codex_app_server_protocol::ThreadPendingInteractionEvent;
use codex_app_server_protocol::ThreadPendingInteractionEventKind;
use codex_app_server_protocol::ThreadPendingInteractionKind;
use codex_app_server_protocol::ThreadPendingInteractionListParams;
use codex_app_server_protocol::ThreadPendingInteractionListResponse;
use codex_app_server_protocol::ThreadPendingInteractionReadParams;
use codex_app_server_protocol::ThreadPendingInteractionReadResponse;
use codex_app_server_protocol::ThreadPendingInteractionRespondParams;
use codex_app_server_protocol::ThreadPendingInteractionRespondResponse;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionSourceKind;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
use codex_app_server_protocol::ToolRequestUserInputResponse;
use serde_json::json;

impl ThreadRequestProcessor {
    pub(crate) async fn thread_pending_interaction_list(
        &self,
        params: ThreadPendingInteractionListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_pending_interaction_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_pending_interaction_read(
        &self,
        params: ThreadPendingInteractionReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_pending_interaction_read_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_pending_interaction_respond(
        &self,
        params: ThreadPendingInteractionRespondParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_pending_interaction_respond_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(super) async fn thread_pending_interaction_list_inner(
        &self,
        params: ThreadPendingInteractionListParams,
    ) -> Result<ThreadPendingInteractionListResponse, JSONRPCErrorError> {
        let state_db = self.state_db_for_pending_interactions()?;
        let thread_id = params
            .thread_id
            .as_deref()
            .map(parse_pending_interaction_thread_id)
            .transpose()?;
        if let Some(thread_id) = thread_id {
            ensure_pending_interaction_thread_exists(state_db.as_ref(), thread_id).await?;
        }
        let limit = params
            .limit
            .unwrap_or(codex_state::DEFAULT_PENDING_INTERACTION_LIST_LIMIT);
        let page = state_db
            .list_thread_pending_interactions(codex_state::PendingInteractionListParams {
                thread_id,
                statuses: params
                    .statuses
                    .unwrap_or_else(|| {
                        vec![
                            ThreadPendingInteractionStatus::Pending,
                            ThreadPendingInteractionStatus::Delivered,
                        ]
                    })
                    .into_iter()
                    .map(api_pending_interaction_status_to_state)
                    .collect(),
                kinds: params
                    .kinds
                    .unwrap_or_default()
                    .into_iter()
                    .map(api_pending_interaction_kind_to_state)
                    .collect(),
                cursor: params.cursor,
                limit,
            })
            .await
            .map_err(pending_interaction_store_error)?;
        Ok(ThreadPendingInteractionListResponse {
            data: page.data.into_iter().map(api_pending_interaction).collect(),
            next_cursor: page.next_cursor,
        })
    }

    pub(super) async fn thread_pending_interaction_read_inner(
        &self,
        params: ThreadPendingInteractionReadParams,
    ) -> Result<ThreadPendingInteractionReadResponse, JSONRPCErrorError> {
        let state_db = self.state_db_for_pending_interactions()?;
        let thread_id = params
            .thread_id
            .as_deref()
            .map(parse_pending_interaction_thread_id)
            .transpose()?;
        let interaction =
            read_pending_interaction(state_db.as_ref(), params.interaction_id.as_str(), thread_id)
                .await?;
        let events = state_db
            .list_thread_pending_interaction_events(interaction.interaction_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to read pending interaction events: {err}"))
            })?;
        Ok(ThreadPendingInteractionReadResponse {
            interaction: api_pending_interaction(interaction),
            events: events
                .into_iter()
                .map(api_pending_interaction_event)
                .collect(),
        })
    }

    pub(super) async fn thread_pending_interaction_respond_inner(
        &self,
        params: ThreadPendingInteractionRespondParams,
    ) -> Result<ThreadPendingInteractionRespondResponse, JSONRPCErrorError> {
        let state_db = self.state_db_for_pending_interactions()?;
        let thread_id = params
            .thread_id
            .as_deref()
            .map(parse_pending_interaction_thread_id)
            .transpose()?;
        let interaction =
            read_pending_interaction(state_db.as_ref(), params.interaction_id.as_str(), thread_id)
                .await?;
        validate_response_matches_interaction(interaction.kind, &params.response)?;
        let response_result = response_payload_to_result(&params.response)?;
        let stored_response = redacted_response_payload(&params.response);
        let terminal_status =
            api_pending_interaction_terminal_status_to_state(params.terminal_status);

        let routed = if terminal_status == codex_state::PendingInteractionStatus::Responded
            || matches!(
                (&params.response, terminal_status),
                (
                    ThreadPendingInteractionResponsePayload::CommandApproval { .. }
                        | ThreadPendingInteractionResponsePayload::FileChangeApproval { .. }
                        | ThreadPendingInteractionResponsePayload::RequestUserInput { .. }
                        | ThreadPendingInteractionResponsePayload::McpElicitation { .. }
                        | ThreadPendingInteractionResponsePayload::PermissionsApproval { .. }
                        | ThreadPendingInteractionResponsePayload::DynamicTool { .. },
                    _
                )
            ) {
            match interaction
                .server_request_id_json
                .clone()
                .map(serde_json::from_value::<RequestId>)
                .transpose()
                .map_err(|err| internal_error(format!("invalid stored server request id: {err}")))?
            {
                Some(request_id) => {
                    self.outgoing
                        .notify_client_response(request_id, response_result)
                        .await
                }
                None => false,
            }
        } else {
            false
        };

        let terminal_response = matches!(
            &params.response,
            ThreadPendingInteractionResponsePayload::Terminal { .. }
        );
        let final_status = if routed
            || terminal_response
            || matches!(
                terminal_status,
                codex_state::PendingInteractionStatus::Cancelled
                    | codex_state::PendingInteractionStatus::Denied
                    | codex_state::PendingInteractionStatus::Expired
            ) {
            terminal_status
        } else {
            codex_state::PendingInteractionStatus::NoLongerWaiting
        };

        let updated = state_db
            .respond_thread_pending_interaction(&codex_state::PendingInteractionRespondParams {
                interaction_id: interaction.interaction_id.clone(),
                response_payload_json: stored_response.payload,
                response_payload_preview: stored_response.preview,
                response_redactions_json: json!(stored_response.redactions),
                terminal_status: final_status,
            })
            .await
            .map_err(|err| {
                internal_error(format!("failed to respond to pending interaction: {err}"))
            })?;
        let interaction = state_db
            .get_thread_pending_interaction(interaction.interaction_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to reload pending interaction: {err}")))?
            .map(api_pending_interaction);

        Ok(ThreadPendingInteractionRespondResponse {
            updated,
            interaction,
        })
    }

    pub(super) fn state_db_for_pending_interactions(
        &self,
    ) -> Result<StateDbHandle, JSONRPCErrorError> {
        self.state_db
            .clone()
            .ok_or_else(|| internal_error("sqlite state db unavailable for pending interactions"))
    }
}

pub(super) struct RedactedResponsePayload {
    pub(super) payload: serde_json::Value,
    pub(super) preview: String,
    pub(super) redactions: Vec<String>,
}

pub(super) async fn read_pending_interaction(
    state_db: &codex_state::StateRuntime,
    interaction_id: &str,
    thread_id: Option<ThreadId>,
) -> Result<codex_state::PendingInteraction, JSONRPCErrorError> {
    let interaction = state_db
        .get_thread_pending_interaction(interaction_id)
        .await
        .map_err(|err| internal_error(format!("failed to read pending interaction: {err}")))?
        .ok_or_else(|| {
            invalid_request(format!("pending interaction not found: {interaction_id}"))
        })?;
    if let Some(thread_id) = thread_id
        && interaction.thread_id != thread_id
    {
        return Err(invalid_request(format!(
            "pending interaction not found: {interaction_id}"
        )));
    }
    Ok(interaction)
}

fn parse_pending_interaction_thread_id(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}

async fn ensure_pending_interaction_thread_exists(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<(), JSONRPCErrorError> {
    state_db
        .get_thread(thread_id)
        .await
        .map_err(|err| internal_error(format!("failed to read thread metadata: {err}")))?
        .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?;
    Ok(())
}

pub(super) fn validate_response_matches_interaction(
    kind: codex_state::PendingInteractionKind,
    response: &ThreadPendingInteractionResponsePayload,
) -> Result<(), JSONRPCErrorError> {
    let matches_kind = matches!(
        (kind, response),
        (
            codex_state::PendingInteractionKind::CommandApproval,
            ThreadPendingInteractionResponsePayload::CommandApproval { .. }
        ) | (
            codex_state::PendingInteractionKind::FileChangeApproval,
            ThreadPendingInteractionResponsePayload::FileChangeApproval { .. }
        ) | (
            codex_state::PendingInteractionKind::UserInput,
            ThreadPendingInteractionResponsePayload::RequestUserInput { .. }
        ) | (
            codex_state::PendingInteractionKind::McpElicitation,
            ThreadPendingInteractionResponsePayload::McpElicitation { .. }
        ) | (
            codex_state::PendingInteractionKind::PermissionGrant,
            ThreadPendingInteractionResponsePayload::PermissionsApproval { .. }
        ) | (
            codex_state::PendingInteractionKind::DynamicTool,
            ThreadPendingInteractionResponsePayload::DynamicTool { .. }
        ) | (
            codex_state::PendingInteractionKind::UsageLimit
                | codex_state::PendingInteractionKind::ProfileSwitch
                | codex_state::PendingInteractionKind::Blocked,
            ThreadPendingInteractionResponsePayload::Terminal { .. }
        )
    );
    if matches_kind {
        Ok(())
    } else {
        Err(invalid_request(
            "pending interaction response kind mismatch",
        ))
    }
}

fn response_payload_to_result(
    response: &ThreadPendingInteractionResponsePayload,
) -> Result<serde_json::Value, JSONRPCErrorError> {
    match response {
        ThreadPendingInteractionResponsePayload::CommandApproval { decision } => {
            serde_json::to_value(CommandExecutionRequestApprovalResponse {
                decision: decision.clone(),
            })
        }
        ThreadPendingInteractionResponsePayload::FileChangeApproval { decision } => {
            serde_json::to_value(FileChangeRequestApprovalResponse {
                decision: decision.clone(),
            })
        }
        ThreadPendingInteractionResponsePayload::RequestUserInput { answers } => {
            serde_json::to_value(ToolRequestUserInputResponse {
                answers: answers.clone(),
            })
        }
        ThreadPendingInteractionResponsePayload::McpElicitation {
            action,
            content,
            meta,
        } => serde_json::to_value(McpServerElicitationRequestResponse {
            action: *action,
            content: content.clone(),
            meta: meta.clone(),
        }),
        ThreadPendingInteractionResponsePayload::PermissionsApproval {
            permissions,
            scope,
            strict_auto_review,
        } => serde_json::to_value(PermissionsRequestApprovalResponse {
            permissions: permissions.clone(),
            scope: *scope,
            strict_auto_review: *strict_auto_review,
        }),
        ThreadPendingInteractionResponsePayload::DynamicTool {
            content_items,
            success,
        } => serde_json::to_value(codex_app_server_protocol::DynamicToolCallResponse {
            content_items: content_items.clone(),
            success: *success,
        }),
        ThreadPendingInteractionResponsePayload::Terminal { reason } => {
            Ok(json!({ "reason": reason }))
        }
    }
    .map_err(|err| invalid_request(format!("pending interaction response is invalid: {err}")))
}

pub(super) fn redacted_response_payload(
    response: &ThreadPendingInteractionResponsePayload,
) -> RedactedResponsePayload {
    match response {
        ThreadPendingInteractionResponsePayload::CommandApproval { decision } => {
            RedactedResponsePayload {
                payload: json!({"type": "commandApproval", "decision": decision}),
                preview: format!("command approval: {decision:?}"),
                redactions: Vec::new(),
            }
        }
        ThreadPendingInteractionResponsePayload::FileChangeApproval { decision } => {
            RedactedResponsePayload {
                payload: json!({"type": "fileChangeApproval", "decision": decision}),
                preview: format!("file change approval: {decision:?}"),
                redactions: Vec::new(),
            }
        }
        ThreadPendingInteractionResponsePayload::RequestUserInput { answers } => {
            RedactedResponsePayload {
                payload: json!({
                    "type": "requestUserInput",
                    "answerCount": answers.len(),
                }),
                preview: format!("{} user input answer(s)", answers.len()),
                redactions: vec!["responsePayload".to_string()],
            }
        }
        ThreadPendingInteractionResponsePayload::McpElicitation {
            action,
            content,
            meta,
        } => RedactedResponsePayload {
            payload: json!({
                "type": "mcpElicitation",
                "action": action,
                "contentRedacted": content.is_some(),
                "metaRedacted": meta.is_some(),
            }),
            preview: format!("MCP elicitation: {action:?}"),
            redactions: vec!["responsePayload".to_string()],
        },
        ThreadPendingInteractionResponsePayload::PermissionsApproval {
            permissions,
            scope,
            strict_auto_review,
        } => RedactedResponsePayload {
            payload: json!({
                "type": "permissionsApproval",
                "permissions": permissions,
                "scope": scope,
                "strictAutoReview": strict_auto_review,
            }),
            preview: format!("permissions approval: {scope:?}"),
            redactions: Vec::new(),
        },
        ThreadPendingInteractionResponsePayload::DynamicTool { success, .. } => {
            RedactedResponsePayload {
                payload: json!({
                    "type": "dynamicTool",
                    "success": success,
                }),
                preview: format!("dynamic tool response: success={success}"),
                redactions: vec!["responsePayload".to_string()],
            }
        }
        ThreadPendingInteractionResponsePayload::Terminal { reason } => RedactedResponsePayload {
            payload: json!({"type": "terminal", "reason": reason}),
            preview: truncate_pending_interaction_preview(reason),
            redactions: Vec::new(),
        },
    }
}

pub(super) fn api_pending_interaction(
    interaction: codex_state::PendingInteraction,
) -> ThreadPendingInteraction {
    ThreadPendingInteraction {
        interaction_id: interaction.interaction_id,
        thread_id: interaction.thread_id.to_string(),
        source_kind: state_pending_interaction_source_to_api(interaction.source_kind),
        source_id: interaction.source_id,
        turn_id: interaction.turn_id,
        worker_request_id: interaction.worker_request_id,
        kind: state_pending_interaction_kind_to_api(interaction.kind),
        status: state_pending_interaction_status_to_api(interaction.status),
        request_payload: interaction.request_payload_json,
        request_payload_sha256: interaction.request_payload_sha256,
        request_payload_preview: interaction.request_payload_preview,
        request_redactions: string_array_from_value(interaction.request_redactions_json),
        response_payload: interaction.response_payload_json,
        response_payload_sha256: interaction.response_payload_sha256,
        response_payload_preview: interaction.response_payload_preview,
        response_redactions: interaction
            .response_redactions_json
            .map(string_array_from_value)
            .unwrap_or_default(),
        no_client_policy: interaction.no_client_policy,
        timeout_at: interaction
            .timeout_at
            .map(|timestamp| timestamp.timestamp()),
        created_at: interaction.created_at.timestamp(),
        delivered_at: interaction
            .delivered_at
            .map(|timestamp| timestamp.timestamp()),
        responded_at: interaction
            .responded_at
            .map(|timestamp| timestamp.timestamp()),
        terminal_at: interaction
            .terminal_at
            .map(|timestamp| timestamp.timestamp()),
        updated_at: interaction.updated_at.timestamp(),
    }
}

pub(super) fn api_pending_interaction_event(
    event: codex_state::PendingInteractionEvent,
) -> ThreadPendingInteractionEvent {
    ThreadPendingInteractionEvent {
        event_id: event.event_id,
        interaction_id: event.interaction_id,
        thread_id: event.thread_id.to_string(),
        event_kind: state_pending_interaction_event_kind_to_api(event.event_kind),
        status: state_pending_interaction_status_to_api(event.status),
        payload: event.payload_json,
        payload_sha256: event.payload_sha256,
        payload_preview: event.payload_preview,
        redactions: string_array_from_value(event.redactions_json),
        created_at: event.created_at.timestamp(),
    }
}

fn state_pending_interaction_event_kind_to_api(
    kind: codex_state::PendingInteractionEventKind,
) -> ThreadPendingInteractionEventKind {
    match kind {
        codex_state::PendingInteractionEventKind::Created => {
            ThreadPendingInteractionEventKind::Created
        }
        codex_state::PendingInteractionEventKind::Delivered => {
            ThreadPendingInteractionEventKind::Delivered
        }
        codex_state::PendingInteractionEventKind::Responded => {
            ThreadPendingInteractionEventKind::Responded
        }
        codex_state::PendingInteractionEventKind::Expired => {
            ThreadPendingInteractionEventKind::Expired
        }
        codex_state::PendingInteractionEventKind::Cancelled => {
            ThreadPendingInteractionEventKind::Cancelled
        }
        codex_state::PendingInteractionEventKind::Denied => {
            ThreadPendingInteractionEventKind::Denied
        }
        codex_state::PendingInteractionEventKind::NoLongerWaiting => {
            ThreadPendingInteractionEventKind::NoLongerWaiting
        }
    }
}

fn string_array_from_value(value: serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn api_pending_interaction_kind_to_state(
    kind: ThreadPendingInteractionKind,
) -> codex_state::PendingInteractionKind {
    match kind {
        ThreadPendingInteractionKind::CommandApproval => {
            codex_state::PendingInteractionKind::CommandApproval
        }
        ThreadPendingInteractionKind::FileChangeApproval => {
            codex_state::PendingInteractionKind::FileChangeApproval
        }
        ThreadPendingInteractionKind::UserInput => codex_state::PendingInteractionKind::UserInput,
        ThreadPendingInteractionKind::McpElicitation => {
            codex_state::PendingInteractionKind::McpElicitation
        }
        ThreadPendingInteractionKind::PermissionGrant => {
            codex_state::PendingInteractionKind::PermissionGrant
        }
        ThreadPendingInteractionKind::DynamicTool => {
            codex_state::PendingInteractionKind::DynamicTool
        }
        ThreadPendingInteractionKind::UsageLimit => codex_state::PendingInteractionKind::UsageLimit,
        ThreadPendingInteractionKind::ProfileSwitch => {
            codex_state::PendingInteractionKind::ProfileSwitch
        }
        ThreadPendingInteractionKind::Blocked => codex_state::PendingInteractionKind::Blocked,
    }
}

fn state_pending_interaction_kind_to_api(
    kind: codex_state::PendingInteractionKind,
) -> ThreadPendingInteractionKind {
    match kind {
        codex_state::PendingInteractionKind::CommandApproval => {
            ThreadPendingInteractionKind::CommandApproval
        }
        codex_state::PendingInteractionKind::FileChangeApproval => {
            ThreadPendingInteractionKind::FileChangeApproval
        }
        codex_state::PendingInteractionKind::UserInput => ThreadPendingInteractionKind::UserInput,
        codex_state::PendingInteractionKind::McpElicitation => {
            ThreadPendingInteractionKind::McpElicitation
        }
        codex_state::PendingInteractionKind::PermissionGrant => {
            ThreadPendingInteractionKind::PermissionGrant
        }
        codex_state::PendingInteractionKind::DynamicTool => {
            ThreadPendingInteractionKind::DynamicTool
        }
        codex_state::PendingInteractionKind::UsageLimit => ThreadPendingInteractionKind::UsageLimit,
        codex_state::PendingInteractionKind::ProfileSwitch => {
            ThreadPendingInteractionKind::ProfileSwitch
        }
        codex_state::PendingInteractionKind::Blocked => ThreadPendingInteractionKind::Blocked,
    }
}

pub(super) fn api_pending_interaction_status_to_state(
    status: ThreadPendingInteractionStatus,
) -> codex_state::PendingInteractionStatus {
    match status {
        ThreadPendingInteractionStatus::Pending => codex_state::PendingInteractionStatus::Pending,
        ThreadPendingInteractionStatus::Delivered => {
            codex_state::PendingInteractionStatus::Delivered
        }
        ThreadPendingInteractionStatus::Responded => {
            codex_state::PendingInteractionStatus::Responded
        }
        ThreadPendingInteractionStatus::Expired => codex_state::PendingInteractionStatus::Expired,
        ThreadPendingInteractionStatus::Cancelled => {
            codex_state::PendingInteractionStatus::Cancelled
        }
        ThreadPendingInteractionStatus::Denied => codex_state::PendingInteractionStatus::Denied,
        ThreadPendingInteractionStatus::NoLongerWaiting => {
            codex_state::PendingInteractionStatus::NoLongerWaiting
        }
    }
}

fn state_pending_interaction_status_to_api(
    status: codex_state::PendingInteractionStatus,
) -> ThreadPendingInteractionStatus {
    match status {
        codex_state::PendingInteractionStatus::Pending => ThreadPendingInteractionStatus::Pending,
        codex_state::PendingInteractionStatus::Delivered => {
            ThreadPendingInteractionStatus::Delivered
        }
        codex_state::PendingInteractionStatus::Responded => {
            ThreadPendingInteractionStatus::Responded
        }
        codex_state::PendingInteractionStatus::Expired => ThreadPendingInteractionStatus::Expired,
        codex_state::PendingInteractionStatus::Cancelled => {
            ThreadPendingInteractionStatus::Cancelled
        }
        codex_state::PendingInteractionStatus::Denied => ThreadPendingInteractionStatus::Denied,
        codex_state::PendingInteractionStatus::NoLongerWaiting => {
            ThreadPendingInteractionStatus::NoLongerWaiting
        }
    }
}

pub(super) fn api_pending_interaction_terminal_status_to_state(
    status: ThreadPendingInteractionTerminalStatus,
) -> codex_state::PendingInteractionStatus {
    match status {
        ThreadPendingInteractionTerminalStatus::Responded => {
            codex_state::PendingInteractionStatus::Responded
        }
        ThreadPendingInteractionTerminalStatus::Expired => {
            codex_state::PendingInteractionStatus::Expired
        }
        ThreadPendingInteractionTerminalStatus::Cancelled => {
            codex_state::PendingInteractionStatus::Cancelled
        }
        ThreadPendingInteractionTerminalStatus::Denied => {
            codex_state::PendingInteractionStatus::Denied
        }
        ThreadPendingInteractionTerminalStatus::NoLongerWaiting => {
            codex_state::PendingInteractionStatus::NoLongerWaiting
        }
    }
}

fn state_pending_interaction_source_to_api(
    source: codex_state::PendingInteractionSourceKind,
) -> ThreadPendingInteractionSourceKind {
    match source {
        codex_state::PendingInteractionSourceKind::Thread => {
            ThreadPendingInteractionSourceKind::Thread
        }
        codex_state::PendingInteractionSourceKind::BackgroundAgent => {
            ThreadPendingInteractionSourceKind::BackgroundAgent
        }
        codex_state::PendingInteractionSourceKind::Goal => ThreadPendingInteractionSourceKind::Goal,
        codex_state::PendingInteractionSourceKind::UsageProfile => {
            ThreadPendingInteractionSourceKind::UsageProfile
        }
    }
}

fn pending_interaction_store_error(err: anyhow::Error) -> JSONRPCErrorError {
    let message = err.to_string();
    if message.contains("pending interaction list limit")
        || message.contains("invalid pending interaction cursor")
    {
        invalid_request(message)
    } else {
        internal_error(format!("pending interaction store failed: {err}"))
    }
}

fn truncate_pending_interaction_preview(value: &str) -> String {
    value.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::DynamicToolCallOutputContentItem;
    use codex_app_server_protocol::ToolRequestUserInputAnswer;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    #[test]
    fn redacted_response_payload_omits_user_input_answers() {
        let redacted =
            redacted_response_payload(&ThreadPendingInteractionResponsePayload::RequestUserInput {
                answers: HashMap::from([(
                    "decision".to_string(),
                    ToolRequestUserInputAnswer {
                        answers: vec!["ship it".to_string()],
                    },
                )]),
            });

        assert_eq!(
            redacted.payload,
            json!({
                "type": "requestUserInput",
                "answerCount": 1,
            })
        );
        assert_eq!(redacted.preview, "1 user input answer(s)");
        assert_eq!(redacted.redactions, vec!["responsePayload".to_string()]);
    }

    #[test]
    fn redacted_response_payload_omits_dynamic_tool_output() {
        let redacted =
            redacted_response_payload(&ThreadPendingInteractionResponsePayload::DynamicTool {
                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                    text: "secret output".to_string(),
                }],
                success: true,
            });

        assert_eq!(
            redacted.payload,
            json!({
                "type": "dynamicTool",
                "success": true,
            })
        );
        assert_eq!(redacted.preview, "dynamic tool response: success=true");
        assert_eq!(redacted.redactions, vec!["responsePayload".to_string()]);
    }
}
