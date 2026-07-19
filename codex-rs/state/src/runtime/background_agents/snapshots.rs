use super::*;

pub(super) async fn refresh_background_agent_status_snapshot_pending_count_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
UPDATE background_agent_status_snapshots
SET
    pending_interaction_count = (
        SELECT COUNT(*)
        FROM background_agent_pending_interactions
        WHERE run_id = ?
          AND status IN (?, ?)
    ),
    last_event_seq = (
        SELECT last_event_seq
        FROM background_agent_runs
        WHERE id = ?
    ),
    updated_at = ?
WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
    .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
    .bind(run_id)
    .bind(now)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl StateRuntime {
    pub async fn upsert_background_agent_status_snapshot(
        &self,
        params: &BackgroundAgentStatusSnapshotParams,
    ) -> anyhow::Result<BackgroundAgentStatusSnapshot> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        upsert_background_agent_status_snapshot_in_tx(&mut tx, params, now).await?;
        tx.commit().await?;

        self.get_background_agent_status_snapshot(params.run_id.as_str())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load background agent status snapshot for run {}",
                    params.run_id
                )
            })
    }

    pub async fn get_background_agent_status_snapshot(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentStatusSnapshot>> {
        let row = sqlx::query_as::<_, BackgroundAgentStatusSnapshotRow>(
            r#"
SELECT
    run_id,
    seq,
    status,
    desired_state,
    summary,
    pending_interaction_count,
    last_event_seq,
    payload_json,
    updated_at
FROM background_agent_status_snapshots
WHERE run_id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentStatusSnapshot::try_from).transpose()
    }

    pub async fn create_background_agent_execution_snapshot(
        &self,
        params: &BackgroundAgentExecutionSnapshotParams,
    ) -> anyhow::Result<BackgroundAgentExecutionSnapshot> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let snapshot =
            create_background_agent_execution_snapshot_in_tx(&mut tx, params, now).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    pub async fn get_latest_background_agent_execution_snapshot(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentExecutionSnapshot>> {
        let mut tx = self.pool.begin().await?;
        let snapshot =
            get_latest_background_agent_execution_snapshot_in_tx(&mut tx, run_id).await?;
        tx.commit().await?;
        Ok(snapshot)
    }
}

pub(super) async fn get_background_agent_status_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<BackgroundAgentStatusSnapshot>> {
    let row = sqlx::query_as::<_, BackgroundAgentStatusSnapshotRow>(
        r#"
SELECT
    run_id,
    seq,
    status,
    desired_state,
    summary,
    pending_interaction_count,
    last_event_seq,
    payload_json,
    updated_at
FROM background_agent_status_snapshots
WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(BackgroundAgentStatusSnapshot::try_from).transpose()
}

pub(super) async fn create_background_agent_execution_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &BackgroundAgentExecutionSnapshotParams,
    now: i64,
) -> anyhow::Result<BackgroundAgentExecutionSnapshot> {
    let payload_json = serde_json::to_string(&params.payload_json)?;
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

    get_background_agent_execution_snapshot_in_tx(tx, id)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "failed to load background agent execution snapshot {id} for run {}",
                params.run_id
            )
        })
}

pub(super) async fn get_latest_background_agent_execution_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
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
WHERE run_id = ?
ORDER BY seq DESC
LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(BackgroundAgentExecutionSnapshot::try_from)
        .transpose()
}

async fn get_background_agent_execution_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    snapshot_id: i64,
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
WHERE id = ?
        "#,
    )
    .bind(snapshot_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(BackgroundAgentExecutionSnapshot::try_from)
        .transpose()
}

pub(super) async fn upsert_background_agent_status_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &BackgroundAgentStatusSnapshotParams,
    now: i64,
) -> anyhow::Result<()> {
    let payload_json = serde_json::to_string(&params.payload_json)?;
    sqlx::query(
        r#"
INSERT INTO background_agent_status_snapshots (
    run_id,
    seq,
    status,
    desired_state,
    summary,
    pending_interaction_count,
    last_event_seq,
    payload_json,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(run_id) DO UPDATE SET
    seq = excluded.seq,
    status = excluded.status,
    desired_state = excluded.desired_state,
    summary = excluded.summary,
    pending_interaction_count = excluded.pending_interaction_count,
    last_event_seq = excluded.last_event_seq,
    payload_json = excluded.payload_json,
    updated_at = excluded.updated_at
            "#,
    )
    .bind(params.run_id.as_str())
    .bind(params.seq)
    .bind(params.status.as_str())
    .bind(params.desired_state.as_str())
    .bind(params.summary.as_deref())
    .bind(params.pending_interaction_count)
    .bind(params.last_event_seq)
    .bind(payload_json)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
UPDATE background_agent_runs
SET last_snapshot_seq = ?, updated_at = ?
WHERE id = ?
            "#,
    )
    .bind(params.seq)
    .bind(now)
    .bind(params.run_id.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
