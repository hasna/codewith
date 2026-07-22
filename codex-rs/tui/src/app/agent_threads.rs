//! User-created child agent threads from the `/agent` picker.

use super::*;

impl App {
    pub(super) async fn create_agent_thread_from_picker(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
    ) -> Result<()> {
        if !self.config.features.enabled(Feature::Collab) {
            self.chat_widget.open_multi_agent_enable_prompt();
            return Ok(());
        }

        let Some(parent_thread_id) = self
            .active_side_parent_thread_id()
            .or_else(|| self.current_displayed_thread_id())
        else {
            self.chat_widget.add_error_message(
                "'/agent' is unavailable until the current conversation has started.".to_string(),
            );
            return Ok(());
        };

        self.session_telemetry.counter(
            "codex.thread.agent",
            /*inc*/ 1,
            &[("source", "slash_command")],
        );
        self.refresh_in_memory_config_from_disk_best_effort("starting an agent thread")
            .await;

        let mut config = self.chat_widget.config_ref().clone();
        let parent_model = self.chat_widget.current_model();
        if !parent_model.trim().is_empty() {
            config.model = Some(parent_model.to_string());
        }
        config.model_reasoning_effort = self.chat_widget.current_reasoning_effort();
        config.service_tier = self.chat_widget.configured_service_tier();

        let started = match app_server
            .start_agent_thread(&config, parent_thread_id)
            .await
        {
            Ok(started) => started,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to create agent thread: {err}"));
                return Ok(());
            }
        };

        let child_thread_id = started.session.thread_id;
        let child_thread_name = started.session.thread_name.clone();
        let channel = self.ensure_thread_channel(child_thread_id);
        {
            let mut store = channel.store.lock().await;
            store.set_session(started.session, started.turns);
        }
        self.upsert_agent_picker_thread(
            child_thread_id,
            /*agent_nickname*/ None,
            /*agent_role*/ None,
            Some(parent_thread_id),
            /*agent_path*/ None,
            /*is_closed*/ false,
        );
        self.agent_navigation
            .set_thread_name(child_thread_id, child_thread_name);
        self.sync_active_agent_label();

        if let Err(err) = self
            .select_agent_thread_and_discard_side(tui, app_server, child_thread_id)
            .await
        {
            self.chat_widget.add_error_message(format!(
                "Failed to switch into agent thread {child_thread_id}: {err}"
            ));
        }

        Ok(())
    }
}
