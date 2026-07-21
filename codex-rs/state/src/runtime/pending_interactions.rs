use super::*;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

pub const DEFAULT_PENDING_INTERACTION_LIST_LIMIT: u32 = 100;
pub const MAX_PENDING_INTERACTION_LIST_LIMIT: u32 = 500;

#[derive(Debug, Clone)]
pub struct PendingInteractionListParams {
    pub thread_id: Option<ThreadId>,
    pub statuses: Vec<PendingInteractionStatus>,
    pub kinds: Vec<PendingInteractionKind>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingInteractionPage {
    pub data: Vec<PendingInteraction>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingInteractionRespondForSourceParams {
    pub thread_id: ThreadId,
    pub source_kind: PendingInteractionSourceKind,
    pub source_id: String,
    pub kinds: Vec<PendingInteractionKind>,
    pub response_payload_json: Value,
    pub response_payload_preview: String,
    pub response_redactions_json: Value,
    pub terminal_status: PendingInteractionStatus,
}

#[derive(Clone, Copy)]
enum PendingInteractionInsertMode {
    Insert,
    InsertOrIgnore,
}

impl StateRuntime {
    pub async fn create_thread_pending_interaction(
        &self,
        params: &PendingInteractionCreateParams,
    ) -> anyhow::Result<PendingInteraction> {
        self.insert_thread_pending_interaction(params, PendingInteractionInsertMode::Insert)
            .await
    }

    pub async fn create_thread_pending_interaction_if_absent(
        &self,
        params: &PendingInteractionCreateParams,
    ) -> anyhow::Result<PendingInteraction> {
        if params.worker_request_id.is_none() {
            anyhow::bail!("idempotent pending interaction creation requires worker_request_id");
        }
        self.insert_thread_pending_interaction(params, PendingInteractionInsertMode::InsertOrIgnore)
            .await
    }

    async fn insert_thread_pending_interaction(
        &self,
        params: &PendingInteractionCreateParams,
        mode: PendingInteractionInsertMode,
    ) -> anyhow::Result<PendingInteraction> {
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let timeout_at_ms = params.timeout_at.map(datetime_to_epoch_millis);
        let request_payload_json = redact_state_json_string(&params.request_payload_json)?;
        let request_payload_sha256 = payload_sha256(request_payload_json.as_bytes());
        let request_redactions_json = redact_state_json_string(&params.request_redactions_json)?;
        let server_request_id_json = params
            .server_request_id_json
            .as_ref()
            .map(redact_state_json_string)
            .transpose()?;
        let request_payload_preview = redact_state_string(params.request_payload_preview.as_str());
        let thread_id = params.thread_id.to_string();
        let insert_sql = match mode {
            PendingInteractionInsertMode::Insert => {
                r#"
INSERT INTO thread_pending_interactions (
    interaction_id,
    thread_id,
    source_kind,
    source_id,
    turn_id,
    worker_request_id,
    server_request_id_json,
    kind,
    status,
    request_payload_json,
    request_payload_sha256,
    request_payload_preview,
    request_redactions_json,
    no_client_policy,
    timeout_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#
            }
            PendingInteractionInsertMode::InsertOrIgnore => {
                r#"
INSERT OR IGNORE INTO thread_pending_interactions (
    interaction_id,
    thread_id,
    source_kind,
    source_id,
    turn_id,
    worker_request_id,
    server_request_id_json,
    kind,
    status,
    request_payload_json,
    request_payload_sha256,
    request_payload_preview,
    request_redactions_json,
    no_client_policy,
    timeout_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#
            }
        };
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(insert_sql)
            .bind(params.interaction_id.as_str())
            .bind(thread_id.as_str())
            .bind(params.source_kind.as_str())
            .bind(params.source_id.as_deref())
            .bind(params.turn_id.as_deref())
            .bind(params.worker_request_id.as_deref())
            .bind(server_request_id_json)
            .bind(params.kind.as_str())
            .bind(PendingInteractionStatus::Pending.as_str())
            .bind(request_payload_json)
            .bind(request_payload_sha256)
            .bind(request_payload_preview.as_str())
            .bind(request_redactions_json)
            .bind(params.no_client_policy.as_str())
            .bind(timeout_at_ms)
            .bind(now_ms)
            .bind(now_ms)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() > 0 {
            insert_pending_interaction_event_in_tx(
                &mut tx,
                PendingInteractionEventInsert {
                    interaction_id: params.interaction_id.as_str(),
                    thread_id: thread_id.as_str(),
                    event_kind: PendingInteractionEventKind::Created,
                    status: PendingInteractionStatus::Pending,
                    payload_json: &params.request_payload_json,
                    payload_preview: request_payload_preview.as_str(),
                    redactions_json: &params.request_redactions_json,
                    created_at_ms: now_ms,
                },
            )
            .await?;
        }
        tx.commit().await?;

        if result.rows_affected() == 0 {
            let Some(worker_request_id) = params.worker_request_id.as_deref() else {
                anyhow::bail!("ignored pending interaction insert has no worker_request_id");
            };
            return self
                .get_thread_pending_interaction_by_worker_request(
                    params.thread_id,
                    params.kind,
                    worker_request_id,
                )
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "failed to load existing pending interaction for worker request {worker_request_id}"
                    )
                });
        }
        self.get_thread_pending_interaction(params.interaction_id.as_str())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load pending interaction {}",
                    params.interaction_id
                )
            })
    }

    pub async fn get_thread_pending_interaction(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<Option<PendingInteraction>> {
        let row = sqlx::query(
            r#"
SELECT
    interaction_id,
    thread_id,
    source_kind,
    source_id,
    turn_id,
    worker_request_id,
    server_request_id_json,
    kind,
    status,
    request_payload_json,
    request_payload_sha256,
    request_payload_preview,
    request_redactions_json,
    response_payload_json,
    response_payload_sha256,
    response_payload_preview,
    response_redactions_json,
    no_client_policy,
    timeout_at_ms,
    created_at_ms,
    delivered_at_ms,
    responded_at_ms,
    terminal_at_ms,
    updated_at_ms
FROM thread_pending_interactions
WHERE interaction_id = ?
            "#,
        )
        .bind(interaction_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.as_ref()
            .map(PendingInteractionRow::try_from_row)
            .transpose()?
            .map(PendingInteraction::try_from)
            .transpose()
    }

    async fn get_thread_pending_interaction_by_worker_request(
        &self,
        thread_id: ThreadId,
        kind: PendingInteractionKind,
        worker_request_id: &str,
    ) -> anyhow::Result<Option<PendingInteraction>> {
        let row = sqlx::query(
            r#"
SELECT
    interaction_id,
    thread_id,
    source_kind,
    source_id,
    turn_id,
    worker_request_id,
    server_request_id_json,
    kind,
    status,
    request_payload_json,
    request_payload_sha256,
    request_payload_preview,
    request_redactions_json,
    response_payload_json,
    response_payload_sha256,
    response_payload_preview,
    response_redactions_json,
    no_client_policy,
    timeout_at_ms,
    created_at_ms,
    delivered_at_ms,
    responded_at_ms,
    terminal_at_ms,
    updated_at_ms
FROM thread_pending_interactions
WHERE thread_id = ? AND kind = ? AND worker_request_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .bind(kind.as_str())
        .bind(worker_request_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.as_ref()
            .map(PendingInteractionRow::try_from_row)
            .transpose()?
            .map(PendingInteraction::try_from)
            .transpose()
    }

    pub async fn list_thread_pending_interactions(
        &self,
        params: PendingInteractionListParams,
    ) -> anyhow::Result<PendingInteractionPage> {
        if params.limit == 0 || params.limit > MAX_PENDING_INTERACTION_LIST_LIMIT {
            anyhow::bail!(
                "pending interaction list limit must be between 1 and {MAX_PENDING_INTERACTION_LIST_LIMIT}"
            );
        }
        let cursor = params
            .cursor
            .as_deref()
            .map(decode_pending_interaction_cursor)
            .transpose()?;
        let limit = i64::from(params.limit) + 1;
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
    interaction_id,
    thread_id,
    source_kind,
    source_id,
    turn_id,
    worker_request_id,
    server_request_id_json,
    kind,
    status,
    request_payload_json,
    request_payload_sha256,
    request_payload_preview,
    request_redactions_json,
    response_payload_json,
    response_payload_sha256,
    response_payload_preview,
    response_redactions_json,
    no_client_policy,
    timeout_at_ms,
    created_at_ms,
    delivered_at_ms,
    responded_at_ms,
    terminal_at_ms,
    updated_at_ms
FROM thread_pending_interactions
WHERE 1 = 1
            "#,
        );
        if let Some(thread_id) = params.thread_id {
            builder.push(" AND thread_id = ");
            builder.push_bind(thread_id.to_string());
        }
        if !params.statuses.is_empty() {
            builder.push(" AND status IN (");
            let mut separated = builder.separated(", ");
            for status in params.statuses {
                separated.push_bind(status.as_str());
            }
            separated.push_unseparated(")");
        }
        if !params.kinds.is_empty() {
            builder.push(" AND kind IN (");
            let mut separated = builder.separated(", ");
            for kind in params.kinds {
                separated.push_bind(kind.as_str());
            }
            separated.push_unseparated(")");
        }
        if let Some(cursor) = cursor {
            builder.push(" AND (created_at_ms < ");
            builder.push_bind(cursor.created_at_ms);
            builder.push(" OR (created_at_ms = ");
            builder.push_bind(cursor.created_at_ms);
            builder.push(" AND interaction_id < ");
            builder.push_bind(cursor.interaction_id);
            builder.push("))");
        }
        builder.push(" ORDER BY created_at_ms DESC, interaction_id DESC LIMIT ");
        builder.push_bind(limit);
        let rows = builder.build().fetch_all(self.pool.as_ref()).await?;
        let mut data = rows
            .iter()
            .map(PendingInteractionRow::try_from_row)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(PendingInteraction::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = data.len() > params.limit as usize;
        if has_more {
            data.truncate(params.limit as usize);
        }
        let next_cursor = has_more
            .then(|| {
                data.last().map(|interaction| {
                    encode_pending_interaction_cursor(
                        datetime_to_epoch_millis(interaction.created_at),
                        &interaction.interaction_id,
                    )
                })
            })
            .flatten();
        Ok(PendingInteractionPage { data, next_cursor })
    }

    pub async fn list_thread_pending_interaction_events(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<Vec<PendingInteractionEvent>> {
        let rows = sqlx::query(
            r#"
SELECT
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
FROM thread_pending_interaction_events
WHERE interaction_id = ?
ORDER BY created_at_ms ASC, insertion_seq ASC
            "#,
        )
        .bind(interaction_id)
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.iter()
            .map(PendingInteractionEventRow::try_from_row)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(PendingInteractionEvent::try_from)
            .collect()
    }

    pub async fn mark_thread_pending_interaction_delivered(
        &self,
        interaction_id: &str,
    ) -> anyhow::Result<bool> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let thread_id: Option<String> = sqlx::query_scalar(
            r#"
SELECT thread_id
FROM thread_pending_interactions
WHERE interaction_id = ?
            "#,
        )
        .bind(interaction_id)
        .fetch_optional(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"
UPDATE thread_pending_interactions
SET status = ?, delivered_at_ms = COALESCE(delivered_at_ms, ?), updated_at_ms = ?
WHERE interaction_id = ? AND status = ?
            "#,
        )
        .bind(PendingInteractionStatus::Delivered.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .bind(interaction_id)
        .bind(PendingInteractionStatus::Pending.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() > 0 {
            let thread_id = thread_id.ok_or_else(|| {
                anyhow::anyhow!("failed to load delivered pending interaction thread id")
            })?;
            let payload_json = serde_json::json!({"status": "delivered"});
            let redactions_json = serde_json::json!([]);
            insert_pending_interaction_event_in_tx(
                &mut tx,
                PendingInteractionEventInsert {
                    interaction_id,
                    thread_id: thread_id.as_str(),
                    event_kind: PendingInteractionEventKind::Delivered,
                    status: PendingInteractionStatus::Delivered,
                    payload_json: &payload_json,
                    payload_preview: "delivered",
                    redactions_json: &redactions_json,
                    created_at_ms: now_ms,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn respond_thread_pending_interaction(
        &self,
        params: &PendingInteractionRespondParams,
    ) -> anyhow::Result<bool> {
        if !params.terminal_status.is_terminal() {
            anyhow::bail!("pending interaction response status must be terminal");
        }
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let response_payload_json = redact_state_json_string(&params.response_payload_json)?;
        let response_payload_sha256 = payload_sha256(response_payload_json.as_bytes());
        let response_redactions_json = redact_state_json_string(&params.response_redactions_json)?;
        let response_payload_preview =
            redact_state_string(params.response_payload_preview.as_str());
        let mut tx = self.pool.begin().await?;
        let thread_id: Option<String> = sqlx::query_scalar(
            r#"
SELECT thread_id
FROM thread_pending_interactions
WHERE interaction_id = ?
            "#,
        )
        .bind(params.interaction_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"
UPDATE thread_pending_interactions
SET
    status = ?,
    response_payload_json = ?,
    response_payload_sha256 = ?,
    response_payload_preview = ?,
    response_redactions_json = ?,
    responded_at_ms = ?,
    terminal_at_ms = ?,
    updated_at_ms = ?
WHERE interaction_id = ? AND status IN (?, ?)
            "#,
        )
        .bind(params.terminal_status.as_str())
        .bind(response_payload_json)
        .bind(response_payload_sha256)
        .bind(response_payload_preview.as_str())
        .bind(response_redactions_json)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .bind(params.interaction_id.as_str())
        .bind(PendingInteractionStatus::Pending.as_str())
        .bind(PendingInteractionStatus::Delivered.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() > 0 {
            let thread_id = thread_id.ok_or_else(|| {
                anyhow::anyhow!("failed to load responded pending interaction thread id")
            })?;
            insert_pending_interaction_event_in_tx(
                &mut tx,
                PendingInteractionEventInsert {
                    interaction_id: params.interaction_id.as_str(),
                    thread_id: thread_id.as_str(),
                    event_kind: PendingInteractionEventKind::from_terminal_status(
                        params.terminal_status,
                    )?,
                    status: params.terminal_status,
                    payload_json: &params.response_payload_json,
                    payload_preview: response_payload_preview.as_str(),
                    redactions_json: &params.response_redactions_json,
                    created_at_ms: now_ms,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn respond_thread_pending_interactions_for_source(
        &self,
        params: PendingInteractionRespondForSourceParams,
    ) -> anyhow::Result<usize> {
        if !params.terminal_status.is_terminal() {
            anyhow::bail!("pending interaction response status must be terminal");
        }
        if params.kinds.is_empty() {
            return Ok(0);
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT interaction_id
FROM thread_pending_interactions
WHERE thread_id =
            "#,
        );
        builder.push(" ");
        builder.push_bind(params.thread_id.to_string());
        builder.push(" AND source_kind = ");
        builder.push_bind(params.source_kind.as_str());
        builder.push(" AND source_id = ");
        builder.push_bind(params.source_id.as_str());
        builder.push(" AND status IN (");
        let mut statuses = builder.separated(", ");
        statuses.push_bind(PendingInteractionStatus::Pending.as_str());
        statuses.push_bind(PendingInteractionStatus::Delivered.as_str());
        statuses.push_unseparated(")");
        builder.push(" AND kind IN (");
        let mut separated = builder.separated(", ");
        for kind in &params.kinds {
            separated.push_bind(kind.as_str());
        }
        separated.push_unseparated(")");

        let rows = builder.build().fetch_all(self.pool.as_ref()).await?;
        let mut updated = 0;
        for row in rows {
            let interaction_id: String = row.try_get("interaction_id")?;
            if self
                .respond_thread_pending_interaction(&PendingInteractionRespondParams {
                    interaction_id,
                    response_payload_json: params.response_payload_json.clone(),
                    response_payload_preview: params.response_payload_preview.clone(),
                    response_redactions_json: params.response_redactions_json.clone(),
                    terminal_status: params.terminal_status,
                })
                .await?
            {
                updated += 1;
            }
        }
        Ok(updated)
    }

    pub async fn expire_timed_out_thread_pending_interactions(&self) -> anyhow::Result<usize> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let response_payload = serde_json::json!({
            "reason": "timeout",
        });
        let response_payload_json = redact_state_json_string(&response_payload)?;
        let response_payload_sha256 = payload_sha256(response_payload_json.as_bytes());
        let response_redactions = serde_json::json!([]);
        let response_redactions_json = redact_state_json_string(&response_redactions)?;
        let mut tx = self.pool.begin().await?;
        let expiring_rows = sqlx::query(
            r#"
SELECT interaction_id, thread_id
FROM thread_pending_interactions
WHERE timeout_at_ms IS NOT NULL
  AND timeout_at_ms <= ?
  AND status IN (?, ?)
            "#,
        )
        .bind(now_ms)
        .bind(PendingInteractionStatus::Pending.as_str())
        .bind(PendingInteractionStatus::Delivered.as_str())
        .fetch_all(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"
UPDATE thread_pending_interactions
SET
    status = ?,
    response_payload_json = ?,
    response_payload_sha256 = ?,
    response_payload_preview = ?,
    response_redactions_json = ?,
    responded_at_ms = ?,
    terminal_at_ms = ?,
    updated_at_ms = ?
WHERE timeout_at_ms IS NOT NULL
  AND timeout_at_ms <= ?
  AND status IN (?, ?)
            "#,
        )
        .bind(PendingInteractionStatus::Expired.as_str())
        .bind(response_payload_json)
        .bind(response_payload_sha256)
        .bind("timeout")
        .bind(response_redactions_json)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .bind(PendingInteractionStatus::Pending.as_str())
        .bind(PendingInteractionStatus::Delivered.as_str())
        .execute(&mut *tx)
        .await?;
        for row in expiring_rows {
            let interaction_id: String = row.try_get("interaction_id")?;
            let thread_id: String = row.try_get("thread_id")?;
            insert_pending_interaction_event_in_tx(
                &mut tx,
                PendingInteractionEventInsert {
                    interaction_id: interaction_id.as_str(),
                    thread_id: thread_id.as_str(),
                    event_kind: PendingInteractionEventKind::Expired,
                    status: PendingInteractionStatus::Expired,
                    payload_json: &response_payload,
                    payload_preview: "timeout",
                    redactions_json: &response_redactions,
                    created_at_ms: now_ms,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() as usize)
    }
}

struct PendingInteractionEventInsert<'a> {
    interaction_id: &'a str,
    thread_id: &'a str,
    event_kind: PendingInteractionEventKind,
    status: PendingInteractionStatus,
    payload_json: &'a Value,
    payload_preview: &'a str,
    redactions_json: &'a Value,
    created_at_ms: i64,
}

async fn insert_pending_interaction_event_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    event: PendingInteractionEventInsert<'_>,
) -> anyhow::Result<()> {
    let payload_json = redact_state_json_string(event.payload_json)?;
    let payload_sha256 = payload_sha256(payload_json.as_bytes());
    let redactions_json = redact_state_json_string(event.redactions_json)?;
    let payload_preview = redact_state_string(event.payload_preview);
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
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(event.interaction_id)
    .bind(event.thread_id)
    .bind(event.event_kind.as_str())
    .bind(event.status.as_str())
    .bind(payload_json)
    .bind(payload_sha256)
    .bind(payload_preview)
    .bind(redactions_json)
    .bind(event.created_at_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn payload_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug)]
struct PendingInteractionCursor {
    created_at_ms: i64,
    interaction_id: String,
}

fn encode_pending_interaction_cursor(created_at_ms: i64, interaction_id: &str) -> String {
    format!("{created_at_ms}:{interaction_id}")
}

fn decode_pending_interaction_cursor(cursor: &str) -> anyhow::Result<PendingInteractionCursor> {
    let Some((created_at_ms, interaction_id)) = cursor.split_once(':') else {
        anyhow::bail!("invalid pending interaction cursor");
    };
    if interaction_id.is_empty() {
        anyhow::bail!("invalid pending interaction cursor");
    }
    Ok(PendingInteractionCursor {
        created_at_ms: created_at_ms
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid pending interaction cursor"))?,
        interaction_id: interaction_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PendingInteractionSourceKind;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn create_list_and_page_pending_interactions() -> anyhow::Result<()> {
        let runtime = test_runtime_with_thread().await?;
        let thread_id = test_thread_id();
        let first = create_test_interaction(
            runtime.as_ref(),
            "interaction-1",
            thread_id,
            PendingInteractionKind::UserInput,
            "turn-1",
            "request-1",
        )
        .await?;
        let second = create_test_interaction(
            runtime.as_ref(),
            "interaction-2",
            thread_id,
            PendingInteractionKind::CommandApproval,
            "turn-2",
            "request-2",
        )
        .await?;

        assert_eq!(first.status, PendingInteractionStatus::Pending);
        assert_eq!(second.kind, PendingInteractionKind::CommandApproval);

        let page = runtime
            .list_thread_pending_interactions(PendingInteractionListParams {
                thread_id: Some(thread_id),
                statuses: vec![PendingInteractionStatus::Pending],
                kinds: Vec::new(),
                cursor: None,
                limit: 1,
            })
            .await?;

        assert_eq!(page.data.len(), 1);
        assert!(page.next_cursor.is_some());

        let next_page = runtime
            .list_thread_pending_interactions(PendingInteractionListParams {
                thread_id: Some(thread_id),
                statuses: vec![PendingInteractionStatus::Pending],
                kinds: Vec::new(),
                cursor: page.next_cursor,
                limit: 1,
            })
            .await?;

        assert_eq!(next_page.data.len(), 1);
        assert_eq!(next_page.next_cursor, None);
        assert_ne!(
            page.data[0].interaction_id,
            next_page.data[0].interaction_id
        );

        Ok(())
    }

    #[tokio::test]
    async fn delivered_and_responded_are_terminal_transitions() -> anyhow::Result<()> {
        let runtime = test_runtime_with_thread().await?;
        let thread_id = test_thread_id();
        let interaction = create_test_interaction(
            runtime.as_ref(),
            "interaction-response",
            thread_id,
            PendingInteractionKind::PermissionGrant,
            "turn-1",
            "request-1",
        )
        .await?;

        assert!(
            runtime
                .mark_thread_pending_interaction_delivered(interaction.interaction_id.as_str())
                .await?
        );
        let delivered = runtime
            .get_thread_pending_interaction(interaction.interaction_id.as_str())
            .await?
            .expect("interaction");
        assert_eq!(delivered.status, PendingInteractionStatus::Delivered);
        assert!(delivered.delivered_at.is_some());

        assert!(
            runtime
                .respond_thread_pending_interaction(&PendingInteractionRespondParams {
                    interaction_id: interaction.interaction_id.clone(),
                    response_payload_json: json!({"accepted": true}),
                    response_payload_preview: "accepted".to_string(),
                    response_redactions_json: json!([]),
                    terminal_status: PendingInteractionStatus::Responded,
                })
                .await?
        );
        let responded = runtime
            .get_thread_pending_interaction(interaction.interaction_id.as_str())
            .await?
            .expect("interaction");
        assert_eq!(responded.status, PendingInteractionStatus::Responded);
        assert_eq!(
            responded.response_payload_json,
            Some(json!({"accepted": true}))
        );
        assert!(responded.responded_at.is_some());
        assert!(responded.terminal_at.is_some());
        let events = runtime
            .list_thread_pending_interaction_events(interaction.interaction_id.as_str())
            .await?;
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_kind)
                .collect::<Vec<_>>(),
            vec![
                PendingInteractionEventKind::Created,
                PendingInteractionEventKind::Delivered,
                PendingInteractionEventKind::Responded
            ]
        );
        assert_eq!(events[2].payload_json, json!({"accepted": true}));

        assert!(
            !runtime
                .respond_thread_pending_interaction(&PendingInteractionRespondParams {
                    interaction_id: interaction.interaction_id.clone(),
                    response_payload_json: json!({"accepted": false}),
                    response_payload_preview: "denied".to_string(),
                    response_redactions_json: json!([]),
                    terminal_status: PendingInteractionStatus::Denied,
                })
                .await?
        );

        Ok(())
    }

    #[tokio::test]
    async fn events_with_equal_timestamps_preserve_insertion_order() -> anyhow::Result<()> {
        let runtime = test_runtime_with_thread().await?;
        let thread_id = test_thread_id();
        let interaction = create_test_interaction(
            runtime.as_ref(),
            "interaction-event-order",
            thread_id,
            PendingInteractionKind::PermissionGrant,
            "turn-1",
            "request-1",
        )
        .await?;
        let payload_json = serde_json::to_string(&json!({"status": "pending"}))?;
        let payload_sha256 = payload_sha256(payload_json.as_bytes());
        let created_at_ms = 1_700_000_000_000_i64;

        sqlx::query("DELETE FROM thread_pending_interaction_events WHERE interaction_id = ?")
            .bind(interaction.interaction_id.as_str())
            .execute(runtime.pool.as_ref())
            .await?;
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
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(event_id)
            .bind(interaction.interaction_id.as_str())
            .bind(thread_id.to_string())
            .bind(event_kind)
            .bind(status)
            .bind(payload_json.as_str())
            .bind(payload_sha256.as_str())
            .bind("pending")
            .bind("[]")
            .bind(created_at_ms)
            .execute(runtime.pool.as_ref())
            .await?;
        }

        let events = runtime
            .list_thread_pending_interaction_events(interaction.interaction_id.as_str())
            .await?;
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-z-first", "event-a-second"]
        );

        Ok(())
    }

    #[tokio::test]
    async fn timeout_expiry_only_updates_active_interactions() -> anyhow::Result<()> {
        let runtime = test_runtime_with_thread().await?;
        let thread_id = test_thread_id();
        let expired = runtime
            .create_thread_pending_interaction(&PendingInteractionCreateParams {
                interaction_id: "interaction-expired".to_string(),
                thread_id,
                source_kind: PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("request-1".to_string()),
                server_request_id_json: Some(json!(1)),
                kind: PendingInteractionKind::UserInput,
                request_payload_json: json!({"type": "requestUserInput"}),
                request_payload_preview: "requestUserInput".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "cancel".to_string(),
                timeout_at: Some(Utc::now() - chrono::Duration::seconds(1)),
            })
            .await?;
        create_test_interaction(
            runtime.as_ref(),
            "interaction-active",
            thread_id,
            PendingInteractionKind::CommandApproval,
            "turn-2",
            "request-2",
        )
        .await?;

        assert_eq!(
            runtime
                .expire_timed_out_thread_pending_interactions()
                .await?,
            1
        );
        let expired = runtime
            .get_thread_pending_interaction(expired.interaction_id.as_str())
            .await?
            .expect("expired interaction");
        assert_eq!(expired.status, PendingInteractionStatus::Expired);
        let expired_events = runtime
            .list_thread_pending_interaction_events(expired.interaction_id.as_str())
            .await?;
        assert_eq!(
            expired_events
                .iter()
                .map(|event| event.event_kind)
                .collect::<Vec<_>>(),
            vec![
                PendingInteractionEventKind::Created,
                PendingInteractionEventKind::Expired
            ]
        );

        let active = runtime
            .list_thread_pending_interactions(PendingInteractionListParams {
                thread_id: Some(thread_id),
                statuses: vec![PendingInteractionStatus::Pending],
                kinds: Vec::new(),
                cursor: None,
                limit: 10,
            })
            .await?;
        assert_eq!(active.data.len(), 1);
        assert_eq!(
            active.data[0].interaction_id,
            "interaction-active".to_string()
        );

        Ok(())
    }

    #[tokio::test]
    async fn idempotent_create_and_source_terminalization_close_goal_waits() -> anyhow::Result<()> {
        let runtime = test_runtime_with_thread().await?;
        let thread_id = test_thread_id();
        let params = PendingInteractionCreateParams {
            interaction_id: "goal-wait-1".to_string(),
            thread_id,
            source_kind: PendingInteractionSourceKind::Goal,
            source_id: Some("goal-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            worker_request_id: Some("goal-1:blocked".to_string()),
            server_request_id_json: None,
            kind: PendingInteractionKind::Blocked,
            request_payload_json: json!({"type": "goalStatusWait", "status": "blocked"}),
            request_payload_preview: "blocked: goal".to_string(),
            request_redactions_json: json!([]),
            no_client_policy: "record-and-wait-for-coordinator".to_string(),
            timeout_at: None,
        };
        let created = runtime
            .create_thread_pending_interaction_if_absent(&params)
            .await?;
        let duplicate = runtime
            .create_thread_pending_interaction_if_absent(&PendingInteractionCreateParams {
                interaction_id: "goal-wait-duplicate".to_string(),
                ..params.clone()
            })
            .await?;
        assert_eq!(created, duplicate);

        let updated = runtime
            .respond_thread_pending_interactions_for_source(
                PendingInteractionRespondForSourceParams {
                    thread_id,
                    source_kind: PendingInteractionSourceKind::Goal,
                    source_id: "goal-1".to_string(),
                    kinds: vec![PendingInteractionKind::Blocked],
                    response_payload_json: json!({"type": "terminal", "reason": "goal resumed"}),
                    response_payload_preview: "goal resumed".to_string(),
                    response_redactions_json: json!([]),
                    terminal_status: PendingInteractionStatus::NoLongerWaiting,
                },
            )
            .await?;
        assert_eq!(updated, 1);
        let closed = runtime
            .get_thread_pending_interaction(created.interaction_id.as_str())
            .await?
            .expect("closed interaction");
        assert_eq!(closed.status, PendingInteractionStatus::NoLongerWaiting);
        let events = runtime
            .list_thread_pending_interaction_events(created.interaction_id.as_str())
            .await?;
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_kind)
                .collect::<Vec<_>>(),
            vec![
                PendingInteractionEventKind::Created,
                PendingInteractionEventKind::NoLongerWaiting
            ]
        );

        Ok(())
    }

    async fn test_runtime_with_thread() -> anyhow::Result<Arc<StateRuntime>> {
        let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
        runtime
            .upsert_thread(&test_thread_metadata(
                runtime.codex_home(),
                test_thread_id(),
                runtime.codex_home().join("workspace"),
            ))
            .await?;
        Ok(runtime)
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("thread id")
    }

    async fn create_test_interaction(
        runtime: &StateRuntime,
        interaction_id: &str,
        thread_id: ThreadId,
        kind: PendingInteractionKind,
        turn_id: &str,
        worker_request_id: &str,
    ) -> anyhow::Result<PendingInteraction> {
        runtime
            .create_thread_pending_interaction(&PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some(turn_id.to_string()),
                worker_request_id: Some(worker_request_id.to_string()),
                server_request_id_json: Some(json!(worker_request_id)),
                kind,
                request_payload_json: json!({
                    "turnId": turn_id,
                    "workerRequestId": worker_request_id,
                }),
                request_payload_preview: worker_request_id.to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "cancel".to_string(),
                timeout_at: None,
            })
            .await
    }
}
