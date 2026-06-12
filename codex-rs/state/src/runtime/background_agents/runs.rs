use super::*;

impl StateRuntime {
    pub async fn create_background_agent_run(
        &self,
        params: &BackgroundAgentRunCreateParams,
    ) -> anyhow::Result<BackgroundAgentRun> {
        if let Some(idempotency_key) = params.idempotency_key.as_deref()
            && let Some(existing) = self
                .get_background_agent_run_by_idempotency_key(idempotency_key)
                .await?
        {
            return Ok(existing);
        }
        if let Some(request_id) = params.request_id.as_deref()
            && let Some(existing) = self
                .get_background_agent_run_by_request_id(request_id)
                .await?
        {
            return Ok(existing);
        }

        let now = Utc::now().timestamp();
        let spawn_linkage_json = params
            .spawn_linkage_json
            .as_ref()
            .map(serde_json::to_string)
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
        .execute(self.pool.as_ref())
        .await;
        if let Err(err) = insert_result {
            if params.idempotency_key.is_some()
                && is_background_agent_unique_constraint_violation(&err)
                && let Some(idempotency_key) = params.idempotency_key.as_deref()
                && let Some(existing) = self
                    .get_background_agent_run_by_idempotency_key(idempotency_key)
                    .await?
            {
                return Ok(existing);
            }
            if params.request_id.is_some()
                && is_background_agent_unique_constraint_violation(&err)
                && let Some(request_id) = params.request_id.as_deref()
                && let Some(existing) = self
                    .get_background_agent_run_by_request_id(request_id)
                    .await?
            {
                return Ok(existing);
            }
            return Err(err.into());
        }

        self.get_background_agent_run(params.id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("failed to load background agent run {}", params.id))
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
            "#,
        )
        .bind(limit as i64)
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
WHERE retention_state = 'active'
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
WHERE id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(status_reason)
        .bind(now)
        .bind(started_at)
        .bind(completed_at)
        .bind(run_id)
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
        .bind(status_reason)
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
        let mut tx = self.pool.begin().await?;
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
        .bind(params.status_reason)
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

        let event_id = super::events::append_background_agent_event_in_tx(
            &mut tx,
            params.run_id,
            params.event_type,
            params.event_payload_json,
            now,
        )
        .await?;
        let (last_event_seq, desired_state): (i64, String) = sqlx::query_as(
            "SELECT last_event_seq, desired_state FROM background_agent_runs WHERE id = ?",
        )
        .bind(params.run_id)
        .fetch_one(&mut *tx)
        .await?;
        super::snapshots::upsert_background_agent_status_snapshot_in_tx(
            &mut tx,
            &BackgroundAgentStatusSnapshotParams {
                run_id: params.run_id.to_string(),
                seq: last_event_seq,
                status: params.status,
                desired_state: BackgroundAgentDesiredState::parse(desired_state.as_str())?,
                summary: params.summary.map(str::to_string),
                pending_interaction_count: params.pending_interaction_count,
                last_event_seq,
                payload_json: params.status_payload_json.clone(),
            },
            now,
        )
        .await?;
        tx.commit().await?;

        self.get_background_agent_event(event_id).await
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
        let result = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    desired_state = ?,
    retention_state = ?,
    delete_after = COALESCE(delete_after, ?),
    updated_at = ?
WHERE id = ? AND retention_state != ?
            "#,
        )
        .bind(BackgroundAgentDesiredState::Deleted.as_str())
        .bind(crate::BackgroundAgentRetentionState::DeleteRequested.as_str())
        .bind(now)
        .bind(now)
        .bind(run_id)
        .bind(crate::BackgroundAgentRetentionState::Deleted.as_str())
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
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
            .execute(self.pool.as_ref())
            .await?;

            if result.rows_affected() == 0 {
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
            .execute(self.pool.as_ref())
            .await?;

            self.append_background_agent_event(
                run_id.as_str(),
                "agent.orphaned",
                &serde_json::json!({
                    "reason": "supervisor_heartbeat_stale",
                    "previousSupervisorId": supervisor_id,
                    "generation": generation,
                    "staleBefore": stale_before,
                }),
            )
            .await?;
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
            .execute(self.pool.as_ref())
            .await?;

            if result.rows_affected() == 0 {
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
            .execute(self.pool.as_ref())
            .await?;

            self.append_background_agent_event(
                run_id.as_str(),
                "agent.cancelled",
                &serde_json::json!({
                    "reason": "stop_heartbeat_stale",
                    "previousSupervisorId": supervisor_id,
                    "generation": generation,
                    "staleBefore": stale_before,
                }),
            )
            .await?;
            finalized += 1;
        }

        Ok(finalized)
    }

    pub async fn claim_background_agent_supervisor(
        &self,
        run_id: &str,
        supervisor_id: &str,
        process_lease_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let current: Option<(i64, String, String, String)> = sqlx::query_as(
            r#"
SELECT generation, desired_state, status, retention_state
FROM background_agent_runs
WHERE id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((current_generation, desired_state, status, retention_state)) = current else {
            tx.rollback().await?;
            return Ok(None);
        };
        let eligible = desired_state == BackgroundAgentDesiredState::Running.as_str()
            && retention_state == crate::BackgroundAgentRetentionState::Active.as_str()
            && matches!(status.as_str(), "queued" | "orphaned");
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
        .bind(idempotency_key)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentRun::try_from).transpose()
    }

    pub async fn get_background_agent_run_by_request_id(
        &self,
        request_id: &str,
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
WHERE request_id = ?
            "#,
        )
        .bind(request_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentRun::try_from).transpose()
    }
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
