use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailboxMessageKind {
    UserInstruction,
    UserReply,
    Control,
}

impl MailboxMessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserInstruction => "user_instruction",
            Self::UserReply => "user_reply",
            Self::Control => "control",
        }
    }
}

impl TryFrom<&str> for MailboxMessageKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "user_instruction" => Ok(Self::UserInstruction),
            "user_reply" => Ok(Self::UserReply),
            "control" => Ok(Self::Control),
            other => Err(anyhow!("unknown mailbox message kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailboxMessageStatus {
    Queued,
    Claimed,
    Acknowledged,
    Failed,
    Poisoned,
    Expired,
    Canceled,
}

impl MailboxMessageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Acknowledged => "acknowledged",
            Self::Failed => "failed",
            Self::Poisoned => "poisoned",
            Self::Expired => "expired",
            Self::Canceled => "canceled",
        }
    }
}

impl TryFrom<&str> for MailboxMessageStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "claimed" => Ok(Self::Claimed),
            "acknowledged" => Ok(Self::Acknowledged),
            "failed" => Ok(Self::Failed),
            "poisoned" => Ok(Self::Poisoned),
            "expired" => Ok(Self::Expired),
            "canceled" => Ok(Self::Canceled),
            other => Err(anyhow!("unknown mailbox message status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailboxDeliveryAttemptStatus {
    Claimed,
    Acknowledged,
    Failed,
    Expired,
}

impl MailboxDeliveryAttemptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Acknowledged => "acknowledged",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }
}

impl TryFrom<&str> for MailboxDeliveryAttemptStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "claimed" => Ok(Self::Claimed),
            "acknowledged" => Ok(Self::Acknowledged),
            "failed" => Ok(Self::Failed),
            "expired" => Ok(Self::Expired),
            other => Err(anyhow!("unknown mailbox delivery attempt status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailboxReceiptKind {
    Enqueued,
    Claimed,
    Acknowledged,
    Failed,
    Poisoned,
    Canceled,
    Expired,
    LeaseExpired,
}

impl MailboxReceiptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enqueued => "enqueued",
            Self::Claimed => "claimed",
            Self::Acknowledged => "acknowledged",
            Self::Failed => "failed",
            Self::Poisoned => "poisoned",
            Self::Canceled => "canceled",
            Self::Expired => "expired",
            Self::LeaseExpired => "lease_expired",
        }
    }
}

impl TryFrom<&str> for MailboxReceiptKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "enqueued" => Ok(Self::Enqueued),
            "claimed" => Ok(Self::Claimed),
            "acknowledged" => Ok(Self::Acknowledged),
            "failed" => Ok(Self::Failed),
            "poisoned" => Ok(Self::Poisoned),
            "canceled" => Ok(Self::Canceled),
            "expired" => Ok(Self::Expired),
            "lease_expired" => Ok(Self::LeaseExpired),
            other => Err(anyhow!("unknown mailbox receipt kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MailboxMessage {
    pub message_id: String,
    pub target_thread_id: ThreadId,
    pub sender_thread_id: Option<ThreadId>,
    pub sender_label: Option<String>,
    pub idempotency_key: Option<String>,
    pub kind: MailboxMessageKind,
    pub status: MailboxMessageStatus,
    pub payload_json: Value,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub priority: i64,
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub next_attempt_at: DateTime<Utc>,
    pub lease_id: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub last_attempt_id: Option<String>,
    pub last_error: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub terminal_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxDeliveryAttempt {
    pub attempt_id: String,
    pub message_id: String,
    pub lease_id: String,
    pub lease_owner: String,
    pub attempt_number: i64,
    pub status: MailboxDeliveryAttemptStatus,
    pub claimed_at: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MailboxReceipt {
    pub receipt_id: String,
    pub message_id: String,
    pub attempt_id: Option<String>,
    pub thread_id: ThreadId,
    pub kind: MailboxReceiptKind,
    pub status_after: MailboxMessageStatus,
    pub payload_json: Option<Value>,
    pub created_at: DateTime<Utc>,
}

pub(crate) struct MailboxMessageRow {
    pub message_id: String,
    pub target_thread_id: String,
    pub sender_thread_id: Option<String>,
    pub sender_label: Option<String>,
    pub idempotency_key: Option<String>,
    pub kind: String,
    pub status: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub priority: i64,
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub next_attempt_at_ms: i64,
    pub lease_id: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub last_attempt_id: Option<String>,
    pub last_error: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub acknowledged_at_ms: Option<i64>,
    pub terminal_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl MailboxMessageRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            message_id: row.try_get("message_id")?,
            target_thread_id: row.try_get("target_thread_id")?,
            sender_thread_id: row.try_get("sender_thread_id")?,
            sender_label: row.try_get("sender_label")?,
            idempotency_key: row.try_get("idempotency_key")?,
            kind: row.try_get("kind")?,
            status: row.try_get("status")?,
            payload_json: row.try_get("payload_json")?,
            payload_sha256: row.try_get("payload_sha256")?,
            payload_preview: row.try_get("payload_preview")?,
            priority: row.try_get("priority")?,
            attempt_count: row.try_get("attempt_count")?,
            max_attempts: row.try_get("max_attempts")?,
            next_attempt_at_ms: row.try_get("next_attempt_at_ms")?,
            lease_id: row.try_get("lease_id")?,
            lease_owner: row.try_get("lease_owner")?,
            lease_expires_at_ms: row.try_get("lease_expires_at_ms")?,
            last_attempt_id: row.try_get("last_attempt_id")?,
            last_error: row.try_get("last_error")?,
            expires_at_ms: row.try_get("expires_at_ms")?,
            acknowledged_at_ms: row.try_get("acknowledged_at_ms")?,
            terminal_at_ms: row.try_get("terminal_at_ms")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<MailboxMessageRow> for MailboxMessage {
    type Error = anyhow::Error;

    fn try_from(row: MailboxMessageRow) -> Result<Self> {
        Ok(Self {
            message_id: row.message_id,
            target_thread_id: ThreadId::try_from(row.target_thread_id)?,
            sender_thread_id: row.sender_thread_id.map(ThreadId::try_from).transpose()?,
            sender_label: row.sender_label,
            idempotency_key: row.idempotency_key,
            kind: MailboxMessageKind::try_from(row.kind.as_str())?,
            status: MailboxMessageStatus::try_from(row.status.as_str())?,
            payload_json: serde_json::from_str(&row.payload_json)?,
            payload_sha256: row.payload_sha256,
            payload_preview: row.payload_preview,
            priority: row.priority,
            attempt_count: row.attempt_count,
            max_attempts: row.max_attempts,
            next_attempt_at: epoch_millis_to_datetime(row.next_attempt_at_ms)?,
            lease_id: row.lease_id,
            lease_owner: row.lease_owner,
            lease_expires_at: optional_epoch_millis_to_datetime(row.lease_expires_at_ms)?,
            last_attempt_id: row.last_attempt_id,
            last_error: row.last_error,
            expires_at: optional_epoch_millis_to_datetime(row.expires_at_ms)?,
            acknowledged_at: optional_epoch_millis_to_datetime(row.acknowledged_at_ms)?,
            terminal_at: optional_epoch_millis_to_datetime(row.terminal_at_ms)?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

pub(crate) struct MailboxDeliveryAttemptRow {
    pub attempt_id: String,
    pub message_id: String,
    pub lease_id: String,
    pub lease_owner: String,
    pub attempt_number: i64,
    pub status: String,
    pub claimed_at_ms: i64,
    pub lease_expires_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub error: Option<String>,
}

impl MailboxDeliveryAttemptRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            attempt_id: row.try_get("attempt_id")?,
            message_id: row.try_get("message_id")?,
            lease_id: row.try_get("lease_id")?,
            lease_owner: row.try_get("lease_owner")?,
            attempt_number: row.try_get("attempt_number")?,
            status: row.try_get("status")?,
            claimed_at_ms: row.try_get("claimed_at_ms")?,
            lease_expires_at_ms: row.try_get("lease_expires_at_ms")?,
            completed_at_ms: row.try_get("completed_at_ms")?,
            error: row.try_get("error")?,
        })
    }
}

impl TryFrom<MailboxDeliveryAttemptRow> for MailboxDeliveryAttempt {
    type Error = anyhow::Error;

    fn try_from(row: MailboxDeliveryAttemptRow) -> Result<Self> {
        Ok(Self {
            attempt_id: row.attempt_id,
            message_id: row.message_id,
            lease_id: row.lease_id,
            lease_owner: row.lease_owner,
            attempt_number: row.attempt_number,
            status: MailboxDeliveryAttemptStatus::try_from(row.status.as_str())?,
            claimed_at: epoch_millis_to_datetime(row.claimed_at_ms)?,
            lease_expires_at: epoch_millis_to_datetime(row.lease_expires_at_ms)?,
            completed_at: optional_epoch_millis_to_datetime(row.completed_at_ms)?,
            error: row.error,
        })
    }
}

pub(crate) struct MailboxReceiptRow {
    pub receipt_id: String,
    pub message_id: String,
    pub attempt_id: Option<String>,
    pub thread_id: String,
    pub kind: String,
    pub status_after: String,
    pub payload_json: Option<String>,
    pub created_at_ms: i64,
}

impl MailboxReceiptRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            receipt_id: row.try_get("receipt_id")?,
            message_id: row.try_get("message_id")?,
            attempt_id: row.try_get("attempt_id")?,
            thread_id: row.try_get("thread_id")?,
            kind: row.try_get("kind")?,
            status_after: row.try_get("status_after")?,
            payload_json: row.try_get("payload_json")?,
            created_at_ms: row.try_get("created_at_ms")?,
        })
    }
}

impl TryFrom<MailboxReceiptRow> for MailboxReceipt {
    type Error = anyhow::Error;

    fn try_from(row: MailboxReceiptRow) -> Result<Self> {
        Ok(Self {
            receipt_id: row.receipt_id,
            message_id: row.message_id,
            attempt_id: row.attempt_id,
            thread_id: ThreadId::try_from(row.thread_id)?,
            kind: MailboxReceiptKind::try_from(row.kind.as_str())?,
            status_after: MailboxMessageStatus::try_from(row.status_after.as_str())?,
            payload_json: row
                .payload_json
                .map(|payload| serde_json::from_str(&payload))
                .transpose()?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
        })
    }
}

fn optional_epoch_millis_to_datetime(value: Option<i64>) -> Result<Option<DateTime<Utc>>> {
    value.map(epoch_millis_to_datetime).transpose()
}
