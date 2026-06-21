use super::background_agent_processor::BackgroundAgentRequestProcessor;
use super::background_agent_processor::api_worktree_from_state;
use super::background_agent_processor::api_worktree_merge_candidate_from_state;
use super::thread_processor::ThreadRequestProcessor;
use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use anyhow::Context;
use chrono::DateTime;
use chrono::Duration as ChronoDuration;
use chrono::Utc;
use codex_app_server_protocol::AgentAttachParams;
use codex_app_server_protocol::AgentDaemonDiagnosticsParams;
use codex_app_server_protocol::AgentDeleteParams;
use codex_app_server_protocol::AgentDetachParams;
use codex_app_server_protocol::AgentEventsListParams;
use codex_app_server_protocol::AgentExecutionContextParams;
use codex_app_server_protocol::AgentListParams;
use codex_app_server_protocol::AgentPendingInteractionRespondParams;
use codex_app_server_protocol::AgentReadParams;
use codex_app_server_protocol::AgentStartParams;
use codex_app_server_protocol::AgentStopParams;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::WorktreeAttachParams;
use codex_app_server_protocol::WorktreeCleanupParams;
use codex_app_server_protocol::WorktreeCleanupPolicy;
use codex_app_server_protocol::WorktreeCleanupResponse;
use codex_app_server_protocol::WorktreeCreateParams;
use codex_app_server_protocol::WorktreeCreateResponse;
use codex_app_server_protocol::WorktreeDetachParams;
use codex_app_server_protocol::WorktreeListParams;
use codex_app_server_protocol::WorktreeListResponse;
use codex_app_server_protocol::WorktreeMergeCandidateApplyParams;
use codex_app_server_protocol::WorktreeMergeCandidateApplyResponse;
use codex_app_server_protocol::WorktreeMergeCandidateDismissParams;
use codex_app_server_protocol::WorktreeMergeCandidateDismissResponse;
use codex_app_server_protocol::WorktreeMergeCandidateListParams;
use codex_app_server_protocol::WorktreeMergeCandidateListResponse;
use codex_app_server_protocol::WorktreeMergeCandidateRefreshParams;
use codex_app_server_protocol::WorktreeMergeCandidateRefreshResponse;
use codex_app_server_protocol::WorktreeMergeCandidateStatus;
use codex_app_server_protocol::WorktreePolicy;
use codex_app_server_protocol::WorktreeReadParams;
use codex_app_server_protocol::WorktreeReadResponse;
use codex_app_server_protocol::WorktreeReconcileParams;
use codex_app_server_protocol::WorktreeReconcileResponse;
use codex_app_server_protocol::WorktreeReleaseParams;
use codex_app_server_protocol::WorktreeReleaseResponse;
use codex_app_server_protocol::WorktreeSessionMode;
use codex_background_agent::AgentEventJournal;
use codex_background_agent::AgentRunStore;
use codex_background_agent::AgentSnapshotStore;
use codex_background_agent::BackgroundAgentDesiredState;
use codex_background_agent::BackgroundAgentEvent;
use codex_background_agent::BackgroundAgentExecutionHandleParams;
use codex_background_agent::BackgroundAgentExecutionSnapshotParams;
use codex_background_agent::BackgroundAgentPendingInteraction;
use codex_background_agent::BackgroundAgentPendingInteractionCreateParams;
use codex_background_agent::BackgroundAgentPendingInteractionKind;
use codex_background_agent::BackgroundAgentPendingInteractionStatus;
use codex_background_agent::BackgroundAgentRun;
use codex_background_agent::BackgroundAgentRunStatus;
use codex_background_agent::BackgroundAgentStatusEventForSupervisorParams;
use codex_background_agent::BackgroundAgentStatusSnapshotParams;
use codex_background_agent::BackgroundAgentThreadBindingParams;
use codex_background_agent::BackgroundAgentWorkspaceCleanup;
use codex_background_agent::PendingInteractionLedger;
use codex_background_agent::daemon::background_agent_daemon_state_dir;
use codex_background_agent::process_lifecycle::WorkerProcessCommand;
use codex_background_agent::process_lifecycle::WorkerProcessController;
use codex_background_agent::process_lifecycle::WorkerProcessHandle;
use codex_background_agent::process_lifecycle::WorkerProcessStatus;
use codex_core::NewThread;
use codex_core::StartThreadOptions;
use codex_core::config::ConfigOverrides;
use codex_core::config::WorktreeCleanupMode as CoreWorktreeCleanupMode;
use codex_core::config::WorktreeSessionMode as CoreWorktreeSessionMode;
use codex_exec_server::LOCAL_FS;
use codex_git_utils::GitWorktreeAddOptions;
use codex_git_utils::GitWorktreeStatusSnapshot;
use codex_git_utils::add_linked_git_worktree;
use codex_git_utils::fast_forward_merge_ref;
use codex_git_utils::get_git_worktree_status_snapshot;
use codex_git_utils::list_git_worktrees;
use codex_git_utils::merge_tree_dry_run;
use codex_git_utils::remove_linked_git_worktree;
use codex_git_utils::resolve_git_ref;
use codex_git_utils::resolve_root_git_project_for_trust;
use codex_git_utils::validate_git_branch_name;
use codex_git_utils::worktree_has_commits_after;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadSource;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::request_permissions::PermissionGrantScope;
use codex_protocol::request_permissions::RequestPermissionProfile;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use codex_rollout::StateDbHandle;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::future::join_all;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::debug;
use tracing::error;
use tracing::warn;
use uuid::Uuid;

const BACKGROUND_AGENT_SUPERVISOR_PREFIX: &str = "app-server-background-agent";
const BACKGROUND_AGENT_THREAD_STORE_KIND: &str = "thread-store";
const BACKGROUND_AGENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const BACKGROUND_AGENT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
const BACKGROUND_AGENT_INTERACTION_POLL_INTERVAL: Duration = Duration::from_millis(250);
const BACKGROUND_AGENT_INTERACTION_TIMEOUT: ChronoDuration = ChronoDuration::hours(12);
const BACKGROUND_AGENT_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const BACKGROUND_AGENT_WORKER_STDERR_DIR: &str = "workers";
const MANAGED_WORKTREE_CLEANUP_BATCH_LIMIT: u32 = 20;
const MANAGED_WORKTREE_CLEANUP_RETRY_DELAY: ChronoDuration = ChronoDuration::minutes(15);
const BACKGROUND_AGENT_USAGE_PROFILE_WAIT_PREFIX: &str = "usage_profile_wait_until:";

#[derive(Debug)]
struct BackgroundAgentOwnershipLost {
    run_id: String,
    generation: i64,
}

impl std::fmt::Display for BackgroundAgentOwnershipLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "background agent worker lost ownership of run {} generation {}",
            self.run_id, self.generation
        )
    }
}

impl std::error::Error for BackgroundAgentOwnershipLost {}

fn background_agent_ownership_lost(run_id: &str, generation: i64) -> anyhow::Error {
    BackgroundAgentOwnershipLost {
        run_id: run_id.to_string(),
        generation,
    }
    .into()
}

fn is_background_agent_ownership_lost(err: &anyhow::Error) -> bool {
    err.downcast_ref::<BackgroundAgentOwnershipLost>().is_some()
}

fn api_worktree_cleanup_policy_from_config(mode: CoreWorktreeCleanupMode) -> WorktreeCleanupPolicy {
    match mode {
        CoreWorktreeCleanupMode::Retain => WorktreeCleanupPolicy::Retain,
        CoreWorktreeCleanupMode::DeleteIfClean => WorktreeCleanupPolicy::DeleteIfClean,
        CoreWorktreeCleanupMode::ForceDelete => WorktreeCleanupPolicy::ForceDelete,
    }
}

fn api_worktree_session_mode_from_config(mode: CoreWorktreeSessionMode) -> WorktreeSessionMode {
    match mode {
        CoreWorktreeSessionMode::Off => WorktreeSessionMode::Off,
        CoreWorktreeSessionMode::Manual => WorktreeSessionMode::Manual,
        CoreWorktreeSessionMode::Auto => WorktreeSessionMode::Auto,
    }
}

impl ThreadRequestProcessor {
    pub(crate) fn start_background_agent_supervisor(&self) {
        let Some(context) = self.background_agent_worker_context() else {
            return;
        };
        let cancel_token = self.background_agent_supervisor_token.clone();
        self.background_tasks.spawn(async move {
            let mut interval = tokio::time::interval(BACKGROUND_AGENT_RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if let Err(err) =
                    reconcile_background_agents(context.clone(), /*only_run_id*/ None).await
                {
                    warn!("background agent reconcile failed: {err}");
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {}
                }
            }
        });
    }

    pub(crate) fn start_background_agent_process_supervisor(&self) {
        let Some(context) = self
            .background_agent_worker_context()
            .map(|context| context.process_supervisor_context())
        else {
            return;
        };
        let cancel_token = self.background_agent_supervisor_token.clone();
        self.background_tasks.spawn(async move {
            let mut interval = tokio::time::interval(BACKGROUND_AGENT_RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if let Err(err) =
                    reconcile_background_agent_worker_processes(context.clone(), None).await
                {
                    warn!("background agent process reconcile failed: {err}");
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {}
                }
            }
        });
    }

    pub(crate) async fn run_background_agent_worker_once(
        &self,
        run_id: String,
    ) -> anyhow::Result<()> {
        let Some(context) = self.background_agent_worker_context() else {
            anyhow::bail!("background-agent worker requires a state db");
        };
        context
            .state_db
            .orphan_stale_background_agent_runs(BACKGROUND_AGENT_HEARTBEAT_TIMEOUT)
            .await?;
        let Some(run) = context.state_db.get_run(run_id.as_str()).await? else {
            anyhow::bail!("background-agent run not found: {run_id}");
        };
        if !should_start_background_run(&run) {
            debug!(run_id = %run.id, status = ?run.status, "background-agent worker had nothing to start");
            return Ok(());
        }
        let token = CancellationToken::new();
        {
            let mut active = context.active_workers.lock().await;
            active.insert(run.id.clone(), token.clone());
        }
        let result = run_background_agent_worker(context.clone(), run.clone(), token).await;
        context.active_workers.lock().await.remove(run.id.as_str());
        match &result {
            Err(err) if is_background_agent_ownership_lost(err) => {
                debug!(run_id = %run.id, "background-agent worker exited after losing ownership");
                return Ok(());
            }
            Err(err) => {
                mark_background_agent_worker_failed(&context, run.id.as_str(), err).await?;
            }
            Ok(()) => {}
        }
        result
    }

    pub(crate) async fn cancel_background_agent_workers(&self) {
        self.background_agent_supervisor_token.cancel();
        let tokens = {
            let mut active = self.background_agent_workers.lock().await;
            active.drain().map(|(_, token)| token).collect::<Vec<_>>()
        };
        for token in tokens {
            token.cancel();
        }
        let process_handles = {
            let mut active = self.background_agent_worker_processes.lock().await;
            active.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };
        let stop_tasks = process_handles.into_iter().map(|handle| async move {
            let controller = WorkerProcessController::default();
            if let Err(err) = controller.stop(&handle).await {
                warn!("failed to stop background-agent worker process: {err}");
            }
        });
        join_all(stop_tasks).await;
    }

    pub(crate) async fn agent_start(
        &self,
        mut params: AgentStartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.freeze_start_execution_context(&mut params);
        let response = self
            .background_agent_state_processor()
            .agent_start_inner(params)
            .await?;
        self.spawn_background_agent_reconcile(Some(response.agent.agent_id.clone()));
        Ok(Some(response.into()))
    }

    pub(crate) async fn agent_list(
        &self,
        params: AgentListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn agent_read(
        &self,
        params: AgentReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_read_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn agent_attach(
        &self,
        params: AgentAttachParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_attach_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn agent_detach(
        &self,
        params: AgentDetachParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_detach_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn agent_stop(
        &self,
        params: AgentStopParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let agent_id = params.agent_id.clone();
        let response = self
            .background_agent_state_processor()
            .agent_stop_inner(params)
            .await?;
        self.cancel_background_agent_worker(agent_id.as_str()).await;
        Ok(Some(response.into()))
    }

    pub(crate) async fn agent_delete(
        &self,
        params: AgentDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let agent_id = params.agent_id.clone();
        let response = self
            .background_agent_state_processor()
            .agent_delete_inner(params)
            .await?;
        self.cancel_background_agent_worker(agent_id.as_str()).await;
        Ok(Some(response.into()))
    }

    pub(crate) async fn agent_events_list(
        &self,
        params: AgentEventsListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_events_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn agent_pending_interaction_respond(
        &self,
        params: AgentPendingInteractionRespondParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_pending_interaction_respond_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn agent_daemon_diagnostics(
        &self,
        _params: AgentDaemonDiagnosticsParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.background_agent_state_processor()
            .agent_daemon_diagnostics_inner()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn worktree_list(
        &self,
        mut params: WorktreeListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let base_repo_path = self
            .resolve_worktree_base_repo_path(params.base_repo_path.as_deref())
            .await?;
        let policy = self.worktree_policy(base_repo_path.as_deref());
        let Some(base_repo_path) = base_repo_path else {
            return Ok(Some(
                WorktreeListResponse {
                    data: Vec::new(),
                    next_cursor: None,
                    policy,
                }
                .into(),
            ));
        };
        params.base_repo_path = Some(base_repo_path.to_string_lossy().into_owned());
        self.background_agent_state_processor()
            .worktree_list_inner(params, policy)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn worktree_read(
        &self,
        params: WorktreeReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let base_repo_path = self
            .resolve_worktree_base_repo_path(params.base_repo_path.as_deref())
            .await?;
        let policy = self.worktree_policy(base_repo_path.as_deref());
        let Some(base_repo_path) = base_repo_path else {
            return Ok(Some(
                WorktreeReadResponse {
                    worktree: None,
                    policy,
                }
                .into(),
            ));
        };
        self.background_agent_state_processor()
            .worktree_read_inner(params, Some(base_repo_path.as_path()), policy)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn worktree_create(
        &self,
        params: WorktreeCreateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let base_repo_path = self
            .resolve_worktree_base_repo_path(params.base_repo_path.as_deref())
            .await?
            .ok_or_else(|| invalid_params("worktree/create requires a git repository"))?;
        let policy = self.worktree_policy(Some(base_repo_path.as_path()));
        ensure_worktree_policy_enabled(&policy)?;
        let attach_thread_id = params
            .thread_id
            .as_deref()
            .map(codex_protocol::ThreadId::from_string)
            .transpose()
            .map_err(|err| invalid_params(format!("invalid threadId: {err}")))?;
        if attach_thread_id.is_some() && policy.main_sessions == WorktreeSessionMode::Off {
            return Err(invalid_params(
                "main-session worktrees are disabled in config",
            ));
        }

        let worktree_id = Uuid::new_v4().to_string();
        let branch = worktree_branch_name(
            params.branch.as_deref(),
            params.name.as_deref(),
            worktree_id.as_str(),
        )?;
        let valid_branch_name = run_git_worktree_task("failed to validate worktree branch name", {
            let base_repo_path = base_repo_path.clone();
            let branch = branch.clone();
            move || validate_git_branch_name(base_repo_path.as_path(), branch.as_str())
        })
        .await?;
        if !valid_branch_name {
            return Err(invalid_params(format!(
                "worktree/create branch `{branch}` is not a valid git branch name"
            )));
        }
        let start_point = params
            .start_point
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD")
            .to_string();
        let worktree_root = self.worktree_root_path(base_repo_path.as_path())?;
        std::fs::create_dir_all(worktree_root.as_path()).map_err(|err| {
            internal_error(format!(
                "failed to create worktree root {}: {err}",
                worktree_root.display()
            ))
        })?;
        let worktree_path = worktree_root.join(worktree_path_name(
            params.name.as_deref().or(Some(branch.as_str())),
            worktree_id.as_str(),
        ));
        let base_sha = run_git_worktree_task("failed to resolve worktree start point", {
            let base_repo_path = base_repo_path.clone();
            let start_point = start_point.clone();
            move || resolve_git_ref(base_repo_path.as_path(), start_point.as_str())
        })
        .await?
        .ok_or_else(|| {
            invalid_params(format!(
                "worktree/create startPoint `{start_point}` does not resolve to a commit"
            ))
        })?;
        let entry = run_git_worktree_task("failed to create git worktree", {
            let base_repo_path = base_repo_path.clone();
            let worktree_path = worktree_path.clone();
            let branch = branch.clone();
            let start_point = start_point.clone();
            move || {
                add_linked_git_worktree(
                    base_repo_path.as_path(),
                    GitWorktreeAddOptions {
                        worktree_path,
                        branch,
                        start_point,
                    },
                )
            }
        })
        .await?;
        let status_snapshot = run_git_worktree_task("failed to inspect created worktree", {
            let worktree_path = worktree_path.clone();
            move || get_git_worktree_status_snapshot(worktree_path.as_path())
        })
        .await?;
        let cleanup_policy = params
            .cleanup_policy
            .map(state_worktree_cleanup_policy)
            .unwrap_or_else(|| state_worktree_cleanup_policy(policy.cleanup_default));
        let create_result = state_db
            .managed_worktrees()
            .create_managed_worktree(codex_state::ManagedWorktreeCreateParams {
                worktree_id: Some(worktree_id.clone()),
                identity: Some(format!("manual:{worktree_id}")),
                mode: codex_state::ManagedWorktreeMode::IsolatedWorktree,
                base_repo_path: base_repo_path.clone(),
                worktree_path: worktree_path.clone(),
                branch: status_snapshot
                    .branch
                    .clone()
                    .or_else(|| entry.branch.as_deref().map(short_branch_name)),
                base_sha: Some(base_sha),
                head_sha: status_snapshot
                    .head_sha
                    .clone()
                    .or_else(|| entry.head_sha.clone()),
                status_snapshot_json: git_status_snapshot_json(status_snapshot.clone()),
                dirty: status_snapshot.dirty,
                cleanup_policy,
                owner_kind: codex_state::ManagedWorktreeOwnerKind::Manual,
                owner_thread_id: None,
                owner_agent_run_id: None,
                cleanup_after: None,
            })
            .await;
        let mut worktree = match create_result {
            Ok(worktree) => worktree,
            Err(err) => {
                let _ = run_git_worktree_task("failed to roll back created git worktree", {
                    let base_repo_path = base_repo_path.clone();
                    let worktree_path = worktree_path.clone();
                    move || {
                        remove_linked_git_worktree(
                            base_repo_path.as_path(),
                            worktree_path.as_path(),
                            /*force*/ true,
                        )
                    }
                })
                .await;
                return Err(invalid_params(format!(
                    "failed to record managed worktree: {err}"
                )));
            }
        };
        if let Some(thread_id) = attach_thread_id {
            let target = codex_state::ManagedWorktreeAssignmentTarget::Thread(thread_id);
            worktree = state_db
                .managed_worktrees()
                .attach_managed_worktree(codex_state::ManagedWorktreeAttachParams {
                    worktree_id: worktree.worktree_id.clone(),
                    target,
                })
                .await
                .map_err(|err| invalid_params(format!("failed to attach worktree: {err}")))?;
        }
        Ok(Some(
            WorktreeCreateResponse {
                worktree: api_worktree_from_state(state_db.as_ref(), worktree).await?,
                policy,
            }
            .into(),
        ))
    }

    pub(crate) async fn worktree_reconcile(
        &self,
        params: WorktreeReconcileParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let base_repo_path = self
            .resolve_worktree_base_repo_path(params.base_repo_path.as_deref())
            .await?
            .ok_or_else(|| invalid_params("worktree/reconcile requires a git repository"))?;
        let policy = self.worktree_policy(Some(base_repo_path.as_path()));
        ensure_worktree_policy_enabled(&policy)?;
        let worktree_root = self.worktree_root_path(base_repo_path.as_path())?;
        let git_entries = run_git_worktree_task("failed to list git worktrees", {
            let base_repo_path = base_repo_path.clone();
            move || list_git_worktrees(base_repo_path.as_path())
        })
        .await?;
        let mut state_worktrees =
            load_all_managed_worktrees(state_db.as_ref(), base_repo_path.as_path()).await?;
        let codewith_entries = git_entries
            .into_iter()
            .filter(|entry| {
                !entry.is_main && path_is_inside(entry.path.as_path(), worktree_root.as_path())
            })
            .collect::<Vec<_>>();
        let mut discovered = 0_u32;
        let mut updated = 0_u32;
        for entry in &codewith_entries {
            let status_snapshot = run_git_worktree_task("failed to inspect git worktree", {
                let worktree_path = entry.path.clone();
                move || get_git_worktree_status_snapshot(worktree_path.as_path())
            })
            .await?;
            if let Some(existing) = state_worktrees.iter().find(|worktree| {
                paths_match(worktree.worktree_path.as_path(), entry.path.as_path())
            }) {
                state_db
                    .managed_worktrees()
                    .update_managed_worktree_status(
                        codex_state::ManagedWorktreeStatusUpdateParams {
                            worktree_id: existing.worktree_id.clone(),
                            branch: status_snapshot
                                .branch
                                .clone()
                                .or_else(|| entry.branch.as_deref().map(short_branch_name)),
                            head_sha: status_snapshot
                                .head_sha
                                .clone()
                                .or_else(|| entry.head_sha.clone()),
                            status_snapshot_json: git_status_snapshot_json(status_snapshot.clone()),
                            dirty: status_snapshot.dirty,
                        },
                    )
                    .await
                    .map_err(|err| {
                        internal_error(format!("failed to update managed worktree: {err}"))
                    })?;
                updated += 1;
            } else {
                let worktree_id = Uuid::new_v4().to_string();
                state_db
                    .managed_worktrees()
                    .create_managed_worktree(codex_state::ManagedWorktreeCreateParams {
                        worktree_id: Some(worktree_id.clone()),
                        identity: Some(format!("discovered:{worktree_id}")),
                        mode: codex_state::ManagedWorktreeMode::IsolatedWorktree,
                        base_repo_path: base_repo_path.clone(),
                        worktree_path: entry.path.clone(),
                        branch: status_snapshot
                            .branch
                            .clone()
                            .or_else(|| entry.branch.as_deref().map(short_branch_name)),
                        base_sha: None,
                        head_sha: status_snapshot
                            .head_sha
                            .clone()
                            .or_else(|| entry.head_sha.clone()),
                        status_snapshot_json: git_status_snapshot_json(status_snapshot.clone()),
                        dirty: status_snapshot.dirty,
                        cleanup_policy: state_worktree_cleanup_policy(policy.cleanup_default),
                        owner_kind: codex_state::ManagedWorktreeOwnerKind::Manual,
                        owner_thread_id: None,
                        owner_agent_run_id: None,
                        cleanup_after: None,
                    })
                    .await
                    .map_err(|err| {
                        internal_error(format!("failed to record discovered worktree: {err}"))
                    })?;
                discovered += 1;
            }
        }

        state_worktrees =
            load_all_managed_worktrees(state_db.as_ref(), base_repo_path.as_path()).await?;
        let mut deleted = 0_u32;
        for worktree in state_worktrees {
            if worktree.mode != codex_state::ManagedWorktreeMode::IsolatedWorktree
                || worktree.lifecycle_status == codex_state::ManagedWorktreeLifecycleStatus::Deleted
                || !path_is_inside(worktree.worktree_path.as_path(), worktree_root.as_path())
                || codewith_entries.iter().any(|entry| {
                    paths_match(entry.path.as_path(), worktree.worktree_path.as_path())
                })
            {
                continue;
            }
            if state_db
                .managed_worktrees()
                .mark_managed_worktree_deleted(worktree.worktree_id.as_str())
                .await
                .map_err(|err| {
                    internal_error(format!("failed to mark missing worktree deleted: {err}"))
                })?
                .is_some()
            {
                deleted += 1;
            }
        }

        let response = self
            .background_agent_state_processor()
            .worktree_list_inner(
                WorktreeListParams {
                    base_repo_path: Some(base_repo_path.display().to_string()),
                    include_deleted: Some(true),
                    cursor: None,
                    limit: Some(codex_state::MAX_MANAGED_WORKTREE_LIST_LIMIT),
                },
                policy.clone(),
            )
            .await?;
        Ok(Some(
            WorktreeReconcileResponse {
                data: response.data,
                policy,
                discovered,
                updated,
                deleted,
            }
            .into(),
        ))
    }

    pub(crate) async fn worktree_attach(
        &self,
        params: WorktreeAttachParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let policy = self.worktree_policy(/*current_base_repo_path*/ None);
        if !policy.enabled {
            return Err(invalid_params("managed worktrees are disabled in config"));
        }
        if params.thread_id.is_some() && policy.main_sessions == WorktreeSessionMode::Off {
            return Err(invalid_params(
                "main-session worktrees are disabled in config",
            ));
        }
        if params.agent_run_id.is_some() && policy.sub_sessions == WorktreeSessionMode::Off {
            return Err(invalid_params(
                "sub-session worktrees are disabled in config",
            ));
        }
        self.background_agent_state_processor()
            .worktree_attach_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn worktree_detach(
        &self,
        params: WorktreeDetachParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let policy = self.worktree_policy(/*current_base_repo_path*/ None);
        if !policy.enabled {
            return Err(invalid_params("managed worktrees are disabled in config"));
        }
        if params.thread_id.is_some() && policy.main_sessions == WorktreeSessionMode::Off {
            return Err(invalid_params(
                "main-session worktrees are disabled in config",
            ));
        }
        if params.agent_run_id.is_some() && policy.sub_sessions == WorktreeSessionMode::Off {
            return Err(invalid_params(
                "sub-session worktrees are disabled in config",
            ));
        }
        self.background_agent_state_processor()
            .worktree_detach_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn worktree_release(
        &self,
        params: WorktreeReleaseParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let policy = self.worktree_policy(/*current_base_repo_path*/ None);
        ensure_worktree_policy_enabled(&policy)?;
        let worktree = state_db
            .managed_worktrees()
            .get_managed_worktree(params.worktree_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read worktree: {err}")))?;
        let Some(worktree) = worktree else {
            return Ok(Some(WorktreeReleaseResponse { worktree: None }.into()));
        };
        if worktree.lifecycle_status == codex_state::ManagedWorktreeLifecycleStatus::Deleted
            || worktree.deleted_at.is_some()
        {
            let worktree = api_worktree_from_state(state_db.as_ref(), worktree).await?;
            return Ok(Some(
                WorktreeReleaseResponse {
                    worktree: Some(worktree),
                }
                .into(),
            ));
        }
        let status_snapshot = status_snapshot_for_release(&worktree, params.force_delete).await?;
        let force_delete = params.force_delete.unwrap_or(false);
        let cleanup_policy = params
            .cleanup_policy
            .map(state_worktree_cleanup_policy)
            .unwrap_or(worktree.cleanup_policy);
        if let Some(worktree) = release_background_agent_worktree_lease_if_present(
            &state_db,
            &worktree,
            cleanup_policy,
            force_delete,
            &status_snapshot,
        )
        .await?
        {
            let worktree = match worktree {
                Some(worktree) => Some(api_worktree_from_state(state_db.as_ref(), worktree).await?),
                None => None,
            };
            return Ok(Some(WorktreeReleaseResponse { worktree }.into()));
        }
        let released = state_db
            .managed_worktrees()
            .release_managed_worktree(codex_state::ManagedWorktreeReleaseParams {
                worktree_id: worktree.worktree_id,
                cleanup_policy,
                force_delete,
                status_snapshot_json: git_status_snapshot_json(status_snapshot.clone()),
                dirty: status_snapshot.dirty,
            })
            .await
            .map_err(|err| invalid_params(format!("failed to release worktree: {err}")))?;
        let worktree = match released {
            Some(worktree) => Some(api_worktree_from_state(state_db.as_ref(), worktree).await?),
            None => None,
        };
        Ok(Some(WorktreeReleaseResponse { worktree }.into()))
    }

    pub(crate) async fn worktree_cleanup(
        &self,
        params: WorktreeCleanupParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let policy = self.worktree_policy(/*current_base_repo_path*/ None);
        ensure_worktree_policy_enabled(&policy)?;
        let worktree = state_db
            .managed_worktrees()
            .get_managed_worktree(params.worktree_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read worktree: {err}")))?;
        let Some(worktree) = worktree else {
            return Ok(Some(WorktreeCleanupResponse { worktree: None }.into()));
        };
        if worktree.lifecycle_status == codex_state::ManagedWorktreeLifecycleStatus::Deleted
            || worktree.deleted_at.is_some()
        {
            let worktree = api_worktree_from_state(state_db.as_ref(), worktree).await?;
            return Ok(Some(
                WorktreeCleanupResponse {
                    worktree: Some(worktree),
                }
                .into(),
            ));
        }
        let force_delete = params.force_delete.unwrap_or(false);
        let status_snapshot = status_snapshot_for_release(&worktree, Some(force_delete)).await?;
        let cleanup_policy = if force_delete {
            codex_state::ManagedWorktreeCleanupPolicy::ForceDelete
        } else {
            codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean
        };
        let released = match release_background_agent_worktree_lease_if_present(
            &state_db,
            &worktree,
            cleanup_policy,
            force_delete,
            &status_snapshot,
        )
        .await?
        {
            Some(worktree) => worktree,
            None => state_db
                .managed_worktrees()
                .release_managed_worktree(codex_state::ManagedWorktreeReleaseParams {
                    worktree_id: worktree.worktree_id.clone(),
                    cleanup_policy,
                    force_delete,
                    status_snapshot_json: git_status_snapshot_json(status_snapshot.clone()),
                    dirty: status_snapshot.dirty,
                })
                .await
                .map_err(|err| {
                    invalid_params(format!("failed to queue worktree cleanup: {err}"))
                })?,
        };
        if let Some(candidate) = released
            && candidate.lifecycle_status
                == codex_state::ManagedWorktreeLifecycleStatus::CleanupPending
        {
            cleanup_managed_worktree_candidate(&state_db, candidate)
                .await
                .map_err(|err| internal_error(format!("failed to clean up worktree: {err}")))?;
        }
        let worktree = state_db
            .managed_worktrees()
            .get_managed_worktree(params.worktree_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read worktree: {err}")))?;
        let worktree = match worktree {
            Some(worktree) => Some(api_worktree_from_state(state_db.as_ref(), worktree).await?),
            None => None,
        };
        Ok(Some(WorktreeCleanupResponse { worktree }.into()))
    }

    pub(crate) async fn worktree_merge_candidate_list(
        &self,
        params: WorktreeMergeCandidateListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let status = params.status.map(state_worktree_merge_candidate_status);
        let candidates = state_db
            .managed_worktrees()
            .list_merge_candidates(
                params.worktree_id.as_str(),
                status,
                params
                    .limit
                    .unwrap_or(codex_state::DEFAULT_MANAGED_WORKTREE_LIST_LIMIT),
            )
            .await
            .map_err(|err| internal_error(format!("failed to list merge candidates: {err}")))?;
        Ok(Some(
            WorktreeMergeCandidateListResponse {
                data: candidates
                    .into_iter()
                    .map(api_worktree_merge_candidate_from_state)
                    .collect(),
            }
            .into(),
        ))
    }

    pub(crate) async fn worktree_merge_candidate_refresh(
        &self,
        params: WorktreeMergeCandidateRefreshParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let mut worktree = state_db
            .managed_worktrees()
            .get_managed_worktree(params.worktree_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read worktree: {err}")))?
            .ok_or_else(|| invalid_params("worktree/mergeCandidate/refresh worktree not found"))?;
        if worktree.lifecycle_status != codex_state::ManagedWorktreeLifecycleStatus::Active {
            return Err(invalid_params(
                "worktree/mergeCandidate/refresh requires an active worktree",
            ));
        }
        let status_snapshot = run_git_worktree_task("failed to inspect worktree", {
            let worktree_path = worktree.worktree_path.clone();
            move || get_git_worktree_status_snapshot(worktree_path.as_path())
        })
        .await?;
        if status_snapshot.dirty {
            return Err(invalid_params(
                "worktree/mergeCandidate/refresh requires a clean worktree",
            ));
        }
        if let Some(updated_worktree) = state_db
            .managed_worktrees()
            .update_managed_worktree_status(codex_state::ManagedWorktreeStatusUpdateParams {
                worktree_id: worktree.worktree_id.clone(),
                branch: status_snapshot
                    .branch
                    .clone()
                    .or_else(|| worktree.branch.clone()),
                head_sha: status_snapshot
                    .head_sha
                    .clone()
                    .or_else(|| worktree.head_sha.clone()),
                status_snapshot_json: git_status_snapshot_json(status_snapshot.clone()),
                dirty: status_snapshot.dirty,
            })
            .await
            .map_err(|err| internal_error(format!("failed to update managed worktree: {err}")))?
        {
            worktree = updated_worktree;
        }
        let target_ref = params
            .target_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD")
            .to_string();
        let target_sha = run_git_worktree_task("failed to resolve merge target", {
            let base_repo_path = worktree.base_repo_path.clone();
            let target_ref = target_ref.clone();
            move || resolve_git_ref(base_repo_path.as_path(), target_ref.as_str())
        })
        .await?
        .ok_or_else(|| {
            invalid_params(format!(
                "worktree/mergeCandidate/refresh targetRef `{target_ref}` does not resolve"
            ))
        })?;
        let head_sha = worktree
            .head_sha
            .clone()
            .ok_or_else(|| invalid_params("worktree has no head SHA to merge"))?;
        let dry_run = run_git_worktree_task("failed to dry-run worktree merge", {
            let base_repo_path = worktree.base_repo_path.clone();
            let target_ref = target_ref.clone();
            let head_sha = head_sha.clone();
            move || {
                merge_tree_dry_run(
                    base_repo_path.as_path(),
                    target_ref.as_str(),
                    head_sha.as_str(),
                )
            }
        })
        .await?;
        let candidate = state_db
            .managed_worktrees()
            .record_merge_candidate(codex_state::ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: None,
                worktree_id: worktree.worktree_id,
                target_ref,
                target_sha: Some(target_sha.clone()),
                base_sha: worktree.base_sha.unwrap_or(target_sha),
                head_sha,
                status: if dry_run.clean {
                    codex_state::ManagedWorktreeMergeCandidateStatus::Open
                } else {
                    codex_state::ManagedWorktreeMergeCandidateStatus::Blocked
                },
                conflict_summary: (!dry_run.conflicted_paths.is_empty())
                    .then(|| dry_run.conflicted_paths.join(", ")),
                test_summary_json: None,
            })
            .await
            .map_err(|err| internal_error(format!("failed to record merge candidate: {err}")))?;
        Ok(Some(
            WorktreeMergeCandidateRefreshResponse {
                candidate: api_worktree_merge_candidate_from_state(candidate),
            }
            .into(),
        ))
    }

    pub(crate) async fn worktree_merge_candidate_apply(
        &self,
        params: WorktreeMergeCandidateApplyParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let candidate = state_db
            .managed_worktrees()
            .get_merge_candidate(params.candidate_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read merge candidate: {err}")))?;
        let Some(candidate) = candidate else {
            return Ok(Some(
                WorktreeMergeCandidateApplyResponse { candidate: None }.into(),
            ));
        };
        if candidate.status != codex_state::ManagedWorktreeMergeCandidateStatus::Open {
            return Err(invalid_params(
                "worktree/mergeCandidate/apply requires an open candidate",
            ));
        }
        let worktree = state_db
            .managed_worktrees()
            .get_managed_worktree(candidate.worktree_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read worktree: {err}")))?
            .ok_or_else(|| invalid_params("merge candidate worktree not found"))?;
        if worktree.lifecycle_status != codex_state::ManagedWorktreeLifecycleStatus::Active {
            return Err(invalid_params(
                "worktree/mergeCandidate/apply requires an active worktree",
            ));
        }
        let worktree_status = run_git_worktree_task("failed to inspect merge source worktree", {
            let worktree_path = worktree.worktree_path.clone();
            move || get_git_worktree_status_snapshot(worktree_path.as_path())
        })
        .await?;
        if worktree_status.dirty {
            return Err(invalid_params(
                "worktree/mergeCandidate/apply requires a clean source worktree",
            ));
        }
        if worktree_status.head_sha.as_deref() != Some(candidate.head_sha.as_str()) {
            return Err(invalid_params(
                "worktree/mergeCandidate/apply source changed; refresh before applying",
            ));
        }
        let target_status = run_git_worktree_task("failed to inspect merge target checkout", {
            let base_repo_path = worktree.base_repo_path.clone();
            move || get_git_worktree_status_snapshot(base_repo_path.as_path())
        })
        .await?;
        if status_snapshot_has_merge_target_changes(&target_status) {
            return Err(invalid_params(
                "worktree/mergeCandidate/apply requires a clean target checkout",
            ));
        }
        if candidate.target_ref != "HEAD" {
            let target_branch = short_branch_name(candidate.target_ref.as_str());
            if target_status.branch.as_deref() != Some(target_branch.as_str()) {
                return Err(invalid_params(format!(
                    "worktree/mergeCandidate/apply requires the base repo checkout to be on targetRef `{}`",
                    candidate.target_ref
                )));
            }
        }
        if let Some(target_sha) = candidate.target_sha.as_deref() {
            let current_target_sha =
                run_git_worktree_task("failed to resolve current merge target", {
                    let base_repo_path = worktree.base_repo_path.clone();
                    move || resolve_git_ref(base_repo_path.as_path(), "HEAD")
                })
                .await?
                .ok_or_else(|| {
                    invalid_params("worktree/mergeCandidate/apply target HEAD does not resolve")
                })?;
            if current_target_sha != target_sha {
                return Err(invalid_params(format!(
                    "worktree/mergeCandidate/apply target changed from {target_sha} to {current_target_sha}; refresh before applying"
                )));
            }
        }
        run_git_worktree_task("failed to fast-forward merge candidate", {
            let base_repo_path = worktree.base_repo_path.clone();
            let head_sha = candidate.head_sha.clone();
            move || fast_forward_merge_ref(base_repo_path.as_path(), head_sha.as_str())
        })
        .await?;
        let candidate = state_db
            .managed_worktrees()
            .mark_merge_candidate_status(
                params.candidate_id.as_str(),
                codex_state::ManagedWorktreeMergeCandidateStatus::Applied,
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to mark merge candidate applied: {err}"))
            })?
            .map(api_worktree_merge_candidate_from_state);
        Ok(Some(
            WorktreeMergeCandidateApplyResponse { candidate }.into(),
        ))
    }

    pub(crate) async fn worktree_merge_candidate_dismiss(
        &self,
        params: WorktreeMergeCandidateDismissParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.worktree_state_db()?;
        let candidate = state_db
            .managed_worktrees()
            .mark_merge_candidate_status(
                params.candidate_id.as_str(),
                codex_state::ManagedWorktreeMergeCandidateStatus::Dismissed,
            )
            .await
            .map_err(|err| internal_error(format!("failed to dismiss merge candidate: {err}")))?
            .map(api_worktree_merge_candidate_from_state);
        Ok(Some(
            WorktreeMergeCandidateDismissResponse { candidate }.into(),
        ))
    }

    async fn resolve_worktree_base_repo_path(
        &self,
        requested_base_repo_path: Option<&str>,
    ) -> Result<Option<PathBuf>, JSONRPCErrorError> {
        let current_base_repo_path =
            resolve_root_git_project_for_trust(LOCAL_FS.as_ref(), &self.config.cwd)
                .await
                .map(AbsolutePathBuf::into_path_buf);
        let requested_base_repo_path = requested_base_repo_path
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let Some(requested_base_repo_path) = requested_base_repo_path else {
            return Ok(current_base_repo_path);
        };

        let requested_base_repo_path = PathBuf::from(requested_base_repo_path);
        if !requested_base_repo_path.is_absolute() {
            return Err(invalid_params("worktree baseRepoPath must be absolute"));
        }
        let requested_base_repo_path =
            AbsolutePathBuf::from_absolute_path_checked(requested_base_repo_path)
                .map_err(|err| invalid_params(format!("invalid worktree baseRepoPath: {err}")))?;
        let resolved_base_repo_path =
            resolve_root_git_project_for_trust(LOCAL_FS.as_ref(), &requested_base_repo_path)
                .await
                .unwrap_or(requested_base_repo_path);
        Ok(Some(resolved_base_repo_path.into_path_buf()))
    }

    fn worktree_policy(&self, current_base_repo_path: Option<&Path>) -> WorktreePolicy {
        let config = &self.config.worktrees;
        WorktreePolicy {
            enabled: config.enabled,
            root: config.root.clone(),
            cleanup_default: api_worktree_cleanup_policy_from_config(config.cleanup_default),
            main_sessions: api_worktree_session_mode_from_config(config.main_sessions),
            sub_sessions: api_worktree_session_mode_from_config(config.sub_sessions),
            current_base_repo_path: current_base_repo_path
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    fn worktree_root_path(&self, base_repo_path: &Path) -> Result<PathBuf, JSONRPCErrorError> {
        let Some(root) = self
            .config
            .worktrees
            .root
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(base_repo_path.join(".codewith").join("worktrees"));
        };
        let root = PathBuf::from(root);
        if root.is_absolute() {
            Ok(root)
        } else {
            Ok(base_repo_path.join(root))
        }
    }

    fn worktree_state_db(&self) -> Result<StateDbHandle, JSONRPCErrorError> {
        self.state_db
            .clone()
            .ok_or_else(|| internal_error("managed worktree state store is unavailable"))
    }

    async fn cancel_background_agent_worker(&self, run_id: &str) {
        let token = self
            .background_agent_workers
            .lock()
            .await
            .get(run_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
        }
        let process_handle = self
            .background_agent_worker_processes
            .lock()
            .await
            .remove(run_id);
        if let Some(handle) = process_handle {
            let controller = WorkerProcessController::default();
            if let Err(err) = controller.stop(&handle).await {
                warn!(
                    run_id,
                    "failed to stop background-agent worker process: {err}"
                );
            }
        }
    }

    fn background_agent_state_processor(&self) -> BackgroundAgentRequestProcessor {
        BackgroundAgentRequestProcessor::new(self.state_db.clone())
    }

    fn freeze_start_execution_context(&self, params: &mut AgentStartParams) {
        let context = params.execution_context.get_or_insert_with(|| {
            Box::new(AgentExecutionContextParams {
                workspace_roots: None,
                approval_policy: None,
                permission_profile: None,
                sandbox_policy: None,
                network_policy: None,
                model: None,
                provider: None,
                service_tier: None,
                mcp_tool_allowlist: None,
                env_snapshot_policy: Some("inherit-minimal".to_string()),
                shell_snapshot: None,
                config_source_hashes: None,
                max_runtime_seconds: None,
                max_tokens: None,
                recovery_policy: Some("abort_mid_turn_resume_at_safe_boundary".to_string()),
            })
        });
        params.cwd = Some(self.config.cwd.display().to_string());
        context.workspace_roots = Some(
            self.config
                .workspace_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect(),
        );
        context.approval_policy = Some(self.config.permissions.approval_policy.value().into());
        context.permission_profile =
            serde_json::to_value(self.config.permissions.effective_permission_profile()).ok();
        context.model = self.config.model.clone();
        context.provider = Some(self.config.model_provider_id.clone());
        context.service_tier = self.config.service_tier.clone();
        params.auth_profile_ref = self.config.selected_auth_profile.clone();
    }

    fn spawn_background_agent_reconcile(&self, only_run_id: Option<String>) {
        let Some(context) = self.background_agent_worker_context() else {
            return;
        };
        self.background_tasks.spawn(async move {
            if let Err(err) = reconcile_background_agents(context, only_run_id).await {
                warn!("background agent reconcile failed: {err}");
            }
        });
    }

    fn background_agent_worker_context(&self) -> Option<BackgroundAgentWorkerContext> {
        let state_db = self.state_db.clone()?;
        let context = BackgroundAgentWorkerContext {
            state_db,
            supervisor_id: self.background_agent_supervisor_id.clone(),
            thread_manager: Arc::clone(&self.thread_manager),
            auth_manager: Arc::clone(&self.auth_manager),
            config_manager: self.config_manager.clone(),
            arg0_paths: self.arg0_paths.clone(),
            active_workers: Arc::clone(&self.background_agent_workers),
            active_worker_processes: Arc::clone(&self.background_agent_worker_processes),
            task_tracker: self.background_tasks.clone(),
            codex_home: self.config.codex_home.to_path_buf(),
            codex_bin: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codewith")),
            worker_process: self.background_agent_worker_process,
        };
        Some(context)
    }
}

#[derive(Clone)]
struct BackgroundAgentWorkerContext {
    state_db: StateDbHandle,
    supervisor_id: String,
    thread_manager: Arc<codex_core::ThreadManager>,
    auth_manager: Arc<codex_login::AuthManager>,
    config_manager: crate::config_manager::ConfigManager,
    arg0_paths: codex_arg0::Arg0DispatchPaths,
    active_workers: Arc<Mutex<HashMap<String, CancellationToken>>>,
    active_worker_processes: Arc<Mutex<HashMap<String, WorkerProcessHandle>>>,
    task_tracker: TaskTracker,
    codex_home: PathBuf,
    codex_bin: PathBuf,
    worker_process: bool,
}

#[derive(Clone)]
struct BackgroundAgentProcessSupervisorContext {
    state_db: StateDbHandle,
    supervisor_id: String,
    active_worker_processes: Arc<Mutex<HashMap<String, WorkerProcessHandle>>>,
    codex_home: PathBuf,
    codex_bin: PathBuf,
}

impl BackgroundAgentWorkerContext {
    fn process_supervisor_context(&self) -> BackgroundAgentProcessSupervisorContext {
        BackgroundAgentProcessSupervisorContext {
            state_db: Arc::clone(&self.state_db),
            supervisor_id: self.supervisor_id.clone(),
            active_worker_processes: Arc::clone(&self.active_worker_processes),
            codex_home: self.codex_home.clone(),
            codex_bin: self.codex_bin.clone(),
        }
    }
}

async fn reconcile_background_agents(
    context: BackgroundAgentWorkerContext,
    only_run_id: Option<String>,
) -> anyhow::Result<()> {
    context
        .state_db
        .orphan_stale_background_agent_runs(BACKGROUND_AGENT_HEARTBEAT_TIMEOUT)
        .await?;
    reconcile_managed_worktree_cleanup(&context.state_db).await?;
    let runs = match only_run_id {
        Some(run_id) => context
            .state_db
            .get_run(run_id.as_str())
            .await?
            .into_iter()
            .collect::<Vec<_>>(),
        None => context.state_db.list_runs(Some(200)).await?,
    };
    for run in runs {
        let active_handle = context
            .active_worker_processes
            .lock()
            .await
            .get(run.id.as_str())
            .cloned();
        if let Some(handle) = active_handle
            && (run.desired_state != BackgroundAgentDesiredState::Running
                || run.status == BackgroundAgentRunStatus::Orphaned
                || is_terminal_background_agent_status(run.status))
        {
            let controller = WorkerProcessController::default();
            if let Err(err) = controller.stop(&handle).await {
                warn!(
                    run_id = %run.id,
                    "failed to stop inactive background-agent worker process: {err}"
                );
            }
            context
                .active_worker_processes
                .lock()
                .await
                .remove(run.id.as_str());
            continue;
        }
        if !should_start_background_run(&run) {
            continue;
        }
        let token = CancellationToken::new();
        {
            let mut active = context.active_workers.lock().await;
            if active.contains_key(run.id.as_str()) {
                continue;
            }
            active.insert(run.id.clone(), token.clone());
        }
        let worker_context = context.clone();
        context.task_tracker.spawn(async move {
            let run_id = run.id.clone();
            if let Err(err) = run_background_agent_worker(worker_context.clone(), run, token).await
            {
                error!(run_id, "background agent worker failed: {err}");
                if let Err(mark_err) =
                    mark_background_agent_worker_failed(&worker_context, run_id.as_str(), &err)
                        .await
                {
                    warn!(
                        run_id,
                        "failed to mark background agent worker failure: {mark_err}"
                    );
                }
            }
            worker_context
                .active_workers
                .lock()
                .await
                .remove(run_id.as_str());
        });
    }
    Ok(())
}

async fn reconcile_background_agent_worker_processes(
    context: BackgroundAgentProcessSupervisorContext,
    only_run_id: Option<String>,
) -> anyhow::Result<()> {
    context
        .state_db
        .orphan_stale_background_agent_runs(BACKGROUND_AGENT_HEARTBEAT_TIMEOUT)
        .await?;
    reconcile_managed_worktree_cleanup(&context.state_db).await?;
    rehydrate_background_agent_worker_processes(&context).await?;
    prune_finished_background_agent_worker_processes(&context).await;
    let runs = match only_run_id {
        Some(run_id) => context
            .state_db
            .get_run(run_id.as_str())
            .await?
            .into_iter()
            .collect::<Vec<_>>(),
        None => context.state_db.list_runs(Some(200)).await?,
    };
    for run in runs {
        let active_handle = context
            .active_worker_processes
            .lock()
            .await
            .get(run.id.as_str())
            .cloned();
        if let Some(handle) = active_handle
            && (run.desired_state != BackgroundAgentDesiredState::Running
                || run.status == BackgroundAgentRunStatus::Orphaned
                || is_terminal_background_agent_status(run.status))
        {
            let controller = WorkerProcessController::default();
            if let Err(err) = controller.stop(&handle).await {
                warn!(
                    run_id = %run.id,
                    "failed to stop inactive background-agent worker process: {err}"
                );
            }
            context
                .active_worker_processes
                .lock()
                .await
                .remove(run.id.as_str());
            if run.desired_state != BackgroundAgentDesiredState::Running {
                continue;
            }
        }
        if !should_start_background_run(&run) {
            continue;
        }
        if context
            .active_worker_processes
            .lock()
            .await
            .contains_key(run.id.as_str())
        {
            continue;
        }
        let stderr_log_path = background_agent_worker_stderr_log_path(&context, run.id.as_str());
        let command = WorkerProcessCommand::new(&context.codex_bin, &stderr_log_path)
            .arg(OsString::from("app-server"))
            .arg(OsString::from("--listen"))
            .arg(OsString::from("off"))
            .arg(OsString::from("--background-agent-worker"))
            .arg(OsString::from(run.id.clone()));
        let handle = WorkerProcessController::default().spawn(command).await?;
        context
            .active_worker_processes
            .lock()
            .await
            .insert(run.id.clone(), handle.clone());
        context
            .state_db
            .append_background_agent_event(
                run.id.as_str(),
                "agent.workerProcessSpawned",
                &json!({
                    "supervisorId": context.supervisor_id,
                    "pid": handle.pid,
                    "pgid": handle.pgid,
                    "stderrLogPath": stderr_log_path.display().to_string(),
                }),
            )
            .await?;
    }
    Ok(())
}

async fn reconcile_managed_worktree_cleanup(state_db: &StateDbHandle) -> anyhow::Result<()> {
    let candidates = state_db
        .managed_worktrees()
        .list_cleanup_candidates(Utc::now(), MANAGED_WORKTREE_CLEANUP_BATCH_LIMIT)
        .await?;
    for worktree in candidates {
        if let Err(err) = cleanup_managed_worktree_candidate(state_db, worktree).await {
            warn!("managed worktree cleanup reconcile failed: {err}");
        }
    }
    Ok(())
}

async fn cleanup_managed_worktree_candidate(
    state_db: &StateDbHandle,
    worktree: codex_state::ManagedWorktree,
) -> anyhow::Result<()> {
    let base_repo_path = worktree.base_repo_path.clone();
    let worktree_path = worktree.worktree_path.clone();
    let status_result = tokio::task::spawn_blocking({
        let worktree_path = worktree_path.clone();
        move || get_git_worktree_status_snapshot(&worktree_path)
    })
    .await?;
    let status_snapshot = match status_result {
        Ok(status_snapshot) => status_snapshot,
        Err(err) => {
            if linked_worktree_is_absent(base_repo_path.clone(), worktree_path.clone()).await? {
                state_db
                    .mark_managed_worktree_cleanup_succeeded(worktree.worktree_id.as_str())
                    .await?;
            } else {
                record_managed_worktree_cleanup_failure(
                    state_db,
                    worktree.worktree_id.as_str(),
                    format!("git worktree status failed: {err}"),
                    GitWorktreeStatusSnapshot {
                        dirty: true,
                        branch: None,
                        head_sha: None,
                        records: vec![format!("status probe failed: {err}")],
                    },
                    /*force_delete_required*/ false,
                )
                .await?;
            }
            return Ok(());
        }
    };
    let force_delete = worktree.force_delete_requested
        || worktree.cleanup_policy == codex_state::ManagedWorktreeCleanupPolicy::ForceDelete;
    if status_snapshot.dirty && !force_delete {
        record_managed_worktree_cleanup_failure(
            state_db,
            worktree.worktree_id.as_str(),
            "dirty worktree retained",
            status_snapshot,
            /*force_delete_required*/ true,
        )
        .await?;
        return Ok(());
    }
    let has_new_commits_result = match worktree.base_sha.clone() {
        Some(base_sha) => {
            let worktree_path = worktree_path.clone();
            tokio::task::spawn_blocking(move || {
                worktree_has_commits_after(&worktree_path, base_sha.as_str())
            })
            .await?
        }
        None => Ok(true),
    };
    match has_new_commits_result {
        Ok(true) if !force_delete => {
            record_managed_worktree_cleanup_failure(
                state_db,
                worktree.worktree_id.as_str(),
                "worktree has commits after its managed base",
                status_snapshot,
                /*force_delete_required*/ true,
            )
            .await?;
            return Ok(());
        }
        Ok(_) => {}
        Err(err) if !force_delete => {
            record_managed_worktree_cleanup_failure(
                state_db,
                worktree.worktree_id.as_str(),
                format!("git worktree commit-safety check failed: {err}"),
                status_snapshot,
                /*force_delete_required*/ false,
            )
            .await?;
            return Ok(());
        }
        Err(err) => {
            warn!(
                worktree_id = worktree.worktree_id.as_str(),
                "force cleanup continuing after commit-safety check failed: {err}"
            );
        }
    }

    let remove_result = tokio::task::spawn_blocking({
        let base_repo_path = base_repo_path.clone();
        let worktree_path = worktree_path.clone();
        move || remove_linked_git_worktree(&base_repo_path, &worktree_path, force_delete)
    })
    .await?;
    match remove_result {
        Ok(()) => {
            state_db
                .mark_managed_worktree_cleanup_succeeded(worktree.worktree_id.as_str())
                .await?;
        }
        Err(err) => {
            record_managed_worktree_cleanup_failure(
                state_db,
                worktree.worktree_id.as_str(),
                format!("git worktree remove failed: {err}"),
                status_snapshot,
                !force_delete,
            )
            .await?;
        }
    }
    Ok(())
}

async fn linked_worktree_is_absent(
    base_repo_path: PathBuf,
    worktree_path: PathBuf,
) -> anyhow::Result<bool> {
    let entries =
        tokio::task::spawn_blocking(move || list_git_worktrees(&base_repo_path)).await??;
    Ok(!entries
        .iter()
        .any(|entry| paths_match(entry.path.as_path(), worktree_path.as_path())))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn ensure_worktree_policy_enabled(policy: &WorktreePolicy) -> Result<(), JSONRPCErrorError> {
    if policy.enabled {
        Ok(())
    } else {
        Err(invalid_params("managed worktrees are disabled in config"))
    }
}

fn state_worktree_cleanup_policy(
    policy: WorktreeCleanupPolicy,
) -> codex_state::ManagedWorktreeCleanupPolicy {
    match policy {
        WorktreeCleanupPolicy::Retain => codex_state::ManagedWorktreeCleanupPolicy::Retain,
        WorktreeCleanupPolicy::DeleteIfClean => {
            codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean
        }
        WorktreeCleanupPolicy::ForceDelete => {
            codex_state::ManagedWorktreeCleanupPolicy::ForceDelete
        }
    }
}

fn background_agent_workspace_cleanup(
    policy: codex_state::ManagedWorktreeCleanupPolicy,
    force_delete: bool,
) -> BackgroundAgentWorkspaceCleanup {
    if force_delete {
        return BackgroundAgentWorkspaceCleanup::ForceDelete;
    }
    match policy {
        codex_state::ManagedWorktreeCleanupPolicy::Retain => {
            BackgroundAgentWorkspaceCleanup::Retain
        }
        codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean => {
            BackgroundAgentWorkspaceCleanup::DeleteIfClean
        }
        codex_state::ManagedWorktreeCleanupPolicy::ForceDelete => {
            BackgroundAgentWorkspaceCleanup::ForceDelete
        }
    }
}

async fn release_background_agent_worktree_lease_if_present(
    state_db: &StateDbHandle,
    worktree: &codex_state::ManagedWorktree,
    cleanup_policy: codex_state::ManagedWorktreeCleanupPolicy,
    force_delete: bool,
    status_snapshot: &GitWorktreeStatusSnapshot,
) -> Result<Option<Option<codex_state::ManagedWorktree>>, JSONRPCErrorError> {
    let worktree_id = worktree.worktree_id.clone();
    let background_agent_lease = state_db
        .get_background_agent_worktree_lease(worktree_id.as_str())
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to read background agent worktree lease: {err}"
            ))
        })?;
    let Some(lease) = background_agent_lease.as_ref() else {
        return Ok(None);
    };
    if lease.deleted_at.is_some() {
        return Ok(None);
    }
    let run = state_db
        .get_run(lease.run_id.as_str())
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to read background agent run for worktree lease: {err}"
            ))
        })?;
    if let Some(run) = run
        && run.desired_state == BackgroundAgentDesiredState::Running
        && run.status != BackgroundAgentRunStatus::Orphaned
        && !is_terminal_background_agent_status(run.status)
    {
        return Err(invalid_params(format!(
            "worktree is owned by active background agent run {}; stop the agent before release or cleanup",
            run.id
        )));
    }

    let status_snapshot_json = git_status_snapshot_json(status_snapshot.clone());
    state_db
        .update_background_agent_worktree_lease_status(
            worktree_id.as_str(),
            status_snapshot.dirty,
            &status_snapshot_json,
        )
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to update background agent worktree lease: {err}"
            ))
        })?;
    state_db
        .release_background_agent_worktree_lease(
            worktree_id.as_str(),
            background_agent_workspace_cleanup(cleanup_policy, force_delete),
        )
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to release background agent worktree lease: {err}"
            ))
        })?;
    let worktree = state_db
        .managed_worktrees()
        .get_managed_worktree(worktree_id.as_str())
        .await
        .map_err(|err| internal_error(format!("failed to read released worktree: {err}")))?;
    Ok(Some(worktree))
}

fn state_worktree_merge_candidate_status(
    status: WorktreeMergeCandidateStatus,
) -> codex_state::ManagedWorktreeMergeCandidateStatus {
    match status {
        WorktreeMergeCandidateStatus::Open => {
            codex_state::ManagedWorktreeMergeCandidateStatus::Open
        }
        WorktreeMergeCandidateStatus::Blocked => {
            codex_state::ManagedWorktreeMergeCandidateStatus::Blocked
        }
        WorktreeMergeCandidateStatus::Applied => {
            codex_state::ManagedWorktreeMergeCandidateStatus::Applied
        }
        WorktreeMergeCandidateStatus::Dismissed => {
            codex_state::ManagedWorktreeMergeCandidateStatus::Dismissed
        }
    }
}

fn worktree_branch_name(
    requested_branch: Option<&str>,
    requested_name: Option<&str>,
    worktree_id: &str,
) -> Result<String, JSONRPCErrorError> {
    if let Some(branch) = requested_branch
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
    {
        if branch.chars().any(char::is_whitespace) {
            return Err(invalid_params(
                "worktree/create branch must not contain whitespace",
            ));
        }
        return Ok(branch.to_string());
    }
    Ok(format!(
        "codewith/{}-{}",
        sanitize_worktree_component(requested_name.unwrap_or("worktree")),
        short_worktree_id(worktree_id)
    ))
}

fn worktree_path_name(requested_name: Option<&str>, worktree_id: &str) -> String {
    format!(
        "{}-{}",
        sanitize_worktree_component(requested_name.unwrap_or("worktree")),
        short_worktree_id(worktree_id)
    )
}

fn sanitize_worktree_component(value: &str) -> String {
    let mut component = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while component.contains("--") {
        component = component.replace("--", "-");
    }
    let component = component.trim_matches(['-', '.', '_']).to_string();
    if component.is_empty() {
        "worktree".to_string()
    } else {
        component
    }
}

fn short_worktree_id(worktree_id: &str) -> String {
    worktree_id.chars().take(8).collect()
}

fn short_branch_name(branch: &str) -> String {
    branch
        .strip_prefix("refs/heads/")
        .unwrap_or(branch)
        .to_string()
}

fn path_is_inside(path: &Path, root: &Path) -> bool {
    if let (Ok(path), Ok(root)) = (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        return path.starts_with(root);
    }
    path.starts_with(root)
}

async fn run_git_worktree_task<T, F>(label: &'static str, task: F) -> Result<T, JSONRPCErrorError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, codex_git_utils::GitToolingError> + Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|err| internal_error(format!("{label}: {err}")))?
        .map_err(|err| invalid_params(format!("{label}: {err}")))
}

async fn load_all_managed_worktrees(
    state_db: &codex_state::StateRuntime,
    base_repo_path: &Path,
) -> Result<Vec<codex_state::ManagedWorktree>, JSONRPCErrorError> {
    let mut cursor = None;
    let mut worktrees = Vec::new();
    loop {
        let page = state_db
            .managed_worktrees()
            .list_managed_worktrees_page(
                Some(base_repo_path),
                /*include_deleted*/ false,
                cursor.as_deref(),
                codex_state::MAX_MANAGED_WORKTREE_LIST_LIMIT,
            )
            .await
            .map_err(|err| internal_error(format!("failed to list managed worktrees: {err}")))?;
        worktrees.extend(page.data);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(worktrees);
        };
        cursor = Some(next_cursor);
    }
}

async fn status_snapshot_for_release(
    worktree: &codex_state::ManagedWorktree,
    force_delete: Option<bool>,
) -> Result<GitWorktreeStatusSnapshot, JSONRPCErrorError> {
    match run_git_worktree_task("failed to inspect worktree", {
        let worktree_path = worktree.worktree_path.clone();
        move || get_git_worktree_status_snapshot(worktree_path.as_path())
    })
    .await
    {
        Ok(status_snapshot) => Ok(status_snapshot),
        Err(err) if force_delete.unwrap_or(false) => Ok(GitWorktreeStatusSnapshot {
            dirty: true,
            branch: worktree.branch.clone(),
            head_sha: worktree.head_sha.clone(),
            records: vec![format!("status probe failed before force cleanup: {err:?}")],
        }),
        Err(err) => Err(err),
    }
}

fn status_snapshot_has_merge_target_changes(status_snapshot: &GitWorktreeStatusSnapshot) -> bool {
    status_snapshot.records.iter().any(|record| {
        if record.starts_with("# ") {
            return false;
        }
        if let Some(path) = record.strip_prefix("? ") {
            return !path.starts_with(".codewith/worktrees/");
        }
        !record.trim().is_empty()
    })
}

async fn record_managed_worktree_cleanup_failure(
    state_db: &StateDbHandle,
    worktree_id: &str,
    reason: impl Into<String>,
    status_snapshot: GitWorktreeStatusSnapshot,
    force_delete_required: bool,
) -> anyhow::Result<()> {
    state_db
        .record_managed_worktree_cleanup_failure(codex_state::ManagedWorktreeCleanupFailureParams {
            worktree_id: worktree_id.to_string(),
            reason: reason.into(),
            dirty: status_snapshot.dirty,
            status_snapshot_json: git_status_snapshot_json(status_snapshot),
            retry_after: Some(Utc::now() + MANAGED_WORKTREE_CLEANUP_RETRY_DELAY),
            force_delete_required,
        })
        .await?;
    Ok(())
}

fn git_status_snapshot_json(status_snapshot: GitWorktreeStatusSnapshot) -> Value {
    json!({
        "branch": status_snapshot.branch,
        "headSha": status_snapshot.head_sha,
        "dirty": status_snapshot.dirty,
        "records": status_snapshot.records,
    })
}

async fn rehydrate_background_agent_worker_processes(
    context: &BackgroundAgentProcessSupervisorContext,
) -> anyhow::Result<()> {
    let records = context
        .state_db
        .list_background_agent_active_process_handles()
        .await?;
    if records.is_empty() {
        return Ok(());
    }

    let active_run_ids = context
        .active_worker_processes
        .lock()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let controller = WorkerProcessController::default();
    let mut recovered = Vec::new();
    for record in records {
        if active_run_ids.iter().any(|run_id| run_id == &record.run_id) {
            continue;
        }
        let handle = WorkerProcessHandle {
            pid: record.pid,
            pgid: record.pgid,
            start_token: Some(record.start_token),
            stderr_log_path: record.stderr_log_path,
        };
        match controller.status(&handle).await {
            Ok(WorkerProcessStatus::Running) => recovered.push((record.run_id, handle)),
            Ok(WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord) => {}
            Err(err) => {
                warn!(
                    run_id = record.run_id,
                    generation = record.generation,
                    "failed to inspect persisted background-agent worker process: {err}"
                );
            }
        }
    }
    if recovered.is_empty() {
        return Ok(());
    }

    let mut active = context.active_worker_processes.lock().await;
    for (run_id, handle) in recovered {
        active.entry(run_id).or_insert(handle);
    }
    Ok(())
}

async fn prune_finished_background_agent_worker_processes(
    context: &BackgroundAgentProcessSupervisorContext,
) {
    let handles = context
        .active_worker_processes
        .lock()
        .await
        .iter()
        .map(|(run_id, handle)| (run_id.clone(), handle.clone()))
        .collect::<Vec<_>>();
    if handles.is_empty() {
        return;
    }
    let controller = WorkerProcessController::default();
    let mut finished = Vec::new();
    for (run_id, handle) in handles {
        match controller.status(&handle).await {
            Ok(WorkerProcessStatus::Running) => {}
            Ok(WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord) => {
                finished.push(run_id);
            }
            Err(err) => {
                warn!(
                    run_id,
                    "failed to inspect background-agent worker process: {err}"
                );
            }
        }
    }
    if !finished.is_empty() {
        let mut active = context.active_worker_processes.lock().await;
        for run_id in finished {
            active.remove(run_id.as_str());
        }
    }
}

fn background_agent_worker_stderr_log_path(
    context: &BackgroundAgentProcessSupervisorContext,
    run_id: &str,
) -> PathBuf {
    background_agent_worker_stderr_log_path_for_home(context.codex_home.as_path(), run_id)
}

fn background_agent_worker_stderr_log_path_for_home(codex_home: &Path, run_id: &str) -> PathBuf {
    background_agent_daemon_state_dir(codex_home)
        .join(BACKGROUND_AGENT_WORKER_STDERR_DIR)
        .join(format!(
            "{}.stderr.log",
            encode_background_agent_file_component(run_id)
        ))
}

fn encode_background_agent_file_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(unix)]
async fn worker_process_start_token(worker_process: bool) -> anyhow::Result<Option<String>> {
    if worker_process {
        codex_background_agent::process_lifecycle::current_process_start_token()
            .await
            .map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(not(unix))]
async fn worker_process_start_token(_worker_process: bool) -> anyhow::Result<Option<String>> {
    Ok(None)
}

fn should_start_background_run(run: &BackgroundAgentRun) -> bool {
    if run.desired_state != BackgroundAgentDesiredState::Running
        || !matches!(
            run.status,
            BackgroundAgentRunStatus::Queued | BackgroundAgentRunStatus::Orphaned
        )
    {
        return false;
    }
    if let Some(wait_until) =
        background_agent_usage_profile_wait_until(run.status_reason.as_deref())
    {
        return wait_until <= Utc::now().timestamp();
    }
    true
}

fn is_terminal_background_agent_status(status: BackgroundAgentRunStatus) -> bool {
    matches!(
        status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    )
}

async fn run_background_agent_worker(
    context: BackgroundAgentWorkerContext,
    run: BackgroundAgentRun,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    let process_lease_id = format!(
        "{}:{}:{}",
        context.supervisor_id,
        run.id,
        run.generation.saturating_add(1)
    );
    let Some(generation) = context
        .state_db
        .claim_background_agent_supervisor(
            run.id.as_str(),
            context.supervisor_id.as_str(),
            process_lease_id.as_str(),
        )
        .await?
    else {
        debug!(run_id = %run.id, "background agent run was not claimable");
        return Ok(());
    };

    let config = match resolve_background_agent_config(&context, &run).await? {
        BackgroundAgentConfigResolution::Ready(config) => *config,
        BackgroundAgentConfigResolution::UsageProfileWait { retry_at } => {
            defer_background_agent_for_usage_profile_wait(
                &context,
                run.id.as_str(),
                generation,
                retry_at,
            )
            .await?;
            return Ok(());
        }
    };

    let pid_value = i64::from(std::process::id());
    let pid = Some(pid_value);
    let pgid = context.worker_process.then_some(pid_value);
    let job_id = if context.worker_process {
        Some(format!("worker-process:{}", run.id))
    } else {
        Some("in-process".to_string())
    };
    let process_start_token = worker_process_start_token(context.worker_process).await?;
    let stderr_log_path = context.worker_process.then(|| {
        background_agent_worker_stderr_log_path_for_home(
            context.codex_home.as_path(),
            run.id.as_str(),
        )
    });
    let stderr_log_path_string = stderr_log_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string());
    if !context
        .state_db
        .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
            run_id: run.id.as_str(),
            supervisor_id: context.supervisor_id.as_str(),
            generation,
            pid,
            pgid,
            job_id: job_id.as_deref(),
            start_token: process_start_token.as_deref(),
            stderr_log_path: stderr_log_path_string.as_deref(),
        })
        .await?
    {
        return Err(background_agent_ownership_lost(run.id.as_str(), generation));
    }
    append_status(
        &context,
        run.id.as_str(),
        generation,
        BackgroundAgentRunStatus::Starting,
        "starting background thread",
        "agent.workerStarting",
        json!({
            "supervisorId": context.supervisor_id,
            "generation": generation,
            "pid": pid,
            "pgid": pgid,
            "jobId": job_id,
            "workerProcess": context.worker_process,
        }),
    )
    .await?;

    let should_submit_prompt = run.thread_id.is_none() && run.rollout_path.is_none();
    let prompt = if should_submit_prompt {
        Some(load_background_agent_prompt(&context.state_db, run.id.as_str()).await?)
    } else {
        None
    };
    let NewThread {
        thread_id,
        thread,
        session_configured,
        ..
    } = with_startup_heartbeat(
        &context,
        run.id.as_str(),
        generation,
        "start background thread",
        retry_transient_sqlite_busy("start background thread", || {
            start_or_resume_background_thread(&context, &run, config.clone())
        }),
    )
    .await?;
    let rollout_path = session_configured
        .rollout_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string());
    let thread_id_string = thread_id.to_string();
    let session_id_string = session_configured.session_id.to_string();
    let binding_params = BackgroundAgentThreadBindingParams {
        run_id: run.id.clone(),
        supervisor_id: context.supervisor_id.clone(),
        generation,
        thread_id: thread_id_string.clone(),
        thread_store_kind: BACKGROUND_AGENT_THREAD_STORE_KIND.to_string(),
        thread_store_id: Some(session_id_string.clone()),
        rollout_path: rollout_path.clone(),
    };
    let bound = retry_transient_sqlite_busy("bind background agent thread", || {
        context
            .state_db
            .bind_background_agent_thread(&binding_params)
    })
    .await?;
    if !bound {
        return Err(background_agent_ownership_lost(run.id.as_str(), generation));
    }
    ensure_background_agent_worker_current(&context, run.id.as_str(), generation, false).await?;
    retry_transient_sqlite_busy("create background agent execution snapshot", || {
        context
            .state_db
            .create_execution_snapshot(BackgroundAgentExecutionSnapshotParams {
                run_id: run.id.clone(),
                snapshot_kind: "worker_thread_bound".to_string(),
                payload_json: json!({
                    "threadId": thread_id.to_string(),
                    "sessionId": session_id_string,
                    "rolloutPath": rollout_path,
                }),
                recovery_policy: "resume_or_orphan".to_string(),
                config_fingerprint: run.config_fingerprint.clone(),
            })
    })
    .await?;
    append_status(
        &context,
        run.id.as_str(),
        generation,
        BackgroundAgentRunStatus::Running,
        "background thread running",
        "agent.workerRunning",
        json!({"threadId": thread_id.to_string()}),
    )
    .await?;

    if should_submit_prompt {
        let prompt =
            prompt.ok_or_else(|| anyhow::anyhow!("background agent prompt missing for new run"))?;
        with_startup_heartbeat(
            &context,
            run.id.as_str(),
            generation,
            "submit initial prompt",
            async {
                thread
                    .submit(Op::UserInput {
                        items: vec![UserInput::Text {
                            text: prompt,
                            text_elements: Vec::new(),
                        }],
                        final_output_json_schema: None,
                        responsesapi_client_metadata: None,
                        additional_context: Default::default(),
                        environments: None,
                        thread_settings: Default::default(),
                    })
                    .await?;
                Ok(())
            },
        )
        .await?;
    }

    let mut heartbeat = tokio::time::interval(BACKGROUND_AGENT_HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                stop_background_thread(&context, &thread, run.id.as_str(), generation).await?;
                return Ok(());
            }
            _ = heartbeat.tick() => {
                if !heartbeat_and_continue(&context, run.id.as_str(), generation, &thread).await? {
                    return Ok(());
                }
            }
            event = thread.next_event() => {
                let event = event?;
                if handle_background_agent_event(
                    &context,
                    run.id.as_str(),
                    generation,
                    thread.clone(),
                    event.msg,
                    &cancel_token,
                ).await? {
                    return Ok(());
                }
            }
        }
    }
}

async fn start_or_resume_background_thread(
    context: &BackgroundAgentWorkerContext,
    run: &BackgroundAgentRun,
    config: codex_core::config::Config,
) -> anyhow::Result<NewThread> {
    if let Some(rollout_path) = run.rollout_path.as_ref() {
        return context
            .thread_manager
            .resume_thread_from_rollout(
                config,
                PathBuf::from(rollout_path),
                Arc::clone(&context.auth_manager),
                /*parent_trace*/ None,
            )
            .await
            .map_err(anyhow::Error::from);
    }
    let environments = context
        .thread_manager
        .default_environment_selections(&config.cwd);
    context
        .thread_manager
        .start_thread_with_options(StartThreadOptions {
            config,
            initial_history: InitialHistory::New,
            session_source: Some(SessionSource::Custom("background_agent".to_string())),
            thread_source: Some(ThreadSource::Subagent),
            dynamic_tools: Vec::new(),
            metrics_service_name: Some("background-agent".to_string()),
            parent_trace: None,
            environments,
        })
        .await
        .map_err(anyhow::Error::from)
}

enum BackgroundAgentConfigResolution {
    Ready(Box<codex_core::config::Config>),
    UsageProfileWait { retry_at: DateTime<Utc> },
}

async fn resolve_background_agent_config(
    context: &BackgroundAgentWorkerContext,
    run: &BackgroundAgentRun,
) -> anyhow::Result<BackgroundAgentConfigResolution> {
    let snapshot = context
        .state_db
        .get_latest_execution_snapshot(run.id.as_str())
        .await?;
    let payload = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.payload_json.as_object());
    let cwd = payload
        .and_then(|payload| payload.get("cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let workspace_roots = payload
        .and_then(|payload| payload.get("workspaceRoots"))
        .and_then(Value::as_array)
        .map(|roots| {
            roots
                .iter()
                .filter_map(Value::as_str)
                .map(AbsolutePathBuf::relative_to_current_dir)
                .collect::<std::io::Result<Vec<_>>>()
        })
        .transpose()?;
    let model = payload
        .and_then(|payload| payload.get("model"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let model_provider = payload
        .and_then(|payload| payload.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let model_gateway = payload
        .and_then(|payload| {
            payload
                .get("modelGateway")
                .or_else(|| payload.get("model_gateway"))
        })
        .and_then(Value::as_str)
        .map(str::to_string);
    let reasoning_effort = payload
        .and_then(|payload| {
            payload
                .get("reasoning")
                .or_else(|| payload.get("modelReasoningEffort"))
                .or_else(|| payload.get("model_reasoning_effort"))
        })
        .and_then(Value::as_str)
        .map(str::to_string);
    let service_tier = payload
        .and_then(|payload| payload.get("serviceTier"))
        .and_then(Value::as_str)
        .map(|value| Some(value.to_string()));
    let approval_policy = payload
        .and_then(|payload| payload.get("approvalPolicy"))
        .cloned()
        .map(serde_json::from_value::<codex_protocol::protocol::AskForApproval>)
        .transpose()?;
    let permission_profile_value = payload.and_then(|payload| payload.get("permissionProfile"));
    let permission_profile = permission_profile_value
        .filter(|value| is_background_agent_core_permission_profile_value(value))
        .cloned()
        .map(serde_json::from_value::<PermissionProfile>)
        .transpose()?;
    let default_permissions = payload
        .and_then(|payload| payload.get("permissionProfile"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let sandbox_mode = if permission_profile.is_none() {
        let mode_from_permission_profile =
            match permission_profile_value.filter(|value| value.is_object()) {
                Some(value) => background_agent_snapshot_sandbox_mode(value)?,
                None => None,
            };
        if mode_from_permission_profile.is_some() {
            mode_from_permission_profile
        } else {
            match payload.and_then(|payload| payload.get("sandboxPolicy")) {
                Some(value) => background_agent_snapshot_sandbox_mode(value)?,
                None => None,
            }
        }
    } else {
        None
    };

    let mut request_overrides = HashMap::new();
    if let Some(model_gateway) = model_gateway {
        request_overrides.insert("model_gateway".to_string(), json!(model_gateway));
    }
    if let Some(reasoning_effort) = reasoning_effort {
        request_overrides.insert(
            "model_reasoning_effort".to_string(),
            json!(reasoning_effort),
        );
    }
    let request_overrides = (!request_overrides.is_empty()).then_some(request_overrides);

    let mut config_overrides = ConfigOverrides {
        model,
        model_provider,
        service_tier,
        cwd,
        workspace_roots,
        approval_policy,
        sandbox_mode,
        permission_profile,
        default_permissions,
        auth_profile: Some(run.auth_profile_ref.clone()),
        codex_linux_sandbox_exe: context.arg0_paths.codex_linux_sandbox_exe.clone(),
        main_execve_wrapper_exe: context.arg0_paths.main_execve_wrapper_exe.clone(),
        ..Default::default()
    };
    let config = context
        .config_manager
        .load_with_overrides(request_overrides.clone(), config_overrides.clone())
        .await
        .map_err(anyhow::Error::from)?;

    let broker_decision = super::usage_profile_broker::resolve_dispatch_auth_profile(
        &context.auth_manager,
        &config,
        config_overrides.auth_profile.clone(),
    )
    .await;
    if let Some(profile) = broker_decision.selected_profile.as_ref() {
        tracing::debug!(
            run_id = %run.id,
            auth_profile = %profile,
            reason = ?broker_decision.reason,
            "usage profile broker selected auth profile for background agent"
        );
        config_overrides.auth_profile = Some(Some(profile.clone()));
        return context
            .config_manager
            .load_with_overrides(request_overrides, config_overrides)
            .await
            .map(Box::new)
            .map(BackgroundAgentConfigResolution::Ready)
            .map_err(anyhow::Error::from);
    }
    if let Some(retry_at) = broker_decision.retry_at
        && let Some(retry_at) = background_agent_broker_retry_at(&config, retry_at)
    {
        tracing::debug!(
            run_id = %run.id,
            retry_at = %retry_at.to_rfc3339(),
            reason = ?broker_decision.reason,
            "usage profile broker deferred background agent"
        );
        return Ok(BackgroundAgentConfigResolution::UsageProfileWait { retry_at });
    }
    Ok(BackgroundAgentConfigResolution::Ready(Box::new(config)))
}

async fn defer_background_agent_for_usage_profile_wait(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    retry_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let status_reason = background_agent_usage_profile_wait_reason(retry_at);
    append_status(
        context,
        run_id,
        generation,
        BackgroundAgentRunStatus::Queued,
        status_reason.as_str(),
        "agent.usageProfileWait",
        json!({
            "retryAt": retry_at.timestamp(),
        }),
    )
    .await?;
    retry_transient_sqlite_busy("finish background agent usage profile wait lease", || {
        context.state_db.finish_background_agent_process_lease(
            run_id,
            context.supervisor_id.as_str(),
            generation,
            /*exit_code*/ None,
            /*exit_signal*/ None,
            Some("usage profile wait"),
        )
    })
    .await?;
    Ok(())
}

fn background_agent_broker_retry_at(
    config: &codex_core::config::Config,
    retry_at: i64,
) -> Option<DateTime<Utc>> {
    let retry_at = DateTime::<Utc>::from_timestamp(retry_at, /*nsecs*/ 0)?;
    let buffer_secs = i64::try_from(config.usage_self_heal.reset_retry_buffer_secs).ok()?;
    let retry_at = retry_at + ChronoDuration::seconds(buffer_secs);
    (retry_at > Utc::now()).then_some(retry_at)
}

fn background_agent_usage_profile_wait_reason(retry_at: DateTime<Utc>) -> String {
    format!(
        "{BACKGROUND_AGENT_USAGE_PROFILE_WAIT_PREFIX}{}",
        retry_at.timestamp()
    )
}

fn background_agent_usage_profile_wait_until(status_reason: Option<&str>) -> Option<i64> {
    status_reason?
        .strip_prefix(BACKGROUND_AGENT_USAGE_PROFILE_WAIT_PREFIX)?
        .parse()
        .ok()
}

fn is_background_agent_core_permission_profile_value(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.contains_key("type")
            || object.contains_key("file_system")
            || object.contains_key("network")
    })
}

fn background_agent_snapshot_sandbox_mode(value: &Value) -> anyhow::Result<Option<SandboxMode>> {
    let mode = match value {
        Value::String(_) => Some(value),
        Value::Object(object) => object
            .get("mode")
            .or_else(|| object.get("sandbox"))
            .filter(|value| value.is_string()),
        _ => None,
    };
    mode.cloned()
        .map(serde_json::from_value::<SandboxMode>)
        .transpose()
        .map_err(anyhow::Error::from)
}

async fn load_background_agent_prompt(
    state_db: &StateDbHandle,
    run_id: &str,
) -> anyhow::Result<String> {
    let events = state_db
        .list_events_after(run_id, /*after_seq*/ None, Some(20))
        .await?;
    events
        .into_iter()
        .find(|event| event.event_type == "agent.started")
        .and_then(|event| {
            event
                .payload_json
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| anyhow::anyhow!("background agent prompt missing from start event"))
}

async fn handle_background_agent_event(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    thread: Arc<codex_core::CodexThread>,
    msg: EventMsg,
    cancel_token: &CancellationToken,
) -> anyhow::Result<bool> {
    match msg {
        EventMsg::TurnStarted(event) => {
            append_status(
                context,
                run_id,
                generation,
                BackgroundAgentRunStatus::Running,
                "turn started",
                "agent.turnStarted",
                json!({"turnId": event.turn_id, "startedAt": event.started_at}),
            )
            .await?;
        }
        EventMsg::AgentMessageContentDelta(delta) => {
            append_background_agent_event_with_retry(
                context,
                run_id,
                generation,
                "agent.messageDelta",
                &json!({
                    "threadId": delta.thread_id,
                    "turnId": delta.turn_id,
                    "itemId": delta.item_id,
                    "delta": delta.delta,
                }),
                false,
            )
            .await?;
        }
        EventMsg::PlanDelta(delta) => {
            append_background_agent_event_with_retry(
                context,
                run_id,
                generation,
                "agent.planDelta",
                &json!({
                    "threadId": delta.thread_id,
                    "turnId": delta.turn_id,
                    "itemId": delta.item_id,
                    "delta": delta.delta,
                }),
                false,
            )
            .await?;
        }
        EventMsg::ReasoningContentDelta(delta) => {
            append_background_agent_event_with_retry(
                context,
                run_id,
                generation,
                "agent.reasoningDelta",
                &json!({
                    "threadId": delta.thread_id,
                    "turnId": delta.turn_id,
                    "itemId": delta.item_id,
                    "delta": delta.delta,
                }),
                false,
            )
            .await?;
        }
        EventMsg::ExecApprovalRequest(event) => {
            let interaction = create_pending_interaction(
                context,
                run_id,
                generation,
                BackgroundAgentPendingInteractionKind::Approval,
                Some(event.effective_approval_id()),
                json!({
                    "type": "execApproval",
                    "callId": event.call_id,
                    "approvalId": event.approval_id,
                    "turnId": event.turn_id,
                    "command": event.command,
                    "cwd": event.cwd,
                    "reason": event.reason,
                    "availableDecisions": event.effective_available_decisions(),
                }),
                "deny",
            )
            .await?;
            let response = wait_for_pending_interaction(
                context,
                run_id,
                generation,
                &interaction,
                cancel_token,
            )
            .await?;
            thread
                .submit(Op::ExecApproval {
                    id: interaction
                        .worker_request_id
                        .clone()
                        .unwrap_or(interaction.id.clone()),
                    turn_id: response
                        .request_payload_json
                        .get("turnId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    decision: review_decision_from_response(&response),
                })
                .await?;
        }
        EventMsg::ApplyPatchApprovalRequest(event) => {
            let interaction = create_pending_interaction(
                context,
                run_id,
                generation,
                BackgroundAgentPendingInteractionKind::Approval,
                Some(event.call_id.clone()),
                json!({
                    "type": "patchApproval",
                    "callId": event.call_id,
                    "turnId": event.turn_id,
                    "changes": event.changes,
                    "reason": event.reason,
                    "grantRoot": event.grant_root,
                }),
                "deny",
            )
            .await?;
            let response = wait_for_pending_interaction(
                context,
                run_id,
                generation,
                &interaction,
                cancel_token,
            )
            .await?;
            thread
                .submit(Op::PatchApproval {
                    id: interaction
                        .worker_request_id
                        .clone()
                        .unwrap_or(interaction.id.clone()),
                    decision: review_decision_from_response(&response),
                })
                .await?;
        }
        EventMsg::RequestPermissions(event) => {
            let interaction = create_pending_interaction(
                context,
                run_id,
                generation,
                BackgroundAgentPendingInteractionKind::PermissionGrant,
                Some(event.call_id.clone()),
                json!({
                    "type": "requestPermissions",
                    "callId": event.call_id,
                    "turnId": event.turn_id,
                    "startedAtMs": event.started_at_ms,
                    "reason": event.reason,
                    "permissions": event.permissions,
                    "cwd": event.cwd,
                }),
                "deny",
            )
            .await?;
            let response = wait_for_pending_interaction(
                context,
                run_id,
                generation,
                &interaction,
                cancel_token,
            )
            .await?;
            thread
                .submit(Op::RequestPermissionsResponse {
                    id: interaction
                        .worker_request_id
                        .clone()
                        .unwrap_or(interaction.id.clone()),
                    response: request_permissions_response_from_response(&response),
                })
                .await?;
        }
        EventMsg::RequestUserInput(event) => {
            let interaction = create_pending_interaction(
                context,
                run_id,
                generation,
                BackgroundAgentPendingInteractionKind::UserInput,
                Some(event.call_id.clone()),
                json!({
                    "type": "requestUserInput",
                    "callId": event.call_id,
                    "turnId": event.turn_id,
                    "questions": event.questions,
                }),
                "cancel",
            )
            .await?;
            let response = wait_for_pending_interaction(
                context,
                run_id,
                generation,
                &interaction,
                cancel_token,
            )
            .await?;
            thread
                .submit(Op::UserInputAnswer {
                    id: interaction
                        .worker_request_id
                        .clone()
                        .unwrap_or(interaction.id.clone()),
                    response: user_input_response_from_response(&response),
                })
                .await?;
        }
        EventMsg::ElicitationRequest(event) => {
            let interaction = create_pending_interaction(
                context,
                run_id,
                generation,
                BackgroundAgentPendingInteractionKind::McpElicitation,
                Some(event.id.to_string()),
                json!({
                    "type": "mcpElicitation",
                    "turnId": event.turn_id,
                    "serverName": event.server_name,
                    "requestId": event.id,
                    "request": event.request,
                }),
                "cancel",
            )
            .await?;
            let response = wait_for_pending_interaction(
                context,
                run_id,
                generation,
                &interaction,
                cancel_token,
            )
            .await?;
            let (decision, content, meta) = elicitation_response_from_response(&response);
            thread
                .submit(Op::ResolveElicitation {
                    server_name: response
                        .request_payload_json
                        .get("serverName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    request_id: serde_json::from_value(
                        response
                            .request_payload_json
                            .get("requestId")
                            .cloned()
                            .unwrap_or(Value::Null),
                    )?,
                    decision,
                    content,
                    meta,
                })
                .await?;
        }
        EventMsg::TurnComplete(event) => {
            append_status(
                context,
                run_id,
                generation,
                BackgroundAgentRunStatus::Completed,
                "turn completed",
                "agent.completed",
                json!({
                    "turnId": event.turn_id,
                    "lastAgentMessage": event.last_agent_message,
                    "completedAt": event.completed_at,
                    "durationMs": event.duration_ms,
                }),
            )
            .await?;
            context
                .state_db
                .finish_background_agent_process_lease(
                    run_id,
                    context.supervisor_id.as_str(),
                    generation,
                    Some(0),
                    None,
                    Some("completed"),
                )
                .await?;
            return Ok(true);
        }
        EventMsg::TurnAborted(event) => {
            let status = match event.reason {
                TurnAbortReason::Interrupted | TurnAbortReason::Replaced => {
                    BackgroundAgentRunStatus::Cancelled
                }
                TurnAbortReason::ReviewEnded | TurnAbortReason::BudgetLimited => {
                    BackgroundAgentRunStatus::Failed
                }
            };
            append_status(
                context,
                run_id,
                generation,
                status,
                "turn aborted",
                "agent.aborted",
                json!({
                    "turnId": event.turn_id,
                    "reason": event.reason,
                    "completedAt": event.completed_at,
                    "durationMs": event.duration_ms,
                }),
            )
            .await?;
            context
                .state_db
                .finish_background_agent_process_lease(
                    run_id,
                    context.supervisor_id.as_str(),
                    generation,
                    Some(1),
                    None,
                    Some("turn aborted"),
                )
                .await?;
            return Ok(true);
        }
        EventMsg::ShutdownComplete => {
            append_status(
                context,
                run_id,
                generation,
                BackgroundAgentRunStatus::Cancelled,
                "worker shutdown completed",
                "agent.shutdownComplete",
                json!({}),
            )
            .await?;
            context
                .state_db
                .finish_background_agent_process_lease(
                    run_id,
                    context.supervisor_id.as_str(),
                    generation,
                    Some(1),
                    None,
                    Some("worker shutdown completed"),
                )
                .await?;
            return Ok(true);
        }
        other => {
            let event_type = core_event_type(&other);
            append_background_agent_event_with_retry(
                context,
                run_id,
                generation,
                event_type,
                &json!({}),
                false,
            )
            .await?;
        }
    }
    Ok(false)
}

async fn create_pending_interaction(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    kind: BackgroundAgentPendingInteractionKind,
    worker_request_id: Option<String>,
    request_payload_json: Value,
    no_client_policy: &str,
) -> anyhow::Result<BackgroundAgentPendingInteraction> {
    let status = match kind {
        BackgroundAgentPendingInteractionKind::Approval
        | BackgroundAgentPendingInteractionKind::PermissionGrant => {
            BackgroundAgentRunStatus::WaitingOnApproval
        }
        BackgroundAgentPendingInteractionKind::UserInput
        | BackgroundAgentPendingInteractionKind::McpElicitation => {
            BackgroundAgentRunStatus::WaitingOnUser
        }
    };
    let Some(interaction) = context
        .state_db
        .create_background_agent_pending_interaction_for_supervisor(
            &BackgroundAgentPendingInteractionCreateParams {
                id: uuid::Uuid::now_v7().to_string(),
                run_id: run_id.to_string(),
                worker_request_id,
                kind,
                request_payload_json,
                no_client_policy: no_client_policy.to_string(),
                timeout_at: Some(Utc::now() + BACKGROUND_AGENT_INTERACTION_TIMEOUT),
            },
            context.supervisor_id.as_str(),
            generation,
            status,
        )
        .await?
    else {
        return Err(background_agent_ownership_lost(run_id, generation));
    };
    upsert_interaction_status_snapshot(context, run_id, generation, status, &interaction).await?;
    Ok(interaction)
}

async fn wait_for_pending_interaction(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    interaction: &BackgroundAgentPendingInteraction,
    cancel_token: &CancellationToken,
) -> anyhow::Result<BackgroundAgentPendingInteraction> {
    let mut poll = tokio::time::interval(BACKGROUND_AGENT_INTERACTION_POLL_INTERVAL);
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                let _ = context
                    .state_db
                    .respond_pending_interaction(
                        interaction.id.as_str(),
                        &json!({"reason": "worker_stopped"}),
                        BackgroundAgentPendingInteractionStatus::Cancelled,
                    )
                    .await;
                anyhow::bail!("background agent stopped while waiting for interaction");
            }
            _ = poll.tick() => {
                context.state_db.expire_timed_out_interactions().await?;
                if !context
                    .state_db
                    .heartbeat_background_agent_run(
                        run_id,
                        context.supervisor_id.as_str(),
                        generation,
                    )
                    .await?
                {
                    return Err(background_agent_ownership_lost(run_id, generation));
                }
                let Some(run) = context.state_db.get_run(run_id).await? else {
                    anyhow::bail!("background agent run disappeared while waiting");
                };
                if run.desired_state != BackgroundAgentDesiredState::Running {
                    anyhow::bail!("background agent no longer wants to run");
                }
                let Some(updated) = context
                    .state_db
                    .get_pending_interaction(interaction.id.as_str())
                    .await?
                else {
                    anyhow::bail!("pending interaction disappeared: {}", interaction.id);
                };
                if matches!(
                    updated.status,
                    BackgroundAgentPendingInteractionStatus::Responded
                        | BackgroundAgentPendingInteractionStatus::Expired
                        | BackgroundAgentPendingInteractionStatus::Cancelled
                        | BackgroundAgentPendingInteractionStatus::Denied
                        | BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting
                ) {
                    if !context
                        .state_db
                        .update_background_agent_run_status_for_supervisor(
                            run_id,
                            context.supervisor_id.as_str(),
                            generation,
                            BackgroundAgentRunStatus::Running,
                            Some("pending interaction resolved"),
                        )
                        .await?
                    {
                        return Err(background_agent_ownership_lost(run_id, generation));
                    }
                    return Ok(updated);
                }
            }
        }
    }
}

async fn refresh_startup_heartbeat(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
) -> anyhow::Result<()> {
    if context
        .state_db
        .heartbeat_background_agent_run(run_id, context.supervisor_id.as_str(), generation)
        .await?
    {
        Ok(())
    } else {
        Err(background_agent_ownership_lost(run_id, generation))
    }
}

async fn with_startup_heartbeat<T, F>(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    phase: &'static str,
    operation: F,
) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let mut heartbeat = tokio::time::interval(BACKGROUND_AGENT_HEARTBEAT_INTERVAL);
    tokio::pin!(operation);
    loop {
        tokio::select! {
            result = &mut operation => return result,
            _ = heartbeat.tick() => {
                refresh_startup_heartbeat(context, run_id, generation)
                    .await
                    .with_context(|| format!("background agent lost ownership during {phase}"))?;
            }
        }
    }
}

async fn heartbeat_and_continue(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    thread: &Arc<codex_core::CodexThread>,
) -> anyhow::Result<bool> {
    if !context
        .state_db
        .heartbeat_background_agent_run(run_id, context.supervisor_id.as_str(), generation)
        .await?
    {
        let _ = thread.submit(Op::Interrupt).await;
        return Ok(false);
    }
    let Some(run) = context.state_db.get_run(run_id).await? else {
        return Ok(false);
    };
    if run.desired_state == BackgroundAgentDesiredState::Running {
        return Ok(true);
    }
    let _ = thread.submit(Op::Interrupt).await;
    append_status(
        context,
        run_id,
        generation,
        BackgroundAgentRunStatus::Cancelled,
        "background agent stopped",
        "agent.cancelled",
        json!({"reason": "desired_state_changed"}),
    )
    .await?;
    retry_transient_sqlite_busy("finish background agent desired-state stop lease", || {
        context.state_db.finish_background_agent_process_lease(
            run_id,
            context.supervisor_id.as_str(),
            generation,
            /*exit_code*/ Some(1),
            /*exit_signal*/ None,
            Some("background agent stopped"),
        )
    })
    .await?;
    Ok(false)
}

async fn stop_background_thread(
    context: &BackgroundAgentWorkerContext,
    thread: &Arc<codex_core::CodexThread>,
    run_id: &str,
    generation: i64,
) -> anyhow::Result<()> {
    let run = context.state_db.get_run(run_id).await?;
    let desired_state = run
        .as_ref()
        .map(|run| run.desired_state)
        .unwrap_or(BackgroundAgentDesiredState::Stopped);
    let _ = thread.submit(Op::Interrupt).await;
    let status = if desired_state == BackgroundAgentDesiredState::Running {
        BackgroundAgentRunStatus::Orphaned
    } else {
        BackgroundAgentRunStatus::Cancelled
    };
    let reason = if status == BackgroundAgentRunStatus::Orphaned {
        "supervisor shutdown"
    } else {
        "stop requested"
    };
    append_status(
        context,
        run_id,
        generation,
        status,
        reason,
        if status == BackgroundAgentRunStatus::Orphaned {
            "agent.orphaned"
        } else {
            "agent.cancelled"
        },
        json!({"reason": reason}),
    )
    .await?;
    context
        .state_db
        .finish_background_agent_process_lease(
            run_id,
            context.supervisor_id.as_str(),
            generation,
            Some(1),
            None,
            Some(reason),
        )
        .await?;
    Ok(())
}

async fn append_status(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    status: BackgroundAgentRunStatus,
    status_reason: &str,
    event_type: &str,
    payload_json: Value,
) -> anyhow::Result<BackgroundAgentEvent> {
    let pending_count = count_active_pending_interactions_for_run(context, run_id).await?;
    let execution_payload = latest_execution_payload(context, run_id).await?;
    let status_payload = status_snapshot_payload(
        status,
        status_reason,
        generation,
        &payload_json,
        execution_payload.as_ref(),
    );
    let event = retry_transient_sqlite_busy("append background agent status event", || {
        context
            .state_db
            .append_background_agent_status_event_for_supervisor(
                BackgroundAgentStatusEventForSupervisorParams {
                    run_id,
                    supervisor_id: context.supervisor_id.as_str(),
                    generation,
                    status,
                    status_reason: Some(status_reason),
                    event_type,
                    event_payload_json: &payload_json,
                    summary: Some(status_summary(status, status_reason)),
                    pending_interaction_count: pending_count,
                    status_payload_json: &status_payload,
                },
            )
    })
    .await?;
    event.ok_or_else(|| background_agent_ownership_lost(run_id, generation))
}

async fn upsert_interaction_status_snapshot(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    status: BackgroundAgentRunStatus,
    interaction: &BackgroundAgentPendingInteraction,
) -> anyhow::Result<()> {
    let run = retry_transient_sqlite_busy("load background agent run", || {
        context.state_db.get_run(run_id)
    })
    .await?
    .ok_or_else(|| anyhow::anyhow!("background agent run missing after pending interaction"))?;
    let pending_count = count_active_pending_interactions_for_run(context, run_id).await?;
    let execution_payload = latest_execution_payload(context, run_id).await?;
    let waiting_reason = interaction
        .request_payload_json
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| interaction.kind.as_str());
    retry_transient_sqlite_busy("upsert background agent waiting status snapshot", || {
        context
            .state_db
            .upsert_status_snapshot(BackgroundAgentStatusSnapshotParams {
                run_id: run_id.to_string(),
                seq: run.last_event_seq,
                status,
                desired_state: run.desired_state,
                summary: Some(
                    status_summary(status, "waiting for pending interaction").to_string(),
                ),
                pending_interaction_count: pending_count,
                last_event_seq: run.last_event_seq,
                payload_json: status_snapshot_payload(
                    status,
                    waiting_reason,
                    generation,
                    &json!({
                        "interactionId": interaction.id,
                        "workerRequestId": interaction.worker_request_id,
                        "kind": interaction.kind.as_str(),
                        "waitingReason": waiting_reason,
                        "requestPayload": interaction.request_payload_json,
                        "timeoutAt": interaction.timeout_at.map(|value| value.timestamp()),
                    }),
                    execution_payload.as_ref(),
                ),
            })
    })
    .await?;
    Ok(())
}

async fn latest_execution_payload(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
) -> anyhow::Result<Option<Value>> {
    Ok(context
        .state_db
        .get_latest_execution_snapshot(run_id)
        .await?
        .map(|snapshot| snapshot.payload_json))
}

async fn append_background_agent_event_with_retry(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    event_type: &str,
    payload_json: &Value,
    allow_terminal_current: bool,
) -> anyhow::Result<BackgroundAgentEvent> {
    ensure_background_agent_worker_current(context, run_id, generation, allow_terminal_current)
        .await?;
    retry_transient_sqlite_busy("append background agent event", || {
        context
            .state_db
            .append_event(run_id, event_type, payload_json)
    })
    .await
}

async fn ensure_background_agent_worker_current(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    generation: i64,
    allow_terminal_current: bool,
) -> anyhow::Result<()> {
    let Some(run) = context.state_db.get_run(run_id).await? else {
        return Err(background_agent_ownership_lost(run_id, generation));
    };
    if run.supervisor_id.as_deref() != Some(context.supervisor_id.as_str())
        || run.generation != generation
    {
        return Err(background_agent_ownership_lost(run_id, generation));
    }
    if !allow_terminal_current
        && !matches!(
            run.status,
            BackgroundAgentRunStatus::Starting
                | BackgroundAgentRunStatus::Running
                | BackgroundAgentRunStatus::WaitingOnApproval
                | BackgroundAgentRunStatus::WaitingOnUser
                | BackgroundAgentRunStatus::Stopping
        )
    {
        return Err(background_agent_ownership_lost(run_id, generation));
    }
    Ok(())
}

async fn count_active_pending_interactions_for_run(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
) -> anyhow::Result<i64> {
    let interactions = context
        .state_db
        .list_pending_interactions(run_id, None)
        .await?;
    Ok(interactions
        .into_iter()
        .filter(|interaction| {
            matches!(
                interaction.status,
                BackgroundAgentPendingInteractionStatus::Pending
                    | BackgroundAgentPendingInteractionStatus::Delivered
            )
        })
        .count() as i64)
}

async fn mark_background_agent_worker_failed(
    context: &BackgroundAgentWorkerContext,
    run_id: &str,
    err: &anyhow::Error,
) -> anyhow::Result<()> {
    let Some(run) = context.state_db.get_run(run_id).await? else {
        return Ok(());
    };
    if run.supervisor_id.as_deref() != Some(context.supervisor_id.as_str()) {
        return Ok(());
    }
    if matches!(
        run.status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    ) {
        return Ok(());
    }
    if run.desired_state != BackgroundAgentDesiredState::Running {
        append_status(
            context,
            run_id,
            run.generation,
            BackgroundAgentRunStatus::Cancelled,
            "worker stopped",
            "agent.cancelled",
            json!({"reason": "desired_state_changed"}),
        )
        .await?;
        context
            .state_db
            .finish_background_agent_process_lease(
                run_id,
                context.supervisor_id.as_str(),
                run.generation,
                Some(1),
                None,
                Some("worker stopped"),
            )
            .await?;
        return Ok(());
    }
    append_status(
        context,
        run_id,
        run.generation,
        BackgroundAgentRunStatus::Failed,
        "worker failed",
        "agent.failed",
        json!({"error": err.to_string()}),
    )
    .await?;
    context
        .state_db
        .finish_background_agent_process_lease(
            run_id,
            context.supervisor_id.as_str(),
            run.generation,
            Some(1),
            None,
            Some("worker failed"),
        )
        .await?;
    Ok(())
}

fn status_summary(status: BackgroundAgentRunStatus, status_reason: &str) -> &str {
    match status {
        BackgroundAgentRunStatus::Queued => "Queued",
        BackgroundAgentRunStatus::Starting => "Starting",
        BackgroundAgentRunStatus::Running => "Running",
        BackgroundAgentRunStatus::WaitingOnApproval => "Needs approval",
        BackgroundAgentRunStatus::WaitingOnUser => "Needs input",
        BackgroundAgentRunStatus::Stopping => "Stopping",
        BackgroundAgentRunStatus::Completed => "Completed",
        BackgroundAgentRunStatus::Failed => "Failed",
        BackgroundAgentRunStatus::Cancelled => "Stopped",
        BackgroundAgentRunStatus::Orphaned => {
            if status_reason.is_empty() {
                "Recoverable"
            } else {
                "Recoverable after supervisor exit"
            }
        }
    }
}

fn status_snapshot_payload(
    status: BackgroundAgentRunStatus,
    status_reason: &str,
    generation: i64,
    event_payload: &Value,
    execution_payload: Option<&Value>,
) -> Value {
    let mut payload = json!({
        "statusReason": status_reason,
        "generation": generation,
        "currentActivity": status_summary(status, status_reason),
        "eventPayload": event_payload,
    });
    let Some(object) = payload.as_object_mut() else {
        return payload;
    };
    for key in [
        "turnId",
        "startedAt",
        "completedAt",
        "durationMs",
        "threadId",
        "interactionId",
        "waitingReason",
    ] {
        if let Some(value) = event_payload.get(key).cloned() {
            object.insert(key.to_string(), value);
        }
    }
    if let Some(execution_payload) = execution_payload {
        for key in ["model", "provider", "serviceTier", "cwd", "authProfileRef"] {
            if let Some(value) = execution_payload.get(key).cloned() {
                object.insert(key.to_string(), value);
            }
        }
    }
    match status {
        BackgroundAgentRunStatus::WaitingOnApproval | BackgroundAgentRunStatus::WaitingOnUser => {
            if !object.contains_key("waitingReason") {
                object.insert("waitingReason".to_string(), json!(status_reason));
            }
        }
        BackgroundAgentRunStatus::Completed => {
            if let Some(value) = event_payload.get("lastAgentMessage").cloned() {
                object.insert("finalResult".to_string(), value);
            }
        }
        BackgroundAgentRunStatus::Failed
        | BackgroundAgentRunStatus::Cancelled
        | BackgroundAgentRunStatus::Orphaned => {
            if let Some(value) = event_payload.get("reason").cloned() {
                object.insert("terminalReason".to_string(), value);
            } else if let Some(value) = event_payload.get("error").cloned() {
                object.insert("terminalReason".to_string(), value);
            }
        }
        BackgroundAgentRunStatus::Queued
        | BackgroundAgentRunStatus::Starting
        | BackgroundAgentRunStatus::Running
        | BackgroundAgentRunStatus::Stopping => {}
    }
    payload
}

fn review_decision_from_response(
    interaction: &BackgroundAgentPendingInteraction,
) -> ReviewDecision {
    match interaction.status {
        BackgroundAgentPendingInteractionStatus::Responded => {}
        BackgroundAgentPendingInteractionStatus::Denied => return ReviewDecision::Denied,
        BackgroundAgentPendingInteractionStatus::Expired => return ReviewDecision::TimedOut,
        BackgroundAgentPendingInteractionStatus::Pending
        | BackgroundAgentPendingInteractionStatus::Delivered
        | BackgroundAgentPendingInteractionStatus::Cancelled
        | BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting => {
            return ReviewDecision::Abort;
        }
    }
    interaction
        .response_payload_json
        .as_ref()
        .and_then(|value| {
            value
                .get("decision")
                .cloned()
                .or_else(|| Some(value.clone()))
                .and_then(|value| serde_json::from_value::<ReviewDecision>(value).ok())
        })
        .unwrap_or(ReviewDecision::Abort)
}

fn request_permissions_response_from_response(
    interaction: &BackgroundAgentPendingInteraction,
) -> RequestPermissionsResponse {
    if interaction.status != BackgroundAgentPendingInteractionStatus::Responded {
        return RequestPermissionsResponse {
            permissions: RequestPermissionProfile::default(),
            scope: PermissionGrantScope::Turn,
            strict_auto_review: false,
        };
    }
    interaction
        .response_payload_json
        .as_ref()
        .and_then(|value| serde_json::from_value::<RequestPermissionsResponse>(value.clone()).ok())
        .unwrap_or_else(|| RequestPermissionsResponse {
            permissions: RequestPermissionProfile::default(),
            scope: PermissionGrantScope::Turn,
            strict_auto_review: false,
        })
}

fn user_input_response_from_response(
    interaction: &BackgroundAgentPendingInteraction,
) -> RequestUserInputResponse {
    if interaction.status != BackgroundAgentPendingInteractionStatus::Responded {
        return RequestUserInputResponse {
            answers: HashMap::new(),
        };
    }
    interaction
        .response_payload_json
        .as_ref()
        .and_then(|value| serde_json::from_value::<RequestUserInputResponse>(value.clone()).ok())
        .unwrap_or_else(|| RequestUserInputResponse {
            answers: HashMap::new(),
        })
}

fn elicitation_response_from_response(
    interaction: &BackgroundAgentPendingInteraction,
) -> (ElicitationAction, Option<Value>, Option<Value>) {
    match interaction.status {
        BackgroundAgentPendingInteractionStatus::Responded => {}
        BackgroundAgentPendingInteractionStatus::Denied => {
            return (ElicitationAction::Decline, None, None);
        }
        BackgroundAgentPendingInteractionStatus::Pending
        | BackgroundAgentPendingInteractionStatus::Delivered
        | BackgroundAgentPendingInteractionStatus::Expired
        | BackgroundAgentPendingInteractionStatus::Cancelled
        | BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting => {
            return (ElicitationAction::Cancel, None, None);
        }
    }
    let Some(payload) = interaction.response_payload_json.as_ref() else {
        return (ElicitationAction::Cancel, None, None);
    };
    let decision = payload
        .get("decision")
        .cloned()
        .and_then(|value| serde_json::from_value::<ElicitationAction>(value).ok())
        .unwrap_or(ElicitationAction::Cancel);
    let content = payload.get("content").cloned();
    let meta = payload.get("meta").cloned();
    (decision, content, meta)
}

fn core_event_type(msg: &EventMsg) -> &'static str {
    match msg {
        EventMsg::TurnStarted(_) => "core.turnStarted",
        EventMsg::AgentReasoning(_) => "core.agentReasoning",
        EventMsg::AgentMessage(_) => "core.agentMessage",
        EventMsg::ExecCommandBegin(_) => "core.execCommandBegin",
        EventMsg::ExecCommandOutputDelta(_) => "core.execCommandOutputDelta",
        EventMsg::ExecCommandEnd(_) => "core.execCommandEnd",
        EventMsg::McpToolCallBegin(_) => "core.mcpToolCallBegin",
        EventMsg::McpToolCallEnd(_) => "core.mcpToolCallEnd",
        EventMsg::PatchApplyBegin(_) => "core.patchApplyBegin",
        EventMsg::PatchApplyUpdated(_) => "core.patchApplyUpdated",
        EventMsg::PatchApplyEnd(_) => "core.patchApplyEnd",
        EventMsg::TurnDiff(_) => "core.turnDiff",
        EventMsg::PlanUpdate(_) => "core.planUpdate",
        EventMsg::ItemStarted(_) => "core.itemStarted",
        EventMsg::ItemCompleted(_) => "core.itemCompleted",
        EventMsg::HookStarted(_) => "core.hookStarted",
        EventMsg::HookCompleted(_) => "core.hookCompleted",
        _ => "core.event",
    }
}

async fn retry_transient_sqlite_busy<T, F, Fut>(operation: &str, mut f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut delay = Duration::from_millis(25);
    for attempt in 0..5 {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) if is_transient_sqlite_busy(&err) && attempt < 4 => {
                debug!(
                    operation,
                    attempt = attempt + 1,
                    "retrying background agent operation after SQLite busy: {err}"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("retry loop should return on success or final error")
}

fn is_transient_sqlite_busy(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("database is locked")
        || message.contains("database is busy")
        || message.contains("code: 5")
        || message.contains("code: 517")
}

pub(super) fn new_background_agent_supervisor_id() -> String {
    format!(
        "{}:{}:{}",
        BACKGROUND_AGENT_SUPERVISOR_PREFIX,
        std::process::id(),
        uuid::Uuid::now_v7()
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn process_supervisor_spawns_worker_process_and_records_event() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(temp.path().to_path_buf(), "test-provider".to_string())
                .await?;
        seed_queued_run(state_db.as_ref(), "run/with path").await?;
        let active_worker_processes = Arc::new(Mutex::new(HashMap::new()));
        let context = BackgroundAgentProcessSupervisorContext {
            state_db: Arc::clone(&state_db),
            supervisor_id: "process-supervisor-test".to_string(),
            active_worker_processes: Arc::clone(&active_worker_processes),
            codex_home: temp.path().to_path_buf(),
            codex_bin: PathBuf::from("/bin/true"),
        };

        reconcile_background_agent_worker_processes(
            context.clone(),
            Some("run/with path".to_string()),
        )
        .await?;

        let handle = active_worker_processes
            .lock()
            .await
            .get("run/with path")
            .cloned()
            .expect("worker process handle should be tracked");
        assert_eq!(handle.pgid, Some(handle.pid));
        assert!(
            handle
                .stderr_log_path
                .ends_with("background-agent-daemon/workers/run%2Fwith%20path.stderr.log")
        );
        let events = state_db
            .list_background_agent_events_after("run/with path", /*after_seq*/ None, None)
            .await?;
        let event_types = events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec!["agent.started", "agent.workerProcessSpawned"]
        );
        let spawn_event = events
            .last()
            .expect("spawn event should be present")
            .payload_json
            .clone();
        assert_eq!(
            spawn_event.get("supervisorId").and_then(Value::as_str),
            Some("process-supervisor-test")
        );
        assert_eq!(
            spawn_event.get("pid").and_then(Value::as_u64),
            Some(u64::from(handle.pid))
        );
        assert_eq!(
            spawn_event.get("pgid").and_then(Value::as_u64),
            Some(u64::from(handle.pid))
        );

        Ok(())
    }

    #[test]
    fn worker_stderr_log_paths_are_collision_free_for_unsafe_run_ids() {
        let slash_component = encode_background_agent_file_component("run/a");
        let underscore_component = encode_background_agent_file_component("run_a");

        assert_ne!(slash_component, underscore_component);
        assert_eq!(slash_component, "run%2Fa");
        assert_eq!(underscore_component, "run_a");
    }

    #[tokio::test]
    async fn usage_profile_wait_status_reason_defers_queued_run_until_reset() -> anyhow::Result<()>
    {
        let temp = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(temp.path().to_path_buf(), "test-provider".to_string())
                .await?;
        seed_queued_run(state_db.as_ref(), "usage-wait-run").await?;

        let future_retry_at = Utc::now() + ChronoDuration::seconds(60);
        let future_wait_reason = background_agent_usage_profile_wait_reason(future_retry_at);
        state_db
            .update_background_agent_run_status(
                "usage-wait-run",
                BackgroundAgentRunStatus::Queued,
                Some(future_wait_reason.as_str()),
            )
            .await?;
        let run = state_db
            .get_background_agent_run("usage-wait-run")
            .await?
            .expect("seeded run should exist");
        assert!(!should_start_background_run(&run));

        let past_retry_at = Utc::now() - ChronoDuration::seconds(60);
        let past_wait_reason = background_agent_usage_profile_wait_reason(past_retry_at);
        state_db
            .update_background_agent_run_status(
                "usage-wait-run",
                BackgroundAgentRunStatus::Queued,
                Some(past_wait_reason.as_str()),
            )
            .await?;
        let run = state_db
            .get_background_agent_run("usage-wait-run")
            .await?
            .expect("seeded run should exist");
        assert!(should_start_background_run(&run));

        Ok(())
    }

    #[tokio::test]
    async fn process_supervisor_prunes_finished_worker_handles() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(temp.path().to_path_buf(), "test-provider".to_string())
                .await?;
        seed_queued_run(state_db.as_ref(), "finished-run").await?;
        let active_worker_processes = Arc::new(Mutex::new(HashMap::new()));
        let context = BackgroundAgentProcessSupervisorContext {
            state_db,
            supervisor_id: "process-supervisor-test".to_string(),
            active_worker_processes: Arc::clone(&active_worker_processes),
            codex_home: temp.path().to_path_buf(),
            codex_bin: PathBuf::from("/bin/true"),
        };
        reconcile_background_agent_worker_processes(context.clone(), Some("finished-run".into()))
            .await?;
        assert!(
            active_worker_processes
                .lock()
                .await
                .contains_key("finished-run")
        );

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            prune_finished_background_agent_worker_processes(&context).await;
            if !active_worker_processes
                .lock()
                .await
                .contains_key("finished-run")
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for finished worker handle to be pruned");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn process_supervisor_replaces_live_handle_for_orphaned_run() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(temp.path().to_path_buf(), "test-provider".to_string())
                .await?;
        seed_queued_run(state_db.as_ref(), "orphaned-run").await?;
        let generation = state_db
            .claim_background_agent_supervisor("orphaned-run", "old-supervisor", "old-lease")
            .await?
            .expect("run should be claimed");
        let controller = WorkerProcessController::default();
        let old_handle = controller
            .spawn(
                WorkerProcessCommand::new("/bin/sh", temp.path().join("old.stderr.log"))
                    .arg("-c")
                    .arg("sleep 60"),
            )
            .await?;
        let old_stderr_log_path = old_handle.stderr_log_path.to_string_lossy().to_string();
        state_db
            .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
                run_id: "orphaned-run",
                supervisor_id: "old-supervisor",
                generation,
                pid: Some(i64::from(old_handle.pid)),
                pgid: old_handle.pgid.map(i64::from),
                job_id: Some("old-worker"),
                start_token: old_handle.start_token.as_deref(),
                stderr_log_path: Some(old_stderr_log_path.as_str()),
            })
            .await?;
        let active_worker_processes = Arc::new(Mutex::new(HashMap::from([(
            "orphaned-run".to_string(),
            old_handle.clone(),
        )])));
        assert_eq!(
            state_db
                .orphan_stale_background_agent_runs(Duration::ZERO)
                .await?,
            1
        );
        let context = BackgroundAgentProcessSupervisorContext {
            state_db: Arc::clone(&state_db),
            supervisor_id: "new-process-supervisor".to_string(),
            active_worker_processes: Arc::clone(&active_worker_processes),
            codex_home: temp.path().to_path_buf(),
            codex_bin: PathBuf::from("/bin/true"),
        };

        reconcile_background_agent_worker_processes(context, Some("orphaned-run".to_string()))
            .await?;

        let replacement = active_worker_processes
            .lock()
            .await
            .get("orphaned-run")
            .cloned()
            .expect("replacement handle should be tracked");
        assert_ne!(old_handle.pid, replacement.pid);
        assert_ne!(
            controller.status(&old_handle).await?,
            WorkerProcessStatus::Running
        );
        Ok(())
    }

    #[tokio::test]
    async fn process_supervisor_stops_live_handle_for_terminal_run() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(temp.path().to_path_buf(), "test-provider".to_string())
                .await?;
        seed_queued_run(state_db.as_ref(), "terminal-run").await?;
        state_db
            .update_background_agent_run_status(
                "terminal-run",
                BackgroundAgentRunStatus::Completed,
                Some("already completed"),
            )
            .await?;
        let controller = WorkerProcessController::default();
        let old_handle = controller
            .spawn(
                WorkerProcessCommand::new("/bin/sh", temp.path().join("terminal.stderr.log"))
                    .arg("-c")
                    .arg("sleep 60"),
            )
            .await?;
        let active_worker_processes = Arc::new(Mutex::new(HashMap::from([(
            "terminal-run".to_string(),
            old_handle.clone(),
        )])));
        let context = BackgroundAgentProcessSupervisorContext {
            state_db,
            supervisor_id: "process-supervisor-test".to_string(),
            active_worker_processes: Arc::clone(&active_worker_processes),
            codex_home: temp.path().to_path_buf(),
            codex_bin: PathBuf::from("/bin/true"),
        };

        reconcile_background_agent_worker_processes(context, Some("terminal-run".to_string()))
            .await?;

        assert!(
            !active_worker_processes
                .lock()
                .await
                .contains_key("terminal-run")
        );
        assert_ne!(
            controller.status(&old_handle).await?,
            WorkerProcessStatus::Running
        );
        Ok(())
    }

    #[test]
    fn terminal_pending_interaction_statuses_map_to_protocol_decisions() {
        let approval = BackgroundAgentPendingInteraction {
            id: "approval-1".to_string(),
            run_id: "run-1".to_string(),
            worker_request_id: Some("worker-request-1".to_string()),
            kind: BackgroundAgentPendingInteractionKind::Approval,
            status: BackgroundAgentPendingInteractionStatus::Denied,
            request_payload_json: json!({}),
            response_payload_json: None,
            no_client_policy: "deny".to_string(),
            timeout_at: None,
            created_at: Utc::now(),
            delivered_at: None,
            responded_at: Some(Utc::now()),
            updated_at: Utc::now(),
        };
        assert_eq!(
            ReviewDecision::Denied,
            review_decision_from_response(&approval)
        );

        let elicitation = BackgroundAgentPendingInteraction {
            kind: BackgroundAgentPendingInteractionKind::McpElicitation,
            ..approval
        };
        assert_eq!(
            (ElicitationAction::Decline, None, None),
            elicitation_response_from_response(&elicitation)
        );
    }

    async fn seed_queued_run(
        state_db: &codex_state::StateRuntime,
        run_id: &str,
    ) -> anyhow::Result<()> {
        state_db
            .create_background_agent_run(&codex_state::BackgroundAgentRunCreateParams {
                id: run_id.to_string(),
                idempotency_key: None,
                request_id: None,
                source: "process-supervisor-test".to_string(),
                prompt_snapshot_ref: format!("inline:{run_id}:prompt"),
                input_snapshot_ref: None,
                thread_id: None,
                thread_store_kind: "background-agent".to_string(),
                thread_store_id: None,
                rollout_path: None,
                parent_thread_id: None,
                parent_agent_run_id: None,
                spawn_linkage_json: None,
                auth_profile_ref: None,
                status_reason: Some("queued by process supervisor test".to_string()),
                config_fingerprint: Some("cfg-test".to_string()),
                version_fingerprint: Some("version-test".to_string()),
            })
            .await?;
        state_db
            .append_background_agent_event(
                run_id,
                "agent.started",
                &json!({
                    "cwd": null,
                    "prompt": "process supervisor test",
                    "promptSnapshotRef": format!("inline:{run_id}:prompt"),
                }),
            )
            .await?;
        Ok(())
    }
}
