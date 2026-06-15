use super::App;
use crate::app_server_session::AppServerSession;
use crate::status::format_directory_display;
use codex_app_server_protocol::ActiveSessionListParams;
use codex_app_server_protocol::ActiveSessionMessageDelivery;
use codex_app_server_protocol::ActiveSessionPeer;
use codex_app_server_protocol::ActiveSessionPeerKind;
use codex_app_server_protocol::ActiveSessionSendStatus;
use codex_protocol::ThreadId;

const ACTIVE_SESSION_SEND_HINT: &str =
    "Use /agent send <thread-id> <message> or /agent send --wake <thread-id> <message>.";
const ACTIVE_SESSION_ACTIVE_ONLY_HINT: &str =
    "Only loaded sessions can receive messages; no offline delivery is attempted.";

impl App {
    pub(super) async fn list_active_sessions(&mut self, app_server: &mut AppServerSession) {
        match app_server
            .active_session_list(ActiveSessionListParams::default())
            .await
        {
            Ok(response) if response.data.is_empty() => {
                self.chat_widget.add_info_message(
                    "No active sessions".to_string(),
                    Some(ACTIVE_SESSION_ACTIVE_ONLY_HINT.to_string()),
                );
            }
            Ok(response) => {
                let active_thread_id = self.active_thread_id.map(|thread_id| thread_id.to_string());
                let details = response
                    .data
                    .iter()
                    .map(|peer| format_active_session_peer(peer, active_thread_id.as_deref()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let hint = format!(
                    "{details}\n\n{ACTIVE_SESSION_SEND_HINT}\n{ACTIVE_SESSION_ACTIVE_ONLY_HINT}"
                );
                self.chat_widget.add_info_message(
                    format!("Active sessions ({})", response.data.len()),
                    Some(hint),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to list active sessions: {err}")),
        }
    }

    pub(super) async fn send_active_session_message(
        &mut self,
        app_server: &mut AppServerSession,
        target_thread_id: String,
        message: String,
        wake: bool,
    ) {
        let sender_thread_id = self
            .active_thread_id
            .or_else(|| self.chat_widget.thread_id());
        if is_current_thread_target(sender_thread_id, target_thread_id.as_str()) {
            self.chat_widget.add_error_message(
                "Cannot send an active-session message to the current thread.".to_string(),
            );
            return;
        }
        let delivery = if wake {
            ActiveSessionMessageDelivery::TriggerTurn
        } else {
            ActiveSessionMessageDelivery::QueueOnly
        };
        match app_server
            .active_session_send(
                target_thread_id.clone(),
                message,
                sender_thread_id,
                delivery,
            )
            .await
        {
            Ok(response) => match response.status {
                ActiveSessionSendStatus::Delivered => {
                    let delivery_hint = if wake {
                        "Target was asked to wake and process the message."
                    } else {
                        "Message is queued for the target's next mailbox drain."
                    };
                    self.chat_widget.add_info_message(
                        "Active session message delivered".to_string(),
                        Some(format!(
                            "{} to {}. {delivery_hint}",
                            short_id(&response.message_id),
                            short_id(&response.target_thread_id)
                        )),
                    );
                }
                ActiveSessionSendStatus::NotLoaded => {
                    let reason = response
                        .reason
                        .unwrap_or_else(|| ACTIVE_SESSION_ACTIVE_ONLY_HINT.to_string());
                    self.chat_widget.add_info_message(
                        "Active session unavailable".to_string(),
                        Some(format!("{reason}\n{ACTIVE_SESSION_ACTIVE_ONLY_HINT}")),
                    );
                }
            },
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to send active session message: {err}")),
        }
    }
}

fn format_active_session_peer(peer: &ActiveSessionPeer, active_thread_id: Option<&str>) -> String {
    let marker = if active_thread_id == Some(peer.thread_id.as_str()) {
        " current"
    } else {
        ""
    };
    let label = peer
        .display_name
        .as_deref()
        .or(peer.agent_path.as_deref())
        .unwrap_or("session");
    let cwd = format_directory_display(peer.cwd.as_path(), /*max_width*/ None);
    format!(
        "{}  {}{}  {}  {}",
        peer.thread_id,
        active_session_kind(peer.kind),
        marker,
        label,
        cwd
    )
}

fn active_session_kind(kind: ActiveSessionPeerKind) -> &'static str {
    match kind {
        ActiveSessionPeerKind::CodewithSession => "session",
        ActiveSessionPeerKind::SpawnedAgent => "agent",
        ActiveSessionPeerKind::BridgeAdapter => "bridge",
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn is_current_thread_target(current_thread_id: Option<ThreadId>, target_thread_id: &str) -> bool {
    let Ok(target_thread_id) = ThreadId::from_string(target_thread_id) else {
        return false;
    };
    current_thread_id == Some(target_thread_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_utils_absolute_path::AbsolutePathBuf;

    #[test]
    fn active_session_peer_row_snapshot() {
        let peer = ActiveSessionPeer {
            peer_id: "019eca00-0000-7000-8000-000000000001".to_string(),
            kind: ActiveSessionPeerKind::SpawnedAgent,
            thread_id: "019eca00-0000-7000-8000-000000000001".to_string(),
            session_id: "019eca00-0000-7000-8000-000000000001".to_string(),
            cwd: AbsolutePathBuf::from_absolute_path("/workspace/open-codewith")
                .expect("absolute cwd"),
            display_name: Some("reviewer".to_string()),
            agent_path: Some("/root/reviewer".to_string()),
            capabilities: Vec::new(),
            last_seen_at: 1_781_512_883,
        };

        insta::assert_snapshot!(format_active_session_peer(
            &peer,
            Some(peer.thread_id.as_str())
        ));
    }

    #[test]
    fn active_session_active_only_hint_snapshot() {
        insta::assert_snapshot!(ACTIVE_SESSION_ACTIVE_ONLY_HINT);
    }

    #[test]
    fn self_send_detection_normalizes_thread_id_text() {
        let current_thread_id = ThreadId::from_string("019eca00-0000-7000-8000-000000000001")
            .expect("thread id should parse");

        assert!(is_current_thread_target(
            Some(current_thread_id),
            "019ECA00-0000-7000-8000-000000000001"
        ));
        assert!(!is_current_thread_target(
            Some(current_thread_id),
            "not-a-thread-id"
        ));
    }
}
