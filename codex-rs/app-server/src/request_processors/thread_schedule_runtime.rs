use super::*;
use crate::request_processors::thread_schedule_api::api_thread_schedule_from_state;
use crate::request_processors::thread_schedule_api::api_thread_schedule_run_from_state;
use anyhow::Context as _;
use chrono_tz::Tz;
use codex_goal_extension::GoalObjectiveUpdate;
use codex_goal_extension::GoalService;
use codex_goal_extension::GoalSetRequest;
use codex_goal_extension::GoalTitleUpdate;
use codex_goal_extension::GoalTokenBudgetUpdate;
use codex_protocol::protocol::CodexErrorInfo as CoreCodexErrorInfo;
use croner::Cron;
use std::fmt::Write as _;
use std::str::FromStr;

const SCHEDULE_POLL_INTERVAL: Duration = Duration::from_secs(10);
const SCHEDULE_LEASE_DURATION: Duration = Duration::from_secs(10 * 60);
const SCHEDULE_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAX_SCHEDULE_CLAIMS_PER_TICK: usize = 16;
const DEFAULT_DYNAMIC_INTERVAL_MINUTES: i64 = 1;
const DEFAULT_SCHEDULE_EXPIRATION_DAYS: i64 = 7;
const MAX_SCHEDULE_RUN_ERROR_CHARS: usize = 1_000;
const SCHEDULE_IDLE_RETRY_DELAY_SECONDS: i64 = 30;
const SCHEDULE_LOCAL_ACTIVE_SESSION_STALE_AFTER: Duration = Duration::from_secs(15);

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
    local_active_owner_id: String,
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
        local_active_owner_id: String,
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
            local_active_owner_id,
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

    pub(crate) fn local_active_owner_id(&self) -> &str {
        self.local_active_owner_id.as_str()
    }

    pub(crate) fn local_active_fresh_after(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        schedule_local_active_fresh_after(now)
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
        let local_active_fresh_after = schedule_local_active_fresh_after(now);
        for _ in 0..MAX_SCHEDULE_CLAIMS_PER_TICK {
            let lease_id = Uuid::new_v4().to_string();
            let claim = match state_db
                .thread_schedules()
                .claim_due_thread_schedule_with_params(codex_state::ThreadScheduleDueClaimParams {
                    now,
                    lease_id: lease_id.as_str(),
                    lease_duration: SCHEDULE_LEASE_DURATION,
                    local_active_owner_id: Some(self.local_active_owner_id.as_str()),
                    local_active_fresh_after: Some(local_active_fresh_after),
                })
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
            if let Some(wait) = err.downcast_ref::<ScheduleUsageProfileWait>() {
                self.defer_claimed_run_for_usage_profile_wait(state_db, claim, wait.clone())
                    .await;
                return;
            }
            if let Some(deferral) = err.downcast_ref::<ScheduleRunDeferral>() {
                self.defer_claimed_run(state_db, claim, deferral.clone())
                    .await;
                return;
            }
            self.fail_claimed_run_after_submit_error(state_db, claim, schedule_submit_error(&err))
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
        let broker_decision = super::usage_profile_broker::resolve_dispatch_auth_profile(
            &self.auth_manager,
            &self.config,
            claim_auth_profile.clone(),
        )
        .await;
        let claim_auth_profile = match schedule_auth_profile_after_broker_decision(
            claim_auth_profile,
            broker_decision,
            self.config.usage_self_heal.reset_retry_buffer_secs,
            Utc::now(),
        ) {
            Ok(resolved) => resolved,
            Err(wait) => return Err(anyhow::Error::new(wait)),
        };
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
                &claim.schedule,
            )
        } else {
            scheduled_thread_prompt(
                &prompt,
                &claim.schedule,
                claim.run.run_id.as_str(),
                claim.run.scheduled_for,
            )
        };
        let thread_settings = scheduled_thread_settings_from_snapshot(
            thread.config_snapshot().await,
            claim_auth_profile,
        );
        let turn_id = Uuid::now_v7().to_string();

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
                return Err(anyhow::anyhow!(
                    "claimed schedule run {} disappeared before it could start",
                    claim.run.run_id
                ));
            }
            Err(err) => return Err(err),
        };
        {
            let mut thread_state = thread_state.lock().await;
            thread_state.track_scheduled_run(
                turn_id.clone(),
                crate::thread_state::ScheduledThreadScheduleRun {
                    schedule_id: claim.schedule.schedule_id.clone(),
                    run_id: claim.run.run_id.clone(),
                    lease_id: claim.run.lease_id.clone(),
                    state_db: state_db.clone(),
                },
            );
        }
        let start_result = thread
            .try_start_user_input_turn_if_idle(
                turn_id.clone(),
                vec![CoreInputItem::Text {
                    text: turn_prompt,
                    text_elements: Vec::new(),
                }],
                Default::default(),
                thread_settings,
            )
            .await;
        if let Err(err) = start_result {
            thread_state
                .lock()
                .await
                .take_scheduled_run(turn_id.as_str());
            if let Some(deferral) = schedule_deferral_for_idle_rejection(&err, Utc::now()) {
                return Err(anyhow::Error::new(deferral));
            }
            return Err(anyhow::anyhow!("failed to start scheduled prompt: {err}"));
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
                    title: GoalTitleUpdate::Keep,
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
            .with_context(|| {
                format!(
                    "failed to load rollout {} for scheduled task",
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
            .context("failed to load config for scheduled task")?;
        self.thread_manager
            .resume_thread_with_history(
                config,
                initial_history,
                Arc::clone(&self.auth_manager),
                /*parent_trace*/ None,
            )
            .await
            .map(|new_thread| new_thread.thread)
            .context("failed to resume scheduled task thread")
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
            state_db: self.state_db.clone(),
            local_active_owner_id: self.local_active_owner_id.clone(),
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

    async fn defer_claimed_run_for_usage_profile_wait(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
        wait: ScheduleUsageProfileWait,
    ) {
        self.defer_claimed_run(
            state_db,
            claim,
            ScheduleRunDeferral {
                retry_at: wait.retry_at,
                error: wait.to_string(),
            },
        )
        .await;
    }

    async fn defer_claimed_run(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
        deferral: ScheduleRunDeferral,
    ) {
        match defer_scheduled_run_state(&state_db, &claim, &deferral, Utc::now()).await {
            Ok(Some((schedule, run))) => {
                self.emit_schedule_updated(claim.schedule.thread_id, schedule)
                    .await;
                self.emit_schedule_run_updated(claim.schedule.thread_id, run)
                    .await;
            }
            Ok(None) => {}
            Err(err) => warn!(
                schedule_id = %claim.schedule.schedule_id,
                "failed to defer scheduled thread run: {err}"
            ),
        }
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
    schedule: &codex_state::ThreadSchedule,
    run_id: &str,
    scheduled_for: Option<DateTime<Utc>>,
) -> String {
    let scheduled_for = scheduled_for
        .map(|scheduled_for| scheduled_for.to_rfc3339())
        .unwrap_or_else(|| "immediate".to_string());
    let parent_schedule_id = schedule.parent_schedule_id.as_deref().unwrap_or("none");
    let can_be_nested_parent = matches!(
        schedule.schedule,
        codex_state::ThreadScheduleSpec::Dynamic | codex_state::ThreadScheduleSpec::Interval(_)
    );
    let nesting_guidance = if schedule.nesting_depth
        >= codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH
    {
        "This loop is already at the maximum nesting depth; do not create nested loops from this run.".to_string()
    } else if !can_be_nested_parent {
        "This schedule cannot be used as a nested-loop parent; do not create nested loops from this run.".to_string()
    } else {
        format!(
            "If the scheduled prompt explicitly asks for a nested loop, call manage_loop create with parent_schedule_id set to {}. Nested loops are limited to depth {}, must use dynamic or interval cadences, and the child cadence must be slower than the parent cadence.",
            schedule.schedule_id,
            codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH
        )
    };
    format!(
        "\
You are running one new scheduled Codewith prompt.

Loop schedule id: {}
Parent loop schedule id: {parent_schedule_id}
Loop nesting depth: {}/{}
Run id: {run_id}
Scheduled for: {scheduled_for}

This is a distinct run even if the scheduled prompt matches earlier runs. Execute only the scheduled prompt below for this run. Produce exactly one visible final response for this scheduled run, even if there are no changes, no action is needed, or the run is blocked. Do not wait, sleep, or start a timer; Codewith manages scheduling. If the scheduled prompt asks for durable follow-up work, use native create_goal or create_goal_plan goal tools; if it asks to extend an existing goal chain, use create_goal_plan with append_to_plan_id. Do not create follow-up schedules unless the scheduled prompt explicitly asks for nested scheduling. {nesting_guidance} If the prompt mentions a cadence like \"every minute\", treat that as schedule context, not as an instruction to implement the cadence yourself.

Scheduled prompt:
{prompt}",
        schedule.schedule_id,
        schedule.nesting_depth,
        codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH
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
    schedule: &codex_state::ThreadSchedule,
) -> String {
    let scheduled_for = scheduled_for
        .map(|scheduled_for| scheduled_for.to_rfc3339())
        .unwrap_or_else(|| "immediate".to_string());
    let nesting_guidance = scheduled_loop_nesting_guidance(schedule);
    format!(
        "\
You are running one new scheduled Codewith goal objective.

Run id: {run_id}
Scheduled for: {scheduled_for}
{nesting_guidance}

The active thread goal has already been persisted for this scheduled run. Work on only the goal objective below for this run. Produce exactly one visible final response for this scheduled run, even if there are no changes, no action is needed, or the run is blocked. Do not create new goals, schedules, monitors, timers, or follow-up runs unless this objective explicitly asks for a native child loop; Codewith manages scheduling. If the objective mentions a cadence like \"every minute\", treat that as schedule context, not as an instruction to implement the cadence yourself.

Goal objective:
{objective}"
    )
}

fn scheduled_loop_nesting_guidance(schedule: &codex_state::ThreadSchedule) -> String {
    let depth = schedule.nesting_depth;
    let schedule_id = schedule.schedule_id.as_str();
    let can_be_nested_parent = matches!(
        schedule.schedule,
        codex_state::ThreadScheduleSpec::Dynamic | codex_state::ThreadScheduleSpec::Interval(_)
    );
    if depth >= codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH {
        format!(
            "\nLoop schedule id: {schedule_id}\nLoop nesting: level {depth}/{}. This is the maximum nesting level; do not create nested child loops from this run.",
            codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH
        )
    } else if !can_be_nested_parent {
        format!(
            "\nLoop schedule id: {schedule_id}\nLoop nesting: level {depth}/{}. This schedule cannot be used as a nested-loop parent; do not create nested child loops from this run.",
            codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH
        )
    } else {
        format!(
            "\nLoop schedule id: {schedule_id}\nLoop nesting: level {depth}/{}. Create a child loop only if this scheduled prompt explicitly asks for one; pass {schedule_id} as parent_schedule_id and use a slower child cadence.",
            codex_state::MAX_THREAD_SCHEDULE_NESTING_DEPTH
        )
    }
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
            "scheduled turn completed without a final assistant message",
        ))),
        EventMsg::TurnAborted(aborted) => Some(ScheduledTurnFinish::Failed(schedule_run_error(
            format!("scheduled turn aborted: {:?}", aborted.reason),
        ))),
        EventMsg::Error(error) => Some(ScheduledTurnFinish::Failed(schedule_turn_event_error(
            error,
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

fn next_thread_schedule_run_after_completion(
    schedule: &codex_state::ThreadScheduleSpec,
    timezone: &str,
    scheduled_for: Option<DateTime<Utc>>,
    completed_at: DateTime<Utc>,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    let interval_duration = match schedule {
        codex_state::ThreadScheduleSpec::Dynamic => {
            Some(ChronoDuration::minutes(DEFAULT_DYNAMIC_INTERVAL_MINUTES))
        }
        codex_state::ThreadScheduleSpec::Interval(interval) => {
            let amount = interval.amount;
            Some(match interval.unit {
                codex_state::ThreadScheduleIntervalUnit::Minutes => ChronoDuration::minutes(amount),
                codex_state::ThreadScheduleIntervalUnit::Hours => ChronoDuration::hours(amount),
                codex_state::ThreadScheduleIntervalUnit::Days => ChronoDuration::days(amount),
            })
        }
        codex_state::ThreadScheduleSpec::Once | codex_state::ThreadScheduleSpec::Cron { .. } => {
            None
        }
    };

    if let (Some(interval_duration), Some(scheduled_for)) = (interval_duration, scheduled_for) {
        let Some(next_run_at) = scheduled_for.checked_add_signed(interval_duration) else {
            return Ok(None);
        };
        if next_run_at > completed_at {
            return Ok(Some(next_run_at));
        }
        let duration_ms = interval_duration.num_milliseconds();
        if duration_ms <= 0 {
            return Ok(None);
        }
        let elapsed_ms = completed_at
            .signed_duration_since(scheduled_for)
            .num_milliseconds();
        let periods_elapsed = elapsed_ms.div_euclid(duration_ms).saturating_add(1);
        return Ok(duration_ms
            .checked_mul(periods_elapsed)
            .and_then(|advance_ms| {
                scheduled_for.checked_add_signed(ChronoDuration::milliseconds(advance_ms))
            }));
    }

    next_thread_schedule_run_at(schedule, timezone, completed_at)
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
        (Some(_), Some(error)) => Some(schedule_turn_error(&error)),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduleUsageProfileWait {
    retry_at: DateTime<Utc>,
}

impl std::fmt::Display for ScheduleUsageProfileWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "all eligible auth profiles are exhausted; retrying scheduled run after {}",
            self.retry_at.to_rfc3339()
        )
    }
}

impl std::error::Error for ScheduleUsageProfileWait {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduleRunDeferral {
    retry_at: DateTime<Utc>,
    error: String,
}

impl std::fmt::Display for ScheduleRunDeferral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}; retrying scheduled run after {}",
            self.error,
            self.retry_at.to_rfc3339()
        )
    }
}

impl std::error::Error for ScheduleRunDeferral {}

fn schedule_deferral_for_idle_rejection(
    err: &codex_core::TryStartUserInputTurnIfIdleError,
    now: DateTime<Utc>,
) -> Option<ScheduleRunDeferral> {
    let reason = err.reason()?;
    let error = match reason {
        codex_core::TryStartTurnIfIdleRejectionReason::Busy => {
            "scheduled thread is busy".to_string()
        }
        codex_core::TryStartTurnIfIdleRejectionReason::PendingTriggerTurn => {
            "scheduled thread has pending mailbox trigger-turn work".to_string()
        }
        codex_core::TryStartTurnIfIdleRejectionReason::PlanMode => return None,
    };
    Some(ScheduleRunDeferral {
        retry_at: now + ChronoDuration::seconds(SCHEDULE_IDLE_RETRY_DELAY_SECONDS),
        error,
    })
}

fn schedule_auth_profile_after_broker_decision(
    current_auth_profile: Option<Option<String>>,
    decision: super::usage_profile_broker::UsageProfileBrokerDecision,
    reset_retry_buffer_secs: u64,
    now: DateTime<Utc>,
) -> Result<Option<Option<String>>, ScheduleUsageProfileWait> {
    if let Some(profile) = decision.selected_profile {
        return Ok(Some(Some(profile)));
    }
    if let Some(retry_at) = decision.retry_at
        && let Some(retry_at) =
            schedule_broker_retry_at_datetime(reset_retry_buffer_secs, retry_at, now)
    {
        return Err(ScheduleUsageProfileWait { retry_at });
    }
    Ok(current_auth_profile)
}

fn schedule_broker_retry_at_datetime(
    reset_retry_buffer_secs: u64,
    retry_at: i64,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let retry_at = DateTime::<Utc>::from_timestamp(retry_at, /*nsecs*/ 0)?;
    let buffer_secs = i64::try_from(reset_retry_buffer_secs).ok()?;
    let retry_at = retry_at + ChronoDuration::seconds(buffer_secs);
    (retry_at > now).then_some(retry_at)
}

fn schedule_local_active_fresh_after(now: DateTime<Utc>) -> DateTime<Utc> {
    now - ChronoDuration::seconds(duration_seconds_i64(
        SCHEDULE_LOCAL_ACTIVE_SESSION_STALE_AFTER,
    ))
}

fn duration_seconds_i64(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
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
    let scheduled_for = state_db
        .thread_schedules()
        .get_thread_schedule_run(run_id)
        .await?
        .and_then(|run| run.scheduled_for);
    let natural_next_run_at = next_thread_schedule_run_after_completion(
        &schedule.schedule,
        &schedule.timezone,
        scheduled_for,
        completed_at,
    )?;

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

#[cfg(test)]
async fn defer_scheduled_run_for_usage_profile_wait_state(
    state_db: &StateDbHandle,
    claim: &codex_state::ThreadScheduleClaim,
    wait: &ScheduleUsageProfileWait,
    completed_at: DateTime<Utc>,
) -> anyhow::Result<Option<(codex_state::ThreadSchedule, codex_state::ThreadScheduleRun)>> {
    defer_scheduled_run_state(
        state_db,
        claim,
        &ScheduleRunDeferral {
            retry_at: wait.retry_at,
            error: wait.to_string(),
        },
        completed_at,
    )
    .await
}

async fn defer_scheduled_run_state(
    state_db: &StateDbHandle,
    claim: &codex_state::ThreadScheduleClaim,
    deferral: &ScheduleRunDeferral,
    completed_at: DateTime<Utc>,
) -> anyhow::Result<Option<(codex_state::ThreadSchedule, codex_state::ThreadScheduleRun)>> {
    let updated = state_db
        .thread_schedules()
        .defer_thread_schedule_run(
            claim.schedule.schedule_id.as_str(),
            claim.run.run_id.as_str(),
            claim.run.lease_id.as_str(),
            completed_at,
            deferral.retry_at,
            deferral.error.clone(),
        )
        .await?;
    if !updated {
        return Ok(None);
    }
    let schedule = state_db
        .thread_schedules()
        .get_thread_schedule(&claim.schedule.schedule_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("deferred schedule missing after state update"))?;
    let run = state_db
        .thread_schedules()
        .get_thread_schedule_run(&claim.run.run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("deferred schedule run missing after state update"))?;
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
) -> codex_core::CodexThreadSettingsOverrides {
    codex_core::CodexThreadSettingsOverrides {
        cwd: Some(snapshot.cwd),
        workspace_roots: Some(snapshot.workspace_roots),
        profile_workspace_roots: Some(snapshot.profile_workspace_roots),
        approval_policy: Some(snapshot.approval_policy),
        approvals_reviewer: Some(snapshot.approvals_reviewer),
        permission_profile: Some(snapshot.permission_profile),
        active_permission_profile: snapshot.active_permission_profile,
        auth_profile,
        ..codex_core::CodexThreadSettingsOverrides::default()
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

pub(super) fn schedule_resume_auth_profile(
    schedule_auth_profile: Option<Option<String>>,
    initial_history: &InitialHistory,
) -> Option<Option<String>> {
    if schedule_auth_profile.is_some() {
        return schedule_auth_profile;
    }

    let history_auth_profile = initial_history.get_auth_profile();
    if matches!(history_auth_profile, Some(None))
        && let Some(Some(auth_profile)) = latest_session_auth_profile(initial_history)
    {
        return Some(Some(auth_profile));
    }
    history_auth_profile
}

fn latest_session_auth_profile(initial_history: &InitialHistory) -> Option<Option<String>> {
    match initial_history {
        InitialHistory::New | InitialHistory::Cleared => None,
        InitialHistory::Resumed(resumed) => {
            resumed.history.iter().rev().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line)
                    if meta_line.meta.id == resumed.conversation_id =>
                {
                    meta_line.meta.auth_profile.clone()
                }
                _ => None,
            })
        }
        InitialHistory::Forked(items) => items.iter().rev().find_map(|item| match item {
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

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
#[derive(Clone, Copy)]
enum ScheduleRunErrorClass {
    UsageLimit,
    ContextWindow,
}

impl ScheduleRunErrorClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::UsageLimit => "usage-limit",
            Self::ContextWindow => "context-window",
        }
    }
}

fn schedule_submit_error(error: &anyhow::Error) -> String {
    let message = format_schedule_error_chain(error);
    let classification = classify_schedule_run_error_message(&message);
    schedule_run_error_with_classification(&message, classification)
}

fn format_schedule_error_chain(error: &anyhow::Error) -> String {
    let mut message = error.to_string();
    for cause in error.chain().skip(1) {
        let _ = write!(message, ": {cause}");
    }
    message
}

fn schedule_turn_event_error(error: &codex_protocol::protocol::ErrorEvent) -> String {
    let classification = error
        .codex_error_info
        .as_ref()
        .and_then(classify_core_codex_error_info);
    schedule_run_error_with_classification(
        &format!("scheduled turn failed: {}", error.message),
        classification,
    )
}

fn schedule_turn_error(error: &codex_app_server_protocol::TurnError) -> String {
    let classification = error
        .codex_error_info
        .as_ref()
        .and_then(classify_codex_error_info);
    schedule_run_error_with_classification(
        &format!("scheduled turn failed: {}", error.message),
        classification,
    )
}

fn schedule_run_error(error: impl AsRef<str>) -> String {
    schedule_run_error_with_classification(error.as_ref(), None)
}

fn schedule_run_error_with_classification(
    error: &str,
    classification: Option<ScheduleRunErrorClass>,
) -> String {
    let classification = classification.or_else(|| classify_schedule_run_error_message(error));
    let error = match classification {
        Some(classification) => format!("[{}] {error}", classification.as_str()),
        None => error.to_string(),
    };
    truncate_schedule_run_error(redact_schedule_run_error(&error))
}

fn classify_core_codex_error_info(info: &CoreCodexErrorInfo) -> Option<ScheduleRunErrorClass> {
    match info {
        CoreCodexErrorInfo::UsageLimitExceeded => Some(ScheduleRunErrorClass::UsageLimit),
        CoreCodexErrorInfo::ContextWindowExceeded => Some(ScheduleRunErrorClass::ContextWindow),
        CoreCodexErrorInfo::ServerOverloaded
        | CoreCodexErrorInfo::CyberPolicy
        | CoreCodexErrorInfo::HttpConnectionFailed { .. }
        | CoreCodexErrorInfo::ResponseStreamConnectionFailed { .. }
        | CoreCodexErrorInfo::InternalServerError
        | CoreCodexErrorInfo::Unauthorized
        | CoreCodexErrorInfo::BadRequest
        | CoreCodexErrorInfo::ThreadRollbackFailed
        | CoreCodexErrorInfo::SandboxError
        | CoreCodexErrorInfo::ResponseStreamDisconnected { .. }
        | CoreCodexErrorInfo::ResponseTooManyFailedAttempts { .. }
        | CoreCodexErrorInfo::ActiveTurnNotSteerable { .. }
        | CoreCodexErrorInfo::Other => None,
    }
}

fn classify_codex_error_info(info: &CodexErrorInfo) -> Option<ScheduleRunErrorClass> {
    match info {
        CodexErrorInfo::UsageLimitExceeded => Some(ScheduleRunErrorClass::UsageLimit),
        CodexErrorInfo::ContextWindowExceeded => Some(ScheduleRunErrorClass::ContextWindow),
        CodexErrorInfo::ServerOverloaded
        | CodexErrorInfo::CyberPolicy
        | CodexErrorInfo::HttpConnectionFailed { .. }
        | CodexErrorInfo::ResponseStreamConnectionFailed { .. }
        | CodexErrorInfo::InternalServerError
        | CodexErrorInfo::Unauthorized
        | CodexErrorInfo::BadRequest
        | CodexErrorInfo::ThreadRollbackFailed
        | CodexErrorInfo::SandboxError
        | CodexErrorInfo::ResponseStreamDisconnected { .. }
        | CodexErrorInfo::ResponseTooManyFailedAttempts { .. }
        | CodexErrorInfo::ActiveTurnNotSteerable { .. }
        | CodexErrorInfo::Other => None,
    }
}

fn classify_schedule_run_error_message(message: &str) -> Option<ScheduleRunErrorClass> {
    let message = message.to_ascii_lowercase();
    if message.contains("usage limit")
        || message.contains("usage_limit")
        || message.contains("usage-limit")
        || message.contains("quota exceeded")
        || message.contains("usage not included")
    {
        return Some(ScheduleRunErrorClass::UsageLimit);
    }
    if message.contains("context_length_exceeded")
        || message.contains("context-length")
        || message.contains("context length")
        || message.contains("context window")
        || message.contains("ran out of room in the model")
    {
        return Some(ScheduleRunErrorClass::ContextWindow);
    }
    None
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
    use codex_protocol::protocol::ErrorEvent;
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

    fn prompt_test_schedule(nesting_depth: i64) -> codex_state::ThreadSchedule {
        codex_state::ThreadSchedule {
            thread_id: ThreadId::new(),
            schedule_id: "schedule-parent".to_string(),
            parent_schedule_id: (nesting_depth > 1).then(|| "schedule-root".to_string()),
            nesting_depth,
            auth_profile: None,
            prompt: "check status".to_string(),
            prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
            schedule: codex_state::ThreadScheduleSpec::Interval(
                codex_state::ThreadScheduleInterval {
                    amount: 5,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                },
            ),
            timezone: "UTC".to_string(),
            status: codex_state::ThreadScheduleStatus::Active,
            next_run_at: Some(at(/*seconds*/ 1_700_000_000)),
            last_run_at: None,
            expires_at: None,
            failure_count: 0,
            lease_id: None,
            lease_expires_at: None,
            created_at: at(/*seconds*/ 1_700_000_000),
            updated_at: at(/*seconds*/ 1_700_000_000),
        }
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
                    machine_id: None,
                    machine_name: None,
                    approval_policy: AskForApproval::OnRequest,
                    sandbox_policy: SandboxPolicy::DangerFullAccess,
                    permission_profile: None,
                    network: None,
                    file_system_sandbox_policy: None,
                    model: "gpt-5.5".to_string(),
                    model_provider_id: None,
                    personality: None,
                    collaboration_mode: None,
                    session_prompt: None,
                    worktree_mode: codex_protocol::protocol::SessionWorktreeMode::Manual,
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
        let completed = at(/*seconds*/ 1_000_000);
        let natural = at(/*seconds*/ 1_000_060); // natural cadence: 60s after completion.

        // Early failures: 30s/60s backoff is still earlier than the natural
        // 60s cadence, so the natural next run wins.
        assert_eq!(
            natural,
            schedule_failure_backoff_run_at(natural, completed, /*consecutive_failures*/ 1)
        );
        assert_eq!(
            natural,
            schedule_failure_backoff_run_at(natural, completed, /*consecutive_failures*/ 2)
        );

        // A longer streak backs off past the natural cadence...
        assert_eq!(
            completed + chrono::Duration::seconds(240),
            schedule_failure_backoff_run_at(natural, completed, /*consecutive_failures*/ 4)
        );
        // ...and caps at one hour regardless of how long the streak is.
        assert_eq!(
            completed + chrono::Duration::seconds(3600),
            schedule_failure_backoff_run_at(natural, completed, /*consecutive_failures*/ 50)
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
                session_prompt: None,
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
                worktree_mode: codex_protocol::protocol::SessionWorktreeMode::Manual,
                forked_from_thread_id: None,
                parent_thread_id: None,
                thread_source: None,
            },
            Some(Some("schedule-work".to_string())),
        );

        assert_eq!(Some(cwd), settings.cwd);
        assert_eq!(Some(vec![workspace_root]), settings.workspace_roots);
        assert_eq!(Some(vec![profile_root]), settings.profile_workspace_roots);
        assert_eq!(Some(AskForApproval::Never), settings.approval_policy);
        assert_eq!(
            Some(codex_protocol::config_types::ApprovalsReviewer::User),
            settings.approvals_reviewer
        );
        assert_eq!(Some(permission_profile), settings.permission_profile);
        assert_eq!(
            active_permission_profile,
            settings.active_permission_profile
        );
        assert_eq!(
            Some(Some("schedule-work".to_string())),
            settings.auth_profile
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
            at(/*seconds*/ 1_700_000_000),
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
            /*schedule_auth_profile*/ None,
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
            at(/*seconds*/ 1_700_000_000),
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
                    machine_id: None,
                    machine_name: None,
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
                    session_prompt: None,
                    multi_agent_version: None,
                    worktree_mode: codex_protocol::protocol::SessionWorktreeMode::Manual,
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
            /*schedule_auth_profile*/ None,
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
            /*schedule_auth_profile*/ None,
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
            /*schedule_auth_profile*/ None,
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
    fn legacy_schedule_auth_prefers_session_profile_over_root_turn() {
        let thread_id = ThreadId::new();
        let history = resumed_history_with_session_and_turn_auth_profile(
            thread_id,
            Some(Some("account002")),
            Some(None),
        );

        assert_eq!(Some(None), history.get_auth_profile());
        assert_eq!(
            Some(Some("account002".to_string())),
            schedule_resume_auth_profile(/*schedule_auth_profile*/ None, &history)
        );
    }

    #[test]
    fn legacy_schedule_auth_preserves_latest_root_session_profile() {
        let thread_id = ThreadId::new();
        let mut history = resumed_history_with_session_and_turn_auth_profile(
            thread_id,
            Some(Some("account002")),
            Some(None),
        );
        let InitialHistory::Resumed(resumed) = &mut history else {
            panic!("test history should be resumed");
        };
        resumed.history.insert(
            1,
            RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    auth_profile: Some(None),
                    ..SessionMeta::default()
                },
                git: None,
            }),
        );

        assert_eq!(Some(None), history.get_auth_profile());
        assert_eq!(
            Some(None),
            schedule_resume_auth_profile(/*schedule_auth_profile*/ None, &history)
        );
    }

    #[test]
    fn schedule_auth_profile_uses_broker_selected_profile_for_pinned_loop() {
        let decision = super::usage_profile_broker::UsageProfileBrokerDecision {
            selected_profile: Some("account003".to_string()),
            retry_at: None,
            reason: super::usage_profile_broker::UsageProfileBrokerDecisionReason::SelectedHealthyProfile,
        };

        let resolved = schedule_auth_profile_after_broker_decision(
            Some(Some("account001".to_string())),
            decision,
            /*reset_retry_buffer_secs*/ 30,
            at(/*seconds*/ 1_700_000_000),
        )
        .expect("broker-selected profile should be usable");

        assert_eq!(Some(Some("account003".to_string())), resolved);
    }

    #[test]
    fn schedule_auth_profile_reports_usage_wait_when_profiles_exhausted() {
        let now = at(/*seconds*/ 1_700_000_000);
        let decision = super::usage_profile_broker::UsageProfileBrokerDecision {
            selected_profile: None,
            retry_at: Some(now.timestamp() + 120),
            reason:
                super::usage_profile_broker::UsageProfileBrokerDecisionReason::NoAvailableProfiles,
        };

        let wait = schedule_auth_profile_after_broker_decision(
            Some(Some("account001".to_string())),
            decision,
            /*reset_retry_buffer_secs*/ 45,
            now,
        )
        .expect_err("exhausted profiles should defer the scheduled run");

        assert_eq!(
            ScheduleUsageProfileWait {
                retry_at: now + chrono::Duration::seconds(165),
            },
            wait
        );
        assert_eq!(
            "all eligible auth profiles are exhausted; retrying scheduled run after 2023-11-14T22:16:05+00:00",
            wait.to_string()
        );
    }

    #[test]
    fn stringified_usage_profile_wait_does_not_downcast() {
        let wait = ScheduleUsageProfileWait {
            retry_at: at(/*seconds*/ 1_700_000_165),
        };
        let error = anyhow::anyhow!(wait.to_string());

        assert!(error.downcast_ref::<ScheduleUsageProfileWait>().is_none());
        assert_eq!(wait.to_string(), schedule_submit_error(&error));
    }

    #[test]
    fn idle_rejection_deferral_only_retries_transient_busy_states() {
        let now = at(/*seconds*/ 1_700_000_000);

        assert_eq!(
            Some(ScheduleRunDeferral {
                retry_at: at(/*seconds*/ 1_700_000_030),
                error: "scheduled thread is busy".to_string(),
            }),
            schedule_deferral_for_idle_rejection(
                &codex_core::TryStartUserInputTurnIfIdleError::Rejected(
                    codex_core::TryStartTurnIfIdleRejectionReason::Busy,
                ),
                now,
            )
        );
        assert_eq!(
            Some(ScheduleRunDeferral {
                retry_at: at(/*seconds*/ 1_700_000_030),
                error: "scheduled thread has pending mailbox trigger-turn work".to_string(),
            }),
            schedule_deferral_for_idle_rejection(
                &codex_core::TryStartUserInputTurnIfIdleError::Rejected(
                    codex_core::TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
                ),
                now,
            )
        );
        assert_eq!(
            None,
            schedule_deferral_for_idle_rejection(
                &codex_core::TryStartUserInputTurnIfIdleError::Rejected(
                    codex_core::TryStartTurnIfIdleRejectionReason::PlanMode,
                ),
                now,
            )
        );
    }

    #[tokio::test]
    async fn idle_rejection_deferral_rearms_without_incrementing_failure_count() {
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
            at(/*seconds*/ 1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        state_db
            .upsert_thread(&builder.build("fallback-provider"))
            .await
            .expect("thread metadata should persist");
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = state_db
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: "wait until the thread is idle".to_string(),
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule: codex_state::ThreadScheduleSpec::Interval(
                    codex_state::ThreadScheduleInterval {
                        amount: 5,
                        unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                    },
                ),
                timezone: "UTC".to_string(),
                status: codex_state::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: None,
            })
            .await
            .expect("schedule should create");
        let claim = state_db
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-busy", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        let completed_at = now + chrono::Duration::seconds(5);
        let deferral = ScheduleRunDeferral {
            retry_at: now + chrono::Duration::seconds(SCHEDULE_IDLE_RETRY_DELAY_SECONDS),
            error: "scheduled thread is busy".to_string(),
        };

        let (deferred_schedule, deferred_run) =
            defer_scheduled_run_state(&state_db, &claim, &deferral, completed_at)
                .await
                .expect("idle rejection should defer")
                .expect("deferred rows should load");

        assert_eq!(
            codex_state::ThreadSchedule {
                next_run_at: Some(deferral.retry_at),
                last_run_at: Some(completed_at),
                failure_count: 0,
                lease_id: None,
                lease_expires_at: None,
                updated_at: deferred_schedule.updated_at,
                ..schedule
            },
            deferred_schedule
        );
        assert_eq!(
            codex_state::ThreadScheduleRun {
                status: codex_state::ThreadScheduleRunStatus::Deferred,
                turn_id: None,
                error: Some(deferral.error),
                completed_at: Some(completed_at),
                ..claim.run
            },
            deferred_run
        );
    }

    #[tokio::test]
    async fn usage_profile_wait_defers_claim_without_incrementing_failure_count() {
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
            at(/*seconds*/ 1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        state_db
            .upsert_thread(&builder.build("fallback-provider"))
            .await
            .expect("thread metadata should persist");
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = state_db
            .thread_schedules()
            .create_thread_schedule_for_auth_profile(
                codex_state::ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "wait for usage".to_string(),
                    prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                    schedule: codex_state::ThreadScheduleSpec::Interval(
                        codex_state::ThreadScheduleInterval {
                            amount: 5,
                            unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                        },
                    ),
                    timezone: "UTC".to_string(),
                    status: codex_state::ThreadScheduleStatus::Active,
                    next_run_at: Some(now),
                    expires_at: None,
                },
                Some("account001".to_string()),
            )
            .await
            .expect("schedule should create");
        let claim = state_db
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-wait", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        let completed_at = now + chrono::Duration::seconds(5);
        let wait = ScheduleUsageProfileWait {
            retry_at: now + chrono::Duration::minutes(20),
        };
        let error = anyhow::Error::new(wait.clone());
        let wait = error
            .downcast_ref::<ScheduleUsageProfileWait>()
            .expect("typed usage wait should survive anyhow wrapping")
            .clone();

        let (deferred_schedule, deferred_run) = defer_scheduled_run_for_usage_profile_wait_state(
            &state_db,
            &claim,
            &wait,
            completed_at,
        )
        .await
        .expect("usage wait should defer")
        .expect("deferred rows should load");

        assert_eq!(
            codex_state::ThreadSchedule {
                next_run_at: Some(wait.retry_at),
                last_run_at: Some(completed_at),
                lease_id: None,
                lease_expires_at: None,
                updated_at: deferred_schedule.updated_at,
                ..schedule
            },
            deferred_schedule
        );
        assert_eq!(
            codex_state::ThreadScheduleRun {
                status: codex_state::ThreadScheduleRunStatus::Deferred,
                turn_id: None,
                error: Some(wait.to_string()),
                completed_at: Some(completed_at),
                ..claim.run
            },
            deferred_run
        );
    }

    #[tokio::test]
    async fn usage_profile_wait_retry_completion_rearms_from_retry_run() {
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
            at(/*seconds*/ 1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        state_db
            .upsert_thread(&builder.build("fallback-provider"))
            .await
            .expect("thread metadata should persist");
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = state_db
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: "wait for usage".to_string(),
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule: codex_state::ThreadScheduleSpec::Interval(
                    codex_state::ThreadScheduleInterval {
                        amount: 5,
                        unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                    },
                ),
                timezone: "UTC".to_string(),
                status: codex_state::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: None,
            })
            .await
            .expect("schedule should create");
        let first_claim = state_db
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-wait", Duration::from_secs(300))
            .await
            .expect("first claim should succeed")
            .expect("schedule should claim");
        let wait = ScheduleUsageProfileWait {
            retry_at: now + chrono::Duration::minutes(3),
        };
        defer_scheduled_run_for_usage_profile_wait_state(
            &state_db,
            &first_claim,
            &wait,
            now + chrono::Duration::seconds(5),
        )
        .await
        .expect("usage wait should defer")
        .expect("deferred rows should load");

        let retry_claim = state_db
            .thread_schedules()
            .claim_due_thread_schedule(wait.retry_at, "lease-retry", Duration::from_secs(300))
            .await
            .expect("retry claim should succeed")
            .expect("retry should claim deferred schedule");
        assert_eq!(Some(wait.retry_at), retry_claim.run.scheduled_for);
        let completed_at = wait.retry_at + chrono::Duration::seconds(5);

        let (finished_schedule, finished_run) = finish_scheduled_run_state(
            &state_db,
            &schedule.schedule_id,
            &retry_claim.run.run_id,
            "lease-retry",
            None,
            completed_at,
        )
        .await
        .expect("retry run should finish")
        .expect("finished rows should load");

        assert_eq!(
            codex_state::ThreadSchedule {
                next_run_at: Some(wait.retry_at + chrono::Duration::minutes(5)),
                last_run_at: Some(completed_at),
                lease_id: None,
                lease_expires_at: None,
                updated_at: finished_schedule.updated_at,
                ..schedule
            },
            finished_schedule
        );
        assert_eq!(
            codex_state::ThreadScheduleRun {
                status: codex_state::ThreadScheduleRunStatus::Completed,
                completed_at: Some(completed_at),
                ..retry_claim.run
            },
            finished_run
        );
    }

    #[tokio::test]
    async fn finishing_recurring_run_preserves_scheduled_cadence() {
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
            at(/*seconds*/ 1_700_000_000),
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        state_db
            .upsert_thread(&builder.build("fallback-provider"))
            .await
            .expect("thread metadata should persist");
        let scheduled_for = at(/*seconds*/ 1_700_000_000);
        let schedule = state_db
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: "tick".to_string(),
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule: codex_state::ThreadScheduleSpec::Interval(
                    codex_state::ThreadScheduleInterval {
                        amount: 1,
                        unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                    },
                ),
                timezone: "UTC".to_string(),
                status: codex_state::ThreadScheduleStatus::Active,
                next_run_at: Some(scheduled_for),
                expires_at: None,
            })
            .await
            .expect("schedule should create");
        let claim = state_db
            .thread_schedules()
            .claim_due_thread_schedule(scheduled_for, "lease-run", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        let completed_at = scheduled_for + chrono::Duration::seconds(5);

        let (finished_schedule, finished_run) = finish_scheduled_run_state(
            &state_db,
            &schedule.schedule_id,
            &claim.run.run_id,
            "lease-run",
            None,
            completed_at,
        )
        .await
        .expect("run should finish")
        .expect("finished rows should load");

        assert_eq!(
            codex_state::ThreadSchedule {
                next_run_at: Some(scheduled_for + chrono::Duration::minutes(1)),
                last_run_at: Some(completed_at),
                lease_id: None,
                lease_expires_at: None,
                updated_at: finished_schedule.updated_at,
                ..schedule
            },
            finished_schedule
        );
        assert_eq!(
            codex_state::ThreadScheduleRun {
                status: codex_state::ThreadScheduleRunStatus::Completed,
                completed_at: Some(completed_at),
                ..claim.run
            },
            finished_run
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
    fn computes_recurring_next_run_from_scheduled_time_without_drift() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_000_060)),
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                    amount: 1,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                }),
                "UTC",
                Some(at(/*seconds*/ 1_700_000_000)),
                at(/*seconds*/ 1_700_000_005),
            )
            .expect("next interval should compute")
        );
    }

    #[test]
    fn computes_recurring_next_run_skipping_missed_intervals() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_000_180)),
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                    amount: 1,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                }),
                "UTC",
                Some(at(/*seconds*/ 1_700_000_000)),
                at(/*seconds*/ 1_700_000_125),
            )
            .expect("next interval should compute")
        );
    }

    #[test]
    fn computes_recurring_next_run_from_completion_without_scheduled_time() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_000_360)),
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                    amount: 1,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                }),
                "UTC",
                None,
                at(/*seconds*/ 1_700_000_300),
            )
            .expect("next interval should compute")
        );
    }

    #[test]
    fn computes_dynamic_next_run_from_scheduled_time_without_drift() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_000_060)),
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Dynamic,
                "UTC",
                Some(at(/*seconds*/ 1_700_000_000)),
                at(/*seconds*/ 1_700_000_005),
            )
            .expect("next dynamic run should compute")
        );
    }

    #[test]
    fn computes_manual_recurring_run_next_from_manual_scheduled_time() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_000_360)),
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                    amount: 1,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                }),
                "UTC",
                Some(at(/*seconds*/ 1_700_000_300)),
                at(/*seconds*/ 1_700_000_305),
            )
            .expect("manual recurring run should compute from its own scheduled time")
        );
    }

    #[test]
    fn computes_once_completion_without_rearming() {
        assert_eq!(
            None,
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Once,
                "UTC",
                Some(at(/*seconds*/ 1_700_000_000)),
                at(/*seconds*/ 1_700_000_005),
            )
            .expect("one-time completion should not compute a follow-up run")
        );
    }

    #[test]
    fn computes_cron_completion_from_completion_time() {
        assert_eq!(
            Some(at(/*seconds*/ 1_700_031_600)),
            next_thread_schedule_run_after_completion(
                &codex_state::ThreadScheduleSpec::Cron {
                    expression: "0 9 * * *".to_string(),
                },
                "Europe/Bucharest",
                Some(at(/*seconds*/ 1_699_913_600)),
                at(/*seconds*/ 1_700_000_000),
            )
            .expect("cron completion should compute from completion time")
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

    fn scheduled_prompt_test_schedule(
        schedule: codex_state::ThreadScheduleSpec,
    ) -> codex_state::ThreadSchedule {
        codex_state::ThreadSchedule {
            thread_id: ThreadId::new(),
            schedule_id: "schedule-123".to_string(),
            parent_schedule_id: None,
            nesting_depth: 1,
            auth_profile: None,
            prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
            prompt: "ask me a funny question every minute".to_string(),
            schedule,
            timezone: "UTC".to_string(),
            status: codex_state::ThreadScheduleStatus::Active,
            next_run_at: Some(at(/*seconds*/ 1_700_000_000)),
            last_run_at: None,
            expires_at: None,
            failure_count: 0,
            lease_id: None,
            lease_expires_at: None,
            created_at: at(/*seconds*/ 1_700_000_000),
            updated_at: at(/*seconds*/ 1_700_000_000),
        }
    }

    #[test]
    fn scheduled_thread_prompt_tells_model_not_to_wait() {
        let schedule = scheduled_prompt_test_schedule(codex_state::ThreadScheduleSpec::Dynamic);
        let prompt = scheduled_thread_prompt(
            "ask me a funny question every minute",
            &schedule,
            "run-123",
            Some(at(/*seconds*/ 1_700_000_000)),
        );

        assert!(prompt.contains("one new scheduled Codewith prompt"));
        assert!(prompt.contains("Loop schedule id: schedule-123"));
        assert!(prompt.contains("Parent loop schedule id: none"));
        assert!(prompt.contains("Loop nesting depth: 1/5"));
        assert!(prompt.contains("Run id: run-123"));
        assert!(prompt.contains("Scheduled for: 2023-11-14T22:13:20+00:00"));
        assert!(prompt.contains("This is a distinct run"));
        assert!(prompt.contains("Produce exactly one visible final response"));
        assert!(prompt.contains("Do not wait, sleep, or start a timer"));
        assert!(prompt.contains("use native create_goal or create_goal_plan goal tools"));
        assert!(prompt.contains("create_goal_plan with append_to_plan_id"));
        assert!(prompt.contains("parent_schedule_id set to schedule-123"));
        assert!(prompt.contains("child cadence must be slower than the parent cadence"));
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
        let mut schedule = prompt_test_schedule(/*nesting_depth*/ 5);
        schedule.schedule =
            codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                amount: 5,
                unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
            });
        let prompt = scheduled_goal_thread_prompt(
            "finish release readiness checks every hour",
            "run-123",
            Some(at(/*seconds*/ 1_700_000_000)),
            &schedule,
        );

        assert!(prompt.contains("one new scheduled Codewith goal objective"));
        assert!(prompt.contains("Run id: run-123"));
        assert!(prompt.contains("Scheduled for: 2023-11-14T22:13:20+00:00"));
        assert!(prompt.contains("Loop schedule id: schedule-parent"));
        assert!(prompt.contains("Loop nesting: level 5/5"));
        assert!(prompt.contains("do not create nested child loops"));
        assert!(prompt.contains("active thread goal has already been persisted"));
        assert!(prompt.contains("Produce exactly one visible final response"));
        assert!(prompt.contains("unless this objective explicitly asks for a native child loop"));
        assert!(prompt.contains("not as an instruction to implement the cadence yourself"));
        assert!(prompt.ends_with("finish release readiness checks every hour"));
        assert!(!prompt.contains("/goal finish release readiness checks"));
    }

    #[test]
    fn scheduled_goal_thread_prompt_skips_nested_loop_guidance_for_unsupported_parent_cadence() {
        let mut cron_schedule = prompt_test_schedule(/*nesting_depth*/ 1);
        cron_schedule.schedule = codex_state::ThreadScheduleSpec::Cron {
            expression: "*/5 * * * *".to_string(),
        };
        let cron_prompt = scheduled_goal_thread_prompt(
            "start nested work every ten minutes",
            "run-123",
            Some(at(/*seconds*/ 1_700_000_000)),
            &cron_schedule,
        );
        assert!(cron_prompt.contains("cannot be used as a nested-loop parent"));
        assert!(!cron_prompt.contains("pass schedule-parent as parent_schedule_id"));

        let mut once_schedule = prompt_test_schedule(/*nesting_depth*/ 1);
        once_schedule.schedule = codex_state::ThreadScheduleSpec::Once;
        let once_prompt = scheduled_goal_thread_prompt(
            "start nested work every ten minutes",
            "run-456",
            Some(at(/*seconds*/ 1_700_000_000)),
            &once_schedule,
        );
        assert!(once_prompt.contains("cannot be used as a nested-loop parent"));
        assert!(!once_prompt.contains("pass schedule-parent as parent_schedule_id"));
    }

    #[test]
    fn scheduled_thread_prompt_skips_nested_loop_guidance_for_unsupported_parent_cadence() {
        let cron_schedule = scheduled_prompt_test_schedule(codex_state::ThreadScheduleSpec::Cron {
            expression: "*/5 * * * *".to_string(),
        });
        let cron_prompt = scheduled_thread_prompt(
            "start nested work every ten minutes",
            &cron_schedule,
            "run-123",
            Some(at(/*seconds*/ 1_700_000_000)),
        );
        assert!(cron_prompt.contains("cannot be used as a nested-loop parent"));
        assert!(!cron_prompt.contains("parent_schedule_id set to schedule-123"));

        let once_schedule = scheduled_prompt_test_schedule(codex_state::ThreadScheduleSpec::Once);
        let once_prompt = scheduled_thread_prompt(
            "start nested work every ten minutes",
            &once_schedule,
            "run-456",
            Some(at(/*seconds*/ 1_700_000_000)),
        );
        assert!(once_prompt.contains("cannot be used as a nested-loop parent"));
        assert!(!once_prompt.contains("parent_schedule_id set to schedule-123"));
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
    fn scheduled_turn_error_fails() {
        let finish = scheduled_turn_finish(&EventMsg::Error(ErrorEvent {
            message: "auth profile missing".to_string(),
            codex_error_info: None,
        }));

        assert_eq!(
            Some(ScheduledTurnFinish::Failed(
                "scheduled turn failed: auth profile missing".to_string()
            )),
            finish
        );
    }

    #[test]
    fn scheduled_turn_usage_limit_error_is_classified_and_redacted() {
        let finish = scheduled_turn_finish(&EventMsg::Error(ErrorEvent {
            message: "You've hit your usage limit. OPENAI_API_KEY=sk-test-secret".to_string(),
            codex_error_info: Some(CoreCodexErrorInfo::UsageLimitExceeded),
        }));

        let expected = "[usage-limit] scheduled turn failed: You've hit your usage limit. OPENAI_API_KEY=[redacted]".to_string();
        assert_eq!(Some(ScheduledTurnFinish::Failed(expected)), finish);
    }

    #[test]
    fn scheduled_turn_context_window_error_preserves_node_init_detail() {
        let error = codex_app_server_protocol::TurnError {
            message: "node init failed: process exited with code 1: child stderr: Codewith ran out of room in the model's context window. token=sk-test-secret".to_string(),
            codex_error_info: Some(CodexErrorInfo::ContextWindowExceeded),
            additional_details: None,
        };

        let sanitized = schedule_turn_error(&error);

        assert!(sanitized.starts_with("[context-window] scheduled turn failed:"));
        assert!(sanitized.contains("node init failed"));
        assert!(sanitized.contains("context window"));
        assert!(!sanitized.contains("sk-test-secret"));
        assert_ne!(
            "node init failed: process exited with code 1",
            sanitized.as_str()
        );
    }

    #[test]
    fn submit_error_preserves_usage_limit_cause_below_node_init() {
        let error =
            anyhow::anyhow!("child stderr: You've hit your usage limit. Bearer sk-test-secret")
                .context("node init failed: process exited with code 1");

        let sanitized = schedule_submit_error(&error);

        assert!(sanitized.starts_with("[usage-limit]"));
        assert!(sanitized.contains("node init failed"));
        assert!(sanitized.contains("usage limit"));
        assert!(!sanitized.contains("sk-test-secret"));
        assert_ne!(
            "node init failed: process exited with code 1",
            sanitized.as_str()
        );
    }

    #[test]
    fn schedule_run_error_heuristically_classifies_context_length() {
        let sanitized = schedule_run_error(
            "node init failed: process exited with code 1: context_length_exceeded: input too large",
        );

        assert!(sanitized.starts_with("[context-window]"));
        assert!(sanitized.contains("context_length_exceeded"));
    }

    #[test]
    fn redacts_sensitive_schedule_run_error_values() {
        let sanitized = schedule_run_error(
            "failed with OPENAI_API_KEY=sk-test-secret token: plain-secret Bearer sk-bearer-secret",
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
            "unexpected status 401 Unauthorized: Incorrect API key provided: sk-work.",
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
