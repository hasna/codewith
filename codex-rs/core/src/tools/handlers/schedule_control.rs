//! Built-in model tool handler for managing thread schedules.

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::schedule_control_spec::MANAGE_SCHEDULE_TOOL_NAME;
use crate::tools::handlers::schedule_control_spec::create_manage_schedule_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use chrono::DateTime;
use chrono::Duration as ChronoDuration;
use chrono::Utc;
use chrono_tz::Tz;
use codex_protocol::ThreadId;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use croner::Cron;
use serde::Deserialize;
use serde::Serialize;
use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::Arc;

const DEFAULT_DYNAMIC_INTERVAL_MINUTES: i64 = 1;
const DEFAULT_SCHEDULE_EXPIRATION_DAYS: i64 = 7;
const MAX_THREAD_SCHEDULE_PROMPT_CHARS: usize = 4_000;
const MAX_THREAD_SCHEDULES: usize = 50;

pub struct ManageScheduleHandler;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ManageScheduleArgs {
    action: ScheduleAction,
    schedule_id: Option<String>,
    prompt: Option<String>,
    schedule: Option<ScheduleSpecArg>,
    timezone: Option<String>,
    next_run_at: Option<i64>,
    expires_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ScheduleAction {
    Create,
    List,
    Update,
    #[serde(alias = "stop")]
    Pause,
    #[serde(alias = "start")]
    Resume,
    #[serde(alias = "clear", alias = "remove", alias = "cancel")]
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ScheduleSpecArg {
    Once,
    Dynamic,
    Interval {
        amount: i64,
        unit: ScheduleIntervalUnitArg,
    },
    Cron {
        expression: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScheduleIntervalUnitArg {
    Minutes,
    Hours,
    Days,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageScheduleResponse {
    action: ScheduleAction,
    schedule_id: Option<String>,
    affected_schedule: Option<ScheduleSnapshot>,
    schedules: Vec<ScheduleSnapshot>,
    deleted: Option<bool>,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleSnapshot {
    thread_id: String,
    schedule_id: String,
    prompt: String,
    prompt_source: String,
    schedule: ScheduleSpecSnapshot,
    timezone: String,
    status: String,
    next_run_at: Option<i64>,
    last_run_at: Option<i64>,
    expires_at: Option<i64>,
    failure_count: i64,
    lease_expires_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ScheduleSpecSnapshot {
    Once,
    Dynamic,
    Interval { amount: i64, unit: String },
    Cron { expression: String },
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for ManageScheduleHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MANAGE_SCHEDULE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_manage_schedule_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session, payload, ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "manage_schedule handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ManageScheduleArgs = parse_arguments(&arguments)?;
        let state_db = session.state_db().ok_or_else(|| {
            FunctionCallError::Fatal("sqlite state db is unavailable for this session".to_string())
        })?;
        let auth_profile = session.selected_auth_profile().await;
        let response = manage_schedule(state_db, session.thread_id(), auth_profile, args).await?;
        schedule_response(response).map(boxed_tool_output)
    }
}

impl CoreToolRuntime for ManageScheduleHandler {}

async fn manage_schedule(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    auth_profile: Option<String>,
    args: ManageScheduleArgs,
) -> Result<ManageScheduleResponse, FunctionCallError> {
    match args.action {
        ScheduleAction::Create => create_schedule(state_db, thread_id, auth_profile, args).await,
        ScheduleAction::List => {
            let schedules = list_schedule_snapshots(&state_db, thread_id).await?;
            Ok(ManageScheduleResponse {
                action: ScheduleAction::List,
                schedule_id: None,
                affected_schedule: None,
                deleted: None,
                message: if schedules.is_empty() {
                    "No schedules are created for this thread.".to_string()
                } else {
                    format!("Found {} schedule(s) for this thread.", schedules.len())
                },
                schedules,
            })
        }
        ScheduleAction::Update => update_schedule(state_db, thread_id, args).await,
        ScheduleAction::Pause | ScheduleAction::Resume => {
            set_schedule_status(state_db, thread_id, args).await
        }
        ScheduleAction::Delete => delete_schedule(state_db, thread_id, args).await,
    }
}

async fn create_schedule(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    auth_profile: Option<String>,
    args: ManageScheduleArgs,
) -> Result<ManageScheduleResponse, FunctionCallError> {
    ensure_schedule_capacity(&state_db, thread_id).await?;
    let prompt = validate_schedule_prompt(
        args.prompt
            .as_deref()
            .ok_or_else(|| model_error("prompt is required when action is create"))?,
    )?;
    let schedule = args
        .schedule
        .ok_or_else(|| model_error("schedule is required when action is create"))
        .and_then(schedule_spec_arg_to_state)?;
    let timezone = normalize_timezone(args.timezone)?;
    let now = Utc::now();
    let explicit_next_run_at = args
        .next_run_at
        .map(|timestamp| timestamp_to_datetime(timestamp, "next_run_at"))
        .transpose()?;
    let next_run_at = match explicit_next_run_at {
        Some(next_run_at) => Some(next_run_at),
        None if matches!(schedule, codex_state::ThreadScheduleSpec::Once) => {
            return Err(model_error(
                "next_run_at is required for one-time schedules",
            ));
        }
        None => next_schedule_run_at(&schedule, timezone.as_str(), now)?,
    };
    let expires_at = args
        .expires_at
        .map(|timestamp| timestamp_to_datetime(timestamp, "expires_at"))
        .transpose()?
        .or_else(|| {
            if matches!(schedule, codex_state::ThreadScheduleSpec::Once) {
                None
            } else {
                default_schedule_expires_at(now)
            }
        });
    validate_schedule_expiry(next_run_at, expires_at)?;

    let schedule = state_db
        .thread_schedules()
        .create_thread_schedule_for_auth_profile(
            codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt,
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule,
                timezone,
                status: codex_state::ThreadScheduleStatus::Active,
                next_run_at,
                expires_at,
            },
            auth_profile,
        )
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?;
    let affected_schedule = ScheduleSnapshot::from(schedule);
    let schedules = list_schedule_snapshots(&state_db, thread_id).await?;
    let schedule_id = affected_schedule.schedule_id.clone();
    Ok(ManageScheduleResponse {
        action: ScheduleAction::Create,
        schedule_id: Some(schedule_id.clone()),
        affected_schedule: Some(affected_schedule),
        schedules,
        deleted: None,
        message: format!("Schedule {schedule_id} created."),
    })
}

async fn update_schedule(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageScheduleArgs,
) -> Result<ManageScheduleResponse, FunctionCallError> {
    let schedule_id = resolve_schedule_id(
        &state_db,
        thread_id,
        args.schedule_id.clone(),
        ScheduleAction::Update,
    )
    .await?;
    let existing =
        ensure_current_thread_schedule(&state_db, thread_id, schedule_id.as_str()).await?;
    let prompt = args
        .prompt
        .as_deref()
        .map(validate_schedule_prompt)
        .transpose()?;
    let prompt_source = prompt
        .as_ref()
        .map(|_| codex_state::ThreadSchedulePromptSource::Inline);
    let schedule = args.schedule.map(schedule_spec_arg_to_state).transpose()?;
    let timezone = args.timezone.map(normalize_timezone_value).transpose()?;
    let explicit_next_run_at = args
        .next_run_at
        .map(|timestamp| timestamp_to_datetime(timestamp, "next_run_at"))
        .transpose()?;
    let next_run_at = if let Some(next_run_at) = explicit_next_run_at {
        Some(Some(next_run_at))
    } else if schedule.is_some() || timezone.is_some() {
        let effective_schedule = schedule.as_ref().unwrap_or(&existing.schedule).clone();
        let effective_timezone = timezone.as_ref().unwrap_or(&existing.timezone);
        if matches!(effective_schedule, codex_state::ThreadScheduleSpec::Once) {
            if existing.next_run_at.is_none() {
                return Err(model_error(
                    "next_run_at is required for one-time schedules",
                ));
            }
            None
        } else {
            Some(next_schedule_run_at(
                &effective_schedule,
                effective_timezone.as_str(),
                Utc::now(),
            )?)
        }
    } else {
        None
    };
    let expires_at = args
        .expires_at
        .map(|timestamp| timestamp_to_datetime(timestamp, "expires_at"))
        .transpose()?
        .map(Some);
    let effective_next_run_at = next_run_at.unwrap_or(existing.next_run_at);
    let effective_expires_at = expires_at.unwrap_or(existing.expires_at);
    validate_schedule_expiry(effective_next_run_at, effective_expires_at)?;

    let schedule = state_db
        .thread_schedules()
        .update_thread_schedule(
            schedule_id.as_str(),
            codex_state::ThreadScheduleUpdate {
                prompt,
                prompt_source,
                schedule,
                timezone,
                status: None,
                next_run_at,
                expires_at,
            },
        )
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?
        .ok_or_else(|| missing_schedule_error(schedule_id.as_str()))?;
    if schedule.thread_id != thread_id {
        return Err(missing_schedule_error(schedule_id.as_str()));
    }
    let affected_schedule = ScheduleSnapshot::from(schedule);
    let schedules = list_schedule_snapshots(&state_db, thread_id).await?;
    Ok(ManageScheduleResponse {
        action: ScheduleAction::Update,
        schedule_id: Some(schedule_id.clone()),
        affected_schedule: Some(affected_schedule),
        schedules,
        deleted: None,
        message: format!("Schedule {schedule_id} updated."),
    })
}

async fn set_schedule_status(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageScheduleArgs,
) -> Result<ManageScheduleResponse, FunctionCallError> {
    let schedule_id =
        resolve_schedule_id(&state_db, thread_id, args.schedule_id.clone(), args.action).await?;
    let existing =
        ensure_current_thread_schedule(&state_db, thread_id, schedule_id.as_str()).await?;
    let status = match args.action {
        ScheduleAction::Pause => codex_state::ThreadScheduleStatus::Paused,
        ScheduleAction::Resume => codex_state::ThreadScheduleStatus::Active,
        ScheduleAction::Create
        | ScheduleAction::List
        | ScheduleAction::Update
        | ScheduleAction::Delete => unreachable!("action matched above"),
    };
    if status == codex_state::ThreadScheduleStatus::Active && existing.next_run_at.is_none() {
        return Err(model_error(
            "next_run_at is required for one-time schedules",
        ));
    }
    if status == codex_state::ThreadScheduleStatus::Active {
        if let Some(expires_at) = existing.expires_at
            && expires_at <= Utc::now()
        {
            return Err(model_error(
                "schedule expires_at must be in the future to resume",
            ));
        }
        validate_schedule_expiry(existing.next_run_at, existing.expires_at)?;
    }
    let schedule = match args.action {
        ScheduleAction::Resume => {
            state_db
                .thread_schedules()
                .resume_thread_schedule(schedule_id.as_str())
                .await
        }
        ScheduleAction::Pause => {
            state_db
                .thread_schedules()
                .set_thread_schedule_status(schedule_id.as_str(), status)
                .await
        }
        ScheduleAction::Create
        | ScheduleAction::List
        | ScheduleAction::Update
        | ScheduleAction::Delete => unreachable!("action matched above"),
    }
    .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?
    .ok_or_else(|| missing_schedule_error(schedule_id.as_str()))?;
    if schedule.thread_id != thread_id {
        return Err(missing_schedule_error(schedule_id.as_str()));
    }
    let affected_schedule = ScheduleSnapshot::from(schedule);
    let schedules = list_schedule_snapshots(&state_db, thread_id).await?;
    let message = match args.action {
        ScheduleAction::Pause => {
            format!("Schedule {schedule_id} paused; future runs are paused.")
        }
        ScheduleAction::Resume => {
            format!("Schedule {schedule_id} resumed; future runs are active.")
        }
        ScheduleAction::Create
        | ScheduleAction::List
        | ScheduleAction::Update
        | ScheduleAction::Delete => unreachable!("action matched above"),
    };
    Ok(ManageScheduleResponse {
        action: args.action,
        schedule_id: Some(schedule_id),
        affected_schedule: Some(affected_schedule),
        schedules,
        deleted: None,
        message,
    })
}

async fn delete_schedule(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageScheduleArgs,
) -> Result<ManageScheduleResponse, FunctionCallError> {
    let schedule_id = resolve_schedule_id(
        &state_db,
        thread_id,
        args.schedule_id.clone(),
        ScheduleAction::Delete,
    )
    .await?;
    let schedule =
        ensure_current_thread_schedule(&state_db, thread_id, schedule_id.as_str()).await?;
    let deleted = state_db
        .thread_schedules()
        .delete_thread_schedule(schedule_id.as_str())
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?;
    let schedules = list_schedule_snapshots(&state_db, thread_id).await?;
    Ok(ManageScheduleResponse {
        action: ScheduleAction::Delete,
        schedule_id: Some(schedule_id.clone()),
        affected_schedule: Some(ScheduleSnapshot::from(schedule)),
        schedules,
        deleted: Some(deleted),
        message: if deleted {
            format!("Schedule {schedule_id} deleted.")
        } else {
            format!("Schedule {schedule_id} was already absent.")
        },
    })
}

async fn list_schedule_snapshots(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<Vec<ScheduleSnapshot>, FunctionCallError> {
    let schedules = state_db
        .thread_schedules()
        .list_thread_schedules(thread_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?;
    Ok(schedules
        .into_iter()
        .filter(is_one_time_schedule)
        .map(ScheduleSnapshot::from)
        .collect())
}

async fn ensure_schedule_capacity(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<(), FunctionCallError> {
    let schedules = state_db
        .thread_schedules()
        .list_thread_schedules(thread_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?;
    if schedules.len() >= MAX_THREAD_SCHEDULES {
        return Err(model_error(format!(
            "thread already has the maximum of {MAX_THREAD_SCHEDULES} schedules"
        )));
    }
    Ok(())
}

async fn resolve_schedule_id(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    schedule_id: Option<String>,
    action: ScheduleAction,
) -> Result<String, FunctionCallError> {
    if let Some(schedule_id) = schedule_id.map(|value| value.trim().to_string())
        && !schedule_id.is_empty()
    {
        return Ok(schedule_id);
    }

    let schedules = state_db
        .thread_schedules()
        .list_thread_schedules(thread_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?
        .into_iter()
        .filter(is_one_time_schedule)
        .filter(|schedule| schedule.status != codex_state::ThreadScheduleStatus::Expired)
        .collect::<Vec<_>>();
    match schedules.as_slice() {
        [] => Err(model_error(format!(
            "cannot {} a schedule because no schedules are created for this thread",
            action.verb()
        ))),
        [schedule] => Ok(schedule.schedule_id.clone()),
        _ => Err(model_error(format!(
            "cannot {} a schedule without schedule_id because this thread has multiple schedules; call manage_schedule with action=list, then pass the chosen schedule_id",
            action.verb()
        ))),
    }
}

async fn ensure_current_thread_schedule(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    schedule_id: &str,
) -> Result<codex_state::ThreadSchedule, FunctionCallError> {
    let schedule = state_db
        .thread_schedules()
        .get_thread_schedule(schedule_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_schedule_error(err)))?
        .ok_or_else(|| missing_schedule_error(schedule_id))?;
    if schedule.thread_id != thread_id || !is_one_time_schedule(&schedule) {
        return Err(missing_schedule_error(schedule_id));
    }
    Ok(schedule)
}

fn is_one_time_schedule(schedule: &codex_state::ThreadSchedule) -> bool {
    matches!(schedule.schedule, codex_state::ThreadScheduleSpec::Once)
}

fn validate_schedule_prompt(prompt: &str) -> Result<String, FunctionCallError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(model_error("schedule prompt must not be empty"));
    }
    if prompt.chars().count() > MAX_THREAD_SCHEDULE_PROMPT_CHARS {
        return Err(model_error(format!(
            "schedule prompt must be at most {MAX_THREAD_SCHEDULE_PROMPT_CHARS} characters"
        )));
    }
    Ok(prompt.to_string())
}

fn schedule_spec_arg_to_state(
    schedule: ScheduleSpecArg,
) -> Result<codex_state::ThreadScheduleSpec, FunctionCallError> {
    match schedule {
        ScheduleSpecArg::Once => Ok(codex_state::ThreadScheduleSpec::Once),
        ScheduleSpecArg::Dynamic
        | ScheduleSpecArg::Interval { .. }
        | ScheduleSpecArg::Cron { .. } => Err(model_error(
            "recurring schedules belong in /loop; use schedule type `once` for /schedule",
        )),
    }
}

fn normalize_timezone(timezone: Option<String>) -> Result<String, FunctionCallError> {
    timezone.map_or_else(
        || {
            let timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
            normalize_timezone_value(timezone)
        },
        normalize_timezone_value,
    )
}

fn normalize_timezone_value(timezone: String) -> Result<String, FunctionCallError> {
    let timezone = timezone.trim();
    if timezone.is_empty() {
        return Err(model_error("schedule timezone must not be empty"));
    }
    parse_schedule_timezone(timezone).map(|timezone| timezone.name().to_string())
}

fn parse_schedule_timezone(timezone: &str) -> Result<Tz, FunctionCallError> {
    timezone
        .parse::<Tz>()
        .map_err(|err| model_error(format!("invalid schedule timezone `{timezone}`: {err}")))
}

fn timestamp_to_datetime(value: i64, field_name: &str) -> Result<DateTime<Utc>, FunctionCallError> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| model_error(format!("{field_name} must be a valid Unix timestamp")))
}

fn default_schedule_expires_at(now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    now.checked_add_signed(ChronoDuration::days(DEFAULT_SCHEDULE_EXPIRATION_DAYS))
}

fn next_schedule_run_at(
    schedule: &codex_state::ThreadScheduleSpec,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, FunctionCallError> {
    let next = match schedule {
        codex_state::ThreadScheduleSpec::Once => None,
        codex_state::ThreadScheduleSpec::Dynamic => {
            after.checked_add_signed(ChronoDuration::minutes(DEFAULT_DYNAMIC_INTERVAL_MINUTES))
        }
        codex_state::ThreadScheduleSpec::Interval(interval) => {
            let duration = match interval.unit {
                codex_state::ThreadScheduleIntervalUnit::Minutes => {
                    ChronoDuration::minutes(interval.amount)
                }
                codex_state::ThreadScheduleIntervalUnit::Hours => {
                    ChronoDuration::hours(interval.amount)
                }
                codex_state::ThreadScheduleIntervalUnit::Days => {
                    ChronoDuration::days(interval.amount)
                }
            };
            after.checked_add_signed(duration)
        }
        codex_state::ThreadScheduleSpec::Cron { expression } => {
            let timezone = parse_schedule_timezone(timezone)?;
            let cron = Cron::from_str(expression).map_err(|err| {
                model_error(format!("invalid cron expression `{expression}`: {err}"))
            })?;
            let local_after = after.with_timezone(&timezone);
            let next = cron
                .find_next_occurrence(&local_after, /*inclusive*/ false)
                .map_err(|err| model_error(err.to_string()))?;
            Some(next.with_timezone(&Utc))
        }
    };
    Ok(next)
}

fn validate_schedule_expiry(
    next_run_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), FunctionCallError> {
    if let (Some(next_run_at), Some(expires_at)) = (next_run_at, expires_at)
        && expires_at <= next_run_at
    {
        return Err(model_error(
            "schedule expires_at must be later than next_run_at",
        ));
    }
    Ok(())
}

impl ScheduleAction {
    fn verb(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::List => "list",
            Self::Update => "update",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Delete => "delete",
        }
    }
}

impl From<codex_state::ThreadSchedule> for ScheduleSnapshot {
    fn from(schedule: codex_state::ThreadSchedule) -> Self {
        Self {
            thread_id: schedule.thread_id.to_string(),
            schedule_id: schedule.schedule_id,
            prompt: schedule.prompt,
            prompt_source: schedule.prompt_source.as_str().to_string(),
            schedule: ScheduleSpecSnapshot::from(schedule.schedule),
            timezone: schedule.timezone,
            status: schedule.status.as_str().to_string(),
            next_run_at: timestamp_seconds(schedule.next_run_at),
            last_run_at: timestamp_seconds(schedule.last_run_at),
            expires_at: timestamp_seconds(schedule.expires_at),
            failure_count: schedule.failure_count,
            lease_expires_at: timestamp_seconds(schedule.lease_expires_at),
            created_at: schedule.created_at.timestamp(),
            updated_at: schedule.updated_at.timestamp(),
        }
    }
}

impl From<codex_state::ThreadScheduleSpec> for ScheduleSpecSnapshot {
    fn from(schedule: codex_state::ThreadScheduleSpec) -> Self {
        match schedule {
            codex_state::ThreadScheduleSpec::Once => Self::Once,
            codex_state::ThreadScheduleSpec::Dynamic => Self::Dynamic,
            codex_state::ThreadScheduleSpec::Interval(interval) => Self::Interval {
                amount: interval.amount,
                unit: interval.unit.as_str().to_string(),
            },
            codex_state::ThreadScheduleSpec::Cron { expression } => Self::Cron { expression },
        }
    }
}

fn timestamp_seconds(value: Option<DateTime<Utc>>) -> Option<i64> {
    value.map(|datetime| datetime.timestamp())
}

fn missing_schedule_error(schedule_id: &str) -> FunctionCallError {
    model_error(format!(
        "no schedule with schedule_id `{schedule_id}` exists in this thread"
    ))
}

fn model_error(message: impl Into<String>) -> FunctionCallError {
    FunctionCallError::RespondToModel(message.into())
}

fn format_schedule_error(err: anyhow::Error) -> String {
    let mut message = err.to_string();
    for cause in err.chain().skip(1) {
        let _ = write!(message, ": {cause}");
    }
    message
}

fn schedule_response(
    response: ManageScheduleResponse,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let response = serde_json::to_string_pretty(&response)
        .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    Ok(FunctionToolOutput::from_text(response, Some(true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use codex_protocol::protocol::SessionSource;
    use codex_state::ThreadMetadataBuilder;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    async fn test_runtime() -> (TempDir, Arc<codex_state::StateRuntime>) {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let runtime =
            codex_state::StateRuntime::init(temp_dir.path().to_path_buf(), "test-provider".into())
                .await
                .expect("state db should initialize");
        (temp_dir, runtime)
    }

    fn test_thread_id(id: u32) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{id:012}"))
            .expect("valid thread id")
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }

    async fn upsert_test_thread(runtime: &codex_state::StateRuntime, thread_id: ThreadId) {
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            runtime.codex_home().join(format!("{thread_id}.jsonl")),
            at(/*seconds*/ 1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = runtime.codex_home().join("workspace");
        runtime
            .upsert_thread(&builder.build("test-provider"))
            .await
            .expect("test thread should be upserted");
    }

    async fn create_once_schedule(
        runtime: &codex_state::StateRuntime,
        thread_id: ThreadId,
        prompt: &str,
    ) -> codex_state::ThreadSchedule {
        runtime
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: prompt.to_string(),
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule: codex_state::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: codex_state::ThreadScheduleStatus::Active,
                next_run_at: Some(at(/*seconds*/ 1_700_000_300)),
                expires_at: None,
            })
            .await
            .expect("schedule should be created")
    }

    #[tokio::test]
    async fn create_adds_one_time_schedule() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;

        let response = manage_schedule(
            runtime.clone(),
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Create,
                schedule_id: None,
                prompt: Some("check CI".to_string()),
                schedule: Some(ScheduleSpecArg::Once),
                timezone: Some("UTC".to_string()),
                next_run_at: Some(1_700_000_300),
                expires_at: None,
            },
        )
        .await
        .expect("schedule should be created");

        assert_eq!(response.action, ScheduleAction::Create);
        assert_eq!(response.schedules.len(), 1);
        let schedule = runtime
            .thread_schedules()
            .get_thread_schedule(response.schedule_id.as_deref().expect("schedule id"))
            .await
            .expect("lookup should succeed")
            .expect("schedule should exist");
        assert_eq!(schedule.prompt, "check CI");
        assert_eq!(schedule.timezone, "UTC");
        assert_eq!(schedule.schedule, codex_state::ThreadScheduleSpec::Once);
        assert_eq!(schedule.next_run_at, Some(at(/*seconds*/ 1_700_000_300)));
    }

    #[tokio::test]
    async fn create_rejects_one_time_schedule_without_next_run_at() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 13);
        upsert_test_thread(&runtime, thread_id).await;

        let error = manage_schedule(
            runtime,
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Create,
                schedule_id: None,
                prompt: Some("ask one question".to_string()),
                schedule: Some(ScheduleSpecArg::Once),
                timezone: Some("UTC".to_string()),
                next_run_at: None,
                expires_at: None,
            },
        )
        .await
        .expect_err("one-time schedule without next_run_at should fail");

        assert_eq!(
            error,
            model_error("next_run_at is required for one-time schedules")
        );
    }

    #[tokio::test]
    async fn create_rejects_recurring_schedule_specs() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 14);
        upsert_test_thread(&runtime, thread_id).await;

        let error = manage_schedule(
            runtime,
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Create,
                schedule_id: None,
                prompt: Some("check CI".to_string()),
                schedule: Some(ScheduleSpecArg::Interval {
                    amount: 5,
                    unit: ScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                next_run_at: Some(1_700_000_300),
                expires_at: None,
            },
        )
        .await
        .expect_err("recurring schedule spec should fail");

        assert_eq!(
            error,
            model_error(
                "recurring schedules belong in /loop; use schedule type `once` for /schedule"
            )
        );
    }

    #[tokio::test]
    async fn update_changes_prompt_and_schedule() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_once_schedule(&runtime, thread_id, "check CI").await;

        let response = manage_schedule(
            runtime.clone(),
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Update,
                schedule_id: Some(schedule.schedule_id.clone()),
                prompt: Some("write handoff".to_string()),
                schedule: Some(ScheduleSpecArg::Once),
                timezone: Some("UTC".to_string()),
                next_run_at: Some(1_700_001_000),
                expires_at: None,
            },
        )
        .await
        .expect("schedule should update");

        assert_eq!(response.action, ScheduleAction::Update);
        let updated = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("lookup should succeed")
            .expect("schedule should exist");
        assert_eq!(updated.prompt, "write handoff");
        assert_eq!(updated.schedule, codex_state::ThreadScheduleSpec::Once);
        assert_eq!(updated.next_run_at, Some(at(/*seconds*/ 1_700_001_000)));
    }

    #[tokio::test]
    async fn resume_rejects_one_time_schedule_without_next_run_at() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 15);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_once_schedule(&runtime, thread_id, "check CI").await;
        runtime
            .thread_schedules()
            .update_thread_schedule(
                schedule.schedule_id.as_str(),
                codex_state::ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: None,
                    timezone: None,
                    status: Some(codex_state::ThreadScheduleStatus::Expired),
                    next_run_at: Some(None),
                    expires_at: None,
                },
            )
            .await
            .expect("schedule should update")
            .expect("schedule should exist");

        let error = manage_schedule(
            runtime,
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Resume,
                schedule_id: Some(schedule.schedule_id),
                prompt: None,
                schedule: None,
                timezone: None,
                next_run_at: None,
                expires_at: None,
            },
        )
        .await
        .expect_err("schedule without next_run_at should not resume");

        assert_eq!(
            error,
            model_error("next_run_at is required for one-time schedules")
        );
    }

    #[tokio::test]
    async fn resume_resets_failure_count() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 16);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_once_schedule(&runtime, thread_id, "check CI").await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(
                at(/*seconds*/ 1_700_000_301),
                "lease-1",
                std::time::Duration::from_secs(60),
            )
            .await
            .expect("schedule claim should succeed")
            .expect("schedule should be due");
        runtime
            .thread_schedules()
            .fail_thread_schedule_run(
                schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                at(/*seconds*/ 1_700_000_302),
                Some(at(/*seconds*/ 1_700_000_600)),
                "transient failure".to_string(),
            )
            .await
            .expect("schedule failure should be recorded");
        runtime
            .thread_schedules()
            .set_thread_schedule_status(
                schedule.schedule_id.as_str(),
                codex_state::ThreadScheduleStatus::Paused,
            )
            .await
            .expect("schedule should pause")
            .expect("schedule should exist");

        let before_resume = runtime
            .thread_schedules()
            .get_thread_schedule(schedule.schedule_id.as_str())
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(1, before_resume.failure_count);

        let response = manage_schedule(
            runtime.clone(),
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Resume,
                schedule_id: Some(schedule.schedule_id),
                prompt: None,
                schedule: None,
                timezone: None,
                next_run_at: None,
                expires_at: None,
            },
        )
        .await
        .expect("schedule should resume");

        let affected = response
            .affected_schedule
            .expect("resumed schedule should be returned");
        assert_eq!("active", affected.status);
        assert_eq!(0, affected.failure_count);
    }

    #[tokio::test]
    async fn resume_rejects_schedule_with_past_expiry() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 17);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_once_schedule(&runtime, thread_id, "check CI").await;
        runtime
            .thread_schedules()
            .update_thread_schedule(
                schedule.schedule_id.as_str(),
                codex_state::ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: None,
                    timezone: None,
                    status: Some(codex_state::ThreadScheduleStatus::Paused),
                    next_run_at: None,
                    expires_at: Some(Some(at(/*seconds*/ 1_700_000_600))),
                },
            )
            .await
            .expect("schedule should update")
            .expect("schedule should exist");

        let error = manage_schedule(
            runtime,
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Resume,
                schedule_id: Some(schedule.schedule_id),
                prompt: None,
                schedule: None,
                timezone: None,
                next_run_at: None,
                expires_at: None,
            },
        )
        .await
        .expect_err("expired schedule should not resume");

        assert_eq!(
            error,
            model_error("schedule expires_at must be in the future to resume")
        );
    }

    #[tokio::test]
    async fn delete_removes_specific_schedule() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 3);
        upsert_test_thread(&runtime, thread_id).await;
        let first = create_once_schedule(&runtime, thread_id, "check CI").await;
        let second = create_once_schedule(&runtime, thread_id, "write handoff").await;

        let response = manage_schedule(
            runtime.clone(),
            thread_id,
            None,
            ManageScheduleArgs {
                action: ScheduleAction::Delete,
                schedule_id: Some(first.schedule_id.clone()),
                prompt: None,
                schedule: None,
                timezone: None,
                next_run_at: None,
                expires_at: None,
            },
        )
        .await
        .expect("schedule should delete");

        assert_eq!(response.action, ScheduleAction::Delete);
        assert_eq!(response.deleted, Some(true));
        assert!(
            runtime
                .thread_schedules()
                .get_thread_schedule(&first.schedule_id)
                .await
                .expect("lookup should succeed")
                .is_none()
        );
        assert!(
            runtime
                .thread_schedules()
                .get_thread_schedule(&second.schedule_id)
                .await
                .expect("lookup should succeed")
                .is_some()
        );
    }
}
