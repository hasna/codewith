use super::epoch_seconds_to_datetime;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentWorkspaceMode {
    IsolatedWorktree,
    SharedRepository,
}

impl BackgroundAgentWorkspaceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            BackgroundAgentWorkspaceMode::IsolatedWorktree => "isolated_worktree",
            BackgroundAgentWorkspaceMode::SharedRepository => "shared_repository",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "isolated_worktree" => Ok(Self::IsolatedWorktree),
            "shared_repository" => Ok(Self::SharedRepository),
            _ => Err(anyhow::anyhow!(
                "invalid background agent workspace mode: {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentWorkspaceCleanup {
    Retain,
    DeleteIfClean,
    ForceDelete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAgentWorktreeLease {
    pub id: String,
    pub run_id: String,
    pub identity: String,
    pub mode: BackgroundAgentWorkspaceMode,
    pub base_repo_path: String,
    pub worktree_path: String,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub status_snapshot_json: Value,
    pub dirty: bool,
    pub cleanup_after: Option<DateTime<Utc>>,
    pub force_delete_requested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentWorktreeLeaseCreateParams {
    pub id: String,
    pub run_id: String,
    pub identity: String,
    pub mode: BackgroundAgentWorkspaceMode,
    pub base_repo_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub status_snapshot_json: Value,
    pub dirty: bool,
    pub cleanup_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackgroundAgentWorktreeLeaseRow {
    pub id: String,
    pub run_id: String,
    pub identity: String,
    pub mode: String,
    pub base_repo_path: String,
    pub worktree_path: String,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub status_snapshot_json: String,
    pub dirty: i64,
    pub cleanup_after: Option<i64>,
    pub force_delete_requested: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub released_at: Option<i64>,
    pub deleted_at: Option<i64>,
}

impl TryFrom<BackgroundAgentWorktreeLeaseRow> for BackgroundAgentWorktreeLease {
    type Error = anyhow::Error;

    fn try_from(value: BackgroundAgentWorktreeLeaseRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            run_id: value.run_id,
            identity: value.identity,
            mode: BackgroundAgentWorkspaceMode::parse(value.mode.as_str())?,
            base_repo_path: value.base_repo_path,
            worktree_path: value.worktree_path,
            branch: value.branch,
            head_sha: value.head_sha,
            status_snapshot_json: serde_json::from_str(value.status_snapshot_json.as_str())?,
            dirty: value.dirty != 0,
            cleanup_after: value
                .cleanup_after
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            force_delete_requested: value.force_delete_requested != 0,
            created_at: epoch_seconds_to_datetime(value.created_at)?,
            updated_at: epoch_seconds_to_datetime(value.updated_at)?,
            released_at: value
                .released_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            deleted_at: value
                .deleted_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
        })
    }
}
