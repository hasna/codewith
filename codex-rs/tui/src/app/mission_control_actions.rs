use super::App;
use crate::app_server_session::AppServerSession;
use codex_app_server_protocol::MissionControlRespondInteractionParams;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;

impl App {
    pub(super) async fn open_mission_control_overview(
        &mut self,
        app_server: &mut AppServerSession,
    ) {
        match app_server.mission_control_overview().await {
            Ok(response) => self.chat_widget.show_mission_control_overview(response),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read mission control overview: {err}")),
        }
    }

    pub(super) async fn respond_mission_control_interaction(
        &mut self,
        app_server: &mut AppServerSession,
        interaction_id: String,
        thread_id: Option<String>,
        terminal_status: ThreadPendingInteractionTerminalStatus,
        response: ThreadPendingInteractionResponsePayload,
    ) {
        match app_server
            .mission_control_respond_interaction(MissionControlRespondInteractionParams {
                interaction_id: interaction_id.clone(),
                thread_id,
                terminal_status,
                response,
                dry_run: false,
            })
            .await
        {
            Ok(result) => {
                let suffix = if result.updated {
                    "updated"
                } else {
                    "already current"
                };
                self.chat_widget.add_info_message(
                    format!("Recorded pending interaction response {interaction_id}"),
                    Some(format!("Ledger status: {suffix}.")),
                );
                self.open_mission_control_overview(app_server).await;
            }
            Err(err) => self.chat_widget.add_error_message(format!(
                "Failed to record pending interaction response {interaction_id}: {err}"
            )),
        }
    }
}
