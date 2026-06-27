use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use serde_json::Value;

pub struct MailboxEnqueueParams {
    pub target_thread_id: ThreadId,
    pub sender_thread_id: Option<ThreadId>,
    pub sender_label: Option<String>,
    pub idempotency_key: Option<String>,
    pub kind: crate::MailboxMessageKind,
    pub payload_json: Value,
    pub payload_preview: String,
    pub priority: i64,
    pub max_attempts: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct MailboxEnqueueOutcome {
    pub message: crate::MailboxMessage,
    pub created: bool,
}

pub struct MailboxMessageStoreListParams {
    pub target_thread_id: Option<ThreadId>,
    pub statuses: Vec<crate::MailboxMessageStatus>,
    pub cursor: Option<String>,
    pub limit: u32,
}

pub struct MailboxMessagePage {
    pub data: Vec<crate::MailboxMessage>,
    pub next_cursor: Option<String>,
}

pub struct MailboxClaimParams {
    pub target_thread_id: ThreadId,
    pub lease_owner: String,
    pub lease_duration: std::time::Duration,
    pub now: DateTime<Utc>,
}

pub struct MailboxDispatchClaimParams {
    pub lease_owner: String,
    pub lease_duration: std::time::Duration,
    pub now: DateTime<Utc>,
    pub local_active_owner_id: String,
    pub local_active_fresh_after: DateTime<Utc>,
}

pub struct MailboxClaim {
    pub message: crate::MailboxMessage,
    pub attempt: crate::MailboxDeliveryAttempt,
}

pub struct MailboxAckParams {
    pub message_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub receipt_payload_json: Option<Value>,
    pub now: DateTime<Utc>,
}

pub enum MailboxFailDisposition {
    Retry { next_attempt_at: DateTime<Utc> },
    Terminal,
}

pub struct MailboxFailParams {
    pub message_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub error: String,
    pub disposition: MailboxFailDisposition,
    pub now: DateTime<Utc>,
}
