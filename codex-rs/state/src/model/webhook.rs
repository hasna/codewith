use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WebhookEventStatus {
    Unread,
    Processed,
    Archived,
    Injected,
    Queued,
}

impl WebhookEventStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Processed => "processed",
            Self::Archived => "archived",
            Self::Injected => "injected",
            Self::Queued => "queued",
        }
    }
}

impl TryFrom<&str> for WebhookEventStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "unread" => Ok(Self::Unread),
            "processed" => Ok(Self::Processed),
            "archived" => Ok(Self::Archived),
            "injected" => Ok(Self::Injected),
            "queued" => Ok(Self::Queued),
            other => Err(anyhow!("unknown webhook event status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookPayloadRedaction {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebhookEvent {
    pub event_id: String,
    pub source_app_id: String,
    pub source_app_name: Option<String>,
    pub subscription_id: Option<String>,
    pub event_type: String,
    pub external_delivery_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub target_thread_id: Option<ThreadId>,
    pub status: WebhookEventStatus,
    pub payload_json: Value,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions: Vec<WebhookPayloadRedaction>,
    pub received_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) struct WebhookEventRow {
    pub event_id: String,
    pub source_app_id: String,
    pub source_app_name: Option<String>,
    pub subscription_id: Option<String>,
    pub event_type: String,
    pub external_delivery_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub target_thread_id: Option<String>,
    pub status: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions_json: String,
    pub received_at_ms: i64,
    pub updated_at_ms: i64,
}

impl WebhookEventRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            event_id: row.try_get("event_id")?,
            source_app_id: row.try_get("source_app_id")?,
            source_app_name: row.try_get("source_app_name")?,
            subscription_id: row.try_get("subscription_id")?,
            event_type: row.try_get("event_type")?,
            external_delivery_id: row.try_get("external_delivery_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            target_thread_id: row.try_get("target_thread_id")?,
            status: row.try_get("status")?,
            payload_json: row.try_get("payload_json")?,
            payload_sha256: row.try_get("payload_sha256")?,
            payload_preview: row.try_get("payload_preview")?,
            redactions_json: row.try_get("redactions_json")?,
            received_at_ms: row.try_get("received_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<WebhookEventRow> for WebhookEvent {
    type Error = anyhow::Error;

    fn try_from(row: WebhookEventRow) -> Result<Self> {
        Ok(Self {
            event_id: row.event_id,
            source_app_id: row.source_app_id,
            source_app_name: row.source_app_name,
            subscription_id: row.subscription_id,
            event_type: row.event_type,
            external_delivery_id: row.external_delivery_id,
            idempotency_key: row.idempotency_key,
            target_thread_id: row.target_thread_id.map(ThreadId::try_from).transpose()?,
            status: WebhookEventStatus::try_from(row.status.as_str())?,
            payload_json: serde_json::from_str(&row.payload_json)?,
            payload_sha256: row.payload_sha256,
            payload_preview: row.payload_preview,
            redactions: serde_json::from_str(&row.redactions_json)?,
            received_at: epoch_millis_to_datetime(row.received_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}
