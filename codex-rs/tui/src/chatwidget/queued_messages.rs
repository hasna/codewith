//! Queued-message management for `/queued`.

use super::*;
use codex_app_server_protocol::ThreadQueuedMessage;
use codex_app_server_protocol::ThreadQueuedMessageListResponse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalQueuedMessageMoveDirection {
    Up,
    Down,
}

impl ChatWidget {
    pub(crate) fn show_queued_messages(
        &mut self,
        agent_messages: Option<ThreadQueuedMessageListResponse>,
    ) {
        let retry = self.rejected_steer_rows();
        let local = self.local_queued_message_rows();
        let mut lines = vec![Line::from("Queued messages".bold())];
        let agent_total = agent_messages
            .as_ref()
            .map(|response| response.stats.total)
            .unwrap_or_default();
        let agent_trigger_turn = agent_messages
            .as_ref()
            .map(|response| response.stats.trigger_turn)
            .unwrap_or_default();

        if retry.is_empty() && local.is_empty() && agent_total == 0 {
            lines.push("No queued messages.".dim().into());
            lines.push(queued_usage_line());
            self.add_plain_history_lines(lines);
            return;
        }

        lines.push(
            format!(
                "Stats: {} user queued ({} retry first), {} agent queued, {} agent wake-on-delivery",
                retry.len() + local.len(),
                retry.len(),
                agent_total,
                agent_trigger_turn
            )
            .dim()
            .into(),
        );

        if !retry.is_empty() {
            lines.push("Retry-first queue".bold().into());
            lines.extend(retry);
        }

        if !local.is_empty() {
            lines.push("User queue".bold().into());
            lines.extend(local);
        }

        if let Some(agent_messages) = agent_messages
            && !agent_messages.data.is_empty()
        {
            lines.push("Agent queue".bold().into());
            lines.extend(agent_messages.data.iter().map(agent_queued_message_line));
        }

        lines.push(queued_usage_line());
        self.add_plain_history_lines(lines);
    }

    pub(super) fn edit_rejected_queued_message(
        &mut self,
        position: usize,
        text: String,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Queued message text must not be empty.".to_string());
        }
        let index = local_queue_index(position)?;
        let Some(message) = self.input_queue.rejected_steers_queue.get_mut(index) else {
            return Err(format!("No queued retry message at position {position}."));
        };

        message.text = text;
        message.text_elements.clear();
        message.mention_bindings.clear();
        self.ensure_rejected_steer_history_records();
        if let Some(record) = self
            .input_queue
            .rejected_steer_history_records
            .get_mut(index)
        {
            *record = UserMessageHistoryRecord::UserMessageText;
        }
        self.refresh_pending_input_preview();
        Ok(())
    }

    pub(super) fn edit_local_queued_message(
        &mut self,
        position: usize,
        text: String,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Queued message text must not be empty.".to_string());
        }
        let index = local_queue_index(position)?;
        let Some(message) = self.input_queue.queued_user_messages.get_mut(index) else {
            return Err(format!("No queued user message at position {position}."));
        };

        message.user_message.text = text;
        message.user_message.text_elements.clear();
        message.user_message.mention_bindings.clear();
        self.ensure_local_queued_history_records();
        if let Some(record) = self
            .input_queue
            .queued_user_message_history_records
            .get_mut(index)
        {
            *record = UserMessageHistoryRecord::UserMessageText;
        }
        self.refresh_pending_input_preview();
        Ok(())
    }

    pub(super) fn move_rejected_queued_message(
        &mut self,
        position: usize,
        direction: LocalQueuedMessageMoveDirection,
    ) -> Result<usize, String> {
        let index = local_queue_index(position)?;
        let len = self.input_queue.rejected_steers_queue.len();
        if index >= len {
            return Err(format!("No queued retry message at position {position}."));
        }
        let target = match direction {
            LocalQueuedMessageMoveDirection::Up => index
                .checked_sub(1)
                .ok_or_else(|| "Queued retry message is already first.".to_string())?,
            LocalQueuedMessageMoveDirection::Down => {
                let next = index + 1;
                if next >= len {
                    return Err("Queued retry message is already last.".to_string());
                }
                next
            }
        };

        self.ensure_rejected_steer_history_records();
        self.input_queue.rejected_steers_queue.swap(index, target);
        self.input_queue
            .rejected_steer_history_records
            .swap(index, target);
        self.refresh_pending_input_preview();
        Ok(target + 1)
    }

    pub(super) fn move_local_queued_message(
        &mut self,
        position: usize,
        direction: LocalQueuedMessageMoveDirection,
    ) -> Result<usize, String> {
        let index = local_queue_index(position)?;
        let len = self.input_queue.queued_user_messages.len();
        if index >= len {
            return Err(format!("No queued user message at position {position}."));
        }
        let target = match direction {
            LocalQueuedMessageMoveDirection::Up => index
                .checked_sub(1)
                .ok_or_else(|| "Queued user message is already first.".to_string())?,
            LocalQueuedMessageMoveDirection::Down => {
                let next = index + 1;
                if next >= len {
                    return Err("Queued user message is already last.".to_string());
                }
                next
            }
        };

        self.ensure_local_queued_history_records();
        self.input_queue.queued_user_messages.swap(index, target);
        self.input_queue
            .queued_user_message_history_records
            .swap(index, target);
        self.refresh_pending_input_preview();
        Ok(target + 1)
    }

    fn rejected_steer_rows(&self) -> Vec<Line<'static>> {
        self.input_queue
            .rejected_steers_queue
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let preview = user_message_preview_text(
                    message,
                    self.input_queue.rejected_steer_history_records.get(idx),
                );
                vec![
                    "  ".into(),
                    format!("retry:{}.", idx + 1).cyan(),
                    " ".into(),
                    truncate_text(&preview, 140).into(),
                ]
                .into()
            })
            .collect()
    }

    fn local_queued_message_rows(&self) -> Vec<Line<'static>> {
        self.input_queue
            .queued_user_messages
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let preview = user_message_preview_text(
                    message,
                    self.input_queue
                        .queued_user_message_history_records
                        .get(idx),
                );
                vec![
                    "  ".into(),
                    format!("{}.", idx + 1).cyan(),
                    " ".into(),
                    truncate_text(&preview, 140).into(),
                ]
                .into()
            })
            .collect()
    }

    fn ensure_local_queued_history_records(&mut self) {
        while self.input_queue.queued_user_message_history_records.len()
            < self.input_queue.queued_user_messages.len()
        {
            self.input_queue
                .queued_user_message_history_records
                .push_back(UserMessageHistoryRecord::UserMessageText);
        }
    }

    fn ensure_rejected_steer_history_records(&mut self) {
        while self.input_queue.rejected_steer_history_records.len()
            < self.input_queue.rejected_steers_queue.len()
        {
            self.input_queue
                .rejected_steer_history_records
                .push_back(UserMessageHistoryRecord::UserMessageText);
        }
    }
}

fn local_queue_index(position: usize) -> Result<usize, String> {
    position
        .checked_sub(1)
        .ok_or_else(|| "Queued message positions start at 1.".to_string())
}

fn agent_queued_message_line(message: &ThreadQueuedMessage) -> Line<'static> {
    let wake = if message.trigger_turn { " wake" } else { "" };
    vec![
        "  ".into(),
        format!("agent:{}", message.message_id).cyan(),
        format!(" #{}{} ", message.position, wake).dim(),
        format!("{} -> {} ", message.author, message.recipient).dim(),
        truncate_text(&message.text, 140).into(),
    ]
    .into()
}

fn queued_usage_line() -> Line<'static> {
    "Commands: /queued edit <position|retry:n|agent:id> <text>, /queued up <position|retry:n|agent:id>, /queued down <position|retry:n|agent:id>."
        .dim()
        .into()
}
