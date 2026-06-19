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
    /// Active peer routing key returned by activeSession/list. For local
    /// Codewith sessions this matches thread_id; bridge peers may use a
    /// transport-scoped id.
    #[serde(default)]
    pub peer_id: String,
    #[serde(default)]
    pub kind: ActiveSessionPeerKind,
    /// Codewith thread that owns the active peer registration.
    pub thread_id: String,
    /// Live session instance id for the loaded thread. This is not a durable
    /// mailbox or remote machine id.
    #[serde(default)]
    pub session_id: String,
    pub cwd: AbsolutePathBuf,
    pub display_name: Option<String>,
    pub agent_path: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<ActiveSessionCapability>,
    pub last_seen_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ActiveSessionPeerKind {
    #[default]
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
    /// Legacy local Codewith-thread target. target_peer_id is preferred for
    /// peers returned by activeSession/list.
    #[ts(optional = nullable)]
    pub target_thread_id: Option<String>,
    /// Active peer routing key from activeSession/list.
    #[ts(optional = nullable)]
    pub target_peer_id: Option<String>,
    pub message: String,
    /// Claimed sender thread for attribution only. activeSession/send does not
    /// treat this as authentication or authorization.
    #[ts(optional = nullable)]
    pub sender_thread_id: Option<String>,
    /// Claimed sender label for attribution only.
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
    /// App-server-generated id for this active delivery attempt. It is not a
    /// durable mailbox id or read receipt.
    pub message_id: String,
    #[serde(default)]
    pub target_peer_id: String,
    pub target_thread_id: Option<String>,
    /// Echoes the claimed sender_thread_id when present and valid.
    pub sender_thread_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ActiveSessionSendStatus {
    Delivered,
    NotLoaded,
    Unsupported,
}
