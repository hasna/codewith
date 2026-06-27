use super::*;
use listing::list_mailbox_messages;
use listing::list_mailbox_receipts;
use maintenance::expire_due_messages_in_tx;
use maintenance::expire_stale_mailbox_leases_in_tx;
use queries::insert_mailbox_dead_letter;
use queries::insert_mailbox_delivery_attempt;
use queries::insert_mailbox_receipt;
use queries::insert_mailbox_receipt_raw_payload;
use queries::mailbox_message_from_row;
use queries::mailbox_message_returning;
use queries::payload_sha256;
use queries::select_attempt_for_lease_in_tx;
use queries::select_message_by_id;
use queries::select_message_by_id_in_tx;
use queries::select_message_by_target_idempotency_in_tx;
use queries::update_failed_claim_for_retry;
use queries::update_failed_claim_terminal;
use uuid::Uuid;

mod cursor;
mod listing;
mod maintenance;
mod queries;
#[cfg(test)]
mod tests;
mod types;

pub use types::MailboxAckParams;
pub use types::MailboxClaim;
pub use types::MailboxClaimParams;
pub use types::MailboxDispatchClaimParams;
pub use types::MailboxEnqueueOutcome;
pub use types::MailboxEnqueueParams;
pub use types::MailboxFailDisposition;
pub use types::MailboxFailParams;
pub use types::MailboxMessagePage;
pub use types::MailboxMessageStoreListParams;

pub const DEFAULT_MAILBOX_MESSAGE_LIST_LIMIT: u32 = 25;
pub const MAX_MAILBOX_MESSAGE_LIST_LIMIT: u32 = 100;

#[derive(Clone)]
pub struct MailboxMessageStore {
    pool: Arc<SqlitePool>,
}

enum MailboxClaimScope {
    Target(ThreadId),
    Dispatch {
        local_active_owner_id: String,
        local_active_fresh_after: DateTime<Utc>,
    },
}

impl MailboxMessageStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

async fn reserve_mailbox_target_lease_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    target_thread_id: ThreadId,
    owner_id: &str,
    lease_id: &str,
    lease_expires_at_ms: i64,
    now_ms: i64,
) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
INSERT INTO thread_mailbox_target_leases (
    target_thread_id,
    owner_id,
    lease_id,
    lease_expires_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT(target_thread_id) DO UPDATE SET
    owner_id = excluded.owner_id,
    lease_id = excluded.lease_id,
    lease_expires_at_ms = excluded.lease_expires_at_ms,
    updated_at_ms = excluded.updated_at_ms
WHERE thread_mailbox_target_leases.owner_id = excluded.owner_id
   OR thread_mailbox_target_leases.lease_expires_at_ms <= ?
        "#,
    )
    .bind(target_thread_id.to_string())
    .bind(owner_id)
    .bind(lease_id)
    .bind(lease_expires_at_ms)
    .bind(now_ms)
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "mailbox target lease conflict for target thread {target_thread_id}"
    );
    Ok(())
}

impl MailboxMessageStore {
    pub async fn enqueue_message(
        &self,
        params: MailboxEnqueueParams,
    ) -> anyhow::Result<MailboxEnqueueOutcome> {
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let next_attempt_at_ms = params
            .next_attempt_at
            .map(datetime_to_epoch_millis)
            .unwrap_or(now_ms);
        let expires_at_ms = params.expires_at.map(datetime_to_epoch_millis);
        let message_id = Uuid::new_v4().to_string();
        let payload_json = serde_json::to_string(&params.payload_json)?;
        let payload_sha256 = payload_sha256(payload_json.as_bytes());
        let mut tx = self.pool.begin().await?;
        let sql = mailbox_message_returning(
            r#"
INSERT OR IGNORE INTO thread_mailbox_messages (
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
    max_attempts,
    next_attempt_at_ms,
    expires_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING
"#,
        );
        let inserted = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(message_id.as_str())
            .bind(params.target_thread_id.to_string())
            .bind(
                params
                    .sender_thread_id
                    .map(|thread_id| thread_id.to_string()),
            )
            .bind(params.sender_label)
            .bind(params.idempotency_key.as_deref())
            .bind(params.kind.as_str())
            .bind(crate::MailboxMessageStatus::Queued.as_str())
            .bind(payload_json)
            .bind(payload_sha256)
            .bind(params.payload_preview)
            .bind(params.priority)
            .bind(params.max_attempts)
            .bind(next_attempt_at_ms)
            .bind(expires_at_ms)
            .bind(now_ms)
            .bind(now_ms)
            .fetch_optional(&mut *tx)
            .await?;

        let (message, created) = if let Some(row) = inserted {
            let message = mailbox_message_from_row(&row)?;
            insert_mailbox_receipt(
                &mut tx,
                &message,
                /*attempt_id*/ None,
                crate::MailboxReceiptKind::Enqueued,
                /*payload_json*/ None,
                now_ms,
            )
            .await?;
            (message, true)
        } else if let Some(idempotency_key) = params.idempotency_key {
            let message = select_message_by_target_idempotency_in_tx(
                &mut tx,
                params.target_thread_id,
                idempotency_key.as_str(),
            )
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("mailbox insert was ignored but no idempotent row exists")
            })?;
            (message, false)
        } else {
            anyhow::bail!("mailbox insert was ignored without an idempotency key");
        };

        tx.commit().await?;
        Ok(MailboxEnqueueOutcome { message, created })
    }

    pub async fn get_message(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Option<crate::MailboxMessage>> {
        select_message_by_id(self.pool.as_ref(), message_id).await
    }

    pub async fn list_messages(
        &self,
        params: MailboxMessageStoreListParams,
    ) -> anyhow::Result<MailboxMessagePage> {
        list_mailbox_messages(self.pool.as_ref(), params).await
    }

    pub async fn claim_next_message(
        &self,
        params: MailboxClaimParams,
    ) -> anyhow::Result<Option<MailboxClaim>> {
        self.claim_next_message_inner(
            MailboxClaimScope::Target(params.target_thread_id),
            params.lease_owner,
            params.lease_duration,
            params.now,
        )
        .await
    }

    pub async fn claim_next_due_message(
        &self,
        params: MailboxDispatchClaimParams,
    ) -> anyhow::Result<Option<MailboxClaim>> {
        self.claim_next_message_inner(
            MailboxClaimScope::Dispatch {
                local_active_owner_id: params.local_active_owner_id,
                local_active_fresh_after: params.local_active_fresh_after,
            },
            params.lease_owner,
            params.lease_duration,
            params.now,
        )
        .await
    }

    pub async fn release_dispatch_target_lease(
        &self,
        target_thread_id: ThreadId,
        owner_id: &str,
        lease_id: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            r#"
DELETE FROM thread_mailbox_target_leases
WHERE target_thread_id = ? AND owner_id = ? AND lease_id = ?
            "#,
        )
        .bind(target_thread_id.to_string())
        .bind(owner_id)
        .bind(lease_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn claim_next_message_inner(
        &self,
        scope: MailboxClaimScope,
        lease_owner: String,
        lease_duration: std::time::Duration,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<MailboxClaim>> {
        let now_ms = datetime_to_epoch_millis(now);
        let lease_expires_at = now + chrono::Duration::from_std(lease_duration)?;
        let lease_expires_at_ms = datetime_to_epoch_millis(lease_expires_at);
        let lease_id = Uuid::new_v4().to_string();
        let attempt_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;
        expire_stale_mailbox_leases_in_tx(&mut tx, now_ms).await?;
        expire_due_messages_in_tx(&mut tx, now_ms).await?;
        if matches!(scope, MailboxClaimScope::Dispatch { .. }) {
            sqlx::query("DELETE FROM thread_mailbox_target_leases WHERE lease_expires_at_ms <= ?")
                .bind(now_ms)
                .execute(&mut *tx)
                .await?;
        }

        let sql = mailbox_message_returning(match &scope {
            MailboxClaimScope::Target(_) => {
                r#"
UPDATE thread_mailbox_messages
SET
    status = ?,
    lease_id = ?,
    lease_owner = ?,
    lease_expires_at_ms = ?,
    attempt_count = attempt_count + 1,
    last_attempt_id = ?,
    updated_at_ms = ?
WHERE message_id = (
    SELECT message_id
    FROM thread_mailbox_messages
    WHERE target_thread_id = ?
      AND status = 'queued'
      AND next_attempt_at_ms <= ?
      AND (expires_at_ms IS NULL OR expires_at_ms > ?)
      AND attempt_count < max_attempts
    ORDER BY priority DESC, next_attempt_at_ms ASC, created_at_ms ASC, message_id ASC
    LIMIT 1
)
RETURNING
"#
            }
            MailboxClaimScope::Dispatch { .. } => {
                r#"
UPDATE thread_mailbox_messages
SET
    status = ?,
    lease_id = ?,
    lease_owner = ?,
    lease_expires_at_ms = ?,
    attempt_count = attempt_count + 1,
    last_attempt_id = ?,
    updated_at_ms = ?
WHERE message_id = (
    SELECT message_id
    FROM thread_mailbox_messages
    WHERE status = 'queued'
      AND next_attempt_at_ms <= ?
      AND (expires_at_ms IS NULL OR expires_at_ms > ?)
      AND attempt_count < max_attempts
      AND NOT EXISTS (
          SELECT 1
          FROM local_active_sessions AS active
          WHERE active.thread_id = thread_mailbox_messages.target_thread_id
            AND active.owner_id != ?
            AND active.last_seen_at_ms >= ?
      )
      AND NOT EXISTS (
          SELECT 1
          FROM thread_mailbox_target_leases AS target_lease
          WHERE target_lease.target_thread_id = thread_mailbox_messages.target_thread_id
            AND target_lease.owner_id != ?
            AND target_lease.lease_expires_at_ms > ?
      )
    ORDER BY priority DESC, next_attempt_at_ms ASC, created_at_ms ASC, message_id ASC
    LIMIT 1
)
RETURNING
"#
            }
        });
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(crate::MailboxMessageStatus::Claimed.as_str())
            .bind(lease_id.as_str())
            .bind(lease_owner.as_str())
            .bind(lease_expires_at_ms)
            .bind(attempt_id.as_str())
            .bind(now_ms);
        match &scope {
            MailboxClaimScope::Target(target_thread_id) => {
                query = query
                    .bind(target_thread_id.to_string())
                    .bind(now_ms)
                    .bind(now_ms);
            }
            MailboxClaimScope::Dispatch {
                local_active_owner_id,
                local_active_fresh_after,
            } => {
                query = query
                    .bind(now_ms)
                    .bind(now_ms)
                    .bind(local_active_owner_id.as_str())
                    .bind(datetime_to_epoch_millis(*local_active_fresh_after))
                    .bind(local_active_owner_id.as_str())
                    .bind(now_ms);
            }
        }
        let message_row = query.fetch_optional(&mut *tx).await?;
        let Some(message_row) = message_row else {
            tx.commit().await?;
            return Ok(None);
        };
        let message = mailbox_message_from_row(&message_row)?;
        if let MailboxClaimScope::Dispatch {
            local_active_owner_id,
            ..
        } = &scope
        {
            reserve_mailbox_target_lease_in_tx(
                &mut tx,
                message.target_thread_id,
                local_active_owner_id,
                lease_id.as_str(),
                lease_expires_at_ms,
                now_ms,
            )
            .await?;
        }
        let attempt = insert_mailbox_delivery_attempt(
            &mut tx,
            &attempt_id,
            &message,
            &lease_id,
            lease_owner.as_str(),
            now_ms,
            lease_expires_at_ms,
        )
        .await?;
        insert_mailbox_receipt(
            &mut tx,
            &message,
            Some(attempt.attempt_id.as_str()),
            crate::MailboxReceiptKind::Claimed,
            /*payload_json*/ None,
            now_ms,
        )
        .await?;
        tx.commit().await?;
        Ok(Some(MailboxClaim { message, attempt }))
    }

    pub async fn ack_message(
        &self,
        params: MailboxAckParams,
    ) -> anyhow::Result<Option<crate::MailboxMessage>> {
        let now_ms = datetime_to_epoch_millis(params.now);
        let receipt_payload_json = params
            .receipt_payload_json
            .map(|payload| serde_json::to_string(&payload))
            .transpose()?;
        let mut tx = self.pool.begin().await?;
        let Some(attempt) = select_attempt_for_lease_in_tx(
            &mut tx,
            &params.message_id,
            &params.attempt_id,
            &params.lease_id,
        )
        .await?
        else {
            tx.rollback().await?;
            return Ok(None);
        };
        match attempt.status {
            crate::MailboxDeliveryAttemptStatus::Acknowledged => {
                let message = select_message_by_id_in_tx(&mut tx, &params.message_id).await?;
                tx.commit().await?;
                return Ok(message);
            }
            crate::MailboxDeliveryAttemptStatus::Claimed => {}
            crate::MailboxDeliveryAttemptStatus::Failed
            | crate::MailboxDeliveryAttemptStatus::Expired => {
                tx.rollback().await?;
                return Ok(None);
            }
        }

        sqlx::query(
            r#"
UPDATE thread_mailbox_delivery_attempts
SET status = ?, completed_at_ms = ?
WHERE attempt_id = ? AND message_id = ? AND lease_id = ? AND status = ?
"#,
        )
        .bind(crate::MailboxDeliveryAttemptStatus::Acknowledged.as_str())
        .bind(now_ms)
        .bind(params.attempt_id.as_str())
        .bind(params.message_id.as_str())
        .bind(params.lease_id.as_str())
        .bind(crate::MailboxDeliveryAttemptStatus::Claimed.as_str())
        .execute(&mut *tx)
        .await?;
        let sql = mailbox_message_returning(
            r#"
UPDATE thread_mailbox_messages
SET
    status = ?,
    lease_id = NULL,
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    acknowledged_at_ms = ?,
    terminal_at_ms = ?,
    updated_at_ms = ?
WHERE message_id = ? AND lease_id = ? AND status = ?
RETURNING
"#,
        );
        let message_row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(crate::MailboxMessageStatus::Acknowledged.as_str())
            .bind(now_ms)
            .bind(now_ms)
            .bind(now_ms)
            .bind(params.message_id.as_str())
            .bind(params.lease_id.as_str())
            .bind(crate::MailboxMessageStatus::Claimed.as_str())
            .fetch_optional(&mut *tx)
            .await?;
        let Some(message_row) = message_row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let message = mailbox_message_from_row(&message_row)?;
        insert_mailbox_receipt_raw_payload(
            &mut tx,
            &message,
            Some(params.attempt_id.as_str()),
            crate::MailboxReceiptKind::Acknowledged,
            receipt_payload_json,
            now_ms,
        )
        .await?;
        tx.commit().await?;
        Ok(Some(message))
    }

    pub async fn fail_message(
        &self,
        params: MailboxFailParams,
    ) -> anyhow::Result<Option<crate::MailboxMessage>> {
        let now_ms = datetime_to_epoch_millis(params.now);
        let mut tx = self.pool.begin().await?;
        let Some(attempt) = select_attempt_for_lease_in_tx(
            &mut tx,
            &params.message_id,
            &params.attempt_id,
            &params.lease_id,
        )
        .await?
        else {
            tx.rollback().await?;
            return Ok(None);
        };
        match attempt.status {
            crate::MailboxDeliveryAttemptStatus::Failed => {
                let message = select_message_by_id_in_tx(&mut tx, &params.message_id).await?;
                tx.commit().await?;
                return Ok(message);
            }
            crate::MailboxDeliveryAttemptStatus::Claimed => {}
            crate::MailboxDeliveryAttemptStatus::Acknowledged
            | crate::MailboxDeliveryAttemptStatus::Expired => {
                tx.rollback().await?;
                return Ok(None);
            }
        }

        sqlx::query(
            r#"
UPDATE thread_mailbox_delivery_attempts
SET status = ?, completed_at_ms = ?, error = ?
WHERE attempt_id = ? AND message_id = ? AND lease_id = ? AND status = ?
"#,
        )
        .bind(crate::MailboxDeliveryAttemptStatus::Failed.as_str())
        .bind(now_ms)
        .bind(params.error.as_str())
        .bind(params.attempt_id.as_str())
        .bind(params.message_id.as_str())
        .bind(params.lease_id.as_str())
        .bind(crate::MailboxDeliveryAttemptStatus::Claimed.as_str())
        .execute(&mut *tx)
        .await?;

        let message = match params.disposition {
            MailboxFailDisposition::Retry { next_attempt_at } => {
                update_failed_claim_for_retry(
                    &mut tx,
                    &params.message_id,
                    &params.lease_id,
                    &params.error,
                    datetime_to_epoch_millis(next_attempt_at),
                    now_ms,
                )
                .await?
            }
            MailboxFailDisposition::Terminal => {
                update_failed_claim_terminal(
                    &mut tx,
                    &params.message_id,
                    &params.lease_id,
                    &params.error,
                    now_ms,
                )
                .await?
            }
        };
        let Some(message) = message else {
            tx.rollback().await?;
            return Ok(None);
        };
        let receipt_kind = match message.status {
            crate::MailboxMessageStatus::Poisoned => {
                insert_mailbox_dead_letter(
                    &mut tx,
                    &message.message_id,
                    &params.attempt_id,
                    &params.error,
                    now_ms,
                )
                .await?;
                crate::MailboxReceiptKind::Poisoned
            }
            crate::MailboxMessageStatus::Failed => crate::MailboxReceiptKind::Failed,
            crate::MailboxMessageStatus::Queued => crate::MailboxReceiptKind::Failed,
            crate::MailboxMessageStatus::Claimed
            | crate::MailboxMessageStatus::Acknowledged
            | crate::MailboxMessageStatus::Expired
            | crate::MailboxMessageStatus::Canceled => {
                tx.rollback().await?;
                return Ok(None);
            }
        };
        insert_mailbox_receipt(
            &mut tx,
            &message,
            Some(params.attempt_id.as_str()),
            receipt_kind,
            Some(serde_json::json!({ "error": params.error })),
            now_ms,
        )
        .await?;
        tx.commit().await?;
        Ok(Some(message))
    }

    pub async fn list_receipts(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Vec<crate::MailboxReceipt>> {
        list_mailbox_receipts(self.pool.as_ref(), message_id).await
    }
}
