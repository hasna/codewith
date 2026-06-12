use super::epoch_seconds_to_datetime;
use super::run::BackgroundAgentDesiredState;
use super::run::BackgroundAgentRunStatus;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAgentStatusSnapshot {
    pub run_id: String,
    pub seq: i64,
    pub status: BackgroundAgentRunStatus,
    pub desired_state: BackgroundAgentDesiredState,
    pub summary: Option<String>,
    pub pending_interaction_count: i64,
    pub last_event_seq: i64,
    pub payload_json: Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentStatusSnapshotParams {
    pub run_id: String,
    pub seq: i64,
    pub status: BackgroundAgentRunStatus,
    pub desired_state: BackgroundAgentDesiredState,
    pub summary: Option<String>,
    pub pending_interaction_count: i64,
    pub last_event_seq: i64,
    pub payload_json: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAgentExecutionSnapshot {
    pub id: i64,
    pub run_id: String,
    pub seq: i64,
    pub snapshot_kind: String,
    pub payload_json: Value,
    pub recovery_policy: String,
    pub config_fingerprint: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentExecutionSnapshotParams {
    pub run_id: String,
    pub snapshot_kind: String,
    pub payload_json: Value,
    pub recovery_policy: String,
    pub config_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentExecutionSnapshotForSupervisorParams<'a> {
    pub snapshot: BackgroundAgentExecutionSnapshotParams,
    pub supervisor_id: &'a str,
    pub generation: i64,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentStatusSnapshotForSupervisorParams<'a> {
    pub snapshot: BackgroundAgentStatusSnapshotParams,
    pub supervisor_id: &'a str,
    pub generation: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackgroundAgentStatusSnapshotRow {
    pub run_id: String,
    pub seq: i64,
    pub status: String,
    pub desired_state: String,
    pub summary: Option<String>,
    pub pending_interaction_count: i64,
    pub last_event_seq: i64,
    pub payload_json: String,
    pub updated_at: i64,
}

impl TryFrom<BackgroundAgentStatusSnapshotRow> for BackgroundAgentStatusSnapshot {
    type Error = anyhow::Error;

    fn try_from(value: BackgroundAgentStatusSnapshotRow) -> Result<Self, Self::Error> {
        Ok(Self {
            run_id: value.run_id,
            seq: value.seq,
            status: BackgroundAgentRunStatus::parse(value.status.as_str())?,
            desired_state: BackgroundAgentDesiredState::parse(value.desired_state.as_str())?,
            summary: value.summary,
            pending_interaction_count: value.pending_interaction_count,
            last_event_seq: value.last_event_seq,
            payload_json: serde_json::from_str(value.payload_json.as_str())?,
            updated_at: epoch_seconds_to_datetime(value.updated_at)?,
        })
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackgroundAgentExecutionSnapshotRow {
    pub id: i64,
    pub run_id: String,
    pub seq: i64,
    pub snapshot_kind: String,
    pub payload_json: String,
    pub recovery_policy: String,
    pub config_fingerprint: Option<String>,
    pub created_at: i64,
}

impl TryFrom<BackgroundAgentExecutionSnapshotRow> for BackgroundAgentExecutionSnapshot {
    type Error = anyhow::Error;

    fn try_from(value: BackgroundAgentExecutionSnapshotRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            run_id: value.run_id,
            seq: value.seq,
            snapshot_kind: value.snapshot_kind,
            payload_json: serde_json::from_str(value.payload_json.as_str())?,
            recovery_policy: value.recovery_policy,
            config_fingerprint: value.config_fingerprint,
            created_at: epoch_seconds_to_datetime(value.created_at)?,
        })
    }
}
