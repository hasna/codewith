use crate::error_code::internal_error;
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
use codex_rollout::StateDbHandle;
use serde_json::json;
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
            state_db
                .append_event(
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
                .list_events_after(run.id.as_str(), None, Some(1))
                .await
                .map_err(|err| {
                    internal_error(format!("failed to list background agent events: {err}"))
                })?;
            match events.pop() {
                Some(event) => event,
                None => state_db
                    .append_event(
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
            .list_runs(Some(offset.saturating_add(limit).saturating_add(1)))
            .await
            .map_err(|err| internal_error(format!("failed to list background agents: {err}")))?;
        let has_more = runs.len() > offset.saturating_add(limit);
        runs = runs.into_iter().skip(offset).take(limit).collect();
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
        expire_timed_out_pending_interactions(state_db.as_ref()).await?;
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
            .list_pending_interactions(run.id.as_str(), None)
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
        let limit = normalize_agent_list_limit(params.limit)?;
        let after_seq = decode_event_cursor(params.cursor.as_deref())?;
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
        let pending_interactions = state_db
            .list_pending_interactions(run.id.as_str(), None)
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
            let event = state_db
                .append_event(
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
            if terminalize_immediately {
                cancel_active_pending_interactions_for_run(
                    state_db.as_ref(),
                    run.id.as_str(),
                    "client_requested_stop",
                )
                .await?;
                upsert_lifecycle_status_snapshot(
                    state_db.as_ref(),
                    run.id.as_str(),
                    status,
                    event.seq,
                    "Stopped",
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
            let terminalized_immediately = existing_run.as_ref().is_some_and(|run| {
                !is_terminal_agent_status(run.status) && should_terminalize_unclaimed_agent_run(run)
            });
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
            let event = state_db
                .append_event(
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
            if terminalized_immediately {
                cancel_active_pending_interactions_for_run(
                    state_db.as_ref(),
                    params.agent_id.as_str(),
                    "client_requested_delete",
                )
                .await?;
                upsert_lifecycle_status_snapshot(
                    state_db.as_ref(),
                    params.agent_id.as_str(),
                    BackgroundAgentRunStatus::Cancelled,
                    event.seq,
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
        if updated {
            state_db
                .append_event(
                    params.agent_id.as_str(),
                    "agent.pendingInteractionResponded",
                    &json!({
                        "interactionId": params.interaction_id,
                        "terminalStatus": api_agent_pending_interaction_status_from_background(
                            terminal_status
                        ),
                    }),
                )
                .await
                .map_err(|err| {
                    internal_error(format!("failed to append background agent event: {err}"))
                })?;
        }
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

async fn cancel_active_pending_interactions_for_run(
    state_db: &codex_state::StateRuntime,
    run_id: &str,
    reason: &str,
) -> Result<(), JSONRPCErrorError> {
    let interactions = state_db
        .list_pending_interactions(run_id, None)
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
    event_seq: i64,
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
        .list_pending_interactions(run_id, None)
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
            seq: event_seq,
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
