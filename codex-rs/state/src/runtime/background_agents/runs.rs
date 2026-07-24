use super::*;
use sha2::Digest;
use sha2::Sha256;

const BACKGROUND_AGENT_ADMISSION_CAPACITY_EXCEEDED: &str =
    "background_agent_admission_capacity_exceeded";
const BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH: &str =
    "background_agent_admission_identity_mismatch";
const BACKGROUND_AGENT_RUNTIME_INCOMPATIBLE_REASON: &str =
    "background agent runtime package is incompatible with the installed build";

/// One-way storage form for a caller-supplied idempotency key.
///
/// Idempotency keys are opaque, caller-controlled strings that can carry
/// credential-shaped material, so they are never persisted in a form that can
/// be reversed back into the original value. Dedupe and replay lookups compare
/// digests, which preserves byte-exact identity matching without keeping the
/// plaintext in the local state database.
pub(in crate::runtime) fn background_agent_idempotency_key_digest(value: &str) -> String {
    StateRuntime::background_agent_identity_sha256(value.as_bytes())
}

#[derive(sqlx::FromRow)]
struct BackgroundAgentSupervisorClaimState {
    generation: i64,
    desired_state: String,
    status: String,
    retention_state: String,
    admission_identity_sha256: Option<String>,
    admission_ready_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct BackgroundAgentDeleteState {
    supervisor_id: Option<String>,
    generation: i64,
    status: String,
    retention_state: String,
    status_reason: Option<String>,
}

pub(in crate::runtime) enum ExistingBackgroundAgentAdmissionIdentity<'a> {
    RunFields,
    AdmissionDigest(&'a str),
}

impl StateRuntime {
    /// Returns a stable digest for opaque background-agent admission identity.
    pub fn background_agent_identity_sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    pub async fn create_background_agent_run(
        &self,
        params: &BackgroundAgentRunCreateParams,
    ) -> anyhow::Result<BackgroundAgentRun> {
        self.create_background_agent_run_row(params)
            .await
            .map(|(run, _created)| run)
    }

    pub async fn admit_background_agent_run(
        &self,
        params: &BackgroundAgentRunCreateParams,
        start_event_payload_json: &serde_json::Value,
        execution_snapshot_params: &BackgroundAgentExecutionSnapshotParams,
        max_active_runs: i64,
    ) -> anyhow::Result<(
        BackgroundAgentRun,
        bool,
        BackgroundAgentEvent,
        BackgroundAgentExecutionSnapshot,
        BackgroundAgentStatusSnapshot,
    )> {
        let idempotency_key = params.idempotency_key.as_deref();
        let mut execution_snapshot_params = execution_snapshot_params.clone();
        execution_snapshot_params.run_id.clone_from(&params.id);
        let admission_identity_sha256 = background_agent_admission_identity_sha256(
            params,
            start_event_payload_json,
            &execution_snapshot_params,
        )?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(idempotency_key) = idempotency_key
            && let Some(existing_id) = validate_existing_background_agent_admission_in_tx(
                &mut tx,
                idempotency_key,
                params,
                ExistingBackgroundAgentAdmissionIdentity::AdmissionDigest(
                    admission_identity_sha256.as_str(),
                ),
            )
            .await?
        {
            let (event, execution_snapshot_id) =
                recover_or_validate_background_agent_initial_state_in_tx(
                    &mut tx,
                    existing_id.as_str(),
                    params,
                    admission_identity_sha256.as_str(),
                    start_event_payload_json,
                    &execution_snapshot_params,
                    max_active_runs,
                )
                .await?;
            tx.commit().await?;
            return self
                .load_background_agent_admission_result(
                    existing_id.as_str(),
                    /*created*/ false,
                    event,
                    execution_snapshot_id,
                )
                .await;
        }

        ensure_background_agent_capacity_in_tx(&mut tx, max_active_runs).await?;
        let now = Utc::now().timestamp();
        insert_background_agent_run_in_tx(&mut tx, params, now).await?;
        append_background_agent_admission_receipt_in_tx(&mut tx, params.id.as_str(), params, now)
            .await?;
        let start_event_payload_json = background_agent_start_event_payload(
            start_event_payload_json,
            admission_identity_sha256.as_str(),
        );
        let event = super::events::append_background_agent_event_in_tx(
            &mut tx,
            params.id.as_str(),
            "agent.started",
            &start_event_payload_json,
            now,
        )
        .await?;
        let execution_snapshot_id = insert_background_agent_execution_snapshot_in_tx(
            &mut tx,
            &execution_snapshot_params,
            now,
        )
        .await?;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(
            &mut tx,
            &BackgroundAgentStatusSnapshotParams {
                run_id: params.id.clone(),
                seq: event.seq,
                status: BackgroundAgentRunStatus::Queued,
                desired_state: BackgroundAgentDesiredState::Running,
                summary: Some("Queued".to_string()),
                pending_interaction_count: 0,
                last_event_seq: event.seq,
                payload_json: serde_json::json!({"phase": "queued"}),
            },
            now,
        )
        .await?;
        sqlx::query(
            r#"
UPDATE background_agent_runs
SET admission_identity_sha256 = ?, admission_ready_at = ?, updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(admission_identity_sha256)
        .bind(now)
        .bind(now)
        .bind(params.id.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.load_background_agent_admission_result(
            params.id.as_str(),
            /*created*/ true,
            event,
            execution_snapshot_id,
        )
        .await
    }

    async fn create_background_agent_run_row(
        &self,
        params: &BackgroundAgentRunCreateParams,
    ) -> anyhow::Result<(BackgroundAgentRun, bool)> {
        let idempotency_key = params.idempotency_key.as_deref();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(idempotency_key) = idempotency_key
            && let Some(existing_id) = validate_existing_background_agent_admission_in_tx(
                &mut tx,
                idempotency_key,
                params,
                ExistingBackgroundAgentAdmissionIdentity::RunFields,
            )
            .await?
        {
            tx.commit().await?;
            let existing = self
                .get_background_agent_run(existing_id.as_str())
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "background agent {existing_id} disappeared after idempotent admission"
                    )
                })?;
            return Ok((existing, false));
        }

        let now = Utc::now().timestamp();
        insert_background_agent_run_in_tx(&mut tx, params, now).await?;
        tx.commit().await?;
        let run = self
            .get_background_agent_run(params.id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("failed to load background agent run {}", params.id))?;
        Ok((run, true))
    }

    async fn load_background_agent_admission_result(
        &self,
        run_id: &str,
        created: bool,
        event: BackgroundAgentEvent,
        execution_snapshot_id: i64,
    ) -> anyhow::Result<(
        BackgroundAgentRun,
        bool,
        BackgroundAgentEvent,
        BackgroundAgentExecutionSnapshot,
        BackgroundAgentStatusSnapshot,
    )> {
        let run = self
            .get_background_agent_run(run_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("failed to load background agent run {run_id}"))?;
        let execution_snapshot = self
            .get_background_agent_execution_snapshot(execution_snapshot_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load background agent execution snapshot {execution_snapshot_id}"
                )
            })?;
        let status_snapshot = self
            .get_background_agent_status_snapshot(run_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("failed to load background agent status snapshot for run {run_id}")
            })?;
        Ok((run, created, event, execution_snapshot, status_snapshot))
    }

    pub async fn get_background_agent_run(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentRun>> {
        let row = sqlx::query_as::<_, BackgroundAgentRunRow>(
            r#"
SELECT
    id,
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
    spawn_linkage_json,
    worktree_lease_id,
    auth_profile_ref,
    desired_state,
    status,
    status_reason,
    config_fingerprint,
    version_fingerprint,
    retention_state,
    archive_after,
    delete_after,
    archived_at,
    deleted_at,
    supervisor_id,
    generation,
    pid,
    pgid,
    job_id,
    heartbeat_at,
    crash_reason,
    exit_code,
    exit_signal,
    last_event_seq,
    last_snapshot_seq,
    created_at,
    updated_at,
    started_at,
    completed_at
FROM background_agent_runs
WHERE id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentRun::try_from).transpose()
    }

    pub async fn list_background_agent_runs(
        &self,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<BackgroundAgentRun>> {
        let limit = limit.unwrap_or(100).min(500);
        self.list_background_agent_runs_page(/*offset*/ 0, limit)
            .await
    }

    pub async fn list_background_agent_runs_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<BackgroundAgentRun>> {
        let limit = limit.min(500);
        let rows = sqlx::query_as::<_, BackgroundAgentRunRow>(
            r#"
SELECT
    id,
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
    spawn_linkage_json,
    worktree_lease_id,
    auth_profile_ref,
    desired_state,
    status,
    status_reason,
    config_fingerprint,
    version_fingerprint,
    retention_state,
    archive_after,
    delete_after,
    archived_at,
    deleted_at,
    supervisor_id,
    generation,
    pid,
    pgid,
    job_id,
    heartbeat_at,
    crash_reason,
    exit_code,
    exit_signal,
    last_event_seq,
    last_snapshot_seq,
    created_at,
    updated_at,
    started_at,
    completed_at
FROM background_agent_runs
WHERE retention_state != 'deleted'
ORDER BY updated_at DESC, id ASC
LIMIT ?
OFFSET ?
            "#,
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.into_iter().map(BackgroundAgentRun::try_from).collect()
    }

    pub async fn count_background_agent_runs_by_status(
        &self,
    ) -> anyhow::Result<Vec<(BackgroundAgentRunStatus, i64)>> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            r#"
SELECT status, COUNT(*) as count
FROM background_agent_runs
WHERE
    status IN (
        'queued',
        'starting',
        'running',
        'waiting_on_approval',
        'waiting_on_user',
        'stopping',
        'orphaned'
    )
    AND (
        status = 'stopping'
        OR (
            status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
            AND supervisor_id IS NOT NULL
        )
        OR (
            status IN ('queued', 'orphaned')
            AND desired_state = 'running'
            AND retention_state = 'active'
            AND admission_identity_sha256 IS NOT NULL
            AND admission_ready_at IS NOT NULL
        )
    )
GROUP BY status
            "#,
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        rows.into_iter()
            .map(|(status, count)| Ok((BackgroundAgentRunStatus::parse(status.as_str())?, count)))
            .collect()
    }

    pub async fn update_background_agent_run_status(
        &self,
        run_id: &str,
        status: BackgroundAgentRunStatus,
        status_reason: Option<&str>,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let (started_at, completed_at) = background_agent_status_timestamps(status, now);
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    updated_at = ?,
    started_at = COALESCE(started_at, ?),
    completed_at = COALESCE(?, completed_at)
WHERE
    id = ?
    AND (
        status NOT IN ('completed', 'failed', 'cancelled')
        OR status = ?
    )
            "#,
        )
        .bind(status.as_str())
        .bind(status_reason.map(redact_state_string))
        .bind(now)
        .bind(started_at)
        .bind(completed_at)
        .bind(run_id)
        .bind(status.as_str())
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_background_agent_run_status_for_supervisor(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
        status: BackgroundAgentRunStatus,
        status_reason: Option<&str>,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let (started_at, completed_at) = background_agent_status_timestamps(status, now);
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    updated_at = ?,
    started_at = COALESCE(started_at, ?),
    completed_at = COALESCE(?, completed_at)
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user', 'stopping')
    AND (
        (desired_state = ? AND retention_state = ?)
        OR ? = ?
    )
            "#,
        )
        .bind(status.as_str())
        .bind(status_reason.map(redact_state_string))
        .bind(now)
        .bind(started_at)
        .bind(completed_at)
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .bind(status.as_str())
        .bind(BackgroundAgentRunStatus::Cancelled.as_str())
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn append_background_agent_status_event_for_supervisor(
        &self,
        params: BackgroundAgentStatusEventForSupervisorParams<'_>,
    ) -> anyhow::Result<Option<BackgroundAgentEvent>> {
        let now = Utc::now().timestamp();
        let (started_at, completed_at) = background_agent_status_timestamps(params.status, now);
        let receipt_key = format!(
            "status:{}:{}:{}",
            params.generation,
            params.event_type,
            params.status.as_str()
        );
        let operation_diagnostics_json = serde_json::json!({
            "status": params.status.as_str(),
            "statusReason": params.status_reason,
            "eventPayload": params.event_payload_json,
            "summary": params.summary,
            "pendingInteractionCount": params.pending_interaction_count,
            "statusPayload": params.status_payload_json,
        });
        let operation_identity_sha256 =
            super::events::background_agent_receipt_operation_identity_sha256(
                params.event_type,
                params.generation,
                /*attempt*/ None,
                &operation_diagnostics_json,
            )?;
        let diagnostics_json = super::events::bounded_background_agent_receipt_diagnostics(
            &operation_diagnostics_json,
        )?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(event) = super::events::get_background_agent_lifecycle_receipt_in_tx(
            &mut tx,
            params.run_id,
            params.event_type,
            receipt_key.as_str(),
            params.generation,
            /*attempt*/ None,
            &diagnostics_json,
            operation_identity_sha256.as_str(),
        )
        .await?
        {
            tx.commit().await?;
            return Ok(Some(event));
        }
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    updated_at = ?,
    started_at = COALESCE(started_at, ?),
    completed_at = COALESCE(?, completed_at)
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user', 'stopping')
    AND (
        (desired_state = ? AND retention_state = ?)
        OR ? = ?
    )
            "#,
        )
        .bind(params.status.as_str())
        .bind(params.status_reason.map(redact_state_string))
        .bind(now)
        .bind(started_at)
        .bind(completed_at)
        .bind(params.run_id)
        .bind(params.supervisor_id)
        .bind(params.generation)
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .bind(params.status.as_str())
        .bind(BackgroundAgentRunStatus::Cancelled.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }

        let event = super::events::append_background_agent_lifecycle_receipt_in_tx(
            &mut tx,
            params.run_id,
            params.event_type,
            receipt_key.as_str(),
            params.generation,
            /*attempt*/ None,
            &operation_diagnostics_json,
            now,
        )
        .await?;
        let desired_state: String =
            sqlx::query_scalar("SELECT desired_state FROM background_agent_runs WHERE id = ?")
                .bind(params.run_id)
                .fetch_one(&mut *tx)
                .await?;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(
            &mut tx,
            &BackgroundAgentStatusSnapshotParams {
                run_id: params.run_id.to_string(),
                seq: event.seq,
                status: params.status,
                desired_state: BackgroundAgentDesiredState::parse(desired_state.as_str())?,
                summary: params.summary.map(str::to_string),
                pending_interaction_count: params.pending_interaction_count,
                last_event_seq: event.seq,
                payload_json: params.status_payload_json.clone(),
            },
            now,
        )
        .await?;
        tx.commit().await?;

        Ok(Some(event))
    }

    pub async fn request_background_agent_stop_for_generation(
        &self,
        run_id: &str,
        expected_supervisor_id: Option<&str>,
        expected_generation: i64,
        status_reason: &str,
        diagnostics_json: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let operation_diagnostics_json = serde_json::json!({
            "statusReason": status_reason,
            "diagnostics": diagnostics_json,
        });
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    desired_state = ?,
    status = CASE
        WHEN supervisor_id IS NULL OR status IN ('queued', 'orphaned') THEN ?
        ELSE ?
    END,
    status_reason = ?,
    updated_at = ?,
    completed_at = CASE
        WHEN supervisor_id IS NULL OR status IN ('queued', 'orphaned')
        THEN COALESCE(completed_at, ?)
        ELSE completed_at
    END
WHERE
    id = ?
    AND generation = ?
    AND (
        (supervisor_id IS NULL AND ? IS NULL)
        OR supervisor_id = ?
    )
    AND retention_state = ?
    AND status IN (
        'queued',
        'starting',
        'running',
        'waiting_on_approval',
        'waiting_on_user',
        'stopping',
        'orphaned'
    )
            "#,
        )
        .bind(BackgroundAgentDesiredState::Stopped.as_str())
        .bind(BackgroundAgentRunStatus::Cancelled.as_str())
        .bind(BackgroundAgentRunStatus::Stopping.as_str())
        .bind(redact_state_string(status_reason))
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(expected_generation)
        .bind(expected_supervisor_id)
        .bind(expected_supervisor_id)
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .execute(&mut *tx)
        .await?;

        let status = if result.rows_affected() == 0 {
            let idempotent_status: Option<String> = sqlx::query_scalar(
                r#"
SELECT status
FROM background_agent_runs
WHERE
    id = ?
    AND generation = ?
    AND (
        (supervisor_id IS NULL AND ? IS NULL)
        OR supervisor_id = ?
    )
    AND desired_state = ?
    AND status IN ('stopping', 'cancelled')
                "#,
            )
            .bind(run_id)
            .bind(expected_generation)
            .bind(expected_supervisor_id)
            .bind(expected_supervisor_id)
            .bind(BackgroundAgentDesiredState::Stopped.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(status) = idempotent_status else {
                tx.commit().await?;
                return Ok(false);
            };
            status
        } else {
            sqlx::query_scalar::<_, String>("SELECT status FROM background_agent_runs WHERE id = ?")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await?
        };
        let status = BackgroundAgentRunStatus::parse(status.as_str())?;
        let receipt_key = format!("stop:{expected_generation}");
        super::events::append_background_agent_lifecycle_receipt_in_tx(
            &mut tx,
            run_id,
            "agent.stopRequested",
            receipt_key.as_str(),
            expected_generation,
            /*attempt*/ None,
            &operation_diagnostics_json,
            now,
        )
        .await?;
        if status == BackgroundAgentRunStatus::Cancelled {
            super::interactions::terminalize_active_background_agent_pending_interactions_in_tx(
                &mut tx,
                run_id,
                BackgroundAgentPendingInteractionStatus::Cancelled,
                &operation_diagnostics_json,
                now,
            )
            .await?;
        }
        let last_event_seq: i64 =
            sqlx::query_scalar("SELECT last_event_seq FROM background_agent_runs WHERE id = ?")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await?;
        let pending_interaction_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM background_agent_pending_interactions
WHERE run_id = ? AND status IN (?, ?)
            "#,
        )
        .bind(run_id)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .fetch_one(&mut *tx)
        .await?;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(
            &mut tx,
            &BackgroundAgentStatusSnapshotParams {
                run_id: run_id.to_string(),
                seq: last_event_seq,
                status,
                desired_state: BackgroundAgentDesiredState::Stopped,
                summary: Some(status_reason.to_string()),
                pending_interaction_count,
                last_event_seq,
                payload_json: serde_json::json!({
                    "reason": status_reason,
                    "event": operation_diagnostics_json,
                }),
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn bind_background_agent_thread(
        &self,
        params: &BackgroundAgentThreadBindingParams,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    thread_id = ?,
    thread_store_kind = ?,
    thread_store_id = ?,
    rollout_path = ?,
    updated_at = ?
WHERE
    id = ?
    AND supervisor_id = ?
            AND generation = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
            "#,
        )
        .bind(params.thread_id.as_str())
        .bind(params.thread_store_kind.as_str())
        .bind(params.thread_store_id.as_deref())
        .bind(params.rollout_path.as_deref())
        .bind(now)
        .bind(params.run_id.as_str())
        .bind(params.supervisor_id.as_str())
        .bind(params.generation)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn create_background_agent_execution_snapshot_for_supervisor(
        &self,
        params: &BackgroundAgentExecutionSnapshotParams,
        supervisor_id: &str,
        generation: i64,
    ) -> anyhow::Result<Option<BackgroundAgentExecutionSnapshot>> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current: Option<i64> = sqlx::query_scalar(
            r#"
SELECT 1
FROM background_agent_runs
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
            "#,
        )
        .bind(params.run_id.as_str())
        .bind(supervisor_id)
        .bind(generation)
        .fetch_optional(&mut *tx)
        .await?;
        if current.is_none() {
            tx.commit().await?;
            return Ok(None);
        }
        let snapshot_id =
            insert_background_agent_execution_snapshot_in_tx(&mut tx, params, now).await?;
        tx.commit().await?;
        self.get_background_agent_execution_snapshot(snapshot_id)
            .await
    }

    pub async fn upsert_background_agent_status_snapshot_for_supervisor(
        &self,
        params: &BackgroundAgentStatusSnapshotParams,
        supervisor_id: &str,
        generation: i64,
    ) -> anyhow::Result<Option<BackgroundAgentStatusSnapshot>> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current_last_event_seq: Option<i64> = sqlx::query_scalar(
            r#"
SELECT last_event_seq
FROM background_agent_runs
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status = ?
    AND desired_state = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
            "#,
        )
        .bind(params.run_id.as_str())
        .bind(supervisor_id)
        .bind(generation)
        .bind(params.status.as_str())
        .bind(params.desired_state.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(current_last_event_seq) = current_last_event_seq else {
            tx.commit().await?;
            return Ok(None);
        };
        let mut params = params.clone();
        params.seq = params.seq.max(current_last_event_seq);
        params.last_event_seq = current_last_event_seq;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(&mut tx, &params, now)
            .await?;
        tx.commit().await?;
        self.get_background_agent_status_snapshot(params.run_id.as_str())
            .await
    }

    pub async fn set_background_agent_desired_state(
        &self,
        run_id: &str,
        desired_state: BackgroundAgentDesiredState,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET desired_state = ?, updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(desired_state.as_str())
        .bind(now)
        .bind(run_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn request_background_agent_delete(&self, run_id: &str) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = sqlx::query_as::<_, BackgroundAgentDeleteState>(
            r#"
SELECT supervisor_id, generation, status, retention_state, status_reason
FROM background_agent_runs
WHERE id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(BackgroundAgentDeleteState {
            supervisor_id,
            generation,
            status,
            retention_state,
            status_reason: stored_status_reason,
        }) = current
        else {
            tx.commit().await?;
            return Ok(false);
        };
        let current_status = BackgroundAgentRunStatus::parse(status.as_str())?;
        let replaying = matches!(retention_state.as_str(), "delete_requested" | "deleted");
        let (next_status, status_reason, terminalize_immediately) = if replaying {
            let status_reason = stored_status_reason.ok_or_else(|| {
                anyhow::anyhow!(
                    "background agent delete receipt replay is missing its status reason"
                )
            })?;
            (current_status, status_reason, false)
        } else {
            let already_terminal = matches!(
                current_status,
                BackgroundAgentRunStatus::Completed
                    | BackgroundAgentRunStatus::Failed
                    | BackgroundAgentRunStatus::Cancelled
            );
            let terminalize_immediately = !already_terminal
                && (supervisor_id.is_none() || matches!(status.as_str(), "queued" | "orphaned"));
            let next_status = if already_terminal {
                current_status
            } else if terminalize_immediately {
                BackgroundAgentRunStatus::Cancelled
            } else {
                BackgroundAgentRunStatus::Stopping
            };
            let status_reason = if already_terminal {
                "delete requested for terminal run"
            } else if terminalize_immediately {
                "delete requested before worker claim"
            } else {
                "delete requested"
            };
            (
                next_status,
                status_reason.to_string(),
                terminalize_immediately,
            )
        };
        if !replaying {
            let result = sqlx::query(
                r#"
UPDATE background_agent_runs
SET
    desired_state = ?,
    retention_state = ?,
    status = ?,
    status_reason = ?,
    delete_after = COALESCE(delete_after, ?),
    updated_at = ?,
    completed_at = CASE
        WHEN ? = ? THEN COALESCE(completed_at, ?)
        ELSE completed_at
    END
WHERE
    id = ?
    AND generation = ?
    AND (
        (supervisor_id IS NULL AND ? IS NULL)
        OR supervisor_id = ?
    )
    AND retention_state = ?
            "#,
            )
            .bind(BackgroundAgentDesiredState::Deleted.as_str())
            .bind(crate::BackgroundAgentRetentionState::DeleteRequested.as_str())
            .bind(next_status.as_str())
            .bind(status_reason.as_str())
            .bind(now)
            .bind(now)
            .bind(next_status.as_str())
            .bind(BackgroundAgentRunStatus::Cancelled.as_str())
            .bind(now)
            .bind(run_id)
            .bind(generation)
            .bind(supervisor_id.as_deref())
            .bind(supervisor_id.as_deref())
            .bind(crate::BackgroundAgentRetentionState::Active.as_str())
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                tx.commit().await?;
                return Ok(false);
            }
        }
        let diagnostics_json = serde_json::json!({
            "reason": "delete_requested",
            "statusReason": status_reason.as_str(),
        });
        let receipt_key = format!("delete:{generation}");
        super::events::append_background_agent_lifecycle_receipt_in_tx(
            &mut tx,
            run_id,
            "agent.deleteRequested",
            receipt_key.as_str(),
            generation,
            /*attempt*/ None,
            &diagnostics_json,
            now,
        )
        .await?;
        if terminalize_immediately {
            super::interactions::terminalize_active_background_agent_pending_interactions_in_tx(
                &mut tx,
                run_id,
                BackgroundAgentPendingInteractionStatus::Cancelled,
                &diagnostics_json,
                now,
            )
            .await?;
        }
        let last_event_seq: i64 =
            sqlx::query_scalar("SELECT last_event_seq FROM background_agent_runs WHERE id = ?")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await?;
        let pending_interaction_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM background_agent_pending_interactions
WHERE run_id = ? AND status IN (?, ?)
            "#,
        )
        .bind(run_id)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .fetch_one(&mut *tx)
        .await?;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(
            &mut tx,
            &BackgroundAgentStatusSnapshotParams {
                run_id: run_id.to_string(),
                seq: last_event_seq,
                status: next_status,
                desired_state: BackgroundAgentDesiredState::Deleted,
                summary: Some(status_reason.clone()),
                pending_interaction_count,
                last_event_seq,
                payload_json: serde_json::json!({
                    "phase": next_status.as_str(),
                    "reason": "delete_requested",
                    "statusReason": status_reason.as_str(),
                }),
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn orphan_stale_background_agent_runs(
        &self,
        heartbeat_timeout: Duration,
    ) -> anyhow::Result<usize> {
        let now = Utc::now().timestamp();
        let timeout_secs = i64::try_from(heartbeat_timeout.as_secs()).unwrap_or(i64::MAX);
        let stale_before = now.saturating_sub(timeout_secs);
        let orphan_candidates = sqlx::query_as::<_, (String, String, i64)>(
            r#"
SELECT id, supervisor_id, generation
FROM background_agent_runs
WHERE
    desired_state = ?
    AND retention_state = ?
    AND supervisor_id IS NOT NULL
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
    AND (heartbeat_at IS NULL OR heartbeat_at <= ?)
            "#,
        )
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .bind(stale_before)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut finalized = 0;
        for (run_id, supervisor_id, generation) in orphan_candidates {
            let mut tx = self.pool.begin().await?;
            let result = sqlx::query(
                r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    crash_reason = COALESCE(crash_reason, ?),
    updated_at = ?
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND desired_state = ?
    AND retention_state = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
    AND (heartbeat_at IS NULL OR heartbeat_at <= ?)
                "#,
            )
            .bind(BackgroundAgentRunStatus::Orphaned.as_str())
            .bind("supervisor heartbeat stale")
            .bind("supervisor heartbeat stale")
            .bind(now)
            .bind(run_id.as_str())
            .bind(supervisor_id.as_str())
            .bind(generation)
            .bind(BackgroundAgentDesiredState::Running.as_str())
            .bind(crate::BackgroundAgentRetentionState::Active.as_str())
            .bind(stale_before)
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() == 0 {
                tx.commit().await?;
                continue;
            }

            sqlx::query(
                r#"
UPDATE background_agent_process_leases
SET
    status = 'orphaned',
    exit_reason = COALESCE(exit_reason, ?),
    updated_at = ?
WHERE run_id = ? AND supervisor_id = ? AND generation = ?
                "#,
            )
            .bind("supervisor heartbeat stale")
            .bind(now)
            .bind(run_id.as_str())
            .bind(supervisor_id.as_str())
            .bind(generation)
            .execute(&mut *tx)
            .await?;

            let payload_json = serde_json::json!({
                "reason": "supervisor_heartbeat_stale",
                "previousSupervisorId": supervisor_id,
                "generation": generation,
                "staleBefore": stale_before,
            });
            super::interactions::terminalize_active_background_agent_pending_interactions_in_tx(
                &mut tx,
                run_id.as_str(),
                BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting,
                &payload_json,
                now,
            )
            .await?;
            append_terminal_stale_background_agent_status_in_tx(
                &mut tx,
                run_id.as_str(),
                BackgroundAgentRunStatus::Orphaned,
                "supervisor heartbeat stale",
                "agent.orphaned",
                &payload_json,
                now,
            )
            .await?;
            tx.commit().await?;
            finalized += 1;
        }

        let stopping_candidates = sqlx::query_as::<_, (String, String, i64)>(
            r#"
SELECT id, supervisor_id, generation
FROM background_agent_runs
WHERE
    status = ?
    AND supervisor_id IS NOT NULL
    AND (desired_state != ? OR retention_state = ?)
    AND (heartbeat_at IS NULL OR heartbeat_at <= ?)
            "#,
        )
        .bind(BackgroundAgentRunStatus::Stopping.as_str())
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::DeleteRequested.as_str())
        .bind(stale_before)
        .fetch_all(self.pool.as_ref())
        .await?;

        for (run_id, supervisor_id, generation) in stopping_candidates {
            let mut tx = self.pool.begin().await?;
            let result = sqlx::query(
                r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    crash_reason = COALESCE(crash_reason, ?),
    updated_at = ?,
    completed_at = COALESCE(completed_at, ?)
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status = ?
    AND (desired_state != ? OR retention_state = ?)
    AND (heartbeat_at IS NULL OR heartbeat_at <= ?)
                "#,
            )
            .bind(BackgroundAgentRunStatus::Cancelled.as_str())
            .bind("stop heartbeat stale")
            .bind("stop heartbeat stale")
            .bind(now)
            .bind(now)
            .bind(run_id.as_str())
            .bind(supervisor_id.as_str())
            .bind(generation)
            .bind(BackgroundAgentRunStatus::Stopping.as_str())
            .bind(BackgroundAgentDesiredState::Running.as_str())
            .bind(crate::BackgroundAgentRetentionState::DeleteRequested.as_str())
            .bind(stale_before)
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() == 0 {
                tx.commit().await?;
                continue;
            }

            sqlx::query(
                r#"
UPDATE background_agent_process_leases
SET
    status = 'stopped',
    exit_reason = COALESCE(exit_reason, ?),
    updated_at = ?,
    stopped_at = COALESCE(stopped_at, ?)
WHERE run_id = ? AND supervisor_id = ? AND generation = ?
                "#,
            )
            .bind("stop heartbeat stale")
            .bind(now)
            .bind(now)
            .bind(run_id.as_str())
            .bind(supervisor_id.as_str())
            .bind(generation)
            .execute(&mut *tx)
            .await?;

            let payload_json = serde_json::json!({
                "reason": "stop_heartbeat_stale",
                "previousSupervisorId": supervisor_id,
                "generation": generation,
                "staleBefore": stale_before,
            });
            super::interactions::terminalize_active_background_agent_pending_interactions_in_tx(
                &mut tx,
                run_id.as_str(),
                BackgroundAgentPendingInteractionStatus::Cancelled,
                &payload_json,
                now,
            )
            .await?;
            append_terminal_stale_background_agent_status_in_tx(
                &mut tx,
                run_id.as_str(),
                BackgroundAgentRunStatus::Cancelled,
                "stop heartbeat stale",
                "agent.cancelled",
                &payload_json,
                now,
            )
            .await?;
            tx.commit().await?;
            finalized += 1;
        }

        Ok(finalized)
    }

    pub async fn finalize_stopped_background_agent_process(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
        status_reason: &str,
        event_payload_json: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    updated_at = ?,
    completed_at = COALESCE(completed_at, ?)
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user', 'stopping')
    AND (desired_state != ? OR retention_state = ?)
            "#,
        )
        .bind(BackgroundAgentRunStatus::Cancelled.as_str())
        .bind(redact_state_string(status_reason))
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::DeleteRequested.as_str())
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }

        sqlx::query(
            r#"
UPDATE background_agent_process_leases
SET
    status = 'stopped',
    exit_reason = COALESCE(exit_reason, ?),
    updated_at = ?,
    stopped_at = COALESCE(stopped_at, ?)
WHERE run_id = ? AND supervisor_id = ? AND generation = ?
            "#,
        )
        .bind(redact_state_string(status_reason))
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?;

        super::interactions::terminalize_active_background_agent_pending_interactions_in_tx(
            &mut tx,
            run_id,
            BackgroundAgentPendingInteractionStatus::Cancelled,
            event_payload_json,
            now,
        )
        .await?;
        append_terminal_stale_background_agent_status_in_tx(
            &mut tx,
            run_id,
            BackgroundAgentRunStatus::Cancelled,
            status_reason,
            "agent.cancelled",
            event_payload_json,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn fail_unclaimed_background_agent_process_spawn(
        &self,
        run_id: &str,
        status_reason: &str,
        event_payload_json: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    status = ?,
    status_reason = ?,
    crash_reason = COALESCE(crash_reason, ?),
    updated_at = ?,
    completed_at = COALESCE(completed_at, ?)
WHERE
    id = ?
    AND desired_state = ?
    AND retention_state = ?
    AND status = ?
    AND supervisor_id IS NULL
            "#,
        )
        .bind(BackgroundAgentRunStatus::Failed.as_str())
        .bind(redact_state_string(status_reason))
        .bind(redact_state_string(status_reason))
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .bind(BackgroundAgentRunStatus::Queued.as_str())
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }

        append_terminal_stale_background_agent_status_in_tx(
            &mut tx,
            run_id,
            BackgroundAgentRunStatus::Failed,
            status_reason,
            "agent.failed",
            event_payload_json,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn claim_background_agent_supervisor(
        &self,
        run_id: &str,
        supervisor_id: &str,
        process_lease_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        self.claim_background_agent_supervisor_inner(
            run_id,
            supervisor_id,
            process_lease_id,
            /*required_version_fingerprint*/ None,
            /*required_package_fingerprint*/ None,
        )
        .await
    }

    pub async fn claim_background_agent_supervisor_compatible(
        &self,
        run_id: &str,
        supervisor_id: &str,
        process_lease_id: &str,
        required_version_fingerprint: &str,
        required_package_fingerprint: &str,
    ) -> anyhow::Result<Option<i64>> {
        self.claim_background_agent_supervisor_inner(
            run_id,
            supervisor_id,
            process_lease_id,
            Some(required_version_fingerprint),
            Some(required_package_fingerprint),
        )
        .await
    }

    pub async fn background_agent_admission_is_ready(
        &self,
        run_id: &str,
        required_version_fingerprint: &str,
        required_package_fingerprint: &str,
    ) -> anyhow::Result<bool> {
        let ready: Option<i64> = sqlx::query_scalar(
            r#"
SELECT 1
FROM background_agent_runs
WHERE
    id = ?
    AND version_fingerprint = ?
    AND admission_identity_sha256 IS NOT NULL
    AND admission_ready_at IS NOT NULL
    AND EXISTS (
        SELECT 1
        FROM background_agent_lifecycle_receipts r
        WHERE
            r.run_id = background_agent_runs.id
            AND r.event_type = 'agent.admitted'
    )
    AND EXISTS (
        SELECT 1
        FROM background_agent_events e
        WHERE
            e.run_id = background_agent_runs.id
            AND e.event_type IN ('agent.started', 'agent.startRecovered')
    )
    AND EXISTS (
        SELECT 1
        FROM background_agent_execution_snapshots s
        WHERE
            s.run_id = background_agent_runs.id
            AND s.snapshot_kind = 'initial_execution_context'
            AND json_extract(s.payload_json, '$.packageFingerprint') = ?
            AND (
                json_extract(s.payload_json, '$.managedWorktreeId') IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM managed_worktree_assignments a
                    WHERE
                        a.worktree_id = json_extract(
                            s.payload_json,
                            '$.managedWorktreeId'
                        )
                        AND a.agent_run_id = background_agent_runs.id
                        AND a.detached_at_ms IS NULL
                )
            )
    )
    AND EXISTS (
        SELECT 1
        FROM background_agent_status_snapshots s
        WHERE s.run_id = background_agent_runs.id
    )
            "#,
        )
        .bind(run_id)
        .bind(required_version_fingerprint)
        .bind(required_package_fingerprint)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(ready.is_some())
    }

    pub async fn get_background_agent_initial_execution_snapshot(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentExecutionSnapshot>> {
        let row = sqlx::query_as::<_, BackgroundAgentExecutionSnapshotRow>(
            r#"
SELECT
    id,
    run_id,
    seq,
    snapshot_kind,
    payload_json,
    recovery_policy,
    config_fingerprint,
    created_at
FROM background_agent_execution_snapshots
WHERE run_id = ? AND snapshot_kind = 'initial_execution_context'
ORDER BY seq ASC
LIMIT 1
            "#,
        )
        .bind(run_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentExecutionSnapshot::try_from)
            .transpose()
    }

    async fn claim_background_agent_supervisor_inner(
        &self,
        run_id: &str,
        supervisor_id: &str,
        process_lease_id: &str,
        required_version_fingerprint: Option<&str>,
        required_package_fingerprint: Option<&str>,
    ) -> anyhow::Result<Option<i64>> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let current = sqlx::query_as::<_, BackgroundAgentSupervisorClaimState>(
            r#"
SELECT
    generation,
    desired_state,
    status,
    retention_state,
    admission_identity_sha256,
    admission_ready_at
FROM background_agent_runs
WHERE id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(BackgroundAgentSupervisorClaimState {
            generation: current_generation,
            desired_state,
            status,
            retention_state,
            admission_identity_sha256,
            admission_ready_at,
        }) = current
        else {
            tx.rollback().await?;
            return Ok(None);
        };
        let admission_ready = required_version_fingerprint.is_none()
            || (admission_identity_sha256.is_some() && admission_ready_at.is_some());
        let eligible = desired_state == BackgroundAgentDesiredState::Running.as_str()
            && retention_state == crate::BackgroundAgentRetentionState::Active.as_str()
            && matches!(status.as_str(), "queued" | "orphaned")
            && admission_ready;
        if !eligible {
            tx.rollback().await?;
            return Ok(None);
        }

        let generation = current_generation + 1;
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    supervisor_id = ?,
    generation = ?,
    status = ?,
    status_reason = ?,
    heartbeat_at = ?,
    updated_at = ?,
    started_at = COALESCE(started_at, ?)
WHERE
    id = ?
    AND generation = ?
    AND desired_state = ?
    AND retention_state = ?
    AND status IN ('queued', 'orphaned')
    AND (
        ? IS NULL
        OR (
            version_fingerprint = ?
            AND admission_identity_sha256 IS NOT NULL
            AND admission_ready_at IS NOT NULL
            AND EXISTS (
                SELECT 1
                FROM background_agent_lifecycle_receipts r
                WHERE
                    r.run_id = background_agent_runs.id
                    AND r.event_type = 'agent.admitted'
            )
            AND EXISTS (
                SELECT 1
                FROM background_agent_events e
                WHERE
                    e.run_id = background_agent_runs.id
                    AND e.event_type IN ('agent.started', 'agent.startRecovered')
            )
            AND EXISTS (
                SELECT 1
                FROM background_agent_execution_snapshots s
                WHERE
                    s.run_id = background_agent_runs.id
                    AND s.snapshot_kind = 'initial_execution_context'
                    AND json_extract(s.payload_json, '$.packageFingerprint') = ?
                    AND (
                        json_extract(s.payload_json, '$.managedWorktreeId') IS NULL
                        OR EXISTS (
                            SELECT 1
                            FROM managed_worktree_assignments a
                            WHERE
                                a.worktree_id = json_extract(
                                    s.payload_json,
                                    '$.managedWorktreeId'
                                )
                                AND a.agent_run_id = background_agent_runs.id
                                AND a.detached_at_ms IS NULL
                        )
                    )
            )
            AND EXISTS (
                SELECT 1
                FROM background_agent_status_snapshots s
                WHERE s.run_id = background_agent_runs.id
            )
        )
    )
            "#,
        )
        .bind(supervisor_id)
        .bind(generation)
        .bind(BackgroundAgentRunStatus::Starting.as_str())
        .bind("claimed by background supervisor")
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(current_generation)
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .bind(required_version_fingerprint)
        .bind(required_version_fingerprint)
        .bind(required_package_fingerprint)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }

        sqlx::query(
            r#"
INSERT INTO background_agent_process_leases (
    id,
    run_id,
    supervisor_id,
    generation,
    status,
    heartbeat_at,
    created_at,
    updated_at,
    started_at
) VALUES (?, ?, ?, ?, 'starting', ?, ?, ?, ?)
            "#,
        )
        .bind(process_lease_id)
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let receipt_key = format!("claim:{generation}");
        super::events::append_background_agent_lifecycle_receipt_in_tx(
            &mut tx,
            run_id,
            "agent.claimed",
            receipt_key.as_str(),
            generation,
            /*attempt*/ None,
            &serde_json::json!({
                "supervisorId": supervisor_id,
                "processLeaseId": process_lease_id,
            }),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(Some(generation))
    }

    pub async fn record_background_agent_execution_handle(
        &self,
        params: BackgroundAgentExecutionHandleParams<'_>,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    pid = ?,
    pgid = ?,
    job_id = ?,
    heartbeat_at = ?,
    updated_at = ?
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status = ?
            "#,
        )
        .bind(params.pid)
        .bind(params.pgid)
        .bind(params.job_id)
        .bind(now)
        .bind(now)
        .bind(params.run_id)
        .bind(params.supervisor_id)
        .bind(params.generation)
        .bind(BackgroundAgentRunStatus::Starting.as_str())
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() > 0 {
            sqlx::query(
                r#"
UPDATE background_agent_process_leases
SET
    pid = ?,
    pgid = ?,
    job_id = ?,
    start_token = ?,
    stderr_log_path = ?,
    status = 'running',
    heartbeat_at = ?,
    updated_at = ?
WHERE run_id = ? AND supervisor_id = ? AND generation = ?
                "#,
            )
            .bind(params.pid)
            .bind(params.pgid)
            .bind(params.job_id)
            .bind(params.start_token)
            .bind(params.stderr_log_path)
            .bind(now)
            .bind(now)
            .bind(params.run_id)
            .bind(params.supervisor_id)
            .bind(params.generation)
            .execute(&mut *tx)
            .await?;
            let receipt_key = format!("heartbeat:{}", params.generation);
            super::events::append_background_agent_lifecycle_receipt_in_tx(
                &mut tx,
                params.run_id,
                "agent.heartbeat",
                receipt_key.as_str(),
                params.generation,
                /*attempt*/ None,
                &serde_json::json!({
                    "supervisorId": params.supervisor_id,
                    "pid": params.pid,
                    "pgid": params.pgid,
                    "jobId": params.job_id,
                    "startToken": params.start_token,
                    "stderrLogPath": params.stderr_log_path,
                }),
                now,
            )
            .await?;
        }

        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_background_agent_active_process_handles(
        &self,
    ) -> anyhow::Result<Vec<BackgroundAgentProcessHandleRecord>> {
        let rows = sqlx::query_as::<_, (String, i64, i64, Option<i64>, String, String)>(
            r#"
SELECT
    l.run_id,
    l.generation,
    l.pid,
    l.pgid,
    l.start_token,
    l.stderr_log_path
FROM background_agent_process_leases l
JOIN background_agent_runs r
    ON r.id = l.run_id
    AND r.generation = l.generation
WHERE
    l.status IN ('starting', 'running')
    AND l.pid IS NOT NULL
    AND l.start_token IS NOT NULL
    AND l.stderr_log_path IS NOT NULL
    AND r.retention_state != ?
    AND r.status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user', 'stopping', 'orphaned')
            "#,
        )
        .bind(crate::BackgroundAgentRetentionState::Deleted.as_str())
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.into_iter()
            .map(
                |(run_id, generation, pid, pgid, start_token, stderr_log_path)| {
                    Ok(BackgroundAgentProcessHandleRecord {
                        run_id,
                        generation,
                        pid: u32::try_from(pid)?,
                        pgid: pgid.map(u32::try_from).transpose()?,
                        start_token,
                        stderr_log_path: PathBuf::from(stderr_log_path),
                    })
                },
            )
            .collect()
    }

    pub async fn heartbeat_background_agent_run(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET heartbeat_at = ?, updated_at = ?
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user', 'stopping')
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() > 0 {
            sqlx::query(
                r#"
UPDATE background_agent_process_leases
SET heartbeat_at = ?, updated_at = ?
WHERE run_id = ? AND supervisor_id = ? AND generation = ?
                "#,
            )
            .bind(now)
            .bind(now)
            .bind(run_id)
            .bind(supervisor_id)
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn finish_background_agent_process_lease(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
        exit_code: Option<i64>,
        exit_signal: Option<i64>,
        exit_reason: Option<&str>,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let result = sqlx::query(
            r#"
UPDATE background_agent_process_leases
SET
    status = 'stopped',
    exit_code = ?,
    exit_signal = ?,
    exit_reason = ?,
    updated_at = ?,
    stopped_at = COALESCE(stopped_at, ?)
WHERE run_id = ? AND supervisor_id = ? AND generation = ?
            "#,
        )
        .bind(exit_code)
        .bind(exit_signal)
        .bind(exit_reason)
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Fails closed any unclaimed run that the currently installed runtime can
    /// never claim, so that an upgrade cannot permanently strand admission
    /// capacity.
    ///
    /// A claim requires the persisted admission schema *and* the execution
    /// snapshot `packageFingerprint` to match the running binary. Queued and
    /// orphaned rows keep consuming a live-or-recoverable capacity slot, so
    /// after a version bump those rows would otherwise be both unclaimable and
    /// undeletable by any reconciliation pass. Terminalizing them releases the
    /// slot and leaves an explicit lifecycle receipt describing why.
    pub async fn terminalize_incompatible_background_agent_runs(
        &self,
        required_version_fingerprint: &str,
        required_package_fingerprint: &str,
    ) -> anyhow::Result<usize> {
        let now = Utc::now().timestamp();
        // The `NOT EXISTS` / fingerprint predicate below is the exact negation
        // of the compatibility clause enforced by
        // `claim_background_agent_supervisor_compatible`; keep the two in sync.
        let candidates = sqlx::query_scalar::<_, String>(
            r#"
SELECT id
FROM background_agent_runs
WHERE
    desired_state = ?
    AND retention_state = ?
    AND status IN ('queued', 'orphaned')
    AND (
        COALESCE(version_fingerprint, '') != ?
        OR NOT EXISTS (
            SELECT 1
            FROM background_agent_execution_snapshots s
            WHERE
                s.run_id = background_agent_runs.id
                AND s.snapshot_kind = 'initial_execution_context'
                AND json_extract(s.payload_json, '$.packageFingerprint') = ?
        )
    )
            "#,
        )
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .bind(required_version_fingerprint)
        .bind(required_package_fingerprint)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut finalized = 0;
        for run_id in candidates {
            let mut tx = self.pool.begin().await?;
            let result = sqlx::query(
                r#"
UPDATE background_agent_runs
SET
    desired_state = ?,
    status = ?,
    status_reason = ?,
    crash_reason = COALESCE(crash_reason, ?),
    updated_at = ?,
    completed_at = COALESCE(completed_at, ?)
WHERE
    id = ?
    AND desired_state = ?
    AND retention_state = ?
    AND status IN ('queued', 'orphaned')
    AND (
        COALESCE(version_fingerprint, '') != ?
        OR NOT EXISTS (
            SELECT 1
            FROM background_agent_execution_snapshots s
            WHERE
                s.run_id = background_agent_runs.id
                AND s.snapshot_kind = 'initial_execution_context'
                AND json_extract(s.payload_json, '$.packageFingerprint') = ?
        )
    )
                "#,
            )
            .bind(BackgroundAgentDesiredState::Stopped.as_str())
            .bind(BackgroundAgentRunStatus::Failed.as_str())
            .bind(BACKGROUND_AGENT_RUNTIME_INCOMPATIBLE_REASON)
            .bind(BACKGROUND_AGENT_RUNTIME_INCOMPATIBLE_REASON)
            .bind(now)
            .bind(now)
            .bind(run_id.as_str())
            .bind(BackgroundAgentDesiredState::Running.as_str())
            .bind(crate::BackgroundAgentRetentionState::Active.as_str())
            .bind(required_version_fingerprint)
            .bind(required_package_fingerprint)
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() == 0 {
                tx.commit().await?;
                continue;
            }

            sqlx::query(
                r#"
UPDATE background_agent_process_leases
SET
    status = 'stopped',
    exit_reason = COALESCE(exit_reason, ?),
    updated_at = ?,
    stopped_at = COALESCE(stopped_at, ?)
WHERE run_id = ? AND status != 'stopped'
                "#,
            )
            .bind(BACKGROUND_AGENT_RUNTIME_INCOMPATIBLE_REASON)
            .bind(now)
            .bind(now)
            .bind(run_id.as_str())
            .execute(&mut *tx)
            .await?;

            let payload_json = serde_json::json!({
                "reason": "runtime_package_incompatible",
                "requiredVersionFingerprint": required_version_fingerprint,
                "requiredPackageFingerprint": required_package_fingerprint,
            });
            super::interactions::terminalize_active_background_agent_pending_interactions_in_tx(
                &mut tx,
                run_id.as_str(),
                BackgroundAgentPendingInteractionStatus::Cancelled,
                &payload_json,
                now,
            )
            .await?;
            append_terminal_stale_background_agent_status_in_tx(
                &mut tx,
                run_id.as_str(),
                BackgroundAgentRunStatus::Failed,
                BACKGROUND_AGENT_RUNTIME_INCOMPATIBLE_REASON,
                "agent.failed",
                &payload_json,
                now,
            )
            .await?;
            tx.commit().await?;
            finalized += 1;
        }

        Ok(finalized)
    }

    pub async fn get_background_agent_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<BackgroundAgentRun>> {
        let row = sqlx::query_as::<_, BackgroundAgentRunRow>(
            r#"
SELECT
    id,
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
    spawn_linkage_json,
    worktree_lease_id,
    auth_profile_ref,
    desired_state,
    status,
    status_reason,
    config_fingerprint,
    version_fingerprint,
    retention_state,
    archive_after,
    delete_after,
    archived_at,
    deleted_at,
    supervisor_id,
    generation,
    pid,
    pgid,
    job_id,
    heartbeat_at,
    crash_reason,
    exit_code,
    exit_signal,
    last_event_seq,
    last_snapshot_seq,
    created_at,
    updated_at,
    started_at,
    completed_at
FROM background_agent_runs
WHERE idempotency_key = ?
            "#,
        )
        .bind(background_agent_idempotency_key_digest(idempotency_key))
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentRun::try_from).transpose()
    }
}

async fn ensure_background_agent_capacity_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    max_active_runs: i64,
) -> anyhow::Result<()> {
    let active_run_count = count_live_or_recoverable_background_agent_runs_in_tx(tx).await?;
    if active_run_count >= max_active_runs {
        anyhow::bail!(
            "{BACKGROUND_AGENT_ADMISSION_CAPACITY_EXCEEDED}: \
             {active_run_count} live or recoverable run(s), max {max_active_runs}"
        );
    }
    Ok(())
}

pub(in crate::runtime) async fn count_live_or_recoverable_background_agent_runs_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
) -> anyhow::Result<i64> {
    sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM background_agent_runs
WHERE
    status IN (
        'queued',
        'starting',
        'running',
        'waiting_on_approval',
        'waiting_on_user',
        'stopping',
        'orphaned'
    )
    AND (
        status = 'stopping'
        OR (
            status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
            AND supervisor_id IS NOT NULL
        )
        OR (
            status IN ('queued', 'orphaned')
            AND desired_state = 'running'
            AND retention_state = 'active'
            AND admission_identity_sha256 IS NOT NULL
            AND admission_ready_at IS NOT NULL
        )
    )
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(anyhow::Error::from)
}

pub(in crate::runtime) async fn insert_background_agent_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &BackgroundAgentRunCreateParams,
    now: i64,
) -> anyhow::Result<()> {
    let spawn_linkage_json = params
        .spawn_linkage_json
        .as_ref()
        .map(redact_state_json_string)
        .transpose()?;
    let insert_result = sqlx::query(
        r#"
INSERT INTO background_agent_runs (
    id,
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
    spawn_linkage_json,
    auth_profile_ref,
    desired_state,
    status,
    status_reason,
    config_fingerprint,
    version_fingerprint,
    retention_state,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(params.id.as_str())
    .bind(
        params
            .idempotency_key
            .as_deref()
            .map(background_agent_idempotency_key_digest),
    )
    .bind(params.request_id.as_deref().map(redact_state_string))
    .bind(params.source.as_str())
    .bind(params.prompt_snapshot_ref.as_str())
    .bind(
        params
            .input_snapshot_ref
            .as_deref()
            .map(redact_state_string),
    )
    .bind(params.thread_id.as_deref())
    .bind(params.thread_store_kind.as_str())
    .bind(params.thread_store_id.as_deref())
    .bind(params.rollout_path.as_deref())
    .bind(params.parent_thread_id.as_deref())
    .bind(params.parent_agent_run_id.as_deref())
    .bind(spawn_linkage_json.as_deref())
    .bind(params.auth_profile_ref.as_deref().map(redact_state_string))
    .bind(BackgroundAgentDesiredState::Running.as_str())
    .bind(BackgroundAgentRunStatus::Queued.as_str())
    .bind(params.status_reason.as_deref().map(redact_state_string))
    .bind(params.config_fingerprint.as_deref())
    .bind(params.version_fingerprint.as_deref())
    .bind(crate::BackgroundAgentRetentionState::Active.as_str())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await;
    if let Err(err) = insert_result {
        if is_background_agent_unique_constraint_violation(&err) {
            anyhow::bail!(
                "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
                 request identity conflicts with an existing background agent"
            );
        }
        return Err(err.into());
    }
    Ok(())
}

pub(in crate::runtime) fn background_agent_admission_identity_sha256(
    params: &BackgroundAgentRunCreateParams,
    start_event_payload_json: &serde_json::Value,
    execution_snapshot_params: &BackgroundAgentExecutionSnapshotParams,
) -> anyhow::Result<String> {
    let identity = serde_json::json!({
        "idempotencyKey": params.idempotency_key,
        "requestId": params.request_id,
        "source": params.source,
        "promptSnapshotRef": params.prompt_snapshot_ref,
        "inputSnapshotRef": params.input_snapshot_ref,
        "threadId": params.thread_id,
        "threadStoreKind": params.thread_store_kind,
        "threadStoreId": params.thread_store_id,
        "rolloutPath": params.rollout_path,
        "parentThreadId": params.parent_thread_id,
        "parentAgentRunId": params.parent_agent_run_id,
        "spawnLinkage": params.spawn_linkage_json,
        "authProfileRef": params.auth_profile_ref,
        "configFingerprint": params.config_fingerprint,
        "versionFingerprint": params.version_fingerprint,
        "startEvent": start_event_payload_json,
        "executionSnapshot": {
            "snapshotKind": execution_snapshot_params.snapshot_kind,
            "payload": execution_snapshot_params.payload_json,
            "recoveryPolicy": execution_snapshot_params.recovery_policy,
            "configFingerprint": execution_snapshot_params.config_fingerprint,
        },
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity)?)
    ))
}

async fn append_background_agent_admission_receipt_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    params: &BackgroundAgentRunCreateParams,
    now: i64,
) -> anyhow::Result<BackgroundAgentEvent> {
    let receipt_identity = params.idempotency_key.as_deref().unwrap_or(run_id);
    let receipt_key = format!(
        "admission:{:x}",
        Sha256::digest(receipt_identity.as_bytes())
    );
    super::events::append_background_agent_lifecycle_receipt_in_tx(
        tx,
        run_id,
        "agent.admitted",
        receipt_key.as_str(),
        /*generation*/ 0,
        /*attempt*/ None,
        &serde_json::json!({
            "source": params.source,
            "requestId": params.request_id,
            "versionFingerprint": params.version_fingerprint,
        }),
        now,
    )
    .await
}

fn background_agent_start_event_payload(
    payload_json: &serde_json::Value,
    admission_identity_sha256: &str,
) -> serde_json::Value {
    let mut payload_json = payload_json.clone();
    if let Some(payload) = payload_json.as_object_mut() {
        payload.insert(
            "admissionIdentitySha256".to_string(),
            serde_json::Value::String(admission_identity_sha256.to_string()),
        );
    }
    payload_json
}

async fn insert_background_agent_execution_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &BackgroundAgentExecutionSnapshotParams,
    now: i64,
) -> anyhow::Result<i64> {
    let payload_json = redact_state_json_string(&params.payload_json)?;
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM background_agent_execution_snapshots WHERE run_id = ?",
    )
    .bind(params.run_id.as_str())
    .fetch_one(&mut **tx)
    .await?;
    let id = sqlx::query(
        r#"
INSERT INTO background_agent_execution_snapshots (
    run_id,
    seq,
    snapshot_kind,
    payload_json,
    recovery_policy,
    config_fingerprint,
    created_at
) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(params.run_id.as_str())
    .bind(seq)
    .bind(params.snapshot_kind.as_str())
    .bind(payload_json)
    .bind(params.recovery_policy.as_str())
    .bind(params.config_fingerprint.as_deref())
    .bind(now)
    .execute(&mut **tx)
    .await?
    .last_insert_rowid();
    sqlx::query(
        r#"
UPDATE background_agent_runs
SET last_snapshot_seq = ?, updated_at = ?
WHERE id = ?
        "#,
    )
    .bind(seq)
    .bind(now)
    .bind(params.run_id.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

pub(in crate::runtime) async fn recover_or_validate_background_agent_initial_state_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    params: &BackgroundAgentRunCreateParams,
    admission_identity_sha256: &str,
    start_event_payload_json: &serde_json::Value,
    execution_snapshot_params: &BackgroundAgentExecutionSnapshotParams,
    max_active_runs: i64,
) -> anyhow::Result<(BackgroundAgentEvent, i64)> {
    let mut execution_snapshot_params = execution_snapshot_params.clone();
    execution_snapshot_params.run_id = run_id.to_string();
    let (stored_identity, admission_ready_at): (Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT admission_identity_sha256, admission_ready_at \
         FROM background_agent_runs WHERE id = ?",
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    if let Some(stored_identity) = stored_identity.as_deref()
        && stored_identity != admission_identity_sha256
    {
        anyhow::bail!(
            "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
             idempotency key is already bound to different prompt or execution context"
        );
    }
    if admission_ready_at.is_none() {
        ensure_background_agent_capacity_in_tx(tx, max_active_runs).await?;
    }

    let existing_event = sqlx::query_as::<_, BackgroundAgentEventRow>(
        r#"
SELECT id, run_id, seq, event_type, payload_json, created_at
FROM background_agent_events
WHERE
    run_id = ?
    AND event_type IN ('agent.started', 'agent.startRecovered')
ORDER BY
    CASE WHEN event_type = 'agent.started' THEN 0 ELSE 1 END,
    seq ASC
LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(BackgroundAgentEvent::try_from)
    .transpose()?;
    let proposed_event_payload =
        background_agent_start_event_payload(start_event_payload_json, admission_identity_sha256);
    let now = Utc::now().timestamp();
    append_background_agent_admission_receipt_in_tx(tx, run_id, params, now).await?;
    let event = match existing_event {
        Some(event)
            if stored_identity.is_some()
                || legacy_background_agent_start_event_matches(
                    &event.payload_json,
                    start_event_payload_json,
                ) =>
        {
            event
        }
        Some(_) => {
            anyhow::bail!(
                "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
                 idempotency key is already bound to a different start event"
            );
        }
        None if stored_identity.is_none() => {
            super::events::append_background_agent_event_in_tx(
                tx,
                run_id,
                "agent.started",
                &proposed_event_payload,
                now,
            )
            .await?
        }
        None => {
            anyhow::bail!(
                "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
                 admitted background agent is missing its authoritative start event"
            );
        }
    };

    let existing_snapshot = sqlx::query_as::<_, (i64, String, String, Option<String>)>(
        r#"
SELECT id, payload_json, recovery_policy, config_fingerprint
FROM background_agent_execution_snapshots
WHERE run_id = ? AND snapshot_kind = 'initial_execution_context'
ORDER BY seq DESC
LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    let proposed_snapshot_payload: serde_json::Value = serde_json::from_str(
        redact_state_json_string(&execution_snapshot_params.payload_json)?.as_str(),
    )?;
    let execution_snapshot_id = match existing_snapshot {
        Some((id, payload_json, recovery_policy, config_fingerprint)) => {
            let payload_json: serde_json::Value = serde_json::from_str(payload_json.as_str())?;
            if payload_json != proposed_snapshot_payload
                || recovery_policy != execution_snapshot_params.recovery_policy
                || config_fingerprint != execution_snapshot_params.config_fingerprint
            {
                anyhow::bail!(
                    "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
                     idempotency key is already bound to a different execution snapshot"
                );
            }
            id
        }
        None => {
            insert_background_agent_execution_snapshot_in_tx(tx, &execution_snapshot_params, now)
                .await?
        }
    };

    let status_snapshot_exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM background_agent_status_snapshots WHERE run_id = ?")
            .bind(run_id)
            .fetch_optional(&mut **tx)
            .await?;
    if status_snapshot_exists.is_none() {
        let run_state = sqlx::query_as::<_, (String, String)>(
            "SELECT status, desired_state FROM background_agent_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
        let status = BackgroundAgentRunStatus::parse(run_state.0.as_str())?;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(
            tx,
            &BackgroundAgentStatusSnapshotParams {
                run_id: run_id.to_string(),
                seq: event.seq,
                status,
                desired_state: BackgroundAgentDesiredState::parse(run_state.1.as_str())?,
                summary: Some(status.as_str().to_string()),
                pending_interaction_count: 0,
                last_event_seq: event.seq,
                payload_json: serde_json::json!({"phase": status.as_str()}),
            },
            now,
        )
        .await?;
    }
    sqlx::query(
        r#"
UPDATE background_agent_runs
SET admission_identity_sha256 = ?, admission_ready_at = COALESCE(admission_ready_at, ?)
WHERE id = ?
        "#,
    )
    .bind(admission_identity_sha256)
    .bind(now)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    Ok((event, execution_snapshot_id))
}

fn legacy_background_agent_start_event_matches(
    stored: &serde_json::Value,
    proposed: &serde_json::Value,
) -> bool {
    ["prompt", "cwd", "promptSnapshotRef", "initialGoalObjective"]
        .into_iter()
        .all(|key| stored.get(key) == proposed.get(key))
}

pub(in crate::runtime) async fn validate_existing_background_agent_admission_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    idempotency_key: &str,
    params: &BackgroundAgentRunCreateParams,
    identity: ExistingBackgroundAgentAdmissionIdentity<'_>,
) -> anyhow::Result<Option<String>> {
    let existing = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        r#"
SELECT
    id,
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
    spawn_linkage_json,
    auth_profile_ref,
    config_fingerprint,
    version_fingerprint,
    admission_identity_sha256
FROM background_agent_runs
WHERE idempotency_key = ?
        "#,
    )
    .bind(background_agent_idempotency_key_digest(idempotency_key))
    .fetch_optional(&mut **tx)
    .await?;
    let Some((
        existing_id,
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
        spawn_linkage_json,
        auth_profile_ref,
        config_fingerprint,
        version_fingerprint,
        admission_identity_sha256,
    )) = existing
    else {
        return Ok(None);
    };
    if let (
        ExistingBackgroundAgentAdmissionIdentity::AdmissionDigest(requested_identity),
        Some(stored_identity),
    ) = (identity, admission_identity_sha256.as_deref())
    {
        if stored_identity != requested_identity {
            anyhow::bail!(
                "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
                 idempotency key is already bound to different prompt or execution context"
            );
        }
        return Ok(Some(existing_id));
    }
    let requested_spawn_linkage_json = params
        .spawn_linkage_json
        .as_ref()
        .map(redact_state_json_string)
        .transpose()?;
    let identity_matches = request_id == params.request_id.as_deref().map(redact_state_string)
        && source == params.source
        && prompt_snapshot_ref == params.prompt_snapshot_ref
        && input_snapshot_ref
            == params
                .input_snapshot_ref
                .as_deref()
                .map(redact_state_string)
        && thread_id == params.thread_id
        && thread_store_kind == params.thread_store_kind
        && thread_store_id == params.thread_store_id
        && rollout_path == params.rollout_path
        && parent_thread_id == params.parent_thread_id
        && parent_agent_run_id == params.parent_agent_run_id
        && spawn_linkage_json == requested_spawn_linkage_json
        && auth_profile_ref == params.auth_profile_ref.as_deref().map(redact_state_string)
        && config_fingerprint == params.config_fingerprint
        && version_fingerprint == params.version_fingerprint;
    if !identity_matches {
        anyhow::bail!(
            "{BACKGROUND_AGENT_ADMISSION_IDENTITY_MISMATCH}: \
             idempotency key is already bound to a different background agent identity"
        );
    }
    Ok(Some(existing_id))
}

async fn append_terminal_stale_background_agent_status_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    status: BackgroundAgentRunStatus,
    status_reason: &str,
    event_type: &str,
    event_payload_json: &serde_json::Value,
    now: i64,
) -> anyhow::Result<()> {
    let generation: i64 =
        sqlx::query_scalar("SELECT generation FROM background_agent_runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(&mut **tx)
            .await?;
    let receipt_key = format!("lifecycle:{generation}:{event_type}:{}", status.as_str());
    let event = super::events::append_background_agent_lifecycle_receipt_in_tx(
        tx,
        run_id,
        event_type,
        receipt_key.as_str(),
        generation,
        /*attempt*/ None,
        event_payload_json,
        now,
    )
    .await?;
    let desired_state: String =
        sqlx::query_scalar("SELECT desired_state FROM background_agent_runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(&mut **tx)
            .await?;
    let pending_interaction_count: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM background_agent_pending_interactions
WHERE run_id = ? AND status IN (?, ?)
        "#,
    )
    .bind(run_id)
    .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
    .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
    .fetch_one(&mut **tx)
    .await?;
    super::snapshots::upsert_background_agent_status_snapshot_in_tx(
        tx,
        &BackgroundAgentStatusSnapshotParams {
            run_id: run_id.to_string(),
            seq: event.seq,
            status,
            desired_state: BackgroundAgentDesiredState::parse(desired_state.as_str())?,
            summary: Some(status_reason.to_string()),
            pending_interaction_count,
            last_event_seq: event.seq,
            payload_json: serde_json::json!({
                "reason": status_reason,
                "event": event_payload_json,
            }),
        },
        now,
    )
    .await?;
    Ok(())
}

fn background_agent_status_timestamps(
    status: BackgroundAgentRunStatus,
    now: i64,
) -> (Option<i64>, Option<i64>) {
    let started_at = if matches!(
        status,
        BackgroundAgentRunStatus::Starting | BackgroundAgentRunStatus::Running
    ) {
        Some(now)
    } else {
        None
    };
    let completed_at = if matches!(
        status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    ) {
        Some(now)
    } else {
        None
    };
    (started_at, completed_at)
}

fn is_background_agent_unique_constraint_violation(err: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database_err) = err else {
        return false;
    };
    database_err.is_unique_violation()
}
