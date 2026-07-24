use std::ops::Deref;
use std::time::Duration;

use serde_json::json;

use crate::AgentAttachSnapshot;
use crate::AgentEventJournal;
use crate::AgentExecution;
use crate::AgentRunStore;
use crate::AgentSnapshotStore;
use crate::AgentSupervisor;
use crate::BackgroundAgentDesiredState;
use crate::BackgroundAgentExecutionHandleParams;
use crate::BackgroundAgentPendingInteractionStatus;
use crate::BackgroundAgentRun;
use crate::BackgroundAgentRunStatus;
use crate::PendingInteractionLedger;
use crate::SupervisorReconcileReport;

const DEFAULT_RECONCILE_LIMIT: usize = 100;
const DEFAULT_ATTACH_EVENT_LIMIT: usize = 200;
const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableAgentSupervisorConfig {
    pub supervisor_id: String,
    pub reconcile_limit: usize,
    pub attach_event_limit: usize,
    pub heartbeat_timeout: Duration,
}

impl DurableAgentSupervisorConfig {
    pub fn new(supervisor_id: impl Into<String>) -> Self {
        Self {
            supervisor_id: supervisor_id.into(),
            reconcile_limit: DEFAULT_RECONCILE_LIMIT,
            attach_event_limit: DEFAULT_ATTACH_EVENT_LIMIT,
            heartbeat_timeout: DEFAULT_HEARTBEAT_TIMEOUT,
        }
    }
}

/// Durable supervisor orchestration over the runtime/store traits.
///
/// The supervisor claims a durable run before spawning a worker. That keeps
/// duplicate workers out of the common race where two supervisors reconcile the
/// same stale roster entry at the same time.
#[derive(Debug, Clone)]
pub struct DurableAgentSupervisor<S, E> {
    store: S,
    execution: E,
    config: DurableAgentSupervisorConfig,
}

impl<S, E> DurableAgentSupervisor<S, E> {
    pub fn new(store: S, execution: E, config: DurableAgentSupervisorConfig) -> Self {
        Self {
            store,
            execution,
            config,
        }
    }
}

impl<S, E> DurableAgentSupervisor<S, E>
where
    S: Deref + Send + Sync,
    S::Target: AgentRunStore
        + AgentEventJournal
        + AgentSnapshotStore
        + PendingInteractionLedger
        + Send
        + Sync,
    E: AgentExecution + Send + Sync,
{
    fn store(&self) -> &S::Target {
        self.store.deref()
    }

    pub async fn reconcile(&self) -> anyhow::Result<SupervisorReconcileReport> {
        let stale_runs_orphaned = self
            .store()
            .orphan_stale_runs(self.config.heartbeat_timeout)
            .await?;
        let runs = self
            .store()
            .list_runs(Some(self.config.reconcile_limit))
            .await?;
        let mut adopted_runs = 0;
        let mut orphaned_runs = 0;

        for run in runs {
            match run.status {
                BackgroundAgentRunStatus::Queued | BackgroundAgentRunStatus::Orphaned => {
                    if run.status == BackgroundAgentRunStatus::Orphaned {
                        orphaned_runs += 1;
                    }
                    if run.desired_state == BackgroundAgentDesiredState::Running
                        && self.start_run(run).await?
                    {
                        adopted_runs += 1;
                    }
                }
                BackgroundAgentRunStatus::Running
                    if run.supervisor_id.as_deref() == Some(self.config.supervisor_id.as_str()) =>
                {
                    let _ = self
                        .store()
                        .heartbeat(&run.id, self.config.supervisor_id.as_str(), run.generation)
                        .await?;
                }
                BackgroundAgentRunStatus::Starting
                | BackgroundAgentRunStatus::Running
                | BackgroundAgentRunStatus::WaitingOnApproval
                | BackgroundAgentRunStatus::WaitingOnUser
                | BackgroundAgentRunStatus::Stopping
                | BackgroundAgentRunStatus::Completed
                | BackgroundAgentRunStatus::Failed
                | BackgroundAgentRunStatus::Cancelled => {}
            }
        }

        Ok(SupervisorReconcileReport {
            stale_runs_orphaned,
            adopted_runs,
            orphaned_runs,
        })
    }

    pub async fn attach(&self, run_id: &str) -> anyhow::Result<Option<AgentAttachSnapshot>> {
        if self.store().get_run(run_id).await?.is_none() {
            return Ok(None);
        }
        self.store().expire_timed_out_interactions().await?;
        for interaction in self
            .store()
            .list_pending_interactions(
                run_id,
                Some(BackgroundAgentPendingInteractionStatus::Pending),
            )
            .await?
        {
            self.store()
                .mark_pending_interaction_delivered(interaction.id.as_str())
                .await?;
        }
        let Some(run) = self.store().get_run(run_id).await? else {
            return Ok(None);
        };
        let status_snapshot = self.store().get_status_snapshot(run_id).await?;
        let pending_interactions = self
            .store()
            .list_pending_interactions(run_id, /*status*/ None)
            .await?;
        let events = self
            .store()
            .list_events_after(
                run_id,
                /*after_seq*/ None,
                Some(self.config.attach_event_limit),
            )
            .await?;

        Ok(Some(AgentAttachSnapshot {
            run,
            status_snapshot,
            pending_interactions,
            events,
        }))
    }

    pub async fn detach(&self, _run_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn request_stop(&self, run_id: &str) -> anyhow::Result<bool> {
        let Some(run) = self.store().get_run(run_id).await? else {
            return Ok(false);
        };
        if matches!(
            run.retention_state,
            codex_state::BackgroundAgentRetentionState::DeleteRequested
                | codex_state::BackgroundAgentRetentionState::Deleted
        ) {
            return Ok(true);
        }
        if is_terminal_agent_status(run.status) {
            return Ok(true);
        }
        let terminalize_immediately = should_terminalize_unclaimed_agent_run(&run);
        let status_reason = if terminalize_immediately {
            "stop requested before worker claim"
        } else {
            "stop requested"
        };
        let stopped = self
            .store()
            .request_stop_run(
                run_id,
                run.supervisor_id.as_deref(),
                run.generation,
                status_reason,
                &json!({
                    "reason": "supervisor_requested_stop",
                    "supervisorId": self.config.supervisor_id,
                }),
            )
            .await?;
        if stopped && terminalize_immediately {
            cancel_active_pending_interactions_for_run(
                self.store(),
                run_id,
                "supervisor_requested_stop",
            )
            .await?;
        }
        Ok(stopped)
    }

    async fn start_run(&self, run: BackgroundAgentRun) -> anyhow::Result<bool> {
        let run_id = run.id.clone();
        let process_lease_id = format!(
            "{}:{}:{}",
            self.config.supervisor_id,
            run_id,
            run.generation.saturating_add(1)
        );
        let Some(generation) = self
            .store()
            .claim_supervisor(
                run_id.as_str(),
                self.config.supervisor_id.as_str(),
                process_lease_id.as_str(),
                crate::BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION,
                crate::BACKGROUND_AGENT_RUNTIME_COMPATIBILITY_FINGERPRINT,
            )
            .await?
        else {
            return Ok(false);
        };
        let claimed_run = self.store().get_run(run_id.as_str()).await?.unwrap_or(run);

        match self
            .execution
            .start(claimed_run, process_lease_id.clone())
            .await
        {
            Ok(handle) => {
                let recorded = self
                    .store()
                    .record_execution_handle(BackgroundAgentExecutionHandleParams {
                        run_id: run_id.as_str(),
                        supervisor_id: self.config.supervisor_id.as_str(),
                        generation,
                        pid: handle.pid,
                        pgid: handle.pgid,
                        job_id: handle.job_id.as_deref(),
                        start_token: None,
                        stderr_log_path: None,
                    })
                    .await?;
                if !recorded {
                    self.execution.stop(handle).await?;
                    return Ok(false);
                }
                let event_payload = json!({
                    "processLeaseId": handle.process_lease_id,
                    "pid": handle.pid,
                    "pgid": handle.pgid,
                    "jobId": handle.job_id,
                    "generation": generation,
                });
                let running = self
                    .store()
                    .append_status_event_for_supervisor(
                        crate::BackgroundAgentStatusEventForSupervisorParams {
                            run_id: run_id.as_str(),
                            supervisor_id: self.config.supervisor_id.as_str(),
                            generation,
                            status: BackgroundAgentRunStatus::Running,
                            status_reason: Some("worker started"),
                            event_type: "agent.workerStarted",
                            event_payload_json: &event_payload,
                            summary: Some("Running"),
                            pending_interaction_count: 0,
                            status_payload_json: &json!({
                                "phase": "running",
                            }),
                        },
                    )
                    .await?;
                if running.is_none() {
                    self.execution.stop(handle).await?;
                    return Ok(false);
                }
                Ok(true)
            }
            Err(err) => {
                let reason = format!("worker start failed: {err}");
                let event_payload = json!({
                    "generation": generation,
                    "error": err.to_string(),
                });
                let _failed = self
                    .store()
                    .append_status_event_for_supervisor(
                        crate::BackgroundAgentStatusEventForSupervisorParams {
                            run_id: run_id.as_str(),
                            supervisor_id: self.config.supervisor_id.as_str(),
                            generation,
                            status: BackgroundAgentRunStatus::Failed,
                            status_reason: Some(reason.as_str()),
                            event_type: "agent.workerStartFailed",
                            event_payload_json: &event_payload,
                            summary: Some(reason.as_str()),
                            pending_interaction_count: 0,
                            status_payload_json: &json!({
                                "phase": "failed",
                                "reason": reason,
                            }),
                        },
                    )
                    .await?;
                Ok(false)
            }
        }
    }
}

impl<S, E> AgentSupervisor for DurableAgentSupervisor<S, E>
where
    S: Deref + Send + Sync,
    S::Target: AgentRunStore
        + AgentEventJournal
        + AgentSnapshotStore
        + PendingInteractionLedger
        + Send
        + Sync,
    E: AgentExecution + Send + Sync,
{
    async fn reconcile(&self) -> anyhow::Result<SupervisorReconcileReport> {
        DurableAgentSupervisor::reconcile(self).await
    }

    async fn attach(&self, run_id: &str) -> anyhow::Result<AgentAttachSnapshot> {
        DurableAgentSupervisor::attach(self, run_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("background agent run not found: {run_id}"))
    }

    async fn detach(&self, run_id: &str) -> anyhow::Result<()> {
        DurableAgentSupervisor::detach(self, run_id).await
    }

    async fn request_stop(&self, run_id: &str) -> anyhow::Result<()> {
        DurableAgentSupervisor::request_stop(self, run_id)
            .await?
            .then_some(())
            .ok_or_else(|| anyhow::anyhow!("background agent run not found: {run_id}"))
    }
}

fn is_terminal_agent_status(status: BackgroundAgentRunStatus) -> bool {
    matches!(
        status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    )
}

fn should_terminalize_unclaimed_agent_run(run: &BackgroundAgentRun) -> bool {
    run.supervisor_id.is_none()
        || matches!(
            run.status,
            BackgroundAgentRunStatus::Queued | BackgroundAgentRunStatus::Orphaned
        )
}

async fn cancel_active_pending_interactions_for_run(
    store: &(impl PendingInteractionLedger + ?Sized),
    run_id: &str,
    reason: &str,
) -> anyhow::Result<()> {
    for interaction in store
        .list_pending_interactions(run_id, /*status*/ None)
        .await?
    {
        if matches!(
            interaction.status,
            BackgroundAgentPendingInteractionStatus::Pending
                | BackgroundAgentPendingInteractionStatus::Delivered
        ) {
            store
                .respond_pending_interaction(
                    interaction.id.as_str(),
                    &json!({"reason": reason}),
                    BackgroundAgentPendingInteractionStatus::Cancelled,
                )
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use codex_state::BackgroundAgentPendingInteractionCreateParams;
    use codex_state::BackgroundAgentPendingInteractionKind;
    use codex_state::BackgroundAgentRunCreateParams;
    use codex_state::StateRuntime;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::AgentExecutionHandle;
    use crate::BackgroundAgentExecutionSnapshotParams;

    #[derive(Debug, Clone, Default)]
    struct RecordingExecution {
        starts: Arc<Mutex<Vec<BackgroundAgentRun>>>,
        stops: Arc<Mutex<Vec<AgentExecutionHandle>>>,
        fail_start: bool,
    }

    impl AgentExecution for RecordingExecution {
        async fn start(
            &self,
            run: BackgroundAgentRun,
            process_lease_id: String,
        ) -> anyhow::Result<AgentExecutionHandle> {
            self.starts.lock().expect("starts lock").push(run);
            if self.fail_start {
                anyhow::bail!("configured start failure");
            }
            Ok(AgentExecutionHandle {
                process_lease_id,
                pid: Some(42),
                pgid: Some(42),
                job_id: Some("job-42".to_string()),
            })
        }

        async fn stop(&self, handle: AgentExecutionHandle) -> anyhow::Result<()> {
            self.stops.lock().expect("stops lock").push(handle);
            Ok(())
        }
    }

    #[tokio::test]
    async fn reconcile_claims_run_before_starting_worker() -> anyhow::Result<()> {
        let state = temp_state().await?;
        create_run(state.runtime.as_ref(), "run-1").await?;
        let execution = RecordingExecution::default();
        let supervisor = DurableAgentSupervisor::new(
            state.runtime.clone(),
            execution.clone(),
            DurableAgentSupervisorConfig::new("supervisor-1"),
        );

        let report = supervisor.reconcile().await?;

        assert_eq!(
            report,
            SupervisorReconcileReport {
                stale_runs_orphaned: 0,
                adopted_runs: 1,
                orphaned_runs: 0,
            }
        );
        let starts = execution.starts.lock().expect("starts lock").clone();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].status, BackgroundAgentRunStatus::Starting);
        assert_eq!(starts[0].supervisor_id, Some("supervisor-1".to_string()));

        let run = state
            .runtime
            .get_background_agent_run("run-1")
            .await?
            .expect("run should exist");
        assert_eq!(run.status, BackgroundAgentRunStatus::Running);
        assert_eq!(run.supervisor_id, Some("supervisor-1".to_string()));
        assert_eq!(run.generation, 1);
        assert_eq!(run.pid, Some(42));

        let second_execution = RecordingExecution::default();
        let second_supervisor = DurableAgentSupervisor::new(
            state.runtime.clone(),
            second_execution.clone(),
            DurableAgentSupervisorConfig::new("supervisor-2"),
        );
        assert_eq!(
            second_supervisor.reconcile().await?,
            SupervisorReconcileReport {
                stale_runs_orphaned: 0,
                adopted_runs: 0,
                orphaned_runs: 0,
            }
        );
        assert!(
            second_execution
                .starts
                .lock()
                .expect("starts lock")
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn reconcile_records_worker_start_failure() -> anyhow::Result<()> {
        let state = temp_state().await?;
        create_run(state.runtime.as_ref(), "run-1").await?;
        let execution = RecordingExecution {
            fail_start: true,
            ..RecordingExecution::default()
        };
        let supervisor = DurableAgentSupervisor::new(
            state.runtime.clone(),
            execution,
            DurableAgentSupervisorConfig::new("supervisor-1"),
        );

        let report = supervisor.reconcile().await?;

        assert_eq!(
            report,
            SupervisorReconcileReport {
                stale_runs_orphaned: 0,
                adopted_runs: 0,
                orphaned_runs: 0,
            }
        );
        let run = state
            .runtime
            .get_background_agent_run("run-1")
            .await?
            .expect("run should exist");
        assert_eq!(run.status, BackgroundAgentRunStatus::Failed);
        assert_eq!(
            state
                .runtime
                .list_background_agent_events_after(
                    "run-1", /*after_seq*/ None, /*limit*/ None
                )
                .await?
                .into_iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            vec![
                "agent.admitted".to_string(),
                "agent.started".to_string(),
                "agent.claimed".to_string(),
                "agent.workerStartFailed".to_string()
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn reconcile_orphans_and_adopts_stale_owned_run() -> anyhow::Result<()> {
        let state = temp_state().await?;
        create_run(state.runtime.as_ref(), "run-1").await?;
        let generation = state
            .runtime
            .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
            .await?
            .expect("run should be claimed");
        state
            .runtime
            .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
                run_id: "run-1",
                supervisor_id: "supervisor-1",
                generation,
                pid: Some(1),
                pgid: Some(1),
                job_id: Some("job-1"),
                start_token: None,
                stderr_log_path: None,
            })
            .await?;
        let execution = RecordingExecution::default();
        let mut config = DurableAgentSupervisorConfig::new("supervisor-2");
        config.heartbeat_timeout = Duration::ZERO;
        let supervisor =
            DurableAgentSupervisor::new(state.runtime.clone(), execution.clone(), config);

        let report = supervisor.reconcile().await?;

        assert_eq!(
            report,
            SupervisorReconcileReport {
                stale_runs_orphaned: 1,
                adopted_runs: 1,
                orphaned_runs: 1,
            }
        );
        let starts = execution.starts.lock().expect("starts lock").clone();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].status, BackgroundAgentRunStatus::Starting);
        assert_eq!(starts[0].supervisor_id, Some("supervisor-2".to_string()));
        assert_eq!(starts[0].generation, generation + 1);

        let run = state
            .runtime
            .get_background_agent_run("run-1")
            .await?
            .expect("run should exist");
        assert_eq!(run.status, BackgroundAgentRunStatus::Running);
        assert_eq!(run.supervisor_id, Some("supervisor-2".to_string()));
        assert_eq!(run.generation, generation + 1);
        Ok(())
    }

    #[tokio::test]
    async fn attach_marks_pending_interactions_delivered_before_replay() -> anyhow::Result<()> {
        let state = temp_state().await?;
        create_run(state.runtime.as_ref(), "run-1").await?;
        state
            .runtime
            .create_background_agent_pending_interaction(
                &BackgroundAgentPendingInteractionCreateParams {
                    id: "pending-1".to_string(),
                    run_id: "run-1".to_string(),
                    worker_request_id: Some("worker-req-1".to_string()),
                    kind: BackgroundAgentPendingInteractionKind::UserInput,
                    request_payload_json: serde_json::json!({"prompt": "continue?"}),
                    no_client_policy: "cancel".to_string(),
                    timeout_at: None,
                },
            )
            .await?;
        let supervisor = DurableAgentSupervisor::new(
            state.runtime.clone(),
            RecordingExecution::default(),
            DurableAgentSupervisorConfig::new("supervisor-1"),
        );

        let snapshot = supervisor
            .attach("run-1")
            .await?
            .expect("run should be attachable");

        assert_eq!(snapshot.pending_interactions.len(), 1);
        assert_eq!(
            snapshot.pending_interactions[0].status,
            BackgroundAgentPendingInteractionStatus::Delivered
        );
        let delivered = state
            .runtime
            .get_background_agent_pending_interaction("pending-1")
            .await?
            .expect("interaction should exist");
        assert_eq!(
            delivered.status,
            BackgroundAgentPendingInteractionStatus::Delivered
        );
        assert_eq!(snapshot.run.last_event_seq, 4);
        assert_eq!(
            state
                .runtime
                .list_background_agent_events_after(
                    "run-1", /*after_seq*/ None, /*limit*/ None
                )
                .await?
                .into_iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            vec![
                "agent.admitted".to_string(),
                "agent.started".to_string(),
                "interaction.created".to_string(),
                "interaction.delivered".to_string()
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn request_stop_does_not_clobber_delete_requested_run() -> anyhow::Result<()> {
        let state = temp_state().await?;
        create_run(state.runtime.as_ref(), "run-1").await?;
        assert!(
            state
                .runtime
                .request_background_agent_delete("run-1")
                .await?
        );
        let supervisor = DurableAgentSupervisor::new(
            state.runtime.clone(),
            RecordingExecution::default(),
            DurableAgentSupervisorConfig::new("supervisor-1"),
        );

        assert!(supervisor.request_stop("run-1").await?);

        let run = state
            .runtime
            .get_background_agent_run("run-1")
            .await?
            .expect("run should exist");
        assert_eq!(run.desired_state, BackgroundAgentDesiredState::Deleted);
        assert_eq!(
            run.retention_state,
            codex_state::BackgroundAgentRetentionState::DeleteRequested
        );
        Ok(())
    }

    #[tokio::test]
    async fn request_stop_terminalizes_unclaimed_run_and_cancels_interactions() -> anyhow::Result<()>
    {
        let state = temp_state().await?;
        create_run(state.runtime.as_ref(), "run-1").await?;
        state
            .runtime
            .create_background_agent_pending_interaction(
                &BackgroundAgentPendingInteractionCreateParams {
                    id: "pending-1".to_string(),
                    run_id: "run-1".to_string(),
                    worker_request_id: Some("worker-req-1".to_string()),
                    kind: BackgroundAgentPendingInteractionKind::UserInput,
                    request_payload_json: serde_json::json!({"prompt": "continue?"}),
                    no_client_policy: "cancel".to_string(),
                    timeout_at: None,
                },
            )
            .await?;
        let supervisor = DurableAgentSupervisor::new(
            state.runtime.clone(),
            RecordingExecution::default(),
            DurableAgentSupervisorConfig::new("supervisor-1"),
        );

        assert!(supervisor.request_stop("run-1").await?);

        let run = state
            .runtime
            .get_background_agent_run("run-1")
            .await?
            .expect("run should exist");
        assert_eq!(run.desired_state, BackgroundAgentDesiredState::Stopped);
        assert_eq!(run.status, BackgroundAgentRunStatus::Cancelled);
        assert_eq!(
            run.status_reason.as_deref(),
            Some("stop requested before worker claim")
        );
        let interaction = state
            .runtime
            .get_background_agent_pending_interaction("pending-1")
            .await?
            .expect("interaction should exist");
        assert_eq!(
            interaction.status,
            BackgroundAgentPendingInteractionStatus::Cancelled
        );
        assert_eq!(
            state
                .runtime
                .list_background_agent_events_after(
                    "run-1", /*after_seq*/ None, /*limit*/ None
                )
                .await?
                .into_iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            vec![
                "agent.admitted".to_string(),
                "agent.started".to_string(),
                "interaction.created".to_string(),
                "agent.stopRequested".to_string(),
                "interaction.cancelled".to_string()
            ]
        );
        Ok(())
    }

    struct TestState {
        _temp: tempfile::TempDir,
        runtime: Arc<StateRuntime>,
    }

    async fn temp_state() -> anyhow::Result<TestState> {
        let temp = tempfile::tempdir()?;
        let runtime =
            StateRuntime::init(temp.path().to_path_buf(), "test-provider".to_string()).await?;
        Ok(TestState {
            _temp: temp,
            runtime,
        })
    }

    async fn create_run(runtime: &StateRuntime, id: &str) -> anyhow::Result<BackgroundAgentRun> {
        runtime
            .admit_run(
                BackgroundAgentRunCreateParams {
                    id: id.to_string(),
                    idempotency_key: None,
                    request_id: None,
                    source: "test".to_string(),
                    prompt_snapshot_ref: format!("prompt://{id}"),
                    input_snapshot_ref: None,
                    thread_id: None,
                    thread_store_kind: "local".to_string(),
                    thread_store_id: None,
                    rollout_path: None,
                    parent_thread_id: None,
                    parent_agent_run_id: None,
                    spawn_linkage_json: None,
                    auth_profile_ref: None,
                    status_reason: Some("created".to_string()),
                    config_fingerprint: None,
                    version_fingerprint: Some(
                        crate::BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION.to_string(),
                    ),
                },
                &json!({
                    "prompt": format!("prompt for {id}"),
                    "promptSnapshotRef": format!("prompt://{id}"),
                }),
                &BackgroundAgentExecutionSnapshotParams {
                    run_id: id.to_string(),
                    snapshot_kind: "initial_execution_context".to_string(),
                    payload_json: json!({
                        "cwd": "/tmp",
                        "packageFingerprint":
                            crate::BACKGROUND_AGENT_RUNTIME_COMPATIBILITY_FINGERPRINT,
                    }),
                    recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
                    config_fingerprint: None,
                },
                /*max_active_runs*/ 8,
            )
            .await
    }
}
