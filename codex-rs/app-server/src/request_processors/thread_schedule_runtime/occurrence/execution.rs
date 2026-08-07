//! Admission, stable-turn submission, and crash recovery execution.

use super::*;

impl ThreadScheduleRuntime {
    pub(super) async fn execute_occurrence_claim(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
    ) {
        let thread_id = claim.schedule.thread_id;
        if claim.occurrence_state == codex_state::ThreadScheduleOccurrenceState::Terminal {
            self.replay_terminal_claim(state_db, claim).await;
            return;
        }
        if claim.occurrence_state == codex_state::ThreadScheduleOccurrenceState::Started {
            let turn_id = match claim.run.turn_id.as_deref() {
                Some(turn_id) => turn_id,
                None => {
                    let goal_id = claim.run.goal_id.clone();
                    self.fail_claimed_run_after_submit_error(
                        state_db,
                        claim,
                        goal_id,
                        "accepted scheduled turn cannot be resumed because its stable turn identifier is unavailable"
                            .to_string(),
                    )
                    .await;
                    return;
                }
            };
            match self
                .persisted_terminal_scheduled_turn(&state_db, thread_id, turn_id)
                .await
            {
                Ok(Some(terminal)) => {
                    self.finish_claim_from_persisted_terminal(state_db, claim, terminal)
                        .await;
                    return;
                }
                Ok(None) => match self
                    .held_started_scheduled_goal(&state_db, thread_id, claim.run.goal_id.as_deref())
                    .await
                {
                    Ok(Some(held)) => {
                        let goal_id = held.goal_id.clone();
                        self.fail_claimed_run_after_submit_error(
                            state_db,
                            claim,
                            Some(goal_id),
                            held.to_string(),
                        )
                        .await;
                        return;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let goal_id = claim.run.goal_id.clone();
                        self.fail_claimed_run_after_submit_error(
                            state_db,
                            claim,
                            goal_id,
                            format!(
                                "accepted scheduled turn cannot be safely resumed because its goal state could not be inspected: {error}"
                            ),
                        )
                        .await;
                        return;
                    }
                },
                Err(error) => {
                    let goal_id = claim.run.goal_id.clone();
                    self.fail_claimed_run_after_submit_error(
                        state_db,
                        claim,
                        goal_id,
                        format!(
                            "accepted scheduled turn cannot be safely resumed because its durable rollout could not be inspected: {error}"
                        ),
                    )
                    .await;
                    return;
                }
            }
        }
        if claim.occurrence_state == codex_state::ThreadScheduleOccurrenceState::Started
            && claim.turn_input.is_none()
        {
            let goal_id = claim.run.goal_id.clone();
            self.fail_claimed_run_after_submit_error(
                state_db,
                claim,
                goal_id,
                "accepted scheduled turn cannot be resumed because its persisted input is unavailable"
                    .to_string(),
            )
            .await;
            return;
        }

        let resolved_prompt = if claim.turn_input.is_some() {
            Ok(String::new())
        } else {
            self.resolve_claim_prompt(&state_db, thread_id, &claim.schedule)
                .await
        };
        let result = match resolved_prompt {
            Ok(prompt) => {
                let scheduled_goal_objective = claim
                    .turn_input
                    .is_none()
                    .then(|| scheduled_goal_objective(&prompt).map(str::to_string))
                    .flatten();
                self.submit_claimed_schedule(
                    thread_id,
                    state_db.clone(),
                    &claim,
                    prompt,
                    scheduled_goal_objective,
                )
                .await
            }
            Err(error) => Err(ScheduleSubmitError {
                error,
                goal_id: None,
            }),
        };
        if let Err(ScheduleSubmitError { error, goal_id }) = result {
            warn!(
                schedule_id = %claim.schedule.schedule_id,
                thread_id = %thread_id,
                "failed to submit scheduled thread run: {error}"
            );
            if let Some(wait) = error.downcast_ref::<ScheduleUsageProfileWait>() {
                self.defer_claimed_run_for_usage_profile_wait(state_db, claim, wait.clone())
                    .await;
                return;
            }
            if let Some(deferral) = error.downcast_ref::<ScheduleRunDeferral>() {
                self.defer_claimed_run(state_db, claim, deferral.clone())
                    .await;
                return;
            }
            self.fail_claimed_run_after_submit_error(
                state_db,
                claim,
                goal_id,
                schedule_submit_error(&error),
            )
            .await;
        }
    }

    async fn submit_claimed_schedule(
        &self,
        thread_id: ThreadId,
        state_db: StateDbHandle,
        claim: &codex_state::ThreadScheduleClaim,
        prompt: String,
        scheduled_goal_objective: Option<String>,
    ) -> Result<(), ScheduleSubmitError> {
        let claim_auth_profile = match claim.occurrence_auth_profile.clone() {
            Some(auth_profile) => Some(auth_profile),
            None => {
                self.claim_auth_profile(&state_db, thread_id, &claim.schedule)
                    .await
            }
        };
        let claim_auth_profile = if matches!(
            claim.occurrence_state,
            codex_state::ThreadScheduleOccurrenceState::Enqueued
                | codex_state::ThreadScheduleOccurrenceState::Started
        ) {
            claim_auth_profile
        } else {
            let broker_decision = super::usage_profile_broker::resolve_dispatch_auth_profile(
                &self.auth_manager,
                &self.config,
                claim_auth_profile.clone(),
            )
            .await;
            match schedule_auth_profile_after_broker_decision(
                claim_auth_profile,
                broker_decision,
                self.config.usage_self_heal.reset_retry_buffer_secs,
                Utc::now(),
            ) {
                Ok(resolved) => resolved,
                Err(wait) => {
                    return Err(ScheduleSubmitError {
                        error: anyhow::Error::new(wait),
                        goal_id: None,
                    });
                }
            }
        };
        let thread = self
            .load_or_resume_thread(thread_id, claim_auth_profile.clone())
            .await
            .map_err(|error| ScheduleSubmitError {
                error,
                goal_id: None,
            })?;
        self.ensure_schedule_listener(thread_id, thread.clone())
            .await
            .map_err(|error| ScheduleSubmitError {
                error,
                goal_id: None,
            })?;
        let thread_state = self.thread_state_manager.thread_state(thread_id).await;
        let listener_command_tx = {
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        let (turn_prompt, scheduled_goal_id) = if let Some(turn_input) = claim.turn_input.clone() {
            (turn_input, claim.run.goal_id.clone())
        } else if let Some(objective) = scheduled_goal_objective.as_deref() {
            let scheduled_goal_id = self
                .prepare_scheduled_goal(
                    thread_id,
                    &state_db,
                    objective,
                    listener_command_tx.clone(),
                )
                .await
                .map_err(|error| {
                    let goal_id = error
                        .downcast_ref::<ScheduledGoalHeld>()
                        .map(|held| held.goal_id.clone());
                    ScheduleSubmitError { error, goal_id }
                })?;
            (
                scheduled_goal_thread_prompt(
                    objective,
                    claim.run.run_id.as_str(),
                    claim.run.scheduled_for,
                    &claim.schedule,
                ),
                Some(scheduled_goal_id),
            )
        } else {
            (
                scheduled_thread_prompt(
                    &prompt,
                    &claim.schedule,
                    claim.run.run_id.as_str(),
                    claim.run.scheduled_for,
                ),
                None,
            )
        };
        let thread_settings = scheduled_thread_settings_from_snapshot(
            thread.config_snapshot().await,
            claim_auth_profile.clone(),
        );
        let turn_id = claim
            .run
            .turn_id
            .clone()
            .ok_or_else(|| ScheduleSubmitError {
                error: anyhow::anyhow!(
                    "claimed schedule occurrence {} has no stable turn identifier",
                    claim.run.run_id
                ),
                goal_id: scheduled_goal_id.clone(),
            })?;
        let run = if claim.occurrence_state == codex_state::ThreadScheduleOccurrenceState::Started {
            claim.run.clone()
        } else {
            state_db
                .thread_schedules()
                .enqueue_thread_schedule_run(codex_state::ThreadScheduleRunEnqueueParams {
                    schedule_id: claim.schedule.schedule_id.as_str(),
                    run_id: claim.run.run_id.as_str(),
                    lease_id: claim.run.lease_id.as_str(),
                    goal_id: scheduled_goal_id.as_deref(),
                    auth_profile_recorded: claim_auth_profile.is_some(),
                    auth_profile: claim_auth_profile.as_ref().and_then(Option::as_deref),
                    turn_input: turn_prompt.as_str(),
                    now: Utc::now(),
                })
                .await
                .map_err(|error| ScheduleSubmitError {
                    error,
                    goal_id: scheduled_goal_id.clone(),
                })?
                .ok_or_else(|| ScheduleSubmitError {
                    error: anyhow::anyhow!(
                        "claimed schedule occurrence {} lost ownership before enqueue",
                        claim.run.run_id
                    ),
                    goal_id: scheduled_goal_id.clone(),
                })?
        };
        let ownership_lost = match self.start_lease_heartbeat(state_db.clone(), &run).await {
            Ok(Some(ownership_lost)) => ownership_lost,
            Ok(None) => {
                return Err(ScheduleSubmitError {
                    error: anyhow::anyhow!(
                        "claimed schedule run {} lost lease ownership before dispatch readiness",
                        claim.run.run_id
                    ),
                    goal_id: scheduled_goal_id,
                });
            }
            Err(error) => {
                return Err(ScheduleSubmitError {
                    error,
                    goal_id: scheduled_goal_id,
                });
            }
        };
        {
            let mut thread_state = thread_state.lock().await;
            thread_state.track_scheduled_run(
                turn_id.clone(),
                crate::thread_state::ScheduledThreadScheduleRun {
                    schedule_id: run.schedule_id.clone(),
                    run_id: run.run_id.clone(),
                    lease_id: run.lease_id.clone(),
                    goal_id: run.goal_id.clone(),
                    state_db: state_db.clone(),
                },
            );
        }
        let lease_is_owned = state_db
            .thread_schedules()
            .extend_thread_schedule_lease(codex_state::ThreadScheduleRunLeaseParams {
                schedule_id: run.schedule_id.as_str(),
                run_id: run.run_id.as_str(),
                lease_id: run.lease_id.as_str(),
                now: Utc::now(),
                lease_duration: SCHEDULE_LEASE_DURATION,
            })
            .await
            .map_err(|error| ScheduleSubmitError {
                error,
                goal_id: scheduled_goal_id.clone(),
            })?;
        if !lease_is_owned || ownership_lost.is_cancelled() {
            thread_state
                .lock()
                .await
                .take_scheduled_run(turn_id.as_str());
            return Err(ScheduleSubmitError {
                error: anyhow::anyhow!(
                    "claimed schedule run {} lost lease ownership before turn submission",
                    claim.run.run_id
                ),
                goal_id: scheduled_goal_id,
            });
        }
        // Once core reserves the idle turn, await it to completion. Dropping
        // this future on a concurrent heartbeat cancellation could strand both
        // the idle reservation and a newly persisted Started occurrence.
        let start_result = thread
            .try_start_scheduled_user_input_turn_if_idle(
                turn_id.clone(),
                vec![CoreInputItem::Text {
                    text: turn_prompt,
                    text_elements: Vec::new(),
                }],
                Default::default(),
                thread_settings,
                codex_core::ScheduledTurnStart {
                    schedule_id: run.schedule_id.clone(),
                    run_id: run.run_id.clone(),
                    lease_id: run.lease_id.clone(),
                    goal_id: run.goal_id.clone(),
                    lease_duration: SCHEDULE_LEASE_DURATION,
                },
            )
            .await;
        let run = match start_result {
            Ok(run) => run,
            Err(err) => {
                thread_state
                    .lock()
                    .await
                    .take_scheduled_run(turn_id.as_str());
                if let Some(deferral) = schedule_deferral_for_idle_rejection(&err, Utc::now()) {
                    return Err(ScheduleSubmitError {
                        error: anyhow::Error::new(deferral),
                        goal_id: scheduled_goal_id,
                    });
                }
                return Err(ScheduleSubmitError {
                    error: anyhow::anyhow!("failed to start scheduled prompt: {err}"),
                    goal_id: scheduled_goal_id,
                });
            }
        };
        self.emit_schedule_run_updated(thread_id, run).await;
        Ok(())
    }

    async fn replay_terminal_claim(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
    ) {
        let completed_at = claim.run.completed_at.unwrap_or_else(Utc::now);
        let error = (claim.run.status == codex_state::ThreadScheduleRunStatus::Failed).then(|| {
            claim
                .run
                .error
                .clone()
                .unwrap_or_else(|| "scheduled turn failed without a recorded error".to_string())
        });
        match finish_scheduled_run_state(
            &state_db,
            claim.schedule.schedule_id.as_str(),
            claim.run.run_id.as_str(),
            claim.run.lease_id.as_str(),
            claim.run.goal_id.as_deref(),
            error,
            completed_at,
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
                "failed to replay terminal scheduled occurrence finalization: {err}"
            ),
        }
    }

    async fn persisted_terminal_scheduled_turn(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        turn_id: &str,
    ) -> anyhow::Result<Option<PersistedScheduledTurnTerminal>> {
        let rollout_path = codex_rollout::find_thread_path_by_id_str(
            &self.config.codex_home,
            &thread_id.to_string(),
            Some(state_db),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("thread rollout not found for {thread_id}"))?;
        let history = codex_rollout::RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .with_context(|| {
                format!(
                    "failed to load rollout {} for scheduled turn recovery",
                    rollout_path.display()
                )
            })?;
        Ok(persisted_scheduled_turn_terminal(
            &history,
            turn_id,
            Utc::now(),
        ))
    }

    async fn held_started_scheduled_goal(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        goal_id: Option<&str>,
    ) -> anyhow::Result<Option<ScheduledGoalHeld>> {
        let Some(goal_id) = goal_id else {
            return Ok(None);
        };
        let goal = state_db.thread_goals().get_thread_goal(thread_id).await?;
        Ok(goal
            .filter(|goal| goal.goal_id == goal_id && scheduled_goal_status_is_held(goal.status))
            .map(|goal| ScheduledGoalHeld {
                goal_id: goal.goal_id,
                status: goal.status,
            }))
    }

    async fn finish_claim_from_persisted_terminal(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
        terminal: PersistedScheduledTurnTerminal,
    ) {
        match finish_scheduled_run_state(
            &state_db,
            claim.schedule.schedule_id.as_str(),
            claim.run.run_id.as_str(),
            claim.run.lease_id.as_str(),
            claim.run.goal_id.as_deref(),
            terminal.error,
            terminal.completed_at,
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
                "failed to finalize scheduled occurrence from durable terminal turn: {err}"
            ),
        }
    }
}
