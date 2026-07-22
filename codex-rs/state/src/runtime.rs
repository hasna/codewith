use crate::AgentJob;
use crate::AgentJobCreateParams;
use crate::AgentJobItem;
use crate::AgentJobItemCreateParams;
use crate::AgentJobItemStatus;
use crate::AgentJobProgress;
use crate::AgentJobStatus;
use crate::BackgroundAgentDesiredState;
use crate::BackgroundAgentEvent;
use crate::BackgroundAgentExecutionHandleParams;
use crate::BackgroundAgentExecutionSnapshot;
use crate::BackgroundAgentExecutionSnapshotParams;
use crate::BackgroundAgentPendingInteraction;
use crate::BackgroundAgentPendingInteractionCreateParams;
use crate::BackgroundAgentPendingInteractionStatus;
use crate::BackgroundAgentProcessHandleRecord;
use crate::BackgroundAgentRun;
use crate::BackgroundAgentRunCreateParams;
use crate::BackgroundAgentRunStatus;
use crate::BackgroundAgentStatusEventForSupervisorParams;
use crate::BackgroundAgentStatusSnapshot;
use crate::BackgroundAgentStatusSnapshotParams;
use crate::BackgroundAgentThreadBindingParams;
use crate::BackgroundAgentWorkspaceCleanup;
use crate::BackgroundAgentWorkspaceMode;
use crate::BackgroundAgentWorktreeLease;
use crate::BackgroundAgentWorktreeLeaseCreateParams;
use crate::GOALS_DB_FILENAME;
use crate::LOGS_DB_FILENAME;
use crate::LogEntry;
use crate::LogQuery;
use crate::LogRow;
use crate::MEMORIES_DB_FILENAME;
use crate::ManagedWorktreeCleanupPolicy;
use crate::ManagedWorktreeLifecycleStatus;
use crate::ManagedWorktreeOwnerKind;
use crate::PendingInteraction;
use crate::PendingInteractionCreateParams;
use crate::PendingInteractionEvent;
use crate::PendingInteractionEventKind;
use crate::PendingInteractionKind;
use crate::PendingInteractionRespondParams;
use crate::PendingInteractionSourceKind;
use crate::PendingInteractionStatus;
use crate::STATE_DB_FILENAME;
use crate::SortKey;
use crate::ThreadMetadata;
use crate::ThreadMetadataBuilder;
use crate::ThreadsPage;
use crate::apply_rollout_item;
use crate::migrations::runtime_goals_migrator;
use crate::migrations::runtime_logs_migrator;
use crate::migrations::runtime_memories_migrator;
use crate::migrations::runtime_state_migrator;
use crate::model::AgentJobRow;
use crate::model::BackgroundAgentEventRow;
use crate::model::BackgroundAgentExecutionSnapshotRow;
use crate::model::BackgroundAgentPendingInteractionRow;
use crate::model::BackgroundAgentRunRow;
use crate::model::BackgroundAgentStatusSnapshotRow;
use crate::model::BackgroundAgentWorktreeLeaseRow;
use crate::model::PendingInteractionEventRow;
use crate::model::PendingInteractionRow;
use crate::model::ThreadRow;
use crate::model::anchor_from_item;
use crate::model::datetime_to_epoch_millis;
use crate::model::datetime_to_epoch_seconds;
use crate::model::epoch_millis_to_datetime;
use crate::paths::file_modified_time_utc;
use crate::telemetry::DbKind;
use crate::telemetry::DbTelemetry;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::protocol::RolloutItem;
use log::LevelFilter;
use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::QueryBuilder;
use sqlx::Row;
use sqlx::Sqlite;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteAutoVacuum;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;
use std::time::Instant;
use tracing::warn;

mod agent_jobs;
mod backfill;
mod background_agents;
mod goal_plans;
mod goals;
mod local_active_sessions;
mod logs;
pub(crate) use logs::LogPruneTargets;
mod machine_registry;
mod mailbox;
mod managed_worktrees;
mod memories;
mod monitors;
mod pending_interactions;
mod remote_control;
mod schedules;
#[cfg(test)]
mod test_support;
mod threads;
mod webhooks;
mod workflow_automation;
mod workflow_goal_plan_projections;
mod workflow_orchestrator;
mod workflow_verifiers;
mod workflows;

const STATE_RUNTIME_STARTUP_LOCK_FILENAME: &str = ".state-runtime-startup.lock";
const STATE_RUNTIME_STARTUP_LOCK_TIMEOUT: Duration = Duration::from_secs(60);
const STATE_RUNTIME_STARTUP_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Truncate the WAL back down to (at most) this size whenever a checkpoint
/// resets it, and escalate opportunistic checkpoints to TRUNCATE once the
/// WAL grows past it.
const WAL_JOURNAL_SIZE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
/// How often long-lived processes opportunistically checkpoint the state DB
/// WAL so that checkpoint starvation cannot let it grow without bound.
const STATE_WAL_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(300);
/// Auto-checkpoint threshold in WAL pages. With SQLite's default 4 KiB page
/// size this is ~4 MiB, matching SQLite's built-in default but set explicitly
/// so short-lived processes reliably checkpoint on their own.
const WAL_AUTOCHECKPOINT_PAGES: u32 = 1000;
/// Writer pools are capped at a single connection so that, within one process,
/// at most one connection per DB can ever hold (or attempt to acquire) the
/// SQLite write lock. This eliminates *intra-process*
/// `SQLITE_BUSY_SNAPSHOT` (extended code 517), which can otherwise occur when
/// one pooled connection commits a write while a second pooled connection is
/// mid-read-snapshot and then tries to write against the now-stale snapshot —
/// no other process needs to be involved. Serializing writes onto one
/// connection does not hurt read throughput because read-only query paths are
/// routed to a separate multi-connection reader pool (see
/// [`READER_MAX_CONNECTIONS`]); WAL permits many concurrent readers alongside
/// the single writer.
const WRITER_MAX_CONNECTIONS: u32 = 1;
/// Reader pools open the database read-only (`SQLITE_OPEN_READONLY`) and allow
/// several concurrent connections. Under WAL these readers run concurrently
/// with the single writer, and because a read-only connection never attempts a
/// write it can never raise 517. `read_only(true)` is also a safety belt: if a
/// write-bearing statement is ever routed here by mistake it fails loudly at
/// the database layer instead of silently re-introducing multi-writer 517.
const READER_MAX_CONNECTIONS: u32 = 5;

pub(crate) fn redact_state_string(input: impl AsRef<str>) -> String {
    crate::redact_local_state_string(input)
}

pub(crate) fn redact_state_optional_string(input: Option<String>) -> Option<String> {
    input.map(redact_state_string)
}

pub(crate) fn redact_state_json_string(value: &Value) -> anyhow::Result<String> {
    crate::redacted_local_state_json_string(value)
}

pub use goal_plans::DEFAULT_THREAD_GOAL_PLAN_LIST_LIMIT;
pub use goal_plans::MAX_THREAD_GOAL_PLAN_LIST_LIMIT;
pub use goal_plans::ThreadGoalPlanAddOutcome;
pub use goal_plans::ThreadGoalPlanAddParams;
pub use goal_plans::ThreadGoalPlanAdvanceOutcome;
pub use goal_plans::ThreadGoalPlanAppendParams;
pub use goal_plans::ThreadGoalPlanCreateParams;
pub use goal_plans::ThreadGoalPlanListPage;
pub use goal_plans::ThreadGoalPlanNodeCreateParams;
pub use goals::GoalAccountingMode;
pub use goals::GoalAccountingOutcome;
pub use goals::GoalBlockerAuditOutcome;
pub use goals::GoalDeleteOutcome;
pub use goals::GoalStore;
pub use goals::GoalUpdate;
pub use local_active_sessions::LocalActiveSessionHeartbeatParams;
pub use local_active_sessions::LocalActiveSessionPruneOwnerParams;
pub use local_active_sessions::LocalActiveSessionRecord;
pub use local_active_sessions::LocalActiveSessionStore;
pub use machine_registry::DEFAULT_MACHINE_REGISTRY_LIST_LIMIT;
pub use machine_registry::MAX_MACHINE_REGISTRY_LIST_LIMIT;
pub use machine_registry::MachineEndpointUpsertParams;
pub use machine_registry::MachineRegistryListPage;
pub use machine_registry::MachineRegistryListParams;
pub use machine_registry::MachineRegistryStore;
pub use machine_registry::MachineRegistryUpsertParams;
pub use mailbox::DEFAULT_MAILBOX_MESSAGE_LIST_LIMIT;
pub use mailbox::MAX_MAILBOX_MESSAGE_LIST_LIMIT;
pub use mailbox::MailboxAckParams;
pub use mailbox::MailboxClaim;
pub use mailbox::MailboxClaimParams;
pub use mailbox::MailboxDispatchClaimParams;
pub use mailbox::MailboxEnqueueOutcome;
pub use mailbox::MailboxEnqueueParams;
pub use mailbox::MailboxFailDisposition;
pub use mailbox::MailboxFailParams;
pub use mailbox::MailboxMessagePage;
pub use mailbox::MailboxMessageStore;
pub use mailbox::MailboxMessageStoreListParams;
pub use managed_worktrees::DEFAULT_MANAGED_WORKTREE_LIST_LIMIT;
pub use managed_worktrees::MAX_MANAGED_WORKTREE_LIST_LIMIT;
pub use managed_worktrees::ManagedWorktreeAssignmentTarget;
pub use managed_worktrees::ManagedWorktreeAttachParams;
pub use managed_worktrees::ManagedWorktreeCleanupFailureParams;
pub use managed_worktrees::ManagedWorktreeCreateParams;
pub use managed_worktrees::ManagedWorktreeDetachParams;
pub use managed_worktrees::ManagedWorktreeListPage;
pub use managed_worktrees::ManagedWorktreeMergeCandidateRecordParams;
pub use managed_worktrees::ManagedWorktreeReleaseParams;
pub use managed_worktrees::ManagedWorktreeStatusUpdateParams;
pub use managed_worktrees::ManagedWorktreeStore;
pub use memories::MemoryStore;
pub use monitors::MonitorStore;
pub use monitors::ThreadMonitorCreateParams;
pub use monitors::ThreadMonitorEventCreateParams;
pub use monitors::ThreadMonitorUpdate;
pub use pending_interactions::DEFAULT_PENDING_INTERACTION_LIST_LIMIT;
pub use pending_interactions::MAX_PENDING_INTERACTION_LIST_LIMIT;
pub use pending_interactions::PendingInteractionListParams;
pub use pending_interactions::PendingInteractionPage;
pub use pending_interactions::PendingInteractionRespondForSourceParams;
pub use remote_control::RemoteControlEnrollmentRecord;
pub use schedules::MAX_THREAD_SCHEDULE_NESTING_DEPTH;
pub use schedules::ScheduleStore;
pub use schedules::ThreadScheduleClaim;
pub use schedules::ThreadScheduleCreateParams;
pub use schedules::ThreadScheduleDueClaimParams;
pub use schedules::ThreadScheduleNowClaimParams;
pub use schedules::ThreadScheduleUpdate;
pub use threads::ThreadFilterOptions;
pub use webhooks::DEFAULT_WEBHOOK_EVENT_LIST_LIMIT;
pub use webhooks::MAX_WEBHOOK_EVENT_LIST_LIMIT;
pub use webhooks::WEBHOOK_EVENT_DEDUPE_CONFLICT_MESSAGE;
pub use webhooks::WebhookEventIngestOutcome;
pub use webhooks::WebhookEventIngestParams;
pub use webhooks::WebhookEventListPage;
pub use webhooks::WebhookEventListParams;
pub use webhooks::WebhookEventStore;
pub use workflow_automation::WorkflowAutomationStore;
pub use workflow_automation::WorkflowMonitorObservationOutcome;
pub use workflow_automation::WorkflowMonitorObservationParams;
pub use workflow_automation::WorkflowTimerClaim;
pub use workflow_automation::WorkflowTimerClaimParams;
pub use workflow_automation::WorkflowTimerFireCompleteOutcome;
pub use workflow_automation::WorkflowTimerFireCompleteParams;
pub use workflow_goal_plan_projections::WorkflowGoalPlanProjectionOutcome;
pub use workflow_goal_plan_projections::WorkflowGoalPlanProjectionParams;
pub use workflow_orchestrator::WorkflowRunAdvanceOutcome;
pub use workflow_orchestrator::WorkflowRunAdvanceParams;
pub use workflow_orchestrator::WorkflowRunBranchAdmission;
pub use workflow_orchestrator::WorkflowRunBranchAdmissionOutcome;
pub use workflow_orchestrator::WorkflowRunBranchAdmissionParams;
pub use workflow_orchestrator::WorkflowRunBranchReconcileOutcome;
pub use workflow_orchestrator::WorkflowRunBranchReconcileParams;
pub use workflow_orchestrator::WorkflowRunClaimOutcome;
pub use workflow_orchestrator::WorkflowRunClaimParams;
pub use workflow_verifiers::WorkflowRunVerifierClaimOutcome;
pub use workflow_verifiers::WorkflowRunVerifierClaimParams;
pub use workflow_verifiers::WorkflowRunVerifierClaimSelection;
pub use workflow_verifiers::WorkflowRunVerifierOutcomeStatus;
pub use workflow_verifiers::WorkflowRunVerifierRecordResultOutcome;
pub use workflow_verifiers::WorkflowRunVerifierRecordResultParams;
pub use workflow_verifiers::WorkflowRunVerifierResultSummary;
pub use workflows::DEFAULT_THREAD_WORKFLOW_LIST_LIMIT;
pub use workflows::DEFAULT_THREAD_WORKFLOW_RUN_LIST_LIMIT;
pub use workflows::MAX_THREAD_WORKFLOW_LIST_LIMIT;
pub use workflows::MAX_THREAD_WORKFLOW_RUN_LIST_LIMIT;
pub use workflows::WORKFLOW_STEP_APPROVAL_APPROVED;
pub use workflows::WORKFLOW_STEP_APPROVAL_PENDING;
pub use workflows::WORKFLOW_STEP_APPROVAL_REJECTED;
pub use workflows::WorkflowRunCancelParams;
pub use workflows::WorkflowRunCreateParams;
pub use workflows::WorkflowRunListPage;
pub use workflows::WorkflowRunPauseParams;
pub use workflows::WorkflowRunResumeParams;
pub use workflows::WorkflowRunStatusMutationOutcome;
pub use workflows::WorkflowRunStepApprovalDecision;
pub use workflows::WorkflowRunStepApprovalOutcome;
pub use workflows::WorkflowRunStepApprovalParams;
pub use workflows::WorkflowSpecCreateParams;
pub use workflows::WorkflowSpecDeleteOutcome;
pub use workflows::WorkflowSpecListPage;
pub use workflows::WorkflowStore;

// "Partition" is the retained-log-content bucket we cap at 10 MiB:
// - one bucket per non-null thread_id
// - one bucket per threadless (thread_id IS NULL) non-null process_uuid
// - one bucket for threadless rows with process_uuid IS NULL
// This budget tracks each row's persisted rendered log body plus non-body
// metadata, rather than the exact sum of all persisted SQLite column bytes.
const LOG_PARTITION_SIZE_LIMIT_BYTES: i64 = 10 * 1024 * 1024;
const LOG_PARTITION_ROW_LIMIT: i64 = 1_000;

#[derive(Clone, Copy)]
struct RuntimeDbSpec {
    label: &'static str,
    filename: &'static str,
    kind: DbKind,
    open_phase: &'static str,
    open_reader_phase: &'static str,
    migrate_phase: &'static str,
}

impl RuntimeDbSpec {
    fn path(self, codex_home: &Path) -> PathBuf {
        codex_home.join(self.filename)
    }
}

const STATE_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "state DB",
    filename: STATE_DB_FILENAME,
    kind: DbKind::State,
    open_phase: "open_state",
    open_reader_phase: "open_state_reader",
    migrate_phase: "migrate_state",
};

const LOGS_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "log DB",
    filename: LOGS_DB_FILENAME,
    kind: DbKind::Logs,
    open_phase: "open_logs",
    open_reader_phase: "open_logs_reader",
    migrate_phase: "migrate_logs",
};

const GOALS_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "goals DB",
    filename: GOALS_DB_FILENAME,
    kind: DbKind::Goals,
    open_phase: "open_goals",
    open_reader_phase: "open_goals_reader",
    migrate_phase: "migrate_goals",
};

const MEMORIES_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "memories DB",
    filename: MEMORIES_DB_FILENAME,
    kind: DbKind::Memories,
    open_phase: "open_memories",
    open_reader_phase: "open_memories_reader",
    migrate_phase: "migrate_memories",
};

const RUNTIME_DBS: [RuntimeDbSpec; 4] = [STATE_DB, LOGS_DB, GOALS_DB, MEMORIES_DB];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDbPath {
    pub label: &'static str,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct StateRuntime {
    codex_home: PathBuf,
    default_provider: String,
    /// Single-connection writer pool for the state DB (see
    /// [`WRITER_MAX_CONNECTIONS`]). All writes and read-then-write
    /// transactions run here; this is the pool every store is handed.
    pool: Arc<sqlx::SqlitePool>,
    /// Read-only, multi-connection reader pool for the state DB (see
    /// [`READER_MAX_CONNECTIONS`]). Pure-`SELECT` query paths (e.g. thread
    /// listing) are routed here so they stay concurrent despite the
    /// single-connection writer.
    reader_pool: Arc<sqlx::SqlitePool>,
    logs_pool: Arc<sqlx::SqlitePool>,
    /// Read-only reader pool for the logs DB; see [`Self::reader_pool`].
    logs_reader_pool: Arc<sqlx::SqlitePool>,
    goals_pool: Arc<sqlx::SqlitePool>,
    memories_pool: Arc<sqlx::SqlitePool>,
    thread_goals: GoalStore,
    thread_schedules: ScheduleStore,
    thread_monitors: MonitorStore,
    local_active_sessions: LocalActiveSessionStore,
    webhook_events: WebhookEventStore,
    machine_registry: MachineRegistryStore,
    mailbox_messages: MailboxMessageStore,
    managed_worktrees: ManagedWorktreeStore,
    workflows: WorkflowStore,
    workflow_automation: WorkflowAutomationStore,
    memories: MemoryStore,
    thread_updated_at_millis: Arc<AtomicI64>,
}

/// Guard proving that this process owns the state runtime startup lock.
pub struct StateRuntimeStartupLock {
    _file: std::fs::File,
    path: PathBuf,
}

pub fn state_runtime_startup_lock_path(sqlite_home: &Path) -> PathBuf {
    sqlite_home.join(STATE_RUNTIME_STARTUP_LOCK_FILENAME)
}

pub async fn acquire_state_runtime_startup_lock(
    sqlite_home: &Path,
) -> anyhow::Result<StateRuntimeStartupLock> {
    tokio::fs::create_dir_all(sqlite_home).await?;
    let lock_path = state_runtime_startup_lock_path(sqlite_home);
    let deadline = Instant::now() + STATE_RUNTIME_STARTUP_LOCK_TIMEOUT;
    loop {
        if let Some(lock) = try_acquire_state_runtime_startup_lock(lock_path.clone()).await? {
            return Ok(lock);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for Codewith state startup lock at {} after {:?}; another Codewith process may be starting, backfilling, or suspended while holding the lock",
                lock_path.display(),
                STATE_RUNTIME_STARTUP_LOCK_TIMEOUT
            );
        }
        tokio::time::sleep(STATE_RUNTIME_STARTUP_LOCK_POLL_INTERVAL).await;
    }
}

async fn try_acquire_state_runtime_startup_lock(
    lock_path: PathBuf,
) -> anyhow::Result<Option<StateRuntimeStartupLock>> {
    tokio::task::spawn_blocking(move || -> io::Result<Option<StateRuntimeStartupLock>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path.as_path())?;
        match file.try_lock() {
            Ok(()) => Ok(Some(StateRuntimeStartupLock {
                _file: file,
                path: lock_path,
            })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await
    .map_err(|err| anyhow::anyhow!("state startup lock task failed: {err}"))?
    .map_err(anyhow::Error::from)
}

impl StateRuntime {
    /// Initialize the state runtime using the provided Codewith home and default provider.
    ///
    /// This opens (and migrates) the SQLite databases under `codex_home`,
    /// keeping logs in a dedicated file to reduce lock contention with the
    /// rest of the state store.
    pub async fn init(codex_home: PathBuf, default_provider: String) -> anyhow::Result<Arc<Self>> {
        let startup_lock = acquire_state_runtime_startup_lock(codex_home.as_path()).await?;
        Self::init_with_acquired_startup_lock(codex_home, default_provider, &startup_lock).await
    }

    pub async fn init_with_acquired_startup_lock(
        codex_home: PathBuf,
        default_provider: String,
        startup_lock: &StateRuntimeStartupLock,
    ) -> anyhow::Result<Arc<Self>> {
        let expected_lock_path = state_runtime_startup_lock_path(codex_home.as_path());
        anyhow::ensure!(
            startup_lock.path == expected_lock_path,
            "state startup lock at {} does not guard {}",
            startup_lock.path.display(),
            codex_home.display()
        );
        Self::init_inner(
            codex_home,
            default_provider,
            /*telemetry_override*/ None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn init_with_telemetry_for_tests(
        codex_home: PathBuf,
        default_provider: String,
        telemetry_override: &dyn DbTelemetry,
    ) -> anyhow::Result<Arc<Self>> {
        let _startup_lock = acquire_state_runtime_startup_lock(codex_home.as_path()).await?;
        Self::init_inner(codex_home, default_provider, Some(telemetry_override)).await
    }

    async fn init_inner(
        codex_home: PathBuf,
        default_provider: String,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<Arc<Self>> {
        tokio::fs::create_dir_all(&codex_home).await?;
        crate::set_owner_only_dir(codex_home.as_path())?;
        let state_migrator = runtime_state_migrator();
        let logs_migrator = runtime_logs_migrator();
        let goals_migrator = runtime_goals_migrator();
        let memories_migrator = runtime_memories_migrator();
        let state_path = STATE_DB.path(codex_home.as_path());
        let logs_path = LOGS_DB.path(codex_home.as_path());
        let goals_path = GOALS_DB.path(codex_home.as_path());
        let memories_path = MEMORIES_DB.path(codex_home.as_path());
        let pool = match open_state_sqlite(&state_path, &state_migrator, telemetry_override).await {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!("failed to open state db at {}: {err}", state_path.display());
                return Err(err);
            }
        };
        let logs_pool = match open_logs_sqlite(&logs_path, &logs_migrator, telemetry_override).await
        {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!("failed to open logs db at {}: {err}", logs_path.display());
                return Err(err);
            }
        };
        let goals_pool =
            match open_goals_sqlite(&goals_path, &goals_migrator, telemetry_override).await {
                Ok(db) => Arc::new(db),
                Err(err) => {
                    warn!("failed to open goals db at {}: {err}", goals_path.display());
                    return Err(err);
                }
            };
        let memories_pool = match open_memories_sqlite(
            &memories_path,
            &memories_migrator,
            telemetry_override,
        )
        .await
        {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!(
                    "failed to open memories db at {}: {err}",
                    memories_path.display()
                );
                return Err(err);
            }
        };
        // Reader pools are opened only after the writer pools above have created
        // and migrated the database files, so a read-only connection never has
        // to create the file or run WAL recovery. They serve the read-heavy,
        // pure-`SELECT` query paths concurrently with the single writer.
        let reader_pool = match open_reader_sqlite(&state_path, STATE_DB, telemetry_override).await
        {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!(
                    "failed to open state reader db at {}: {err}",
                    state_path.display()
                );
                return Err(err);
            }
        };
        let logs_reader_pool =
            match open_reader_sqlite(&logs_path, LOGS_DB, telemetry_override).await {
                Ok(db) => Arc::new(db),
                Err(err) => {
                    warn!(
                        "failed to open logs reader db at {}: {err}",
                        logs_path.display()
                    );
                    return Err(err);
                }
            };
        let started = Instant::now();
        let backfill_state_result = ensure_backfill_state_row_in_pool(pool.as_ref()).await;
        crate::telemetry::record_init_result(
            telemetry_override,
            DbKind::State,
            "ensure_backfill_state",
            started.elapsed(),
            &backfill_state_result,
        );
        backfill_state_result?;
        let started = Instant::now();
        let thread_updated_at_millis_result: anyhow::Result<Option<i64>> =
            sqlx::query_scalar("SELECT MAX(threads.updated_at_ms) FROM threads")
                .fetch_one(pool.as_ref())
                .await
                .map_err(anyhow::Error::from);
        crate::telemetry::record_init_result(
            telemetry_override,
            DbKind::State,
            "post_init_query",
            started.elapsed(),
            &thread_updated_at_millis_result,
        );
        let thread_updated_at_millis = thread_updated_at_millis_result?;
        let thread_updated_at_millis = thread_updated_at_millis.unwrap_or(0);
        let runtime = Arc::new(Self {
            thread_goals: GoalStore::new(Arc::clone(&goals_pool)),
            thread_schedules: ScheduleStore::new(Arc::clone(&pool)),
            thread_monitors: MonitorStore::new(Arc::clone(&pool)),
            local_active_sessions: LocalActiveSessionStore::new(Arc::clone(&pool)),
            webhook_events: WebhookEventStore::new(Arc::clone(&pool)),
            machine_registry: MachineRegistryStore::new(Arc::clone(&pool)),
            mailbox_messages: MailboxMessageStore::new(Arc::clone(&pool)),
            managed_worktrees: ManagedWorktreeStore::new(Arc::clone(&pool)),
            workflows: WorkflowStore::new(Arc::clone(&pool)),
            workflow_automation: WorkflowAutomationStore::new(Arc::clone(&pool)),
            memories: MemoryStore::new(Arc::clone(&memories_pool), Arc::clone(&pool)),
            pool,
            reader_pool,
            logs_pool,
            logs_reader_pool,
            goals_pool: Arc::clone(&goals_pool),
            memories_pool: Arc::clone(&memories_pool),
            codex_home,
            default_provider,
            thread_updated_at_millis: Arc::new(AtomicI64::new(thread_updated_at_millis)),
        });
        if let Err(err) =
            managed_worktrees::path_backfill::backfill_legacy_managed_worktree_path_keys(
                runtime.pool.as_ref(),
            )
            .await
        {
            warn!("managed worktree path-key startup backfill failed: {err}");
        }
        if let Err(err) = runtime.run_logs_startup_maintenance().await {
            warn!(
                "failed to run startup maintenance for logs db at {}: {err}",
                logs_path.display(),
            );
        }
        if let Err(err) = runtime.run_state_wal_checkpoint_maintenance().await {
            warn!(
                "failed to run WAL checkpoint maintenance for state db at {}: {err}",
                state_path.display(),
            );
        }
        spawn_state_wal_checkpoint_task(&runtime);
        Ok(runtime)
    }

    /// Return the configured Codewith home directory for this runtime.
    pub fn codex_home(&self) -> &Path {
        self.codex_home.as_path()
    }

    pub fn thread_goals(&self) -> &GoalStore {
        &self.thread_goals
    }

    pub fn thread_schedules(&self) -> &ScheduleStore {
        &self.thread_schedules
    }

    pub fn thread_monitors(&self) -> &MonitorStore {
        &self.thread_monitors
    }

    pub fn local_active_sessions(&self) -> &LocalActiveSessionStore {
        &self.local_active_sessions
    }

    pub fn webhook_events(&self) -> &WebhookEventStore {
        &self.webhook_events
    }

    pub fn machine_registry(&self) -> &MachineRegistryStore {
        &self.machine_registry
    }

    pub fn mailbox_messages(&self) -> &MailboxMessageStore {
        &self.mailbox_messages
    }

    pub fn managed_worktrees(&self) -> &ManagedWorktreeStore {
        &self.managed_worktrees
    }

    pub fn workflows(&self) -> &WorkflowStore {
        &self.workflows
    }

    pub fn workflow_automation(&self) -> &WorkflowAutomationStore {
        &self.workflow_automation
    }

    pub fn memories(&self) -> &MemoryStore {
        &self.memories
    }

    pub async fn clear_memory_data_in_sqlite_home(sqlite_home: &Path) -> anyhow::Result<bool> {
        let memories_path = MEMORIES_DB.path(sqlite_home);
        if !tokio::fs::try_exists(&memories_path).await? {
            return Ok(false);
        }

        let memories_migrator = runtime_memories_migrator();
        let pool = open_memories_sqlite(
            &memories_path,
            &memories_migrator,
            /*telemetry_override*/ None,
        )
        .await?;
        memories::clear_memory_data_in_pool(&pool).await?;
        pool.close().await;
        Ok(true)
    }

    /// Opportunistically checkpoints the WAL for every runtime SQLite pool
    /// (state, logs, goals, memories) so that checkpoint starvation across many
    /// concurrent processes cannot let any WAL grow without bound (which in
    /// turn inflates busy/snapshot contention). The logs WAL in particular is
    /// the hottest write target, so it must be checkpointed on the same cadence
    /// as the state DB rather than only on startup.
    ///
    /// Uses a non-blocking PASSIVE checkpoint by default and escalates to
    /// TRUNCATE once a WAL exceeds [`WAL_JOURNAL_SIZE_LIMIT_BYTES`]. Each pool
    /// is attempted independently; a failure on one pool is logged and does not
    /// prevent the others from being checkpointed.
    pub async fn run_state_wal_checkpoint_maintenance(&self) -> anyhow::Result<()> {
        let home = self.codex_home.as_path();
        let pools: [(&SqlitePool, PathBuf); 4] = [
            (self.pool.as_ref(), STATE_DB.path(home)),
            (self.logs_pool.as_ref(), LOGS_DB.path(home)),
            (self.goals_pool.as_ref(), GOALS_DB.path(home)),
            (self.memories_pool.as_ref(), MEMORIES_DB.path(home)),
        ];
        let mut first_error: Option<anyhow::Error> = None;
        for (pool, path) in pools {
            if let Err(err) = checkpoint_wal_in_pool(pool, path.as_path()).await {
                tracing::debug!(
                    "WAL checkpoint maintenance failed for {}: {err}",
                    path.display()
                );
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

fn wal_path_for_db(db_path: &Path) -> PathBuf {
    let mut wal = db_path.as_os_str().to_os_string();
    wal.push("-wal");
    PathBuf::from(wal)
}

async fn checkpoint_wal_in_pool(pool: &SqlitePool, db_path: &Path) -> anyhow::Result<()> {
    let wal_len = tokio::fs::metadata(wal_path_for_db(db_path))
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    // PASSIVE checkpoints copy whatever is immediately available without
    // waiting on readers or writers. Once the WAL exceeds the journal size
    // limit, escalate to TRUNCATE (bounded by the connection busy_timeout)
    // so the file is actually reset instead of growing without bound.
    let statement = if wal_len > WAL_JOURNAL_SIZE_LIMIT_BYTES {
        "PRAGMA wal_checkpoint(TRUNCATE)"
    } else {
        "PRAGMA wal_checkpoint(PASSIVE)"
    };
    sqlx::query(statement).execute(pool).await?;
    Ok(())
}

/// Spawns a background task that periodically runs WAL checkpoint
/// maintenance for as long as the runtime is alive. Short-lived CLI
/// processes exit before the first tick; long-lived daemons get a periodic
/// checkpoint even when foreground write traffic starves automatic ones.
fn spawn_state_wal_checkpoint_task(runtime: &Arc<StateRuntime>) {
    let weak = Arc::downgrade(runtime);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(STATE_WAL_CHECKPOINT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first tick; startup maintenance already ran.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if let Err(err) = runtime.run_state_wal_checkpoint_maintenance().await {
                tracing::debug!("state db WAL checkpoint maintenance failed: {err}");
            }
        }
    });
}

fn base_sqlite_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30))
        // Cap how large the WAL file may remain after a checkpoint resets
        // it. Without this, a WAL that ballooned under checkpoint starvation
        // stays huge on disk forever and keeps checkpoint latencies (and
        // busy contention) high.
        .pragma(
            "journal_size_limit",
            WAL_JOURNAL_SIZE_LIMIT_BYTES.to_string(),
        )
        // Explicitly bound the auto-checkpoint threshold (in WAL pages) so even
        // short-lived processes that exit before the periodic checkpoint task
        // ticks still checkpoint opportunistically once the WAL crosses this
        // size. Left implicit, a process that only ever writes small batches
        // could leave an ever-growing WAL for the next process to inherit.
        .pragma("wal_autocheckpoint", WAL_AUTOCHECKPOINT_PAGES.to_string())
        .log_statements(LevelFilter::Off)
}

async fn open_state_sqlite(
    path: &Path,
    migrator: &Migrator,
    telemetry_override: Option<&dyn DbTelemetry>,
) -> anyhow::Result<SqlitePool> {
    // New state DBs should use incremental auto-vacuum, but retrofitting an
    // existing DB requires a full VACUUM. Do not attempt that during process
    // startup: it is maintenance work that can contend with foreground writers.
    open_sqlite(path, migrator, STATE_DB, telemetry_override).await
}

async fn open_logs_sqlite(
    path: &Path,
    migrator: &Migrator,
    telemetry_override: Option<&dyn DbTelemetry>,
) -> anyhow::Result<SqlitePool> {
    open_sqlite(path, migrator, LOGS_DB, telemetry_override).await
}

async fn open_goals_sqlite(
    path: &Path,
    migrator: &Migrator,
    telemetry_override: Option<&dyn DbTelemetry>,
) -> anyhow::Result<SqlitePool> {
    open_sqlite(path, migrator, GOALS_DB, telemetry_override).await
}

async fn open_memories_sqlite(
    path: &Path,
    migrator: &Migrator,
    telemetry_override: Option<&dyn DbTelemetry>,
) -> anyhow::Result<SqlitePool> {
    open_sqlite(path, migrator, MEMORIES_DB, telemetry_override).await
}

async fn open_sqlite(
    path: &Path,
    migrator: &Migrator,
    spec: RuntimeDbSpec,
    telemetry_override: Option<&dyn DbTelemetry>,
) -> anyhow::Result<SqlitePool> {
    let options = base_sqlite_options(path).auto_vacuum(SqliteAutoVacuum::Incremental);
    let started = Instant::now();
    // Single writer connection per DB: see WRITER_MAX_CONNECTIONS. This is the
    // pool used for every write and every read-then-write transaction, so at
    // most one connection per process can hold the SQLite write lock and
    // intra-process SQLITE_BUSY_SNAPSHOT (517) cannot occur.
    let pool_result = SqlitePoolOptions::new()
        .max_connections(WRITER_MAX_CONNECTIONS)
        .connect_with(options)
        .await
        .map_err(anyhow::Error::from);
    crate::telemetry::record_init_result(
        telemetry_override,
        spec.kind,
        spec.open_phase,
        started.elapsed(),
        &pool_result,
    );
    let pool = pool_result?;
    let started = Instant::now();
    let migrate_result = async {
        if matches!(spec.kind, DbKind::Goals) {
            repair_legacy_goals_deferred_migration_stamp(&pool, migrator).await?;
        }
        migrator
            .run(&pool)
            .await
            .map_err(|err| explain_migration_error(spec.label, err))
    }
    .await;
    crate::telemetry::record_init_result(
        telemetry_override,
        spec.kind,
        spec.migrate_phase,
        started.elapsed(),
        &migrate_result,
    );
    migrate_result?;
    enforce_sqlite_owner_only_paths(path)?;
    Ok(pool)
}

fn enforce_sqlite_owner_only_paths(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        crate::set_owner_only_file(path)?;
    }
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            crate::set_owner_only_file(sidecar.as_path())?;
        }
    }
    Ok(())
}

/// Connect options for a read-only reader pool.
///
/// Deliberately minimal: it opens the file read-only and does **not** set the
/// `journal_mode`/`synchronous`/`journal_size_limit`/`wal_autocheckpoint`
/// pragmas, all of which require write access and would fail on a
/// `SQLITE_OPEN_READONLY` connection. WAL journaling and its tuning are owned
/// by the writer pool ([`base_sqlite_options`]); a read-only connection simply
/// attaches to the existing WAL to read the latest committed snapshot.
/// `create_if_missing(false)` ensures the reader never races the writer to
/// create the database or its `-wal`/`-shm` sidecars.
fn reader_sqlite_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .busy_timeout(Duration::from_secs(30))
        .log_statements(LevelFilter::Off)
}

/// Open a read-only, multi-connection reader pool for `spec`'s database.
///
/// Must be called only after the corresponding writer pool has created and
/// migrated the file (see [`open_sqlite`]); reading is otherwise racy against
/// file/WAL creation.
async fn open_reader_sqlite(
    path: &Path,
    spec: RuntimeDbSpec,
    telemetry_override: Option<&dyn DbTelemetry>,
) -> anyhow::Result<SqlitePool> {
    let options = reader_sqlite_options(path);
    let started = Instant::now();
    let pool_result = SqlitePoolOptions::new()
        .max_connections(READER_MAX_CONNECTIONS)
        .connect_with(options)
        .await
        .map_err(anyhow::Error::from);
    crate::telemetry::record_init_result(
        telemetry_override,
        spec.kind,
        spec.open_reader_phase,
        started.elapsed(),
        &pool_result,
    );
    pool_result
}

/// Convert a migration failure into an actionable error.
///
/// A `VersionMismatch` means a migration that was already applied has a
/// different checksum than the one embedded in this binary. In a fleet where
/// several codewith versions run against the same `~/.codewith` databases (for
/// example a background-agent daemon worker started by an older managed install),
/// this is almost always version skew — a differently-versioned binary opening a
/// database migrated by another version — not corruption. sqlx's default message
/// ("migration N was previously applied but has been modified") is opaque and has
/// caused daemon-startup failures to be misdiagnosed, so surface the real cause
/// and the safe remediation (align binary versions) instead of resetting state.
///
/// Non-mismatch errors are passed through unchanged.
fn explain_migration_error(db_label: &str, err: sqlx::migrate::MigrateError) -> anyhow::Error {
    if let sqlx::migrate::MigrateError::VersionMismatch(version) = err {
        return anyhow::anyhow!(
            "{db_label} migration {version} was previously applied with a different checksum. \
             This usually means the database was migrated by a different codewith version than \
             the one now opening it (fleet/worker version skew), not corruption. Upgrade every \
             codewith process using this home directory to the same version so their embedded \
             migrations match; the database does not need to be reset."
        );
    }
    anyhow::Error::from(err)
}

/// SHA-384 checksum of the goals migration file
/// `0005_thread_goal_deferred.sql` exactly as shipped in published codewith
/// 0.1.48 builds (tag `rust-v0.1.48`). Validated against sqlx's checksum
/// algorithm and the archived migration bytes by
/// `legacy_0148_goals_deferred_checksum_matches_sqlx_checksum`.
const LEGACY_0148_GOALS_DEFERRED_V5_CHECKSUM_HEX: &str = "3cfd6e6b956509f5cd9946b7d648daf1773baffa75d5ee6c472aa521987c2cf392dbdf39e6d9d8a8a64586f793331e6c";

/// Version of the "thread goal deferred" migration in the current goals
/// migration set (`goals_migrations/0008_thread_goal_deferred.sql`).
const GOALS_DEFERRED_MIGRATION_VERSION: i64 = 8;

fn decode_hex_checksum(hex: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        hex.len().is_multiple_of(2),
        "hex checksum must have an even number of digits"
    );
    (0..hex.len())
        .step_by(2)
        .map(|idx| {
            u8::from_str_radix(&hex[idx..idx + 2], 16)
                .map_err(|err| anyhow::anyhow!("invalid hex checksum digit: {err}"))
        })
        .collect()
}

/// Re-stamp goals `_sqlx_migrations` rows written by published codewith
/// 0.1.48 before running the current migrator.
///
/// 0.1.48 shipped the "thread goal deferred" table rebuild as goals migration
/// version 5, while published 0.1.45 (and the current set) use versions 5-7
/// for plan-node assignments, goal context lifecycle, and goal titles, and
/// the current set renumbers the deferred rebuild to version 8. A database
/// stamped by 0.1.48 therefore fails checksum validation with
/// `VersionMismatch(5)` against the current migrator, which would disable the
/// entire sqlite state layer at startup.
///
/// Repair: move the 0.1.48 stamp (matched by its exact legacy checksum, so
/// 0.1.45-lineage databases are untouched) from version 5 to version 8 with
/// the embedded version-8 checksum. The migrator then applies the missing
/// versions 5-7 on top of the legacy schema; those migrations are additive
/// (ALTER TABLE ADD COLUMN / CREATE TABLE / CREATE INDEX on objects absent
/// from the 0.1.48 schema) and are folded into the current version-8 rebuild,
/// so the repaired database converges on the same schema as a fresh one.
///
/// The state-runtime startup lock is held while this runs, so the repair
/// cannot race another process's migrator.
async fn repair_legacy_goals_deferred_migration_stamp(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> anyhow::Result<()> {
    let has_migrations_table: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?;
    if has_migrations_table.is_none() {
        // Fresh database: nothing to repair.
        return Ok(());
    }
    let deferred = migrator
        .iter()
        .find(|migration| migration.version == GOALS_DEFERRED_MIGRATION_VERSION)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "goals migration set is missing version {GOALS_DEFERRED_MIGRATION_VERSION}"
            )
        })?;
    let legacy_checksum = decode_hex_checksum(LEGACY_0148_GOALS_DEFERRED_V5_CHECKSUM_HEX)?;
    let repaired = sqlx::query(
        "UPDATE _sqlx_migrations SET version = ?, description = ?, checksum = ? WHERE version = 5 AND checksum = ?",
    )
    .bind(deferred.version)
    .bind(deferred.description.as_ref())
    .bind(deferred.checksum.as_ref())
    .bind(legacy_checksum)
    .execute(pool)
    .await?;
    if repaired.rows_affected() > 0 {
        warn!(
            "re-stamped legacy codewith 0.1.48 goals migration version 5 as version {GOALS_DEFERRED_MIGRATION_VERSION}"
        );
    }
    Ok(())
}

pub(super) async fn ensure_backfill_state_row_in_pool(
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
INSERT INTO backfill_state (id, status, last_watermark, last_success_at, updated_at)
VALUES (?, ?, NULL, NULL, ?)
ON CONFLICT(id) DO NOTHING
            "#,
    )
    .bind(1_i64)
    .bind(crate::BackfillStatus::Pending.as_str())
    .bind(Utc::now().timestamp())
    .execute(pool)
    .await?;
    Ok(())
}

pub fn state_db_filename() -> String {
    STATE_DB.filename.to_string()
}

pub fn state_db_path(codex_home: &Path) -> PathBuf {
    STATE_DB.path(codex_home)
}

pub fn logs_db_filename() -> String {
    LOGS_DB.filename.to_string()
}

pub fn logs_db_path(codex_home: &Path) -> PathBuf {
    LOGS_DB.path(codex_home)
}

pub fn goals_db_filename() -> String {
    GOALS_DB.filename.to_string()
}

pub fn goals_db_path(codex_home: &Path) -> PathBuf {
    GOALS_DB.path(codex_home)
}

pub fn memories_db_filename() -> String {
    MEMORIES_DB.filename.to_string()
}

pub fn memories_db_path(codex_home: &Path) -> PathBuf {
    MEMORIES_DB.path(codex_home)
}

pub fn runtime_db_paths(codex_home: &Path) -> Vec<RuntimeDbPath> {
    RUNTIME_DBS
        .iter()
        .map(|spec| RuntimeDbPath {
            label: spec.label,
            path: spec.path(codex_home),
        })
        .collect()
}

/// Run SQLite's built-in integrity check against an existing database file.
pub async fn sqlite_integrity_check(path: &Path) -> anyhow::Result<Vec<String>> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .log_statements(LevelFilter::Off);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let rows = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_all(&pool)
        .await?;
    pool.close().await;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::GOALS_DEFERRED_MIGRATION_VERSION;
    use super::LEGACY_0148_GOALS_DEFERRED_V5_CHECKSUM_HEX;
    use super::StateRuntime;
    use super::decode_hex_checksum;
    use super::explain_migration_error;
    use super::goals_db_path;
    use super::logs_db_path;
    use super::memories_db_path;
    use super::open_state_sqlite;
    use super::runtime_goals_migrator;
    use super::runtime_state_migrator;
    use super::sqlite_integrity_check;
    use super::state_db_path;
    use super::test_support::test_thread_metadata;
    use super::test_support::unique_temp_dir;
    use crate::DB_INIT_METRIC;
    use crate::DbTelemetry;
    use crate::migrations::GOALS_MIGRATOR;
    use crate::migrations::STATE_MIGRATOR;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use sqlx::SqlSafeStr;
    use sqlx::SqlitePool;
    use sqlx::migrate::MigrateError;
    use sqlx::migrate::Migration;
    use sqlx::migrate::MigrationType;
    use sqlx::migrate::Migrator;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Mutex;

    #[test]
    fn explain_migration_error_makes_version_mismatch_actionable() {
        let err = explain_migration_error("state DB", MigrateError::VersionMismatch(5));
        let message = err.to_string();
        assert!(
            message.contains("state DB migration 5"),
            "message should name the mismatched migration: {message}"
        );
        assert!(
            message.contains("version skew"),
            "message should attribute the failure to version skew: {message}"
        );
        assert!(
            message.contains("does not need to be reset"),
            "message should reassure that the database is not corrupt: {message}"
        );
    }

    #[test]
    fn explain_migration_error_passes_through_non_mismatch_errors() {
        let err = explain_migration_error("state DB", MigrateError::VersionMissing(9_999));
        let message = err.to_string();
        assert!(
            !message.contains("version skew"),
            "non-mismatch errors must not be rewritten with the skew guidance: {message}"
        );
        assert!(
            message.contains("9999"),
            "non-mismatch errors should retain their original detail: {message}"
        );
    }

    #[derive(Default)]
    struct TestTelemetry {
        counters: Mutex<Vec<MetricEvent>>,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct MetricEvent {
        name: String,
        tags: BTreeMap<String, String>,
    }

    impl TestTelemetry {
        fn counters(&self) -> Vec<MetricEvent> {
            self.counters
                .lock()
                .expect("telemetry lock")
                .iter()
                .map(|event| MetricEvent {
                    name: event.name.clone(),
                    tags: event.tags.clone(),
                })
                .collect()
        }
    }

    impl DbTelemetry for TestTelemetry {
        fn counter(&self, name: &str, _inc: i64, tags: &[(&str, &str)]) {
            self.counters
                .lock()
                .expect("telemetry lock")
                .push(MetricEvent {
                    name: name.to_string(),
                    tags: tags_to_map(tags),
                });
        }

        fn record_duration(
            &self,
            _name: &str,
            _duration: std::time::Duration,
            _tags: &[(&str, &str)],
        ) {
        }
    }

    fn tags_to_map(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    async fn open_db_pool(path: &Path) -> SqlitePool {
        SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false),
        )
        .await
        .expect("open sqlite pool")
    }

    fn migrator_through(base: &Migrator, version: i64) -> Migrator {
        Migrator {
            migrations: Cow::Owned(
                base.migrations
                    .iter()
                    .filter(|migration| migration.version <= version)
                    .cloned()
                    .collect(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
            table_name: base.table_name.clone(),
            create_schemas: base.create_schemas.clone(),
        }
    }

    #[tokio::test]
    async fn pending_interaction_event_sequence_migration_survives_vacuum() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open state db");
        migrator_through(&STATE_MIGRATOR, /*version*/ 55)
            .run(&pool)
            .await
            .expect("apply pre-sequence state schema");
        sqlx::query(
            r#"
INSERT INTO threads (
    id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode
) VALUES ('thread-1', '', 0, 0, 'cli', 'test-provider', '/', 'fixture', 'workspace-write', 'on-request')
            "#,
        )
        .execute(&pool)
        .await
        .expect("insert pending interaction event thread");
        sqlx::query(
            r#"
INSERT INTO thread_pending_interactions (
    interaction_id,
    thread_id,
    source_kind,
    kind,
    status,
    request_payload_json,
    request_payload_sha256,
    request_payload_preview,
    request_redactions_json,
    no_client_policy,
    created_at_ms,
    updated_at_ms
) VALUES ('interaction-1', 'thread-1', 'thread', 'permission_grant', 'pending', '{}', ?, 'fixture', '[]', 'fixture', 0, 0)
            "#,
        )
        .bind("0".repeat(64))
        .execute(&pool)
        .await
        .expect("insert pending interaction event parent");

        for (event_id, event_kind, status) in [
            ("event-z-first", "created", "pending"),
            ("event-a-second", "delivered", "delivered"),
        ] {
            sqlx::query(
                r#"
INSERT INTO thread_pending_interaction_events (
    event_id,
    interaction_id,
    thread_id,
    event_kind,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    created_at_ms
) VALUES (?, 'interaction-1', 'thread-1', ?, ?, '{}', ?, 'fixture', '[]', 1700000000000)
                "#,
            )
            .bind(event_id)
            .bind(event_kind)
            .bind(status)
            .bind("0".repeat(64))
            .execute(&pool)
            .await
            .expect("insert pre-migration event using named columns");
        }

        STATE_MIGRATOR
            .run(&pool)
            .await
            .expect("apply pending interaction event sequence migration");
        let table_sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'thread_pending_interaction_events'",
        )
        .fetch_one(&pool)
        .await
        .expect("load migrated table definition");
        assert!(table_sql.contains("insertion_seq INTEGER PRIMARY KEY AUTOINCREMENT"));

        let before_vacuum: Vec<String> = sqlx::query_scalar(
            "SELECT event_id FROM thread_pending_interaction_events ORDER BY created_at_ms, insertion_seq",
        )
        .fetch_all(&pool)
        .await
        .expect("read migrated event order");
        sqlx::query("VACUUM")
            .execute(&pool)
            .await
            .expect("vacuum migrated state db");
        let after_vacuum: Vec<String> = sqlx::query_scalar(
            "SELECT event_id FROM thread_pending_interaction_events ORDER BY created_at_ms, insertion_seq",
        )
        .fetch_all(&pool)
        .await
        .expect("read vacuumed event order");
        assert_eq!(before_vacuum, after_vacuum);

        sqlx::query(
            r#"
INSERT INTO thread_pending_interaction_events (
    event_id,
    interaction_id,
    thread_id,
    event_kind,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    created_at_ms
) VALUES ('event-m-after', 'interaction-1', 'thread-1', 'responded', 'responded', '{}', ?, 'fixture', '[]', 1700000000000)
            "#,
        )
        .bind("0".repeat(64))
        .execute(&pool)
        .await
        .expect("named-column insert should remain compatible after migration");
        let event_ids: Vec<String> = sqlx::query_scalar(
            "SELECT event_id FROM thread_pending_interaction_events ORDER BY created_at_ms, insertion_seq",
        )
        .fetch_all(&pool)
        .await
        .expect("read post-migration event order");
        assert_eq!(
            event_ids,
            vec![
                "event-z-first".to_string(),
                "event-a-second".to_string(),
                "event-m-after".to_string(),
            ]
        );

        pool.close().await;
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn sqlite_integrity_check_reports_ok_for_valid_db() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let path = state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .expect("open sqlite db");
        sqlx::query("CREATE TABLE sample (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("create sample table");
        pool.close().await;

        let result = sqlite_integrity_check(&path)
            .await
            .expect("integrity check should run");

        assert_eq!(result, vec!["ok".to_string()]);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn state_runtime_startup_lock_serializes_waiters() {
        let codex_home = unique_temp_dir();
        let first_lock = super::acquire_state_runtime_startup_lock(codex_home.as_path())
            .await
            .expect("first startup lock");
        let mut second_lock = tokio::spawn({
            let codex_home = codex_home.clone();
            async move { super::acquire_state_runtime_startup_lock(codex_home.as_path()).await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !second_lock.is_finished(),
            "second startup lock should wait while first is held"
        );

        drop(first_lock);
        let second_lock = tokio::time::timeout(std::time::Duration::from_secs(2), &mut second_lock)
            .await
            .expect("second startup lock should acquire after first drops")
            .expect("second lock task should complete")
            .expect("second startup lock should succeed");
        drop(second_lock);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn init_with_acquired_startup_lock_rejects_mismatched_home() {
        let locked_home = unique_temp_dir();
        let target_home = unique_temp_dir();
        let startup_lock = super::acquire_state_runtime_startup_lock(locked_home.as_path())
            .await
            .expect("startup lock");

        let result = StateRuntime::init_with_acquired_startup_lock(
            target_home.clone(),
            "test-provider".to_string(),
            &startup_lock,
        )
        .await;
        let err = match result {
            Ok(_) => panic!("mismatched startup lock should be rejected"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("state startup lock at"));
        let _ = tokio::fs::remove_dir_all(locked_home).await;
        let _ = tokio::fs::remove_dir_all(target_home).await;
    }

    #[tokio::test]
    async fn writer_pools_use_a_single_connection_and_readers_stay_concurrent() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");

        // Every writer pool must expose exactly one connection so that, within
        // a single process, at most one connection per DB can hold the SQLite
        // write lock. This is what makes intra-process SQLITE_BUSY_SNAPSHOT
        // (extended code 517) impossible.
        for (label, pool) in [
            ("state", runtime.pool.as_ref()),
            ("logs", runtime.logs_pool.as_ref()),
            ("goals", runtime.goals_pool.as_ref()),
            ("memories", runtime.memories_pool.as_ref()),
        ] {
            assert_eq!(
                1,
                pool.options().get_max_connections(),
                "{label} writer pool must have a single connection"
            );
        }

        // Reader pools keep multiple connections so read-only query paths stay
        // concurrent alongside the single writer.
        for (label, reader) in [
            ("state", runtime.reader_pool.as_ref()),
            ("logs", runtime.logs_reader_pool.as_ref()),
        ] {
            assert_eq!(
                super::READER_MAX_CONNECTIONS,
                reader.options().get_max_connections(),
                "{label} reader pool must allow concurrent connections"
            );
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn concurrent_state_writes_and_reads_do_not_raise_busy_snapshot() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");

        // Seed a known thread that concurrent readers can fetch while writers
        // keep committing new rows.
        let seed_id = ThreadId::new();
        runtime
            .upsert_thread(&test_thread_metadata(
                codex_home.as_path(),
                seed_id,
                codex_home.clone(),
            ))
            .await
            .expect("seed thread should upsert");

        // Fan out many concurrent writers and readers against the state DB.
        // With a multi-connection writer pool, one connection committing a
        // write while another connection held a read snapshot could raise
        // SQLITE_BUSY_SNAPSHOT (517) with no other process involved; the
        // single-connection writer pool plus the read-only reader pool must
        // make that impossible (and must not deadlock).
        let mut handles = Vec::new();
        for _ in 0..12 {
            let runtime = runtime.clone();
            let cwd = codex_home.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..8 {
                    let thread_id = ThreadId::new();
                    let metadata = test_thread_metadata(cwd.as_path(), thread_id, cwd.clone());
                    runtime.upsert_thread(&metadata).await?;
                    // Reads exercise the separate reader pool concurrently with
                    // the writes above.
                    runtime.get_thread(thread_id).await?;
                    runtime.get_thread(seed_id).await?;
                }
                anyhow::Ok(())
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("write/read task should not panic")
                .expect("concurrent writes and reads must not raise 517");
        }

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn open_state_sqlite_tolerates_newer_applied_migrations() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open state db");
        STATE_MIGRATOR
            .run(&pool)
            .await
            .expect("apply current state schema");
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(9_999_i64)
        .bind("future migration")
        .bind(true)
        .bind(vec![1_u8, 2, 3, 4])
        .bind(1_i64)
        .execute(&pool)
        .await
        .expect("insert future migration record");
        pool.close().await;

        let strict_pool = open_db_pool(state_path.as_path()).await;
        let strict_err = STATE_MIGRATOR
            .run(&strict_pool)
            .await
            .expect_err("strict migrator should reject newer applied migrations");
        assert!(matches!(strict_err, MigrateError::VersionMissing(9_999)));
        strict_pool.close().await;

        let tolerant_migrator = runtime_state_migrator();
        let tolerant_pool = open_state_sqlite(
            state_path.as_path(),
            &tolerant_migrator,
            /*telemetry_override*/ None,
        )
        .await
        .expect("runtime migrator should tolerate newer applied migrations");
        tolerant_pool.close().await;

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn managed_worktree_migration_releases_dirty_shared_repo_cleanup_row() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open state db");
        let through_background_agent_worktrees = Migrator {
            migrations: Cow::Owned(
                STATE_MIGRATOR
                    .migrations
                    .iter()
                    .filter(|migration| migration.version <= 39)
                    .cloned()
                    .collect(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
            table_name: STATE_MIGRATOR.table_name.clone(),
            create_schemas: STATE_MIGRATOR.create_schemas.clone(),
        };
        through_background_agent_worktrees
            .run(&pool)
            .await
            .expect("apply legacy background agent worktree schema");
        sqlx::query(
            r#"
INSERT INTO background_agent_runs (
    id,
    source,
    prompt_snapshot_ref,
    thread_store_kind,
    desired_state,
    status,
    created_at,
    updated_at
) VALUES ('legacy-run', 'test', 'prompt', 'background-agent', 'running', 'running', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy background run should insert");
        sqlx::query(
            r#"
INSERT INTO background_agent_worktree_leases (
    id,
    run_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    status_snapshot_json,
    dirty,
    cleanup_after,
    created_at,
    updated_at,
    released_at,
    deleted_at
) VALUES
    ('active-shared', 'legacy-run', 'active-shared', 'shared_repository', '/repo', '/repo', '{}', 0, NULL, 1, 1, NULL, NULL),
    ('dirty-cleanup', 'legacy-run', 'dirty-cleanup', 'shared_repository', '/repo', '/repo', '{}', 1, 3, 1, 2, 2, NULL),
    ('isolated-cleanup', 'legacy-run', 'isolated-cleanup', 'isolated_worktree', '/repo', '/repo/.codewith/worktrees/isolated-cleanup', '{}', 1, 3, 1, 2, 2, NULL)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy worktree rows should insert");
        pool.close().await;

        let current_pool = open_state_sqlite(
            state_path.as_path(),
            &runtime_state_migrator(),
            /*telemetry_override*/ None,
        )
        .await
        .expect("current migration should accept legacy cleanup row");
        let statuses: Vec<String> = sqlx::query_scalar(
            "SELECT lifecycle_status FROM managed_worktrees ORDER BY worktree_id",
        )
        .fetch_all(&current_pool)
        .await
        .expect("managed worktree statuses should query");
        assert_eq!(
            vec![
                "active".to_string(),
                "released".to_string(),
                "cleanup_pending".to_string()
            ],
            statuses
        );
        current_pool.close().await;

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn workflow_automation_migration_upgrades_pre_0045_state_db() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open state db");
        migrator_through(&STATE_MIGRATOR, /*version*/ 44)
            .run(&pool)
            .await
            .expect("apply pre-automation workflow schema");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(crate::WorkflowSpecCreateParams {
                source_thread_id: None,
                source_yaml: codex_prompts::DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save after migration");
        let run = runtime
            .workflows()
            .create_workflow_run(crate::WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: None,
                idempotency_key: Some("automation-migration-run".to_string()),
            })
            .await
            .expect("workflow run should create after migration");

        assert_eq!(
            1,
            run.run.loops_json.as_ref().unwrap()["data"]
                .as_array()
                .unwrap()
                .len()
        );
        assert_eq!(
            1,
            run.run.monitor_links_json.as_ref().unwrap()["data"]
                .as_array()
                .unwrap()
                .len()
        );
        let timer_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_run_timers WHERE run_id = ?")
                .bind(run.run.run_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("timer count should query");
        assert_eq!(1, timer_count);
        let monitor_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_run_monitor_links WHERE run_id = ?")
                .bind(run.run.run_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("monitor count should query");
        assert_eq!(1, monitor_count);

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn workflow_goal_plan_projection_migration_upgrades_pre_0003_goals_db() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let goals_path = goals_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&goals_path)
                .create_if_missing(true),
        )
        .await
        .expect("open goals db");
        migrator_through(&GOALS_MIGRATOR, /*version*/ 2)
            .run(&pool)
            .await
            .expect("apply pre-projection goals schema");
        pool.close().await;

        let current_pool = super::open_goals_sqlite(
            goals_path.as_path(),
            &runtime_goals_migrator(),
            /*telemetry_override*/ None,
        )
        .await
        .expect("current goals migration should apply");
        current_pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000003").expect("valid thread id");
        runtime
            .upsert_thread(&test_thread_metadata(
                runtime.codex_home(),
                thread_id,
                runtime.codex_home().join("workspace"),
            ))
            .await
            .expect("thread should upsert");
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(crate::WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: codex_prompts::DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");
        let run = runtime
            .workflows()
            .create_workflow_run(crate::WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: Some("projection-migration-run".to_string()),
            })
            .await
            .expect("workflow run should create");
        let projection = runtime
            .project_workflow_run_to_goal_plan(crate::WorkflowGoalPlanProjectionParams {
                workflow_run_id: run.run.run_id.clone(),
                thread_id,
                idempotency_key: Some("projection-migration".to_string()),
            })
            .await
            .expect("workflow projection should run")
            .expect("workflow should project");

        assert_eq!(run.run.run_id, projection.run_id);
        assert_eq!(thread_id, projection.thread_id);
        assert_eq!(run.steps.len(), projection.snapshot.nodes.len());
        let goals_query_pool = open_db_pool(goals_path.as_path()).await;
        let projection_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_goal_plan_projections")
                .fetch_one(&goals_query_pool)
                .await
                .expect("projection count should query");
        assert_eq!(1, projection_count);
        let node_projection_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_goal_plan_node_projections")
                .fetch_one(&goals_query_pool)
                .await
                .expect("node projection count should query");
        assert_eq!(
            projection.snapshot.nodes.len() as i64,
            node_projection_count
        );
        goals_query_pool.close().await;

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn thread_goal_cancellation_migration_upgrades_pre_0004_goals_db() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let goals_path = goals_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&goals_path)
                .create_if_missing(true),
        )
        .await
        .expect("open goals db");
        migrator_through(&GOALS_MIGRATOR, /*version*/ 3)
            .run(&pool)
            .await
            .expect("apply pre-cancellation goals schema");
        sqlx::query(
            r#"
INSERT INTO thread_goals (
    thread_id,
    goal_id,
    objective,
    status,
    created_at_ms,
    updated_at_ms
) VALUES ('thread-1', 'goal-1', 'Cancel this goal.', 'active', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy goal should insert");
        sqlx::query(
            r#"
INSERT INTO thread_goal_plans (
    plan_id,
    thread_id,
    status,
    auto_execute,
    created_at_ms,
    updated_at_ms
) VALUES ('plan-1', 'thread-1', 'active', 'ready_only', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy plan should insert");
        sqlx::query(
            r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    created_at_ms,
    updated_at_ms
) VALUES ('node-1', 'plan-1', 'thread-1', 'cancel', 0, 0, 'Cancel this plan node.', 'active', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy plan node should insert");
        pool.close().await;

        let current_pool = super::open_goals_sqlite(
            goals_path.as_path(),
            &runtime_goals_migrator(),
            /*telemetry_override*/ None,
        )
        .await
        .expect("current goals migration should apply");
        sqlx::query("UPDATE thread_goals SET status = 'cancelled'")
            .execute(&current_pool)
            .await
            .expect("goal cancellation status should be accepted");
        sqlx::query("UPDATE thread_goal_plans SET status = 'cancelled'")
            .execute(&current_pool)
            .await
            .expect("plan cancellation status should be accepted");
        sqlx::query("UPDATE thread_goal_plan_nodes SET status = 'cancelled'")
            .execute(&current_pool)
            .await
            .expect("plan node cancellation status should be accepted");
        let statuses: (String, String, String, String) = sqlx::query_as(
            r#"
SELECT
    goal.status,
    plan.status,
    node.status,
    node.assigned_thread_id
FROM thread_goals goal
JOIN thread_goal_plans plan ON plan.thread_id = goal.thread_id
JOIN thread_goal_plan_nodes node ON node.plan_id = plan.plan_id
            "#,
        )
        .fetch_one(&current_pool)
        .await
        .expect("cancelled statuses should query");
        assert_eq!(
            (
                "cancelled".to_string(),
                "cancelled".to_string(),
                "cancelled".to_string(),
                "thread-1".to_string()
            ),
            statuses
        );
        current_pool.close().await;

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    /// Goals migration `0005_thread_goal_deferred.sql` exactly as shipped in
    /// published codewith 0.1.48 (tag `rust-v0.1.48`). Kept outside
    /// `goals_migrations/` so the embedded migrator never picks it up.
    const LEGACY_0148_GOALS_DEFERRED_V5_SQL: &str =
        include_str!("runtime/fixtures/goals_0148_0005_thread_goal_deferred.sql");

    fn legacy_0148_goals_deferred_migration() -> Migration {
        Migration::new(
            5,
            Cow::Borrowed("thread goal deferred"),
            MigrationType::Simple,
            LEGACY_0148_GOALS_DEFERRED_V5_SQL.into_sql_str(),
            /*no_tx*/ true,
        )
    }

    #[test]
    fn legacy_0148_goals_deferred_checksum_matches_sqlx_checksum() {
        let expected = decode_hex_checksum(LEGACY_0148_GOALS_DEFERRED_V5_CHECKSUM_HEX)
            .expect("legacy checksum hex should decode");
        assert_eq!(
            expected.as_slice(),
            legacy_0148_goals_deferred_migration().checksum.as_ref(),
            "hardcoded legacy checksum must match sqlx's checksum of the archived 0.1.48 migration bytes"
        );
    }

    #[tokio::test]
    async fn goals_db_stamped_by_codewith_0148_repairs_and_migrates() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let goals_path = goals_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&goals_path)
                .create_if_missing(true),
        )
        .await
        .expect("open goals db");
        // Recreate a goals database exactly as published codewith 0.1.48
        // left it: shared versions 1-4 plus the deferred table rebuild
        // stamped as version 5.
        migrator_through(&GOALS_MIGRATOR, /*version*/ 4)
            .run(&pool)
            .await
            .expect("apply goals schema through version 4");
        sqlx::query(
            r#"
INSERT INTO thread_goals (
    thread_id,
    goal_id,
    objective,
    status,
    created_at_ms,
    updated_at_ms
) VALUES ('thread-1', 'goal-1', 'Survive the 0.1.48 upgrade.', 'active', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy goal should insert");
        sqlx::query(
            r#"
INSERT INTO thread_goal_plans (
    plan_id,
    thread_id,
    status,
    auto_execute,
    created_at_ms,
    updated_at_ms
) VALUES ('plan-1', 'thread-1', 'active', 'ready_only', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy plan should insert");
        sqlx::query(
            r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    created_at_ms,
    updated_at_ms
) VALUES ('node-1', 'plan-1', 'thread-1', 'step', 0, 0, 'Do the step.', 'pending', 1, 1)
            "#,
        )
        .execute(&pool)
        .await
        .expect("legacy plan node should insert");
        let legacy_migrator = Migrator {
            migrations: Cow::Owned(vec![legacy_0148_goals_deferred_migration()]),
            ignore_missing: true,
            locking: true,
            no_tx: false,
            table_name: GOALS_MIGRATOR.table_name.clone(),
            create_schemas: GOALS_MIGRATOR.create_schemas.clone(),
        };
        legacy_migrator
            .run(&pool)
            .await
            .expect("apply the 0.1.48 deferred migration as version 5");
        // The legacy rebuild introduced the deferred status.
        sqlx::query("UPDATE thread_goals SET status = 'deferred' WHERE thread_id = 'thread-1'")
            .execute(&pool)
            .await
            .expect("legacy schema should accept deferred status");
        let stamped: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .expect("legacy stamps should query");
        assert_eq!(
            vec![1, 2, 3, 4, 5],
            stamped
                .iter()
                .map(|(version, _)| *version)
                .collect::<Vec<_>>()
        );
        let legacy_checksum = decode_hex_checksum(LEGACY_0148_GOALS_DEFERRED_V5_CHECKSUM_HEX)
            .expect("legacy checksum hex should decode");
        assert_eq!(
            legacy_checksum,
            stamped.last().expect("version 5 stamp").1,
            "version 5 must be stamped with the published 0.1.48 checksum"
        );
        pool.close().await;

        // Without the repair, the current migrator rejects the 0.1.48 stamp.
        let strict_pool = open_db_pool(goals_path.as_path()).await;
        let unrepaired_err = runtime_goals_migrator()
            .run(&strict_pool)
            .await
            .expect_err("current migrator must reject the unrepaired 0.1.48 stamp");
        assert!(matches!(unrepaired_err, MigrateError::VersionMismatch(5)));
        strict_pool.close().await;

        // Full runtime init repairs the stamp and applies the remaining
        // migrations on top of the legacy schema.
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state runtime should initialize on a 0.1.48-stamped goals db");
        runtime.pool.close().await;
        runtime.logs_pool.close().await;
        drop(runtime);

        let query_pool = open_db_pool(goals_path.as_path()).await;
        let stamped: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&query_pool)
                .await
                .expect("repaired stamps should query");
        assert_eq!(
            (1..=9).collect::<Vec<i64>>(),
            stamped
                .iter()
                .map(|(version, _)| *version)
                .collect::<Vec<_>>()
        );
        let embedded_deferred_checksum = GOALS_MIGRATOR
            .iter()
            .find(|migration| migration.version == GOALS_DEFERRED_MIGRATION_VERSION)
            .expect("embedded deferred migration")
            .checksum
            .to_vec();
        assert_eq!(
            embedded_deferred_checksum,
            stamped
                .iter()
                .find(|(version, _)| *version == 8)
                .expect("version 8 stamp")
                .1,
            "version 8 must carry the embedded deferred checksum after repair"
        );
        // The repaired database converges on the fresh schema: assignment
        // backfill, title columns, lifecycle table, and all four plan-node
        // indexes.
        let (status, assigned_thread_id, goal_title): (String, String, Option<String>) =
            sqlx::query_as(
                r#"
SELECT goal.status, node.assigned_thread_id, goal.title
FROM thread_goals goal
JOIN thread_goal_plan_nodes node ON node.thread_id = goal.thread_id
                "#,
            )
            .fetch_one(&query_pool)
            .await
            .expect("repaired schema should join goals and nodes");
        assert_eq!("deferred", status);
        assert_eq!("thread-1", assigned_thread_id);
        assert_eq!(None, goal_title);
        let lifecycle_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_goal_context_lifecycle")
                .fetch_one(&query_pool)
                .await
                .expect("goal context lifecycle table should exist");
        assert_eq!(0, lifecycle_count);
        let blocker_audit_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_goal_blocker_audits")
                .fetch_one(&query_pool)
                .await
                .expect("goal blocker audit table should exist");
        assert_eq!(0, blocker_audit_count);
        let blocker_audit_turn_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_goal_blocker_audit_turns")
                .fetch_one(&query_pool)
                .await
                .expect("goal blocker audit turn table should exist");
        assert_eq!(0, blocker_audit_turn_count);
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'thread_goal_plan_nodes' AND name LIKE 'idx_%' ORDER BY name",
        )
        .fetch_all(&query_pool)
        .await
        .expect("plan node indexes should query");
        assert_eq!(
            vec![
                "idx_thread_goal_plan_nodes_assigned_status".to_string(),
                "idx_thread_goal_plan_nodes_plan_status".to_string(),
                "idx_thread_goal_plan_nodes_projected_goal".to_string(),
                "idx_thread_goal_plan_nodes_thread_status".to_string(),
            ],
            indexes
        );
        // Idempotence: a second init sees nothing left to repair.
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state runtime should initialize again after the repair");
        runtime.pool.close().await;
        runtime.logs_pool.close().await;
        drop(runtime);
        query_pool.close().await;

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn init_records_successful_sqlite_init_phases_to_explicit_telemetry() {
        let codex_home = unique_temp_dir();
        let telemetry = TestTelemetry::default();

        let runtime = StateRuntime::init_with_telemetry_for_tests(
            codex_home.clone(),
            "test-provider".to_string(),
            &telemetry,
        )
        .await
        .expect("state runtime should initialize");

        let phases = telemetry
            .counters()
            .into_iter()
            .filter(|event| event.name == DB_INIT_METRIC)
            .filter(|event| event.tags.get("status").map(String::as_str) == Some("success"))
            .filter_map(|event| event.tags.get("phase").cloned())
            .collect::<BTreeSet<_>>();
        let expected = [
            "open_state",
            "migrate_state",
            "open_logs",
            "migrate_logs",
            "open_goals",
            "migrate_goals",
            "open_memories",
            "migrate_memories",
            "open_state_reader",
            "open_logs_reader",
            "ensure_backfill_state",
            "post_init_query",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
        assert_eq!(phases, expected);

        runtime.pool.close().await;
        runtime.logs_pool.close().await;
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn wal_checkpoint_maintenance_checkpoints_all_runtime_pools() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        // Generate WAL frames on the logs pool -- the hottest write target,
        // which previously was not covered by periodic checkpoint maintenance.
        runtime
            .insert_logs(&[crate::LogEntry {
                ts: 1,
                ts_nanos: 0,
                level: "INFO".to_string(),
                target: "cli".to_string(),
                message: Some("checkpoint-coverage".to_string()),
                feedback_log_body: Some("checkpoint-coverage".to_string()),
                thread_id: Some("thread-ckpt".to_string()),
                process_uuid: Some("proc-ckpt".to_string()),
                module_path: Some("mod".to_string()),
                file: Some("main.rs".to_string()),
                line: Some(1),
            }])
            .await
            .expect("insert log");

        // Maintenance must succeed across every runtime pool, not just state.
        runtime
            .run_state_wal_checkpoint_maintenance()
            .await
            .expect("checkpoint maintenance across all pools");

        // All four runtime databases are opened and reachable by maintenance.
        for path in [
            state_db_path(codex_home.as_path()),
            logs_db_path(codex_home.as_path()),
            goals_db_path(codex_home.as_path()),
            memories_db_path(codex_home.as_path()),
        ] {
            assert!(path.exists(), "expected db file at {}", path.display());
        }

        // The inserted log survives the checkpoint (frames flushed into the DB).
        let rows = runtime
            .query_logs(&crate::LogQuery::default())
            .await
            .expect("query logs after checkpoint");
        assert_eq!(rows.len(), 1);

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }
}
