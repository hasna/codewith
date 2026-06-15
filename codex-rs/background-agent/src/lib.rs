use std::future::Future;
use std::time::Duration;

pub mod daemon;
pub mod process_lifecycle;
mod supervisor;

pub use codex_state::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;
pub use codex_state::BackgroundAgentDesiredState;
pub use codex_state::BackgroundAgentEvent;
pub use codex_state::BackgroundAgentExecutionHandleParams;
pub use codex_state::BackgroundAgentExecutionSnapshot;
pub use codex_state::BackgroundAgentExecutionSnapshotParams;
pub use codex_state::BackgroundAgentPendingInteraction;
pub use codex_state::BackgroundAgentPendingInteractionCreateParams;
pub use codex_state::BackgroundAgentPendingInteractionKind;
pub use codex_state::BackgroundAgentPendingInteractionStatus;
pub use codex_state::BackgroundAgentRun;
pub use codex_state::BackgroundAgentRunCreateParams;
pub use codex_state::BackgroundAgentRunStatus;
pub use codex_state::BackgroundAgentStatusEventForSupervisorParams;
pub use codex_state::BackgroundAgentStatusSnapshot;
pub use codex_state::BackgroundAgentStatusSnapshotParams;
pub use codex_state::BackgroundAgentThreadBindingParams;
pub use codex_state::BackgroundAgentWorkspaceCleanup;
pub use codex_state::BackgroundAgentWorkspaceMode;
pub use codex_state::BackgroundAgentWorktreeLease;
pub use codex_state::BackgroundAgentWorktreeLeaseCreateParams;
pub use supervisor::DurableAgentSupervisor;
pub use supervisor::DurableAgentSupervisorConfig;

/// Durable run roster used by the background-agent supervisor.
///
/// Implementations are expected to persist run identity, desired state, liveness
/// ownership, heartbeat, and status independently from loaded app-server
/// threads or connected clients.
pub trait AgentRunStore {
    fn create_run(
        &self,
        params: BackgroundAgentRunCreateParams,
    ) -> impl Future<Output = anyhow::Result<BackgroundAgentRun>> + Send;

    fn get_run(
        &self,
        run_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentRun>>> + Send;

    fn get_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentRun>>> + Send;

    fn list_runs(
        &self,
        limit: Option<usize>,
    ) -> impl Future<Output = anyhow::Result<Vec<BackgroundAgentRun>>> + Send;

    fn count_runs_by_status(
        &self,
    ) -> impl Future<Output = anyhow::Result<Vec<(BackgroundAgentRunStatus, i64)>>> + Send;

    fn update_run_status(
        &self,
        run_id: &str,
        status: BackgroundAgentRunStatus,
        status_reason: Option<&str>,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn set_desired_state(
        &self,
        run_id: &str,
        desired_state: BackgroundAgentDesiredState,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn request_delete_run(&self, run_id: &str)
    -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn orphan_stale_runs(
        &self,
        heartbeat_timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<usize>> + Send;

    fn claim_supervisor(
        &self,
        run_id: &str,
        supervisor_id: &str,
        process_lease_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<i64>>> + Send;

    fn record_execution_handle(
        &self,
        params: BackgroundAgentExecutionHandleParams<'_>,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn heartbeat(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;
}

impl AgentRunStore for codex_state::StateRuntime {
    async fn create_run(
        &self,
        params: BackgroundAgentRunCreateParams,
    ) -> anyhow::Result<BackgroundAgentRun> {
        self.create_background_agent_run(&params).await
    }

    async fn get_run(&self, run_id: &str) -> anyhow::Result<Option<BackgroundAgentRun>> {
        self.get_background_agent_run(run_id).await
    }

    async fn get_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<BackgroundAgentRun>> {
        self.get_background_agent_run_by_idempotency_key(idempotency_key)
            .await
    }

    async fn list_runs(&self, limit: Option<usize>) -> anyhow::Result<Vec<BackgroundAgentRun>> {
        self.list_background_agent_runs(limit).await
    }

    async fn count_runs_by_status(&self) -> anyhow::Result<Vec<(BackgroundAgentRunStatus, i64)>> {
        self.count_background_agent_runs_by_status().await
    }

    async fn update_run_status(
        &self,
        run_id: &str,
        status: BackgroundAgentRunStatus,
        status_reason: Option<&str>,
    ) -> anyhow::Result<bool> {
        self.update_background_agent_run_status(run_id, status, status_reason)
            .await
    }

    async fn set_desired_state(
        &self,
        run_id: &str,
        desired_state: BackgroundAgentDesiredState,
    ) -> anyhow::Result<bool> {
        self.set_background_agent_desired_state(run_id, desired_state)
            .await
    }

    async fn request_delete_run(&self, run_id: &str) -> anyhow::Result<bool> {
        self.request_background_agent_delete(run_id).await
    }

    async fn orphan_stale_runs(&self, heartbeat_timeout: Duration) -> anyhow::Result<usize> {
        self.orphan_stale_background_agent_runs(heartbeat_timeout)
            .await
    }

    async fn claim_supervisor(
        &self,
        run_id: &str,
        supervisor_id: &str,
        process_lease_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        self.claim_background_agent_supervisor(run_id, supervisor_id, process_lease_id)
            .await
    }

    async fn record_execution_handle(
        &self,
        params: BackgroundAgentExecutionHandleParams<'_>,
    ) -> anyhow::Result<bool> {
        self.record_background_agent_execution_handle(params).await
    }

    async fn heartbeat(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
    ) -> anyhow::Result<bool> {
        self.heartbeat_background_agent_run(run_id, supervisor_id, generation)
            .await
    }
}

/// Append-only event journal for replaying agent progress after detach,
/// app-server restart, or worker crash.
pub trait AgentEventJournal {
    fn append_event(
        &self,
        run_id: &str,
        event_type: &str,
        payload_json: &serde_json::Value,
    ) -> impl Future<Output = anyhow::Result<BackgroundAgentEvent>> + Send;

    fn list_events_after(
        &self,
        run_id: &str,
        after_seq: Option<i64>,
        limit: Option<usize>,
    ) -> impl Future<Output = anyhow::Result<Vec<BackgroundAgentEvent>>> + Send;
}

impl AgentEventJournal for codex_state::StateRuntime {
    async fn append_event(
        &self,
        run_id: &str,
        event_type: &str,
        payload_json: &serde_json::Value,
    ) -> anyhow::Result<BackgroundAgentEvent> {
        self.append_background_agent_event(run_id, event_type, payload_json)
            .await
    }

    async fn list_events_after(
        &self,
        run_id: &str,
        after_seq: Option<i64>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<BackgroundAgentEvent>> {
        self.list_background_agent_events_after(run_id, after_seq, limit)
            .await
    }
}

/// Durable ledger for approvals, user input, MCP elicitation, and permission
/// prompts that may outlive any attached client.
pub trait PendingInteractionLedger {
    fn create_pending_interaction(
        &self,
        params: BackgroundAgentPendingInteractionCreateParams,
    ) -> impl Future<Output = anyhow::Result<BackgroundAgentPendingInteraction>> + Send;

    fn list_pending_interactions(
        &self,
        run_id: &str,
        status: Option<BackgroundAgentPendingInteractionStatus>,
    ) -> impl Future<Output = anyhow::Result<Vec<BackgroundAgentPendingInteraction>>> + Send;

    fn count_pending_interactions(
        &self,
        status: Option<BackgroundAgentPendingInteractionStatus>,
    ) -> impl Future<Output = anyhow::Result<i64>> + Send;

    fn get_pending_interaction(
        &self,
        interaction_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentPendingInteraction>>> + Send;

    fn mark_pending_interaction_delivered(
        &self,
        interaction_id: &str,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn respond_pending_interaction(
        &self,
        interaction_id: &str,
        response_payload_json: &serde_json::Value,
        terminal_status: BackgroundAgentPendingInteractionStatus,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn expire_timed_out_interactions(&self) -> impl Future<Output = anyhow::Result<usize>> + Send;
}

impl PendingInteractionLedger for codex_state::StateRuntime {
    async fn create_pending_interaction(
        &self,
        params: BackgroundAgentPendingInteractionCreateParams,
    ) -> anyhow::Result<BackgroundAgentPendingInteraction> {
        self.create_background_agent_pending_interaction(&params)
            .await
    }

    async fn list_pending_interactions(
        &self,
        run_id: &str,
        status: Option<BackgroundAgentPendingInteractionStatus>,
    ) -> anyhow::Result<Vec<BackgroundAgentPendingInteraction>> {
        self.list_background_agent_pending_interactions(run_id, status)
            .await
    }

    async fn count_pending_interactions(
        &self,
        status: Option<BackgroundAgentPendingInteractionStatus>,
    ) -> anyhow::Result<i64> {
        self.count_background_agent_pending_interactions(status)
            .await
    }

    async fn get_pending_interaction(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentPendingInteraction>> {
        self.get_background_agent_pending_interaction(interaction_id)
            .await
    }

    async fn mark_pending_interaction_delivered(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<bool> {
        self.mark_background_agent_pending_interaction_delivered(interaction_id)
            .await
    }

    async fn respond_pending_interaction(
        &self,
        interaction_id: &str,
        response_payload_json: &serde_json::Value,
        terminal_status: BackgroundAgentPendingInteractionStatus,
    ) -> anyhow::Result<bool> {
        self.respond_background_agent_pending_interaction(
            interaction_id,
            response_payload_json,
            terminal_status,
        )
        .await
    }

    async fn expire_timed_out_interactions(&self) -> anyhow::Result<usize> {
        self.expire_background_agent_pending_interactions().await
    }
}

/// Durable status and execution snapshots used for cheap roster reads and
/// crash-restart reconciliation.
pub trait AgentSnapshotStore {
    fn upsert_status_snapshot(
        &self,
        params: BackgroundAgentStatusSnapshotParams,
    ) -> impl Future<Output = anyhow::Result<BackgroundAgentStatusSnapshot>> + Send;

    fn get_status_snapshot(
        &self,
        run_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentStatusSnapshot>>> + Send;

    fn create_execution_snapshot(
        &self,
        params: BackgroundAgentExecutionSnapshotParams,
    ) -> impl Future<Output = anyhow::Result<BackgroundAgentExecutionSnapshot>> + Send;

    fn get_latest_execution_snapshot(
        &self,
        run_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentExecutionSnapshot>>> + Send;
}

impl AgentSnapshotStore for codex_state::StateRuntime {
    async fn upsert_status_snapshot(
        &self,
        params: BackgroundAgentStatusSnapshotParams,
    ) -> anyhow::Result<BackgroundAgentStatusSnapshot> {
        self.upsert_background_agent_status_snapshot(&params).await
    }

    async fn get_status_snapshot(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentStatusSnapshot>> {
        self.get_background_agent_status_snapshot(run_id).await
    }

    async fn create_execution_snapshot(
        &self,
        params: BackgroundAgentExecutionSnapshotParams,
    ) -> anyhow::Result<BackgroundAgentExecutionSnapshot> {
        self.create_background_agent_execution_snapshot(&params)
            .await
    }

    async fn get_latest_execution_snapshot(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentExecutionSnapshot>> {
        self.get_latest_background_agent_execution_snapshot(run_id)
            .await
    }
}

/// Durable workspace lease store for background-agent isolation and cleanup.
///
/// Implementations record the selected workspace mode, git status snapshot,
/// dirty-worktree state, and cleanup decisions. Cleanup must protect dirty or
/// untracked work unless an explicit force path is recorded.
pub trait AgentWorkspaceStore {
    fn create_worktree_lease(
        &self,
        params: BackgroundAgentWorktreeLeaseCreateParams,
    ) -> impl Future<Output = anyhow::Result<BackgroundAgentWorktreeLease>> + Send;

    fn get_worktree_lease_for_run(
        &self,
        run_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentWorktreeLease>>> + Send;

    fn update_worktree_lease_status(
        &self,
        lease_id: &str,
        dirty: bool,
        status_snapshot_json: &serde_json::Value,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    fn release_worktree_lease(
        &self,
        lease_id: &str,
        cleanup: BackgroundAgentWorkspaceCleanup,
    ) -> impl Future<Output = anyhow::Result<Option<BackgroundAgentWorktreeLease>>> + Send;
}

impl AgentWorkspaceStore for codex_state::StateRuntime {
    async fn create_worktree_lease(
        &self,
        params: BackgroundAgentWorktreeLeaseCreateParams,
    ) -> anyhow::Result<BackgroundAgentWorktreeLease> {
        self.create_background_agent_worktree_lease(&params).await
    }

    async fn get_worktree_lease_for_run(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentWorktreeLease>> {
        self.get_background_agent_worktree_lease_for_run(run_id)
            .await
    }

    async fn update_worktree_lease_status(
        &self,
        lease_id: &str,
        dirty: bool,
        status_snapshot_json: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        self.update_background_agent_worktree_lease_status(lease_id, dirty, status_snapshot_json)
            .await
    }

    async fn release_worktree_lease(
        &self,
        lease_id: &str,
        cleanup: BackgroundAgentWorkspaceCleanup,
    ) -> anyhow::Result<Option<BackgroundAgentWorktreeLease>> {
        self.release_background_agent_worktree_lease(lease_id, cleanup)
            .await
    }
}

/// Starts and controls the live worker process for one agent run.
///
/// Implementations own process creation, stdin/stdout routing, termination, and
/// platform-specific process-group or job-object behavior. They should not own
/// the durable run roster.
pub trait AgentExecution {
    fn start(
        &self,
        run: BackgroundAgentRun,
        process_lease_id: String,
    ) -> impl Future<Output = anyhow::Result<AgentExecutionHandle>> + Send;

    fn stop(&self, handle: AgentExecutionHandle)
    -> impl Future<Output = anyhow::Result<()>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExecutionHandle {
    pub process_lease_id: String,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub job_id: Option<String>,
}

/// Allocates and cleans up per-run workspaces for parallel background agents.
///
/// Implementations choose isolated git worktrees by default, record shared-repo
/// mode explicitly, and must protect dirty or untracked work from deletion
/// unless a caller has requested force cleanup through a durable operation.
pub trait WorktreeLeaseManager {
    fn prepare_workspace(
        &self,
        run: BackgroundAgentRun,
    ) -> impl Future<Output = anyhow::Result<WorkspaceLease>> + Send;

    fn release_workspace(
        &self,
        lease: WorkspaceLease,
        cleanup: WorkspaceCleanup,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLease {
    pub id: String,
    pub run_id: String,
    pub mode: WorkspaceMode,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMode {
    IsolatedWorktree,
    SharedRepository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanup {
    Retain,
    DeleteIfClean,
    ForceDelete,
}

/// Supervises durable background-agent runs independently from attached
/// clients.
///
/// Implementations reconcile orphaned leases, attach and detach subscribers,
/// request explicit stops, and keep worker ownership separate from app-server
/// connection lifetime.
pub trait AgentSupervisor {
    fn reconcile(&self) -> impl Future<Output = anyhow::Result<SupervisorReconcileReport>> + Send;

    fn attach(
        &self,
        run_id: &str,
    ) -> impl Future<Output = anyhow::Result<AgentAttachSnapshot>> + Send;

    fn detach(&self, run_id: &str) -> impl Future<Output = anyhow::Result<()>> + Send;

    fn request_stop(&self, run_id: &str) -> impl Future<Output = anyhow::Result<()>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorReconcileReport {
    pub stale_runs_orphaned: usize,
    pub adopted_runs: usize,
    pub orphaned_runs: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentAttachSnapshot {
    pub run: BackgroundAgentRun,
    pub status_snapshot: Option<BackgroundAgentStatusSnapshot>,
    pub pending_interactions: Vec<BackgroundAgentPendingInteraction>,
    pub events: Vec<BackgroundAgentEvent>,
}

/// Policy for detached or unattended pending interactions.
///
/// Implementations must default unsafe work to denial or cancellation. They
/// must never approve detached work without a caller-provided client response.
pub trait UnattendedPolicy {
    fn decision_for(&self, kind: BackgroundAgentPendingInteractionKind) -> UnattendedDecision;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnattendedDecision {
    WaitForClient,
    Deny,
    Cancel,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SafeUnattendedPolicy;

impl UnattendedPolicy for SafeUnattendedPolicy {
    fn decision_for(&self, kind: BackgroundAgentPendingInteractionKind) -> UnattendedDecision {
        match kind {
            BackgroundAgentPendingInteractionKind::Approval
            | BackgroundAgentPendingInteractionKind::PermissionGrant => UnattendedDecision::Deny,
            BackgroundAgentPendingInteractionKind::UserInput
            | BackgroundAgentPendingInteractionKind::McpElicitation => {
                UnattendedDecision::WaitForClient
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Attach,
    Detach,
    Stop,
    Delete,
    ConnectionClosed,
    AppServerRestarted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEffect {
    ReplayState,
    RemoveSubscriberOnly,
    RequestWorkerStop,
    MarkDeleteRequested,
    KeepWorkerRunning,
}

pub fn lifecycle_effect_for(action: LifecycleAction) -> LifecycleEffect {
    match action {
        LifecycleAction::Attach => LifecycleEffect::ReplayState,
        LifecycleAction::Detach => LifecycleEffect::RemoveSubscriberOnly,
        LifecycleAction::Stop => LifecycleEffect::RequestWorkerStop,
        LifecycleAction::Delete => LifecycleEffect::MarkDeleteRequested,
        LifecycleAction::ConnectionClosed | LifecycleAction::AppServerRestarted => {
            LifecycleEffect::KeepWorkerRunning
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_unattended_policy_never_approves_detached_work() {
        let policy = SafeUnattendedPolicy;

        assert_eq!(
            policy.decision_for(BackgroundAgentPendingInteractionKind::Approval),
            UnattendedDecision::Deny
        );
        assert_eq!(
            policy.decision_for(BackgroundAgentPendingInteractionKind::PermissionGrant),
            UnattendedDecision::Deny
        );
        assert_eq!(
            policy.decision_for(BackgroundAgentPendingInteractionKind::UserInput),
            UnattendedDecision::WaitForClient
        );
        assert_eq!(
            policy.decision_for(BackgroundAgentPendingInteractionKind::McpElicitation),
            UnattendedDecision::WaitForClient
        );
    }

    #[test]
    fn disconnect_lifecycle_keeps_worker_running() {
        assert_eq!(
            lifecycle_effect_for(LifecycleAction::ConnectionClosed),
            LifecycleEffect::KeepWorkerRunning
        );
        assert_eq!(
            lifecycle_effect_for(LifecycleAction::AppServerRestarted),
            LifecycleEffect::KeepWorkerRunning
        );
        assert_eq!(
            lifecycle_effect_for(LifecycleAction::Detach),
            LifecycleEffect::RemoveSubscriberOnly
        );
        assert_eq!(
            lifecycle_effect_for(LifecycleAction::Stop),
            LifecycleEffect::RequestWorkerStop
        );
    }
}
