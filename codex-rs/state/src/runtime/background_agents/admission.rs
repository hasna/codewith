use super::runs::is_background_agent_unique_constraint_violation;
use super::snapshots::create_background_agent_execution_snapshot_in_tx;
use super::snapshots::get_background_agent_status_snapshot_in_tx;
use super::snapshots::get_latest_background_agent_execution_snapshot_in_tx;
use super::snapshots::upsert_background_agent_status_snapshot_in_tx;
use super::*;
use crate::runtime::managed_worktrees::path_to_db_string;
use std::path::Path;
use uuid::Uuid;

type ManagedWorktreeOwnerRow = (String, String, Option<i64>, Option<String>, Option<String>);

/// The durable records created when a managed worktree admits a background run.
///
/// All records are committed together with the worktree assignment. A rejected
/// admission therefore leaves no run, snapshot, event, status snapshot, or
/// assignment behind.
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
    pub worktree_id: String,
    pub execution_snapshot: BackgroundAgentExecutionSnapshotParams,
    pub started_event_payload_json: Value,
}

impl StateRuntime {
    /// Atomically reserves a managed worktree and creates the initial durable
    /// background-agent records. The transaction is the admission boundary for
    /// competing `agent/start` requests.
    pub async fn admit_background_agent_run(
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
        claim_managed_worktree_for_background_agent_start_in_tx(
            tx,
            params.worktree_id.as_str(),
            run.id.as_str(),
            now_ms,
        )
        .await?;
    } else if !should_restore_idempotent_managed_worktree_assignment_in_tx(
        tx,
        params.worktree_id.as_str(),
        run.id.as_str(),
        &params.execution_snapshot.payload_json,
    )
    .await?
    {
        anyhow::bail!(
            "agent/start idempotency key is already associated with a different managed worktree"
        );
    } else {
        claim_managed_worktree_for_background_agent_start_in_tx(
            tx,
            params.worktree_id.as_str(),
            run.id.as_str(),
            now_ms,
        )
        .await?;
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
    let status_snapshot =
        match get_background_agent_status_snapshot_in_tx(tx, run.id.as_str()).await? {
            Some(snapshot) => snapshot,
            None => {
                let status_params = BackgroundAgentStatusSnapshotParams {
                    run_id: run.id.clone(),
                    seq: event.seq,
                    status: run.status,
                    desired_state: run.desired_state,
                    summary: Some("Queued".to_string()),
                    pending_interaction_count: 0,
                    last_event_seq: event.seq,
                    payload_json: serde_json::json!({"phase": "queued"}),
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

async fn claim_managed_worktree_for_background_agent_start_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    worktree_id: &str,
    run_id: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    let owner: Option<ManagedWorktreeOwnerRow> = sqlx::query_as(
        "SELECT mode, lifecycle_status, deleted_at_ms, owner_thread_id, owner_agent_run_id \
             FROM managed_worktrees WHERE worktree_id = ?",
    )
    .bind(worktree_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((mode, lifecycle_status, deleted_at_ms, owner_thread_id, owner_agent_run_id)) = owner
    else {
        anyhow::bail!("managed worktree {worktree_id} does not exist");
    };
    if mode != crate::ManagedWorktreeMode::IsolatedWorktree.as_str() {
        anyhow::bail!("agent/start worktree cwd requires an isolated managed worktree");
    }
    if lifecycle_status != crate::ManagedWorktreeLifecycleStatus::Active.as_str()
        || deleted_at_ms.is_some()
    {
        anyhow::bail!("agent/start worktree cwd requires an active managed worktree");
    }
    if owner_thread_id.is_some() {
        anyhow::bail!("agent/start worktree cwd is already assigned to a thread");
    }
    if let Some(owner_agent_run_id) = owner_agent_run_id.as_deref()
        && owner_agent_run_id != run_id
    {
        anyhow::bail!(
            "agent/start worktree cwd is already assigned to background agent run {owner_agent_run_id}"
        );
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
                anyhow::bail!(
                    "managed worktree {worktree_id} is already assigned by {assignment_id}"
                );
            }
            None => anyhow::bail!("managed worktree {worktree_id} could not be assigned"),
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
        anyhow::bail!("managed worktree {worktree_id} could not be reserved");
    }
    Ok(())
}

async fn should_restore_idempotent_managed_worktree_assignment_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    worktree_id: &str,
    run_id: &str,
    execution_payload_json: &Value,
) -> anyhow::Result<bool> {
    let active_worktree_id: Option<String> = sqlx::query_scalar(
        "SELECT worktree_id FROM managed_worktree_assignments \
         WHERE agent_run_id = ? AND detached_at_ms IS NULL LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(active_worktree_id) = active_worktree_id {
        return Ok(active_worktree_id == worktree_id);
    }

    let managed_worktree_path: Option<String> = sqlx::query_scalar(
        "SELECT worktree_path FROM managed_worktrees WHERE worktree_id = ? \
         AND mode = 'isolated_worktree' AND lifecycle_status = 'active' \
         AND deleted_at_ms IS NULL",
    )
    .bind(worktree_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(managed_worktree_path) = managed_worktree_path else {
        return Ok(false);
    };
    let Some(snapshot_cwd) = execution_payload_json.get("cwd").and_then(Value::as_str) else {
        return Ok(false);
    };
    Ok(path_to_db_string(Path::new(snapshot_cwd)) == managed_worktree_path)
}
