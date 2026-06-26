use super::MAX_MAILBOX_MESSAGE_LIST_LIMIT;
use super::cursor::decode_mailbox_cursor;
use super::cursor::encode_mailbox_cursor;
use super::queries::mailbox_message_from_row;
use super::queries::mailbox_receipt_from_row;
use super::types::MailboxMessagePage;
use super::types::MailboxMessageStoreListParams;
use sqlx::QueryBuilder;
use sqlx::Sqlite;
use sqlx::SqlitePool;

pub(crate) async fn list_mailbox_messages(
    pool: &SqlitePool,
    params: MailboxMessageStoreListParams,
) -> anyhow::Result<MailboxMessagePage> {
    let limit = params.limit.clamp(1, MAX_MAILBOX_MESSAGE_LIST_LIMIT);
    let cursor = params
        .cursor
        .as_deref()
        .map(decode_mailbox_cursor)
        .transpose()?;
    let mut builder = QueryBuilder::<Sqlite>::new(
        r#"
SELECT
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
FROM thread_mailbox_messages
WHERE 1 = 1
"#,
    );
    if let Some(target_thread_id) = params.target_thread_id {
        builder.push(" AND target_thread_id = ");
        builder.push_bind(target_thread_id.to_string());
    }
    if !params.statuses.is_empty() {
        builder.push(" AND status IN (");
        let mut separated = builder.separated(", ");
        for status in params.statuses {
            separated.push_bind(status.as_str());
        }
        separated.push_unseparated(")");
    }
    if let Some(cursor) = cursor {
        builder.push(" AND (created_at_ms < ");
        builder.push_bind(cursor.created_at_ms);
        builder.push(" OR (created_at_ms = ");
        builder.push_bind(cursor.created_at_ms);
        builder.push(" AND message_id > ");
        builder.push_bind(cursor.message_id);
        builder.push("))");
    }
    builder.push(" ORDER BY created_at_ms DESC, message_id ASC LIMIT ");
    builder.push_bind(i64::from(limit) + 1);
    let mut data = builder
        .build()
        .fetch_all(pool)
        .await?
        .iter()
        .map(mailbox_message_from_row)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let next_cursor = if data.len() > limit as usize {
        data.truncate(limit as usize);
        data.last()
            .map(|message| encode_mailbox_cursor(message.created_at, &message.message_id))
    } else {
        None
    };
    Ok(MailboxMessagePage { data, next_cursor })
}

pub(crate) async fn list_mailbox_receipts(
    pool: &SqlitePool,
    message_id: &str,
) -> anyhow::Result<Vec<crate::MailboxReceipt>> {
    let rows = sqlx::query(
        r#"
SELECT
    receipt_id,
    message_id,
    attempt_id,
    thread_id,
    kind,
    status_after,
    payload_json,
    created_at_ms
FROM thread_mailbox_receipts
WHERE message_id = ?
ORDER BY created_at_ms ASC, rowid ASC
"#,
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(mailbox_receipt_from_row)
        .collect::<anyhow::Result<Vec<_>>>()
}
