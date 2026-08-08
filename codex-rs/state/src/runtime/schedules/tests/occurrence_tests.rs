use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn legacy_active_run_insert_fails_closed_without_matching_occurrence() {
    let runtime = test_runtime().await;
    let thread_id = test_thread_id(/*id*/ 52);
    upsert_test_thread(&runtime, thread_id).await;
    let now = at(/*seconds*/ 1_700_000_000);
    let schedule =
        create_interval_schedule(&runtime, thread_id, "rollback safety", Some(now)).await;

    let error = sqlx::query(
        r#"
INSERT INTO thread_schedule_runs (
run_id,
schedule_id,
thread_id,
status,
lease_id,
scheduled_for_ms,
started_at_ms
) VALUES (?, ?, ?, 'leased', ?, ?, ?)
        "#,
    )
    .bind("legacy-fresh-run")
    .bind(schedule.schedule_id.as_str())
    .bind(thread_id.to_string())
    .bind("legacy-lease")
    .bind(datetime_to_epoch_millis(now))
    .bind(datetime_to_epoch_millis(now))
    .execute(runtime.pool.as_ref())
    .await
    .expect_err("an older binary must not create active work without an occurrence");
    assert!(
        error
            .to_string()
            .contains("active schedule occurrence must be reused"),
        "unexpected rollback guard error: {error}"
    );

    let claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "new-runtime-lease", Duration::from_secs(30))
        .await
        .expect("new runtime claim should succeed")
        .expect("schedule should claim");
    let run = enqueue_and_start_claim(
        &runtime,
        &claim,
        /*goal_id*/ None,
        "new runtime input",
        now,
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(claim.run.run_id, run.run_id);
    assert_eq!(crate::ThreadScheduleRunStatus::Running, run.status);
}

#[tokio::test]
async fn legacy_schedule_hold_cannot_resurrect_a_pending_occurrence_after_roll_forward() {
    for (index, phase, held_status) in [
        (0, "waiting", crate::ThreadScheduleStatus::Paused),
        (1, "enqueued", crate::ThreadScheduleStatus::Expired),
        (2, "started", crate::ThreadScheduleStatus::Expired),
    ] {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 53 + index);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, &format!("rollback {phase}"), Some(now))
                .await;
        let lease_id = format!("legacy-{phase}-lease");
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, lease_id.as_str(), Duration::from_secs(300))
            .await
            .expect("initial claim should succeed")
            .expect("schedule should claim");
        if phase == "enqueued" {
            runtime
                .thread_schedules()
                .enqueue_thread_schedule_run(ThreadScheduleRunEnqueueParams {
                    schedule_id: schedule.schedule_id.as_str(),
                    run_id: claim.run.run_id.as_str(),
                    lease_id: lease_id.as_str(),
                    goal_id: None,
                    auth_profile_recorded: false,
                    auth_profile: None,
                    turn_input: "accepted before downgrade",
                    now,
                })
                .await
                .expect("occurrence should enqueue")
                .expect("enqueue should retain ownership");
        } else if phase == "started" {
            enqueue_and_start_claim(
                &runtime,
                &claim,
                /*goal_id*/ None,
                "started before downgrade",
                now,
                Duration::from_secs(300),
            )
            .await;
            // Match the older expiration order: terminalize the legacy run
            // first while the schedule still carries its lease, then clear
            // the schedule lease in a separate statement.
            sqlx::query(
                r#"
UPDATE thread_schedule_runs
SET status = 'failed', error = 'legacy expiry', completed_at_ms = ?
WHERE run_id = ? AND status = 'running'
                "#,
            )
            .bind(datetime_to_epoch_millis(now + chrono::Duration::seconds(1)))
            .bind(claim.run.run_id.as_str())
            .execute(runtime.pool.as_ref())
            .await
            .expect("legacy run terminal update should succeed");
        }

        let held_at = now + chrono::Duration::seconds(2);
        sqlx::query(
            r#"
UPDATE thread_schedules
SET status = ?,
next_run_at_ms = NULL,
lease_id = NULL,
lease_expires_at_ms = NULL,
updated_at_ms = ?
WHERE schedule_id = ?
            "#,
        )
        .bind(held_status.as_str())
        .bind(datetime_to_epoch_millis(held_at))
        .bind(schedule.schedule_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("legacy schedule hold should succeed");

        let occurrence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM thread_schedule_occurrences WHERE schedule_id = ?",
        )
        .bind(schedule.schedule_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("occurrence count should load");
        assert_eq!(0, occurrence_count, "stale {phase} occurrence survived");
        let held_run = runtime
            .thread_schedules()
            .get_thread_schedule_run(claim.run.run_id.as_str())
            .await
            .expect("legacy run lookup should succeed");
        if phase == "waiting" {
            assert_eq!(None, held_run, "idle waiting is not a durable run");
        } else {
            assert_eq!(
                Some(crate::ThreadScheduleRunStatus::Failed),
                held_run.map(|run| run.status),
                "accepted {phase} work should be explicitly terminal"
            );
        }

        let resume_at = now + chrono::Duration::minutes(5);
        runtime
            .thread_schedules()
            .resume_thread_schedule_at(schedule.schedule_id.as_str(), resume_at)
            .await
            .expect("roll-forward resume should succeed")
            .expect("held schedule should still exist");
        let resumed_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(
                resume_at,
                &format!("roll-forward-{phase}"),
                Duration::from_secs(300),
            )
            .await
            .expect("roll-forward claim should succeed")
            .expect("resumed schedule should claim");
        assert_ne!(
            claim.run.run_id, resumed_claim.run.run_id,
            "roll-forward must create a new occurrence after legacy {phase} termination"
        );
        assert_eq!(
            ThreadScheduleOccurrenceState::WaitingIdle,
            resumed_claim.occurrence_state
        );
    }
}

#[tokio::test]
async fn terminal_once_occurrence_survives_the_legacy_schedule_hold_trigger() {
    let runtime = test_runtime().await;
    let thread_id = test_thread_id(/*id*/ 57);
    upsert_test_thread(&runtime, thread_id).await;
    let now = at(/*seconds*/ 1_700_000_000);
    let schedule = runtime
        .thread_schedules()
        .create_thread_schedule(ThreadScheduleCreateParams {
            thread_id,
            prompt: "once trigger fencing".to_string(),
            prompt_source: crate::ThreadSchedulePromptSource::Inline,
            schedule: crate::ThreadScheduleSpec::Once,
            timezone: "UTC".to_string(),
            status: crate::ThreadScheduleStatus::Active,
            next_run_at: Some(now),
            expires_at: None,
        })
        .await
        .expect("once schedule should create");
    let claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-once-trigger", Duration::from_secs(30))
        .await
        .expect("once schedule should claim")
        .expect("once schedule should be due");
    enqueue_and_start_claim(
        &runtime,
        &claim,
        /*goal_id*/ None,
        "once trigger fencing",
        now,
        Duration::from_secs(30),
    )
    .await;
    let completed_at = now + chrono::Duration::seconds(1);
    assert!(
        runtime
            .thread_schedules()
            .record_thread_schedule_run_terminal(
                schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                completed_at,
                /*expected_goal_id*/ None,
                /*error*/ None,
            )
            .await
            .expect("terminal outcome should persist")
    );

    let result = sqlx::query(
        r#"
UPDATE thread_schedules
SET status = 'expired',
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    last_run_at_ms = ?,
    next_run_at_ms = NULL,
    updated_at_ms = ?
WHERE schedule_id = ? AND lease_id = ?
        "#,
    )
    .bind(datetime_to_epoch_millis(completed_at))
    .bind(datetime_to_epoch_millis(completed_at))
    .bind(schedule.schedule_id.as_str())
    .bind(claim.run.lease_id.as_str())
    .execute(runtime.pool.as_ref())
    .await
    .expect("new-runtime once finalization step should update the schedule");
    assert_eq!(1, result.rows_affected());

    let occurrence_state: Option<String> =
        sqlx::query_scalar("SELECT state FROM thread_schedule_occurrences WHERE occurrence_id = ?")
            .bind(claim.run.run_id.as_str())
            .fetch_optional(runtime.pool.as_ref())
            .await
            .expect("terminal occurrence should load");
    assert_eq!(
        Some("terminal".to_string()),
        occurrence_state,
        "the compatibility trigger must leave terminal work for fenced runtime finalization"
    );
}

#[tokio::test]
async fn claim_due_thread_schedule_recovers_started_occurrence_without_duplicate_run() {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("state db should initialize");
    let thread_id = test_thread_id(/*id*/ 44);
    upsert_test_thread(runtime.as_ref(), thread_id).await;
    let now = at(/*seconds*/ 1_700_000_000);
    let schedule =
        create_interval_schedule(runtime.as_ref(), thread_id, "restart retry", Some(now)).await;
    let original_claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-before-restart", Duration::from_secs(30))
        .await
        .expect("initial claim should succeed")
        .expect("schedule should claim");
    enqueue_and_start_claim(
        &runtime,
        &original_claim,
        /*goal_id*/ None,
        "restart input",
        now,
        Duration::from_secs(30),
    )
    .await;
    drop(runtime);

    let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
        .await
        .expect("state db should reopen after process restart");
    let retry_at = now + chrono::Duration::seconds(31);
    let retry_claim = reopened
        .thread_schedules()
        .claim_due_thread_schedule(retry_at, "lease-after-restart", Duration::from_secs(30))
        .await
        .expect("expired run recovery should succeed")
        .expect("expired non-goal run should retry exactly once");

    let recovered_run = reopened
        .thread_schedules()
        .get_thread_schedule_run(&original_claim.run.run_id)
        .await
        .expect("recovered run should load")
        .expect("recovered run should exist");
    assert_eq!(
        crate::ThreadScheduleRunStatus::Running,
        recovered_run.status
    );
    assert_eq!(
        crate::ThreadScheduleRunStatus::Running,
        retry_claim.run.status
    );
    assert_eq!(
        original_claim.run.scheduled_for,
        retry_claim.run.scheduled_for
    );
    assert_eq!(original_claim.run.run_id, retry_claim.run.run_id);
    assert_eq!(original_claim.run.turn_id, retry_claim.run.turn_id);
    let stats = reopened
        .thread_schedules()
        .get_thread_schedule_stats(&schedule.schedule_id)
        .await
        .expect("schedule stats should load");
    assert_eq!(1, stats.total_runs);
    assert_eq!(0, stats.leased_runs);
    assert_eq!(1, stats.running_runs);
    assert_eq!(0, stats.failed_runs);
    assert!(
        reopened
            .thread_schedules()
            .claim_due_thread_schedule(retry_at, "lease-duplicate-retry", Duration::from_secs(30),)
            .await
            .expect("duplicate claim check should succeed")
            .is_none(),
        "one expired lease may create at most one replacement claim"
    );
}

#[tokio::test]
async fn claim_due_thread_schedule_recovers_waiting_idle_occurrence_after_restart() {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("state db should initialize");
    let thread_id = test_thread_id(/*id*/ 51);
    upsert_test_thread(runtime.as_ref(), thread_id).await;
    let now = at(/*seconds*/ 1_700_000_000);
    let retry_at = now + chrono::Duration::seconds(30);
    let schedule =
        create_interval_schedule(runtime.as_ref(), thread_id, "waiting restart", Some(now)).await;
    let claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-waiting", Duration::from_secs(30))
        .await
        .expect("initial claim should succeed")
        .expect("schedule should claim");
    assert!(
        runtime
            .thread_schedules()
            .wait_thread_schedule_run_for_idle(
                schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                retry_at,
                now + chrono::Duration::seconds(1),
            )
            .await
            .expect("waiting occurrence should persist its retry")
    );
    drop(runtime);

    let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
        .await
        .expect("state db should reopen");
    let recovered = reopened
        .thread_schedules()
        .claim_due_thread_schedule(retry_at, "lease-waiting-recovery", Duration::from_secs(30))
        .await
        .expect("waiting recovery should succeed")
        .expect("waiting occurrence should be reclaimed");
    assert_eq!(claim.run.run_id, recovered.run.run_id);
    assert_eq!(claim.run.turn_id, recovered.run.turn_id);
    assert_eq!(
        ThreadScheduleOccurrenceState::WaitingIdle,
        recovered.occurrence_state
    );
    assert!(recovered.turn_input.is_none());
    assert_eq!(
        crate::ThreadScheduleStats::default(),
        reopened
            .thread_schedules()
            .get_thread_schedule_stats(schedule.schedule_id.as_str())
            .await
            .expect("stats should load")
    );
}

#[tokio::test]
async fn claim_due_thread_schedule_recovers_enqueued_input_profile_and_stable_ids() {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("state db should initialize");
    let thread_id = test_thread_id(/*id*/ 48);
    upsert_test_thread(runtime.as_ref(), thread_id).await;
    let now = at(/*seconds*/ 1_700_000_000);
    let schedule =
        create_interval_schedule(runtime.as_ref(), thread_id, "enqueued restart", Some(now)).await;
    let claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-enqueued", Duration::from_secs(30))
        .await
        .expect("initial claim should succeed")
        .expect("schedule should claim");
    runtime
        .thread_schedules()
        .enqueue_thread_schedule_run(ThreadScheduleRunEnqueueParams {
            schedule_id: schedule.schedule_id.as_str(),
            run_id: claim.run.run_id.as_str(),
            lease_id: claim.run.lease_id.as_str(),
            goal_id: None,
            auth_profile_recorded: true,
            auth_profile: Some("alternate-profile"),
            turn_input: "persisted enqueued input",
            now,
        })
        .await
        .expect("occurrence should enqueue")
        .expect("occurrence should retain its lease");
    let retry_at = now + chrono::Duration::seconds(30);
    assert!(
        runtime
            .thread_schedules()
            .wait_thread_schedule_run_for_idle(
                schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                retry_at,
                now + chrono::Duration::seconds(1),
            )
            .await
            .expect("enqueued occurrence should wait for idle")
    );
    drop(runtime);

    let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
        .await
        .expect("state db should reopen");
    assert!(
        reopened
            .thread_schedules()
            .claim_due_thread_schedule(
                retry_at - chrono::Duration::seconds(1),
                "lease-enqueued-too-early",
                Duration::from_secs(30),
            )
            .await
            .expect("early enqueued recovery should not fail")
            .is_none(),
        "idle retry must honor the occurrence retry time"
    );
    let recovered = reopened
        .thread_schedules()
        .claim_due_thread_schedule(retry_at, "lease-enqueued-recovery", Duration::from_secs(30))
        .await
        .expect("enqueued recovery should succeed")
        .expect("enqueued occurrence should be reclaimed");
    assert_eq!(claim.run.run_id, recovered.run.run_id);
    assert_eq!(claim.run.turn_id, recovered.run.turn_id);
    assert_eq!(
        ThreadScheduleOccurrenceState::Enqueued,
        recovered.occurrence_state
    );
    assert_eq!(
        Some("persisted enqueued input".to_string()),
        recovered.turn_input
    );
    assert_eq!(
        Some(Some("alternate-profile".to_string())),
        recovered.occurrence_auth_profile
    );
    assert_eq!(
        crate::ThreadScheduleStats::default(),
        reopened
            .thread_schedules()
            .get_thread_schedule_stats(schedule.schedule_id.as_str())
            .await
            .expect("stats should load"),
        "Enqueued is not a durable run"
    );
}

#[tokio::test]
async fn terminal_recovery_finalizes_interval_cron_and_once_cadence_once() {
    let now = at(/*seconds*/ 1_700_000_000);
    let cases = [
        (
            "interval",
            crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                amount: 5,
                unit: crate::ThreadScheduleIntervalUnit::Minutes,
            }),
            Some(now + chrono::Duration::minutes(5)),
            crate::ThreadScheduleStatus::Active,
        ),
        (
            "cron",
            crate::ThreadScheduleSpec::Cron {
                expression: "*/5 * * * *".to_string(),
            },
            Some(now + chrono::Duration::minutes(5)),
            crate::ThreadScheduleStatus::Active,
        ),
        (
            "once",
            crate::ThreadScheduleSpec::Once,
            None,
            crate::ThreadScheduleStatus::Expired,
        ),
    ];

    for (name, schedule_spec, next_run_at, expected_status) in cases {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id = test_thread_id(/*id*/ 49);
        upsert_test_thread(runtime.as_ref(), thread_id).await;
        let schedule = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: format!("{name} terminal recovery"),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: schedule_spec,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: None,
            })
            .await
            .expect("schedule should create");
        let lease_id = format!("lease-{name}");
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, lease_id.as_str(), Duration::from_secs(30))
            .await
            .expect("schedule should claim")
            .expect("schedule should be due");
        enqueue_and_start_claim(
            &runtime,
            &claim,
            /*goal_id*/ None,
            name,
            now,
            Duration::from_secs(30),
        )
        .await;
        let completed_at = now + chrono::Duration::seconds(1);
        assert!(
            runtime
                .thread_schedules()
                .record_thread_schedule_run_terminal(
                    schedule.schedule_id.as_str(),
                    claim.run.run_id.as_str(),
                    claim.run.lease_id.as_str(),
                    completed_at,
                    /*expected_goal_id*/ None,
                    /*error*/ None,
                )
                .await
                .expect("terminal outcome should persist")
        );
        let before_finalization = runtime
            .thread_schedules()
            .get_thread_schedule(schedule.schedule_id.as_str())
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(Some(now), before_finalization.next_run_at);
        assert_eq!(None, before_finalization.last_run_at);
        drop(runtime);

        let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("state db should reopen after terminal persistence");
        let recovery_lease_id = format!("lease-{name}-recovery");
        let recovered = reopened
            .thread_schedules()
            .claim_due_thread_schedule(
                now + chrono::Duration::seconds(31),
                recovery_lease_id.as_str(),
                Duration::from_secs(30),
            )
            .await
            .expect("terminal recovery claim should succeed")
            .expect("terminal occurrence should be reclaimed");
        assert_eq!(claim.run.run_id, recovered.run.run_id);
        assert_eq!(
            ThreadScheduleOccurrenceState::Terminal,
            recovered.occurrence_state
        );
        assert!(
            reopened
                .thread_schedules()
                .finalize_terminal_thread_schedule_run(
                    schedule.schedule_id.as_str(),
                    recovered.run.run_id.as_str(),
                    recovered.run.lease_id.as_str(),
                    completed_at,
                    next_run_at,
                    /*expected_goal_id*/ None,
                )
                .await
                .expect("terminal finalization should succeed")
        );
        assert!(
            !reopened
                .thread_schedules()
                .finalize_terminal_thread_schedule_run(
                    schedule.schedule_id.as_str(),
                    recovered.run.run_id.as_str(),
                    recovered.run.lease_id.as_str(),
                    completed_at,
                    next_run_at,
                    /*expected_goal_id*/ None,
                )
                .await
                .expect("replayed finalization should be idempotent")
        );
        let finalized = reopened
            .thread_schedules()
            .get_thread_schedule(schedule.schedule_id.as_str())
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(expected_status, finalized.status);
        assert_eq!(next_run_at, finalized.next_run_at);
        assert_eq!(Some(completed_at), finalized.last_run_at);
        let stats = reopened
            .thread_schedules()
            .get_thread_schedule_stats(schedule.schedule_id.as_str())
            .await
            .expect("stats should load");
        assert_eq!(1, stats.total_runs);
        assert_eq!(1, stats.completed_runs);
    }
}

#[tokio::test]
async fn fatal_pre_start_failure_creates_one_failed_terminal_run() {
    let runtime = test_runtime().await;
    let thread_id = test_thread_id(/*id*/ 50);
    upsert_test_thread(&runtime, thread_id).await;
    let now = at(/*seconds*/ 1_700_000_000);
    let schedule =
        create_interval_schedule(&runtime, thread_id, "fatal preparation", Some(now)).await;
    let claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-fatal", Duration::from_secs(30))
        .await
        .expect("schedule should claim")
        .expect("schedule should be due");
    let completed_at = now + chrono::Duration::seconds(1);
    assert!(
        runtime
            .thread_schedules()
            .fail_thread_schedule_occurrence_before_start(
                schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                completed_at,
                /*goal_id*/ None,
                "prompt source unavailable".to_string(),
            )
            .await
            .expect("pre-start failure should persist")
    );
    assert!(
        runtime
            .thread_schedules()
            .finalize_terminal_thread_schedule_run(
                schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                claim.run.lease_id.as_str(),
                completed_at,
                Some(now + chrono::Duration::minutes(5)),
                /*expected_goal_id*/ None,
            )
            .await
            .expect("failed terminal should finalize")
    );
    let run = runtime
        .thread_schedules()
        .get_thread_schedule_run(claim.run.run_id.as_str())
        .await
        .expect("run should load")
        .expect("failed run should exist");
    assert_eq!(crate::ThreadScheduleRunStatus::Failed, run.status);
    assert_eq!(Some("prompt source unavailable".to_string()), run.error);
    let stats = runtime
        .thread_schedules()
        .get_thread_schedule_stats(schedule.schedule_id.as_str())
        .await
        .expect("stats should load");
    assert_eq!(1, stats.total_runs);
    assert_eq!(1, stats.failed_runs);
    assert_eq!(0, stats.deferred_runs);
}

#[tokio::test]
async fn claim_due_thread_schedule_returns_started_run_before_honoring_held_goal() {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("state db should initialize");
    let thread_id = test_thread_id(/*id*/ 45);
    upsert_test_thread(runtime.as_ref(), thread_id).await;
    let goal = runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "hold after restart",
            crate::ThreadGoalStatus::Blocked,
            /*token_budget*/ None,
        )
        .await
        .expect("blocked goal should persist");
    let now = at(/*seconds*/ 1_700_000_000);
    let schedule =
        create_interval_schedule(runtime.as_ref(), thread_id, "hold after restart", Some(now))
            .await;
    let original_claim = runtime
        .thread_schedules()
        .claim_due_thread_schedule(now, "lease-goal-restart", Duration::from_secs(30))
        .await
        .expect("initial claim should succeed")
        .expect("schedule should claim");
    enqueue_and_start_claim(
        &runtime,
        &original_claim,
        Some(&goal.goal_id),
        "goal restart input",
        now,
        Duration::from_secs(30),
    )
    .await;
    drop(runtime);

    let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
        .await
        .expect("state db should reopen after process restart");
    let retry_at = now + chrono::Duration::seconds(31);
    let recovered = reopened
        .thread_schedules()
        .claim_due_thread_schedule(retry_at, "lease-held-replacement", Duration::from_secs(30))
        .await
        .expect("expired goal run recovery should succeed")
        .expect("started run must reach durable rollout recovery before goal handling");
    assert_eq!(original_claim.run.run_id, recovered.run.run_id);
    assert_eq!(
        crate::ThreadScheduleOccurrenceState::Started,
        recovered.occurrence_state
    );

    let held_schedule = reopened
        .thread_schedules()
        .get_thread_schedule(&schedule.schedule_id)
        .await
        .expect("schedule should load")
        .expect("schedule should exist");
    assert_eq!(crate::ThreadScheduleStatus::Active, held_schedule.status);
    assert_eq!(Some(now), held_schedule.next_run_at);
    assert_eq!(Some(recovered.run.lease_id.clone()), held_schedule.lease_id);
    let original_run = reopened
        .thread_schedules()
        .get_thread_schedule_run(&original_claim.run.run_id)
        .await
        .expect("original run should load")
        .expect("original run should exist");
    assert_eq!(crate::ThreadScheduleRunStatus::Running, original_run.status);
    assert_eq!(Some(goal.goal_id), original_run.goal_id);
    assert_eq!(None, original_run.completed_at);
    let stats = reopened
        .thread_schedules()
        .get_thread_schedule_stats(&schedule.schedule_id)
        .await
        .expect("schedule stats should load");
    assert_eq!(1, stats.total_runs);
    assert_eq!(0, stats.leased_runs);
    assert_eq!(1, stats.running_runs);
    assert_eq!(0, stats.failed_runs);
}
