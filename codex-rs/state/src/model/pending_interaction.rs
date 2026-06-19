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
pub enum PendingInteractionSourceKind {
    Thread,
    BackgroundAgent,
    Goal,
    UsageProfile,
}

impl PendingInteractionSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thread => "thread",
            Self::BackgroundAgent => "background_agent",
            Self::Goal => "goal",
            Self::UsageProfile => "usage_profile",
        }
    }
}

impl TryFrom<&str> for PendingInteractionSourceKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "thread" => Ok(Self::Thread),
            "background_agent" => Ok(Self::BackgroundAgent),
            "goal" => Ok(Self::Goal),
            "usage_profile" => Ok(Self::UsageProfile),
            other => Err(anyhow!("unknown pending interaction source kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PendingInteractionKind {
    CommandApproval,
    FileChangeApproval,
    UserInput,
    McpElicitation,
    PermissionGrant,
    DynamicTool,
    UsageLimit,
    ProfileSwitch,
    Blocked,
}

impl PendingInteractionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandApproval => "command_approval",
            Self::FileChangeApproval => "file_change_approval",
            Self::UserInput => "user_input",
            Self::McpElicitation => "mcp_elicitation",
            Self::PermissionGrant => "permission_grant",
            Self::DynamicTool => "dynamic_tool",
            Self::UsageLimit => "usage_limit",
            Self::ProfileSwitch => "profile_switch",
            Self::Blocked => "blocked",
        }
    }
}

impl TryFrom<&str> for PendingInteractionKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "command_approval" => Ok(Self::CommandApproval),
            "file_change_approval" => Ok(Self::FileChangeApproval),
            "user_input" => Ok(Self::UserInput),
            "mcp_elicitation" => Ok(Self::McpElicitation),
            "permission_grant" => Ok(Self::PermissionGrant),
            "dynamic_tool" => Ok(Self::DynamicTool),
            "usage_limit" => Ok(Self::UsageLimit),
            "profile_switch" => Ok(Self::ProfileSwitch),
            "blocked" => Ok(Self::Blocked),
            other => Err(anyhow!("unknown pending interaction kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PendingInteractionStatus {
    Pending,
    Delivered,
    Responded,
    Expired,
    Cancelled,
    Denied,
    NoLongerWaiting,
}

impl PendingInteractionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Responded => "responded",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Denied => "denied",
            Self::NoLongerWaiting => "no_longer_waiting",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Delivered)
    }
}

impl TryFrom<&str> for PendingInteractionStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "delivered" => Ok(Self::Delivered),
            "responded" => Ok(Self::Responded),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            "denied" => Ok(Self::Denied),
            "no_longer_waiting" => Ok(Self::NoLongerWaiting),
            other => Err(anyhow!("unknown pending interaction status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingInteraction {
    pub interaction_id: String,
    pub thread_id: ThreadId,
    pub source_kind: PendingInteractionSourceKind,
    pub source_id: Option<String>,
    pub turn_id: Option<String>,
    pub worker_request_id: Option<String>,
    pub server_request_id_json: Option<Value>,
    pub kind: PendingInteractionKind,
    pub status: PendingInteractionStatus,
    pub request_payload_json: Value,
    pub request_payload_sha256: String,
    pub request_payload_preview: String,
    pub request_redactions_json: Value,
    pub response_payload_json: Option<Value>,
    pub response_payload_sha256: Option<String>,
    pub response_payload_preview: Option<String>,
    pub response_redactions_json: Option<Value>,
    pub no_client_policy: String,
    pub timeout_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub responded_at: Option<DateTime<Utc>>,
    pub terminal_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PendingInteractionEventKind {
    Created,
    Delivered,
    Responded,
    Expired,
    Cancelled,
    Denied,
    NoLongerWaiting,
}

impl PendingInteractionEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Delivered => "delivered",
            Self::Responded => "responded",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Denied => "denied",
            Self::NoLongerWaiting => "no_longer_waiting",
        }
    }

    pub fn from_terminal_status(status: PendingInteractionStatus) -> Result<Self> {
        match status {
            PendingInteractionStatus::Responded => Ok(Self::Responded),
            PendingInteractionStatus::Expired => Ok(Self::Expired),
            PendingInteractionStatus::Cancelled => Ok(Self::Cancelled),
            PendingInteractionStatus::Denied => Ok(Self::Denied),
            PendingInteractionStatus::NoLongerWaiting => Ok(Self::NoLongerWaiting),
            PendingInteractionStatus::Pending | PendingInteractionStatus::Delivered => {
                Err(anyhow!(
                    "pending interaction event status must be terminal, got `{}`",
                    status.as_str()
                ))
            }
        }
    }
}

impl TryFrom<&str> for PendingInteractionEventKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "created" => Ok(Self::Created),
            "delivered" => Ok(Self::Delivered),
            "responded" => Ok(Self::Responded),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            "denied" => Ok(Self::Denied),
            "no_longer_waiting" => Ok(Self::NoLongerWaiting),
            other => Err(anyhow!("unknown pending interaction event kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingInteractionEvent {
    pub event_id: String,
    pub interaction_id: String,
    pub thread_id: ThreadId,
    pub event_kind: PendingInteractionEventKind,
    pub status: PendingInteractionStatus,
    pub payload_json: Value,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PendingInteractionCreateParams {
    pub interaction_id: String,
    pub thread_id: ThreadId,
    pub source_kind: PendingInteractionSourceKind,
    pub source_id: Option<String>,
    pub turn_id: Option<String>,
    pub worker_request_id: Option<String>,
    pub server_request_id_json: Option<Value>,
    pub kind: PendingInteractionKind,
    pub request_payload_json: Value,
    pub request_payload_preview: String,
    pub request_redactions_json: Value,
    pub no_client_policy: String,
    pub timeout_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct PendingInteractionRespondParams {
    pub interaction_id: String,
    pub response_payload_json: Value,
    pub response_payload_preview: String,
    pub response_redactions_json: Value,
    pub terminal_status: PendingInteractionStatus,
}

pub(crate) struct PendingInteractionRow {
    pub interaction_id: String,
    pub thread_id: String,
    pub source_kind: String,
    pub source_id: Option<String>,
    pub turn_id: Option<String>,
    pub worker_request_id: Option<String>,
    pub server_request_id_json: Option<String>,
    pub kind: String,
    pub status: String,
    pub request_payload_json: String,
    pub request_payload_sha256: String,
    pub request_payload_preview: String,
    pub request_redactions_json: String,
    pub response_payload_json: Option<String>,
    pub response_payload_sha256: Option<String>,
    pub response_payload_preview: Option<String>,
    pub response_redactions_json: Option<String>,
    pub no_client_policy: String,
    pub timeout_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub delivered_at_ms: Option<i64>,
    pub responded_at_ms: Option<i64>,
    pub terminal_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

pub(crate) struct PendingInteractionEventRow {
    pub event_id: String,
    pub interaction_id: String,
    pub thread_id: String,
    pub event_kind: String,
    pub status: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions_json: String,
    pub created_at_ms: i64,
}

impl PendingInteractionRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            interaction_id: row.try_get("interaction_id")?,
            thread_id: row.try_get("thread_id")?,
            source_kind: row.try_get("source_kind")?,
            source_id: row.try_get("source_id")?,
            turn_id: row.try_get("turn_id")?,
            worker_request_id: row.try_get("worker_request_id")?,
            server_request_id_json: row.try_get("server_request_id_json")?,
            kind: row.try_get("kind")?,
            status: row.try_get("status")?,
            request_payload_json: row.try_get("request_payload_json")?,
            request_payload_sha256: row.try_get("request_payload_sha256")?,
            request_payload_preview: row.try_get("request_payload_preview")?,
            request_redactions_json: row.try_get("request_redactions_json")?,
            response_payload_json: row.try_get("response_payload_json")?,
            response_payload_sha256: row.try_get("response_payload_sha256")?,
            response_payload_preview: row.try_get("response_payload_preview")?,
            response_redactions_json: row.try_get("response_redactions_json")?,
            no_client_policy: row.try_get("no_client_policy")?,
            timeout_at_ms: row.try_get("timeout_at_ms")?,
            created_at_ms: row.try_get("created_at_ms")?,
            delivered_at_ms: row.try_get("delivered_at_ms")?,
            responded_at_ms: row.try_get("responded_at_ms")?,
            terminal_at_ms: row.try_get("terminal_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl PendingInteractionEventRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            event_id: row.try_get("event_id")?,
            interaction_id: row.try_get("interaction_id")?,
            thread_id: row.try_get("thread_id")?,
            event_kind: row.try_get("event_kind")?,
            status: row.try_get("status")?,
            payload_json: row.try_get("payload_json")?,
            payload_sha256: row.try_get("payload_sha256")?,
            payload_preview: row.try_get("payload_preview")?,
            redactions_json: row.try_get("redactions_json")?,
            created_at_ms: row.try_get("created_at_ms")?,
        })
    }
}

impl TryFrom<PendingInteractionRow> for PendingInteraction {
    type Error = anyhow::Error;

    fn try_from(row: PendingInteractionRow) -> Result<Self> {
        Ok(Self {
            interaction_id: row.interaction_id,
            thread_id: ThreadId::try_from(row.thread_id)?,
            source_kind: PendingInteractionSourceKind::try_from(row.source_kind.as_str())?,
            source_id: row.source_id,
            turn_id: row.turn_id,
            worker_request_id: row.worker_request_id,
            server_request_id_json: row
                .server_request_id_json
                .map(|payload| serde_json::from_str(&payload))
                .transpose()?,
            kind: PendingInteractionKind::try_from(row.kind.as_str())?,
            status: PendingInteractionStatus::try_from(row.status.as_str())?,
            request_payload_json: serde_json::from_str(&row.request_payload_json)?,
            request_payload_sha256: row.request_payload_sha256,
            request_payload_preview: row.request_payload_preview,
            request_redactions_json: serde_json::from_str(&row.request_redactions_json)?,
            response_payload_json: row
                .response_payload_json
                .map(|payload| serde_json::from_str(&payload))
                .transpose()?,
            response_payload_sha256: row.response_payload_sha256,
            response_payload_preview: row.response_payload_preview,
            response_redactions_json: row
                .response_redactions_json
                .map(|payload| serde_json::from_str(&payload))
                .transpose()?,
            no_client_policy: row.no_client_policy,
            timeout_at: optional_epoch_millis_to_datetime(row.timeout_at_ms)?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            delivered_at: optional_epoch_millis_to_datetime(row.delivered_at_ms)?,
            responded_at: optional_epoch_millis_to_datetime(row.responded_at_ms)?,
            terminal_at: optional_epoch_millis_to_datetime(row.terminal_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

impl TryFrom<PendingInteractionEventRow> for PendingInteractionEvent {
    type Error = anyhow::Error;

    fn try_from(row: PendingInteractionEventRow) -> Result<Self> {
        Ok(Self {
            event_id: row.event_id,
            interaction_id: row.interaction_id,
            thread_id: ThreadId::try_from(row.thread_id)?,
            event_kind: PendingInteractionEventKind::try_from(row.event_kind.as_str())?,
            status: PendingInteractionStatus::try_from(row.status.as_str())?,
            payload_json: serde_json::from_str(&row.payload_json)?,
            payload_sha256: row.payload_sha256,
            payload_preview: row.payload_preview,
            redactions_json: serde_json::from_str(&row.redactions_json)?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
        })
    }
}

fn optional_epoch_millis_to_datetime(value: Option<i64>) -> Result<Option<DateTime<Utc>>> {
    value.map(epoch_millis_to_datetime).transpose()
}
