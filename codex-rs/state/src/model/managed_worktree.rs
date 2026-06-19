use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use serde_json::Value as JsonValue;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use std::path::PathBuf;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedWorktreeMode {
    IsolatedWorktree,
    SharedRepository,
}

impl ManagedWorktreeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IsolatedWorktree => "isolated_worktree",
            Self::SharedRepository => "shared_repository",
        }
    }
}

impl TryFrom<&str> for ManagedWorktreeMode {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "isolated_worktree" => Ok(Self::IsolatedWorktree),
            "shared_repository" => Ok(Self::SharedRepository),
            other => Err(anyhow!("unknown managed worktree mode `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedWorktreeLifecycleStatus {
    Active,
    Released,
    CleanupPending,
    Deleted,
}

impl ManagedWorktreeLifecycleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
            Self::CleanupPending => "cleanup_pending",
            Self::Deleted => "deleted",
        }
    }
}

impl TryFrom<&str> for ManagedWorktreeLifecycleStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "released" => Ok(Self::Released),
            "cleanup_pending" => Ok(Self::CleanupPending),
            "deleted" => Ok(Self::Deleted),
            other => Err(anyhow!(
                "unknown managed worktree lifecycle status `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedWorktreeCleanupPolicy {
    Retain,
    DeleteIfClean,
    ForceDelete,
}

impl ManagedWorktreeCleanupPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::DeleteIfClean => "delete_if_clean",
            Self::ForceDelete => "force_delete",
        }
    }
}

impl TryFrom<&str> for ManagedWorktreeCleanupPolicy {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "retain" => Ok(Self::Retain),
            "delete_if_clean" => Ok(Self::DeleteIfClean),
            "force_delete" => Ok(Self::ForceDelete),
            other => Err(anyhow!("unknown managed worktree cleanup policy `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedWorktreeOwnerKind {
    Manual,
    MainSession,
    SubSession,
    BackgroundAgent,
}

impl ManagedWorktreeOwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::MainSession => "main_session",
            Self::SubSession => "sub_session",
            Self::BackgroundAgent => "background_agent",
        }
    }
}

impl TryFrom<&str> for ManagedWorktreeOwnerKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "manual" => Ok(Self::Manual),
            "main_session" => Ok(Self::MainSession),
            "sub_session" => Ok(Self::SubSession),
            "background_agent" => Ok(Self::BackgroundAgent),
            other => Err(anyhow!("unknown managed worktree owner kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedWorktreeMergeCandidateStatus {
    Open,
    Blocked,
    Applied,
    Dismissed,
}

impl ManagedWorktreeMergeCandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Blocked => "blocked",
            Self::Applied => "applied",
            Self::Dismissed => "dismissed",
        }
    }
}

impl TryFrom<&str> for ManagedWorktreeMergeCandidateStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "open" => Ok(Self::Open),
            "blocked" => Ok(Self::Blocked),
            "applied" => Ok(Self::Applied),
            "dismissed" => Ok(Self::Dismissed),
            other => Err(anyhow!(
                "unknown managed worktree merge candidate status `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktree {
    pub worktree_id: String,
    pub identity: Option<String>,
    pub mode: ManagedWorktreeMode,
    pub base_repo_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub lifecycle_status: ManagedWorktreeLifecycleStatus,
    pub status_snapshot_json: JsonValue,
    pub dirty: bool,
    pub cleanup_policy: ManagedWorktreeCleanupPolicy,
    pub force_delete_requested: bool,
    pub owner_kind: ManagedWorktreeOwnerKind,
    pub owner_thread_id: Option<ThreadId>,
    pub owner_agent_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub cleanup_after: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktreeMergeCandidate {
    pub candidate_id: String,
    pub worktree_id: String,
    pub target_ref: String,
    pub target_sha: Option<String>,
    pub base_sha: String,
    pub head_sha: String,
    pub status: ManagedWorktreeMergeCandidateStatus,
    pub conflict_summary: Option<String>,
    pub test_summary_json: Option<JsonValue>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
    pub dismissed_at: Option<DateTime<Utc>>,
}

pub(crate) struct ManagedWorktreeRow {
    pub worktree_id: String,
    pub identity: Option<String>,
    pub mode: String,
    pub base_repo_path: String,
    pub worktree_path: String,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub lifecycle_status: String,
    pub status_snapshot_json: String,
    pub dirty: i64,
    pub cleanup_policy: String,
    pub force_delete_requested: i64,
    pub owner_kind: String,
    pub owner_thread_id: Option<String>,
    pub owner_agent_run_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub released_at_ms: Option<i64>,
    pub cleanup_after_ms: Option<i64>,
    pub deleted_at_ms: Option<i64>,
}

pub(crate) struct ManagedWorktreeMergeCandidateRow {
    pub candidate_id: String,
    pub worktree_id: String,
    pub target_ref: String,
    pub target_sha: Option<String>,
    pub base_sha: String,
    pub head_sha: String,
    pub status: String,
    pub conflict_summary: Option<String>,
    pub test_summary_json: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub applied_at_ms: Option<i64>,
    pub dismissed_at_ms: Option<i64>,
}

impl ManagedWorktreeRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            worktree_id: row.try_get("worktree_id")?,
            identity: row.try_get("identity")?,
            mode: row.try_get("mode")?,
            base_repo_path: row.try_get("base_repo_path")?,
            worktree_path: row.try_get("worktree_path")?,
            branch: row.try_get("branch")?,
            base_sha: row.try_get("base_sha")?,
            head_sha: row.try_get("head_sha")?,
            lifecycle_status: row.try_get("lifecycle_status")?,
            status_snapshot_json: row.try_get("status_snapshot_json")?,
            dirty: row.try_get("dirty")?,
            cleanup_policy: row.try_get("cleanup_policy")?,
            force_delete_requested: row.try_get("force_delete_requested")?,
            owner_kind: row.try_get("owner_kind")?,
            owner_thread_id: row.try_get("owner_thread_id")?,
            owner_agent_run_id: row.try_get("owner_agent_run_id")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            released_at_ms: row.try_get("released_at_ms")?,
            cleanup_after_ms: row.try_get("cleanup_after_ms")?,
            deleted_at_ms: row.try_get("deleted_at_ms")?,
        })
    }
}

impl ManagedWorktreeMergeCandidateRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            candidate_id: row.try_get("candidate_id")?,
            worktree_id: row.try_get("worktree_id")?,
            target_ref: row.try_get("target_ref")?,
            target_sha: row.try_get("target_sha")?,
            base_sha: row.try_get("base_sha")?,
            head_sha: row.try_get("head_sha")?,
            status: row.try_get("status")?,
            conflict_summary: row.try_get("conflict_summary")?,
            test_summary_json: row.try_get("test_summary_json")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            applied_at_ms: row.try_get("applied_at_ms")?,
            dismissed_at_ms: row.try_get("dismissed_at_ms")?,
        })
    }
}

impl TryFrom<ManagedWorktreeRow> for ManagedWorktree {
    type Error = anyhow::Error;

    fn try_from(row: ManagedWorktreeRow) -> Result<Self> {
        Ok(Self {
            worktree_id: row.worktree_id,
            identity: row.identity,
            mode: ManagedWorktreeMode::try_from(row.mode.as_str())?,
            base_repo_path: PathBuf::from(row.base_repo_path),
            worktree_path: PathBuf::from(row.worktree_path),
            branch: row.branch,
            base_sha: row.base_sha,
            head_sha: row.head_sha,
            lifecycle_status: ManagedWorktreeLifecycleStatus::try_from(
                row.lifecycle_status.as_str(),
            )?,
            status_snapshot_json: serde_json::from_str(row.status_snapshot_json.as_str())?,
            dirty: row.dirty != 0,
            cleanup_policy: ManagedWorktreeCleanupPolicy::try_from(row.cleanup_policy.as_str())?,
            force_delete_requested: row.force_delete_requested != 0,
            owner_kind: ManagedWorktreeOwnerKind::try_from(row.owner_kind.as_str())?,
            owner_thread_id: row
                .owner_thread_id
                .map(|thread_id| ThreadId::from_string(&thread_id))
                .transpose()?,
            owner_agent_run_id: row.owner_agent_run_id,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            released_at: row
                .released_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            cleanup_after: row
                .cleanup_after_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            deleted_at: row
                .deleted_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
        })
    }
}

impl TryFrom<ManagedWorktreeMergeCandidateRow> for ManagedWorktreeMergeCandidate {
    type Error = anyhow::Error;

    fn try_from(row: ManagedWorktreeMergeCandidateRow) -> Result<Self> {
        Ok(Self {
            candidate_id: row.candidate_id,
            worktree_id: row.worktree_id,
            target_ref: row.target_ref,
            target_sha: row.target_sha,
            base_sha: row.base_sha,
            head_sha: row.head_sha,
            status: ManagedWorktreeMergeCandidateStatus::try_from(row.status.as_str())?,
            conflict_summary: row.conflict_summary,
            test_summary_json: row
                .test_summary_json
                .map(|value| serde_json::from_str(value.as_str()))
                .transpose()?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            applied_at: row
                .applied_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            dismissed_at: row
                .dismissed_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
        })
    }
}
