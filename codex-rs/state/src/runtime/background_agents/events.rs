use super::*;
use crate::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;
use sha2::Digest;
use sha2::Sha256;

const MAX_BACKGROUND_AGENT_RECEIPT_DIAGNOSTICS_BYTES: usize = 4 * 1024;
const MAX_BACKGROUND_AGENT_RECEIPT_DIAGNOSTICS_PREVIEW_CHARS: usize = 1_024;
const MAX_BACKGROUND_AGENT_RECEIPT_KEY_BYTES: usize = 256;
const BACKGROUND_AGENT_RECEIPT_IDENTITY_MISMATCH: &str =
    "background agent lifecycle receipt identity mismatch";

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

#[allow(clippy::too_many_arguments)]
pub(in crate::runtime) async fn append_background_agent_lifecycle_receipt_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    event_type: &str,
    receipt_key: &str,
    generation: i64,
    attempt: Option<i64>,
    diagnostics_json: &serde_json::Value,
    now: i64,
) -> anyhow::Result<BackgroundAgentEvent> {
    validate_background_agent_receipt_key(receipt_key)?;
    let operation_identity_sha256 = background_agent_receipt_operation_identity_sha256(
        event_type,
        generation,
        attempt,
        diagnostics_json,
    )?;
    let diagnostics_json = bounded_background_agent_receipt_diagnostics(diagnostics_json)?;
    if let Some(event) = get_background_agent_lifecycle_receipt_in_tx(
        tx,
        run_id,
        event_type,
        receipt_key,
        generation,
        attempt,
        &diagnostics_json,
        operation_identity_sha256.as_str(),
    )
    .await?
    {
        return Ok(event);
    }

    let payload_json = crate::redacted_local_state_json(&serde_json::json!({
        "receiptKey": receipt_key,
        "runId": run_id,
        "generation": generation,
        "attempt": attempt,
        "occurredAt": now,
        "diagnostics": diagnostics_json,
    }));
    let serialized_payload = serde_json::to_string(&payload_json)?;
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM background_agent_events WHERE run_id = ?",
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    let id = sqlx::query(
        r#"
INSERT INTO background_agent_events (
    run_id,
    seq,
    event_type,
    payload_json,
    created_at,
    receipt_key
) VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(run_id)
    .bind(seq)
    .bind(event_type)
    .bind(serialized_payload.as_str())
    .bind(now)
    .bind(receipt_key)
    .execute(&mut **tx)
    .await?
    .last_insert_rowid();
    sqlx::query(
        r#"
INSERT INTO background_agent_lifecycle_receipts (
    run_id,
    receipt_key,
    event_id,
    event_seq,
    event_type,
    generation,
    attempt,
    operation_identity_sha256,
    payload_json,
    created_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(run_id)
    .bind(receipt_key)
    .bind(id)
    .bind(seq)
    .bind(event_type)
    .bind(generation)
    .bind(attempt)
    .bind(operation_identity_sha256)
    .bind(serialized_payload)
    .bind(now)
    .execute(&mut **tx)
    .await?;

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
        payload_json,
        created_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::runtime) async fn get_background_agent_lifecycle_receipt_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    event_type: &str,
    receipt_key: &str,
    generation: i64,
    attempt: Option<i64>,
    diagnostics_json: &serde_json::Value,
    operation_identity_sha256: &str,
) -> anyhow::Result<Option<BackgroundAgentEvent>> {
    validate_background_agent_receipt_key(receipt_key)?;
    let row = sqlx::query_as::<_, (i64, i64, String, i64, Option<i64>, String, String, i64)>(
        r#"
SELECT
    event_id,
    event_seq,
    event_type,
    generation,
    attempt,
    operation_identity_sha256,
    payload_json,
    created_at
FROM background_agent_lifecycle_receipts
WHERE run_id = ? AND receipt_key = ?
        "#,
    )
    .bind(run_id)
    .bind(receipt_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((
        event_id,
        event_seq,
        stored_event_type,
        stored_generation,
        stored_attempt,
        stored_operation_identity_sha256,
        payload_json,
        created_at,
    )) = row
    else {
        return Ok(None);
    };
    let payload_json: serde_json::Value = serde_json::from_str(payload_json.as_str())?;
    let legacy_identity_matches = stored_operation_identity_sha256.is_empty()
        && payload_json.get("diagnostics") == Some(diagnostics_json);
    if stored_event_type != event_type
        || stored_generation != generation
        || stored_attempt != attempt
        || (stored_operation_identity_sha256 != operation_identity_sha256
            && !legacy_identity_matches)
    {
        anyhow::bail!(
            "{BACKGROUND_AGENT_RECEIPT_IDENTITY_MISMATCH}: \
             receipt key is already bound to a different lifecycle operation"
        );
    }
    let created_at = DateTime::<Utc>::from_timestamp(created_at, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid unix timestamp: {created_at}"))?;
    Ok(Some(BackgroundAgentEvent {
        id: event_id,
        run_id: run_id.to_string(),
        seq: event_seq,
        event_type: stored_event_type,
        payload_json,
        created_at,
    }))
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

    #[allow(clippy::too_many_arguments)]
    pub async fn append_background_agent_event_for_supervisor(
        &self,
        run_id: &str,
        supervisor_id: &str,
        generation: i64,
        event_type: &str,
        payload_json: &serde_json::Value,
        allow_terminal_current: bool,
    ) -> anyhow::Result<Option<BackgroundAgentEvent>> {
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
    AND (
        ? = 1
        OR status IN (
            'starting',
            'running',
            'waiting_on_approval',
            'waiting_on_user',
            'stopping'
        )
    )
            "#,
        )
        .bind(run_id)
        .bind(supervisor_id)
        .bind(generation)
        .bind(allow_terminal_current)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(_) = current else {
            tx.commit().await?;
            return Ok(None);
        };
        let event =
            append_background_agent_event_in_tx(&mut tx, run_id, event_type, payload_json, now)
                .await?;
        tx.commit().await?;
        Ok(Some(event))
    }

    pub async fn append_background_agent_lifecycle_receipt(
        &self,
        run_id: &str,
        event_type: &str,
        receipt_key: &str,
        generation: i64,
        attempt: Option<i64>,
        diagnostics_json: &serde_json::Value,
    ) -> anyhow::Result<BackgroundAgentEvent> {
        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let event = append_background_agent_lifecycle_receipt_in_tx(
            &mut tx,
            run_id,
            event_type,
            receipt_key,
            generation,
            attempt,
            diagnostics_json,
            now,
        )
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

fn validate_background_agent_receipt_key(receipt_key: &str) -> anyhow::Result<()> {
    if receipt_key.len() > MAX_BACKGROUND_AGENT_RECEIPT_KEY_BYTES {
        anyhow::bail!(
            "background agent lifecycle receipt key exceeds \
             {MAX_BACKGROUND_AGENT_RECEIPT_KEY_BYTES} bytes"
        );
    }
    Ok(())
}

pub(in crate::runtime) fn background_agent_receipt_operation_identity_sha256(
    event_type: &str,
    generation: i64,
    attempt: Option<i64>,
    diagnostics_json: &serde_json::Value,
) -> anyhow::Result<String> {
    let identity = serde_json::json!({
        "eventType": event_type,
        "generation": generation,
        "attempt": attempt,
        "diagnostics": diagnostics_json,
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity)?)
    ))
}

pub(in crate::runtime) fn bounded_background_agent_receipt_diagnostics(
    diagnostics_json: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let diagnostics_json = crate::redacted_local_state_json(diagnostics_json);
    let serialized = serde_json::to_string(&diagnostics_json)?;
    if serialized.len() <= MAX_BACKGROUND_AGENT_RECEIPT_DIAGNOSTICS_BYTES {
        return Ok(diagnostics_json);
    }
    let preview = serialized
        .chars()
        .take(MAX_BACKGROUND_AGENT_RECEIPT_DIAGNOSTICS_PREVIEW_CHARS)
        .collect::<String>();
    Ok(serde_json::json!({
        "truncated": true,
        "originalBytes": serialized.len(),
        "preview": preview,
    }))
}
