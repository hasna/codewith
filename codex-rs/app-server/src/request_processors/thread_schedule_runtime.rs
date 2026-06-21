use super::*;
use crate::request_processors::thread_schedule_api::api_thread_schedule_from_state;
use crate::request_processors::thread_schedule_api::api_thread_schedule_run_from_state;
use chrono_tz::Tz;
use codex_goal_extension::GoalObjectiveUpdate;
use codex_goal_extension::GoalService;
use codex_goal_extension::GoalSetRequest;
use codex_goal_extension::GoalTokenBudgetUpdate;
use croner::Cron;
use std::str::FromStr;

const SCHEDULE_POLL_INTERVAL: Duration = Duration::from_secs(10);
const SCHEDULE_LEASE_DURATION: Duration = Duration::from_secs(10 * 60);
const SCHEDULE_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAX_SCHEDULE_CLAIMS_PER_TICK: usize = 16;
const DEFAULT_DYNAMIC_INTERVAL_MINUTES: i64 = 1;
const DEFAULT_SCHEDULE_EXPIRATION_DAYS: i64 = 7;
const MAX_SCHEDULE_RUN_ERROR_CHARS: usize = 1_000;

#[derive(Clone)]
pub(crate) struct ThreadScheduleRuntime {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
    thread_state_manager: ThreadStateManager,
    pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
    thread_watch_manager: ThreadWatchManager,
    thread_list_state_permit: Arc<Semaphore>,
    skills_watcher: Arc<SkillsWatcher>,
    state_db: Option<StateDbHandle>,
    goal_service: Arc<GoalService>,
    cancel_token: CancellationToken,
    tasks: TaskTracker,
}

impl ThreadScheduleRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
        thread_state_manager: ThreadStateManager,
        pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
        thread_watch_manager: ThreadWatchManager,
        thread_list_state_permit: Arc<Semaphore>,
        skills_watcher: Arc<SkillsWatcher>,
        state_db: Option<StateDbHandle>,
        goal_service: Arc<GoalService>,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,
            config,
            config_manager,
            thread_state_manager,
            pending_thread_unloads,
            thread_watch_manager,
            thread_list_state_permit,
            skills_watcher,
            state_db,
            goal_service,
            cancel_token: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }

    pub(crate) fn start(&self) {
        if self.state_db.is_none() {
            return;
        }
        let runtime = self.clone();
        self.tasks.spawn(async move {
            runtime.run().await;
        });
    }

    pub(crate) fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    pub(crate) async fn drain_background_tasks(&self) {
        self.shutdown();
        self.tasks.close();
        if tokio::time::timeout(Duration::from_secs(10), self.tasks.wait())
            .await
            .is_err()
        {
            warn!("timed out waiting for thread schedule runtime to shut down; proceeding");
        }
    }

    pub(crate) fn spawn_claim_execution(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
    ) {
        let runtime = self.clone();
        self.tasks.spawn(async move {
            runtime.execute_claim(state_db, claim).await;
        });
    }

    async fn run(self) {
        let mut interval = tokio::time::interval(SCHEDULE_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => break,
                _ = interval.tick() => self.tick().await,
            }
        }
    }

    async fn tick(&self) {
        if !self.config.features.enabled(Feature::ScheduledTasks) {
            return;
        }
        let Some(state_db) = self.state_db.as_ref() else {
            return;
        };
        let now = Utc::now();
        if let Err(err) = state_db
            .thread_schedules()
            .expire_thread_schedules(now)
            .await
        {
            warn!("failed to expire thread schedules: {err}");
        }
        for _ in 0..MAX_SCHEDULE_CLAIMS_PER_TICK {
            let lease_id = Uuid::new_v4().to_string();
            let claim = match state_db
                .thread_schedules()
                .claim_due_thread_schedule(now, lease_id.as_str(), SCHEDULE_LEASE_DURATION)
                .await
            {
                Ok(Some(claim)) => claim,
                Ok(None) => break,
                Err(err) => {
                    warn!("failed to claim due thread schedule: {err}");
                    break;
                }
            };
            self.execute_claim(state_db.clone(), claim).await;
        }
    }

    async fn execute_claim(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
    ) {
        let thread_id = claim.schedule.thread_id;
        self.emit_schedule_run_updated(thread_id, claim.run.clone())
            .await;

        let result = self
            .submit_claimed_schedule(thread_id, state_db.clone(), &claim)
            .await;
        if let Err(err) = result {
            warn!(
                schedule_id = %claim.schedule.schedule_id,
                thread_id = %thread_id,
                "failed to submit scheduled thread run: {err}"
            );
            self.fail_claimed_run_after_submit_error(
                state_db,
                claim,
                schedule_run_error(err.to_string()),
            )
            .await;
        }
    }

    async fn submit_claimed_schedule(
        &self,
        thread_id: ThreadId,
        state_db: StateDbHandle,
        claim: &codex_state::ThreadScheduleClaim,
    ) -> anyhow::Result<()> {
        let prompt = self
            .resolve_claim_prompt(&state_db, thread_id, &claim.schedule)
            .await?;
        let scheduled_goal_objective = scheduled_goal_objective(&prompt).map(str::to_string);
        let claim_auth_profile = self
            .claim_auth_profile(&state_db, thread_id, &claim.schedule)
            .await;
        let thread = self
            .load_or_resume_thread(thread_id, claim_auth_profile.clone())
            .await?;
        self.ensure_schedule_listener(thread_id, thread.clone())
            .await?;
        let thread_state = self.thread_state_manager.thread_state(thread_id).await;
        let listener_command_tx = {
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        let turn_prompt = if let Some(objective) = scheduled_goal_objective.as_deref() {
            self.prepare_scheduled_goal(
                thread_id,
                &state_db,
                objective,
                listener_command_tx.clone(),
            )
            .await?;
            scheduled_goal_thread_prompt(
                objective,
                claim.run.run_id.as_str(),
                claim.run.scheduled_for,
            )
        } else {
            scheduled_thread_prompt(&prompt, claim.run.run_id.as_str(), claim.run.scheduled_for)
        };
        let thread_settings = scheduled_thread_settings_from_snapshot(
            thread.config_snapshot().await,
            claim_auth_profile,
        );
        thread_state.lock().await.begin_scheduled_run_submission();
        let submit_result = thread
            .submit(Op::UserInput {
                items: vec![CoreInputItem::Text {
                    text: turn_prompt,
                    text_elements: Vec::new(),
                }],
                environments: None,
                final_output_json_schema: None,
                responsesapi_client_metadata: None,
                additional_context: Default::default(),
                thread_settings,
            })
            .await;
        let turn_id = match submit_result {
            Ok(turn_id) => turn_id,
            Err(err) => {
                thread_state.lock().await.finish_scheduled_run_submission();
                return Err(anyhow::anyhow!("failed to submit scheduled prompt: {err}"));
            }
        };

        let run = match state_db
            .thread_schedules()
            .mark_thread_schedule_run_started(
                claim.schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                turn_id.as_str(),
            )
            .await
        {
            Ok(Some(run)) => run,
            Ok(None) => {
                thread_state.lock().await.finish_scheduled_run_submission();
                return Err(anyhow::anyhow!(
                    "claimed schedule run {} disappeared before it could start",
                    claim.run.run_id
                ));
            }
            Err(err) => {
                thread_state.lock().await.finish_scheduled_run_submission();
                return Err(err);
            }
        };
        {
            let mut thread_state = thread_state.lock().await;
            thread_state.finish_scheduled_run_submission();
            thread_state.track_scheduled_run(
                turn_id,
                crate::thread_state::ScheduledThreadScheduleRun {
                    schedule_id: claim.schedule.schedule_id.clone(),
                    run_id: claim.run.run_id.clone(),
                    lease_id: claim.run.lease_id.clone(),
                    state_db: state_db.clone(),
                },
            );
        }
        self.spawn_lease_heartbeat(
            state_db,
            claim.schedule.schedule_id.clone(),
            claim.run.lease_id.clone(),
        );
        self.emit_schedule_run_updated(thread_id, run).await;
        Ok(())
    }

    async fn prepare_scheduled_goal(
        &self,
        thread_id: ThreadId,
        state_db: &StateDbHandle,
        objective: &str,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) -> anyhow::Result<()> {
        if !self.config.features.enabled(Feature::Goals) {
            anyhow::bail!("goals feature is disabled");
        }

        let outcome = self
            .goal_service
            .set_thread_goal(
                state_db,
                GoalSetRequest {
                    thread_id,
                    objective: GoalObjectiveUpdate::Set(objective),
                    status: Some(codex_protocol::protocol::ThreadGoalStatus::Active),
                    token_budget: GoalTokenBudgetUpdate::Keep,
                    auto_execute: thread_goal_processor::goal_auto_execute_from_config(
                        &self.config,
                    ),
                },
            )
            .await
            .map_err(|err| anyhow::anyhow!("failed to set scheduled goal: {err}"))?;
        let goal = ThreadGoal::from(outcome.goal.clone());
        let goal_id = goal.goal_id.clone();
        self.emit_thread_goal_updated_ordered(thread_id, goal, listener_command_tx.clone())
            .await;
        if let Some(plan_update) = outcome.plan_update.clone() {
            let plan = thread_goal_processor::api_thread_goal_plan_from_state(plan_update.snapshot);
            self.emit_thread_goal_plan_updated_ordered(thread_id, plan, listener_command_tx)
                .await;
        }

        self.goal_service
            .suppress_next_idle_continuation(thread_id, goal_id.as_str());
        outcome.apply_runtime_effects(&self.goal_service).await;
        self.goal_service
            .suppress_next_idle_continuation(thread_id, goal_id.as_str());
        Ok(())
    }

    async fn resolve_claim_prompt(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        schedule: &codex_state::ThreadSchedule,
    ) -> anyhow::Result<String> {
        match schedule.prompt_source {
            codex_state::ThreadSchedulePromptSource::Inline => Ok(schedule.prompt.clone()),
            codex_state::ThreadSchedulePromptSource::Default => {
                crate::request_processors::thread_schedule_default_prompt::resolve_default_loop_prompt_for_thread(
                    state_db,
                    thread_id,
                    self.config.cwd.as_path(),
                    self.config.codex_home.as_path(),
                )
                .await
                .map(|resolved| resolved.prompt)
            }
        }
    }

    async fn claim_auth_profile(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        schedule: &codex_state::ThreadSchedule,
    ) -> Option<Option<String>> {
        if schedule.auth_profile.is_some() {
            return schedule.auth_profile.clone();
        }
        self.legacy_schedule_auth_profile_from_rollout(state_db, thread_id)
            .await
    }

    async fn legacy_schedule_auth_profile_from_rollout(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
    ) -> Option<Option<String>> {
        let rollout_path = match codex_rollout::find_thread_path_by_id_str(
            &self.config.codex_home,
            &thread_id.to_string(),
            Some(state_db),
        )
        .await
        {
            Ok(Some(path)) => path,
            Ok(None) => return None,
            Err(err) => {
                warn!(
                    "failed to locate rollout for legacy scheduled auth profile {thread_id}: {err}"
                );
                return None;
            }
        };
        let initial_history =
            match codex_rollout::RolloutRecorder::get_rollout_history(&rollout_path).await {
                Ok(history) => history,
                Err(err) => {
                    warn!(
                        "failed to load rollout {} for legacy scheduled auth profile: {err}",
                        rollout_path.display()
                    );
                    return None;
                }
            };
        schedule_resume_auth_profile(/*schedule_auth_profile*/ None, &initial_history)
    }

    fn spawn_lease_heartbeat(
        &self,
        state_db: StateDbHandle,
        schedule_id: String,
        lease_id: String,
    ) {
        let cancel_token = self.cancel_token.clone();
        self.tasks.spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = tokio::time::sleep(SCHEDULE_LEASE_HEARTBEAT_INTERVAL) => {}
                }
                match state_db
                    .thread_schedules()
                    .extend_thread_schedule_lease(
                        schedule_id.as_str(),
                        lease_id.as_str(),
                        Utc::now(),
                        SCHEDULE_LEASE_DURATION,
                    )
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(err) => warn!(
                        schedule_id = %schedule_id,
                        "failed to refresh scheduled thread lease: {err}"
                    ),
                }
            }
        });
    }

    async fn load_or_resume_thread(
        &self,
        thread_id: ThreadId,
        schedule_auth_profile: Option<Option<String>>,
    ) -> anyhow::Result<Arc<CodexThread>> {
        if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
            return Ok(thread);
        }
        let Some(state_db) = self.state_db.as_ref() else {
            anyhow::bail!("sqlite state db unavailable for scheduled task resume");
        };
        let rollout_path = codex_rollout::find_thread_path_by_id_str(
            &self.config.codex_home,
            &thread_id.to_string(),
            Some(state_db),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("thread not found: {thread_id}"))?;
        let initial_history = codex_rollout::RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .map_err(|err| {
                anyhow::anyhow!(
                    "failed to load rollout {} for scheduled task: {err}",
                    rollout_path.display()
                )
            })?;
        let history_cwd = initial_history.session_cwd();
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides::default();
        apply_persisted_schedule_resume_metadata(
            state_db,
            thread_id,
            schedule_auth_profile,
            &initial_history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;
        let config = self
            .config_manager
            .load_for_cwd(request_overrides, typesafe_overrides, history_cwd)
            .await
            .map_err(|err| anyhow::anyhow!("failed to load config for scheduled task: {err}"))?;
        self.thread_manager
            .resume_thread_with_history(
                config,
                initial_history,
                Arc::clone(&self.auth_manager),
                /*parent_trace*/ None,
            )
            .await
            .map(|new_thread| new_thread.thread)
            .map_err(|err| anyhow::anyhow!("failed to resume scheduled task thread: {err}"))
    }

    async fn ensure_schedule_listener(
        &self,
        thread_id: ThreadId,
        thread: Arc<CodexThread>,
    ) -> anyhow::Result<()> {
        let thread_state = self.thread_state_manager.thread_state(thread_id).await;
        let context = ListenerTaskContext {
            thread_manager: Arc::clone(&self.thread_manager),
            thread_state_manager: self.thread_state_manager.clone(),
            outgoing: Arc::clone(&self.outgoing),
            pending_thread_unloads: Arc::clone(&self.pending_thread_unloads),
            thread_watch_manager: self.thread_watch_manager.clone(),
            thread_list_state_permit: Arc::clone(&self.thread_list_state_permit),
            fallback_model_provider: self.config.model_provider_id.clone(),
            codex_home: self.config.codex_home.to_path_buf(),
            skills_watcher: Arc::clone(&self.skills_watcher),
        };
        ensure_listener_task_running(context, thread_id, thread, thread_state)
            .await
            .map_err(|err| anyhow::anyhow!(err.message))
    }

    async fn fail_claimed_run_after_submit_error(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
        error: String,
    ) {
        match finish_scheduled_run_state(
            &state_db,
            &claim.schedule.schedule_id,
            &claim.run.run_id,
            &claim.run.lease_id,
            Some(error),
            Utc::now(),
        )
        .await
        {
            Ok(Some((schedule, run))) => {
                self.emit_schedule_updated(claim.schedule.thread_id, schedule)
                    .await;
                self.emit_schedule_run_updated(claim.schedule.thread_id, run)
                    .await;
            }
            Ok(None) => {}
            Err(err) => warn!(
                schedule_id = %claim.schedule.schedule_id,
                "failed to mark scheduled thread run as failed: {err}"
            ),
        }
    }

    async fn emit_thread_goal_updated_ordered(
        &self,
        thread_id: ThreadId,
        goal: ThreadGoal,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = ThreadListenerCommand::EmitThreadGoalUpdated {
                turn_id: None,
                goal: goal.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue scheduled goal update for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadGoalUpdated(
                ThreadGoalUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    turn_id: None,
                    goal,
                },
            ))
            .await;
    }

    async fn emit_thread_goal_plan_updated_ordered(
        &self,
        thread_id: ThreadId,
        plan: ThreadGoalPlan,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = ThreadListenerCommand::EmitThreadGoalPlanUpdated {
                turn_id: None,
                plan: plan.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue scheduled goal plan update for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadGoalPlanUpdated(
                ThreadGoalPlanUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    turn_id: None,
                    plan,
                },
            ))
            .await;
    }

    async fn emit_schedule_updated(
        &self,
        thread_id: ThreadId,
        schedule: codex_state::ThreadSchedule,
    ) {
        self.outgoing
            .send_server_notification(ServerNotification::ThreadScheduleUpdated(
                ThreadScheduleUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    schedule: api_thread_schedule_from_state(schedule),
                },
            ))
            .await;
    }

    async fn emit_schedule_run_updated(
        &self,
        thread_id: ThreadId,
        run: codex_state::ThreadScheduleRun,
    ) {
        self.outgoing
            .send_server_notification(ServerNotification::ThreadScheduleRunUpdated(
                ThreadScheduleRunUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    run: api_thread_schedule_run_from_state(run),
                },
            ))
            .await;
    }
}

fn scheduled_thread_prompt(
    prompt: &str,
    run_id: &str,
    scheduled_for: Option<DateTime<Utc>>,
) -> String {
    let scheduled_for = scheduled_for
        .map(|scheduled_for| scheduled_for.to_rfc3339())
        .unwrap_or_else(|| "immediate".to_string());
    format!(
        "\
You are running one new scheduled Codewith prompt.

Run id: {run_id}
Scheduled for: {scheduled_for}

This is a distinct run even if the scheduled prompt matches earlier runs. Execute only the scheduled prompt below for this run. Produce exactly one visible final response for this scheduled run, even if there are no changes, no action is needed, or the run is blocked. Do not wait, sleep, start a timer, or schedule follow-up runs; Codewith manages scheduling. If the prompt mentions a cadence like \"every minute\", treat that as schedule context, not as an instruction to implement the cadence yourself.

Scheduled prompt:
{prompt}"
    )
}

fn scheduled_goal_objective(prompt: &str) -> Option<&str> {
    let trimmed = prompt.trim_start();
    let rest = trimmed.strip_prefix("/goal")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim())
}

fn scheduled_goal_thread_prompt(
    objective: &str,
    run_id: &str,
    scheduled_for: Option<DateTime<Utc>>,
) -> String {
    let scheduled_for = scheduled_for
        .map(|scheduled_for| scheduled_for.to_rfc3339())
        .unwrap_or_else(|| "immediate".to_string());
    format!(
        "\
You are running one new scheduled Codewith goal objective.

Run id: {run_id}
Scheduled for: {scheduled_for}

The active thread goal has already been persisted for this scheduled run. Work on only the goal objective below for this run. Produce exactly one visible final response for this scheduled run, even if there are no changes, no action is needed, or the run is blocked. Do not create new goals, loops, schedules, monitors, timers, or follow-up runs; Codewith manages scheduling. If the objective mentions a cadence like \"every minute\", treat that as schedule context, not as an instruction to implement the cadence yourself.

Goal objective:
{objective}"
    )
}

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
enum ScheduledTurnFinish {
    Complete,
    Failed(String),
}

fn scheduled_turn_finish(event: &EventMsg) -> Option<ScheduledTurnFinish> {
    match event {
        EventMsg::TurnComplete(completed)
            if completed
                .last_agent_message
                .as_deref()
                .is_some_and(|message| !message.trim().is_empty()) =>
        {
            Some(ScheduledTurnFinish::Complete)
        }
        EventMsg::TurnComplete(_) => Some(ScheduledTurnFinish::Failed(schedule_run_error(
            "scheduled turn completed without a final assistant message".to_string(),
        ))),
        EventMsg::TurnAborted(aborted) => Some(ScheduledTurnFinish::Failed(schedule_run_error(
            format!("scheduled turn aborted: {:?}", aborted.reason),
        ))),
        _ => None,
    }
}

pub(super) fn default_thread_schedule_expires_at(now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    now.checked_add_signed(ChronoDuration::days(DEFAULT_SCHEDULE_EXPIRATION_DAYS))
}

pub(super) fn next_thread_schedule_run_at(
    schedule: &codex_state::ThreadScheduleSpec,
    timezone: &str,
    after: DateTime<Utc>,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    let next = match schedule {
        codex_state::ThreadScheduleSpec::Once => None,
        codex_state::ThreadScheduleSpec::Dynamic => {
            after.checked_add_signed(ChronoDuration::minutes(DEFAULT_DYNAMIC_INTERVAL_MINUTES))
        }
        codex_state::ThreadScheduleSpec::Interval(interval) => {
            let amount = interval.amount;
            let duration = match interval.unit {
                codex_state::ThreadScheduleIntervalUnit::Minutes => ChronoDuration::minutes(amount),
                codex_state::ThreadScheduleIntervalUnit::Hours => ChronoDuration::hours(amount),
                codex_state::ThreadScheduleIntervalUnit::Days => ChronoDuration::days(amount),
            };
            after.checked_add_signed(duration)
        }
        codex_state::ThreadScheduleSpec::Cron { expression } => {
            let timezone = parse_schedule_timezone(timezone)?;
            let cron = Cron::from_str(expression)
                .map_err(|err| anyhow::anyhow!("invalid cron expression `{expression}`: {err}"))?;
            let local_after = after.with_timezone(&timezone);
            let next = cron.find_next_occurrence(&local_after, /*inclusive*/ false)?;
            Some(next.with_timezone(&Utc))
        }
    };
    Ok(next)
}

pub(super) fn normalize_schedule_timezone(timezone: &str) -> anyhow::Result<String> {
    parse_schedule_timezone(timezone).map(|timezone| timezone.name().to_string())
}

pub(super) async fn finish_scheduled_run_after_turn(
    thread_id: ThreadId,
    scheduled_run: crate::thread_state::ScheduledThreadScheduleRun,
    event: &EventMsg,
    turn_error: Option<codex_app_server_protocol::TurnError>,
    outgoing: &Arc<OutgoingMessageSender>,
) {
    let completed_at = Utc::now();
    let error = match (scheduled_turn_finish(event), turn_error) {
        (Some(_), Some(error)) => Some(schedule_run_error(format!(
            "scheduled turn failed: {}",
            error.message
        ))),
        (Some(ScheduledTurnFinish::Complete), None) => None,
        (Some(ScheduledTurnFinish::Failed(error)), None) => Some(error),
        (None, _) => return,
    };
    match finish_scheduled_run_state(
        &scheduled_run.state_db,
        scheduled_run.schedule_id.as_str(),
        scheduled_run.run_id.as_str(),
        scheduled_run.lease_id.as_str(),
        error,
        completed_at,
    )
    .await
    {
        Ok(Some((schedule, run))) => {
            outgoing
                .send_server_notification(ServerNotification::ThreadScheduleUpdated(
                    ThreadScheduleUpdatedNotification {
                        thread_id: thread_id.to_string(),
                        schedule: api_thread_schedule_from_state(schedule),
                    },
                ))
                .await;
            outgoing
                .send_server_notification(ServerNotification::ThreadScheduleRunUpdated(
                    ThreadScheduleRunUpdatedNotification {
                        thread_id: thread_id.to_string(),
                        run: api_thread_schedule_run_from_state(run),
                    },
                ))
                .await;
        }
        Ok(None) => {}
        Err(err) => warn!(
            schedule_id = %scheduled_run.schedule_id,
            thread_id = %thread_id,
            "failed to finish scheduled thread run: {err}"
        ),
    }
}

/// Maximum consecutive failed runs before a recurring schedule stops
/// re-arming itself (circuit breaker). The streak resets after a success.
const MAX_CONSECUTIVE_SCHEDULE_FAILURES: i64 = 10;

/// Push a failed schedule's next run out with exponential backoff — 30s, 60s,
/// 120s, … capped at one hour — but never earlier than its natural next
/// cadence. `consecutive_failures` is 1 for the first failure in a streak.
fn schedule_failure_backoff_run_at(
    natural_next: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    consecutive_failures: i64,
) -> DateTime<Utc> {
    let exponent = consecutive_failures.saturating_sub(1).clamp(0, 7) as u32;
    let backoff_seconds = 30i64.saturating_mul(1i64 << exponent).min(3600);
    let backoff_until = completed_at + chrono::Duration::seconds(backoff_seconds);
    natural_next.max(backoff_until)
}

async fn finish_scheduled_run_state(
    state_db: &StateDbHandle,
    schedule_id: &str,
    run_id: &str,
    lease_id: &str,
    error: Option<String>,
    completed_at: DateTime<Utc>,
) -> anyhow::Result<Option<(codex_state::ThreadSchedule, codex_state::ThreadScheduleRun)>> {
    let Some(schedule) = state_db
        .thread_schedules()
        .get_thread_schedule(schedule_id)
        .await?
    else {
        return Ok(None);
    };
    let natural_next_run_at =
        next_thread_schedule_run_at(&schedule.schedule, &schedule.timezone, completed_at)?;

    // On failure, back off (and eventually trip a circuit breaker) so a
    // persistently-failing recurring schedule cannot re-fire every cadence
    // indefinitely until expiry (resource exhaustion). `failure_count` is the
    // streak BEFORE this run; it resets to 0 on the first successful run.
    let next_run_at = match &error {
        Some(_) => {
            let consecutive_failures = schedule.failure_count.saturating_add(1);
            if consecutive_failures >= MAX_CONSECUTIVE_SCHEDULE_FAILURES {
                // Circuit breaker: stop re-arming until a human intervenes.
                None
            } else {
                natural_next_run_at.map(|natural_next| {
                    schedule_failure_backoff_run_at(
                        natural_next,
                        completed_at,
                        consecutive_failures,
                    )
                })
            }
        }
        None => natural_next_run_at,
    };

    // Never schedule a run at or after the schedule's expiry (applied after any
    // failure backoff so backing off past expiry stops the schedule).
    let next_run_at = match (next_run_at, schedule.expires_at) {
        (Some(next_run_at), Some(expires_at)) if next_run_at >= expires_at => None,
        (next_run_at, _) => next_run_at,
    };
    let updated = if let Some(error) = error {
        state_db
            .thread_schedules()
            .fail_thread_schedule_run(
                schedule_id,
                run_id,
                lease_id,
                completed_at,
                next_run_at,
                error,
            )
            .await?
    } else {
        state_db
            .thread_schedules()
            .complete_thread_schedule_run(schedule_id, run_id, lease_id, completed_at, next_run_at)
            .await?
    };
    if !updated {
        return Ok(None);
    }
    let schedule = state_db
        .thread_schedules()
        .get_thread_schedule(schedule_id)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!("schedule {schedule_id} disappeared after finishing run {run_id}")
        })?;
    let run = state_db
        .thread_schedules()
        .get_thread_schedule_run(run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("schedule run {run_id} disappeared after finish"))?;
    Ok(Some((schedule, run)))
}

async fn apply_persisted_schedule_resume_metadata(
    state_db: &StateDbHandle,
    thread_id: ThreadId,
    schedule_auth_profile: Option<Option<String>>,
    initial_history: &InitialHistory,
    request_overrides: &mut Option<HashMap<String, serde_json::Value>>,
    typesafe_overrides: &mut ConfigOverrides,
) {
    if typesafe_overrides.auth_profile.is_none()
        && let Some(auth_profile) =
            schedule_resume_auth_profile(schedule_auth_profile, initial_history)
    {
        typesafe_overrides.auth_profile = Some(auth_profile);
    }
    super::thread_processor::merge_persisted_auth_profile_from_history(
        typesafe_overrides,
        initial_history,
    );

    let persisted_metadata = match state_db.get_thread(thread_id).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return,
        Err(err) => {
            warn!("failed to read persisted metadata for scheduled thread {thread_id}: {err}");
            return;
        }
    };
    typesafe_overrides.model = persisted_metadata.model.clone();
    typesafe_overrides.model_provider = Some(persisted_metadata.model_provider.clone());
    if let Some(reasoning_effort) = persisted_metadata.reasoning_effort.as_ref() {
        request_overrides.get_or_insert_with(HashMap::new).insert(
            "model_reasoning_effort".to_string(),
            serde_json::Value::String(reasoning_effort.to_string()),
        );
    }

    let latest_cwd_from_items = |items: &[RolloutItem]| {
        items.iter().rev().find_map(|item| match item {
            RolloutItem::TurnContext(turn_context) if !turn_context.cwd.as_os_str().is_empty() => {
                Some(turn_context.cwd.clone())
            }
            RolloutItem::SessionMeta(meta_line) if !meta_line.meta.cwd.as_os_str().is_empty() => {
                Some(meta_line.meta.cwd.clone())
            }
            RolloutItem::ResponseItem(_)
            | RolloutItem::Compacted(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::SessionMeta(_) => None,
        })
    };
    let fallback_cwd = match initial_history {
        InitialHistory::New | InitialHistory::Cleared => None,
        InitialHistory::Resumed(resumed) => latest_cwd_from_items(&resumed.history),
        InitialHistory::Forked(items) => latest_cwd_from_items(items),
    };
    let persisted_settings = persisted_schedule_thread_settings_from_metadata(
        &persisted_metadata,
        fallback_cwd.as_deref(),
    );

    // Restore the thread's command permissions so an unattended scheduled run
    // is gated by the same human-in-the-loop and sandbox controls the thread
    // was last run with, not the app-server's launch-time defaults.
    if typesafe_overrides.approval_policy.is_none() {
        typesafe_overrides.approval_policy = persisted_settings.approval_policy;
    }
    if typesafe_overrides.permission_profile.is_none() {
        typesafe_overrides.permission_profile = persisted_settings.permission_profile;
    }
}

fn scheduled_thread_settings_from_snapshot(
    snapshot: ThreadConfigSnapshot,
    auth_profile: Option<Option<String>>,
) -> codex_protocol::protocol::ThreadSettingsOverrides {
    codex_protocol::protocol::ThreadSettingsOverrides {
        cwd: Some(snapshot.cwd),
        workspace_roots: Some(snapshot.workspace_roots),
        profile_workspace_roots: Some(snapshot.profile_workspace_roots),
        approval_policy: Some(snapshot.approval_policy),
        approvals_reviewer: Some(snapshot.approvals_reviewer),
        permission_profile: Some(snapshot.permission_profile),
        active_permission_profile: snapshot.active_permission_profile,
        auth_profile,
        ..codex_protocol::protocol::ThreadSettingsOverrides::default()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PersistedScheduleThreadSettings {
    approval_policy: Option<codex_protocol::protocol::AskForApproval>,
    permission_profile: Option<codex_protocol::models::PermissionProfile>,
}

fn persisted_schedule_thread_settings_from_metadata(
    metadata: &codex_state::ThreadMetadata,
    fallback_cwd: Option<&Path>,
) -> PersistedScheduleThreadSettings {
    let permission_cwd = fallback_cwd.unwrap_or_else(|| {
        if metadata.cwd.as_os_str().is_empty() {
            Path::new(".")
        } else {
            metadata.cwd.as_path()
        }
    });
    PersistedScheduleThreadSettings {
        approval_policy: parse_persisted_approval_mode(&metadata.approval_mode),
        permission_profile: parse_persisted_permission_profile(
            &metadata.sandbox_policy,
            permission_cwd,
        ),
    }
}

fn parse_persisted_permission_profile(
    stored: &str,
    cwd: &Path,
) -> Option<codex_protocol::models::PermissionProfile> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(permission_profile) =
        serde_json::from_str::<codex_protocol::models::PermissionProfile>(trimmed)
    {
        return Some(permission_profile);
    }
    if let Ok(sandbox_policy) =
        serde_json::from_str::<codex_protocol::protocol::SandboxPolicy>(trimmed)
    {
        return Some(
            codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &sandbox_policy,
                cwd,
            ),
        );
    }
    let owned_bare;
    let bare = match serde_json::from_str::<String>(trimmed) {
        Ok(value) => {
            owned_bare = value;
            owned_bare.as_str()
        }
        Err(_) => trimmed,
    };
    let sandbox_policy = match bare {
        "danger-full-access" => Some(codex_protocol::protocol::SandboxPolicy::DangerFullAccess),
        "external-sandbox" => Some(codex_protocol::protocol::SandboxPolicy::ExternalSandbox {
            network_access: codex_protocol::protocol::NetworkAccess::Restricted,
        }),
        "read-only" => Some(codex_protocol::protocol::SandboxPolicy::new_read_only_policy()),
        "workspace-write" => Some(codex_protocol::protocol::SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }),
        _ => None,
    }?;
    Some(
        codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy_for_cwd(
            &sandbox_policy,
            cwd,
        ),
    )
}

fn schedule_resume_auth_profile(
    schedule_auth_profile: Option<Option<String>>,
    initial_history: &InitialHistory,
) -> Option<Option<String>> {
    if schedule_auth_profile.is_some() {
        return schedule_auth_profile;
    }

    let history_auth_profile = initial_history.get_auth_profile();
    if matches!(history_auth_profile, Some(None))
        && let Some(Some(auth_profile)) = initial_session_auth_profile(initial_history)
    {
        return Some(Some(auth_profile));
    }
    history_auth_profile
}

fn initial_session_auth_profile(initial_history: &InitialHistory) -> Option<Option<String>> {
    match initial_history {
        InitialHistory::New | InitialHistory::Cleared => None,
        InitialHistory::Resumed(resumed) => resumed.history.iter().find_map(|item| match item {
            RolloutItem::SessionMeta(meta_line) if meta_line.meta.id == resumed.conversation_id => {
                meta_line.meta.auth_profile.clone()
            }
            _ => None,
        }),
        InitialHistory::Forked(items) => items.iter().find_map(|item| match item {
            RolloutItem::SessionMeta(meta_line) => meta_line.meta.auth_profile.clone(),
            _ => None,
        }),
    }
}

/// Parse the stringified approval mode persisted in thread metadata back into an
/// [`AskForApproval`]. The value is written via `enum_to_string` (a bare serde
/// string such as `"on-request"`); parsing is best-effort and tolerates
/// unknown/legacy formats by returning `None` (leaving the loaded default).
fn parse_persisted_approval_mode(stored: &str) -> Option<codex_protocol::protocol::AskForApproval> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_value(serde_json::Value::String(trimmed.to_string())).ok()
}

fn parse_schedule_timezone(timezone: &str) -> anyhow::Result<Tz> {
    timezone
        .parse::<Tz>()
        .map_err(|err| anyhow::anyhow!("invalid schedule timezone `{timezone}`: {err}"))
}

fn schedule_run_error(error: String) -> String {
    truncate_schedule_run_error(redact_schedule_run_error(&error))
}

fn truncate_schedule_run_error(error: String) -> String {
    if error.chars().count() <= MAX_SCHEDULE_RUN_ERROR_CHARS {
        return error;
    }
    error
        .chars()
        .take(MAX_SCHEDULE_RUN_ERROR_CHARS.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

fn redact_schedule_run_error(error: &str) -> String {
    let mut redacted = String::new();
    let mut redact_next = false;
    for segment in error.split_inclusive(char::is_whitespace) {
        let word_len = segment.trim_end_matches(char::is_whitespace).len();
        let (word, whitespace) = segment.split_at(word_len);
        let (word, should_redact_next) = redact_schedule_run_error_word(word, redact_next);
        redacted.push_str(word.as_str());
        redacted.push_str(whitespace);
        redact_next = should_redact_next;
    }
    redacted
}

fn redact_schedule_run_error_word(word: &str, force_redact: bool) -> (String, bool) {
    if word.is_empty() {
        return (String::new(), force_redact);
    }
    if force_redact {
        return ("[redacted]".to_string(), false);
    }
    if word.eq_ignore_ascii_case("bearer") {
        return (word.to_string(), true);
    }
    if let Some(redacted) = redact_inline_secret_assignment(word) {
        return redacted;
    }
    if looks_like_standalone_secret(word) {
        return ("[redacted]".to_string(), false);
    }
    (word.to_string(), false)
}

fn redact_inline_secret_assignment(word: &str) -> Option<(String, bool)> {
    for delimiter in ['=', ':'] {
        let Some(delimiter_index) = word.find(delimiter) else {
            continue;
        };
        let key = &word[..delimiter_index];
        if !is_sensitive_error_key(key) {
            continue;
        }
        let prefix_end = delimiter_index + delimiter.len_utf8();
        let prefix = &word[..prefix_end];
        if word[prefix_end..].is_empty() {
            return Some((word.to_string(), true));
        }
        return Some((format!("{prefix}[redacted]"), false));
    }
    None
}

fn is_sensitive_error_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .flat_map(char::to_lowercase)
        .collect();
    [
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "auth_token",
        "token",
        "secret",
        "password",
        "passwd",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn looks_like_standalone_secret(word: &str) -> bool {
    let trimmed = word.trim_matches(|ch: char| {
        matches!(
            ch,
            '"' | '\'' | '`' | ',' | ';' | '.' | ')' | '(' | '[' | ']' | '{' | '}'
        )
    });
    // Known credential prefixes. Keep in sync with the repo's pre-commit secret
    // scan so non-`sk-` keys (GitHub, AWS, Google, xAI, Slack, …) are also
    // redacted from persisted run errors and notifications, not just OpenAI keys.
    const SECRET_PREFIXES: &[&str] = &[
        "sk-",         // OpenAI / Anthropic (sk-ant-) / generic
        "rk-",         // OpenAI restricted key
        "xai-",        // xAI
        "AIza",        // Google API keys
        "AKIA",        // AWS access key id
        "ASIA",        // AWS temporary access key id
        "ghp_",        // GitHub personal access token
        "gho_",        // GitHub OAuth token
        "ghu_",        // GitHub user-to-server token
        "ghs_",        // GitHub server-to-server token
        "ghr_",        // GitHub refresh token
        "github_pat_", // GitHub fine-grained PAT
        "glpat-",      // GitLab PAT
        "npm_",        // npm token
        "ctx7sk-",     // Context7
        "xoxb-",       // Slack bot token
        "xoxp-",       // Slack user token
        "hf_",         // Hugging Face
    ];
    if SECRET_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }

    if trimmed.len() < 12 {
        return false;
    }

    // JSON Web Tokens (e.g. bearer tokens) — three base64url segments.
    looks_like_jwt(trimmed)
}

/// Heuristic JWT detector: a `header.payload.signature` triple whose header
/// segment is base64url-encoded JSON (begins with `eyJ`).
fn looks_like_jwt(value: &str) -> bool {
    let mut segments = value.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    let is_base64url = |segment: &str| {
        segment.len() >= 2
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    };
    header.starts_with("eyJ")
        && is_base64url(header)
        && is_base64url(payload)
        && is_base64url(signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::protocol::AskForApproval;
    use codex_protocol::protocol::SandboxPolicy;
    use codex_protocol::protocol::SessionMeta;
    use codex_protocol::protocol::SessionMetaLine;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::protocol::TurnCompleteEvent;
    use codex_protocol::protocol::TurnContextItem;
    use codex_state::ThreadMetadataBuilder;
    use pretty_assertions::assert_eq;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    fn resumed_history_with_auth_profile(
        thread_id: ThreadId,
        auth_profile: Option<Option<&str>>,
    ) -> InitialHistory {
        InitialHistory::Resumed(ResumedHistory {
            conversation_id: thread_id,
            history: vec![RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    auth_profile: auth_profile.map(|profile| profile.map(str::to_string)),
                    ..SessionMeta::default()
                },
                git: None,
            })],
            rollout_path: None,
        })
    }

    fn resumed_history_with_session_and_turn_auth_profile(
        thread_id: ThreadId,
        session_auth_profile: Option<Option<&str>>,
        turn_auth_profile: Option<Option<&str>>,
    ) -> InitialHistory {
        InitialHistory::Resumed(ResumedHistory {
            conversation_id: thread_id,
            history: vec![
                RolloutItem::SessionMeta(SessionMetaLine {
                    meta: SessionMeta {
                        id: thread_id,
                        auth_profile: session_auth_profile
                            .map(|profile| profile.map(str::to_string)),
                        ..SessionMeta::default()
                    },
                    git: None,
                }),
                RolloutItem::TurnContext(TurnContextItem {
                    thread_id: Some(thread_id),
                    turn_id: Some("turn-1".to_string()),
                    cwd: PathBuf::from("/tmp"),
                    workspace_roots: None,
                    current_date: None,
                    timezone: None,
                    approval_policy: AskForApproval::OnRequest,
                    sandbox_policy: SandboxPolicy::DangerFullAccess,
                    permission_profile: None,
                    network: None,
                    file_system_sandbox_policy: None,
                    model: "gpt-5.5".to_string(),
                    model_provider_id: None,
                    personality: None,
                    collaboration_mode: None,
                    multi_agent_version: None,
                    auth_profile: turn_auth_profile.map(|profile| profile.map(str::to_string)),
                    realtime_active: None,
                    effort: None,
                    summary: codex_protocol::config_types::ReasoningSummary::Auto,
                }),
            ],
            rollout_path: None,
        })
    }

    #[test]
    fn schedule_failure_backoff_grows_and_caps() {
        let completed = at(1_000_000);
        let natural = at(1_000_060); // natural cadence: 60s after completion.

        // Early failures: 30s/60s backoff is still earlier than the natural
        // 60s cadence, so the natural next run wins.
        assert_eq!(
            natural,
            schedule_failure_backoff_run_at(natural, completed, 1)
        );
        assert_eq!(
            natural,
            schedule_failure_backoff_run_at(natural, completed, 2)
        );

        // A longer streak backs off past the natural cadence...
        assert_eq!(
            completed + chrono::Duration::seconds(240),
            schedule_failure_backoff_run_at(natural, completed, 4)
        );
        // ...and caps at one hour regardless of how long the streak is.
        assert_eq!(
            completed + chrono::Duration::seconds(3600),
            schedule_failure_backoff_run_at(natural, completed, 50)
        );
    }

    #[test]
    fn parse_persisted_approval_mode_round_trips_thread_metadata_format() {
        use codex_protocol::protocol::AskForApproval;
        // Mirror how thread metadata stores the approval mode (enum_to_string ==
        // bare serde string) and confirm it parses back to the same policy so a
        // scheduled run is gated by the thread's approval policy, not the
        // server default.
        for mode in [
            AskForApproval::OnRequest,
            AskForApproval::Never,
            AskForApproval::OnFailure,
            AskForApproval::UnlessTrusted,
        ] {
            let stored = match serde_json::to_value(mode).expect("serialize approval mode") {
                serde_json::Value::String(value) => value,
                other => other.to_string(),
            };
            assert_eq!(Some(mode), parse_persisted_approval_mode(&stored));
        }

        assert_eq!(None, parse_persisted_approval_mode(""));
        assert_eq!(None, parse_persisted_approval_mode("not-a-real-mode"));
    }

    #[test]
    fn parse_persisted_permission_profile_accepts_current_metadata_format() {
        let profile = PermissionProfile::Disabled;
        let stored = serde_json::to_string(&profile).expect("serialize permission profile");

        assert_eq!(
            Some(profile),
            parse_persisted_permission_profile(&stored, Path::new("/workspace"))
        );
    }

    #[test]
    fn parse_persisted_permission_profile_accepts_legacy_sandbox_policy_metadata() {
        let sandbox_policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let cwd = Path::new("/workspace");
        let stored = serde_json::to_string(&sandbox_policy).expect("serialize sandbox policy");

        assert_eq!(
            Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &sandbox_policy,
                cwd
            )),
            parse_persisted_permission_profile(&stored, cwd)
        );
    }

    #[test]
    fn parse_persisted_permission_profile_accepts_legacy_bare_sandbox_policy_metadata() {
        let cwd = Path::new("/workspace");
        let workspace_write = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };

        for (stored, expected) in [
            ("danger-full-access", SandboxPolicy::DangerFullAccess),
            ("read-only", SandboxPolicy::new_read_only_policy()),
            ("workspace-write", workspace_write),
        ] {
            assert_eq!(
                Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                    &expected, cwd
                )),
                parse_persisted_permission_profile(stored, cwd)
            );
        }

        assert_eq!(
            Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &SandboxPolicy::new_read_only_policy(),
                cwd
            )),
            parse_persisted_permission_profile("\"read-only\"", cwd)
        );
    }

    #[test]
    fn scheduled_thread_settings_preserve_live_permission_snapshot() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let cwd = AbsolutePathBuf::from_absolute_path(temp_dir.path().join("workspace"))
            .expect("absolute cwd");
        let workspace_root = AbsolutePathBuf::from_absolute_path(temp_dir.path().join("root"))
            .expect("absolute workspace root");
        let profile_root = AbsolutePathBuf::from_absolute_path(temp_dir.path().join("profile"))
            .expect("absolute profile root");
        let permission_profile = PermissionProfile::Disabled;
        let active_permission_profile = Some(codex_protocol::models::ActivePermissionProfile::new(
            codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS,
        ));

        let settings = scheduled_thread_settings_from_snapshot(
            ThreadConfigSnapshot {
                model: "gpt-5".to_string(),
                model_provider_id: "openai".to_string(),
                service_tier: None,
                approval_policy: AskForApproval::Never,
                approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer::User,
                permission_profile: permission_profile.clone(),
                active_permission_profile: active_permission_profile.clone(),
                auth_profile: Some("work".to_string()),
                cwd: cwd.clone(),
                workspace_roots: vec![workspace_root.clone()],
                profile_workspace_roots: vec![profile_root.clone()],
                ephemeral: false,
                reasoning_effort: None,
                reasoning_summary: None,
                personality: None,
                collaboration_mode: codex_protocol::config_types::CollaborationMode {
                    mode: codex_protocol::config_types::ModeKind::Default,
                    settings: codex_protocol::config_types::Settings {
                        model: "gpt-5".to_string(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                },
                selected_auth_profile: Some("work".to_string()),
                session_source: SessionSource::Cli,
                forked_from_thread_id: None,
                parent_thread_id: None,
                thread_source: None,
            },
            Some(Some("schedule-work".to_string())),
        );

        assert_eq!(
            codex_protocol::protocol::ThreadSettingsOverrides {
                cwd: Some(cwd),
                workspace_roots: Some(vec![workspace_root]),
                profile_workspace_roots: Some(vec![profile_root]),
                approval_policy: Some(AskForApproval::Never),
                approvals_reviewer: Some(codex_protocol::config_types::ApprovalsReviewer::User),
                permission_profile: Some(permission_profile),
                active_permission_profile,
                auth_profile: Some(Some("schedule-work".to_string())),
                ..codex_protocol::protocol::ThreadSettingsOverrides::default()
            },
            settings
        );
    }

    #[tokio::test]
    async fn scheduled_resume_metadata_restores_auth_profile_and_permissions_from_metadata() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            temp_dir.path().join("thread.jsonl"),
            at(1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        builder.model_provider = Some("openai".to_string());
        builder.approval_mode = codex_protocol::protocol::AskForApproval::Never;
        let mut metadata = builder.build("fallback-provider");
        metadata.model = Some("gpt-5.5".to_string());
        metadata.sandbox_policy =
            serde_json::to_string(&PermissionProfile::Disabled).expect("serialize permissions");
        state_db
            .upsert_thread(&metadata)
            .await
            .expect("thread metadata should persist");

        let history = resumed_history_with_auth_profile(thread_id, Some(Some("work")));
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides::default();
        apply_persisted_schedule_resume_metadata(
            &state_db,
            thread_id,
            None,
            &history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;

        assert_eq!(
            Some(Some("work".to_string())),
            typesafe_overrides.auth_profile
        );
        assert_eq!(Some("gpt-5.5".to_string()), typesafe_overrides.model);
        assert_eq!(
            Some("openai".to_string()),
            typesafe_overrides.model_provider
        );
        assert_eq!(
            Some(codex_protocol::protocol::AskForApproval::Never),
            typesafe_overrides.approval_policy
        );
        assert_eq!(
            Some(PermissionProfile::Disabled),
            typesafe_overrides.permission_profile
        );
    }

    #[tokio::test]
    async fn scheduled_resume_metadata_restores_legacy_sandbox_policy_with_latest_history_cwd() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let sandbox_policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            temp_dir.path().join("thread.jsonl"),
            at(1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = PathBuf::from("/old-workspace");
        builder.model_provider = Some("openai".to_string());
        let mut metadata = builder.build("fallback-provider");
        metadata.sandbox_policy = "workspace-write".to_string();
        state_db
            .upsert_thread(&metadata)
            .await
            .expect("thread metadata should persist");

        let latest_cwd = PathBuf::from("/latest-workspace");
        let history = InitialHistory::Resumed(ResumedHistory {
            conversation_id: thread_id,
            history: vec![
                RolloutItem::SessionMeta(SessionMetaLine {
                    meta: SessionMeta {
                        id: thread_id,
                        cwd: PathBuf::from("/session-workspace"),
                        ..SessionMeta::default()
                    },
                    git: None,
                }),
                RolloutItem::TurnContext(TurnContextItem {
                    thread_id: Some(thread_id),
                    turn_id: Some("turn-latest".to_string()),
                    cwd: latest_cwd.clone(),
                    workspace_roots: None,
                    current_date: None,
                    timezone: None,
                    approval_policy: AskForApproval::OnRequest,
                    sandbox_policy: SandboxPolicy::DangerFullAccess,
                    permission_profile: None,
                    network: None,
                    file_system_sandbox_policy: None,
                    model: "gpt-5.5".to_string(),
                    model_provider_id: None,
                    personality: None,
                    collaboration_mode: None,
                    multi_agent_version: None,
                    auth_profile: None,
                    realtime_active: None,
                    effort: None,
                    summary: codex_protocol::config_types::ReasoningSummary::Auto,
                }),
            ],
            rollout_path: None,
        });
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides::default();
        apply_persisted_schedule_resume_metadata(
            &state_db,
            thread_id,
            None,
            &history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;

        assert_eq!(
            Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &sandbox_policy,
                latest_cwd.as_path()
            )),
            typesafe_overrides.permission_profile
        );
    }

    #[tokio::test]
    async fn scheduled_resume_metadata_preserves_explicit_auth_profile_override() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let history = resumed_history_with_auth_profile(thread_id, Some(Some("work")));
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides {
            auth_profile: Some(None),
            ..ConfigOverrides::default()
        };

        apply_persisted_schedule_resume_metadata(
            &state_db,
            thread_id,
            None,
            &history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;

        assert_eq!(Some(None), typesafe_overrides.auth_profile);
    }

    #[tokio::test]
    async fn scheduled_resume_metadata_uses_schedule_auth_profile_over_history() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let history = resumed_history_with_auth_profile(thread_id, Some(None));
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides::default();

        apply_persisted_schedule_resume_metadata(
            &state_db,
            thread_id,
            Some(Some("account002".to_string())),
            &history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;

        assert_eq!(
            Some(Some("account002".to_string())),
            typesafe_overrides.auth_profile
        );
    }

    #[tokio::test]
    async fn legacy_scheduled_resume_metadata_prefers_session_auth_profile_over_root_turn() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let history = resumed_history_with_session_and_turn_auth_profile(
            thread_id,
            Some(Some("account002")),
            Some(None),
        );
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides::default();

        apply_persisted_schedule_resume_metadata(
            &state_db,
            thread_id,
            None,
            &history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;

        assert_eq!(
            Some(Some("account002".to_string())),
            typesafe_overrides.auth_profile
        );
    }

    #[test]
    fn computes_interval_next_run() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_000_300)),
            next_thread_schedule_run_at(
                &codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                    amount: 5,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                }),
                "UTC",
                at(/*seconds*/ 1_700_000_000),
            )
            .expect("next interval should compute")
        );
    }

    #[test]
    fn computes_once_next_run() {
        assert_eq!(
            None,
            next_thread_schedule_run_at(
                &codex_state::ThreadScheduleSpec::Once,
                "UTC",
                at(/*seconds*/ 1_700_000_000),
            )
            .expect("one-time schedule should not compute a follow-up run")
        );
    }

    #[test]
    fn computes_cron_next_run_in_timezone() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_031_600)),
            next_thread_schedule_run_at(
                &codex_state::ThreadScheduleSpec::Cron {
                    expression: "0 9 * * *".to_string(),
                },
                "Europe/Bucharest",
                at(/*seconds*/ 1_700_000_000),
            )
            .expect("next cron run should compute")
        );
    }

    #[test]
    fn rejects_unknown_timezone() {
        assert!(normalize_schedule_timezone("Nope/Nowhere").is_err());
    }

    #[test]
    fn scheduled_thread_prompt_tells_model_not_to_wait() {
        let prompt = scheduled_thread_prompt(
            "ask me a funny question every minute",
            "run-123",
            Some(at(1_700_000_000)),
        );

        assert!(prompt.contains("one new scheduled Codewith prompt"));
        assert!(prompt.contains("Run id: run-123"));
        assert!(prompt.contains("Scheduled for: 2023-11-14T22:13:20+00:00"));
        assert!(prompt.contains("This is a distinct run"));
        assert!(prompt.contains("Produce exactly one visible final response"));
        assert!(prompt.contains("Do not wait, sleep, start a timer"));
        assert!(prompt.contains("not as an instruction to implement the cadence yourself"));
        assert!(prompt.ends_with("ask me a funny question every minute"));
    }

    #[test]
    fn scheduled_goal_objective_accepts_explicit_goal_command() {
        assert_eq!(
            Some("improve benchmark coverage"),
            scheduled_goal_objective("  /goal improve benchmark coverage")
        );
        assert_eq!(
            Some("ship the release notes"),
            scheduled_goal_objective("/goal\n\nship the release notes")
        );
    }

    #[test]
    fn scheduled_goal_objective_requires_goal_command_boundary() {
        assert_eq!(None, scheduled_goal_objective("please run /goal later"));
        assert_eq!(None, scheduled_goal_objective("/goalkeeper report"));
        assert_eq!(Some(""), scheduled_goal_objective("/goal"));
    }

    #[test]
    fn scheduled_goal_thread_prompt_tells_model_not_to_spawn_followups() {
        let prompt = scheduled_goal_thread_prompt(
            "finish release readiness checks every hour",
            "run-123",
            Some(at(1_700_000_000)),
        );

        assert!(prompt.contains("one new scheduled Codewith goal objective"));
        assert!(prompt.contains("Run id: run-123"));
        assert!(prompt.contains("Scheduled for: 2023-11-14T22:13:20+00:00"));
        assert!(prompt.contains("active thread goal has already been persisted"));
        assert!(prompt.contains("Produce exactly one visible final response"));
        assert!(prompt.contains("Do not create new goals, loops, schedules"));
        assert!(prompt.contains("not as an instruction to implement the cadence yourself"));
        assert!(prompt.ends_with("finish release readiness checks every hour"));
        assert!(!prompt.contains("/goal finish release readiness checks"));
    }

    #[test]
    fn scheduled_turn_without_agent_message_fails() {
        let finish = scheduled_turn_finish(&EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: "turn-1".to_string(),
            last_agent_message: None,
            completed_at: None,
            duration_ms: None,
            time_to_first_token_ms: None,
        }));

        assert_eq!(
            Some(ScheduledTurnFinish::Failed(
                "scheduled turn completed without a final assistant message".to_string()
            )),
            finish
        );
    }

    #[test]
    fn scheduled_turn_with_agent_message_completes() {
        let finish = scheduled_turn_finish(&EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: "turn-1".to_string(),
            last_agent_message: Some("done".to_string()),
            completed_at: None,
            duration_ms: None,
            time_to_first_token_ms: None,
        }));

        assert_eq!(Some(ScheduledTurnFinish::Complete), finish);
    }

    #[test]
    fn redacts_sensitive_schedule_run_error_values() {
        let sanitized = schedule_run_error(
            "failed with OPENAI_API_KEY=sk-test-secret token: plain-secret Bearer sk-bearer-secret"
                .to_string(),
        );

        assert_eq!(
            "failed with OPENAI_API_KEY=[redacted] token: [redacted] Bearer [redacted]",
            sanitized
        );
        assert!(!sanitized.contains("sk-test-secret"));
        assert!(!sanitized.contains("plain-secret"));
        assert!(!sanitized.contains("sk-bearer-secret"));
    }

    #[test]
    fn redacts_short_prefixed_api_keys_from_schedule_run_errors() {
        let sanitized = schedule_run_error(
            "unexpected status 401 Unauthorized: Incorrect API key provided: sk-work.".to_string(),
        );

        assert!(sanitized.contains("[redacted]"));
        assert!(!sanitized.contains("sk-work"));
    }

    #[test]
    fn redacts_non_openai_credential_shapes() {
        // Standalone credentials that are NOT `sk-` prefixed must still be
        // stripped from persisted run errors and notifications.
        for secret in [
            "ghp_0123456789abcdefghijABCDEFGHIJ01",
            "github_pat_11ABCDEFG0abcdefghijKLMNOP",
            "AKIAIOSFODNN7EXAMPLE",
            "AIzaSyA1234567890abcdefghijklmnopqrs",
            "xai-abcdef0123456789abcdef0123",
            "glpat-abcdefghij0123456789",
            "xoxb-1234567890-abcdefghijkl",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9P",
        ] {
            let sanitized = schedule_run_error(format!("run failed near {secret} end"));
            assert!(
                !sanitized.contains(secret),
                "expected `{secret}` to be redacted, got: {sanitized}"
            );
        }
    }

    #[test]
    fn truncates_schedule_run_error_values() {
        let sanitized = schedule_run_error("x".repeat(MAX_SCHEDULE_RUN_ERROR_CHARS + 8));

        assert_eq!(MAX_SCHEDULE_RUN_ERROR_CHARS, sanitized.chars().count());
        assert!(sanitized.ends_with("..."));
    }
}
