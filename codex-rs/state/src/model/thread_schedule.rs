use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadSchedulePromptSource {
    Inline,
    Default,
}

impl ThreadSchedulePromptSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Default => "default",
        }
    }
}

impl TryFrom<&str> for ThreadSchedulePromptSource {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "inline" => Ok(Self::Inline),
            "default" => Ok(Self::Default),
            other => Err(anyhow!("unknown thread schedule prompt source `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadScheduleStatus {
    Active,
    Paused,
    Expired,
}

impl ThreadScheduleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Expired => "expired",
        }
    }
}

impl TryFrom<&str> for ThreadScheduleStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "expired" => Ok(Self::Expired),
            other => Err(anyhow!("unknown thread schedule status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadScheduleSpec {
    Once,
    Dynamic,
    Interval(ThreadScheduleInterval),
    Cron { expression: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadScheduleInterval {
    pub amount: i64,
    pub unit: ThreadScheduleIntervalUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadScheduleIntervalUnit {
    Minutes,
    Hours,
    Days,
}

impl ThreadScheduleIntervalUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minutes => "minutes",
            Self::Hours => "hours",
            Self::Days => "days",
        }
    }
}

impl TryFrom<&str> for ThreadScheduleIntervalUnit {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "minutes" => Ok(Self::Minutes),
            "hours" => Ok(Self::Hours),
            "days" => Ok(Self::Days),
            other => Err(anyhow!("unknown thread schedule interval unit `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadScheduleRunStatus {
    Leased,
    Running,
    Deferred,
    Completed,
    Failed,
}

impl ThreadScheduleRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Leased => "leased",
            Self::Running => "running",
            Self::Deferred => "deferred",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for ThreadScheduleRunStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "leased" => Ok(Self::Leased),
            "running" => Ok(Self::Running),
            "deferred" => Ok(Self::Deferred),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(anyhow!("unknown thread schedule run status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSchedule {
    pub thread_id: ThreadId,
    pub schedule_id: String,
    /// Selected auth profile captured when the schedule was created.
    /// `None` means legacy/unknown; `Some(None)` means the root profile.
    pub auth_profile: Option<Option<String>>,
    pub prompt_source: ThreadSchedulePromptSource,
    pub prompt: String,
    pub schedule: ThreadScheduleSpec,
    pub timezone: String,
    pub status: ThreadScheduleStatus,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub failure_count: i64,
    pub lease_id: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadScheduleRun {
    pub thread_id: ThreadId,
    pub schedule_id: String,
    pub run_id: String,
    pub status: ThreadScheduleRunStatus,
    pub lease_id: String,
    pub turn_id: Option<String>,
    pub error: Option<String>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadScheduleStats {
    pub total_runs: i64,
    pub leased_runs: i64,
    pub running_runs: i64,
    pub deferred_runs: i64,
    pub completed_runs: i64,
    pub failed_runs: i64,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub(crate) struct ThreadScheduleRow {
    pub thread_id: String,
    pub schedule_id: String,
    pub prompt_source: String,
    pub prompt: String,
    pub schedule_kind: String,
    pub interval_amount: Option<i64>,
    pub interval_unit: Option<String>,
    pub cron_expression: Option<String>,
    pub timezone: String,
    pub auth_profile_recorded: i64,
    pub auth_profile: Option<String>,
    pub status: String,
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub failure_count: i64,
    pub lease_id: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ThreadScheduleRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            schedule_id: row.try_get("schedule_id")?,
            prompt_source: row.try_get("prompt_source")?,
            prompt: row.try_get("prompt")?,
            schedule_kind: row.try_get("schedule_kind")?,
            interval_amount: row.try_get("interval_amount")?,
            interval_unit: row.try_get("interval_unit")?,
            cron_expression: row.try_get("cron_expression")?,
            timezone: row.try_get("timezone")?,
            auth_profile_recorded: row.try_get("auth_profile_recorded")?,
            auth_profile: row.try_get("auth_profile")?,
            status: row.try_get("status")?,
            next_run_at_ms: row.try_get("next_run_at_ms")?,
            last_run_at_ms: row.try_get("last_run_at_ms")?,
            expires_at_ms: row.try_get("expires_at_ms")?,
            failure_count: row.try_get("failure_count")?,
            lease_id: row.try_get("lease_id")?,
            lease_expires_at_ms: row.try_get("lease_expires_at_ms")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<ThreadScheduleRow> for ThreadSchedule {
    type Error = anyhow::Error;

    fn try_from(row: ThreadScheduleRow) -> Result<Self> {
        Ok(Self {
            thread_id: ThreadId::try_from(row.thread_id)?,
            schedule_id: row.schedule_id,
            auth_profile: match row.auth_profile_recorded {
                0 => None,
                1 => Some(row.auth_profile),
                other => return Err(anyhow!("invalid auth_profile_recorded value `{other}`")),
            },
            prompt_source: ThreadSchedulePromptSource::try_from(row.prompt_source.as_str())?,
            prompt: row.prompt,
            schedule: schedule_from_row_parts(
                row.schedule_kind.as_str(),
                row.interval_amount,
                row.interval_unit.as_deref(),
                row.cron_expression,
            )?,
            timezone: row.timezone,
            status: ThreadScheduleStatus::try_from(row.status.as_str())?,
            next_run_at: optional_epoch_millis_to_datetime(row.next_run_at_ms)?,
            last_run_at: optional_epoch_millis_to_datetime(row.last_run_at_ms)?,
            expires_at: optional_epoch_millis_to_datetime(row.expires_at_ms)?,
            failure_count: row.failure_count,
            lease_id: row.lease_id,
            lease_expires_at: optional_epoch_millis_to_datetime(row.lease_expires_at_ms)?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

pub(crate) struct ThreadScheduleRunRow {
    pub thread_id: String,
    pub schedule_id: String,
    pub run_id: String,
    pub status: String,
    pub lease_id: String,
    pub turn_id: Option<String>,
    pub error: Option<String>,
    pub scheduled_for_ms: Option<i64>,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

impl ThreadScheduleRunRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            schedule_id: row.try_get("schedule_id")?,
            run_id: row.try_get("run_id")?,
            status: row.try_get("status")?,
            lease_id: row.try_get("lease_id")?,
            turn_id: row.try_get("turn_id")?,
            error: row.try_get("error")?,
            scheduled_for_ms: row.try_get("scheduled_for_ms")?,
            started_at_ms: row.try_get("started_at_ms")?,
            completed_at_ms: row.try_get("completed_at_ms")?,
        })
    }
}

impl TryFrom<ThreadScheduleRunRow> for ThreadScheduleRun {
    type Error = anyhow::Error;

    fn try_from(row: ThreadScheduleRunRow) -> Result<Self> {
        Ok(Self {
            thread_id: ThreadId::try_from(row.thread_id)?,
            schedule_id: row.schedule_id,
            run_id: row.run_id,
            status: ThreadScheduleRunStatus::try_from(row.status.as_str())?,
            lease_id: row.lease_id,
            turn_id: row.turn_id,
            error: row.error,
            scheduled_for: optional_epoch_millis_to_datetime(row.scheduled_for_ms)?,
            started_at: epoch_millis_to_datetime(row.started_at_ms)?,
            completed_at: optional_epoch_millis_to_datetime(row.completed_at_ms)?,
        })
    }
}

fn schedule_from_row_parts(
    kind: &str,
    interval_amount: Option<i64>,
    interval_unit: Option<&str>,
    cron_expression: Option<String>,
) -> Result<ThreadScheduleSpec> {
    match kind {
        "once" => Ok(ThreadScheduleSpec::Once),
        "dynamic" => Ok(ThreadScheduleSpec::Dynamic),
        "interval" => {
            let amount = interval_amount
                .ok_or_else(|| anyhow!("interval schedule missing interval_amount"))?;
            let unit = interval_unit
                .ok_or_else(|| anyhow!("interval schedule missing interval_unit"))
                .and_then(ThreadScheduleIntervalUnit::try_from)?;
            Ok(ThreadScheduleSpec::Interval(ThreadScheduleInterval {
                amount,
                unit,
            }))
        }
        "cron" => {
            let expression =
                cron_expression.ok_or_else(|| anyhow!("cron schedule missing cron_expression"))?;
            Ok(ThreadScheduleSpec::Cron { expression })
        }
        other => Err(anyhow!("unknown thread schedule kind `{other}`")),
    }
}

fn optional_epoch_millis_to_datetime(value: Option<i64>) -> Result<Option<DateTime<Utc>>> {
    value.map(epoch_millis_to_datetime).transpose()
}
