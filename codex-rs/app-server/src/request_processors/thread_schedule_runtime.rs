use super::*;
use crate::request_processors::thread_schedule_processor::api_thread_schedule_from_state;
use crate::request_processors::thread_schedule_processor::api_thread_schedule_run_from_state;
use chrono_tz::Tz;
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
        let thread = self.load_or_resume_thread(thread_id).await?;
        self.ensure_schedule_listener(thread_id, thread.clone())
            .await?;
        let turn_id = thread
            .submit(Op::UserInput {
                items: vec![CoreInputItem::Text {
                    text: scheduled_loop_tick_prompt(&prompt),
                    text_elements: Vec::new(),
                }],
                environments: None,
                final_output_json_schema: None,
                responsesapi_client_metadata: None,
                thread_settings: codex_protocol::protocol::ThreadSettingsOverrides::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("failed to submit scheduled prompt: {err}"))?;

        let run = state_db
            .thread_schedules()
            .mark_thread_schedule_run_started(
                claim.schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                turn_id.as_str(),
            )
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "claimed schedule run {} disappeared before it could start",
                    claim.run.run_id
                )
            })?;
        {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            thread_state.lock().await.track_scheduled_run(
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

    async fn load_or_resume_thread(&self, thread_id: ThreadId) -> anyhow::Result<Arc<CodexThread>> {
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
                /*persist_extended_history*/ false,
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

fn scheduled_loop_tick_prompt(prompt: &str) -> String {
    format!(
        "\
You are running one scheduled /loop tick.

Execute only the loop prompt below for this single tick. Do not wait, sleep, start a timer, or schedule the next tick; Codex already manages the loop cadence. If the loop prompt mentions a cadence like \"every minute\", treat that as the cadence that triggered this tick, not as an instruction to implement the cadence yourself.

Loop prompt:
{prompt}"
    )
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
    outgoing: &Arc<OutgoingMessageSender>,
) {
    let completed_at = Utc::now();
    let error = match event {
        EventMsg::TurnComplete(_) => None,
        EventMsg::TurnAborted(aborted) => Some(schedule_run_error(format!(
            "scheduled turn aborted: {:?}",
            aborted.reason
        ))),
        _ => return,
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
    let next_run_at =
        next_thread_schedule_run_at(&schedule.schedule, &schedule.timezone, completed_at)?;
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
    request_overrides: &mut Option<HashMap<String, serde_json::Value>>,
    typesafe_overrides: &mut ConfigOverrides,
) {
    let persisted_metadata = match state_db.get_thread(thread_id).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return,
        Err(err) => {
            warn!("failed to read persisted metadata for scheduled thread {thread_id}: {err}");
            return;
        }
    };
    typesafe_overrides.model = persisted_metadata.model;
    typesafe_overrides.model_provider = Some(persisted_metadata.model_provider);
    if let Some(reasoning_effort) = persisted_metadata.reasoning_effort {
        request_overrides.get_or_insert_with(HashMap::new).insert(
            "model_reasoning_effort".to_string(),
            serde_json::Value::String(reasoning_effort.to_string()),
        );
    }
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
    trimmed.len() >= 12 && trimmed.starts_with("sk-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    #[test]
    fn computes_interval_next_run() {
        assert_eq!(
            Some(at(1_700_000_300)),
            next_thread_schedule_run_at(
                &codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                    amount: 5,
                    unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
                }),
                "UTC",
                at(1_700_000_000),
            )
            .expect("next interval should compute")
        );
    }

    #[test]
    fn computes_cron_next_run_in_timezone() {
        assert_eq!(
            Some(at(1_700_031_600)),
            next_thread_schedule_run_at(
                &codex_state::ThreadScheduleSpec::Cron {
                    expression: "0 9 * * *".to_string(),
                },
                "Europe/Bucharest",
                at(1_700_000_000),
            )
            .expect("next cron run should compute")
        );
    }

    #[test]
    fn rejects_unknown_timezone() {
        assert!(normalize_schedule_timezone("Nope/Nowhere").is_err());
    }

    #[test]
    fn scheduled_loop_tick_prompt_tells_model_not_to_wait() {
        let prompt = scheduled_loop_tick_prompt("ask me a funny question every minute");

        assert!(prompt.contains("one scheduled /loop tick"));
        assert!(prompt.contains("Do not wait, sleep, start a timer"));
        assert!(prompt.contains("not as an instruction to implement the cadence yourself"));
        assert!(prompt.ends_with("ask me a funny question every minute"));
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
    fn truncates_schedule_run_error_values() {
        let sanitized = schedule_run_error("x".repeat(MAX_SCHEDULE_RUN_ERROR_CHARS + 8));

        assert_eq!(MAX_SCHEDULE_RUN_ERROR_CHARS, sanitized.chars().count());
        assert!(sanitized.ends_with("..."));
    }
}
