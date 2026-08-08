//! Durable rollout terminal detection and idempotent schedule finalization.

use super::*;

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(crate) enum ScheduledTurnFinish {
    Complete,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedScheduledTurnTerminal {
    pub(crate) completed_at: DateTime<Utc>,
    pub(crate) error: Option<String>,
}

pub(in super::super) fn persisted_scheduled_turn_terminal(
    history: &InitialHistory,
    turn_id: &str,
    fallback_completed_at: DateTime<Utc>,
) -> Option<PersistedScheduledTurnTerminal> {
    let rollout_items = history.get_rollout_items();
    let turn = build_api_turns_from_rollout_items(&rollout_items)
        .into_iter()
        .find(|turn| turn.id == turn_id);
    let Some(turn) = turn else {
        let aborted = rollout_items
            .iter()
            .filter_map(|item| match item {
                RolloutItem::EventMsg(EventMsg::TurnAborted(aborted))
                    if aborted.turn_id.as_deref() == Some(turn_id) =>
                {
                    Some(aborted)
                }
                _ => None,
            })
            .last()?;
        return Some(PersistedScheduledTurnTerminal {
            completed_at: aborted
                .completed_at
                .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
                .unwrap_or(fallback_completed_at),
            error: Some(schedule_run_error(format!(
                "scheduled turn aborted: {:?}",
                aborted.reason
            ))),
        });
    };
    let completed_at = turn
        .completed_at
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        .unwrap_or(fallback_completed_at);
    let mut replaying_turn = false;
    let mut replayed_error = None;
    for item in &rollout_items {
        match item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(started)) => {
                if replaying_turn {
                    break;
                }
                replaying_turn = started.turn_id == turn_id;
            }
            RolloutItem::EventMsg(EventMsg::Error(error))
                if replaying_turn && error.affects_turn_status() =>
            {
                replayed_error = Some(schedule_turn_event_error(error));
                break;
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(completed))
                if replaying_turn && completed.turn_id == turn_id =>
            {
                break;
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(aborted))
                if replaying_turn && aborted.turn_id.as_deref() == Some(turn_id) =>
            {
                break;
            }
            _ => {}
        }
    }
    let error = if let Some(error) = replayed_error {
        Some(error)
    } else {
        match turn.status {
            codex_app_server_protocol::TurnStatus::Completed => {
                let finish = rollout_items
                    .iter()
                    .filter_map(|item| match item {
                        RolloutItem::EventMsg(event @ EventMsg::TurnComplete(completed))
                            if completed.turn_id == turn_id =>
                        {
                            scheduled_turn_finish(event)
                        }
                        _ => None,
                    })
                    .last()?;
                match finish {
                    ScheduledTurnFinish::Complete => None,
                    ScheduledTurnFinish::Failed(error) => Some(error),
                }
            }
            codex_app_server_protocol::TurnStatus::Failed => Some(
                turn.error
                    .as_ref()
                    .map(schedule_turn_error)
                    .unwrap_or_else(|| schedule_run_error("scheduled turn failed")),
            ),
            codex_app_server_protocol::TurnStatus::Interrupted => {
                Some(schedule_run_error("scheduled turn was interrupted"))
            }
            codex_app_server_protocol::TurnStatus::InProgress => return None,
        }
    };
    Some(PersistedScheduledTurnTerminal {
        completed_at,
        error,
    })
}

pub(in super::super) fn scheduled_turn_finish(event: &EventMsg) -> Option<ScheduledTurnFinish> {
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
        EventMsg::Error(error) if error.affects_turn_status() => Some(ScheduledTurnFinish::Failed(
            schedule_turn_event_error(error),
        )),
        _ => None,
    }
}

pub(in super::super::super) fn default_thread_schedule_expires_at(
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    now.checked_add_signed(ChronoDuration::days(DEFAULT_SCHEDULE_EXPIRATION_DAYS))
}

pub(in super::super::super) fn next_thread_schedule_run_at(
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

pub(in super::super) fn next_thread_schedule_run_after_completion(
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

pub(in super::super::super) fn normalize_schedule_timezone(
    timezone: &str,
) -> anyhow::Result<String> {
    parse_schedule_timezone(timezone).map(|timezone| timezone.name().to_string())
}

pub(in super::super::super) async fn finish_scheduled_run_after_turn(
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
        scheduled_run.goal_id.as_deref(),
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

pub(in super::super::super) async fn recover_scheduled_run_for_terminal_turn(
    state_db: &StateDbHandle,
    thread_id: ThreadId,
    turn_id: &str,
) -> anyhow::Result<Option<crate::thread_state::ScheduledThreadScheduleRun>> {
    let Some(run) = state_db
        .thread_schedules()
        .get_running_thread_schedule_run_for_turn(thread_id, turn_id)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(crate::thread_state::ScheduledThreadScheduleRun {
        schedule_id: run.schedule_id,
        run_id: run.run_id,
        lease_id: run.lease_id,
        goal_id: run.goal_id,
        state_db: state_db.clone(),
    }))
}
