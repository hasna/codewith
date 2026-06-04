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
use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::Arc;

pub struct ManageLoopHandler;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ManageLoopArgs {
    action: LoopAction,
    schedule_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum LoopAction {
    List,
    #[serde(alias = "pause")]
    Stop,
    #[serde(alias = "start")]
    Resume,
    #[serde(alias = "delete", alias = "remove", alias = "cancel")]
    Clear,
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
        let state_db = session.state_db().ok_or_else(|| {
            FunctionCallError::Fatal("sqlite state db is unavailable for this session".to_string())
        })?;
        let response = manage_loop(state_db, session.thread_id(), args).await?;
        loop_response(response).map(boxed_tool_output)
    }
}

impl CoreToolRuntime for ManageLoopHandler {}

async fn manage_loop(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageLoopArgs,
) -> Result<ManageLoopResponse, FunctionCallError> {
    match args.action {
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
                resolve_loop_schedule_id(&state_db, thread_id, args.schedule_id, args.action)
                    .await?;
            let existing =
                ensure_current_thread_schedule(&state_db, thread_id, schedule_id.as_str()).await?;
            let status = match args.action {
                LoopAction::Stop => codex_state::ThreadScheduleStatus::Paused,
                LoopAction::Resume => codex_state::ThreadScheduleStatus::Active,
                LoopAction::List | LoopAction::Clear => unreachable!("action matched above"),
            };
            let schedule = if args.action == LoopAction::Resume && existing.next_run_at.is_none() {
                let next_run_at =
                    next_loop_run_at(&existing.schedule, &existing.timezone, Utc::now())?
                        .ok_or_else(|| {
                            FunctionCallError::RespondToModel(
                                "cannot resume loop because no next run time could be computed"
                                    .to_string(),
                            )
                        })?;
                if let Some(expires_at) = existing.expires_at
                    && expires_at <= next_run_at
                {
                    return Err(FunctionCallError::RespondToModel(
                        "loop expires_at must be later than next_run_at".to_string(),
                    ));
                }
                state_db
                    .thread_schedules()
                    .update_thread_schedule(
                        schedule_id.as_str(),
                        codex_state::ThreadScheduleUpdate {
                            prompt: None,
                            prompt_source: None,
                            schedule: None,
                            timezone: None,
                            status: Some(status),
                            next_run_at: Some(Some(next_run_at)),
                            expires_at: None,
                        },
                    )
                    .await
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
            let affected_schedule = LoopScheduleSnapshot::from(schedule);
            let schedules = list_loop_snapshots(&state_db, thread_id).await?;
            let message = match args.action {
                LoopAction::Stop => {
                    format!("Loop {schedule_id} stopped; future runs are paused.")
                }
                LoopAction::Resume => {
                    format!("Loop {schedule_id} resumed; future runs are active.")
                }
                LoopAction::List | LoopAction::Clear => unreachable!("action matched above"),
            };
            Ok(ManageLoopResponse {
                action: args.action,
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
            let deleted = state_db
                .thread_schedules()
                .delete_thread_schedule(schedule_id.as_str())
                .await
                .map_err(|err| FunctionCallError::RespondToModel(format_loop_error(err)))?;
            let schedules = list_loop_snapshots(&state_db, thread_id).await?;
            Ok(ManageLoopResponse {
                action: LoopAction::Clear,
                schedule_id: Some(schedule_id.clone()),
                affected_schedule: Some(LoopScheduleSnapshot::from(schedule)),
                schedules,
                deleted: Some(deleted),
                message: if deleted {
                    format!("Loop {schedule_id} cleared.")
                } else {
                    format!("Loop {schedule_id} was already absent.")
                },
            })
        }
    }
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
    Ok(schedules
        .into_iter()
        .filter(is_loop_schedule)
        .map(LoopScheduleSnapshot::from)
        .collect())
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

fn next_loop_run_at(
    schedule: &codex_state::ThreadScheduleSpec,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, FunctionCallError> {
    let next = match schedule {
        codex_state::ThreadScheduleSpec::Once => None,
        codex_state::ThreadScheduleSpec::Dynamic => {
            after.checked_add_signed(ChronoDuration::minutes(1))
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
            let timezone = timezone.parse::<Tz>().map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "invalid loop timezone `{timezone}`: {err}"
                ))
            })?;
            let cron = Cron::from_str(expression).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "invalid cron expression `{expression}`: {err}"
                ))
            })?;
            let local_after = after.with_timezone(&timezone);
            let next = cron
                .find_next_occurrence(&local_after, /*inclusive*/ false)
                .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
            Some(next.with_timezone(&Utc))
        }
    };
    Ok(next)
}

impl LoopAction {
    fn verb(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Stop => "stop",
            Self::Resume => "resume",
            Self::Clear => "clear",
        }
    }
}

impl From<codex_state::ThreadSchedule> for LoopScheduleSnapshot {
    fn from(schedule: codex_state::ThreadSchedule) -> Self {
        Self {
            thread_id: schedule.thread_id.to_string(),
            schedule_id: schedule.schedule_id,
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

fn loop_response(response: ManageLoopResponse) -> Result<FunctionToolOutput, FunctionCallError> {
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
            at(1_700_000_000),
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
                next_run_at: Some(at(1_700_000_300)),
                expires_at: None,
            })
            .await
            .expect("schedule should be created")
    }

    #[tokio::test]
    async fn stop_without_schedule_id_pauses_single_loop() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(1);
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
            ManageLoopArgs {
                action: LoopAction::Stop,
                schedule_id: None,
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
        let thread_id = test_thread_id(2);
        upsert_test_thread(&runtime, thread_id).await;
        let first = create_interval_schedule(
            &runtime,
            thread_id,
            "check CI",
            codex_state::ThreadScheduleStatus::Active,
        )
        .await;
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
            ManageLoopArgs {
                action: LoopAction::Clear,
                schedule_id: Some(first.schedule_id.clone()),
            },
        )
        .await
        .expect("specific loop should clear");

        assert_eq!(response.action, LoopAction::Clear);
        assert_eq!(response.schedule_id, Some(first.schedule_id.clone()));
        assert_eq!(response.deleted, Some(true));
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
    async fn resume_recomputes_missing_next_run_at() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(6);
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
                    next_run_at: Some(None),
                    expires_at: None,
                },
            )
            .await
            .expect("schedule should update")
            .expect("schedule should exist");
        let before_resume = Utc::now();

        let response = manage_loop(
            runtime.clone(),
            thread_id,
            ManageLoopArgs {
                action: LoopAction::Resume,
                schedule_id: Some(schedule.schedule_id.clone()),
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
        let next_run_at = updated.next_run_at.expect("next_run_at should be set");
        assert!(next_run_at >= before_resume);
        let affected_schedule = response
            .affected_schedule
            .expect("affected schedule should be returned");
        assert_eq!(Some(next_run_at.timestamp()), affected_schedule.next_run_at);
    }

    #[tokio::test]
    async fn stop_rejects_schedule_from_another_thread() {
        let (_temp_dir, runtime) = test_runtime().await;
        let thread_id = test_thread_id(3);
        let other_thread_id = test_thread_id(4);
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
            ManageLoopArgs {
                action: LoopAction::Stop,
                schedule_id: Some(other_schedule.schedule_id.clone()),
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
        let thread_id = test_thread_id(5);
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
            ManageLoopArgs {
                action: LoopAction::Clear,
                schedule_id: None,
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
