use super::ActiveSessionCapability;
use super::ActiveSessionPeerKind;
use super::AuthProfileKind;
use super::SessionSource;
use super::SortDirection;
use super::ThreadActiveFlag;
use super::ThreadListCwdFilter;
use super::ThreadSortKey;
use super::ThreadSource;
use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LocalSessionListParams {
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to a reasonable server-side value.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Optional sort key; defaults to created_at.
    #[ts(optional = nullable)]
    pub sort_key: Option<ThreadSortKey>,
    /// Optional sort direction; defaults to descending (newest first).
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
    /// Optional cwd filter or filters; when set, only sessions whose captured
    /// cwd exactly matches one of these paths are returned.
    #[ts(optional = nullable, type = "string | Array<string> | null")]
    pub cwd: Option<ThreadListCwdFilter>,
    /// Optional runtime status filter. When omitted or empty, all local session
    /// states are returned.
    #[ts(optional = nullable)]
    pub statuses: Option<Vec<LocalSessionStatus>>,
    /// Optional substring filter for the extracted thread title.
    #[ts(optional = nullable)]
    pub search_term: Option<String>,
    /// Optional archived filter; when set to true, only archived sessions are
    /// returned. If false or null, only non-archived sessions are returned.
    #[ts(optional = nullable)]
    pub archived: Option<bool>,
    /// If true, return from the state DB without scanning JSONL rollouts to
    /// repair thread metadata. Omitted or false preserves scan-and-repair
    /// behavior.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_state_db_only: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LocalSessionListResponse {
    pub data: Vec<LocalSession>,
    /// Opaque cursor to pass to the next call to continue after the last item.
    /// if None, there are no more items to return.
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LocalSession {
    /// Durable Codewith thread id for the session record.
    pub thread_id: String,
    /// Live session instance id when known. Persisted-only records return null
    /// because a durable thread id is not the same as a runtime session id.
    pub runtime_session_id: Option<String>,
    /// Live active-session routing metadata when this record can receive
    /// in-memory activeSession/send traffic.
    pub peer: Option<LocalSessionPeer>,
    pub status: LocalSessionStatus,
    /// Active-status flags from the thread runtime. Empty for non-active
    /// sessions.
    pub active_flags: Vec<ThreadActiveFlag>,
    pub cwd: AbsolutePathBuf,
    pub display_name: Option<String>,
    pub agent_path: Option<String>,
    pub model_provider: String,
    pub model: Option<String>,
    /// Auth profile selected for model requests in this session, if known.
    #[serde(default)]
    pub auth_profile: Option<String>,
    /// Whether `authProfile` is unknown, the default root profile, or a named profile.
    #[serde(default)]
    pub auth_profile_kind: AuthProfileKind,
    /// Redacted account label for the selected auth profile, when available.
    #[serde(default)]
    pub account_label: Option<String>,
    pub source: SessionSource,
    pub thread_source: Option<ThreadSource>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub path: Option<PathBuf>,
    pub git_info: Option<LocalSessionGitInfo>,
    pub redactions: Vec<LocalSessionRedaction>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LocalSessionPeer {
    pub peer_id: String,
    pub kind: ActiveSessionPeerKind,
    pub capabilities: Vec<ActiveSessionCapability>,
    #[ts(type = "number")]
    pub last_seen_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum LocalSessionStatus {
    Active,
    Idle,
    SystemError,
    Closing,
    LoadedWithoutActivePeer,
    NotLoaded,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LocalSessionGitInfo {
    pub sha: Option<String>,
    pub branch: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum LocalSessionRedaction {
    GitOriginUrl,
    ProcessDetails,
}
