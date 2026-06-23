use super::MAX_USER_INPUT_TEXT_CHARS;
use super::sqlite_retry::retry_transient_sqlite_busy;
use super::worktree_paths::path_to_api_string;
use super::worktree_paths::paths_equivalent;
use crate::error_code::INPUT_TOO_LARGE_ERROR_CODE;
use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::error_code::invalid_request;
use crate::error_code::overloaded;
use codex_app_server_protocol::AgentAttachParams;
use codex_app_server_protocol::AgentAttachResponse;
use codex_app_server_protocol::AgentDaemonDiagnosticsResponse;
use codex_app_server_protocol::AgentDeleteParams;
use codex_app_server_protocol::AgentDeleteResponse;
use codex_app_server_protocol::AgentDesiredState;
use codex_app_server_protocol::AgentDetachParams;
use codex_app_server_protocol::AgentDetachResponse;
use codex_app_server_protocol::AgentEvent;
use codex_app_server_protocol::AgentEventsListParams;
use codex_app_server_protocol::AgentEventsListResponse;
use codex_app_server_protocol::AgentExecutionContextParams;
use codex_app_server_protocol::AgentExecutionSnapshot;
use codex_app_server_protocol::AgentLifecycleEffect;
use codex_app_server_protocol::AgentListParams;
use codex_app_server_protocol::AgentListResponse;
use codex_app_server_protocol::AgentPendingInteraction;
use codex_app_server_protocol::AgentPendingInteractionKind;
use codex_app_server_protocol::AgentPendingInteractionRespondParams;
use codex_app_server_protocol::AgentPendingInteractionRespondResponse;
use codex_app_server_protocol::AgentPendingInteractionStatus;
use codex_app_server_protocol::AgentPendingInteractionTerminalStatus;
use codex_app_server_protocol::AgentReadParams;
use codex_app_server_protocol::AgentReadResponse;
use codex_app_server_protocol::AgentRetentionState;
use codex_app_server_protocol::AgentRun;
use codex_app_server_protocol::AgentRunStatus;
use codex_app_server_protocol::AgentRunStatusCount;
use codex_app_server_protocol::AgentStartParams;
use codex_app_server_protocol::AgentStartResponse;
use codex_app_server_protocol::AgentStatusSnapshot;
use codex_app_server_protocol::AgentStopParams;
use codex_app_server_protocol::AgentStopResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::Worktree;
use codex_app_server_protocol::WorktreeAttachParams;
use codex_app_server_protocol::WorktreeAttachResponse;
use codex_app_server_protocol::WorktreeCleanupPolicy;
use codex_app_server_protocol::WorktreeDetachParams;
use codex_app_server_protocol::WorktreeDetachResponse;
use codex_app_server_protocol::WorktreeLifecycleStatus;
use codex_app_server_protocol::WorktreeListParams;
use codex_app_server_protocol::WorktreeListResponse;
use codex_app_server_protocol::WorktreeMergeCandidate;
use codex_app_server_protocol::WorktreeMergeCandidateStatus;
use codex_app_server_protocol::WorktreeMode;
use codex_app_server_protocol::WorktreeOwnerKind;
use codex_app_server_protocol::WorktreePolicy;
use codex_app_server_protocol::WorktreeReadParams;
use codex_app_server_protocol::WorktreeReadResponse;
use codex_background_agent::AgentEventJournal;
use codex_background_agent::AgentRunStore;
use codex_background_agent::AgentSnapshotStore;
use codex_background_agent::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;
use codex_background_agent::BackgroundAgentDesiredState;
use codex_background_agent::BackgroundAgentEvent;
use codex_background_agent::BackgroundAgentExecutionSnapshot;
use codex_background_agent::BackgroundAgentExecutionSnapshotParams;
use codex_background_agent::BackgroundAgentPendingInteraction;
use codex_background_agent::BackgroundAgentPendingInteractionKind;
use codex_background_agent::BackgroundAgentPendingInteractionStatus;
use codex_background_agent::BackgroundAgentRun;
use codex_background_agent::BackgroundAgentRunCreateParams;
use codex_background_agent::BackgroundAgentRunStatus;
use codex_background_agent::BackgroundAgentStatusSnapshot;
use codex_background_agent::BackgroundAgentStatusSnapshotParams;
use codex_background_agent::LifecycleAction;
use codex_background_agent::LifecycleEffect;
use codex_background_agent::PendingInteractionLedger;
use codex_background_agent::lifecycle_effect_for;
use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_rollout::StateDbHandle;
use codex_state::ManagedWorktreeAssignmentTarget;
use codex_state::ManagedWorktreeAttachParams;
use codex_state::ManagedWorktreeDetachParams;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use uuid::Uuid;

const DEFAULT_AGENT_LIST_LIMIT: usize = 50;
const MAX_AGENT_LIST_LIMIT: usize = 200;
const DEFAULT_MAX_ACTIVE_AGENT_RUNS_PER_USER: i64 = 8;
const AGENT_BACKPRESSURE_ACTIVE_RUN_LIMIT: &str = "active_run_limit";
const AGENT_EVENT_CURSOR_PREFIX: &str = "event:";
static AGENT_START_ADMISSION_LOCK: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Clone)]
pub(crate) struct BackgroundAgentRequestProcessor {
    state_db: Option<StateDbHandle>,
}

impl BackgroundAgentRequestProcessor {
    pub(crate) fn new(state_db: Option<StateDbHandle>) -> Self {
        Self { state_db }
    }

    pub(super) async fn agent_start_inner(
        &self,
        params: AgentStartParams,
    ) -> Result<AgentStartResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let AgentStartParams {
            prompt,
            cwd,
            idempotency_key,
            request_id,
            source,
            prompt_snapshot_ref,
            input_snapshot_ref,
            thread_id,
            thread_store_kind,
            thread_store_id,
            rollout_path,
            parent_thread_id,
            parent_agent_run_id,
            spawn_linkage,
            auth_profile_ref,
            config_fingerprint,
            version_fingerprint,
            execution_context,
        } = params;
        let execution_context = execution_context.map(|context| *context);
        let prompt = validate_agent_prompt(prompt)?;
        let mut existing_run = match idempotency_key.as_deref() {
            Some(idempotency_key) => state_db
                .get_run_by_idempotency_key(idempotency_key)
                .await
                .map_err(|err| {
                    internal_error(format!(
                        "failed to load background agent idempotency key: {err}"
                    ))
                })?,
            None => None,
        };
        let _admission_permit = if existing_run.is_none() {
            let permit = AGENT_START_ADMISSION_LOCK.acquire().await.map_err(|err| {
                internal_error(format!(
                    "failed to acquire background agent admission permit: {err}"
                ))
            })?;
            if let Some(idempotency_key) = idempotency_key.as_deref() {
                existing_run = state_db
                    .get_run_by_idempotency_key(idempotency_key)
                    .await
                    .map_err(|err| {
                        internal_error(format!(
                            "failed to load background agent idempotency key: {err}"
                        ))
                    })?;
            }
            Some(permit)
        } else {
            None
        };
        let new_run_requested = existing_run.is_none();
        if new_run_requested {
            let quota = load_agent_quota_snapshot(state_db.as_ref()).await?;
            if !quota.admission_allowed() {
                return Err(overloaded(format!(
                    "background agent queue is overloaded: {} active run(s), max {}",
                    quota.active_run_count, quota.max_active_runs_per_user
                )));
            }
        }
        let agent_id = Uuid::now_v7().to_string();
        let prompt_snapshot_ref =
            prompt_snapshot_ref.unwrap_or_else(|| format!("inline:{agent_id}:prompt"));
        let source = source.unwrap_or_else(|| "app-server".to_string());
        let thread_store_kind = thread_store_kind.unwrap_or_else(|| "background-agent".to_string());
        validate_agent_start_rollout_path(
            state_db.as_ref(),
            thread_id.as_deref(),
            rollout_path.as_deref(),
        )
        .await?;
        let recovery_policy = execution_context
            .as_ref()
            .and_then(|context| context.recovery_policy.clone())
            .unwrap_or_else(|| "abort_mid_turn_resume_at_safe_boundary".to_string());
        let run = match existing_run {
            Some(run) => run,
            None => state_db
                .create_run(BackgroundAgentRunCreateParams {
                    id: agent_id.clone(),
                    idempotency_key,
                    request_id,
                    source,
                    prompt_snapshot_ref,
                    input_snapshot_ref,
                    thread_id,
                    thread_store_kind,
                    thread_store_id,
                    rollout_path,
                    parent_thread_id,
                    parent_agent_run_id,
                    spawn_linkage_json: spawn_linkage,
                    auth_profile_ref,
                    status_reason: Some("queued for background-agent supervisor".to_string()),
                    config_fingerprint,
                    version_fingerprint,
                })
                .await
                .map_err(|err| {
                    internal_error(format!("failed to create background agent: {err}"))
                })?,
        };
        let created_new_run = run.id == agent_id;
        let execution_payload = initial_execution_snapshot_payload(
            &run,
            InitialExecutionSnapshotPayloadParams {
                cwd: cwd.as_deref(),
                execution_context: execution_context.as_ref(),
                recovery_policy: recovery_policy.as_str(),
            },
        );
        let execution_snapshot = if created_new_run {
            state_db
                .create_execution_snapshot(BackgroundAgentExecutionSnapshotParams {
                    run_id: run.id.clone(),
                    snapshot_kind: "initial_execution_context".to_string(),
                    payload_json: execution_payload,
                    recovery_policy: recovery_policy.clone(),
                    config_fingerprint: run.config_fingerprint.clone(),
                })
                .await
                .map_err(|err| {
                    internal_error(format!(
                        "failed to create background agent execution snapshot: {err}"
                    ))
                })?
        } else {
            match state_db
                .get_latest_execution_snapshot(run.id.as_str())
                .await
                .map_err(|err| {
                    internal_error(format!(
                        "failed to load background agent execution snapshot: {err}"
                    ))
                })? {
                Some(snapshot) => snapshot,
                None => state_db
                    .create_execution_snapshot(BackgroundAgentExecutionSnapshotParams {
                        run_id: run.id.clone(),
                        snapshot_kind: "initial_execution_context".to_string(),
                        payload_json: execution_payload,
                        recovery_policy: recovery_policy.clone(),
                        config_fingerprint: run.config_fingerprint.clone(),
                    })
                    .await
                    .map_err(|err| {
                        internal_error(format!(
                            "failed to create background agent execution snapshot: {err}"
                        ))
                    })?,
            }
        };
        let event = if created_new_run {
            append_background_agent_event_with_retry(
                state_db.as_ref(),
                run.id.as_str(),
                "agent.started",
                &json!({
                    "cwd": cwd,
                    "prompt": prompt,
                    "promptSnapshotRef": run.prompt_snapshot_ref.as_str(),
                }),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to append background agent event: {err}"))
            })?
        } else {
            let mut events = state_db
                .list_events_after(run.id.as_str(), /*after_seq*/ None, Some(1))
                .await
                .map_err(|err| {
                    internal_error(format!("failed to list background agent events: {err}"))
                })?;
            match events.pop() {
                Some(event) => event,
                None => append_background_agent_event_with_retry(
                    state_db.as_ref(),
                    run.id.as_str(),
                    "agent.startRecovered",
                    &json!({
                        "reason": "idempotent_start_without_start_event",
                    }),
                )
                .await
                .map_err(|err| {
                    internal_error(format!("failed to append background agent event: {err}"))
                })?,
            }
        };
        let snapshot = match state_db
            .get_status_snapshot(run.id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to load background agent snapshot: {err}"))
            })? {
            Some(snapshot) => snapshot,
            None => state_db
                .upsert_status_snapshot(BackgroundAgentStatusSnapshotParams {
                    run_id: run.id.clone(),
                    seq: event.seq,
                    status: run.status,
                    desired_state: run.desired_state,
                    summary: Some("Queued".to_string()),
                    pending_interaction_count: 0,
                    last_event_seq: event.seq,
                    payload_json: json!({
                        "phase": "queued",
                    }),
                })
                .await
                .map_err(|err| {
                    internal_error(format!("failed to update background agent snapshot: {err}"))
                })?,
        };
        let run = self
            .load_agent_run(state_db.as_ref(), run.id.as_str())
            .await?
            .ok_or_else(|| internal_error("background agent disappeared after create"))?;

        Ok(AgentStartResponse {
            agent: api_agent_run_from_state(run),
            status_snapshot: api_agent_status_snapshot_from_state(snapshot),
            execution_snapshot: api_agent_execution_snapshot_from_state(execution_snapshot),
            event: api_agent_event_from_state(event),
        })
    }

    pub(super) async fn agent_list_inner(
        &self,
        params: AgentListParams,
    ) -> Result<AgentListResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let limit = normalize_agent_list_limit(params.limit)?;
        let offset = decode_offset_cursor(params.cursor.as_deref())?;
        let mut runs = state_db
            .list_background_agent_runs_page(offset, limit.saturating_add(1))
            .await
            .map_err(|err| internal_error(format!("failed to list background agents: {err}")))?;
        let has_more = runs.len() > limit;
        if has_more {
            runs.truncate(limit);
        }
        Ok(AgentListResponse {
            data: runs.into_iter().map(api_agent_run_from_state).collect(),
            next_cursor: has_more.then(|| offset.saturating_add(limit).to_string()),
        })
    }

    pub(super) async fn agent_read_inner(
        &self,
        params: AgentReadParams,
    ) -> Result<AgentReadResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        expire_timed_out_pending_interactions(state_db.as_ref()).await?;
        let Some(run) = self
            .load_agent_run(state_db.as_ref(), params.agent_id.as_str())
            .await?
        else {
            return Ok(AgentReadResponse {
                agent: None,
                status_snapshot: None,
                execution_snapshot: None,
                pending_interactions: Vec::new(),
            });
        };
        let status_snapshot = state_db
            .get_status_snapshot(run.id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to load background agent snapshot: {err}"))
            })?
            .map(api_agent_status_snapshot_from_state);
        let execution_snapshot = state_db
            .get_latest_execution_snapshot(run.id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to load background agent execution snapshot: {err}"
                ))
            })?
            .map(api_agent_execution_snapshot_from_state);
        let pending_interactions = state_db
            .list_pending_interactions(run.id.as_str(), /*status*/ None)
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to list background agent pending interactions: {err}"
                ))
            })?
            .into_iter()
            .map(api_agent_pending_interaction_from_state)
            .collect();
        Ok(AgentReadResponse {
            agent: Some(api_agent_run_from_state(run)),
            status_snapshot,
            execution_snapshot,
            pending_interactions,
        })
    }

    pub(super) async fn agent_attach_inner(
        &self,
        params: AgentAttachParams,
    ) -> Result<AgentAttachResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let Some(run) = self
            .load_agent_run(state_db.as_ref(), params.agent_id.as_str())
            .await?
        else {
            return Ok(AgentAttachResponse {
                effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(
                    LifecycleAction::Attach,
                )),
                agent: None,
                status_snapshot: None,
                execution_snapshot: None,
                events: Vec::new(),
                pending_interactions: Vec::new(),
                next_cursor: None,
            });
        };
        let limit = normalize_agent_list_limit(params.limit)?;
        let after_seq = decode_event_cursor(params.cursor.as_deref())?;
        state_db
            .list_events_after(run.id.as_str(), after_seq, Some(1))
            .await
            .map_err(map_background_agent_event_replay_error)?;
        expire_timed_out_pending_interactions(state_db.as_ref()).await?;
        let execution_snapshot = state_db
            .get_latest_execution_snapshot(run.id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to load background agent execution snapshot: {err}"
                ))
            })?
            .map(api_agent_execution_snapshot_from_state);
        for interaction in state_db
            .list_pending_interactions(
                run.id.as_str(),
                Some(BackgroundAgentPendingInteractionStatus::Pending),
            )
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to list background agent pending interactions: {err}"
                ))
            })?
        {
            state_db
                .mark_pending_interaction_delivered(interaction.id.as_str())
                .await
                .map_err(|err| {
                    internal_error(format!(
                        "failed to mark background agent pending interaction delivered: {err}"
                    ))
                })?;
        }
        let mut events = state_db
            .list_events_after(run.id.as_str(), after_seq, Some(limit.saturating_add(1)))
            .await
            .map_err(map_background_agent_event_replay_error)?;
        let has_more = events.len() > limit;
        if has_more {
            events.truncate(limit);
        }
        let next_cursor = has_more
            .then(|| events.last().map(|event| encode_event_cursor(event.seq)))
            .flatten();
        let pending_interactions = state_db
            .list_pending_interactions(run.id.as_str(), /*status*/ None)
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to list background agent pending interactions: {err}"
                ))
            })?
            .into_iter()
            .map(api_agent_pending_interaction_from_state)
            .collect();
        let status_snapshot = state_db
            .get_status_snapshot(run.id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to load background agent snapshot: {err}"))
            })?
            .map(api_agent_status_snapshot_from_state);
        let run = self
            .load_agent_run(state_db.as_ref(), run.id.as_str())
            .await?
            .unwrap_or(run);

        Ok(AgentAttachResponse {
            effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(
                LifecycleAction::Attach,
            )),
            agent: Some(api_agent_run_from_state(run)),
            status_snapshot,
            execution_snapshot,
            events: events.into_iter().map(api_agent_event_from_state).collect(),
            pending_interactions,
            next_cursor,
        })
    }

    pub(super) async fn agent_detach_inner(
        &self,
        params: AgentDetachParams,
    ) -> Result<AgentDetachResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let run = self
            .load_agent_run(state_db.as_ref(), params.agent_id.as_str())
            .await?
            .map(api_agent_run_from_state);
        Ok(AgentDetachResponse {
            effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(
                LifecycleAction::Detach,
            )),
            agent: run,
        })
    }

    pub(super) async fn agent_stop_inner(
        &self,
        params: AgentStopParams,
    ) -> Result<AgentStopResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let Some(run) = self
            .load_agent_run(state_db.as_ref(), params.agent_id.as_str())
            .await?
        else {
            return Ok(AgentStopResponse {
                effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(
                    LifecycleAction::Stop,
                )),
                agent: None,
            });
        };
        if matches!(
            run.retention_state,
            codex_state::BackgroundAgentRetentionState::DeleteRequested
                | codex_state::BackgroundAgentRetentionState::Deleted
        ) {
            return Ok(AgentStopResponse {
                effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(
                    LifecycleAction::Stop,
                )),
                agent: Some(api_agent_run_from_state(run)),
            });
        }
        state_db
            .set_desired_state(run.id.as_str(), BackgroundAgentDesiredState::Stopped)
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to update background agent desired state: {err}"
                ))
            })?;
        if !is_terminal_agent_status(run.status) {
            let terminalize_immediately = should_terminalize_unclaimed_agent_run(&run);
            let status = if terminalize_immediately {
                BackgroundAgentRunStatus::Cancelled
            } else {
                BackgroundAgentRunStatus::Stopping
            };
            let status_reason = if terminalize_immediately {
                "stop requested before worker claim"
            } else {
                "stop requested"
            };
            state_db
                .update_run_status(run.id.as_str(), status, Some(status_reason))
                .await
                .map_err(|err| {
                    internal_error(format!("failed to update background agent status: {err}"))
                })?;
            append_background_agent_event_with_retry(
                state_db.as_ref(),
                run.id.as_str(),
                "agent.stopRequested",
                &json!({
                    "reason": "client_requested_stop",
                }),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to append background agent event: {err}"))
            })?;
            cancel_active_pending_interactions_for_run(
                state_db.as_ref(),
                run.id.as_str(),
                "client_requested_stop",
            )
            .await?;
            if terminalize_immediately {
                upsert_lifecycle_status_snapshot(
                    state_db.as_ref(),
                    run.id.as_str(),
                    status,
                    "Stopped",
                    "client_requested_stop",
                )
                .await?;
            } else {
                upsert_lifecycle_status_snapshot(
                    state_db.as_ref(),
                    run.id.as_str(),
                    status,
                    "Stopping",
                    "client_requested_stop",
                )
                .await?;
            }
        }
        let run = self
            .load_agent_run(state_db.as_ref(), run.id.as_str())
            .await?;
        Ok(AgentStopResponse {
            effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(LifecycleAction::Stop)),
            agent: run.map(api_agent_run_from_state),
        })
    }

    pub(super) async fn agent_delete_inner(
        &self,
        params: AgentDeleteParams,
    ) -> Result<AgentDeleteResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let existing_run = self
            .load_agent_run(state_db.as_ref(), params.agent_id.as_str())
            .await?;
        let deleted = state_db
            .request_delete_run(params.agent_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to request background agent delete: {err}"))
            })?;
        if deleted {
            let non_terminal_existing_run = existing_run
                .as_ref()
                .filter(|run| !is_terminal_agent_status(run.status));
            let terminalized_immediately =
                non_terminal_existing_run.is_some_and(should_terminalize_unclaimed_agent_run);
            if terminalized_immediately {
                state_db
                    .update_run_status(
                        params.agent_id.as_str(),
                        BackgroundAgentRunStatus::Cancelled,
                        Some("delete requested before worker claim"),
                    )
                    .await
                    .map_err(|err| {
                        internal_error(format!("failed to update background agent status: {err}"))
                    })?;
            }
            append_background_agent_event_with_retry(
                state_db.as_ref(),
                params.agent_id.as_str(),
                "agent.deleteRequested",
                &json!({
                    "reason": "client_requested_delete",
                }),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to append background agent event: {err}"))
            })?;
            if non_terminal_existing_run.is_some() {
                cancel_active_pending_interactions_for_run(
                    state_db.as_ref(),
                    params.agent_id.as_str(),
                    "client_requested_delete",
                )
                .await?;
                let status = if terminalized_immediately {
                    BackgroundAgentRunStatus::Cancelled
                } else {
                    BackgroundAgentRunStatus::Stopping
                };
                upsert_lifecycle_status_snapshot(
                    state_db.as_ref(),
                    params.agent_id.as_str(),
                    status,
                    if terminalized_immediately {
                        "Deleted"
                    } else {
                        "Deleting"
                    },
                    "client_requested_delete",
                )
                .await?;
            } else if let Some(existing_run) = existing_run.as_ref() {
                upsert_lifecycle_status_snapshot(
                    state_db.as_ref(),
                    params.agent_id.as_str(),
                    existing_run.status,
                    "Deleted",
                    "client_requested_delete",
                )
                .await?;
            }
        }
        let run = self
            .load_agent_run(state_db.as_ref(), params.agent_id.as_str())
            .await?
            .map(api_agent_run_from_state);
        Ok(AgentDeleteResponse {
            effect: api_lifecycle_effect_from_runtime(lifecycle_effect_for(
                LifecycleAction::Delete,
            )),
            agent: run,
            deleted,
        })
    }

    pub(super) async fn agent_events_list_inner(
        &self,
        params: AgentEventsListParams,
    ) -> Result<AgentEventsListResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let limit = normalize_agent_list_limit(params.limit)?;
        let after_seq = decode_event_cursor(params.cursor.as_deref())?;
        let mut events = state_db
            .list_events_after(
                params.agent_id.as_str(),
                after_seq,
                Some(limit.saturating_add(1)),
            )
            .await
            .map_err(map_background_agent_event_replay_error)?;
        let has_more = events.len() > limit;
        if has_more {
            events.truncate(limit);
        }
        let next_cursor = has_more
            .then(|| events.last().map(|event| encode_event_cursor(event.seq)))
            .flatten();
        Ok(AgentEventsListResponse {
            data: events.into_iter().map(api_agent_event_from_state).collect(),
            next_cursor,
        })
    }

    pub(super) async fn agent_pending_interaction_respond_inner(
        &self,
        params: AgentPendingInteractionRespondParams,
    ) -> Result<AgentPendingInteractionRespondResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let terminal_status = background_pending_terminal_status_from_api(params.terminal_status);
        expire_timed_out_pending_interactions(state_db.as_ref()).await?;
        let Some(existing_interaction) = state_db
            .get_pending_interaction(params.interaction_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to load background agent pending interaction: {err}"
                ))
            })?
        else {
            return Ok(AgentPendingInteractionRespondResponse {
                updated: false,
                interaction: None,
            });
        };
        if existing_interaction.run_id != params.agent_id {
            return Err(invalid_request(
                "pending interaction does not belong to requested agent",
            ));
        }
        if !matches!(
            existing_interaction.status,
            BackgroundAgentPendingInteractionStatus::Pending
                | BackgroundAgentPendingInteractionStatus::Delivered
        ) {
            return Ok(AgentPendingInteractionRespondResponse {
                updated: false,
                interaction: Some(api_agent_pending_interaction_from_state(
                    existing_interaction,
                )),
            });
        }
        validate_agent_pending_interaction_response(
            existing_interaction.kind,
            terminal_status,
            &params.response,
        )?;
        let updated = state_db
            .respond_pending_interaction(
                params.interaction_id.as_str(),
                &params.response,
                terminal_status,
            )
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to respond to background agent pending interaction: {err}"
                ))
            })?;
        let interaction = state_db
            .get_pending_interaction(params.interaction_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to load background agent pending interaction: {err}"
                ))
            })?;
        let interaction = interaction.map(api_agent_pending_interaction_from_state);
        Ok(AgentPendingInteractionRespondResponse {
            updated,
            interaction,
        })
    }

    pub(super) async fn agent_daemon_diagnostics_inner(
        &self,
    ) -> Result<AgentDaemonDiagnosticsResponse, JSONRPCErrorError> {
        let Some(state_db) = self.state_db.clone() else {
            let quota = AgentQuotaSnapshot::empty(DEFAULT_MAX_ACTIVE_AGENT_RUNS_PER_USER);
            return Ok(AgentDaemonDiagnosticsResponse {
                state_store_available: false,
                active_run_count: quota.active_run_count,
                queued_run_count: quota.queued_run_count,
                starting_run_count: quota.starting_run_count,
                running_run_count: quota.running_run_count,
                waiting_run_count: quota.waiting_run_count,
                stopping_run_count: quota.stopping_run_count,
                pending_interaction_count: 0,
                runs_by_status: Vec::new(),
                max_active_runs_per_user: quota.max_active_runs_per_user,
                available_active_run_slots: quota.available_active_run_slots(),
                admission_allowed: quota.admission_allowed(),
                backpressure_reasons: quota.backpressure_reasons(),
                max_list_limit: MAX_AGENT_LIST_LIMIT as u32,
            });
        };
        expire_timed_out_pending_interactions(state_db.as_ref()).await?;
        let quota = load_agent_quota_snapshot(state_db.as_ref()).await?;
        let pending_interaction_count = state_db
            .count_pending_interactions(Some(BackgroundAgentPendingInteractionStatus::Pending))
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to count background agent pending interactions: {err}"
                ))
            })?
            + state_db
                .count_pending_interactions(Some(
                    BackgroundAgentPendingInteractionStatus::Delivered,
                ))
                .await
                .map_err(|err| {
                    internal_error(format!(
                        "failed to count background agent delivered interactions: {err}"
                    ))
                })?;
        Ok(AgentDaemonDiagnosticsResponse {
            state_store_available: true,
            active_run_count: quota.active_run_count,
            queued_run_count: quota.queued_run_count,
            starting_run_count: quota.starting_run_count,
            running_run_count: quota.running_run_count,
            waiting_run_count: quota.waiting_run_count,
            stopping_run_count: quota.stopping_run_count,
            pending_interaction_count,
            runs_by_status: quota.api_status_counts(),
            max_active_runs_per_user: quota.max_active_runs_per_user,
            available_active_run_slots: quota.available_active_run_slots(),
            admission_allowed: quota.admission_allowed(),
            backpressure_reasons: quota.backpressure_reasons(),
            max_list_limit: MAX_AGENT_LIST_LIMIT as u32,
        })
    }

    pub(super) async fn worktree_list_inner(
        &self,
        params: WorktreeListParams,
        policy: WorktreePolicy,
    ) -> Result<WorktreeListResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let limit = params
            .limit
            .unwrap_or(codex_state::DEFAULT_MANAGED_WORKTREE_LIST_LIMIT);
        let include_deleted = params.include_deleted.unwrap_or(false);
        let base_repo_path = params
            .base_repo_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from);
        if let Some(base_repo_path) = base_repo_path.as_ref()
            && !base_repo_path.is_absolute()
        {
            return Err(invalid_params(
                "worktree/list baseRepoPath must be absolute",
            ));
        }
        let page = state_db
            .managed_worktrees()
            .list_managed_worktrees_page(
                base_repo_path.as_deref(),
                include_deleted,
                params.cursor.as_deref(),
                limit,
            )
            .await
            .map_err(|err| internal_error(format!("failed to list worktrees: {err}")))?;
        let mut data = Vec::with_capacity(page.data.len());
        for worktree in page.data {
            data.push(api_worktree_from_state(state_db.as_ref(), worktree).await?);
        }
        Ok(WorktreeListResponse {
            data,
            next_cursor: page.next_cursor,
            policy,
        })
    }

    pub(super) async fn worktree_read_inner(
        &self,
        params: WorktreeReadParams,
        base_repo_path: Option<&Path>,
        policy: WorktreePolicy,
    ) -> Result<WorktreeReadResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let worktree = state_db
            .managed_worktrees()
            .get_managed_worktree(params.worktree_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read worktree: {err}")))?;
        let worktree = match worktree {
            Some(worktree) => {
                if base_repo_path.is_some_and(|base_repo_path| {
                    !paths_equivalent(worktree.base_repo_path.as_path(), base_repo_path)
                }) {
                    None
                } else {
                    Some(api_worktree_from_state(state_db.as_ref(), worktree).await?)
                }
            }
            None => None,
        };
        Ok(WorktreeReadResponse { worktree, policy })
    }

    pub(super) async fn worktree_attach_inner(
        &self,
        params: WorktreeAttachParams,
    ) -> Result<WorktreeAttachResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let target =
            worktree_assignment_target("worktree/attach", params.thread_id, params.agent_run_id)?;
        if let ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) = &target {
            let run = self
                .load_agent_run(state_db.as_ref(), agent_run_id.as_str())
                .await?
                .ok_or_else(|| {
                    invalid_params(format!(
                        "worktree/attach agentRunId `{agent_run_id}` does not exist"
                    ))
                })?;
            if is_terminal_agent_status(run.status) {
                return Err(invalid_params(format!(
                    "worktree/attach agentRunId `{agent_run_id}` is terminal"
                )));
            }
        }
        let worktree = state_db
            .managed_worktrees()
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: params.worktree_id,
                target,
            })
            .await
            .map_err(|err| invalid_params(format!("failed to attach worktree: {err}")))?;
        Ok(WorktreeAttachResponse {
            worktree: api_worktree_from_state(state_db.as_ref(), worktree).await?,
        })
    }

    pub(super) async fn worktree_detach_inner(
        &self,
        params: WorktreeDetachParams,
    ) -> Result<WorktreeDetachResponse, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let target =
            worktree_assignment_target("worktree/detach", params.thread_id, params.agent_run_id)?;
        let worktree = state_db
            .managed_worktrees()
            .detach_managed_worktree(ManagedWorktreeDetachParams {
                worktree_id: params.worktree_id,
                target,
            })
            .await
            .map_err(|err| invalid_params(format!("failed to detach worktree: {err}")))?;
        let worktree = match worktree {
            Some(worktree) => Some(api_worktree_from_state(state_db.as_ref(), worktree).await?),
            None => None,
        };
        Ok(WorktreeDetachResponse { worktree })
    }

    pub(super) fn state_db(&self) -> Result<StateDbHandle, JSONRPCErrorError> {
        self.state_db
            .clone()
            .ok_or_else(|| internal_error("background agent state store is unavailable"))
    }

    pub(super) async fn load_agent_run(
        &self,
        state_db: &codex_state::StateRuntime,
        agent_id: &str,
    ) -> Result<Option<BackgroundAgentRun>, JSONRPCErrorError> {
        state_db
            .get_run(agent_id)
            .await
            .map_err(|err| internal_error(format!("failed to load background agent: {err}")))
    }
}

fn worktree_assignment_target(
    method: &str,
    thread_id: Option<String>,
    agent_run_id: Option<String>,
) -> Result<ManagedWorktreeAssignmentTarget, JSONRPCErrorError> {
    match (thread_id, agent_run_id) {
        (Some(thread_id), None) => Ok(ManagedWorktreeAssignmentTarget::Thread(
            ThreadId::from_string(thread_id.as_str())
                .map_err(|err| invalid_params(format!("invalid threadId: {err}")))?,
        )),
        (None, Some(agent_run_id)) => Ok(ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id)),
        (Some(_), Some(_)) => Err(invalid_params(format!(
            "{method} accepts only one of threadId or agentRunId"
        ))),
        (None, None) => Err(invalid_params(format!(
            "{method} requires one of threadId or agentRunId"
        ))),
    }
}

async fn validate_agent_start_rollout_path(
    state_db: &codex_state::StateRuntime,
    thread_id: Option<&str>,
    rollout_path: Option<&str>,
) -> Result<(), JSONRPCErrorError> {
    let (thread_id, rollout_path) = match (thread_id, rollout_path) {
        (None, None) => return Ok(()),
        (Some(_), None) => {
            return Err(invalid_params("agent/start threadId requires rolloutPath"));
        }
        (None, Some(_)) => {
            return Err(invalid_params("agent/start rolloutPath requires threadId"));
        }
        (Some(thread_id), Some(rollout_path)) => (thread_id, rollout_path),
    };
    let thread_id = ThreadId::from_string(thread_id)
        .map_err(|err| invalid_params(format!("invalid threadId: {err}")))?;
    let thread = state_db
        .get_thread(thread_id)
        .await
        .map_err(|err| internal_error(format!("failed to load background thread: {err}")))?
        .ok_or_else(|| invalid_params("agent/start rolloutPath requires a known threadId"))?;
    let requested_rollout_path = Path::new(rollout_path);
    let stored_rollout_path = thread.rollout_path.as_path();
    let rollout_paths_match = requested_rollout_path == stored_rollout_path
        || std::fs::canonicalize(requested_rollout_path)
            .ok()
            .zip(std::fs::canonicalize(stored_rollout_path).ok())
            .is_some_and(|(requested, stored)| requested == stored);
    if !rollout_paths_match {
        return Err(invalid_params(
            "agent/start rolloutPath must match threadId",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct AgentQuotaSnapshot {
    active_run_count: i64,
    queued_run_count: i64,
    starting_run_count: i64,
    running_run_count: i64,
    waiting_run_count: i64,
    stopping_run_count: i64,
    runs_by_status: Vec<(BackgroundAgentRunStatus, i64)>,
    max_active_runs_per_user: i64,
}

impl AgentQuotaSnapshot {
    fn empty(max_active_runs_per_user: i64) -> Self {
        Self {
            active_run_count: 0,
            queued_run_count: 0,
            starting_run_count: 0,
            running_run_count: 0,
            waiting_run_count: 0,
            stopping_run_count: 0,
            runs_by_status: Vec::new(),
            max_active_runs_per_user,
        }
    }

    fn from_status_counts(
        runs_by_status: Vec<(BackgroundAgentRunStatus, i64)>,
        max_active_runs_per_user: i64,
    ) -> Self {
        let mut snapshot = Self::empty(max_active_runs_per_user);
        snapshot.runs_by_status = runs_by_status;
        for (status, count) in &snapshot.runs_by_status {
            if !is_terminal_agent_status(*status) {
                snapshot.active_run_count += *count;
            }
            match status {
                BackgroundAgentRunStatus::Queued => snapshot.queued_run_count += *count,
                BackgroundAgentRunStatus::Starting => snapshot.starting_run_count += *count,
                BackgroundAgentRunStatus::Running => snapshot.running_run_count += *count,
                BackgroundAgentRunStatus::WaitingOnApproval
                | BackgroundAgentRunStatus::WaitingOnUser => snapshot.waiting_run_count += *count,
                BackgroundAgentRunStatus::Stopping => snapshot.stopping_run_count += *count,
                BackgroundAgentRunStatus::Completed
                | BackgroundAgentRunStatus::Failed
                | BackgroundAgentRunStatus::Cancelled
                | BackgroundAgentRunStatus::Orphaned => {}
            }
        }
        snapshot
    }

    fn available_active_run_slots(&self) -> i64 {
        self.max_active_runs_per_user
            .saturating_sub(self.active_run_count)
            .max(0)
    }

    fn admission_allowed(&self) -> bool {
        self.available_active_run_slots() > 0
    }

    fn backpressure_reasons(&self) -> Vec<String> {
        if self.admission_allowed() {
            Vec::new()
        } else {
            vec![AGENT_BACKPRESSURE_ACTIVE_RUN_LIMIT.to_string()]
        }
    }

    fn api_status_counts(&self) -> Vec<AgentRunStatusCount> {
        self.runs_by_status
            .iter()
            .map(|(status, count)| AgentRunStatusCount {
                status: api_agent_run_status_from_background(*status),
                count: *count,
            })
            .collect()
    }
}

async fn load_agent_quota_snapshot(
    state_db: &codex_state::StateRuntime,
) -> Result<AgentQuotaSnapshot, JSONRPCErrorError> {
    let runs_by_status = state_db
        .count_runs_by_status()
        .await
        .map_err(|err| internal_error(format!("failed to count background agents: {err}")))?;
    Ok(AgentQuotaSnapshot::from_status_counts(
        runs_by_status,
        DEFAULT_MAX_ACTIVE_AGENT_RUNS_PER_USER,
    ))
}

struct InitialExecutionSnapshotPayloadParams<'a> {
    cwd: Option<&'a str>,
    execution_context: Option<&'a AgentExecutionContextParams>,
    recovery_policy: &'a str,
}

fn initial_execution_snapshot_payload(
    run: &BackgroundAgentRun,
    params: InitialExecutionSnapshotPayloadParams<'_>,
) -> serde_json::Value {
    json!({
        "snapshotSource": "agent/start",
        "cwd": params.cwd,
        "workspaceRoots": params
            .execution_context
            .and_then(|context| context.workspace_roots.as_ref()),
        "approvalPolicy": params
            .execution_context
            .and_then(|context| context.approval_policy),
        "authProfileRef": run.auth_profile_ref.as_deref(),
        "permissionProfile": params
            .execution_context
            .and_then(|context| context.permission_profile.as_ref()),
        "sandboxPolicy": params
            .execution_context
            .and_then(|context| context.sandbox_policy.as_ref()),
        "networkPolicy": params
            .execution_context
            .and_then(|context| context.network_policy.as_ref()),
        "model": params
            .execution_context
            .and_then(|context| context.model.as_deref()),
        "provider": params
            .execution_context
            .and_then(|context| context.provider.as_deref()),
        "serviceTier": params
            .execution_context
            .and_then(|context| context.service_tier.as_deref()),
        "mcpToolAllowlist": params
            .execution_context
            .and_then(|context| context.mcp_tool_allowlist.as_ref()),
        "envSnapshotPolicy": params
            .execution_context
            .and_then(|context| context.env_snapshot_policy.as_deref())
            .unwrap_or("inherit-minimal"),
        "shellSnapshot": params
            .execution_context
            .and_then(|context| context.shell_snapshot.as_ref()),
        "configSourceHashes": params
            .execution_context
            .and_then(|context| context.config_source_hashes.as_ref()),
        "maxRuntimeSeconds": params
            .execution_context
            .and_then(|context| context.max_runtime_seconds),
        "maxTokens": params
            .execution_context
            .and_then(|context| context.max_tokens),
        "configFingerprint": run.config_fingerprint.as_deref(),
        "versionFingerprint": run.version_fingerprint.as_deref(),
        "recoveryPolicy": params.recovery_policy,
        "midTurnCrashSemantics": "abort_mid_turn_resume_at_safe_boundary",
    })
}

fn validate_agent_prompt(prompt: String) -> Result<String, JSONRPCErrorError> {
    let actual_chars = prompt.chars().count();
    if actual_chars > MAX_USER_INPUT_TEXT_CHARS {
        let mut error = invalid_params(format!(
            "Input exceeds the maximum length of {MAX_USER_INPUT_TEXT_CHARS} characters."
        ));
        error.data = Some(json!({
            "input_error_code": INPUT_TOO_LARGE_ERROR_CODE,
            "max_chars": MAX_USER_INPUT_TEXT_CHARS,
            "actual_chars": actual_chars,
        }));
        return Err(error);
    }
    if prompt.trim().is_empty() {
        return Err(invalid_request("agent prompt must not be empty"));
    }
    Ok(prompt)
}

fn normalize_agent_list_limit(limit: Option<u32>) -> Result<usize, JSONRPCErrorError> {
    let limit = limit.unwrap_or(DEFAULT_AGENT_LIST_LIMIT as u32);
    if limit == 0 {
        return Err(invalid_request("limit must be greater than zero"));
    }
    Ok((limit as usize).min(MAX_AGENT_LIST_LIMIT))
}

async fn expire_timed_out_pending_interactions(
    state_db: &codex_state::StateRuntime,
) -> Result<(), JSONRPCErrorError> {
    state_db
        .expire_timed_out_interactions()
        .await
        .map(|_| ())
        .map_err(|err| {
            internal_error(format!(
                "failed to expire background agent pending interactions: {err}"
            ))
        })
}

fn map_background_agent_event_replay_error(err: anyhow::Error) -> JSONRPCErrorError {
    let message = err.to_string();
    if message.contains(BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED) {
        invalid_request(message)
    } else {
        internal_error(format!("failed to list background agent events: {message}"))
    }
}

async fn append_background_agent_event_with_retry(
    state_db: &codex_state::StateRuntime,
    run_id: &str,
    event_type: &str,
    payload_json: &Value,
) -> anyhow::Result<BackgroundAgentEvent> {
    retry_transient_sqlite_busy("append background agent event", || {
        state_db.append_event(run_id, event_type, payload_json)
    })
    .await
}

fn decode_offset_cursor(cursor: Option<&str>) -> Result<usize, JSONRPCErrorError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let offset = cursor
        .parse::<usize>()
        .map_err(|_| invalid_request("cursor must be a non-negative integer offset"))?;
    Ok(offset)
}

fn decode_event_cursor(cursor: Option<&str>) -> Result<Option<i64>, JSONRPCErrorError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let Some(seq) = cursor.strip_prefix(AGENT_EVENT_CURSOR_PREFIX) else {
        return Err(invalid_request("cursor must be an opaque event cursor"));
    };
    let seq = seq
        .parse::<i64>()
        .map_err(|_| invalid_request("cursor must be an opaque event cursor"))?;
    if seq < 0 {
        return Err(invalid_request("cursor must be an opaque event cursor"));
    }
    Ok(Some(seq))
}

fn encode_event_cursor(seq: i64) -> String {
    format!("{AGENT_EVENT_CURSOR_PREFIX}{seq}")
}

fn should_terminalize_unclaimed_agent_run(run: &BackgroundAgentRun) -> bool {
    run.supervisor_id.is_none()
        || matches!(
            run.status,
            BackgroundAgentRunStatus::Queued | BackgroundAgentRunStatus::Orphaned
        )
}

fn validate_agent_pending_interaction_response(
    kind: BackgroundAgentPendingInteractionKind,
    terminal_status: BackgroundAgentPendingInteractionStatus,
    response: &serde_json::Value,
) -> Result<(), JSONRPCErrorError> {
    if terminal_status != BackgroundAgentPendingInteractionStatus::Responded {
        return Ok(());
    }

    let is_valid = match kind {
        BackgroundAgentPendingInteractionKind::Approval => {
            let decision = response
                .get("decision")
                .cloned()
                .unwrap_or_else(|| response.clone());
            serde_json::from_value::<ReviewDecision>(decision).is_ok()
        }
        BackgroundAgentPendingInteractionKind::UserInput => {
            serde_json::from_value::<RequestUserInputResponse>(response.clone()).is_ok()
        }
        BackgroundAgentPendingInteractionKind::McpElicitation => response
            .get("decision")
            .cloned()
            .and_then(|decision| serde_json::from_value::<ElicitationAction>(decision).ok())
            .is_some(),
        BackgroundAgentPendingInteractionKind::PermissionGrant => {
            serde_json::from_value::<RequestPermissionsResponse>(response.clone()).is_ok()
        }
    };

    if is_valid {
        Ok(())
    } else {
        Err(invalid_request(format!(
            "background agent pending interaction response is invalid for {}",
            kind.as_str()
        )))
    }
}

async fn cancel_active_pending_interactions_for_run(
    state_db: &codex_state::StateRuntime,
    run_id: &str,
    reason: &str,
) -> Result<(), JSONRPCErrorError> {
    let interactions = state_db
        .list_pending_interactions(run_id, /*status*/ None)
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to list background agent pending interactions: {err}"
            ))
        })?;
    for interaction in interactions {
        if matches!(
            interaction.status,
            BackgroundAgentPendingInteractionStatus::Pending
                | BackgroundAgentPendingInteractionStatus::Delivered
        ) {
            state_db
                .respond_pending_interaction(
                    interaction.id.as_str(),
                    &json!({"reason": reason}),
                    BackgroundAgentPendingInteractionStatus::Cancelled,
                )
                .await
                .map_err(|err| {
                    internal_error(format!(
                        "failed to cancel background agent pending interaction: {err}"
                    ))
                })?;
        }
    }
    Ok(())
}

async fn upsert_lifecycle_status_snapshot(
    state_db: &codex_state::StateRuntime,
    run_id: &str,
    status: BackgroundAgentRunStatus,
    summary: &str,
    reason: &str,
) -> Result<(), JSONRPCErrorError> {
    let Some(run) = state_db.get_run(run_id).await.map_err(|err| {
        internal_error(format!(
            "failed to load background agent after lifecycle update: {err}"
        ))
    })?
    else {
        return Ok(());
    };
    let pending_interaction_count = state_db
        .list_pending_interactions(run_id, /*status*/ None)
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to list background agent pending interactions: {err}"
            ))
        })?
        .into_iter()
        .filter(|interaction| {
            matches!(
                interaction.status,
                BackgroundAgentPendingInteractionStatus::Pending
                    | BackgroundAgentPendingInteractionStatus::Delivered
            )
        })
        .count() as i64;
    state_db
        .upsert_status_snapshot(BackgroundAgentStatusSnapshotParams {
            run_id: run_id.to_string(),
            seq: run.last_event_seq,
            status,
            desired_state: run.desired_state,
            summary: Some(summary.to_string()),
            pending_interaction_count,
            last_event_seq: run.last_event_seq,
            payload_json: json!({
                "phase": status.as_str(),
                "reason": reason,
            }),
        })
        .await
        .map(|_| ())
        .map_err(|err| {
            internal_error(format!(
                "failed to update background agent lifecycle snapshot: {err}"
            ))
        })
}

fn is_terminal_agent_status(status: BackgroundAgentRunStatus) -> bool {
    matches!(
        status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    )
}

fn api_agent_run_from_state(value: BackgroundAgentRun) -> AgentRun {
    AgentRun {
        agent_id: value.id,
        idempotency_key: value.idempotency_key,
        request_id: value.request_id,
        source: value.source,
        prompt_snapshot_ref: value.prompt_snapshot_ref,
        input_snapshot_ref: value.input_snapshot_ref,
        thread_id: value.thread_id,
        thread_store_kind: value.thread_store_kind,
        thread_store_id: value.thread_store_id,
        rollout_path: value.rollout_path,
        parent_thread_id: value.parent_thread_id,
        parent_agent_run_id: value.parent_agent_run_id,
        spawn_linkage: value.spawn_linkage_json,
        worktree_lease_id: value.worktree_lease_id,
        auth_profile_ref: value.auth_profile_ref,
        desired_state: api_agent_desired_state_from_background(value.desired_state),
        status: api_agent_run_status_from_background(value.status),
        status_reason: value.status_reason,
        config_fingerprint: value.config_fingerprint,
        version_fingerprint: value.version_fingerprint,
        retention_state: match value.retention_state {
            codex_state::BackgroundAgentRetentionState::Active => AgentRetentionState::Active,
            codex_state::BackgroundAgentRetentionState::Archived => AgentRetentionState::Archived,
            codex_state::BackgroundAgentRetentionState::DeleteRequested => {
                AgentRetentionState::DeleteRequested
            }
            codex_state::BackgroundAgentRetentionState::Deleted => AgentRetentionState::Deleted,
        },
        archive_after: value.archive_after.map(|timestamp| timestamp.timestamp()),
        delete_after: value.delete_after.map(|timestamp| timestamp.timestamp()),
        archived_at: value.archived_at.map(|timestamp| timestamp.timestamp()),
        deleted_at: value.deleted_at.map(|timestamp| timestamp.timestamp()),
        supervisor_id: value.supervisor_id,
        generation: value.generation,
        pid: value.pid,
        pgid: value.pgid,
        job_id: value.job_id,
        heartbeat_at: value.heartbeat_at.map(|timestamp| timestamp.timestamp()),
        crash_reason: value.crash_reason,
        exit_code: value.exit_code,
        exit_signal: value.exit_signal,
        last_event_seq: value.last_event_seq,
        last_snapshot_seq: value.last_snapshot_seq,
        created_at: value.created_at.timestamp(),
        updated_at: value.updated_at.timestamp(),
        started_at: value.started_at.map(|timestamp| timestamp.timestamp()),
        completed_at: value.completed_at.map(|timestamp| timestamp.timestamp()),
    }
}

fn api_agent_event_from_state(value: BackgroundAgentEvent) -> AgentEvent {
    AgentEvent {
        event_id: value.id.to_string(),
        agent_id: value.run_id,
        seq: value.seq,
        event_type: value.event_type,
        payload: value.payload_json,
        created_at: value.created_at.timestamp(),
    }
}

fn api_agent_status_snapshot_from_state(
    value: BackgroundAgentStatusSnapshot,
) -> AgentStatusSnapshot {
    AgentStatusSnapshot {
        agent_id: value.run_id,
        seq: value.seq,
        status: api_agent_run_status_from_background(value.status),
        desired_state: api_agent_desired_state_from_background(value.desired_state),
        summary: value.summary,
        pending_interaction_count: value.pending_interaction_count,
        last_event_seq: value.last_event_seq,
        payload: value.payload_json,
        updated_at: value.updated_at.timestamp(),
    }
}

fn api_agent_execution_snapshot_from_state(
    value: BackgroundAgentExecutionSnapshot,
) -> AgentExecutionSnapshot {
    AgentExecutionSnapshot {
        snapshot_id: value.id.to_string(),
        agent_id: value.run_id,
        seq: value.seq,
        snapshot_kind: value.snapshot_kind,
        payload: value.payload_json,
        recovery_policy: value.recovery_policy,
        config_fingerprint: value.config_fingerprint,
        created_at: value.created_at.timestamp(),
    }
}

fn api_agent_pending_interaction_from_state(
    value: BackgroundAgentPendingInteraction,
) -> AgentPendingInteraction {
    AgentPendingInteraction {
        interaction_id: value.id,
        agent_id: value.run_id,
        worker_request_id: value.worker_request_id,
        kind: match value.kind {
            BackgroundAgentPendingInteractionKind::Approval => {
                AgentPendingInteractionKind::Approval
            }
            BackgroundAgentPendingInteractionKind::UserInput => {
                AgentPendingInteractionKind::UserInput
            }
            BackgroundAgentPendingInteractionKind::McpElicitation => {
                AgentPendingInteractionKind::McpElicitation
            }
            BackgroundAgentPendingInteractionKind::PermissionGrant => {
                AgentPendingInteractionKind::PermissionGrant
            }
        },
        status: api_agent_pending_interaction_status_from_background(value.status),
        request_payload: value.request_payload_json,
        response_payload: value.response_payload_json,
        no_client_policy: value.no_client_policy,
        timeout_at: value.timeout_at.map(|timestamp| timestamp.timestamp()),
        created_at: value.created_at.timestamp(),
        delivered_at: value.delivered_at.map(|timestamp| timestamp.timestamp()),
        responded_at: value.responded_at.map(|timestamp| timestamp.timestamp()),
        updated_at: value.updated_at.timestamp(),
    }
}

pub(crate) async fn api_worktree_from_state(
    state_db: &codex_state::StateRuntime,
    value: codex_state::ManagedWorktree,
) -> Result<Worktree, JSONRPCErrorError> {
    let agent = match value.owner_agent_run_id.as_deref() {
        Some(agent_run_id) => state_db
            .get_run(agent_run_id)
            .await
            .map_err(|err| internal_error(format!("failed to load worktree agent: {err}")))?
            .map(api_agent_run_from_state),
        None => None,
    };
    Ok(Worktree {
        worktree_id: value.worktree_id,
        agent_id: value.owner_agent_run_id.clone(),
        identity: value.identity,
        mode: api_worktree_mode(value.mode),
        lifecycle_status: api_worktree_lifecycle_status(value.lifecycle_status),
        base_repo_path: path_to_api_string(value.base_repo_path.as_path()),
        worktree_path: path_to_api_string(value.worktree_path.as_path()),
        branch: value.branch,
        base_sha: value.base_sha,
        head_sha: value.head_sha,
        status_snapshot: value.status_snapshot_json,
        dirty: value.dirty,
        cleanup_policy: api_worktree_cleanup_policy(value.cleanup_policy),
        cleanup_after: value.cleanup_after.map(|timestamp| timestamp.timestamp()),
        force_delete_requested: value.force_delete_requested,
        owner_kind: api_worktree_owner_kind(value.owner_kind),
        owner_thread_id: value.owner_thread_id.map(|thread_id| thread_id.to_string()),
        owner_agent_run_id: value.owner_agent_run_id,
        created_at: value.created_at.timestamp(),
        updated_at: value.updated_at.timestamp(),
        released_at: value.released_at.map(|timestamp| timestamp.timestamp()),
        deleted_at: value.deleted_at.map(|timestamp| timestamp.timestamp()),
        agent,
    })
}

pub(crate) fn api_worktree_merge_candidate_from_state(
    value: codex_state::ManagedWorktreeMergeCandidate,
) -> WorktreeMergeCandidate {
    WorktreeMergeCandidate {
        candidate_id: value.candidate_id,
        worktree_id: value.worktree_id,
        target_ref: value.target_ref,
        target_sha: value.target_sha,
        base_sha: value.base_sha,
        head_sha: value.head_sha,
        status: api_worktree_merge_candidate_status(value.status),
        conflict_summary: value.conflict_summary,
        created_at: value.created_at.timestamp(),
        updated_at: value.updated_at.timestamp(),
        applied_at: value.applied_at.map(|timestamp| timestamp.timestamp()),
        dismissed_at: value.dismissed_at.map(|timestamp| timestamp.timestamp()),
    }
}

fn api_worktree_mode(mode: codex_state::ManagedWorktreeMode) -> WorktreeMode {
    match mode {
        codex_state::ManagedWorktreeMode::IsolatedWorktree => WorktreeMode::IsolatedWorktree,
        codex_state::ManagedWorktreeMode::SharedRepository => WorktreeMode::SharedRepository,
    }
}

fn api_worktree_lifecycle_status(
    lifecycle_status: codex_state::ManagedWorktreeLifecycleStatus,
) -> WorktreeLifecycleStatus {
    match lifecycle_status {
        codex_state::ManagedWorktreeLifecycleStatus::Active => WorktreeLifecycleStatus::Active,
        codex_state::ManagedWorktreeLifecycleStatus::CleanupPending => {
            WorktreeLifecycleStatus::CleanupPending
        }
        codex_state::ManagedWorktreeLifecycleStatus::Released => WorktreeLifecycleStatus::Released,
        codex_state::ManagedWorktreeLifecycleStatus::Deleted => WorktreeLifecycleStatus::Deleted,
    }
}

fn api_worktree_cleanup_policy(
    cleanup_policy: codex_state::ManagedWorktreeCleanupPolicy,
) -> WorktreeCleanupPolicy {
    match cleanup_policy {
        codex_state::ManagedWorktreeCleanupPolicy::Retain => WorktreeCleanupPolicy::Retain,
        codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean => {
            WorktreeCleanupPolicy::DeleteIfClean
        }
        codex_state::ManagedWorktreeCleanupPolicy::ForceDelete => {
            WorktreeCleanupPolicy::ForceDelete
        }
    }
}

fn api_worktree_owner_kind(owner_kind: codex_state::ManagedWorktreeOwnerKind) -> WorktreeOwnerKind {
    match owner_kind {
        codex_state::ManagedWorktreeOwnerKind::Manual => WorktreeOwnerKind::Manual,
        codex_state::ManagedWorktreeOwnerKind::MainSession => WorktreeOwnerKind::MainSession,
        codex_state::ManagedWorktreeOwnerKind::SubSession => WorktreeOwnerKind::SubSession,
        codex_state::ManagedWorktreeOwnerKind::BackgroundAgent => {
            WorktreeOwnerKind::BackgroundAgent
        }
    }
}

fn api_worktree_merge_candidate_status(
    status: codex_state::ManagedWorktreeMergeCandidateStatus,
) -> WorktreeMergeCandidateStatus {
    match status {
        codex_state::ManagedWorktreeMergeCandidateStatus::Open => {
            WorktreeMergeCandidateStatus::Open
        }
        codex_state::ManagedWorktreeMergeCandidateStatus::Blocked => {
            WorktreeMergeCandidateStatus::Blocked
        }
        codex_state::ManagedWorktreeMergeCandidateStatus::Applied => {
            WorktreeMergeCandidateStatus::Applied
        }
        codex_state::ManagedWorktreeMergeCandidateStatus::Dismissed => {
            WorktreeMergeCandidateStatus::Dismissed
        }
    }
}

fn api_agent_run_status_from_background(status: BackgroundAgentRunStatus) -> AgentRunStatus {
    match status {
        BackgroundAgentRunStatus::Queued => AgentRunStatus::Queued,
        BackgroundAgentRunStatus::Starting => AgentRunStatus::Starting,
        BackgroundAgentRunStatus::Running => AgentRunStatus::Running,
        BackgroundAgentRunStatus::WaitingOnApproval => AgentRunStatus::WaitingOnApproval,
        BackgroundAgentRunStatus::WaitingOnUser => AgentRunStatus::WaitingOnUser,
        BackgroundAgentRunStatus::Stopping => AgentRunStatus::Stopping,
        BackgroundAgentRunStatus::Completed => AgentRunStatus::Completed,
        BackgroundAgentRunStatus::Failed => AgentRunStatus::Failed,
        BackgroundAgentRunStatus::Cancelled => AgentRunStatus::Cancelled,
        BackgroundAgentRunStatus::Orphaned => AgentRunStatus::Orphaned,
    }
}

fn api_agent_desired_state_from_background(
    desired_state: BackgroundAgentDesiredState,
) -> AgentDesiredState {
    match desired_state {
        BackgroundAgentDesiredState::Running => AgentDesiredState::Running,
        BackgroundAgentDesiredState::Stopped => AgentDesiredState::Stopped,
        BackgroundAgentDesiredState::Deleted => AgentDesiredState::Deleted,
    }
}

fn api_agent_pending_interaction_status_from_background(
    status: BackgroundAgentPendingInteractionStatus,
) -> AgentPendingInteractionStatus {
    match status {
        BackgroundAgentPendingInteractionStatus::Pending => AgentPendingInteractionStatus::Pending,
        BackgroundAgentPendingInteractionStatus::Delivered => {
            AgentPendingInteractionStatus::Delivered
        }
        BackgroundAgentPendingInteractionStatus::Responded => {
            AgentPendingInteractionStatus::Responded
        }
        BackgroundAgentPendingInteractionStatus::Expired => AgentPendingInteractionStatus::Expired,
        BackgroundAgentPendingInteractionStatus::Cancelled => {
            AgentPendingInteractionStatus::Cancelled
        }
        BackgroundAgentPendingInteractionStatus::Denied => AgentPendingInteractionStatus::Denied,
        BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting => {
            AgentPendingInteractionStatus::WorkerNoLongerWaiting
        }
    }
}

fn background_pending_terminal_status_from_api(
    status: AgentPendingInteractionTerminalStatus,
) -> BackgroundAgentPendingInteractionStatus {
    match status {
        AgentPendingInteractionTerminalStatus::Responded => {
            BackgroundAgentPendingInteractionStatus::Responded
        }
        AgentPendingInteractionTerminalStatus::Expired => {
            BackgroundAgentPendingInteractionStatus::Expired
        }
        AgentPendingInteractionTerminalStatus::Cancelled => {
            BackgroundAgentPendingInteractionStatus::Cancelled
        }
        AgentPendingInteractionTerminalStatus::Denied => {
            BackgroundAgentPendingInteractionStatus::Denied
        }
        AgentPendingInteractionTerminalStatus::WorkerNoLongerWaiting => {
            BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting
        }
    }
}

fn api_lifecycle_effect_from_runtime(effect: LifecycleEffect) -> AgentLifecycleEffect {
    match effect {
        LifecycleEffect::ReplayState => AgentLifecycleEffect::ReplayState,
        LifecycleEffect::RemoveSubscriberOnly => AgentLifecycleEffect::RemoveSubscriberOnly,
        LifecycleEffect::RequestWorkerStop => AgentLifecycleEffect::RequestWorkerStop,
        LifecycleEffect::MarkDeleteRequested => AgentLifecycleEffect::MarkDeleteRequested,
        LifecycleEffect::KeepWorkerRunning => AgentLifecycleEffect::KeepWorkerRunning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn agent_stop_with_active_pending_interaction_keeps_snapshot_in_sync()
    -> anyhow::Result<()> {
        let (_temp, state_db) = temp_state_db().await?;
        seed_test_agent_run(state_db.as_ref(), "stop-pending-run").await?;
        create_test_pending_interaction(
            state_db.as_ref(),
            "stop-pending-1",
            "stop-pending-run",
            BackgroundAgentPendingInteractionKind::UserInput,
        )
        .await?;
        let processor = BackgroundAgentRequestProcessor::new(Some(state_db.clone()));

        let response = processor
            .agent_stop_inner(AgentStopParams {
                agent_id: "stop-pending-run".to_string(),
            })
            .await
            .expect("stop should succeed");

        let stopped_agent = response.agent.expect("stop should return agent");
        assert_eq!(AgentRunStatus::Cancelled, stopped_agent.status);
        assert_no_active_pending_interactions_and_current_snapshot(
            state_db.as_ref(),
            "stop-pending-run",
            "stop-pending-1",
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn agent_delete_with_active_pending_interaction_keeps_snapshot_in_sync()
    -> anyhow::Result<()> {
        let (_temp, state_db) = temp_state_db().await?;
        seed_test_agent_run(state_db.as_ref(), "delete-pending-run").await?;
        create_test_pending_interaction(
            state_db.as_ref(),
            "delete-pending-1",
            "delete-pending-run",
            BackgroundAgentPendingInteractionKind::Approval,
        )
        .await?;
        let processor = BackgroundAgentRequestProcessor::new(Some(state_db.clone()));

        let response = processor
            .agent_delete_inner(AgentDeleteParams {
                agent_id: "delete-pending-run".to_string(),
            })
            .await
            .expect("delete should succeed");

        let deleted_agent = response.agent.expect("delete should return agent");
        assert_eq!(AgentRunStatus::Cancelled, deleted_agent.status);
        assert_eq!(AgentDesiredState::Deleted, deleted_agent.desired_state);
        assert_no_active_pending_interactions_and_current_snapshot(
            state_db.as_ref(),
            "delete-pending-run",
            "delete-pending-1",
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn validate_agent_start_rollout_path_requires_matching_thread_metadata()
    -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let state_db = codex_state::StateRuntime::init(
            codex_home.path().to_path_buf(),
            "mock_provider".to_string(),
        )
        .await?;
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000321")?;
        let thread_id_string = thread_id.to_string();
        let rollout_path = codex_home.path().join("owned-rollout.jsonl");
        let now = Utc::now();

        validate_agent_start_rollout_path(
            state_db.as_ref(),
            /*thread_id*/ None,
            /*rollout_path*/ None,
        )
        .await
        .expect("missing rolloutPath should be accepted");
        let missing_rollout_path = validate_agent_start_rollout_path(
            state_db.as_ref(),
            Some(thread_id_string.as_str()),
            /*rollout_path*/ None,
        )
        .await
        .expect_err("threadId without rolloutPath should be rejected");
        assert_eq!(
            missing_rollout_path.message,
            "agent/start threadId requires rolloutPath"
        );
        let missing_thread_id = validate_agent_start_rollout_path(
            state_db.as_ref(),
            /*thread_id*/ None,
            Some(rollout_path.to_string_lossy().as_ref()),
        )
        .await
        .expect_err("rolloutPath without threadId should be rejected");
        assert_eq!(
            missing_thread_id.message,
            "agent/start rolloutPath requires threadId"
        );

        let unknown_thread = validate_agent_start_rollout_path(
            state_db.as_ref(),
            Some(thread_id_string.as_str()),
            Some(rollout_path.to_string_lossy().as_ref()),
        )
        .await
        .expect_err("unknown thread should be rejected");
        assert_eq!(
            unknown_thread.message,
            "agent/start rolloutPath requires a known threadId"
        );

        state_db
            .upsert_thread(&codex_state::ThreadMetadata {
                id: thread_id,
                rollout_path: rollout_path.clone(),
                created_at: now,
                updated_at: now,
                source: "cli".to_string(),
                thread_source: None,
                agent_nickname: None,
                agent_role: None,
                agent_path: None,
                model_provider: "mock_provider".to_string(),
                model: Some("mock-model".to_string()),
                reasoning_effort: None,
                cwd: codex_home.path().to_path_buf(),
                cli_version: "0.0.0".to_string(),
                title: String::new(),
                preview: Some("resume target".to_string()),
                sandbox_policy: "read-only".to_string(),
                approval_mode: "never".to_string(),
                tokens_used: 0,
                first_user_message: Some("resume target".to_string()),
                archived_at: None,
                git_sha: None,
                git_branch: None,
                git_origin_url: None,
            })
            .await?;

        validate_agent_start_rollout_path(
            state_db.as_ref(),
            Some(thread_id_string.as_str()),
            Some(rollout_path.to_string_lossy().as_ref()),
        )
        .await
        .expect("matching rolloutPath should be accepted");
        let mismatched_thread = validate_agent_start_rollout_path(
            state_db.as_ref(),
            Some(thread_id_string.as_str()),
            Some(
                codex_home
                    .path()
                    .join("different-rollout.jsonl")
                    .to_string_lossy()
                    .as_ref(),
            ),
        )
        .await
        .expect_err("mismatched rolloutPath should be rejected");
        assert_eq!(
            mismatched_thread.message,
            "agent/start rolloutPath must match threadId"
        );

        Ok(())
    }

    async fn temp_state_db() -> anyhow::Result<(TempDir, StateDbHandle)> {
        let temp = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(temp.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        state_db
            .mark_backfill_complete(/*last_watermark*/ None)
            .await?;
        Ok((temp, state_db))
    }

    async fn seed_test_agent_run(
        state_db: &codex_state::StateRuntime,
        agent_id: &str,
    ) -> anyhow::Result<()> {
        state_db
            .create_background_agent_run(&BackgroundAgentRunCreateParams {
                id: agent_id.to_string(),
                idempotency_key: None,
                request_id: None,
                source: "processor-test".to_string(),
                prompt_snapshot_ref: format!("inline:{agent_id}:prompt"),
                input_snapshot_ref: None,
                thread_id: None,
                thread_store_kind: "background-agent".to_string(),
                thread_store_id: None,
                rollout_path: None,
                parent_thread_id: None,
                parent_agent_run_id: None,
                spawn_linkage_json: None,
                auth_profile_ref: None,
                status_reason: Some("queued by processor test".to_string()),
                config_fingerprint: Some("cfg-test".to_string()),
                version_fingerprint: Some("version-test".to_string()),
            })
            .await?;
        state_db
            .append_background_agent_event(
                agent_id,
                "agent.started",
                &json!({
                    "promptSnapshotRef": format!("inline:{agent_id}:prompt"),
                }),
            )
            .await?;
        Ok(())
    }

    async fn create_test_pending_interaction(
        state_db: &codex_state::StateRuntime,
        interaction_id: &str,
        agent_id: &str,
        kind: BackgroundAgentPendingInteractionKind,
    ) -> anyhow::Result<()> {
        state_db
            .create_background_agent_pending_interaction(
                &codex_background_agent::BackgroundAgentPendingInteractionCreateParams {
                    id: interaction_id.to_string(),
                    run_id: agent_id.to_string(),
                    worker_request_id: Some(format!("{interaction_id}-worker-request")),
                    kind,
                    request_payload_json: json!({"prompt": "continue?"}),
                    no_client_policy: "cancel".to_string(),
                    timeout_at: None,
                },
            )
            .await?;
        Ok(())
    }

    async fn assert_no_active_pending_interactions_and_current_snapshot(
        state_db: &codex_state::StateRuntime,
        agent_id: &str,
        interaction_id: &str,
    ) -> anyhow::Result<()> {
        let run = state_db
            .get_background_agent_run(agent_id)
            .await?
            .expect("run should exist");
        let snapshot = state_db
            .get_background_agent_status_snapshot(agent_id)
            .await?
            .expect("status snapshot should exist");
        assert_eq!(run.last_event_seq, snapshot.last_event_seq);
        assert_eq!(0, snapshot.pending_interaction_count);
        let interaction = state_db
            .get_background_agent_pending_interaction(interaction_id)
            .await?
            .expect("interaction should exist");
        assert_eq!(
            BackgroundAgentPendingInteractionStatus::Cancelled,
            interaction.status
        );
        Ok(())
    }
}
