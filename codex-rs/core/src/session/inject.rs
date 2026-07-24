use super::input_queue::TurnInput;
use super::session::Session;
use super::session::SessionSettingsUpdate;
use super::turn_context::TurnContext;
use crate::codex_thread::ScheduledUserInputTurnStart;
use crate::codex_thread::TryStartTurnIfIdleError;
use crate::codex_thread::TryStartTurnIfIdleRejectionReason;
use crate::codex_thread::TryStartUserInputTurnIfIdleError;
use crate::state::ActiveTurn;
use crate::state::TurnState;
use crate::tasks::RegularTask;
use codex_protocol::config_types::ModeKind;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AdditionalContextEntry;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::user_input::UserInput;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

impl Session {
    /// Returns the input if there is no active turn to inject into.
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "active turn checks and turn state updates must remain atomic"
    )]
    pub async fn inject_if_running(
        &self,
        input: Vec<ResponseItem>,
    ) -> Result<(), Vec<ResponseItem>> {
        let mut active = self.active_turn.lock().await;
        match active.as_mut() {
            Some(active_turn) => {
                self.input_queue
                    .extend_pending_input_for_turn_state(
                        active_turn.turn_state.as_ref(),
                        input.into_iter().map(TurnInput::ResponseItem).collect(),
                    )
                    .await;
                Ok(())
            }
            None => Err(input),
        }
    }

    /// Starts a regular turn with the provided items only if automatic idle work
    /// is allowed for the current session state.
    ///
    /// This is the shared gate for extension-initiated idle work. It refuses to
    /// start a turn when user/client-triggered work is queued, any task is still
    /// active, or the session is currently in Plan mode. Active Review tasks are
    /// covered by the active-task check because Review turns are not steerable.
    pub(crate) async fn try_start_turn_if_idle(
        self: &Arc<Self>,
        input: Vec<ResponseItem>,
    ) -> Result<(), TryStartTurnIfIdleError> {
        if input.is_empty() {
            return Ok(());
        }
        if self.input_queue.has_trigger_turn_mailbox_items().await {
            return Err(TryStartTurnIfIdleError::new(
                TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
                input,
            ));
        }
        if self.collaboration_mode().await.mode == ModeKind::Plan {
            return Err(TryStartTurnIfIdleError::new(
                TryStartTurnIfIdleRejectionReason::PlanMode,
                input,
            ));
        }

        let turn_state = {
            let mut active_turn = self.active_turn.lock().await;
            if active_turn.is_some() {
                return Err(TryStartTurnIfIdleError::new(
                    TryStartTurnIfIdleRejectionReason::Busy,
                    input,
                ));
            }
            let active_turn = active_turn.get_or_insert_with(ActiveTurn::default);
            Arc::clone(&active_turn.turn_state)
        };

        if self.input_queue.has_trigger_turn_mailbox_items().await {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            return Err(TryStartTurnIfIdleError::new(
                TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
                input,
            ));
        }

        let mut turn_context = self
            .new_default_turn_with_sub_id(uuid::Uuid::new_v4().to_string())
            .await;
        if let Some(turn_context) = Arc::get_mut(&mut turn_context) {
            turn_context.enforce_context_window_before_sampling = true;
            turn_context.bound_headless_tool_outputs_for_prompt = true;
        }
        if turn_context.collaboration_mode.mode == ModeKind::Plan {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            return Err(TryStartTurnIfIdleError::new(
                TryStartTurnIfIdleRejectionReason::PlanMode,
                input,
            ));
        }
        self.maybe_emit_unknown_model_warning_for_turn(turn_context.as_ref())
            .await;
        if self.input_queue.has_trigger_turn_mailbox_items().await {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            return Err(TryStartTurnIfIdleError::new(
                TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
                input,
            ));
        }
        let still_reserved = {
            let active_turn = self.active_turn.lock().await;
            active_turn.as_ref().is_some_and(|active_turn| {
                active_turn.task.is_none() && Arc::ptr_eq(&active_turn.turn_state, &turn_state)
            })
        };
        if !still_reserved {
            self.clear_reserved_idle_turn(&turn_state).await;
            return Err(TryStartTurnIfIdleError::new(
                TryStartTurnIfIdleRejectionReason::Busy,
                input,
            ));
        }

        self.input_queue
            .extend_pending_input_for_turn_state(
                turn_state.as_ref(),
                input.into_iter().map(TurnInput::ResponseItem).collect(),
            )
            .await;
        self.start_task(turn_context, Vec::new(), RegularTask::new())
            .await;
        Ok(())
    }

    /// Starts a regular user-input turn with explicit settings only if the
    /// session is idle. Callers that require a distinct turn lifecycle should
    /// use this instead of submitting `Op::UserInput`, which may steer input
    /// into an active turn.
    pub(crate) async fn try_start_user_input_turn_if_idle(
        self: &Arc<Self>,
        sub_id: String,
        input: Vec<UserInput>,
        additional_context: BTreeMap<String, AdditionalContextEntry>,
        updates: SessionSettingsUpdate,
    ) -> Result<String, TryStartUserInputTurnIfIdleError> {
        self.try_start_user_input_turn_if_idle_inner(
            sub_id,
            input,
            additional_context,
            updates,
            None,
        )
        .await
        .map(|outcome| match outcome {
            UserInputTurnStartOutcome::Started(turn_id) => turn_id,
            UserInputTurnStartOutcome::Scheduled { .. } => {
                unreachable!("plain idle turn does not materialize a schedule run")
            }
        })
    }

    pub(crate) async fn try_start_scheduled_user_input_turn_if_idle(
        self: &Arc<Self>,
        materialization: codex_state::ThreadScheduleRunStartParams<'_>,
        materialized_turn_input: String,
        input: Vec<UserInput>,
        additional_context: BTreeMap<String, AdditionalContextEntry>,
        updates: SessionSettingsUpdate,
    ) -> Result<ScheduledUserInputTurnStart, TryStartUserInputTurnIfIdleError> {
        self.try_start_user_input_turn_if_idle_inner(
            materialization.turn_id.to_string(),
            input,
            additional_context,
            updates,
            Some((materialization, materialized_turn_input)),
        )
        .await
        .map(|outcome| match outcome {
            UserInputTurnStartOutcome::Scheduled { run, start_gate } => {
                ScheduledUserInputTurnStart::new(run, start_gate)
            }
            UserInputTurnStartOutcome::Started(_) => {
                unreachable!("scheduled idle turn must materialize its run")
            }
        })
    }

    async fn try_start_user_input_turn_if_idle_inner(
        self: &Arc<Self>,
        sub_id: String,
        input: Vec<UserInput>,
        additional_context: BTreeMap<String, AdditionalContextEntry>,
        updates: SessionSettingsUpdate,
        schedule_materialization: Option<(codex_state::ThreadScheduleRunStartParams<'_>, String)>,
    ) -> Result<UserInputTurnStartOutcome, TryStartUserInputTurnIfIdleError> {
        if input.is_empty() {
            return Err(TryStartUserInputTurnIfIdleError::EmptyInput);
        }
        if self.input_queue.has_trigger_turn_mailbox_items().await {
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
            ));
        }
        if self.collaboration_mode().await.mode == ModeKind::Plan {
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::PlanMode,
            ));
        }
        if let Ok(snapshot) = self.preview_settings(&updates).await
            && snapshot.collaboration_mode.mode == ModeKind::Plan
        {
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::PlanMode,
            ));
        }

        let turn_state = {
            let mut active_turn = self.active_turn.lock().await;
            if active_turn.is_some() {
                return Err(TryStartUserInputTurnIfIdleError::Rejected(
                    TryStartTurnIfIdleRejectionReason::Busy,
                ));
            }
            let active_turn = active_turn.get_or_insert_with(ActiveTurn::default);
            Arc::clone(&active_turn.turn_state)
        };

        if self.input_queue.has_trigger_turn_mailbox_items().await {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
            ));
        }
        let still_reserved = {
            let active_turn = self.active_turn.lock().await;
            active_turn.as_ref().is_some_and(|active_turn| {
                active_turn.task.is_none() && Arc::ptr_eq(&active_turn.turn_state, &turn_state)
            })
        };
        if !still_reserved {
            self.clear_reserved_idle_turn(&turn_state).await;
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::Busy,
            ));
        }
        if self.input_queue.has_trigger_turn_mailbox_items().await {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::PendingTriggerTurn,
            ));
        }

        let materialized_run = match schedule_materialization.as_ref() {
            Some((materialization, materialized_turn_input)) => {
                let result = match self.state_db() {
                    Some(state_db) => {
                        state_db
                            .thread_schedules()
                            .materialize_thread_schedule_run(
                                materialization.clone(),
                                Some(materialized_turn_input.as_str()),
                            )
                            .await
                    }
                    None => Err(anyhow::anyhow!(
                        "sqlite state db unavailable for scheduled turn materialization"
                    )),
                };
                match result {
                    Ok(Some(run)) => Some(run),
                    Ok(None) => {
                        self.clear_reserved_idle_turn(&turn_state).await;
                        self.maybe_start_turn_for_pending_work().await;
                        return Err(TryStartUserInputTurnIfIdleError::State(anyhow::anyhow!(
                            "scheduled turn no longer owns the current unexpired lease"
                        )));
                    }
                    Err(err) => {
                        self.clear_reserved_idle_turn(&turn_state).await;
                        self.maybe_start_turn_for_pending_work().await;
                        return Err(TryStartUserInputTurnIfIdleError::State(err));
                    }
                }
            }
            None => None,
        };
        let scheduled_turn_materialized = materialized_run.is_some();
        let mut turn_context = match self.new_turn_with_sub_id(sub_id.clone(), updates).await {
            Ok(turn_context) => turn_context,
            Err(err) => {
                self.clear_reserved_idle_turn(&turn_state).await;
                self.maybe_start_turn_for_pending_work().await;
                if scheduled_turn_materialized {
                    return Err(TryStartUserInputTurnIfIdleError::State(anyhow::anyhow!(
                        "failed to construct materialized scheduled turn: {err}"
                    )));
                }
                return Err(TryStartUserInputTurnIfIdleError::InvalidRequest(err));
            }
        };
        if let Some(turn_context) = Arc::get_mut(&mut turn_context) {
            turn_context.enforce_context_window_before_sampling = true;
            turn_context.bound_headless_tool_outputs_for_prompt = true;
        }
        if turn_context.collaboration_mode.mode == ModeKind::Plan {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            if scheduled_turn_materialized {
                return Err(TryStartUserInputTurnIfIdleError::State(anyhow::anyhow!(
                    "materialized scheduled turn resolved to plan mode"
                )));
            }
            return Err(TryStartUserInputTurnIfIdleError::Rejected(
                TryStartTurnIfIdleRejectionReason::PlanMode,
            ));
        }
        self.maybe_emit_unknown_model_warning_for_turn(turn_context.as_ref())
            .await;

        let additional_context_input = {
            let mut state = self.state.lock().await;
            state.additional_context.merge(additional_context)
        };
        let mut task_input = additional_context_input
            .into_iter()
            .map(ResponseItem::from)
            .map(TurnInput::ResponseItem)
            .collect::<Vec<_>>();
        task_input.push(TurnInput::UserInput {
            content: input,
            client_id: None,
        });
        let Some(materialized_run) = materialized_run else {
            self.start_task(turn_context, task_input, RegularTask::new())
                .await;
            return Ok(UserInputTurnStartOutcome::Started(sub_id));
        };
        let Some(start_gate) = self
            .start_task_blocked(
                &turn_state,
                Arc::clone(&turn_context),
                task_input,
                RegularTask::with_persisted_turn_started(),
            )
            .await
        else {
            self.clear_reserved_idle_turn(&turn_state).await;
            self.maybe_start_turn_for_pending_work().await;
            return Err(TryStartUserInputTurnIfIdleError::State(anyhow::anyhow!(
                "materialized scheduled turn lost its exact idle reservation"
            )));
        };
        let turn_started_at_unix_ms = turn_context
            .turn_timing_state
            .mark_turn_started(Instant::now())
            .await;
        turn_context
            .turn_metadata_state
            .set_turn_started_at_unix_ms(turn_started_at_unix_ms);
        let turn_started = EventMsg::TurnStarted(TurnStartedEvent {
            turn_id: turn_context.sub_id.clone(),
            trace_id: turn_context.trace_id.clone(),
            started_at: turn_context.turn_timing_state.started_at_unix_secs().await,
            model_context_window: turn_context.model_context_window(),
            collaboration_mode_kind: turn_context.collaboration_mode.mode,
        });
        if let Err(err) = self
            .send_event_strict(turn_context.as_ref(), turn_started)
            .await
        {
            self.clear_blocked_task_without_turn_effect(turn_context.sub_id.as_str())
                .await;
            return Err(TryStartUserInputTurnIfIdleError::State(err));
        }
        Ok(UserInputTurnStartOutcome::Scheduled {
            run: materialized_run,
            start_gate,
        })
    }

    async fn clear_reserved_idle_turn(&self, turn_state: &Arc<tokio::sync::Mutex<TurnState>>) {
        let mut active_turn_guard = self.active_turn.lock().await;
        if let Some(active_turn) = active_turn_guard.as_ref()
            && active_turn.task.is_none()
            && Arc::ptr_eq(&active_turn.turn_state, turn_state)
        {
            *active_turn_guard = None;
        }
    }

    /// Injects items into active work, or records them without starting a turn.
    pub(crate) async fn inject_no_new_turn(
        &self,
        items: Vec<ResponseItem>,
        current_turn_context: Option<&TurnContext>,
    ) {
        let Err(items) = self.inject_if_running(items).await else {
            return;
        };
        let default_turn_context;
        let turn_context = match current_turn_context {
            Some(turn_context) => turn_context,
            None => {
                default_turn_context = self.new_default_turn().await;
                default_turn_context.as_ref()
            }
        };
        self.record_conversation_items(turn_context, &items).await;
    }
}

enum UserInputTurnStartOutcome {
    Started(String),
    Scheduled {
        run: codex_state::ThreadScheduleRun,
        start_gate: crate::tasks::TaskStartGateHandle,
    },
}
