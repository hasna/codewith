use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActiveSessionListParams {
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to no limit.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActiveSessionListResponse {
    pub data: Vec<ActiveSessionPeer>,
    /// Opaque cursor to pass to the next call to continue after the last item.
    /// if None, there are no more items to return.
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActiveSessionPeer {
    pub peer_id: String,
    pub kind: ActiveSessionPeerKind,
    pub thread_id: String,
    pub session_id: String,
    pub cwd: AbsolutePathBuf,
    pub display_name: Option<String>,
    pub agent_path: Option<String>,
    pub capabilities: Vec<ActiveSessionCapability>,
    pub last_seen_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ActiveSessionPeerKind {
    CodewithSession,
    SpawnedAgent,
    BridgeAdapter,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ActiveSessionCapability {
    ReceiveMessage,
    QueueMessage,
    TriggerTurn,
    ClaudeChannelBridge,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActiveSessionSendParams {
    pub target_thread_id: String,
    pub message: String,
    #[ts(optional = nullable)]
    pub sender_thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub sender_label: Option<String>,
    #[ts(optional = nullable)]
    pub delivery: Option<ActiveSessionMessageDelivery>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ActiveSessionMessageDelivery {
    QueueOnly,
    TriggerTurn,
}

impl ActiveSessionMessageDelivery {
    pub fn trigger_turn(self) -> bool {
        match self {
            Self::QueueOnly => false,
            Self::TriggerTurn => true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActiveSessionSendResponse {
    pub status: ActiveSessionSendStatus,
    pub message_id: String,
    pub target_thread_id: String,
    pub sender_thread_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ActiveSessionSendStatus {
    Delivered,
    NotLoaded,
}
