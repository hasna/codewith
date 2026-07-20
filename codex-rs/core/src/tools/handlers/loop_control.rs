//! Built-in model tool handler for managing `/loop` schedules.

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::loop_control_spec::MANAGE_LOOP_TOOL_NAME;
use crate::tools::handlers::loop_control_spec::create_manage_loop_tool;
use crate::tools::handlers::parse_arguments;
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
use serde_json::Value as JsonValue;
use serde_json::json;
use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::Arc;

const DEFAULT_DYNAMIC_INTERVAL_MINUTES: i64 = 1;
const DEFAULT_LOOP_EXPIRATION_DAYS: i64 = 7;
const MAX_THREAD_SCHEDULE_PROMPT_CHARS: usize = 4_000;
const MAX_THREAD_SCHEDULES: usize = 50;
const COMPACT_PROMPT_PREVIEW_CHARS: usize = 160;

pub struct ManageLoopHandler;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ManageLoopArgs {
    action: LoopAction,
    schedule_id: Option<String>,
    parent_schedule_id: Option<String>,
    prompt: Option<String>,
    schedule: Option<LoopScheduleSpecArg>,
    timezone: Option<String>,
    next_run_at: Option<i64>,
    expires_at: Option<i64>,
    verbose: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum LoopAction {
    Create,
    List,
    #[serde(alias = "pause")]
    Stop,
    Resume,
    Start,
    #[serde(alias = "delete", alias = "remove", alias = "cancel")]
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LoopScheduleSpecArg {
    Once,
    Dynamic,
    Interval {
        amount: i64,
        unit: LoopScheduleIntervalUnitArg,
    },
    Cron {
        expression: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LoopScheduleIntervalUnitArg {
    Minutes,
    Hours,
    Days,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageLoopResponse {
    action: LoopAction,
    schedule_id: Option<String>,
    affected_schedule: Option<LoopScheduleSnapshot>,
    schedules: Vec<LoopScheduleSnapshot>,
    deleted: Option<bool>,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoopScheduleSnapshot {
    thread_id: String,
    schedule_id: String,
    parent_schedule_id: Option<String>,
    nesting_depth: i64,
    prompt: String,
    prompt_source: String,
    schedule: LoopScheduleSpecSnapshot,
    timezone: String,
    status: String,
    next_run_at: Option<i64>,
    last_run_at: Option<i64>,
    expires_at: Option<i64>,
    failure_count: i64,
    lease_expires_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    stats: LoopScheduleStatsSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoopScheduleStatsSnapshot {
    total_runs: i64,
    leased_runs: i64,
    running_runs: i64,
    deferred_runs: i64,
    completed_runs: i64,
    failed_runs: i64,
    last_started_at: Option<i64>,
    last_completed_at: Option<i64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum LoopScheduleSpecSnapshot {
    Once,
    Dynamic,
    Interval { amount: i64, unit: String },
    Cron { expression: String },
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for ManageLoopHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MANAGE_LOOP_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_manage_loop_tool()
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
                    "manage_loop handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ManageLoopArgs = parse_arguments(&arguments)?;
        let verbose = args.verbose.unwrap_or(false);
        let state_db = session.state_db().ok_or_else(|| {
            FunctionCallError::Fatal("sqlite state db is unavailable for this session".to_string())
        })?;
        let auth_profile = session.selected_auth_profile().await;
        let response = manage_loop(state_db, session.thread_id(), auth_profile, args).await?;
        loop_response(response, verbose).map(boxed_tool_output)
    }
}

impl CoreToolRuntime for ManageLoopHandler {}

async fn manage_loop(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    auth_profile: Option<String>,
    args: ManageLoopArgs,
) -> Result<ManageLoopResponse, FunctionCallError> {
    let action = match args.action {
        LoopAction::Start => {
            let schedule_id_present = args
                .schedule_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let has_create_fields = args
                .prompt
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || args.schedule.is_some();
            if !schedule_id_present && has_create_fields {
                LoopAction::Create
            } else {
                LoopAction::Resume
            }
        }
        action => action,
    };

    match action {
        LoopAction::Create => create_loop(state_db, thread_id, auth_profile, args).await,
        LoopAction::List => {
            let schedules = list_loop_snapshots(&state_db, thread_id).await?;
            Ok(ManageLoopResponse {
                action: LoopAction::List,
                schedule_id: None,
                affected_schedule: None,
                deleted: None,
                message: if schedules.is_empty() {
                    "No loops are scheduled for this thread.".to_string()
                } else {
                    format!(
                        "Found {} loop schedule(s) for this thread.",
                        schedules.len()
                    )
                },
                schedules,
            })
        }
        LoopAction::Stop | LoopAction::Resume => {
            let schedule_id =
                resolve_loop_schedule_id(&state_db, thread_id, args.schedule_id, action).await?;
            let existing =
                ensure_current_thread_schedule(&state_db, thread_id, schedule_id.as_str()).await?;
            let status = match action {
                LoopAction::Stop => codex_state::ThreadScheduleStatus::Paused,
                LoopAction::Resume => codex_state::ThreadScheduleStatus::Active,
                LoopAction::Create | LoopAction::List | LoopAction::Start | LoopAction::Clear => {
                    unreachable!("action matched above")
                }
            };
            let schedule = if action == LoopAction::Resume {
                let now = Utc::now();
                if let Some(expires_at) = existing.expires_at
                    && expires_at <= now
                {
                    return Err(FunctionCallError::RespondToModel(
                        "loop expires_at must be in the future to resume".to_string(),
                    ));
                }
                if existing.next_run_at.is_none() {
                    let next_run_at =
                        next_loop_run_at(&existing.schedule, &existing.timezone, now)?.ok_or_else(
                            || {
                                FunctionCallError::RespondToModel(
                                    "cannot resume loop because no next run time could be computed"
                                        .to_string(),
                                )
                            },
                        )?;
                    if let Some(expires_at) = existing.expires_at
                        && expires_at <= next_run_at
                    {
                        return Err(FunctionCallError::RespondToModel(
                            "loop expires_at must be later than next_run_at".to_string(),
                        ));
                    }
                    state_db
                        .thread_schedules()
                        .resume_thread_schedule_at(schedule_id.as_str(), next_run_at)
                        .await
                } else {
                    validate_loop_expiry(existing.next_run_at, existing.expires_at)?;
                    state_db
                        .thread_schedules()
                        .resume_thread_schedule(schedule_id.as_str())
                        .await
                }
            } else {
                state_db
                    .thread_schedules()
                    .set_thread_schedule_status(schedule_id.as_str(), status)
                    .await
            }
            .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?
            .ok_or_else(|| missing_loop_error(schedule_id.as_str()))?;
            if schedule.thread_id != thread_id {
                return Err(missing_loop_error(schedule_id.as_str()));
            }
            let affected_schedule = loop_schedule_snapshot(&state_db, schedule).await?;
            let schedules = list_loop_snapshots(&state_db, thread_id).await?;
            let message = match action {
                LoopAction::Stop => {
                    format!("Loop {schedule_id} stopped; future runs are paused.")
                }
                LoopAction::Resume => {
                    format!("Loop {schedule_id} resumed; future runs are active.")
                }
                LoopAction::Create | LoopAction::List | LoopAction::Start | LoopAction::Clear => {
                    unreachable!("action matched above")
                }
            };
            Ok(ManageLoopResponse {
                action,
                schedule_id: Some(schedule_id),
                affected_schedule: Some(affected_schedule),
                schedules,
                deleted: None,
                message,
            })
        }
        LoopAction::Clear => {
            let schedule_id =
                resolve_loop_schedule_id(&state_db, thread_id, args.schedule_id, args.action)
                    .await?;
            let schedule =
                ensure_current_thread_schedule(&state_db, thread_id, schedule_id.as_str()).await?;
            let affected_schedule = loop_schedule_snapshot(&state_db, schedule).await?;
            let deleted_schedule_ids = state_db
                .thread_schedules()
                .delete_thread_schedule_tree(schedule_id.as_str())
                .await
                .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?;
            let deleted = !deleted_schedule_ids.is_empty();
            let deleted_count = deleted_schedule_ids.len();
            let schedules = list_loop_snapshots(&state_db, thread_id).await?;
            Ok(ManageLoopResponse {
                action: LoopAction::Clear,
                schedule_id: Some(schedule_id.clone()),
                affected_schedule: Some(affected_schedule),
                schedules,
                deleted: Some(deleted),
                message: if deleted {
                    if deleted_count > 1 {
                        let child_count = deleted_count - 1;
                        let child_label = if child_count == 1 {
                            "nested child loop"
                        } else {
                            "nested child loops"
                        };
                        format!("Loop {schedule_id} and {child_count} {child_label} cleared.")
                    } else {
                        format!("Loop {schedule_id} cleared.")
                    }
                } else {
                    format!("Loop {schedule_id} was already absent.")
                },
            })
        }
        LoopAction::Start => unreachable!("start action is normalized before dispatch"),
    }
}

async fn create_loop(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    auth_profile: Option<String>,
    args: ManageLoopArgs,
) -> Result<ManageLoopResponse, FunctionCallError> {
    ensure_schedule_capacity(&state_db, thread_id).await?;
    let prompt = validate_loop_prompt(
        args.prompt
            .as_deref()
            .ok_or_else(|| model_error("prompt is required when action is create"))?,
    )?;
    let schedule = args
        .schedule
        .ok_or_else(|| model_error("schedule is required when action is create"))
        .and_then(loop_schedule_spec_arg_to_state)?;
    let parent_schedule_id = args
        .parent_schedule_id
        .as_deref()
        .map(str::trim)
        .map(|value| {
            if value.is_empty() {
                Err(model_error("parent_schedule_id cannot be empty"))
            } else {
                Ok(value.to_string())
            }
        })
        .transpose()?;
    let timezone = normalize_timezone(args.timezone)?;
    let now = Utc::now();
    let explicit_next_run_at = args
        .next_run_at
        .map(|timestamp| timestamp_to_datetime(timestamp, "next_run_at"))
        .transpose()?;
    let next_run_at = match explicit_next_run_at {
        Some(next_run_at) => Some(next_run_at),
        None => Some(
            next_loop_run_at(&schedule, timezone.as_str(), now)?.ok_or_else(|| {
                model_error("cannot create loop because no next run time could be computed")
            })?,
        ),
    };
    let expires_at = args
        .expires_at
        .map(|timestamp| timestamp_to_datetime(timestamp, "expires_at"))
        .transpose()?
        .or_else(|| default_loop_expires_at(now));
    validate_loop_expiry(next_run_at, expires_at)?;

    let create_params = codex_state::ThreadScheduleCreateParams {
        thread_id,
        prompt,
        prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
        schedule,
        timezone,
        status: codex_state::ThreadScheduleStatus::Active,
        next_run_at,
        expires_at,
    };
    let schedule = match parent_schedule_id {
        Some(parent_schedule_id) => {
            state_db
                .thread_schedules()
                .create_nested_thread_schedule_for_auth_profile(
                    create_params,
                    parent_schedule_id,
                    auth_profile,
                )
                .await
        }
        None => {
            state_db
                .thread_schedules()
                .create_thread_schedule_for_auth_profile(create_params, auth_profile)
                .await
        }
    }
    .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?;
    let affected_schedule = loop_schedule_snapshot(&state_db, schedule).await?;
    let schedules = list_loop_snapshots(&state_db, thread_id).await?;
    let schedule_id = affected_schedule.schedule_id.clone();
    Ok(ManageLoopResponse {
        action: LoopAction::Create,
        schedule_id: Some(schedule_id.clone()),
        affected_schedule: Some(affected_schedule),
        schedules,
        deleted: None,
        message: format!("Loop {schedule_id} created; future runs are active."),
    })
}

async fn list_loop_snapshots(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<Vec<LoopScheduleSnapshot>, FunctionCallError> {
    let schedules = state_db
        .thread_schedules()
        .list_thread_schedules(thread_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?;
    let mut snapshots = Vec::new();
    for schedule in schedules.into_iter().filter(is_loop_schedule) {
        snapshots.push(loop_schedule_snapshot(state_db, schedule).await?);
    }
    Ok(snapshots)
}

async fn ensure_schedule_capacity(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<(), FunctionCallError> {
    let schedules = state_db
        .thread_schedules()
        .list_thread_schedules(thread_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?;
    if schedules.len() >= MAX_THREAD_SCHEDULES {
        return Err(model_error(format!(
            "thread already has the maximum of {MAX_THREAD_SCHEDULES} schedules"
        )));
    }
    Ok(())
}

async fn resolve_loop_schedule_id(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    schedule_id: Option<String>,
    action: LoopAction,
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
        .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?
        .into_iter()
        .filter(is_loop_schedule)
        .filter(|schedule| schedule.status != codex_state::ThreadScheduleStatus::Expired)
        .collect::<Vec<_>>();
    match schedules.as_slice() {
        [] => Err(FunctionCallError::RespondToModel(format!(
            "cannot {} a loop because no loops are scheduled for this thread",
            action.verb()
        ))),
        [schedule] => Ok(schedule.schedule_id.clone()),
        _ => Err(FunctionCallError::RespondToModel(format!(
            "cannot {} a loop without schedule_id because this thread has multiple loops; call manage_loop with action=list, then pass the chosen schedule_id",
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
        .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?
        .ok_or_else(|| missing_loop_error(schedule_id))?;
    if schedule.thread_id != thread_id || !is_loop_schedule(&schedule) {
        return Err(missing_loop_error(schedule_id));
    }
    Ok(schedule)
}

fn is_loop_schedule(schedule: &codex_state::ThreadSchedule) -> bool {
    !matches!(schedule.schedule, codex_state::ThreadScheduleSpec::Once)
}

fn validate_loop_prompt(prompt: &str) -> Result<String, FunctionCallError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(model_error("loop prompt must not be empty"));
    }
    if prompt.chars().count() > MAX_THREAD_SCHEDULE_PROMPT_CHARS {
        return Err(model_error(format!(
            "loop prompt must be at most {MAX_THREAD_SCHEDULE_PROMPT_CHARS} characters"
        )));
    }
    Ok(prompt.to_string())
}

fn loop_schedule_spec_arg_to_state(
    schedule: LoopScheduleSpecArg,
) -> Result<codex_state::ThreadScheduleSpec, FunctionCallError> {
    match schedule {
        LoopScheduleSpecArg::Once => Err(model_error(
            "one-time schedules belong in /schedule; use interval, cron, or dynamic for /loop",
        )),
        LoopScheduleSpecArg::Dynamic => Ok(codex_state::ThreadScheduleSpec::Dynamic),
        LoopScheduleSpecArg::Interval { amount, unit } => {
            if amount <= 0 {
                return Err(model_error("loop interval amount must be positive"));
            }
            Ok(codex_state::ThreadScheduleSpec::Interval(
                codex_state::ThreadScheduleInterval {
                    amount,
                    unit: match unit {
                        LoopScheduleIntervalUnitArg::Minutes => {
                            codex_state::ThreadScheduleIntervalUnit::Minutes
                        }
                        LoopScheduleIntervalUnitArg::Hours => {
                            codex_state::ThreadScheduleIntervalUnit::Hours
                        }
                        LoopScheduleIntervalUnitArg::Days => {
                            codex_state::ThreadScheduleIntervalUnit::Days
                        }
                    },
                },
            ))
        }
        LoopScheduleSpecArg::Cron { expression } => {
            if expression.trim().is_empty() {
                return Err(model_error("loop cron expression must not be empty"));
            }
            Ok(codex_state::ThreadScheduleSpec::Cron {
                expression: expression.trim().to_string(),
            })
        }
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
        return Err(model_error("loop timezone must not be empty"));
    }
    parse_loop_timezone(timezone).map(|timezone| timezone.name().to_string())
}

fn parse_loop_timezone(timezone: &str) -> Result<Tz, FunctionCallError> {
    timezone
        .parse::<Tz>()
        .map_err(|err| model_error(format!("invalid loop timezone `{timezone}`: {err}")))
}

fn timestamp_to_datetime(value: i64, field_name: &str) -> Result<DateTime<Utc>, FunctionCallError> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| model_error(format!("{field_name} must be a valid Unix timestamp")))
}

fn default_loop_expires_at(now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    now.checked_add_signed(ChronoDuration::days(DEFAULT_LOOP_EXPIRATION_DAYS))
}

fn validate_loop_expiry(
    next_run_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), FunctionCallError> {
    if let (Some(next_run_at), Some(expires_at)) = (next_run_at, expires_at)
        && expires_at <= next_run_at
    {
        return Err(model_error(
            "loop expires_at must be later than next_run_at",
        ));
    }
    Ok(())
}

fn next_loop_run_at(
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
            let timezone = parse_loop_timezone(timezone)?;
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

impl LoopAction {
    fn verb(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::List => "list",
            Self::Stop => "stop",
            Self::Resume => "resume",
            Self::Start => "start",
            Self::Clear => "clear",
        }
    }
}

async fn loop_schedule_snapshot(
    state_db: &codex_state::StateRuntime,
    schedule: codex_state::ThreadSchedule,
) -> Result<LoopScheduleSnapshot, FunctionCallError> {
    let stats = state_db
        .thread_schedules()
        .get_thread_schedule_stats(&schedule.schedule_id)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?;
    Ok(LoopScheduleSnapshot {
        thread_id: schedule.thread_id.to_string(),
        schedule_id: schedule.schedule_id,
        parent_schedule_id: schedule.parent_schedule_id,
        nesting_depth: schedule.nesting_depth,
        prompt: schedule.prompt,
        prompt_source: schedule.prompt_source.as_str().to_string(),
        schedule: LoopScheduleSpecSnapshot::from(schedule.schedule),
        timezone: schedule.timezone,
        status: schedule.status.as_str().to_string(),
        next_run_at: timestamp_seconds(schedule.next_run_at),
        last_run_at: timestamp_seconds(schedule.last_run_at),
        expires_at: timestamp_seconds(schedule.expires_at),
        failure_count: schedule.failure_count,
        lease_expires_at: timestamp_seconds(schedule.lease_expires_at),
        created_at: schedule.created_at.timestamp(),
        updated_at: schedule.updated_at.timestamp(),
        stats: LoopScheduleStatsSnapshot::from(stats),
    })
}

impl From<codex_state::ThreadScheduleStats> for LoopScheduleStatsSnapshot {
    fn from(stats: codex_state::ThreadScheduleStats) -> Self {
        Self {
            total_runs: stats.total_runs,
            leased_runs: stats.leased_runs,
            running_runs: stats.running_runs,
            deferred_runs: stats.deferred_runs,
            completed_runs: stats.completed_runs,
            failed_runs: stats.failed_runs,
            last_started_at: timestamp_seconds(stats.last_started_at),
            last_completed_at: timestamp_seconds(stats.last_completed_at),
            last_error: stats.last_error,
        }
    }
}

impl From<codex_state::ThreadScheduleSpec> for LoopScheduleSpecSnapshot {
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

fn missing_loop_error(schedule_id: &str) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "no loop with schedule_id `{schedule_id}` exists in this thread"
    ))
}

fn format_loop_error(err: anyhow::Error) -> String {
    let mut message = err.to_string();
    for cause in err.chain().skip(1) {
        let _ = write!(message, ": {cause}");
    }
    message
}

fn loop_response(
    response: ManageLoopResponse,
    verbose: bool,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let response = if verbose {
        serde_json::to_string_pretty(&response)
    } else {
        serde_json::to_string_pretty(&compact_loop_response(&response))
    }
    .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    Ok(FunctionToolOutput::from_text(response, Some(true)))
}

fn compact_loop_response(response: &ManageLoopResponse) -> JsonValue {
    json!({
        "action": response.action,
        "scheduleId": response.schedule_id,
        "message": response.message,
        "deleted": response.deleted,
        "count": response.schedules.len(),
        "affectedSchedule": response
            .affected_schedule
            .as_ref()
            .map(compact_loop_schedule),
        "schedules": response
            .schedules
            .iter()
            .map(compact_loop_schedule)
            .collect::<Vec<_>>(),
        "hint": "Default loop output is compact. Pass verbose=true for full prompts, lease timestamps, and complete run stats.",
    })
}

fn compact_loop_schedule(schedule: &LoopScheduleSnapshot) -> JsonValue {
    json!({
        "scheduleId": schedule.schedule_id,
        "status": schedule.status,
        "schedule": schedule.schedule,
        "timezone": schedule.timezone,
        "nextRunAt": schedule.next_run_at,
        "lastRunAt": schedule.last_run_at,
        "expiresAt": schedule.expires_at,
        "failureCount": schedule.failure_count,
        "promptPreview": compact_text_preview(&schedule.prompt, COMPACT_PROMPT_PREVIEW_CHARS),
        "promptChars": schedule.prompt.chars().count(),
        "runs": {
            "total": schedule.stats.total_runs,
            "running": schedule.stats.running_runs,
            "completed": schedule.stats.completed_runs,
            "failed": schedule.stats.failed_runs,
            "lastErrorPreview": schedule.stats.last_error.as_deref().map(|error| {
                compact_text_preview(error, COMPACT_PROMPT_PREVIEW_CHARS)
            }),
        },
    })
}

fn compact_text_preview(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_end(&normalized, max_chars)
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn model_error(message: impl Into<String>) -> FunctionCallError {
    FunctionCallError::RespondToModel(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use codex_protocol::protocol::SessionSource;
    use codex_state::ThreadMetadataBuilder;
    use pretty_assertions::assert_eq;
    use std::time::Duration;
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

    fn loop_args(action: LoopAction) -> ManageLoopArgs {
        ManageLoopArgs {
            action,
            schedule_id: None,
            parent_schedule_id: None,
            prompt: None,
            schedule: None,
            timezone: None,
            next_run_at: None,
            expires_at: None,
            verbose: None,
        }
    }

    fn loop_snapshot(prompt: &str) -> LoopScheduleSnapshot {
        LoopScheduleSnapshot {
            thread_id: test_thread_id(/*id*/ 99).to_string(),
            schedule_id: "loop-1".to_string(),
            parent_schedule_id: None,
            nesting_depth: 1,
            prompt: prompt.to_string(),
            prompt_source: "inline".to_string(),
            schedule: LoopScheduleSpecSnapshot::Interval {
                amount: 5,
                unit: "minutes".to_string(),
            },
            timezone: "UTC".to_string(),
            status: "active".to_string(),
            next_run_at: Some(1_700_000_300),
            last_run_at: Some(1_700_000_200),
            expires_at: Some(1_700_003_600),
            failure_count: 1,
            lease_expires_at: Some(1_700_000_250),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_100,
            stats: LoopScheduleStatsSnapshot {
                total_runs: 3,
                leased_runs: 0,
                running_runs: 1,
                deferred_runs: 0,
                completed_runs: 1,
                failed_runs: 1,
                last_started_at: Some(1_700_000_200),
                last_completed_at: Some(1_700_000_220),
                last_error: Some("last failure had a very long diagnostic".to_string()),
            },
        }
    }

    #[test]
    fn compact_loop_response_uses_prompt_preview_without_full_prompt() {
        let response = ManageLoopResponse {
            action: LoopAction::List,
            schedule_id: None,
            affected_schedule: None,
            schedules: vec![loop_snapshot(&"loop prompt ".repeat(40))],
            deleted: None,
            message: "Listed loops for this thread.".to_string(),
        };

        let compact = compact_loop_response(&response);

        assert_eq!(compact["count"], 1);
        assert_eq!(compact["schedules"][0]["scheduleId"], "loop-1");
        assert_eq!(compact["schedules"][0]["promptChars"], 480);
        assert!(
            compact["schedules"][0]["promptPreview"]
                .as_str()
                .expect("prompt preview")
                .ends_with("...")
        );
        assert!(compact["schedules"][0]["prompt"].is_null());
        assert!(compact["schedules"][0]["leaseExpiresAt"].is_null());
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

    async fn create_interval_schedule(
        runtime: &codex_state::StateRuntime,
        thread_id: ThreadId,
        prompt: &str,
        status: codex_state::ThreadScheduleStatus,
    ) -> codex_state::ThreadSchedule {
        runtime
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: prompt.to_string(),
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule: codex_state::ThreadScheduleSpec::Interval(
                    codex_state::ThreadScheduleInterval {
                        amount: 5,
                        unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                    },
                ),
                timezone: "UTC".to_string(),
                status,
                next_run_at: Some(at(/*seconds*/ 1_700_000_300)),
                expires_at: None,
            })
            .await
            .expect("schedule should be created")
    }

    #[test]
    fn create_action_deserializes() {
        let args: ManageLoopArgs = serde_json::from_str(
            r#"{
                "action": "create",
                "prompt": "Ask for status",
                "schedule": {
                    "type": "interval",
                    "amount": 15,
                    "unit": "minutes"
                },
                "timezone": "UTC"
            }"#,
        )
        .expect("create action should deserialize");

        assert_eq!(args.action, LoopAction::Create);
        assert_eq!(args.prompt.as_deref(), Some("Ask for status"));
        assert_eq!(
            args.schedule,
            Some(LoopScheduleSpecArg::Interval {
                amount: 15,
                unit: LoopScheduleIntervalUnitArg::Minutes,
            })
        );
    }

    #[test]
    fn start_action_deserializes_separately_from_resume() {
        let args: ManageLoopArgs = serde_json::from_str(r#"{"action": "start"}"#)
            .expect("start action should deserialize");

        assert_eq!(args.action, LoopAction::Start);
    }

    #[tokio::test]
    async fn create_interval_loop_creates_active_schedule() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 7);
        upsert_test_thread(&runtime, thread_id).await;

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                prompt: Some("Ask for status".to_string()),
                schedule: Some(LoopScheduleSpecArg::Interval {
                    amount: 15,
                    unit: LoopScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect("loop should be created");

        assert_eq!(response.action, LoopAction::Create);
        assert_eq!(response.deleted, None);
        let affected_schedule = response
            .affected_schedule
            .clone()
            .expect("affected schedule should be returned");
        assert_eq!(
            response.schedule_id.as_deref(),
            Some(affected_schedule.schedule_id.as_str())
        );
        assert_eq!(affected_schedule.prompt, "Ask for status");
        assert_eq!(affected_schedule.prompt_source, "inline");
        assert_eq!(affected_schedule.timezone, "UTC");
        assert_eq!(affected_schedule.status, "active");
        assert_eq!(
            affected_schedule.schedule,
            LoopScheduleSpecSnapshot::Interval {
                amount: 15,
                unit: "minutes".to_string(),
            }
        );
        assert!(affected_schedule.next_run_at.is_some());
        assert!(affected_schedule.expires_at.is_some());
        assert_eq!(response.schedules, vec![affected_schedule.clone()]);

        let saved_schedule = runtime
            .thread_schedules()
            .get_thread_schedule(&affected_schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert!(is_loop_schedule(&saved_schedule));
        assert_eq!(saved_schedule.prompt, "Ask for status");
        assert_eq!(
            saved_schedule.status,
            codex_state::ThreadScheduleStatus::Active
        );
        assert_eq!(
            saved_schedule.schedule,
            codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                amount: 15,
                unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
            })
        );
    }

    #[tokio::test]
    async fn create_loop_accepts_nested_parent_schedule_id() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 17);
        upsert_test_thread(&runtime, thread_id).await;

        let parent_response = manage_loop(
            runtime.clone(),
            thread_id,
            None,
            ManageLoopArgs {
                prompt: Some("Parent loop".to_string()),
                schedule: Some(LoopScheduleSpecArg::Interval {
                    amount: 1,
                    unit: LoopScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect("parent loop should be created");
        let parent_schedule_id = parent_response
            .schedule_id
            .expect("parent schedule id should be returned");

        let child_response = manage_loop(
            runtime.clone(),
            thread_id,
            None,
            ManageLoopArgs {
                parent_schedule_id: Some(parent_schedule_id.clone()),
                prompt: Some("Child loop".to_string()),
                schedule: Some(LoopScheduleSpecArg::Interval {
                    amount: 2,
                    unit: LoopScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect("child loop should be created");
        let child = child_response
            .affected_schedule
            .expect("child schedule should be returned");
        assert_eq!(Some(parent_schedule_id.clone()), child.parent_schedule_id);
        assert_eq!(2, child.nesting_depth);

        let err = manage_loop(
            runtime,
            thread_id,
            None,
            ManageLoopArgs {
                parent_schedule_id: Some(parent_schedule_id),
                prompt: Some("Same minute child".to_string()),
                schedule: Some(LoopScheduleSpecArg::Dynamic),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect_err("same-minute child should be rejected");
        match err {
            FunctionCallError::RespondToModel(message) => assert!(
                message.contains("child cadence must be slower than parent cadence"),
                "unexpected model error: {message}"
            ),
            other => panic!("expected model error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_loop_nests_loops_to_depth_five() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 18);
        upsert_test_thread(&runtime, thread_id).await;

        let mut parent_schedule_id = None;
        for level in 1..=5 {
            let response = manage_loop(
                runtime.clone(),
                thread_id,
                /*auth_profile*/ None,
                ManageLoopArgs {
                    parent_schedule_id: parent_schedule_id.clone(),
                    prompt: Some(format!("level {level} loop")),
                    schedule: Some(LoopScheduleSpecArg::Interval {
                        amount: level,
                        unit: LoopScheduleIntervalUnitArg::Minutes,
                    }),
                    timezone: Some("UTC".to_string()),
                    ..loop_args(LoopAction::Create)
                },
            )
            .await
            .expect("nested loop should be created");
            let affected_schedule = response
                .affected_schedule
                .clone()
                .expect("affected schedule should be returned");
            assert_eq!(parent_schedule_id, affected_schedule.parent_schedule_id);
            assert_eq!(level, affected_schedule.nesting_depth);
            parent_schedule_id = response.schedule_id;
        }

        let err = manage_loop(
            runtime,
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                parent_schedule_id,
                prompt: Some("level 6 loop".to_string()),
                schedule: Some(LoopScheduleSpecArg::Interval {
                    amount: 6,
                    unit: LoopScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect_err("sixth nesting level should be rejected");

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(message.contains("maximum nesting depth is 5"))
            }
            other => panic!("expected model error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn clear_parent_loop_removes_nested_child_loops() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 19);
        upsert_test_thread(&runtime, thread_id).await;

        let root = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                prompt: Some("root loop".to_string()),
                schedule: Some(LoopScheduleSpecArg::Interval {
                    amount: 1,
                    unit: LoopScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect("root loop should be created");
        let root_schedule_id = root
            .schedule_id
            .expect("root schedule id should be returned");
        manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                parent_schedule_id: Some(root_schedule_id.clone()),
                prompt: Some("child loop".to_string()),
                schedule: Some(LoopScheduleSpecArg::Interval {
                    amount: 2,
                    unit: LoopScheduleIntervalUnitArg::Minutes,
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect("child loop should be created");

        let response = manage_loop(
            runtime,
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                schedule_id: Some(root_schedule_id),
                ..loop_args(LoopAction::Clear)
            },
        )
        .await
        .expect("root loop should clear");

        assert_eq!(Some(true), response.deleted);
        assert!(response.message.contains("and 1 nested child loop cleared"));
        assert_eq!(Vec::<LoopScheduleSnapshot>::new(), response.schedules);
    }

    #[tokio::test]
    async fn list_loop_returns_exact_run_stats() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 11);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;

        let first_run_at = at(/*seconds*/ 1_700_000_300);
        let first_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(first_run_at, "lease-complete", Duration::from_secs(300))
            .await
            .expect("first run should claim")
            .expect("first run should be due");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(
                &schedule.schedule_id,
                &first_claim.run.run_id,
                "lease-complete",
                "turn-complete",
            )
            .await
            .expect("first run should start")
            .expect("first run should exist");

        let second_run_at = at(/*seconds*/ 1_700_000_600);
        runtime
            .thread_schedules()
            .complete_thread_schedule_run(
                &schedule.schedule_id,
                &first_claim.run.run_id,
                "lease-complete",
                first_run_at + ChronoDuration::seconds(10),
                Some(second_run_at),
            )
            .await
            .expect("first run should complete");

        let second_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(second_run_at, "lease-fail", Duration::from_secs(300))
            .await
            .expect("second run should claim")
            .expect("second run should be due");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(
                &schedule.schedule_id,
                &second_claim.run.run_id,
                "lease-fail",
                "turn-fail",
            )
            .await
            .expect("second run should start")
            .expect("second run should exist");
        runtime
            .thread_schedules()
            .fail_thread_schedule_run(
                &schedule.schedule_id,
                &second_claim.run.run_id,
                "lease-fail",
                second_run_at + ChronoDuration::seconds(20),
                Some(at(/*seconds*/ 1_700_000_900)),
                "scheduled turn completed without a final assistant message".to_string(),
            )
            .await
            .expect("second run should fail");

        let response = manage_loop(
            runtime,
            thread_id,
            /*auth_profile*/ None,
            loop_args(LoopAction::List),
        )
        .await
        .expect("loop list should succeed");

        assert_eq!(response.schedules.len(), 1);
        assert_eq!(
            &response.schedules[0].stats,
            &LoopScheduleStatsSnapshot {
                total_runs: 2,
                leased_runs: 0,
                running_runs: 0,
                deferred_runs: 0,
                completed_runs: 1,
                failed_runs: 1,
                last_started_at: Some(second_run_at.timestamp()),
                // Only successfully completed runs contribute to last_completed_at; the second
                // run failed, so this tracks the first (completed) run's finished-at timestamp.
                last_completed_at: Some((first_run_at + ChronoDuration::seconds(10)).timestamp()),
                last_error: Some(
                    "scheduled turn completed without a final assistant message".to_string()
                ),
            }
        );
    }

    #[tokio::test]
    async fn start_with_loop_fields_creates_active_schedule() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 9);
        upsert_test_thread(&runtime, thread_id).await;

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                prompt: Some("Ask for status".to_string()),
                schedule: Some(LoopScheduleSpecArg::Cron {
                    expression: "*/15 * * * *".to_string(),
                }),
                timezone: Some("UTC".to_string()),
                ..loop_args(LoopAction::Start)
            },
        )
        .await
        .expect("start with create fields should create a loop");

        assert_eq!(response.action, LoopAction::Create);
        let affected_schedule = response
            .affected_schedule
            .clone()
            .expect("affected schedule should be returned");
        assert_eq!(affected_schedule.prompt, "Ask for status");
        assert_eq!(
            affected_schedule.schedule,
            LoopScheduleSpecSnapshot::Cron {
                expression: "*/15 * * * *".to_string(),
            }
        );
        assert_eq!(affected_schedule.status, "active");

        let saved_schedule = runtime
            .thread_schedules()
            .get_thread_schedule(&affected_schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(
            saved_schedule.schedule,
            codex_state::ThreadScheduleSpec::Cron {
                expression: "*/15 * * * *".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn start_without_loop_fields_resumes_single_loop() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 10);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Paused,
        )
        .await;

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                ..loop_args(LoopAction::Start)
            },
        )
        .await
        .expect("bare start should resume a single loop");

        assert_eq!(response.action, LoopAction::Resume);
        assert_eq!(response.schedule_id, Some(schedule.schedule_id.clone()));
        assert_eq!(
            runtime
                .thread_schedules()
                .get_thread_schedule(&schedule.schedule_id)
                .await
                .expect("schedule should load")
                .expect("schedule should exist")
                .status,
            codex_state::ThreadScheduleStatus::Active
        );
    }

    #[tokio::test]
    async fn create_loop_rejects_once_schedule() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 8);
        upsert_test_thread(&runtime, thread_id).await;

        let err = manage_loop(
            runtime,
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                prompt: Some("Ask once".to_string()),
                schedule: Some(LoopScheduleSpecArg::Once),
                timezone: Some("UTC".to_string()),
                next_run_at: Some(1_700_000_300),
                ..loop_args(LoopAction::Create)
            },
        )
        .await
        .expect_err("one-time loop should be rejected");

        match err {
            FunctionCallError::RespondToModel(message) => assert_eq!(
                message,
                "one-time schedules belong in /schedule; use interval, cron, or dynamic for /loop"
            ),
            other => panic!("expected model error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stop_without_schedule_id_pauses_single_loop() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                ..loop_args(LoopAction::Stop)
            },
        )
        .await
        .expect("single loop should stop");

        assert_eq!(response.action, LoopAction::Stop);
        assert_eq!(response.schedule_id, Some(schedule.schedule_id.clone()));
        assert_eq!(
            runtime
                .thread_schedules()
                .get_thread_schedule(&schedule.schedule_id)
                .await
                .expect("schedule should load")
                .expect("schedule should exist")
                .status,
            codex_state::ThreadScheduleStatus::Paused
        );
        assert_eq!(
            response
                .affected_schedule
                .expect("affected schedule should be returned")
                .status,
            "paused"
        );
    }

    #[tokio::test]
    async fn clear_deletes_specific_loop() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, thread_id).await;
        let first = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;
        let first_run_at = at(/*seconds*/ 1_700_000_300);
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(first_run_at, "lease-complete", Duration::from_secs(300))
            .await
            .expect("run should claim")
            .expect("run should be due");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(
                &first.schedule_id,
                &claim.run.run_id,
                "lease-complete",
                "turn-complete",
            )
            .await
            .expect("run should start")
            .expect("run should exist");
        runtime
            .thread_schedules()
            .complete_thread_schedule_run(
                &first.schedule_id,
                &claim.run.run_id,
                "lease-complete",
                first_run_at + ChronoDuration::seconds(10),
                Some(at(/*seconds*/ 1_700_000_600)),
            )
            .await
            .expect("run should complete");
        let second = create_interval_schedule(
            &runtime,
            thread_id,
            "write handoff",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                schedule_id: Some(first.schedule_id.clone()),
                ..loop_args(LoopAction::Clear)
            },
        )
        .await
        .expect("specific loop should clear");

        assert_eq!(response.action, LoopAction::Clear);
        assert_eq!(response.schedule_id, Some(first.schedule_id.clone()));
        assert_eq!(response.deleted, Some(true));
        assert_eq!(
            response
                .affected_schedule
                .as_ref()
                .expect("affected schedule should be returned")
                .stats,
            LoopScheduleStatsSnapshot {
                total_runs: 1,
                leased_runs: 0,
                running_runs: 0,
                deferred_runs: 0,
                completed_runs: 1,
                failed_runs: 0,
                last_started_at: Some(first_run_at.timestamp()),
                last_completed_at: Some((first_run_at + ChronoDuration::seconds(10)).timestamp()),
                last_error: None,
            }
        );
        assert!(
            runtime
                .thread_schedules()
                .get_thread_schedule(&first.schedule_id)
                .await
                .expect("schedule lookup should succeed")
                .is_none()
        );
        assert!(
            runtime
                .thread_schedules()
                .get_thread_schedule(&second.schedule_id)
                .await
                .expect("schedule lookup should succeed")
                .is_some()
        );
        assert_eq!(response.schedules.len(), 1);
        assert_eq!(response.schedules[0].schedule_id, second.schedule_id);
    }

    #[tokio::test]
    async fn resume_recomputes_missing_next_run_at_and_resets_failure_count() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 6);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(
                at(/*seconds*/ 1_700_000_300),
                "lease-fail",
                Duration::from_secs(300),
            )
            .await
            .expect("run should claim")
            .expect("run should be due");
        runtime
            .thread_schedules()
            .fail_thread_schedule_run(
                &schedule.schedule_id,
                &claim.run.run_id,
                "lease-fail",
                at(/*seconds*/ 1_700_000_310),
                /*next_run_at*/ None,
                "model unavailable".to_string(),
            )
            .await
            .expect("run should fail");
        let before_resume = Utc::now();

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                schedule_id: Some(schedule.schedule_id.clone()),
                ..loop_args(LoopAction::Resume)
            },
        )
        .await
        .expect("loop should resume");

        let updated = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(codex_state::ThreadScheduleStatus::Active, updated.status);
        assert_eq!(0, updated.failure_count);
        let next_run_at = updated.next_run_at.expect("next_run_at should be set");
        assert!(next_run_at >= before_resume);
        let affected_schedule = response
            .affected_schedule
            .expect("affected schedule should be returned");
        assert_eq!(Some(next_run_at.timestamp()), affected_schedule.next_run_at);
        assert_eq!(0, affected_schedule.failure_count);
    }

    #[tokio::test]
    async fn resume_rejects_loop_with_past_expiry() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 17);
        upsert_test_thread(&runtime, thread_id).await;
        let schedule = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Paused,
        )
        .await;
        runtime
            .thread_schedules()
            .update_thread_schedule(
                &schedule.schedule_id,
                codex_state::ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: None,
                    timezone: None,
                    status: None,
                    next_run_at: None,
                    expires_at: Some(Some(at(/*seconds*/ 1_700_000_600))),
                },
            )
            .await
            .expect("schedule should update")
            .expect("schedule should exist");

        let error = manage_loop(
            runtime,
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                schedule_id: Some(schedule.schedule_id),
                ..loop_args(LoopAction::Resume)
            },
        )
        .await
        .expect_err("expired loop should not resume");

        assert_eq!(
            error,
            FunctionCallError::RespondToModel(
                "loop expires_at must be in the future to resume".to_string()
            )
        );
    }

    #[tokio::test]
    async fn stop_rejects_schedule_from_another_thread() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 3);
        let other_thread_id = test_thread_id(/*id*/ 4);
        upsert_test_thread(&runtime, thread_id).await;
        upsert_test_thread(&runtime, other_thread_id).await;
        let other_schedule = create_interval_schedule(
            &runtime,
            other_thread_id,
            "check other thread",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;

        let err = manage_loop(
            runtime.clone(),
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                schedule_id: Some(other_schedule.schedule_id.clone()),
                ..loop_args(LoopAction::Stop)
            },
        )
        .await
        .expect_err("foreign-thread schedule should be rejected");

        match err {
            FunctionCallError::RespondToModel(message) => assert_eq!(
                message,
                format!(
                    "no loop with schedule_id `{}` exists in this thread",
                    other_schedule.schedule_id
                )
            ),
            other => panic!("expected model error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_loops_require_schedule_id_for_mutations() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 5);
        upsert_test_thread(&runtime, thread_id).await;
        create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;
        create_interval_schedule(
            &runtime,
            thread_id,
            "write handoff",
            codex_state::ThreadScheduleStatus::Paused,
        )
        .await;

        let err = manage_loop(
            runtime,
            thread_id,
            /*auth_profile*/ None,
            ManageLoopArgs {
                ..loop_args(LoopAction::Clear)
            },
        )
        .await
        .expect_err("ambiguous mutation should fail");

        match err {
            FunctionCallError::RespondToModel(message) => assert_eq!(
                message,
                "cannot clear a loop without schedule_id because this thread has multiple loops; call manage_loop with action=list, then pass the chosen schedule_id"
            ),
            other => panic!("expected model error, got {other:?}"),
        }
    }
}
