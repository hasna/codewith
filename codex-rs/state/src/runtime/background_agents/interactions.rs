use super::events::append_background_agent_event_in_tx;
use super::snapshots::refresh_background_agent_status_snapshot_pending_count_in_tx;
use super::*;

impl StateRuntime {
    pub async fn create_background_agent_pending_interaction_for_supervisor(
        &self,
        params: &BackgroundAgentPendingInteractionCreateParams,
        supervisor_id: &str,
        generation: i64,
        waiting_status: BackgroundAgentRunStatus,
    ) -> anyhow::Result<Option<BackgroundAgentPendingInteraction>> {
        let now = Utc::now().timestamp();
        let request_payload_json = redact_state_json_string(&params.request_payload_json)?;
        let timeout_at = params.timeout_at.map(|timestamp| timestamp.timestamp());
        let mut tx = self.pool.begin().await?;
        let status_update = sqlx::query(
            r#"
UPDATE background_agent_runs
SET status = ?, status_reason = ?, updated_at = ?
WHERE
    id = ?
    AND supervisor_id = ?
    AND generation = ?
    AND desired_state = ?
    AND retention_state = ?
    AND status IN ('starting', 'running', 'waiting_on_approval', 'waiting_on_user')
            "#,
        )
        .bind(waiting_status.as_str())
        .bind("waiting for pending interaction")
        .bind(now)
        .bind(params.run_id.as_str())
        .bind(supervisor_id)
        .bind(generation)
        .bind(BackgroundAgentDesiredState::Running.as_str())
        .bind(crate::BackgroundAgentRetentionState::Active.as_str())
        .execute(&mut *tx)
        .await?;
        if status_update.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }

        sqlx::query(
            r#"
INSERT INTO background_agent_pending_interactions (
    id,
    run_id,
    worker_request_id,
    kind,
    status,
    request_payload_json,
    no_client_policy,
    timeout_at,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(params.id.as_str())
        .bind(params.run_id.as_str())
        .bind(params.worker_request_id.as_deref())
        .bind(params.kind.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(request_payload_json)
        .bind(params.no_client_policy.as_str())
        .bind(timeout_at)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        append_background_agent_event_in_tx(
            &mut tx,
            params.run_id.as_str(),
            "interaction.created",
            &serde_json::json!({
                "interactionId": params.id,
                "workerRequestId": params.worker_request_id,
                "kind": params.kind.as_str(),
                "status": BackgroundAgentPendingInteractionStatus::Pending.as_str(),
                "noClientPolicy": params.no_client_policy.as_str(),
                "timeoutAt": timeout_at,
            }),
            now,
        )
        .await?;
        refresh_background_agent_status_snapshot_pending_count_in_tx(
            &mut tx,
            params.run_id.as_str(),
            now,
        )
        .await?;
        tx.commit().await?;

        self.get_background_agent_pending_interaction(params.id.as_str())
            .await?
            .map(Some)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load background agent pending interaction {}",
                    params.id
                )
            })
    }

    pub async fn create_background_agent_pending_interaction(
        &self,
        params: &BackgroundAgentPendingInteractionCreateParams,
    ) -> anyhow::Result<BackgroundAgentPendingInteraction> {
        let now = Utc::now().timestamp();
        let request_payload_json = redact_state_json_string(&params.request_payload_json)?;
        let timeout_at = params.timeout_at.map(|timestamp| timestamp.timestamp());
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
INSERT INTO background_agent_pending_interactions (
    id,
    run_id,
    worker_request_id,
    kind,
    status,
    request_payload_json,
    no_client_policy,
    timeout_at,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(params.id.as_str())
        .bind(params.run_id.as_str())
        .bind(params.worker_request_id.as_deref())
        .bind(params.kind.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(request_payload_json)
        .bind(params.no_client_policy.as_str())
        .bind(timeout_at)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        append_background_agent_event_in_tx(
            &mut tx,
            params.run_id.as_str(),
            "interaction.created",
            &serde_json::json!({
                "interactionId": params.id,
                "workerRequestId": params.worker_request_id,
                "kind": params.kind.as_str(),
                "status": BackgroundAgentPendingInteractionStatus::Pending.as_str(),
                "noClientPolicy": params.no_client_policy,
                "timeoutAt": timeout_at,
            }),
            now,
        )
        .await?;
        refresh_background_agent_status_snapshot_pending_count_in_tx(
            &mut tx,
            params.run_id.as_str(),
            now,
        )
        .await?;
        tx.commit().await?;

        self.get_background_agent_pending_interaction(params.id.as_str())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load background agent pending interaction {}",
                    params.id
                )
            })
    }

    pub async fn get_background_agent_pending_interaction(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentPendingInteraction>> {
        let row = sqlx::query_as::<_, BackgroundAgentPendingInteractionRow>(
            r#"
SELECT
    id,
    run_id,
    worker_request_id,
    kind,
    status,
    request_payload_json,
    response_payload_json,
    no_client_policy,
    timeout_at,
    created_at,
    delivered_at,
    responded_at,
    updated_at
FROM background_agent_pending_interactions
WHERE id = ?
            "#,
        )
        .bind(interaction_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentPendingInteraction::try_from)
            .transpose()
    }

    pub async fn list_background_agent_pending_interactions(
        &self,
        run_id: &str,
        status: Option<BackgroundAgentPendingInteractionStatus>,
    ) -> anyhow::Result<Vec<BackgroundAgentPendingInteraction>> {
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
    id,
    run_id,
    worker_request_id,
    kind,
    status,
    request_payload_json,
    response_payload_json,
    no_client_policy,
    timeout_at,
    created_at,
    delivered_at,
    responded_at,
    updated_at
FROM background_agent_pending_interactions
WHERE run_id =
            "#,
        );
        builder.push_bind(run_id);
        if let Some(status) = status {
            builder.push(" AND status = ");
            builder.push_bind(status.as_str());
        }
        builder.push(" ORDER BY created_at ASC, id ASC");
        let rows: Vec<BackgroundAgentPendingInteractionRow> = builder
            .build_query_as::<BackgroundAgentPendingInteractionRow>()
            .fetch_all(self.pool.as_ref())
            .await?;
        rows.into_iter()
            .map(BackgroundAgentPendingInteraction::try_from)
            .collect()
    }

    pub async fn count_background_agent_pending_interactions(
        &self,
        status: Option<BackgroundAgentPendingInteractionStatus>,
    ) -> anyhow::Result<i64> {
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT COUNT(*)
FROM background_agent_pending_interactions
            "#,
        );
        if let Some(status) = status {
            builder.push(" WHERE status = ");
            builder.push_bind(status.as_str());
        }
        let (count,): (i64,) = builder
            .build_query_as()
            .fetch_one(self.pool.as_ref())
            .await?;
        Ok(count)
    }

    pub async fn mark_background_agent_pending_interaction_delivered(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let row: Option<(String, String)> = sqlx::query_as(
            r#"
SELECT run_id, kind
FROM background_agent_pending_interactions
WHERE id = ? AND status = ?
            "#,
        )
        .bind(interaction_id)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((run_id, kind)) = row else {
            tx.rollback().await?;
            return Ok(false);
        };
        let result = sqlx::query(
            r#"
UPDATE background_agent_pending_interactions
SET status = ?, delivered_at = COALESCE(delivered_at, ?), updated_at = ?
WHERE id = ? AND status = ?
            "#,
        )
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .bind(now)
        .bind(now)
        .bind(interaction_id)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        append_background_agent_event_in_tx(
            &mut tx,
            run_id.as_str(),
            "interaction.delivered",
            &serde_json::json!({
                "interactionId": interaction_id,
                "kind": kind,
                "status": BackgroundAgentPendingInteractionStatus::Delivered.as_str(),
            }),
            now,
        )
        .await?;
        refresh_background_agent_status_snapshot_pending_count_in_tx(&mut tx, run_id.as_str(), now)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn respond_background_agent_pending_interaction(
        &self,
        interaction_id: &str,
        response_payload_json: &serde_json::Value,
        terminal_status: BackgroundAgentPendingInteractionStatus,
    ) -> anyhow::Result<bool> {
        if matches!(
            terminal_status,
            BackgroundAgentPendingInteractionStatus::Pending
                | BackgroundAgentPendingInteractionStatus::Delivered
        ) {
            anyhow::bail!("background agent pending interaction response must be terminal");
        }

        let now = Utc::now().timestamp();
        let response_payload_json = redact_state_json_string(response_payload_json)?;
        let mut tx = self.pool.begin().await?;
        let row: Option<(String, String)> = sqlx::query_as(
            r#"
SELECT run_id, kind
FROM background_agent_pending_interactions
WHERE id = ? AND status IN (?, ?)
            "#,
        )
        .bind(interaction_id)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((run_id, kind)) = row else {
            tx.rollback().await?;
            return Ok(false);
        };
        let result = sqlx::query(
            r#"
UPDATE background_agent_pending_interactions
SET status = ?, response_payload_json = ?, responded_at = ?, updated_at = ?
WHERE id = ? AND status IN (?, ?)
            "#,
        )
        .bind(terminal_status.as_str())
        .bind(response_payload_json)
        .bind(now)
        .bind(now)
        .bind(interaction_id)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        append_background_agent_event_in_tx(
            &mut tx,
            run_id.as_str(),
            pending_interaction_event_type(terminal_status),
            &serde_json::json!({
                "interactionId": interaction_id,
                "kind": kind,
                "status": terminal_status.as_str(),
            }),
            now,
        )
        .await?;
        refresh_background_agent_status_snapshot_pending_count_in_tx(&mut tx, run_id.as_str(), now)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn expire_background_agent_pending_interactions(&self) -> anyhow::Result<usize> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            r#"
SELECT id, run_id, kind, no_client_policy
FROM background_agent_pending_interactions
WHERE timeout_at IS NOT NULL
  AND timeout_at <= ?
  AND status IN (?, ?)
ORDER BY timeout_at ASC, created_at ASC, id ASC
            "#,
        )
        .bind(now)
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .fetch_all(&mut *tx)
        .await?;
        let mut expired = 0;
        for (interaction_id, run_id, kind, no_client_policy) in rows.iter() {
            let response_payload_json = redact_state_json_string(&serde_json::json!({
                "reason": "timeout",
                "noClientPolicy": no_client_policy,
            }))?;
            let result = sqlx::query(
                r#"
UPDATE background_agent_pending_interactions
SET status = ?, response_payload_json = ?, responded_at = ?, updated_at = ?
WHERE id = ? AND status IN (?, ?)
                "#,
            )
            .bind(BackgroundAgentPendingInteractionStatus::Expired.as_str())
            .bind(response_payload_json)
            .bind(now)
            .bind(now)
            .bind(interaction_id.as_str())
            .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
            .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                continue;
            }
            expired += 1;
            append_background_agent_event_in_tx(
                &mut tx,
                run_id.as_str(),
                "interaction.expired",
                &serde_json::json!({
                    "interactionId": interaction_id,
                    "kind": kind,
                    "status": BackgroundAgentPendingInteractionStatus::Expired.as_str(),
                    "reason": "timeout",
                    "noClientPolicy": no_client_policy,
                }),
                now,
            )
            .await?;
            refresh_background_agent_status_snapshot_pending_count_in_tx(
                &mut tx,
                run_id.as_str(),
                now,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(expired)
    }
}

pub(super) async fn terminalize_active_background_agent_pending_interactions_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    terminal_status: BackgroundAgentPendingInteractionStatus,
    response_payload_json: &serde_json::Value,
    now: i64,
) -> anyhow::Result<usize> {
    if matches!(
        terminal_status,
        BackgroundAgentPendingInteractionStatus::Pending
            | BackgroundAgentPendingInteractionStatus::Delivered
    ) {
        anyhow::bail!("background agent pending interaction response must be terminal");
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        r#"
SELECT id, kind
FROM background_agent_pending_interactions
WHERE run_id = ? AND status IN (?, ?)
ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(run_id)
    .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
    .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
    .fetch_all(&mut **tx)
    .await?;
    let response_payload_json = redact_state_json_string(response_payload_json)?;
    let mut terminalized = 0;
    for (interaction_id, kind) in rows {
        let result = sqlx::query(
            r#"
UPDATE background_agent_pending_interactions
SET status = ?, response_payload_json = ?, responded_at = ?, updated_at = ?
WHERE id = ? AND status IN (?, ?)
            "#,
        )
        .bind(terminal_status.as_str())
        .bind(response_payload_json.as_str())
        .bind(now)
        .bind(now)
        .bind(interaction_id.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
        .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() == 0 {
            continue;
        }
        terminalized += 1;
        append_background_agent_event_in_tx(
            tx,
            run_id,
            pending_interaction_event_type(terminal_status),
            &serde_json::json!({
                "interactionId": interaction_id,
                "kind": kind,
                "status": terminal_status.as_str(),
            }),
            now,
        )
        .await?;
    }
    Ok(terminalized)
}

fn pending_interaction_event_type(status: BackgroundAgentPendingInteractionStatus) -> &'static str {
    match status {
        BackgroundAgentPendingInteractionStatus::Responded => "interaction.responded",
        BackgroundAgentPendingInteractionStatus::Expired => "interaction.expired",
        BackgroundAgentPendingInteractionStatus::Cancelled => "interaction.cancelled",
        BackgroundAgentPendingInteractionStatus::Denied => "interaction.denied",
        BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting => {
            "interaction.workerNoLongerWaiting"
        }
        BackgroundAgentPendingInteractionStatus::Pending
        | BackgroundAgentPendingInteractionStatus::Delivered => {
            unreachable!("non-terminal pending interaction status")
        }
    }
}
