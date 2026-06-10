use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadMonitorRouting {
    Stream,
    File,
    Both,
}

impl ThreadMonitorRouting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::File => "file",
            Self::Both => "both",
        }
    }

    pub fn streams_to_thread(self) -> bool {
        matches!(self, Self::Stream | Self::Both)
    }

    pub fn writes_to_file(self) -> bool {
        matches!(self, Self::File | Self::Both)
    }
}

impl TryFrom<&str> for ThreadMonitorRouting {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "stream" => Ok(Self::Stream),
            "file" => Ok(Self::File),
            "both" => Ok(Self::Both),
            other => Err(anyhow!("unknown thread monitor routing `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadMonitorStatus {
    Running,
    Stopped,
    Failed,
}

impl ThreadMonitorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for ThreadMonitorStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "stopped" => Ok(Self::Stopped),
            "failed" => Ok(Self::Failed),
            other => Err(anyhow!("unknown thread monitor status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadMonitorEventStream {
    Stdout,
    Stderr,
    System,
}

impl ThreadMonitorEventStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::System => "system",
        }
    }
}

impl TryFrom<&str> for ThreadMonitorEventStream {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "stdout" => Ok(Self::Stdout),
            "stderr" => Ok(Self::Stderr),
            "system" => Ok(Self::System),
            other => Err(anyhow!("unknown thread monitor event stream `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMonitor {
    pub thread_id: ThreadId,
    pub monitor_id: String,
    pub name: String,
    pub prompt: String,
    pub command: String,
    pub cwd: Option<String>,
    pub routing: ThreadMonitorRouting,
    pub output_file: Option<String>,
    pub status: ThreadMonitorStatus,
    pub generation: i64,
    pub process_id: Option<i64>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMonitorEvent {
    pub thread_id: ThreadId,
    pub monitor_id: String,
    pub event_id: String,
    pub stream: ThreadMonitorEventStream,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

pub(crate) struct ThreadMonitorRow {
    pub thread_id: String,
    pub monitor_id: String,
    pub name: String,
    pub prompt: String,
    pub command: String,
    pub cwd: Option<String>,
    pub routing: String,
    pub output_file: Option<String>,
    pub status: String,
    pub generation: i64,
    pub process_id: Option<i64>,
    pub last_event_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ThreadMonitorRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            monitor_id: row.try_get("monitor_id")?,
            name: row.try_get("name")?,
            prompt: row.try_get("prompt")?,
            command: row.try_get("command")?,
            cwd: row.try_get("cwd")?,
            routing: row.try_get("routing")?,
            output_file: row.try_get("output_file")?,
            status: row.try_get("status")?,
            generation: row.try_get("generation")?,
            process_id: row.try_get("process_id")?,
            last_event_at_ms: row.try_get("last_event_at_ms")?,
            last_error: row.try_get("last_error")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<ThreadMonitorRow> for ThreadMonitor {
    type Error = anyhow::Error;

    fn try_from(row: ThreadMonitorRow) -> Result<Self> {
        Ok(Self {
            thread_id: ThreadId::try_from(row.thread_id)?,
            monitor_id: row.monitor_id,
            name: row.name,
            prompt: row.prompt,
            command: row.command,
            cwd: row.cwd,
            routing: ThreadMonitorRouting::try_from(row.routing.as_str())?,
            output_file: row.output_file,
            status: ThreadMonitorStatus::try_from(row.status.as_str())?,
            generation: row.generation,
            process_id: row.process_id,
            last_event_at: optional_epoch_millis_to_datetime(row.last_event_at_ms)?,
            last_error: row.last_error,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

pub(crate) struct ThreadMonitorEventRow {
    pub thread_id: String,
    pub monitor_id: String,
    pub event_id: String,
    pub stream: String,
    pub text: String,
    pub created_at_ms: i64,
}

impl ThreadMonitorEventRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            monitor_id: row.try_get("monitor_id")?,
            event_id: row.try_get("event_id")?,
            stream: row.try_get("stream")?,
            text: row.try_get("text")?,
            created_at_ms: row.try_get("created_at_ms")?,
        })
    }
}

impl TryFrom<ThreadMonitorEventRow> for ThreadMonitorEvent {
    type Error = anyhow::Error;

    fn try_from(row: ThreadMonitorEventRow) -> Result<Self> {
        Ok(Self {
            thread_id: ThreadId::try_from(row.thread_id)?,
            monitor_id: row.monitor_id,
            event_id: row.event_id,
            stream: ThreadMonitorEventStream::try_from(row.stream.as_str())?,
            text: row.text,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
        })
    }
}

fn optional_epoch_millis_to_datetime(value: Option<i64>) -> Result<Option<DateTime<Utc>>> {
    value.map(epoch_millis_to_datetime).transpose()
}
