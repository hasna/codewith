use super::epoch_seconds_to_datetime;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;

pub const BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED: &str =
    "background agent event cursor has been compacted";

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAgentEvent {
    pub id: i64,
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    pub payload_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAgentWorkerWriteScope {
    Active,
    CurrentOrTerminal,
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentEventForSupervisorParams<'a> {
    pub run_id: &'a str,
    pub supervisor_id: &'a str,
    pub generation: i64,
    pub write_scope: BackgroundAgentWorkerWriteScope,
    pub event_type: &'a str,
    pub payload_json: &'a Value,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackgroundAgentEventRow {
    pub id: i64,
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: i64,
}

impl TryFrom<BackgroundAgentEventRow> for BackgroundAgentEvent {
    type Error = anyhow::Error;

    fn try_from(value: BackgroundAgentEventRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            run_id: value.run_id,
            seq: value.seq,
            event_type: value.event_type,
            payload_json: serde_json::from_str(value.payload_json.as_str())?,
            created_at: epoch_seconds_to_datetime(value.created_at)?,
        })
    }
}
