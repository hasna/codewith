use super::App;
use crate::app_server_session::AppServerSession;
use codex_app_server_protocol::ThreadQueuedMessageMoveDirection;
use codex_protocol::ThreadId;

impl App {
    pub(super) async fn open_queued_messages(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: Option<ThreadId>,
    ) {
        let agent_messages = match thread_id {
            Some(thread_id) => {
                let result = app_server.thread_queued_message_list(thread_id).await;
                if self.current_displayed_thread_id() != Some(thread_id) {
                    return;
                }
                match result {
                    Ok(response) => Some(response),
                    Err(err) => {
                        self.chat_widget.add_error_message(format!(
                            "Failed to read queued agent messages: {err}"
                        ));
                        None
                    }
                }
            }
            None => None,
        };

        self.chat_widget.show_queued_messages(agent_messages);
    }

    pub(super) async fn update_queued_thread_message(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        message_id: String,
        text: String,
    ) {
        let result = app_server
            .thread_queued_message_update(thread_id, message_id.clone(), text)
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if response.message.is_some() {
                    self.chat_widget.add_info_message(
                        "Queued agent message updated".to_string(),
                        Some(format!("messageId: {message_id}")),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "No matching queued agent message".to_string(),
                        Some(format!("Could not find messageId {message_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to update queued agent message: {err}")),
        }
    }

    pub(super) async fn move_queued_thread_message(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        message_id: String,
        direction: ThreadQueuedMessageMoveDirection,
    ) {
        let result = app_server
            .thread_queued_message_move(thread_id, message_id.clone(), direction)
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if response.moved {
                    let position = response
                        .message
                        .as_ref()
                        .map(|message| message.position.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    self.chat_widget.add_info_message(
                        "Queued agent message moved".to_string(),
                        Some(format!("messageId: {message_id}, position: {position}")),
                    );
                } else if response.message.is_some() {
                    self.chat_widget.add_info_message(
                        "Queued agent message was not moved".to_string(),
                        Some("It is already at the requested boundary.".to_string()),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "No matching queued agent message".to_string(),
                        Some(format!("Could not find messageId {message_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to move queued agent message: {err}")),
        }
    }
}
