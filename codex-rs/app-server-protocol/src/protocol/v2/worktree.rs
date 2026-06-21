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
pub enum WorktreeMergeCandidateStatus {
    Open,
    Blocked,
    Applied,
    Dismissed,
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
pub struct WorktreeMergeCandidate {
    pub candidate_id: String,
    pub worktree_id: String,
    pub target_ref: String,
    #[ts(type = "string | null")]
    pub target_sha: Option<String>,
    pub base_sha: String,
    pub head_sha: String,
    pub status: WorktreeMergeCandidateStatus,
    #[ts(type = "string | null")]
    pub conflict_summary: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number | null")]
    pub applied_at: Option<i64>,
    #[ts(type = "number | null")]
    pub dismissed_at: Option<i64>,
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
pub struct WorktreeCreateParams {
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
    #[ts(optional = nullable)]
    pub name: Option<String>,
    #[ts(optional = nullable)]
    pub branch: Option<String>,
    #[ts(optional = nullable)]
    pub start_point: Option<String>,
    #[ts(optional = nullable)]
    pub cleanup_policy: Option<WorktreeCleanupPolicy>,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeCreateResponse {
    pub worktree: Worktree,
    pub policy: WorktreePolicy,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeReconcileParams {
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeReconcileResponse {
    pub data: Vec<Worktree>,
    pub policy: WorktreePolicy,
    pub discovered: u32,
    pub updated: u32,
    pub deleted: u32,
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeReleaseParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub cleanup_policy: Option<WorktreeCleanupPolicy>,
    #[ts(optional = nullable)]
    pub force_delete: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeReleaseResponse {
    pub worktree: Option<Worktree>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeCleanupParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub force_delete: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeCleanupResponse {
    pub worktree: Option<Worktree>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateListParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub status: Option<WorktreeMergeCandidateStatus>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateListResponse {
    pub data: Vec<WorktreeMergeCandidate>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateRefreshParams {
    pub worktree_id: String,
    #[ts(optional = nullable)]
    pub target_ref: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateRefreshResponse {
    pub candidate: WorktreeMergeCandidate,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateApplyParams {
    pub candidate_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateApplyResponse {
    pub candidate: Option<WorktreeMergeCandidate>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateDismissParams {
    pub candidate_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorktreeMergeCandidateDismissResponse {
    pub candidate: Option<WorktreeMergeCandidate>,
}
