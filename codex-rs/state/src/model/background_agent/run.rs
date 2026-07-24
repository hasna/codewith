use super::epoch_seconds_to_datetime;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentRunStatus {
    Queued,
    Starting,
    Running,
    WaitingOnApproval,
    WaitingOnUser,
    Stopping,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
}

impl BackgroundAgentRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            BackgroundAgentRunStatus::Queued => "queued",
            BackgroundAgentRunStatus::Starting => "starting",
            BackgroundAgentRunStatus::Running => "running",
            BackgroundAgentRunStatus::WaitingOnApproval => "waiting_on_approval",
            BackgroundAgentRunStatus::WaitingOnUser => "waiting_on_user",
            BackgroundAgentRunStatus::Stopping => "stopping",
            BackgroundAgentRunStatus::Completed => "completed",
            BackgroundAgentRunStatus::Failed => "failed",
            BackgroundAgentRunStatus::Cancelled => "cancelled",
            BackgroundAgentRunStatus::Orphaned => "orphaned",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "waiting_on_approval" => Ok(Self::WaitingOnApproval),
            "waiting_on_user" => Ok(Self::WaitingOnUser),
            "stopping" => Ok(Self::Stopping),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "orphaned" => Ok(Self::Orphaned),
            _ => Err(anyhow::anyhow!(
                "invalid background agent run status: {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentDesiredState {
    Running,
    Stopped,
    Deleted,
}

impl BackgroundAgentDesiredState {
    pub const fn as_str(self) -> &'static str {
        match self {
            BackgroundAgentDesiredState::Running => "running",
            BackgroundAgentDesiredState::Stopped => "stopped",
            BackgroundAgentDesiredState::Deleted => "deleted",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "stopped" => Ok(Self::Stopped),
            "deleted" => Ok(Self::Deleted),
            _ => Err(anyhow::anyhow!(
                "invalid background agent desired state: {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentRetentionState {
    Active,
    Archived,
    DeleteRequested,
    Deleted,
}

impl BackgroundAgentRetentionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            BackgroundAgentRetentionState::Active => "active",
            BackgroundAgentRetentionState::Archived => "archived",
            BackgroundAgentRetentionState::DeleteRequested => "delete_requested",
            BackgroundAgentRetentionState::Deleted => "deleted",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            "delete_requested" => Ok(Self::DeleteRequested),
            "deleted" => Ok(Self::Deleted),
            _ => Err(anyhow::anyhow!(
                "invalid background agent retention state: {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAgentRun {
    pub id: String,
    pub idempotency_key: Option<String>,
    pub request_id: Option<String>,
    pub source: String,
    pub prompt_snapshot_ref: String,
    pub input_snapshot_ref: Option<String>,
    pub thread_id: Option<String>,
    pub thread_store_kind: String,
    pub thread_store_id: Option<String>,
    pub rollout_path: Option<String>,
    pub parent_thread_id: Option<String>,
    pub parent_agent_run_id: Option<String>,
    pub spawn_linkage_json: Option<Value>,
    pub worktree_lease_id: Option<String>,
    pub auth_profile_ref: Option<String>,
    pub desired_state: BackgroundAgentDesiredState,
    pub status: BackgroundAgentRunStatus,
    pub status_reason: Option<String>,
    pub config_fingerprint: Option<String>,
    pub version_fingerprint: Option<String>,
    pub retention_state: BackgroundAgentRetentionState,
    pub archive_after: Option<DateTime<Utc>>,
    pub delete_after: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub supervisor_id: Option<String>,
    pub generation: i64,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub job_id: Option<String>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub crash_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub exit_signal: Option<i64>,
    pub last_event_seq: i64,
    pub last_snapshot_seq: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub struct BackgroundAgentExecutionHandleParams<'a> {
    pub run_id: &'a str,
    pub supervisor_id: &'a str,
    pub generation: i64,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub job_id: Option<&'a str>,
    pub start_token: Option<&'a str>,
    pub stderr_log_path: Option<&'a str>,
}

pub struct BackgroundAgentStatusEventForSupervisorParams<'a> {
    pub run_id: &'a str,
    pub supervisor_id: &'a str,
    pub generation: i64,
    pub status: BackgroundAgentRunStatus,
    pub status_reason: Option<&'a str>,
    pub event_type: &'a str,
    pub event_payload_json: &'a Value,
    pub summary: Option<&'a str>,
    pub pending_interaction_count: i64,
    pub status_payload_json: &'a Value,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentRunCreateParams {
    pub id: String,
    pub idempotency_key: Option<String>,
    pub request_id: Option<String>,
    pub source: String,
    pub prompt_snapshot_ref: String,
    pub input_snapshot_ref: Option<String>,
    pub thread_id: Option<String>,
    pub thread_store_kind: String,
    pub thread_store_id: Option<String>,
    pub rollout_path: Option<String>,
    pub parent_thread_id: Option<String>,
    pub parent_agent_run_id: Option<String>,
    pub spawn_linkage_json: Option<Value>,
    pub auth_profile_ref: Option<String>,
    pub status_reason: Option<String>,
    pub config_fingerprint: Option<String>,
    pub version_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentThreadBindingParams {
    pub run_id: String,
    pub supervisor_id: String,
    pub generation: i64,
    pub thread_id: String,
    pub thread_store_kind: String,
    pub thread_store_id: Option<String>,
    pub rollout_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundAgentProcessHandleRecord {
    pub run_id: String,
    pub generation: i64,
    pub pid: u32,
    pub pgid: Option<u32>,
    pub start_token: String,
    pub stderr_log_path: PathBuf,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackgroundAgentRunRow {
    pub id: String,
    pub idempotency_key: Option<String>,
    pub request_id: Option<String>,
    pub source: String,
    pub prompt_snapshot_ref: String,
    pub input_snapshot_ref: Option<String>,
    pub thread_id: Option<String>,
    pub thread_store_kind: String,
    pub thread_store_id: Option<String>,
    pub rollout_path: Option<String>,
    pub parent_thread_id: Option<String>,
    pub parent_agent_run_id: Option<String>,
    pub spawn_linkage_json: Option<String>,
    pub worktree_lease_id: Option<String>,
    pub auth_profile_ref: Option<String>,
    pub desired_state: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub config_fingerprint: Option<String>,
    pub version_fingerprint: Option<String>,
    pub retention_state: String,
    pub archive_after: Option<i64>,
    pub delete_after: Option<i64>,
    pub archived_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub supervisor_id: Option<String>,
    pub generation: i64,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub job_id: Option<String>,
    pub heartbeat_at: Option<i64>,
    pub crash_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub exit_signal: Option<i64>,
    pub last_event_seq: i64,
    pub last_snapshot_seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

impl TryFrom<BackgroundAgentRunRow> for BackgroundAgentRun {
    type Error = anyhow::Error;

    fn try_from(value: BackgroundAgentRunRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            // `idempotency_key` is persisted as a one-way digest and
            // `auth_profile_ref` is persisted through state redaction; neither
            // is ever reconstructed back into its original plaintext here.
            idempotency_key: value.idempotency_key,
            request_id: value.request_id,
            source: value.source,
            prompt_snapshot_ref: value.prompt_snapshot_ref,
            input_snapshot_ref: value.input_snapshot_ref,
            thread_id: value.thread_id,
            thread_store_kind: value.thread_store_kind,
            thread_store_id: value.thread_store_id,
            rollout_path: value.rollout_path,
            parent_thread_id: value.parent_thread_id,
            parent_agent_run_id: value.parent_agent_run_id,
            spawn_linkage_json: value
                .spawn_linkage_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            worktree_lease_id: value.worktree_lease_id,
            auth_profile_ref: value.auth_profile_ref,
            desired_state: BackgroundAgentDesiredState::parse(value.desired_state.as_str())?,
            status: BackgroundAgentRunStatus::parse(value.status.as_str())?,
            status_reason: value.status_reason,
            config_fingerprint: value.config_fingerprint,
            version_fingerprint: value.version_fingerprint,
            retention_state: BackgroundAgentRetentionState::parse(value.retention_state.as_str())?,
            archive_after: value
                .archive_after
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            delete_after: value
                .delete_after
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            archived_at: value
                .archived_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            deleted_at: value
                .deleted_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            supervisor_id: value.supervisor_id,
            generation: value.generation,
            pid: value.pid,
            pgid: value.pgid,
            job_id: value.job_id,
            heartbeat_at: value
                .heartbeat_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            crash_reason: value.crash_reason,
            exit_code: value.exit_code,
            exit_signal: value.exit_signal,
            last_event_seq: value.last_event_seq,
            last_snapshot_seq: value.last_snapshot_seq,
            created_at: epoch_seconds_to_datetime(value.created_at)?,
            updated_at: epoch_seconds_to_datetime(value.updated_at)?,
            started_at: value
                .started_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
            completed_at: value
                .completed_at
                .map(epoch_seconds_to_datetime)
                .transpose()?,
        })
    }
}
