use super::epoch_seconds_to_datetime;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentPendingInteractionKind {
    Approval,
    UserInput,
    McpElicitation,
    PermissionGrant,
}

impl BackgroundAgentPendingInteractionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            BackgroundAgentPendingInteractionKind::Approval => "approval",
            BackgroundAgentPendingInteractionKind::UserInput => "user_input",
            BackgroundAgentPendingInteractionKind::McpElicitation => "mcp_elicitation",
            BackgroundAgentPendingInteractionKind::PermissionGrant => "permission_grant",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "approval" => Ok(Self::Approval),
            "user_input" => Ok(Self::UserInput),
            "mcp_elicitation" => Ok(Self::McpElicitation),
            "permission_grant" => Ok(Self::PermissionGrant),
            _ => Err(anyhow::anyhow!(
                "invalid background agent pending interaction kind: {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentPendingInteractionStatus {
    Pending,
    Delivered,
    Responded,
    Expired,
    Cancelled,
    Denied,
    WorkerNoLongerWaiting,
}

impl BackgroundAgentPendingInteractionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            BackgroundAgentPendingInteractionStatus::Pending => "pending",
            BackgroundAgentPendingInteractionStatus::Delivered => "delivered",
            BackgroundAgentPendingInteractionStatus::Responded => "responded",
            BackgroundAgentPendingInteractionStatus::Expired => "expired",
            BackgroundAgentPendingInteractionStatus::Cancelled => "cancelled",
            BackgroundAgentPendingInteractionStatus::Denied => "denied",
            BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting => {
                "worker_no_longer_waiting"
            }
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "delivered" => Ok(Self::Delivered),
            "responded" => Ok(Self::Responded),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            "denied" => Ok(Self::Denied),
            "worker_no_longer_waiting" => Ok(Self::WorkerNoLongerWaiting),
            _ => Err(anyhow::anyhow!(
                "invalid background agent pending interaction status: {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAgentPendingInteraction {
    pub id: String,
    pub run_id: String,
    pub worker_request_id: Option<String>,
    pub kind: BackgroundAgentPendingInteractionKind,
    pub status: BackgroundAgentPendingInteractionStatus,
    pub request_payload_json: Value,
    pub response_payload_json: Option<Value>,
    pub no_client_policy: String,
    pub timeout_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub responded_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentPendingInteractionCreateParams {
    pub id: String,
    pub run_id: String,
    pub worker_request_id: Option<String>,
    pub kind: BackgroundAgentPendingInteractionKind,
    pub request_payload_json: Value,
    pub no_client_policy: String,
    pub timeout_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackgroundAgentPendingInteractionRow {
    pub id: String,
    pub run_id: String,
    pub worker_request_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub request_payload_json: String,
    pub response_payload_json: Option<String>,
    pub no_client_policy: String,
    pub timeout_at: Option<i64>,
    pub created_at: i64,
    pub delivered_at: Option<i64>,
    pub responded_at: Option<i64>,
    pub updated_at: i64,
}

impl TryFrom<BackgroundAgentPendingInteractionRow> for BackgroundAgentPendingInteraction {
    type Error = anyhow::Error;

    fn try_from(value: BackgroundAgentPendingInteractionRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            run_id: value.run_id,
            worker_request_id: value.worker_request_id,
            kind: BackgroundAgentPendingInteractionKind::parse(value.kind.as_str())?,
            status: BackgroundAgentPendingInteractionStatus::parse(value.status.as_str())?,
            request_payload_json: serde_json::from_str(value.request_payload_json.as_str())?,
            response_payload_json: value
                .response_payload_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            no_client_policy: value.no_client_policy,
            timeout_at: value
                .timeout_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            created_at: epoch_seconds_to_datetime(value.created_at)?,
            delivered_at: value
                .delivered_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            responded_at: value
                .responded_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            updated_at: epoch_seconds_to_datetime(value.updated_at)?,
        })
    }
}
