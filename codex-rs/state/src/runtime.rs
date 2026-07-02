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
mod workflow_automation;
mod workflow_goal_plan_projections;
mod workflow_orchestrator;
mod workflow_verifiers;
mod workflows;

const STATE_RUNTIME_STARTUP_LOCK_FILENAME: &str = ".state-runtime-startup.lock";
const STATE_RUNTIME_STARTUP_LOCK_TIMEOUT: Duration = Duration::from_secs(60);
const STATE_RUNTIME_STARTUP_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub use goal_plans::DEFAULT_THREAD_GOAL_PLAN_LIST_LIMIT;
pub use goal_plans::MAX_THREAD_GOAL_PLAN_LIST_LIMIT;
pub use goal_plans::ThreadGoalPlanAddOutcome;
pub use goal_plans::ThreadGoalPlanAddParams;
pub use goal_plans::ThreadGoalPlanAdvanceOutcome;
pub use goal_plans::ThreadGoalPlanCreateParams;
pub use goal_plans::ThreadGoalPlanListPage;
pub use goal_plans::ThreadGoalPlanNodeCreateParams;
pub use goals::GoalAccountingMode;
pub use goals::GoalAccountingOutcome;
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
pub use schedules::ScheduleStore;
pub use schedules::ThreadScheduleClaim;
pub use schedules::ThreadScheduleCreateParams;
pub use schedules::ThreadScheduleUpdate;
pub use threads::ThreadFilterOptions;
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
pub use workflows::WorkflowRunCancelParams;
pub use workflows::WorkflowRunCreateParams;
pub use workflows::WorkflowRunListPage;
pub use workflows::WorkflowRunPauseParams;
pub use workflows::WorkflowRunResumeParams;
pub use workflows::WorkflowRunStatusMutationOutcome;
pub use workflows::WorkflowSpecCreateParams;
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
    migrate_phase: "migrate_state",
};

const LOGS_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "log DB",
    filename: LOGS_DB_FILENAME,
    kind: DbKind::Logs,
    open_phase: "open_logs",
    migrate_phase: "migrate_logs",
};

const GOALS_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "goals DB",
    filename: GOALS_DB_FILENAME,
    kind: DbKind::Goals,
    open_phase: "open_goals",
    migrate_phase: "migrate_goals",
};

const MEMORIES_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "memories DB",
    filename: MEMORIES_DB_FILENAME,
    kind: DbKind::Memories,
    open_phase: "open_memories",
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
    pool: Arc<sqlx::SqlitePool>,
    logs_pool: Arc<sqlx::SqlitePool>,
    thread_goals: GoalStore,
    thread_schedules: ScheduleStore,
    thread_monitors: MonitorStore,
    local_active_sessions: LocalActiveSessionStore,
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
            machine_registry: MachineRegistryStore::new(Arc::clone(&pool)),
            mailbox_messages: MailboxMessageStore::new(Arc::clone(&pool)),
            managed_worktrees: ManagedWorktreeStore::new(Arc::clone(&pool)),
            workflows: WorkflowStore::new(Arc::clone(&pool)),
            workflow_automation: WorkflowAutomationStore::new(Arc::clone(&pool)),
            memories: MemoryStore::new(Arc::clone(&memories_pool), Arc::clone(&pool)),
            pool,
            logs_pool,
            codex_home,
            default_provider,
            thread_updated_at_millis: Arc::new(AtomicI64::new(thread_updated_at_millis)),
        });
        if let Err(err) = runtime.run_logs_startup_maintenance().await {
            warn!(
                "failed to run startup maintenance for logs db at {}: {err}",
                logs_path.display(),
            );
        }
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
}

fn base_sqlite_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30))
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
    let pool_result = SqlitePoolOptions::new()
        .max_connections(5)
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
    let migrate_result = migrator.run(&pool).await.map_err(anyhow::Error::from);
    crate::telemetry::record_init_result(
        telemetry_override,
        spec.kind,
        spec.migrate_phase,
        started.elapsed(),
        &migrate_result,
    );
    migrate_result?;
    Ok(pool)
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
    use super::StateRuntime;
    use super::goals_db_path;
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
    use sqlx::SqlitePool;
    use sqlx::migrate::MigrateError;
    use sqlx::migrate::Migrator;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Mutex;

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
}
