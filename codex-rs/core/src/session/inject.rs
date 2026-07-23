use super::input_queue::TurnInput;
use super::session::Session;
use super::session::SessionSettingsUpdate;
use super::turn_context::TurnContext;
use crate::codex_thread::TryStartTurnIfIdleError;
use crate::codex_thread::TryStartTurnIfIdleRejectionReason;
use crate::codex_thread::TryStartUserInputTurnIfIdleError;
use crate::state::ActiveTurn;
use crate::state::TurnState;
use crate::tasks::RegularTask;
use codex_protocol::config_types::ModeKind;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AdditionalContextEntry;
use codex_protocol::user_input::UserInput;
use std::collections::BTreeMap;
use std::sync::Arc;

impl Session {
    pub(crate) async fn record_session_continuation_if_idle(
        self: &Arc<Self>,
        summary: String,
    ) -> CodexResult<()> {
        if self.input_queue.has_trigger_turn_mailbox_items().await {
            return Err(CodexErr::InvalidRequest(
                "destination thread has pending work".to_string(),
            ));
        }

        let turn_state = {
            let mut active_turn = self.active_turn.lock().await;
            if active_turn.is_some() {
                return Err(CodexErr::InvalidRequest(
                    "destination thread became active before continuation was recorded".to_string(),
                ));
            }
            let active_turn = active_turn.get_or_insert_with(ActiveTurn::default);
            Arc::clone(&active_turn.turn_state)
        };

        let result = async {
            if self.input_queue.has_trigger_turn_mailbox_items().await {
                return Err(CodexErr::InvalidRequest(
                    "destination thread received pending work before continuation was recorded"
                        .to_string(),
                ));
            }
            let turn_context = self.new_default_turn().await;
            if self.reference_context_item().await.is_none() {
                self.record_context_updates_and_set_reference_context_item(turn_context.as_ref())
                    .await;
            }
            let response_item = ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text: summary }],
                phase: None,
            };
            self.record_response_item_and_emit_turn_item(turn_context.as_ref(), response_item)
                .await;
            self.flush_rollout().await?;
            Ok(())
        }
        .await;

        self.clear_reserved_idle_turn(&turn_state).await;
        self.maybe_start_turn_for_pending_work().await;
        result
    }

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

        let mut turn_context = match self.new_turn_with_sub_id(sub_id.clone(), updates).await {
            Ok(turn_context) => turn_context,
            Err(err) => {
                self.clear_reserved_idle_turn(&turn_state).await;
                self.maybe_start_turn_for_pending_work().await;
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
        self.start_task(turn_context, task_input, RegularTask::new())
            .await;
        Ok(sub_id)
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
