use super::CommandExecutionApprovalDecision;
use super::DynamicToolCallOutputContentItem;
use super::FileChangeApprovalDecision;
use super::GrantedPermissionProfile;
use super::McpServerElicitationAction;
use super::PermissionGrantScope;
use super::ToolRequestUserInputAnswer;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadPendingInteractionSourceKind {
    Thread,
    BackgroundAgent,
    Goal,
    UsageProfile,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadPendingInteractionKind {
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadPendingInteractionStatus {
    Pending,
    Delivered,
    Responded,
    Expired,
    Cancelled,
    Denied,
    NoLongerWaiting,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadPendingInteractionTerminalStatus {
    Responded,
    Expired,
    Cancelled,
    Denied,
    NoLongerWaiting,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadPendingInteractionEventKind {
    Created,
    Delivered,
    Responded,
    Expired,
    Cancelled,
    Denied,
    NoLongerWaiting,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionListParams {
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub statuses: Option<Vec<ThreadPendingInteractionStatus>>,
    #[ts(optional = nullable)]
    pub kinds: Option<Vec<ThreadPendingInteractionKind>>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionListResponse {
    pub data: Vec<ThreadPendingInteraction>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionReadParams {
    pub interaction_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionReadResponse {
    pub interaction: ThreadPendingInteraction,
    pub events: Vec<ThreadPendingInteractionEvent>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionRespondParams {
    pub interaction_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    pub terminal_status: ThreadPendingInteractionTerminalStatus,
    pub response: ThreadPendingInteractionResponsePayload,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionRespondResponse {
    pub updated: bool,
    pub interaction: Option<ThreadPendingInteraction>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteraction {
    pub interaction_id: String,
    pub thread_id: String,
    pub source_kind: ThreadPendingInteractionSourceKind,
    pub source_id: Option<String>,
    pub turn_id: Option<String>,
    pub worker_request_id: Option<String>,
    pub kind: ThreadPendingInteractionKind,
    pub status: ThreadPendingInteractionStatus,
    pub request_payload: JsonValue,
    pub request_payload_sha256: String,
    pub request_payload_preview: String,
    pub request_redactions: Vec<String>,
    pub response_payload: Option<JsonValue>,
    pub response_payload_sha256: Option<String>,
    pub response_payload_preview: Option<String>,
    pub response_redactions: Vec<String>,
    pub no_client_policy: String,
    #[ts(type = "number | null")]
    pub timeout_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub delivered_at: Option<i64>,
    #[ts(type = "number | null")]
    pub responded_at: Option<i64>,
    #[ts(type = "number | null")]
    pub terminal_at: Option<i64>,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPendingInteractionEvent {
    pub event_id: String,
    pub interaction_id: String,
    pub thread_id: String,
    pub event_kind: ThreadPendingInteractionEventKind,
    pub status: ThreadPendingInteractionStatus,
    pub payload: JsonValue,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions: Vec<String>,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", export_to = "v2/")]
pub enum ThreadPendingInteractionResponsePayload {
    #[serde(rename_all = "camelCase")]
    CommandApproval {
        decision: CommandExecutionApprovalDecision,
    },
    #[serde(rename_all = "camelCase")]
    FileChangeApproval {
        decision: FileChangeApprovalDecision,
    },
    #[serde(rename_all = "camelCase")]
    RequestUserInput {
        answers: HashMap<String, ToolRequestUserInputAnswer>,
    },
    #[serde(rename_all = "camelCase")]
    McpElicitation {
        action: McpServerElicitationAction,
        content: Option<JsonValue>,
        meta: Option<JsonValue>,
    },
    #[serde(rename_all = "camelCase")]
    PermissionsApproval {
        permissions: GrantedPermissionProfile,
        scope: PermissionGrantScope,
        strict_auto_review: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    DynamicTool {
        content_items: Vec<DynamicToolCallOutputContentItem>,
        success: bool,
    },
    #[serde(rename_all = "camelCase")]
    Terminal { reason: String },
}
