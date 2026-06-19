use super::queries::insert_mailbox_dead_letter;
use super::queries::insert_mailbox_receipt;
use super::queries::mailbox_message_from_row;
use super::queries::mailbox_message_returning;
use sqlx::Sqlite;

pub(crate) async fn expire_stale_mailbox_leases_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    now_ms: i64,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
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
WHERE status = ? AND lease_expires_at_ms <= ?
"#,
    )
    .bind(crate::MailboxMessageStatus::Claimed.as_str())
    .bind(now_ms)
    .fetch_all(&mut **tx)
    .await?;

    for row in rows {
        let message = mailbox_message_from_row(&row)?;
        if let Some(attempt_id) = message.last_attempt_id.as_deref() {
            sqlx::query(
                r#"
UPDATE thread_mailbox_delivery_attempts
SET status = ?, completed_at_ms = ?, error = COALESCE(error, 'lease expired')
WHERE attempt_id = ? AND status = ?
"#,
            )
            .bind(crate::MailboxDeliveryAttemptStatus::Expired.as_str())
            .bind(now_ms)
            .bind(attempt_id)
            .bind(crate::MailboxDeliveryAttemptStatus::Claimed.as_str())
            .execute(&mut **tx)
            .await?;
        }
        let message = update_expired_claim(tx, &message, now_ms).await?;
        let receipt_kind = if message.status == crate::MailboxMessageStatus::Poisoned {
            if let Some(attempt_id) = message.last_attempt_id.as_deref() {
                insert_mailbox_dead_letter(
                    tx,
                    &message.message_id,
                    attempt_id,
                    "lease expired after max attempts",
                    now_ms,
                )
                .await?;
            }
            crate::MailboxReceiptKind::Poisoned
        } else {
            crate::MailboxReceiptKind::LeaseExpired
        };
        insert_mailbox_receipt(
            tx,
            &message,
            message.last_attempt_id.as_deref(),
            receipt_kind,
            None,
            now_ms,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn expire_due_messages_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    now_ms: i64,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
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
WHERE status = ? AND expires_at_ms IS NOT NULL AND expires_at_ms <= ?
"#,
    )
    .bind(crate::MailboxMessageStatus::Queued.as_str())
    .bind(now_ms)
    .fetch_all(&mut **tx)
    .await?;

    for row in rows {
        let message = mailbox_message_from_row(&row)?;
        let sql = mailbox_message_returning(
            r#"
UPDATE thread_mailbox_messages
SET status = ?, terminal_at_ms = ?, updated_at_ms = ?
WHERE message_id = ? AND status = ?
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(crate::MailboxMessageStatus::Expired.as_str())
            .bind(now_ms)
            .bind(now_ms)
            .bind(message.message_id.as_str())
            .bind(crate::MailboxMessageStatus::Queued.as_str())
            .fetch_optional(&mut **tx)
            .await?;
        if let Some(row) = row {
            let message = mailbox_message_from_row(&row)?;
            insert_mailbox_receipt(
                tx,
                &message,
                None,
                crate::MailboxReceiptKind::Expired,
                None,
                now_ms,
            )
            .await?;
        }
    }
    Ok(())
}

async fn update_expired_claim(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    message: &crate::MailboxMessage,
    now_ms: i64,
) -> anyhow::Result<crate::MailboxMessage> {
    let next_status = if message.attempt_count >= message.max_attempts {
        crate::MailboxMessageStatus::Poisoned
    } else {
        crate::MailboxMessageStatus::Queued
    };
    let sql = mailbox_message_returning(
        r#"
UPDATE thread_mailbox_messages
SET
    status = ?,
    lease_id = NULL,
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = COALESCE(last_error, 'lease expired'),
    terminal_at_ms = CASE WHEN ? THEN ? ELSE NULL END,
    updated_at_ms = ?
WHERE message_id = ?
RETURNING
"#,
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(next_status.as_str())
        .bind(next_status == crate::MailboxMessageStatus::Poisoned)
        .bind(now_ms)
        .bind(now_ms)
        .bind(message.message_id.as_str())
        .fetch_one(&mut **tx)
        .await?;
    mailbox_message_from_row(&row)
}
