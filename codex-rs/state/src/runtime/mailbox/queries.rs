use crate::model::MailboxDeliveryAttemptRow;
use crate::model::MailboxMessageRow;
use crate::model::MailboxReceiptRow;
use codex_protocol::ThreadId;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use sqlx::Sqlite;
use sqlx::SqlitePool;
use uuid::Uuid;

pub(crate) async fn insert_mailbox_delivery_attempt(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    attempt_id: &str,
    message: &crate::MailboxMessage,
    lease_id: &str,
    lease_owner: &str,
    claimed_at_ms: i64,
    lease_expires_at_ms: i64,
) -> anyhow::Result<crate::MailboxDeliveryAttempt> {
    let row = sqlx::query(
        r#"
INSERT INTO thread_mailbox_delivery_attempts (
    attempt_id,
    message_id,
    lease_id,
    lease_owner,
    attempt_number,
    status,
    claimed_at_ms,
    lease_expires_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
RETURNING
    attempt_id,
    message_id,
    lease_id,
    lease_owner,
    attempt_number,
    status,
    claimed_at_ms,
    lease_expires_at_ms,
    completed_at_ms,
    error
"#,
    )
    .bind(attempt_id)
    .bind(message.message_id.as_str())
    .bind(lease_id)
    .bind(lease_owner)
    .bind(message.attempt_count)
    .bind(crate::MailboxDeliveryAttemptStatus::Claimed.as_str())
    .bind(claimed_at_ms)
    .bind(lease_expires_at_ms)
    .fetch_one(&mut **tx)
    .await?;
    mailbox_delivery_attempt_from_row(&row)
}

pub(crate) async fn insert_mailbox_receipt(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message: &crate::MailboxMessage,
    attempt_id: Option<&str>,
    kind: crate::MailboxReceiptKind,
    payload_json: Option<Value>,
    created_at_ms: i64,
) -> anyhow::Result<()> {
    let payload_json = payload_json
        .map(|payload| serde_json::to_string(&payload))
        .transpose()?;
    insert_mailbox_receipt_raw_payload(tx, message, attempt_id, kind, payload_json, created_at_ms)
        .await
}

pub(crate) async fn insert_mailbox_receipt_raw_payload(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message: &crate::MailboxMessage,
    attempt_id: Option<&str>,
    kind: crate::MailboxReceiptKind,
    payload_json: Option<String>,
    created_at_ms: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
INSERT INTO thread_mailbox_receipts (
    receipt_id,
    message_id,
    attempt_id,
    thread_id,
    kind,
    status_after,
    payload_json,
    created_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(message.message_id.as_str())
    .bind(attempt_id)
    .bind(message.target_thread_id.to_string())
    .bind(kind.as_str())
    .bind(message.status.as_str())
    .bind(payload_json)
    .bind(created_at_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn insert_mailbox_dead_letter(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message_id: &str,
    attempt_id: &str,
    reason: &str,
    created_at_ms: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
INSERT OR REPLACE INTO thread_mailbox_dead_letters (
    message_id,
    failed_attempt_id,
    reason,
    created_at_ms
) VALUES (?, ?, ?, ?)
"#,
    )
    .bind(message_id)
    .bind(attempt_id)
    .bind(reason)
    .bind(created_at_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn select_attempt_for_lease_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message_id: &str,
    attempt_id: &str,
    lease_id: &str,
) -> anyhow::Result<Option<crate::MailboxDeliveryAttempt>> {
    let row = sqlx::query(
        r#"
SELECT
    attempt_id,
    message_id,
    lease_id,
    lease_owner,
    attempt_number,
    status,
    claimed_at_ms,
    lease_expires_at_ms,
    completed_at_ms,
    error
FROM thread_mailbox_delivery_attempts
WHERE message_id = ? AND attempt_id = ? AND lease_id = ?
"#,
    )
    .bind(message_id)
    .bind(attempt_id)
    .bind(lease_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| mailbox_delivery_attempt_from_row(&row))
        .transpose()
}

pub(crate) async fn update_failed_claim_for_retry(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message_id: &str,
    lease_id: &str,
    error: &str,
    next_attempt_at_ms: i64,
    now_ms: i64,
) -> anyhow::Result<Option<crate::MailboxMessage>> {
    let sql = mailbox_message_returning(
        r#"
UPDATE thread_mailbox_messages
SET
    status = CASE WHEN attempt_count >= max_attempts THEN ? ELSE ? END,
    lease_id = NULL,
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    next_attempt_at_ms = ?,
    last_error = ?,
    terminal_at_ms = CASE WHEN attempt_count >= max_attempts THEN ? ELSE NULL END,
    updated_at_ms = ?
WHERE message_id = ? AND lease_id = ? AND status = ?
RETURNING
"#,
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(crate::MailboxMessageStatus::Poisoned.as_str())
        .bind(crate::MailboxMessageStatus::Queued.as_str())
        .bind(next_attempt_at_ms)
        .bind(error)
        .bind(now_ms)
        .bind(now_ms)
        .bind(message_id)
        .bind(lease_id)
        .bind(crate::MailboxMessageStatus::Claimed.as_str())
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| mailbox_message_from_row(&row)).transpose()
}

pub(crate) async fn update_failed_claim_terminal(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message_id: &str,
    lease_id: &str,
    error: &str,
    now_ms: i64,
) -> anyhow::Result<Option<crate::MailboxMessage>> {
    let sql = mailbox_message_returning(
        r#"
UPDATE thread_mailbox_messages
SET
    status = ?,
    lease_id = NULL,
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    terminal_at_ms = ?,
    updated_at_ms = ?
WHERE message_id = ? AND lease_id = ? AND status = ?
RETURNING
"#,
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(crate::MailboxMessageStatus::Failed.as_str())
        .bind(error)
        .bind(now_ms)
        .bind(now_ms)
        .bind(message_id)
        .bind(lease_id)
        .bind(crate::MailboxMessageStatus::Claimed.as_str())
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| mailbox_message_from_row(&row)).transpose()
}

pub(crate) async fn select_message_by_target_idempotency_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    target_thread_id: ThreadId,
    idempotency_key: &str,
) -> anyhow::Result<Option<crate::MailboxMessage>> {
    let sql = mailbox_message_select(
        r#"
WHERE target_thread_id = ? AND idempotency_key = ?
"#,
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(target_thread_id.to_string())
        .bind(idempotency_key)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| mailbox_message_from_row(&row)).transpose()
}

pub(crate) async fn select_message_by_id(
    pool: &SqlitePool,
    message_id: &str,
) -> anyhow::Result<Option<crate::MailboxMessage>> {
    let sql = mailbox_message_select(
        r#"
WHERE message_id = ?
"#,
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| mailbox_message_from_row(&row)).transpose()
}

pub(crate) async fn select_message_by_id_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message_id: &str,
) -> anyhow::Result<Option<crate::MailboxMessage>> {
    let sql = mailbox_message_select(
        r#"
WHERE message_id = ?
"#,
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(message_id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| mailbox_message_from_row(&row)).transpose()
}

pub(crate) fn mailbox_message_returning(prefix: &'static str) -> String {
    format!("{prefix}\n{}", mailbox_message_columns())
}

fn mailbox_message_select(where_clause: &'static str) -> String {
    format!(
        "SELECT\n{}\nFROM thread_mailbox_messages\n{where_clause}",
        mailbox_message_columns()
    )
}

fn mailbox_message_columns() -> &'static str {
    r#"
    message_id,
    target_thread_id,
    sender_thread_id,
    sender_label,
    idempotency_key,
    kind,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    priority,
    attempt_count,
    max_attempts,
    next_attempt_at_ms,
    lease_id,
    lease_owner,
    lease_expires_at_ms,
    last_attempt_id,
    last_error,
    expires_at_ms,
    acknowledged_at_ms,
    terminal_at_ms,
    created_at_ms,
    updated_at_ms
"#
}

pub(crate) fn mailbox_message_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::MailboxMessage> {
    MailboxMessageRow::try_from_row(row)?.try_into()
}

fn mailbox_delivery_attempt_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::MailboxDeliveryAttempt> {
    MailboxDeliveryAttemptRow::try_from_row(row)?.try_into()
}

pub(crate) fn mailbox_receipt_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::MailboxReceipt> {
    MailboxReceiptRow::try_from_row(row)?.try_into()
}

pub(crate) fn payload_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
