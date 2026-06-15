use super::background_agent_processor::BackgroundAgentRequestProcessor;
use super::thread_processor::ThreadRequestProcessor;
use anyhow::Context;
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
use codex_background_agent::PendingInteractionLedger;
use codex_background_agent::daemon::background_agent_daemon_state_dir;
use codex_background_agent::process_lifecycle::WorkerProcessCommand;
use codex_background_agent::process_lifecycle::WorkerProcessController;
use codex_background_agent::process_lifecycle::WorkerProcessHandle;
use codex_background_agent::process_lifecycle::WorkerProcessStatus;
use codex_core::NewThread;
use codex_core::StartThreadOptions;
use codex_core::config::ConfigOverrides;
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

const BACKGROUND_AGENT_SUPERVISOR_PREFIX: &str = "app-server-background-agent";
const BACKGROUND_AGENT_THREAD_STORE_KIND: &str = "thread-store";
const BACKGROUND_AGENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const BACKGROUND_AGENT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
const BACKGROUND_AGENT_INTERACTION_POLL_INTERVAL: Duration = Duration::from_millis(250);
const BACKGROUND_AGENT_INTERACTION_TIMEOUT: ChronoDuration = ChronoDuration::hours(12);
const BACKGROUND_AGENT_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const BACKGROUND_AGENT_WORKER_STDERR_DIR: &str = "workers";

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
        if params.cwd.is_none() {
            params.cwd = Some(self.config.cwd.display().to_string());
        }
        if context.workspace_roots.is_none() {
            context.workspace_roots = Some(
                self.config
                    .workspace_roots
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect(),
            );
        }
        if context.approval_policy.is_none() {
            context.approval_policy = Some(self.config.permissions.approval_policy.value().into());
        }
        if context.permission_profile.is_none() {
            context.permission_profile =
                serde_json::to_value(self.config.permissions.effective_permission_profile()).ok();
        }
        if context.model.is_none() {
            context.model = self.config.model.clone();
        }
        if context.provider.is_none() {
            context.provider = Some(self.config.model_provider_id.clone());
        }
        if context.service_tier.is_none() {
            context.service_tier = self.config.service_tier.clone();
        }
        if params.auth_profile_ref.is_none() {
            params.auth_profile_ref = self.config.selected_auth_profile.clone();
        }
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
                || run.status == BackgroundAgentRunStatus::Orphaned)
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
    run.desired_state == BackgroundAgentDesiredState::Running
        && matches!(
            run.status,
            BackgroundAgentRunStatus::Queued | BackgroundAgentRunStatus::Orphaned
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
            start_or_resume_background_thread(&context, &run)
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
) -> anyhow::Result<NewThread> {
    let config = load_background_agent_config(context, run).await?;
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

async fn load_background_agent_config(
    context: &BackgroundAgentWorkerContext,
    run: &BackgroundAgentRun,
) -> anyhow::Result<codex_core::config::Config> {
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

    context
        .config_manager
        .load_with_overrides(
            /*config_overrides*/ None,
            ConfigOverrides {
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
            },
        )
        .await
        .map_err(anyhow::Error::from)
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
    if interaction.status != BackgroundAgentPendingInteractionStatus::Responded {
        return ReviewDecision::Abort;
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
    let Some(payload) = interaction.response_payload_json.as_ref() else {
        return (ElicitationAction::Cancel, None, None);
    };
    if interaction.status != BackgroundAgentPendingInteractionStatus::Responded {
        return (ElicitationAction::Cancel, None, None);
    }
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
