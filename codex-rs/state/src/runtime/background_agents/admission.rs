use super::runs::is_background_agent_unique_constraint_violation;
use super::snapshots::create_background_agent_execution_snapshot_in_tx;
use super::snapshots::get_background_agent_status_snapshot_in_tx;
use super::snapshots::get_latest_background_agent_execution_snapshot_in_tx;
use super::snapshots::upsert_background_agent_status_snapshot_in_tx;
use super::*;
use crate::runtime::managed_worktrees::managed_worktree_path_key_from_display;
use crate::runtime::managed_worktrees::path_to_db_string;
use std::fmt;
use uuid::Uuid;

type ManagedWorktreeOwnerRow = (String, String, Option<i64>, Option<String>, Option<String>);

/// The durable records created when a background run is admitted.
///
/// All records are committed together with the optional worktree assignment.
/// A rejected admission therefore leaves no run, snapshot, event, status
/// snapshot, or assignment behind.
#[derive(Debug, Clone)]
pub struct BackgroundAgentRunAdmission {
    pub run: BackgroundAgentRun,
    pub execution_snapshot: BackgroundAgentExecutionSnapshot,
    pub event: BackgroundAgentEvent,
    pub status_snapshot: BackgroundAgentStatusSnapshot,
    pub created_new_run: bool,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentRunAdmissionParams {
    pub run: BackgroundAgentRunCreateParams,
    pub worktree_id: Option<String>,
    pub max_active_runs: i64,
    pub execution_snapshot: BackgroundAgentExecutionSnapshotParams,
    pub started_event_payload_json: Value,
}

/// Expected client-visible rejections from the atomic admission boundary.
///
/// Callers should map these to request/conflict responses rather than treating
/// them as state-store faults.
#[derive(Debug)]
pub enum BackgroundAgentAdmissionError {
    IdempotencyConflict {
        message: String,
    },
    QuotaExceeded {
        active_run_count: i64,
        max_active_runs: i64,
    },
    WorktreeConflict {
        message: String,
    },
}

impl fmt::Display for BackgroundAgentAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdempotencyConflict { message } | Self::WorktreeConflict { message } => {
                formatter.write_str(message)
            }
            Self::QuotaExceeded {
                active_run_count,
                max_active_runs,
            } => write!(
                formatter,
                "background agent queue is overloaded: {active_run_count} active run(s), max {max_active_runs}"
            ),
        }
    }
}

impl std::error::Error for BackgroundAgentAdmissionError {}

impl StateRuntime {
    /// Atomically reserves a managed worktree and creates the initial durable
    /// background-agent records. The transaction is the admission boundary for
    /// competing `agent/start` requests.
    pub async fn admit_background_agent_run(
        &self,
        params: &BackgroundAgentRunAdmissionParams,
    ) -> anyhow::Result<BackgroundAgentRunAdmission> {
        crate::busy_retry::retry_on_busy("admit background agent run", || {
            self.admit_background_agent_run_once(params)
        })
        .await
    }

    async fn admit_background_agent_run_once(
        &self,
        params: &BackgroundAgentRunAdmissionParams,
    ) -> anyhow::Result<BackgroundAgentRunAdmission> {
        let now = Utc::now().timestamp();
        let now_ms = now * 1000;
        let mut tx = self.pool.begin().await?;
        let admission = admit_background_agent_run_in_tx(&mut tx, params, now, now_ms).await?;
        tx.commit().await?;
        Ok(admission)
    }
}

async fn admit_background_agent_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &BackgroundAgentRunAdmissionParams,
    now: i64,
    now_ms: i64,
) -> anyhow::Result<BackgroundAgentRunAdmission> {
    let (run, created_new_run) = create_background_agent_run_in_tx(tx, &params.run, now).await?;
    if created_new_run {
        let active_run_count = active_background_agent_run_count_in_tx(tx).await?;
        if active_run_count > params.max_active_runs {
            return Err(BackgroundAgentAdmissionError::QuotaExceeded {
                active_run_count: active_run_count.saturating_sub(1),
                max_active_runs: params.max_active_runs,
            }
            .into());
        }
    }

    match params.worktree_id.as_deref() {
        Some(worktree_id) if created_new_run => {
            claim_managed_worktree_for_background_agent_start_in_tx(tx, worktree_id, &run, now_ms)
                .await?;
        }
        Some(worktree_id) => {
            if !should_restore_idempotent_managed_worktree_assignment_in_tx(
                tx,
                worktree_id,
                run.id.as_str(),
            )
            .await?
            {
                return Err(BackgroundAgentAdmissionError::IdempotencyConflict {
                    message: "agent/start idempotency key is already associated with a different managed worktree".to_string(),
                }
                .into());
            }
            if !is_terminal_background_agent_run_status(run.status) {
                claim_managed_worktree_for_background_agent_start_in_tx(
                    tx,
                    worktree_id,
                    &run,
                    now_ms,
                )
                .await?;
            }
        }
        None if !created_new_run
            && background_agent_run_has_managed_worktree_assignment_in_tx(tx, run.id.as_str())
                .await? =>
        {
            return Err(BackgroundAgentAdmissionError::IdempotencyConflict {
                message:
                    "agent/start idempotency key is already associated with a managed worktree"
                        .to_string(),
            }
            .into());
        }
        None => {}
    }

    let mut execution_snapshot_params = params.execution_snapshot.clone();
    execution_snapshot_params.run_id = run.id.clone();
    execution_snapshot_params.config_fingerprint = run.config_fingerprint.clone();
    let execution_snapshot =
        match get_latest_background_agent_execution_snapshot_in_tx(tx, run.id.as_str()).await? {
            Some(snapshot) => snapshot,
            None => {
                create_background_agent_execution_snapshot_in_tx(
                    tx,
                    &execution_snapshot_params,
                    now,
                )
                .await?
            }
        };
    let event = match first_background_agent_event_in_tx(tx, run.id.as_str()).await? {
        Some(event) => event,
        None if created_new_run => {
            append_background_agent_event_in_tx(
                tx,
                run.id.as_str(),
                "agent.started",
                &params.started_event_payload_json,
                now,
            )
            .await?
        }
        None => {
            append_background_agent_event_in_tx(
                tx,
                run.id.as_str(),
                "agent.startRecovered",
                &serde_json::json!({"reason": "idempotent_start_without_start_event"}),
                now,
            )
            .await?
        }
    };
    let run = background_agent_run_by_id_in_tx(tx, run.id.as_str())
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "background agent run {} disappeared during admission",
                run.id
            )
        })?;
    let current_event = latest_background_agent_event_in_tx(tx, run.id.as_str())
        .await?
        .unwrap_or_else(|| event.clone());
    let status_snapshot = get_background_agent_status_snapshot_in_tx(tx, run.id.as_str()).await?;
    let snapshot_is_current = status_snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.status == run.status
            && snapshot.desired_state == run.desired_state
            && snapshot.seq == current_event.seq
            && snapshot.last_event_seq == run.last_event_seq
            && current_event.seq == run.last_event_seq
    });
    let status_snapshot = if let Some(snapshot) = status_snapshot.filter(|_| snapshot_is_current) {
        snapshot
    } else {
        let pending_interaction_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM background_agent_pending_interactions
WHERE run_id = ? AND status IN (?, ?)
            "#,
        )
        .bind(run.id.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .fetch_one(&mut **tx)
        .await?;
        let (summary, payload_json) = if created_new_run {
            ("Queued".to_string(), serde_json::json!({"phase": "queued"}))
        } else {
            (
                format!("{:?}", run.status),
                serde_json::json!({
                    "phase": run.status.as_str(),
                    "recovered": true,
                }),
            )
        };
        let status_params = BackgroundAgentStatusSnapshotParams {
            run_id: run.id.clone(),
            seq: current_event.seq,
            status: run.status,
            desired_state: run.desired_state,
            summary: Some(summary),
            pending_interaction_count,
            last_event_seq: current_event.seq,
            payload_json,
        };
        upsert_background_agent_status_snapshot_in_tx(tx, &status_params, now).await?;
        get_background_agent_status_snapshot_in_tx(tx, run.id.as_str())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load background agent status snapshot for run {}",
                    run.id
                )
            })?
    };
    Ok(BackgroundAgentRunAdmission {
        run,
        execution_snapshot,
        event,
        status_snapshot,
        created_new_run,
    })
}

async fn create_background_agent_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &BackgroundAgentRunCreateParams,
    now: i64,
) -> anyhow::Result<(BackgroundAgentRun, bool)> {
    if let Some(idempotency_key) = params.idempotency_key.as_deref()
        && let Some(existing) =
            background_agent_run_by_idempotency_key_in_tx(tx, idempotency_key).await?
    {
        return Ok((existing, false));
    }

    let spawn_linkage_json = params
        .spawn_linkage_json
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let insert_result = sqlx::query(
        r#"
INSERT INTO background_agent_runs (
    id, idempotency_key, request_id, source, prompt_snapshot_ref,
    input_snapshot_ref, thread_id, thread_store_kind, thread_store_id,
    rollout_path, parent_thread_id, parent_agent_run_id, spawn_linkage_json,
    auth_profile_ref, desired_state, status, status_reason, config_fingerprint,
    version_fingerprint, retention_state, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(params.id.as_str())
    .bind(params.idempotency_key.as_deref())
    .bind(params.request_id.as_deref())
    .bind(params.source.as_str())
    .bind(params.prompt_snapshot_ref.as_str())
    .bind(params.input_snapshot_ref.as_deref())
    .bind(params.thread_id.as_deref())
    .bind(params.thread_store_kind.as_str())
    .bind(params.thread_store_id.as_deref())
    .bind(params.rollout_path.as_deref())
    .bind(params.parent_thread_id.as_deref())
    .bind(params.parent_agent_run_id.as_deref())
    .bind(spawn_linkage_json.as_deref())
    .bind(params.auth_profile_ref.as_deref())
    .bind(BackgroundAgentDesiredState::Running.as_str())
    .bind(BackgroundAgentRunStatus::Queued.as_str())
    .bind(params.status_reason.as_deref())
    .bind(params.config_fingerprint.as_deref())
    .bind(params.version_fingerprint.as_deref())
    .bind(crate::BackgroundAgentRetentionState::Active.as_str())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await;
    if let Err(err) = insert_result {
        if params.idempotency_key.is_some()
            && is_background_agent_unique_constraint_violation(&err)
            && let Some(idempotency_key) = params.idempotency_key.as_deref()
            && let Some(existing) =
                background_agent_run_by_idempotency_key_in_tx(tx, idempotency_key).await?
        {
            return Ok((existing, false));
        }
        return Err(err.into());
    }

    let run = background_agent_run_by_id_in_tx(tx, params.id.as_str())
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to load background agent run {}", params.id))?;
    Ok((run, true))
}

async fn background_agent_run_by_id_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<BackgroundAgentRun>> {
    background_agent_run_by_field_in_tx(tx, "id", run_id).await
}

async fn background_agent_run_by_idempotency_key_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    idempotency_key: &str,
) -> anyhow::Result<Option<BackgroundAgentRun>> {
    background_agent_run_by_field_in_tx(tx, "idempotency_key", idempotency_key).await
}

async fn background_agent_run_by_field_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    field: &'static str,
    value: &str,
) -> anyhow::Result<Option<BackgroundAgentRun>> {
    let query = format!(
        "SELECT id, idempotency_key, request_id, source, prompt_snapshot_ref, input_snapshot_ref, \
         thread_id, thread_store_kind, thread_store_id, rollout_path, parent_thread_id, \
         parent_agent_run_id, spawn_linkage_json, worktree_lease_id, auth_profile_ref, \
         desired_state, status, status_reason, config_fingerprint, version_fingerprint, \
         retention_state, archive_after, delete_after, archived_at, deleted_at, supervisor_id, \
         generation, pid, pgid, job_id, heartbeat_at, crash_reason, exit_code, exit_signal, \
         last_event_seq, last_snapshot_seq, created_at, updated_at, started_at, completed_at \
         FROM background_agent_runs WHERE {field} = ?"
    );
    let row = sqlx::query_as::<_, BackgroundAgentRunRow>(sqlx::AssertSqlSafe(query))
        .bind(value)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(BackgroundAgentRun::try_from).transpose()
}

async fn first_background_agent_event_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<BackgroundAgentEvent>> {
    let row = sqlx::query_as::<_, BackgroundAgentEventRow>(
        "SELECT id, run_id, seq, event_type, payload_json, created_at \
         FROM background_agent_events WHERE run_id = ? ORDER BY seq ASC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(BackgroundAgentEvent::try_from).transpose()
}

async fn latest_background_agent_event_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<BackgroundAgentEvent>> {
    let row = sqlx::query_as::<_, BackgroundAgentEventRow>(
        "SELECT id, run_id, seq, event_type, payload_json, created_at \
         FROM background_agent_events WHERE run_id = ? ORDER BY seq DESC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(BackgroundAgentEvent::try_from).transpose()
}

async fn active_background_agent_run_count_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM background_agent_runs \
         WHERE status NOT IN ('completed', 'failed', 'cancelled')",
    )
    .fetch_one(&mut **tx)
    .await?)
}

async fn background_agent_run_has_managed_worktree_assignment_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<bool> {
    let has_assignment: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM managed_worktree_assignments WHERE agent_run_id = ?)",
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    if has_assignment != 0 {
        return Ok(true);
    }
    let Some(initial_snapshot_cwd) =
        initial_background_agent_snapshot_cwd_in_tx(tx, run_id).await?
    else {
        return Ok(false);
    };
    let initial_snapshot_path_key = managed_worktree_path_key_from_display(
        path_to_db_string(std::path::Path::new(initial_snapshot_cwd.as_str())).as_str(),
    );
    let matching_worktree_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM managed_worktrees WHERE worktree_path_key = ?")
            .bind(initial_snapshot_path_key)
            .fetch_one(&mut **tx)
            .await?;
    Ok(matching_worktree_count != 0)
}

async fn claim_managed_worktree_for_background_agent_start_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    worktree_id: &str,
    run: &BackgroundAgentRun,
    now_ms: i64,
) -> anyhow::Result<()> {
    if is_terminal_background_agent_run_status(run.status) {
        return Err(BackgroundAgentAdmissionError::IdempotencyConflict {
            message: format!(
                "agent/start cannot assign managed worktree to terminal background agent run {}",
                run.id
            ),
        }
        .into());
    }
    let run_id = run.id.as_str();
    let owner: Option<ManagedWorktreeOwnerRow> = sqlx::query_as(
        "SELECT mode, lifecycle_status, deleted_at_ms, owner_thread_id, owner_agent_run_id \
             FROM managed_worktrees WHERE worktree_id = ?",
    )
    .bind(worktree_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((mode, lifecycle_status, deleted_at_ms, owner_thread_id, owner_agent_run_id)) = owner
    else {
        return Err(BackgroundAgentAdmissionError::WorktreeConflict {
            message: format!("managed worktree {worktree_id} does not exist"),
        }
        .into());
    };
    if mode != crate::ManagedWorktreeMode::IsolatedWorktree.as_str() {
        return Err(BackgroundAgentAdmissionError::WorktreeConflict {
            message: "agent/start worktree cwd requires an isolated managed worktree".to_string(),
        }
        .into());
    }
    if lifecycle_status != crate::ManagedWorktreeLifecycleStatus::Active.as_str()
        || deleted_at_ms.is_some()
    {
        return Err(BackgroundAgentAdmissionError::WorktreeConflict {
            message: "agent/start worktree cwd requires an active managed worktree".to_string(),
        }
        .into());
    }
    if owner_thread_id.is_some() {
        return Err(BackgroundAgentAdmissionError::WorktreeConflict {
            message: "agent/start worktree cwd is already assigned to a thread".to_string(),
        }
        .into());
    }
    if let Some(owner_agent_run_id) = owner_agent_run_id.as_deref()
        && owner_agent_run_id != run_id
    {
        return Err(BackgroundAgentAdmissionError::WorktreeConflict {
            message: format!(
                "agent/start worktree cwd is already assigned to background agent run {owner_agent_run_id}"
            ),
        }
        .into());
    }

    let inserted = sqlx::query(
        r#"
INSERT INTO managed_worktree_assignments (
    assignment_id, worktree_id, thread_id, agent_run_id, attached_at_ms, detached_at_ms
) VALUES (?, ?, NULL, ?, ?, NULL)
ON CONFLICT(worktree_id) WHERE detached_at_ms IS NULL DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(worktree_id)
    .bind(run_id)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let assignment: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT assignment_id, agent_run_id FROM managed_worktree_assignments \
             WHERE worktree_id = ? AND detached_at_ms IS NULL LIMIT 1",
        )
        .bind(worktree_id)
        .fetch_optional(&mut **tx)
        .await?;
        match assignment {
            Some((_, agent_run_id)) if agent_run_id.as_deref() == Some(run_id) => {}
            Some((assignment_id, _)) => {
                return Err(BackgroundAgentAdmissionError::WorktreeConflict {
                    message: format!(
                        "managed worktree {worktree_id} is already assigned by {assignment_id}"
                    ),
                }
                .into());
            }
            None => {
                return Err(BackgroundAgentAdmissionError::WorktreeConflict {
                    message: format!("managed worktree {worktree_id} could not be assigned"),
                }
                .into());
            }
        }
    }

    let owner_update = sqlx::query(
        r#"
UPDATE managed_worktrees
SET owner_kind = ?, owner_thread_id = NULL, owner_agent_run_id = ?, updated_at_ms = ?
WHERE worktree_id = ? AND lifecycle_status = 'active' AND deleted_at_ms IS NULL
        "#,
    )
    .bind(crate::ManagedWorktreeOwnerKind::BackgroundAgent.as_str())
    .bind(run_id)
    .bind(now_ms)
    .bind(worktree_id)
    .execute(&mut **tx)
    .await?;
    if owner_update.rows_affected() != 1 {
        return Err(BackgroundAgentAdmissionError::WorktreeConflict {
            message: format!("managed worktree {worktree_id} could not be reserved"),
        }
        .into());
    }
    Ok(())
}

async fn should_restore_idempotent_managed_worktree_assignment_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    worktree_id: &str,
    run_id: &str,
) -> anyhow::Result<bool> {
    let admitted_worktree_id: Option<String> = sqlx::query_scalar(
        "SELECT worktree_id FROM managed_worktree_assignments \
         WHERE agent_run_id = ? ORDER BY attached_at_ms ASC, assignment_id ASC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(admitted_worktree_id) = admitted_worktree_id {
        return Ok(admitted_worktree_id == worktree_id);
    }

    let Some(initial_snapshot_cwd) =
        initial_background_agent_snapshot_cwd_in_tx(tx, run_id).await?
    else {
        return Ok(false);
    };
    let managed_worktree_path_key: Option<String> =
        sqlx::query_scalar("SELECT worktree_path_key FROM managed_worktrees WHERE worktree_id = ?")
            .bind(worktree_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(managed_worktree_path_key) = managed_worktree_path_key else {
        return Ok(false);
    };
    let initial_snapshot_path_key = managed_worktree_path_key_from_display(
        path_to_db_string(std::path::Path::new(initial_snapshot_cwd.as_str())).as_str(),
    );
    Ok(managed_worktree_path_key == initial_snapshot_path_key)
}

async fn initial_background_agent_snapshot_cwd_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<String>> {
    let initial_snapshot_payload: Option<String> = sqlx::query_scalar(
        "SELECT payload_json FROM background_agent_execution_snapshots \
         WHERE run_id = ? AND snapshot_kind = 'initial_execution_context' \
         ORDER BY seq ASC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(initial_snapshot_payload) = initial_snapshot_payload else {
        return Ok(None);
    };
    let initial_snapshot_payload: Value = serde_json::from_str(&initial_snapshot_payload)?;
    Ok(initial_snapshot_payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::to_string))
}

fn is_terminal_background_agent_run_status(status: BackgroundAgentRunStatus) -> bool {
    matches!(
        status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    )
}
