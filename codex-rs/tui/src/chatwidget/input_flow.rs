//! User input submission, queue draining, and draft restore flow for `ChatWidget`.
//!
//! The queue data itself lives in `input_queue`; this module owns the app-level
//! effects around taking composer input, submitting user turns, draining queued
//! follow-ups, and restoring draft state across interrupts or thread switches.

use super::*;

/// Remove the `count` entries that a flush merged, skipping any entries the submission
/// itself pushed onto the front of `queue` after the merge snapshot was taken.
fn drain_flushed_entries<T>(
    queue: &mut std::collections::VecDeque<T>,
    len_before_submit: usize,
    count: usize,
) {
    let added = queue.len().saturating_sub(len_before_submit);
    let end = added.saturating_add(count).min(queue.len());
    if added < end {
        queue.drain(added..end);
    }
}

impl ChatWidget {
    pub(super) fn handle_composer_input_result(
        &mut self,
        input_result: InputResult,
        had_modal_or_popup: bool,
    ) {
        match input_result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                let user_message = self.user_message_from_submission(text, text_elements);
                if user_message.text.is_empty()
                    && user_message.local_images.is_empty()
                    && user_message.remote_image_urls.is_empty()
                {
                    return;
                }
                let should_submit_now =
                    self.is_session_configured() && !self.is_plan_streaming_in_tui();
                if should_submit_now {
                    if self.only_user_shell_commands_running()
                        && !user_message.text.starts_with('!')
                    {
                        self.queue_user_message(user_message);
                        return;
                    }
                    // Submitted is emitted when user submits.
                    // Reset any reasoning header only when we are actually submitting a turn.
                    self.reasoning_buffer.clear();
                    self.reasoning_header = None;
                    self.reasoning_summary_parts.clear();
                    self.set_status_header(String::from("Working"));
                    self.submit_user_message(user_message);
                } else {
                    self.queue_user_message(user_message);
                }
            }
            InputResult::Queued {
                text,
                text_elements,
                action,
            } => {
                let user_message = self.user_message_from_submission(text, text_elements);
                self.queue_user_message_with_options(user_message, action);
            }
            InputResult::Command(cmd) => {
                self.handle_slash_command_dispatch(cmd);
            }
            InputResult::ServiceTierCommand(command) => {
                self.handle_service_tier_command_dispatch(command);
            }
            InputResult::ServiceTierCommandWithArgs(command, args) => {
                self.handle_service_tier_command_with_args_dispatch(command, args);
            }
            InputResult::CommandWithArgs(cmd, args, text_elements) => {
                self.handle_slash_command_with_args_dispatch(cmd, args, text_elements);
            }
            InputResult::None => {}
        }
        if had_modal_or_popup && self.bottom_pane.no_modal_or_popup_active() {
            self.maybe_send_next_queued_input();
        }
        self.refresh_plan_mode_nudge();
    }

    pub(super) fn queue_user_message(&mut self, user_message: UserMessage) {
        self.queue_user_message_with_options(user_message, QueuedInputAction::Plain);
    }

    pub(crate) fn set_queue_submissions_until_session_configured(&mut self, queue: bool) {
        self.bottom_pane
            .set_queue_submissions(queue && !self.is_session_configured());
    }

    pub(super) fn queue_user_message_with_options(
        &mut self,
        user_message: UserMessage,
        action: QueuedInputAction,
    ) {
        if !self.is_session_configured() || self.is_user_turn_pending_or_running() {
            self.input_queue
                .queued_user_messages
                .push_back(QueuedUserMessage::new(user_message, action));
            self.input_queue
                .queued_user_message_history_records
                .push_back(UserMessageHistoryRecord::UserMessageText);
            self.refresh_pending_input_preview();
        } else {
            self.submit_user_message(user_message);
        }
    }

    /// Merge the queued follow-ups visible at this keypress into one active-turn steer.
    ///
    /// Rejected steers retain dequeue priority, followed by locally queued messages in FIFO
    /// order, which preserves each queue's message/history alignment. Queued `/slash` and
    /// `!shell` entries are dispatch actions rather than prose, so the merge stops at the
    /// first one and leaves it (and everything behind it) queued for the normal drain.
    /// Already-submitted pending steers are left untouched.
    ///
    /// The merge is built from clones and the queues are only drained once the submission is
    /// accepted. Submission can legitimately be refused (unavailable thread model, blocked
    /// image attachments, a closed op channel), and those refusal paths push the attempted
    /// message back into the composer, so the live draft is snapshotted and restored too.
    /// Draining up front would silently destroy every queued follow-up and overwrite whatever
    /// the user was still typing.
    pub(super) fn flush_queued_messages(&mut self) {
        let rejected_count = self.input_queue.rejected_steers_queue.len();
        let mut rejected_history_records = self
            .input_queue
            .rejected_steer_history_records
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        rejected_history_records.resize(rejected_count, UserMessageHistoryRecord::UserMessageText);

        let mut messages = self
            .input_queue
            .rejected_steers_queue
            .iter()
            .cloned()
            .zip(rejected_history_records)
            .collect::<Vec<_>>();
        let mut queued_count = 0usize;
        while let Some(queued_message) = self.input_queue.queued_user_messages.get(queued_count) {
            if !matches!(queued_message.action, QueuedInputAction::Plain) {
                break;
            }
            let user_message = queued_message.clone().into_user_message();
            let history_record = self
                .input_queue
                .queued_user_message_history_records
                .get(queued_count)
                .cloned()
                .unwrap_or(UserMessageHistoryRecord::UserMessageText);
            messages.push((user_message, history_record));
            queued_count += 1;
        }

        if messages.is_empty() {
            return;
        }

        let (message, history_record) = merge_user_messages_with_history_record(messages);
        let actual_chars = message.text.chars().count();
        if actual_chars > MAX_USER_INPUT_TEXT_CHARS {
            self.add_error_message(format!(
                "Queued messages exceed the maximum combined length of \
                 {MAX_USER_INPUT_TEXT_CHARS} characters ({actual_chars} provided). \
                 Edit or remove queued messages before submitting them together."
            ));
            return;
        }

        let composer_snapshot = self.bottom_pane.composer_draft_snapshot();
        let rejected_len_before = self.input_queue.rejected_steers_queue.len();
        let rejected_history_len_before = self.input_queue.rejected_steer_history_records.len();
        let queued_len_before = self.input_queue.queued_user_messages.len();
        let queued_history_len_before = self.input_queue.queued_user_message_history_records.len();

        let accepted = self.resubmit_queued_user_message_with_history_record(
            message,
            history_record,
            ShellEscapePolicy::Disallow,
        );

        if accepted {
            // An accepted submission can still be re-queued at the *front* while session,
            // auth-profile, or usage-limit state resolves. Remove only the original entries
            // that were merged, skipping anything the submission itself pushed on top.
            drain_flushed_entries(
                &mut self.input_queue.rejected_steers_queue,
                rejected_len_before,
                rejected_count,
            );
            drain_flushed_entries(
                &mut self.input_queue.rejected_steer_history_records,
                rejected_history_len_before,
                rejected_count,
            );
            drain_flushed_entries(
                &mut self.input_queue.queued_user_messages,
                queued_len_before,
                queued_count,
            );
            drain_flushed_entries(
                &mut self.input_queue.queued_user_message_history_records,
                queued_history_len_before,
                queued_count,
            );
        } else {
            // The refusal paths (`restore_user_message_to_composer`,
            // `restore_blocked_image_submission`) replaced the draft with the merged text.
            self.bottom_pane
                .restore_composer_draft_snapshot(composer_snapshot);
        }
        self.refresh_pending_input_preview();
    }

    /// If idle and there are queued inputs, submit exactly one to start the next turn.
    pub(crate) fn maybe_send_next_queued_input(&mut self) -> bool {
        if self.input_queue.suppress_queue_autosend {
            return false;
        }
        if self.is_user_turn_pending_or_running() {
            return false;
        }
        let mut submitted_follow_up = false;
        while !self.is_user_turn_pending_or_running() {
            let Some((queued_message, history_record)) = self.pop_next_queued_user_message() else {
                break;
            };
            match queued_message.action {
                QueuedInputAction::Plain => {
                    let shell_escape_policy = queued_message.shell_escape_policy;
                    submitted_follow_up = self.resubmit_queued_user_message_with_history_record(
                        queued_message.into_user_message(),
                        history_record,
                        shell_escape_policy,
                    );
                    break;
                }
                QueuedInputAction::ParseSlash => {
                    let drain = self.submit_queued_slash_prompt(queued_message.into_user_message());
                    if drain == QueueDrain::Stop {
                        submitted_follow_up = self.is_user_turn_pending_or_running();
                        break;
                    }
                }
                QueuedInputAction::RunShell => {
                    let drain = self.submit_queued_shell_prompt(queued_message.into_user_message());
                    if drain == QueueDrain::Stop {
                        submitted_follow_up = self.is_user_turn_pending_or_running();
                        break;
                    }
                }
            }
        }
        // Update the list to reflect the remaining queued messages (if any).
        self.refresh_pending_input_preview();
        submitted_follow_up
    }

    pub(super) fn is_user_turn_pending_or_running(&self) -> bool {
        self.input_queue.user_turn_pending_start
            || self.bottom_pane.is_task_running()
            || self.automatic_usage_limit_reset_owns_failed_turn()
    }

    pub(super) fn only_user_shell_commands_running(&self) -> bool {
        self.turn_lifecycle.agent_turn_running
            && !self.running_commands.is_empty()
            && self
                .running_commands
                .values()
                .all(|command| command.source == ExecCommandSource::UserShell)
    }

    /// Rebuild and update the bottom-pane pending-input preview.
    pub(super) fn refresh_pending_input_preview(&mut self) {
        let preview = self.input_queue.preview();
        let flush_available = self.turn_lifecycle.agent_turn_running
            && self.input_queue.has_flushable_queued_messages();
        self.bottom_pane.set_pending_input_preview(
            preview.queued_messages,
            preview.pending_steers,
            preview.rejected_steers,
            flush_available,
        );
    }

    pub(crate) fn submit_user_message_with_mode(
        &mut self,
        text: String,
        mut collaboration_mode: CollaborationModeMask,
    ) {
        if collaboration_mode.mode == Some(ModeKind::Plan)
            && let Some(effort) = self.config.plan_mode_reasoning_effort.clone()
        {
            collaboration_mode.reasoning_effort = Some(Some(effort));
        }
        if self.turn_lifecycle.agent_turn_running
            && self.active_collaboration_mask.as_ref() != Some(&collaboration_mode)
        {
            self.add_error_message(
                "Cannot switch collaboration mode while a turn is running.".to_string(),
            );
            return;
        }
        self.set_collaboration_mask_from_user_action(collaboration_mode);
        let should_queue = self.is_plan_streaming_in_tui();
        let user_message = UserMessage {
            text,
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        };
        if should_queue {
            self.queue_user_message(user_message);
        } else {
            self.submit_user_message(user_message);
        }
    }

    #[cfg(test)]
    pub(crate) fn queued_user_message_texts(&self) -> Vec<String> {
        self.input_queue
            .rejected_steers_queue
            .iter()
            .map(|message| message.text.clone())
            .chain(
                self.input_queue
                    .queued_user_messages
                    .iter()
                    .map(|message| message.text.clone()),
            )
            .collect()
    }
}
