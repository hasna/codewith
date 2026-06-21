use super::thread_goal_processor::api_thread_goal_from_state;
use super::thread_goal_processor::api_thread_goal_plan_from_state;
use super::thread_pending_interaction_processor::api_pending_interaction;
use super::thread_pending_interaction_processor::read_pending_interaction;
use super::thread_pending_interaction_processor::validate_response_matches_interaction;
use super::thread_schedule_api::api_thread_schedule_from_state;
use super::*;
use codex_app_server_protocol::LocalSessionListParams;
use codex_app_server_protocol::MissionControlCapabilities;
use codex_app_server_protocol::MissionControlDeliveryPolicy;
use codex_app_server_protocol::MissionControlEnqueueInstructionParams;
use codex_app_server_protocol::MissionControlEnqueueInstructionResponse;
use codex_app_server_protocol::MissionControlMailboxReceiptsParams;
use codex_app_server_protocol::MissionControlMailboxReceiptsResponse;
use codex_app_server_protocol::MissionControlOverviewParams;
use codex_app_server_protocol::MissionControlOverviewResponse;
use codex_app_server_protocol::MissionControlRespondInteractionParams;
use codex_app_server_protocol::MissionControlRespondInteractionResponse;
use codex_app_server_protocol::MissionControlSession;
use codex_app_server_protocol::ThreadMailboxEnqueueParams;
use codex_app_server_protocol::ThreadMailboxMessageKind;
use codex_app_server_protocol::ThreadMailboxReceiptsListParams;
use codex_app_server_protocol::ThreadPendingInteractionListParams;
use codex_app_server_protocol::ThreadPendingInteractionRespondParams;
use serde_json::json;

const MISSION_CONTROL_PREVIEW_CHARS: usize = 240;
const DEFAULT_MISSION_CONTROL_GOAL_PLAN_LIMIT: u32 = 20;

impl ThreadRequestProcessor {
    pub(crate) async fn mission_control_overview(
        &self,
        params: MissionControlOverviewParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.mission_control_overview_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn mission_control_enqueue_instruction(
        &self,
        params: MissionControlEnqueueInstructionParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.mission_control_enqueue_instruction_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn mission_control_mailbox_receipts(
        &self,
        params: MissionControlMailboxReceiptsParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.mission_control_mailbox_receipts_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn mission_control_respond_interaction(
        &self,
        params: MissionControlRespondInteractionParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.mission_control_respond_interaction_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    async fn mission_control_overview_inner(
        &self,
        params: MissionControlOverviewParams,
    ) -> Result<MissionControlOverviewResponse, JSONRPCErrorError> {
        let MissionControlOverviewParams {
            cursor,
            limit,
            sort_key,
            sort_direction,
            cwd,
            session_statuses,
            search_term,
            pending_interaction_cursor,
            pending_interaction_limit,
            pending_interaction_statuses,
            include_goal_plans,
            use_state_db_only,
        } = params;
        let local_sessions = self
            .local_session_list_inner(LocalSessionListParams {
                cursor,
                limit,
                sort_key,
                sort_direction,
                cwd,
                statuses: session_statuses,
                search_term,
                archived: None,
                use_state_db_only,
            })
            .await?;
        let pending_interactions = self
            .thread_pending_interaction_list_inner(ThreadPendingInteractionListParams {
                thread_id: None,
                statuses: pending_interaction_statuses,
                kinds: None,
                cursor: pending_interaction_cursor,
                limit: pending_interaction_limit,
            })
            .await?;
        let goals_enabled = self.config.features.enabled(Feature::Goals);
        let scheduled_tasks_enabled = self.config.features.enabled(Feature::ScheduledTasks);
        let sessions = self
            .mission_control_sessions(
                local_sessions.data,
                include_goal_plans,
                scheduled_tasks_enabled,
            )
            .await?;
        Ok(MissionControlOverviewResponse {
            sessions,
            pending_interactions: pending_interactions.data,
            next_session_cursor: local_sessions.next_cursor,
            next_pending_interaction_cursor: pending_interactions.next_cursor,
            capabilities: mission_control_capabilities(goals_enabled, scheduled_tasks_enabled),
        })
    }

    async fn mission_control_sessions(
        &self,
        sessions: Vec<codex_app_server_protocol::LocalSession>,
        include_goal_plans: bool,
        include_schedules: bool,
    ) -> Result<Vec<MissionControlSession>, JSONRPCErrorError> {
        let mut output = Vec::with_capacity(sessions.len());
        let state_db = self.state_db.clone();
        for session in sessions {
            let Some(state_db) = state_db.as_ref() else {
                output.push(MissionControlSession {
                    session,
                    goal: None,
                    goal_plans: Vec::new(),
                    schedules: Vec::new(),
                });
                continue;
            };
            let thread_id = ThreadId::from_string(session.thread_id.as_str())
                .map_err(|err| invalid_request(format!("invalid thread id: {err}")))?;
            let goal = state_db
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .map_err(|err| internal_error(format!("failed to read thread goal: {err}")))?
                .map(api_thread_goal_from_state);
            let goal_plans = if include_goal_plans {
                state_db
                    .thread_goals()
                    .list_thread_goal_plans_page(
                        thread_id,
                        /*cursor*/ None,
                        DEFAULT_MISSION_CONTROL_GOAL_PLAN_LIMIT,
                    )
                    .await
                    .map_err(|err| {
                        internal_error(format!("failed to read thread goal plans: {err}"))
                    })?
                    .data
                    .into_iter()
                    .map(api_thread_goal_plan_from_state)
                    .collect()
            } else {
                Vec::new()
            };
            let schedules = if include_schedules {
                state_db
                    .thread_schedules()
                    .list_thread_schedules(thread_id)
                    .await
                    .map_err(|err| {
                        internal_error(format!("failed to read thread schedules: {err}"))
                    })?
                    .into_iter()
                    .map(api_thread_schedule_from_state)
                    .collect()
            } else {
                Vec::new()
            };
            output.push(MissionControlSession {
                session,
                goal,
                goal_plans,
                schedules,
            });
        }
        Ok(output)
    }

    async fn mission_control_enqueue_instruction_inner(
        &self,
        params: MissionControlEnqueueInstructionParams,
    ) -> Result<MissionControlEnqueueInstructionResponse, JSONRPCErrorError> {
        let text = validate_required_text("mission-control message", params.message)?;
        let delivery_policy = if params.resume {
            MissionControlDeliveryPolicy::ResumeAndTrigger
        } else {
            MissionControlDeliveryPolicy::LiveOnly
        };
        let preview = truncate_mission_control_preview(text.as_str());
        if params.dry_run {
            return Ok(MissionControlEnqueueInstructionResponse {
                dry_run: true,
                delivery_policy,
                preview,
                message: None,
                created: None,
            });
        }
        let payload = match delivery_policy {
            MissionControlDeliveryPolicy::LiveOnly => json!({ "text": text }),
            MissionControlDeliveryPolicy::ResumeAndTrigger => {
                json!({ "text": text, "delivery": "resumeAndTrigger" })
            }
        };
        let response = self
            .thread_mailbox_enqueue_inner(ThreadMailboxEnqueueParams {
                target_thread_id: params.target_thread_id,
                sender_thread_id: params.sender_thread_id,
                sender_label: params.sender_label,
                idempotency_key: params.idempotency_key,
                kind: ThreadMailboxMessageKind::UserInstruction,
                message: payload,
                preview: Some(preview.clone()),
                priority: params.priority,
                max_attempts: params.max_attempts,
                next_attempt_at: None,
                expires_at: params.expires_at,
            })
            .await?;
        Ok(MissionControlEnqueueInstructionResponse {
            dry_run: false,
            delivery_policy,
            preview,
            message: Some(response.message),
            created: Some(response.created),
        })
    }

    async fn mission_control_mailbox_receipts_inner(
        &self,
        params: MissionControlMailboxReceiptsParams,
    ) -> Result<MissionControlMailboxReceiptsResponse, JSONRPCErrorError> {
        let response = self
            .thread_mailbox_receipts_list_inner(ThreadMailboxReceiptsListParams {
                target_thread_id: params.target_thread_id,
                message_id: params.message_id,
            })
            .await?;
        Ok(MissionControlMailboxReceiptsResponse {
            data: response.data,
        })
    }

    async fn mission_control_respond_interaction_inner(
        &self,
        params: MissionControlRespondInteractionParams,
    ) -> Result<MissionControlRespondInteractionResponse, JSONRPCErrorError> {
        if params.dry_run {
            let state_db = self.state_db_for_pending_interactions()?;
            let thread_id = params
                .thread_id
                .as_deref()
                .map(ThreadId::from_string)
                .transpose()
                .map_err(|err| invalid_request(format!("invalid thread id: {err}")))?;
            let interaction = read_pending_interaction(
                state_db.as_ref(),
                params.interaction_id.as_str(),
                thread_id,
            )
            .await?;
            validate_response_matches_interaction(interaction.kind, &params.response)?;
            return Ok(MissionControlRespondInteractionResponse {
                dry_run: true,
                updated: false,
                interaction: Some(api_pending_interaction(interaction)),
            });
        }
        let response = self
            .thread_pending_interaction_respond_inner(ThreadPendingInteractionRespondParams {
                interaction_id: params.interaction_id,
                thread_id: params.thread_id,
                terminal_status: params.terminal_status,
                response: params.response,
            })
            .await?;
        Ok(MissionControlRespondInteractionResponse {
            dry_run: false,
            updated: response.updated,
            interaction: response.interaction,
        })
    }
}

fn mission_control_capabilities(
    goals_enabled: bool,
    scheduled_tasks_enabled: bool,
) -> MissionControlCapabilities {
    MissionControlCapabilities {
        local_sessions: true,
        durable_mailbox: true,
        pending_interactions: true,
        goals: goals_enabled,
        scheduled_tasks: scheduled_tasks_enabled,
        remote_dispatch: false,
        workflow_mutation: false,
        shell_execution: false,
        filesystem_mutation: false,
    }
}

fn truncate_mission_control_preview(value: &str) -> String {
    value
        .trim()
        .chars()
        .take(MISSION_CONTROL_PREVIEW_CHARS)
        .collect()
}

fn validate_required_text(field_name: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request(format!("{field_name} must not be empty")));
    }
    Ok(value.to_string())
}
