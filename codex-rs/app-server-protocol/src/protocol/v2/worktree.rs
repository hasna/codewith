use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

use super::AgentRun;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum WorktreeMode {
    IsolatedWorktree,
    SharedRepository,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum WorktreeLifecycleStatus {
    Active,
    CleanupPending,
    Released,
    Deleted,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum WorktreeCleanupPolicy {
    Retain,
    DeleteIfClean,
    ForceDelete,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum WorktreeOwnerKind {
    Manual,
    MainSession,
    SubSession,
    BackgroundAgent,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum WorktreeSessionMode {
    Off,
    Manual,
    Auto,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreePolicy {
    pub enabled: bool,
    #[ts(type = "string | null")]
    pub root: Option<String>,
    pub cleanup_default: WorktreeCleanupPolicy,
    pub main_sessions: WorktreeSessionMode,
    pub sub_sessions: WorktreeSessionMode,
    #[ts(type = "string | null")]
    pub current_base_repo_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct Worktree {
    pub worktree_id: String,
    #[ts(type = "string | null")]
    pub agent_id: Option<String>,
    #[ts(type = "string | null")]
    pub identity: Option<String>,
    pub mode: WorktreeMode,
    pub lifecycle_status: WorktreeLifecycleStatus,
    pub base_repo_path: String,
    pub worktree_path: String,
    #[ts(type = "string | null")]
    pub branch: Option<String>,
    #[ts(type = "string | null")]
    pub base_sha: Option<String>,
    #[ts(type = "string | null")]
    pub head_sha: Option<String>,
    pub status_snapshot: JsonValue,
    pub dirty: bool,
    pub cleanup_policy: WorktreeCleanupPolicy,
    #[ts(type = "number | null")]
    pub cleanup_after: Option<i64>,
    pub force_delete_requested: bool,
    pub owner_kind: WorktreeOwnerKind,
    #[ts(type = "string | null")]
    pub owner_thread_id: Option<String>,
    #[ts(type = "string | null")]
    pub owner_agent_run_id: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number | null")]
    pub released_at: Option<i64>,
    #[ts(type = "number | null")]
    pub deleted_at: Option<i64>,
    pub agent: Option<AgentRun>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeListParams {
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
    #[ts(optional = nullable)]
    pub include_deleted: Option<bool>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeListResponse {
    pub data: Vec<Worktree>,
    pub next_cursor: Option<String>,
    pub policy: WorktreePolicy,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeReadParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeReadResponse {
    pub worktree: Option<Worktree>,
    pub policy: WorktreePolicy,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeAttachParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub agent_run_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeAttachResponse {
    pub worktree: Worktree,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeDetachParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub agent_run_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeDetachResponse {
    pub worktree: Option<Worktree>,
}
