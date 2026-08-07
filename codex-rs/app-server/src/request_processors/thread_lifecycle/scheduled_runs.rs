//! Scheduled-run tracking owned by the live thread event listener.

use super::*;

pub(super) struct TrackedScheduledEvent {
    pub(super) raw_events_enabled: bool,
    pub(super) terminal_event: bool,
    pub(super) scheduled_run: Option<crate::thread_state::ScheduledThreadScheduleRun>,
    pub(super) turn_error: Option<codex_app_server_protocol::TurnError>,
}

pub(super) fn track_scheduled_event(
    thread_state: &mut ThreadState,
    turn_id: &str,
    event: &EventMsg,
) -> TrackedScheduledEvent {
    let terminal_event = matches!(event, EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_))
        || matches!(event, EventMsg::Error(error) if error.affects_turn_status());
    thread_state.track_current_turn_event(turn_id, event);
    let scheduled_run = terminal_event
        .then(|| thread_state.take_scheduled_run(turn_id))
        .flatten();
    let turn_error = terminal_event
        .then(|| thread_state.turn_summary.last_error.clone())
        .flatten();
    TrackedScheduledEvent {
        raw_events_enabled: thread_state.experimental_raw_events,
        terminal_event,
        scheduled_run,
        turn_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::CodexErrorInfo;
    use codex_protocol::protocol::ErrorEvent;
    use codex_protocol::protocol::TurnCompleteEvent;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn non_affecting_error_keeps_scheduled_run_running_until_turn_complete() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let now = chrono::DateTime::<Utc>::from_timestamp(1_700_000_000, 0)
            .expect("test timestamp should be valid");
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            temp_dir.path().join("thread.jsonl"),
            now,
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        state_db
            .upsert_thread(&builder.build("fallback-provider"))
            .await
            .expect("thread metadata should persist");
        let schedule = state_db
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: "finish after a non-terminal error".to_string(),
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
            .claim_due_thread_schedule(now, "lease-live", Duration::from_secs(300))
            .await
            .expect("schedule claim should succeed")
            .expect("schedule should be due");
        let turn_id = claim
            .run
            .turn_id
            .as_deref()
            .expect("claimed occurrence should reserve a turn")
            .to_string();
        state_db
            .thread_schedules()
            .enqueue_thread_schedule_run(codex_state::ThreadScheduleRunEnqueueParams {
                schedule_id: schedule.schedule_id.as_str(),
                run_id: claim.run.run_id.as_str(),
                lease_id: claim.run.lease_id.as_str(),
                goal_id: None,
                auth_profile_recorded: true,
                auth_profile: None,
                turn_input: "scheduled input",
                now,
            })
            .await
            .expect("occurrence should enqueue")
            .expect("owned occurrence should enqueue");
        let running = state_db
            .thread_schedules()
            .mark_thread_schedule_run_started(codex_state::ThreadScheduleRunStartParams {
                schedule_id: schedule.schedule_id.as_str(),
                run_id: claim.run.run_id.as_str(),
                lease_id: claim.run.lease_id.as_str(),
                turn_id: turn_id.as_str(),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(300),
            })
            .await
            .expect("occurrence should start")
            .expect("owned occurrence should materialize one run");

        let mut thread_state = ThreadState::default();
        thread_state.track_scheduled_run(
            turn_id.clone(),
            crate::thread_state::ScheduledThreadScheduleRun {
                schedule_id: schedule.schedule_id.clone(),
                run_id: running.run_id.clone(),
                lease_id: running.lease_id.clone(),
                goal_id: None,
                state_db: state_db.clone(),
            },
        );
        let non_terminal_error = EventMsg::Error(ErrorEvent {
            message: "rollback request failed".to_string(),
            codex_error_info: Some(CodexErrorInfo::ThreadRollbackFailed),
        });
        let tracked =
            track_scheduled_event(&mut thread_state, turn_id.as_str(), &non_terminal_error);
        assert!(!tracked.terminal_event);
        assert!(tracked.scheduled_run.is_none());
        assert!(thread_state.has_scheduled_run(turn_id.as_str()));
        assert_eq!(
            codex_state::ThreadScheduleRunStatus::Running,
            state_db
                .thread_schedules()
                .get_thread_schedule_run(running.run_id.as_str())
                .await
                .expect("running row should load")
                .expect("running row should remain")
                .status
        );

        let complete = EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: turn_id.clone(),
            last_agent_message: Some("finished after rollback warning".to_string()),
            completed_at: Some(1_700_000_001),
            duration_ms: Some(1_000),
            time_to_first_token_ms: Some(100),
        });
        let tracked = track_scheduled_event(&mut thread_state, turn_id.as_str(), &complete);
        assert!(tracked.terminal_event);
        assert!(!thread_state.has_scheduled_run(turn_id.as_str()));
        let scheduled_run = tracked
            .scheduled_run
            .expect("terminal event should take the tracked run once");
        let (outgoing_tx, _outgoing_rx) = mpsc::channel(/*buffer*/ 8);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            codex_analytics::AnalyticsEventsClient::disabled(),
        ));
        super::super::thread_schedule_runtime::finish_scheduled_run_after_turn(
            thread_id,
            scheduled_run,
            &complete,
            tracked.turn_error,
            &outgoing,
        )
        .await;
        assert_eq!(
            codex_state::ThreadScheduleRunStatus::Completed,
            state_db
                .thread_schedules()
                .get_thread_schedule_run(running.run_id.as_str())
                .await
                .expect("completed row should load")
                .expect("completed row should remain")
                .status
        );
        assert!(
            super::super::thread_schedule_runtime::recover_scheduled_run_for_terminal_turn(
                &state_db,
                thread_id,
                turn_id.as_str(),
            )
            .await
            .expect("completed run recovery should not fail")
            .is_none(),
            "a completed run must not be finalized twice"
        );
    }
}
