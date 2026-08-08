use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn idle_rejection_reuses_one_pending_occurrence_without_counting_a_run() {
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
    assert!(
        state_db
            .thread_schedules()
            .claim_due_thread_schedule(
                now - chrono::Duration::seconds(1),
                "lease-before-due",
                Duration::from_secs(300),
            )
            .await
            .expect("before-due claim should not fail")
            .is_none()
    );
    assert_eq!(
        codex_state::ThreadScheduleStats::default(),
        state_db
            .thread_schedules()
            .get_thread_schedule_stats(schedule.schedule_id.as_str())
            .await
            .expect("before-due stats should load")
    );
    let claim = state_db
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-busy", Duration::from_secs(300))
        .await
        .expect("claim should succeed")
        .expect("schedule should claim");
    let completed_at = now + chrono::Duration::seconds(5);
    let deferral = ScheduleRunDeferral {
        kind: ScheduleRunDeferralKind::IdleAdmission,
        retry_at: now + chrono::Duration::seconds(SCHEDULE_IDLE_RETRY_DELAY_SECONDS),
        error: "scheduled thread is busy".to_string(),
    };

    let deferred_schedule =
        wait_scheduled_run_for_idle_state(&state_db, &claim, &deferral, completed_at)
            .await
            .expect("idle rejection should defer")
            .expect("waiting schedule should load");

    assert_eq!(
        codex_state::ThreadSchedule {
            next_run_at: Some(now),
            last_run_at: None,
            failure_count: 0,
            lease_id: None,
            lease_expires_at: None,
            updated_at: deferred_schedule.updated_at,
            ..schedule.clone()
        },
        deferred_schedule
    );
    let stats = state_db
        .thread_schedules()
        .get_thread_schedule_stats(schedule.schedule_id.as_str())
        .await
        .expect("schedule stats should load while waiting for idle");
    assert_eq!(
        codex_state::ThreadScheduleStats::default(),
        stats,
        "idle waiting is admission state, not a durable run"
    );

    let retry_claim = state_db
        .thread_schedules()
        .claim_due_thread_schedule(
            deferral.retry_at,
            "lease-busy-retry",
            Duration::from_secs(300),
        )
        .await
        .expect("idle retry claim should succeed")
        .expect("pending occurrence should be reclaimed");
    assert_eq!(claim.run.run_id, retry_claim.run.run_id);
    assert_eq!(claim.run.turn_id, retry_claim.run.turn_id);
    let retry_stats = state_db
        .thread_schedules()
        .get_thread_schedule_stats(schedule.schedule_id.as_str())
        .await
        .expect("schedule stats should load after idle retry claim");
    assert_eq!(codex_state::ThreadScheduleStats::default(), retry_stats);
}

#[tokio::test]
async fn restart_finalizes_a_started_terminal_turn_before_honoring_its_held_goal() {
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
    let goal = state_db
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "recover terminal rollout",
            codex_state::ThreadGoalStatus::Blocked,
            /*token_budget*/ None,
        )
        .await
        .expect("blocked goal should persist");
    let scheduled_for = at(/*seconds*/ 1_700_000_000);
    let schedule = state_db
        .thread_schedules()
        .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
            thread_id,
            prompt: "/goal recover terminal rollout".to_string(),
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
        .claim_due_thread_schedule(scheduled_for, "lease-first", Duration::from_secs(1))
        .await
        .expect("schedule claim should succeed")
        .expect("schedule should claim");
    let started = enqueue_and_start_claim(
        &state_db,
        &claim,
        Some(goal.goal_id.as_str()),
        scheduled_for,
        Duration::from_secs(1),
    )
    .await;
    let turn_id = started
        .turn_id
        .clone()
        .expect("started occurrence should retain its stable turn id");
    drop(state_db);

    let reopened = codex_state::StateRuntime::init(
        temp_dir.path().to_path_buf(),
        "fallback-provider".to_string(),
    )
    .await
    .expect("state db should reopen");
    let recovered = reopened
        .thread_schedules()
        .claim_due_thread_schedule(
            scheduled_for + chrono::Duration::seconds(2),
            "lease-recovered",
            Duration::from_secs(300),
        )
        .await
        .expect("started occurrence recovery should succeed")
        .expect("expired started occurrence should be reclaimed");
    assert_eq!(claim.run.run_id, recovered.run.run_id);
    assert_eq!(Some(turn_id.clone()), recovered.run.turn_id);
    assert_eq!(Some(goal.goal_id.clone()), recovered.run.goal_id);
    assert_eq!(
        codex_state::ThreadScheduleOccurrenceState::Started,
        recovered.occurrence_state
    );

    let completed_at = scheduled_for + chrono::Duration::seconds(1);
    let history = resumed_history_with_turn_events(
        thread_id,
        [
            turn_started(turn_id.as_str(), scheduled_for.timestamp()),
            EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: turn_id.clone(),
                last_agent_message: Some("already finished".to_string()),
                completed_at: Some(completed_at.timestamp()),
                duration_ms: Some(1_000),
                time_to_first_token_ms: Some(100),
            }),
        ],
    );
    let terminal = persisted_scheduled_turn_terminal(
        &history,
        turn_id.as_str(),
        scheduled_for + chrono::Duration::seconds(99),
    )
    .expect("the persisted matching turn should already be terminal");

    let finished = finish_scheduled_run_state(
        &reopened,
        schedule.schedule_id.as_str(),
        recovered.run.run_id.as_str(),
        recovered.run.lease_id.as_str(),
        recovered.run.goal_id.as_deref(),
        terminal.error,
        terminal.completed_at,
    )
    .await
    .expect("persisted terminal turn should finalize")
    .expect("one schedule run should finalize");
    assert_eq!(
        codex_state::ThreadScheduleRunStatus::Completed,
        finished.1.status
    );
    assert_eq!(Some(completed_at), finished.1.completed_at);
    assert_eq!(codex_state::ThreadScheduleStatus::Paused, finished.0.status);
    assert_eq!(None, finished.0.next_run_at);
    assert!(
        finish_scheduled_run_state(
            &reopened,
            schedule.schedule_id.as_str(),
            recovered.run.run_id.as_str(),
            recovered.run.lease_id.as_str(),
            recovered.run.goal_id.as_deref(),
            /*error*/ None,
            completed_at,
        )
        .await
        .expect("replayed finalization should not fail")
        .is_none(),
        "terminal replay must not create or finish another run"
    );
    assert_eq!(
        codex_state::ThreadScheduleStats {
            total_runs: 1,
            completed_runs: 1,
            last_started_at: Some(scheduled_for),
            last_completed_at: Some(completed_at),
            ..codex_state::ThreadScheduleStats::default()
        },
        reopened
            .thread_schedules()
            .get_thread_schedule_stats(schedule.schedule_id.as_str())
            .await
            .expect("schedule stats should load")
    );
}

#[tokio::test]
async fn stale_started_owner_cannot_submit_after_same_occurrence_is_reclaimed() {
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
            prompt: "never submit stale work".to_string(),
            prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
            schedule: codex_state::ThreadScheduleSpec::Interval(
                codex_state::ThreadScheduleInterval {
                    amount: 1,
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
        .claim_due_thread_schedule(now, "lease-suspended", Duration::from_secs(30))
        .await
        .expect("claim should succeed")
        .expect("schedule should claim");
    enqueue_and_start_claim(
        &state_db,
        &claim,
        /*goal_id*/ None,
        now,
        Duration::from_secs(30),
    )
    .await;

    let contender = codex_state::StateRuntime::init(
        temp_dir.path().to_path_buf(),
        "fallback-provider".to_string(),
    )
    .await
    .expect("contending state db should initialize");
    let resumed_at = now + chrono::Duration::seconds(31);
    let recovered = contender
        .thread_schedules()
        .claim_due_thread_schedule(resumed_at, "lease-replacement", Duration::from_secs(30))
        .await
        .expect("expired lease reaper should not error")
        .expect("expired started run should be recovered");
    assert_eq!(claim.run.run_id, recovered.run.run_id);
    assert_eq!(claim.run.turn_id, recovered.run.turn_id);
    let submitted = Arc::new(AtomicBool::new(false));
    let submission_observer = Arc::clone(&submitted);
    let ownership_lost = CancellationToken::new();

    let submission = submit_scheduled_turn_if_owned(
        &state_db,
        codex_state::ThreadScheduleRunLeaseParams {
            schedule_id: schedule.schedule_id.as_str(),
            run_id: claim.run.run_id.as_str(),
            lease_id: claim.run.lease_id.as_str(),
            now: resumed_at,
            lease_duration: Duration::from_secs(30),
        },
        &ownership_lost,
        async move {
            submission_observer.store(true, Ordering::SeqCst);
        },
    )
    .await
    .expect("stale dispatch validation should not error");

    assert_eq!(None, submission);
    assert!(!submitted.load(Ordering::SeqCst));
    assert_eq!(
        codex_state::ThreadScheduleRunStatus::Running,
        state_db
            .thread_schedules()
            .get_thread_schedule_run(claim.run.run_id.as_str())
            .await
            .expect("recovered run should load")
            .expect("recovered run should exist")
            .status
    );
    assert_eq!(
        codex_state::ThreadScheduleRunStatus::Running,
        recovered.run.status
    );

    ownership_lost.cancel();
    let cancelled_submission_observer = Arc::clone(&submitted);
    let cancelled_submission = submit_scheduled_turn_if_owned(
        &state_db,
        codex_state::ThreadScheduleRunLeaseParams {
            schedule_id: schedule.schedule_id.as_str(),
            run_id: recovered.run.run_id.as_str(),
            lease_id: recovered.run.lease_id.as_str(),
            now: resumed_at + chrono::Duration::seconds(1),
            lease_duration: Duration::from_secs(30),
        },
        &ownership_lost,
        async move {
            cancelled_submission_observer.store(true, Ordering::SeqCst);
        },
    )
    .await
    .expect("cancelled dispatch validation should not error");
    assert_eq!(None, cancelled_submission);
    assert!(!submitted.load(Ordering::SeqCst));
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
fn persisted_completed_scheduled_turn_is_terminal_for_the_matching_turn_only() {
    let thread_id = ThreadId::new();
    let completed_at = 1_700_000_005;
    let history = resumed_history_with_turn_events(
        thread_id,
        [
            turn_started("turn-other", /*started_at*/ 1_699_999_990),
            EventMsg::Error(ErrorEvent {
                message: "other turn failed".to_string(),
                codex_error_info: None,
            }),
            EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: "turn-other".to_string(),
                last_agent_message: None,
                completed_at: Some(1_699_999_999),
                duration_ms: Some(9_000),
                time_to_first_token_ms: None,
            }),
            turn_started("turn-scheduled", /*started_at*/ 1_700_000_000),
            EventMsg::Error(ErrorEvent {
                message: "non-terminal rollback warning".to_string(),
                codex_error_info: Some(CoreCodexErrorInfo::ThreadRollbackFailed),
            }),
            EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: "turn-scheduled".to_string(),
                last_agent_message: Some("done".to_string()),
                completed_at: Some(completed_at),
                duration_ms: Some(5_000),
                time_to_first_token_ms: Some(100),
            }),
        ],
    );

    assert_eq!(
        Some(PersistedScheduledTurnTerminal {
            completed_at: at(completed_at),
            error: None,
        }),
        persisted_scheduled_turn_terminal(
            &history,
            "turn-scheduled",
            at(/*seconds*/ 1_700_000_999),
        )
    );
    assert_eq!(
        None,
        persisted_scheduled_turn_terminal(&history, "turn-missing", at(/*seconds*/ 1_700_000_999),)
    );
}

#[test]
fn persisted_failed_scheduled_turn_keeps_the_replayed_failure() {
    let thread_id = ThreadId::new();
    let history = resumed_history_with_turn_events(
        thread_id,
        [
            turn_started("turn-scheduled", /*started_at*/ 1_700_000_000),
            EventMsg::Error(ErrorEvent {
                message: "model failed".to_string(),
                codex_error_info: None,
            }),
            EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: "turn-scheduled".to_string(),
                last_agent_message: None,
                completed_at: Some(1_700_000_006),
                duration_ms: Some(6_000),
                time_to_first_token_ms: None,
            }),
        ],
    );

    assert_eq!(
        Some(PersistedScheduledTurnTerminal {
            completed_at: at(/*seconds*/ 1_700_000_006),
            error: Some("scheduled turn failed: model failed".to_string()),
        }),
        persisted_scheduled_turn_terminal(
            &history,
            "turn-scheduled",
            at(/*seconds*/ 1_700_000_999),
        )
    );
}

#[test]
fn persisted_aborted_scheduled_turn_is_an_explicit_failure() {
    let thread_id = ThreadId::new();
    let history = resumed_history_with_turn_events(
        thread_id,
        [
            turn_started("turn-scheduled", /*started_at*/ 1_700_000_000),
            EventMsg::TurnAborted(TurnAbortedEvent {
                turn_id: Some("turn-scheduled".to_string()),
                reason: TurnAbortReason::Interrupted,
                completed_at: Some(1_700_000_007),
                duration_ms: Some(7_000),
            }),
        ],
    );

    assert_eq!(
        Some(PersistedScheduledTurnTerminal {
            completed_at: at(/*seconds*/ 1_700_000_007),
            error: Some("scheduled turn was interrupted".to_string()),
        }),
        persisted_scheduled_turn_terminal(
            &history,
            "turn-scheduled",
            at(/*seconds*/ 1_700_000_999),
        )
    );
}

#[test]
fn persisted_aborted_scheduled_turn_without_a_start_is_an_explicit_failure() {
    let thread_id = ThreadId::new();
    let history = resumed_history_with_turn_events(
        thread_id,
        [EventMsg::TurnAborted(TurnAbortedEvent {
            turn_id: Some("turn-scheduled".to_string()),
            reason: TurnAbortReason::Interrupted,
            completed_at: Some(1_700_000_007),
            duration_ms: Some(7_000),
        })],
    );

    assert_eq!(
        Some(PersistedScheduledTurnTerminal {
            completed_at: at(/*seconds*/ 1_700_000_007),
            error: Some("scheduled turn aborted: Interrupted".to_string()),
        }),
        persisted_scheduled_turn_terminal(
            &history,
            "turn-scheduled",
            at(/*seconds*/ 1_700_000_999),
        )
    );
}

#[test]
fn persisted_in_progress_scheduled_turn_is_not_terminal() {
    let thread_id = ThreadId::new();
    let history = resumed_history_with_turn_events(
        thread_id,
        [
            turn_started("turn-scheduled", /*started_at*/ 1_700_000_000),
            EventMsg::Error(ErrorEvent {
                message: "rollback request failed".to_string(),
                codex_error_info: Some(CoreCodexErrorInfo::ThreadRollbackFailed),
            }),
        ],
    );

    assert_eq!(
        None,
        persisted_scheduled_turn_terminal(
            &history,
            "turn-scheduled",
            at(/*seconds*/ 1_700_000_999),
        )
    );
}

#[test]
fn scheduled_turn_non_affecting_error_is_not_terminal() {
    assert_eq!(
        None,
        scheduled_turn_finish(&EventMsg::Error(ErrorEvent {
            message: "rollback request failed".to_string(),
            codex_error_info: Some(CoreCodexErrorInfo::ThreadRollbackFailed),
        }))
    );
}
