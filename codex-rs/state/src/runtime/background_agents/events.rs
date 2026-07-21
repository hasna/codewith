use super::*;
use crate::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;

pub(in crate::runtime) async fn append_background_agent_event_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    event_type: &str,
    payload_json: &serde_json::Value,
    now: i64,
) -> anyhow::Result<BackgroundAgentEvent> {
    let event_payload_json = crate::redacted_local_state_json(payload_json);
    let payload_json = serde_json::to_string(&event_payload_json)?;
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM background_agent_events WHERE run_id = ?",
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    let id = sqlx::query(
        r#"
INSERT INTO background_agent_events (run_id, seq, event_type, payload_json, created_at)
VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(run_id)
    .bind(seq)
    .bind(event_type)
    .bind(payload_json.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await?
    .last_insert_rowid();

    sqlx::query(
        r#"
UPDATE background_agent_runs
SET last_event_seq = ?, updated_at = ?
WHERE id = ?
        "#,
    )
    .bind(seq)
    .bind(now)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    let created_at = DateTime::<Utc>::from_timestamp(now, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid unix timestamp: {now}"))?;
    Ok(BackgroundAgentEvent {
        id,
        run_id: run_id.to_string(),
        seq,
        event_type: event_type.to_string(),
        payload_json: event_payload_json,
        created_at,
    })
}

impl StateRuntime {
    pub async fn append_background_agent_event(
        &self,
        run_id: &str,
        event_type: &str,
        payload_json: &serde_json::Value,
    ) -> anyhow::Result<BackgroundAgentEvent> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let event =
            append_background_agent_event_in_tx(&mut tx, run_id, event_type, payload_json, now)
                .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn list_background_agent_events_after(
        &self,
        run_id: &str,
        after_seq: Option<i64>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<BackgroundAgentEvent>> {
        if let Some(after_seq) = after_seq {
            self.ensure_background_agent_event_cursor_retained(run_id, after_seq)
                .await?;
        }
        let after_seq = after_seq.unwrap_or(0);
        let limit = limit.unwrap_or(100).min(500);
        let rows = sqlx::query_as::<_, BackgroundAgentEventRow>(
            r#"
SELECT id, run_id, seq, event_type, payload_json, created_at
FROM background_agent_events
WHERE run_id = ? AND seq > ?
ORDER BY seq ASC
LIMIT ?
            "#,
        )
        .bind(run_id)
        .bind(after_seq)
        .bind(limit as i64)
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.into_iter()
            .map(BackgroundAgentEvent::try_from)
            .collect()
    }

    pub async fn compact_background_agent_events_before_seq(
        &self,
        run_id: &str,
        before_seq: i64,
    ) -> anyhow::Result<usize> {
        let result = sqlx::query(
            r#"
DELETE FROM background_agent_events
WHERE run_id = ? AND seq < ?
            "#,
        )
        .bind(run_id)
        .bind(before_seq)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() as usize)
    }

    async fn ensure_background_agent_event_cursor_retained(
        &self,
        run_id: &str,
        after_seq: i64,
    ) -> anyhow::Result<()> {
        let row = sqlx::query_as::<_, (i64, Option<i64>)>(
            r#"
SELECT
    r.last_event_seq,
    MIN(e.seq)
FROM background_agent_runs r
LEFT JOIN background_agent_events e ON e.run_id = r.id
WHERE r.id = ?
GROUP BY r.id
            "#,
        )
        .bind(run_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        let Some((last_event_seq, first_retained_seq)) = row else {
            return Ok(());
        };
        match first_retained_seq {
            Some(first_retained_seq) if after_seq < first_retained_seq.saturating_sub(1) => {
                anyhow::bail!(
                    "{BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED} for run {run_id}: requested after seq {after_seq}, earliest retained seq is {first_retained_seq}"
                );
            }
            None if after_seq < last_event_seq => {
                anyhow::bail!(
                    "{BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED} for run {run_id}: requested after seq {after_seq}, no events remain before last seq {last_event_seq}"
                );
            }
            _ => {}
        }
        Ok(())
    }
}
