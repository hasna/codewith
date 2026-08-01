use chrono::DateTime;
use chrono::Utc;
use codex_protocol::protocol::ReviewEnvelope;
use codex_protocol::protocol::ReviewPublisherEvent;
use codex_protocol::protocol::ReviewPublisherEventKind;
use codex_protocol::protocol::ReviewPublisherVerdict;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPublisherRunStatus {
    Started,
    Completed,
}

impl ReviewPublisherRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }
}

impl TryFrom<&str> for ReviewPublisherRunStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            other => anyhow::bail!("unknown review publisher run status `{other}`"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPublisherOutboxStatus {
    Pending,
    InFlight,
    Delivered,
    DeadLetter,
}

impl ReviewPublisherOutboxStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Delivered => "delivered",
            Self::DeadLetter => "dead_letter",
        }
    }
}

impl TryFrom<&str> for ReviewPublisherOutboxStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_flight" => Ok(Self::InFlight),
            "delivered" => Ok(Self::Delivered),
            "dead_letter" => Ok(Self::DeadLetter),
            other => anyhow::bail!("unknown review publisher outbox status `{other}`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPublisherRun {
    pub review_run_id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub envelope: ReviewEnvelope,
    pub envelope_sha256: String,
    pub status: ReviewPublisherRunStatus,
    pub verdict: Option<ReviewPublisherVerdict>,
    pub terminal_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPublisherOutboxEvent {
    pub event_id: String,
    pub review_run_id: String,
    pub event_kind: ReviewPublisherEventKind,
    pub sequence: u8,
    pub status: ReviewPublisherOutboxStatus,
    pub payload: ReviewPublisherEvent,
    pub payload_sha256: String,
    pub attempt_count: u32,
    pub next_attempt_at: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub receipt_id: Option<String>,
    pub last_error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPublisherRunSnapshot {
    pub run: ReviewPublisherRun,
    pub events: Vec<ReviewPublisherOutboxEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPublisherOutboxClaim {
    pub event: ReviewPublisherOutboxEvent,
}
