use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxEnqueueParams {
    pub target_thread_id: String,
    #[ts(optional = nullable)]
    pub sender_thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub sender_label: Option<String>,
    #[ts(optional = nullable)]
    pub idempotency_key: Option<String>,
    pub kind: ThreadMailboxMessageKind,
    pub message: JsonValue,
    #[ts(optional = nullable)]
    pub preview: Option<String>,
    #[ts(optional = nullable)]
    pub priority: Option<i64>,
    #[ts(optional = nullable)]
    pub max_attempts: Option<u32>,
    #[ts(type = "number | null", optional = nullable)]
    pub next_attempt_at: Option<i64>,
    #[ts(type = "number | null", optional = nullable)]
    pub expires_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxEnqueueResponse {
    pub message: ThreadMailboxMessageSummary,
    pub created: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxListParams {
    pub target_thread_id: String,
    #[ts(optional = nullable)]
    pub statuses: Option<Vec<ThreadMailboxMessageStatus>>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxListResponse {
    pub data: Vec<ThreadMailboxMessageSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxReadParams {
    pub target_thread_id: String,
    pub message_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxReadResponse {
    pub message: ThreadMailboxMessageDetail,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxClaimParams {
    pub target_thread_id: String,
    #[ts(optional = nullable)]
    pub lease_owner: Option<String>,
    #[ts(optional = nullable)]
    pub lease_seconds: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxClaimResponse {
    pub claim: Option<ThreadMailboxClaim>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxAckParams {
    pub target_thread_id: String,
    pub message_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    #[ts(optional = nullable)]
    pub receipt: Option<JsonValue>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxAckResponse {
    pub message: ThreadMailboxMessageSummary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxFailParams {
    pub target_thread_id: String,
    pub message_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub disposition: ThreadMailboxFailDisposition,
    pub error: String,
    #[ts(type = "number | null", optional = nullable)]
    pub retry_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxFailResponse {
    pub message: ThreadMailboxMessageSummary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxReceiptsListParams {
    pub target_thread_id: String,
    pub message_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxReceiptsListResponse {
    pub data: Vec<ThreadMailboxReceipt>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxMessageSummary {
    pub message_id: String,
    pub target_thread_id: String,
    pub sender_thread_id: Option<String>,
    pub sender_label: Option<String>,
    pub kind: ThreadMailboxMessageKind,
    pub status: ThreadMailboxMessageStatus,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions: Vec<ThreadMailboxRedaction>,
    pub priority: i64,
    pub attempt_count: i64,
    pub max_attempts: i64,
    #[ts(type = "number")]
    pub next_attempt_at: i64,
    #[ts(type = "number | null")]
    pub lease_expires_at: Option<i64>,
    pub last_error: Option<String>,
    #[ts(type = "number | null")]
    pub expires_at: Option<i64>,
    #[ts(type = "number | null")]
    pub acknowledged_at: Option<i64>,
    #[ts(type = "number | null")]
    pub terminal_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxMessageDetail {
    pub summary: ThreadMailboxMessageSummary,
    pub message: JsonValue,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxClaim {
    pub message: ThreadMailboxMessageDetail,
    pub attempt: ThreadMailboxDeliveryAttempt,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxDeliveryAttempt {
    pub attempt_id: String,
    pub lease_id: String,
    pub lease_owner: String,
    pub attempt_number: i64,
    #[ts(type = "number")]
    pub claimed_at: i64,
    #[ts(type = "number")]
    pub lease_expires_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMailboxReceipt {
    pub receipt_id: String,
    pub message_id: String,
    pub attempt_id: Option<String>,
    pub thread_id: String,
    pub kind: ThreadMailboxReceiptKind,
    pub status_after: ThreadMailboxMessageStatus,
    pub payload: Option<JsonValue>,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMailboxMessageKind {
    UserInstruction,
    UserReply,
    Control,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMailboxMessageStatus {
    Queued,
    Claimed,
    Acknowledged,
    Failed,
    Poisoned,
    Expired,
    Canceled,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMailboxFailDisposition {
    Retry,
    Terminal,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMailboxReceiptKind {
    Enqueued,
    Claimed,
    Acknowledged,
    Failed,
    Poisoned,
    Canceled,
    Expired,
    LeaseExpired,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMailboxRedaction {
    MessageBody,
    IdempotencyKey,
}
