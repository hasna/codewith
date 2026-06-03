use super::App;
use crate::app_server_session::AppServerSession;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;
use codex_protocol::ThreadId;

const SCHEDULE_CREATE_HINT: &str = "Create one with /schedule 5m check whether CI is green.";

impl App {
    pub(super) async fn open_thread_schedule_manager(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) {
        let result = app_server.thread_schedule_list(thread_id).await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) if response.data.is_empty() => self.chat_widget.add_info_message(
                "No schedules created".to_string(),
                Some(SCHEDULE_CREATE_HINT.to_string()),
            ),
            Ok(response) => self
                .chat_widget
                .show_schedule_manager(thread_id, response.data),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read schedules: {err}")),
        }
    }

    pub(super) async fn open_thread_schedule_actions(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: String,
    ) {
        let result = app_server
            .thread_schedule_get(thread_id, schedule_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if let Some(schedule) = response.schedule {
                    self.chat_widget.show_schedule_actions(thread_id, schedule);
                } else {
                    self.chat_widget.add_info_message(
                        "No matching schedule".to_string(),
                        Some(format!("Could not find schedule {schedule_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read schedule: {err}")),
        }
    }

    pub(super) async fn open_thread_schedule_editor(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: Option<String>,
    ) {
        let Some(schedule_id) = self
            .resolve_thread_schedule_id(app_server, thread_id, schedule_id, "edit")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_schedule_get(thread_id, schedule_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if let Some(schedule) = response.schedule {
                    self.chat_widget
                        .show_schedule_edit_prompt(thread_id, schedule);
                } else {
                    self.chat_widget.add_info_message(
                        "No matching schedule".to_string(),
                        Some(format!("Could not find schedule {schedule_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read schedule: {err}")),
        }
    }

    pub(super) async fn create_thread_schedule(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        prompt: String,
        prompt_source: ThreadSchedulePromptSource,
        schedule: ThreadScheduleSpec,
    ) {
        let result = app_server
            .thread_schedule_create(thread_id, prompt, prompt_source, schedule)
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                self.chat_widget.show_schedule_created(response.schedule);
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to create schedule: {err}")),
        }
    }

    pub(super) async fn update_thread_schedule_prompt(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: String,
        prompt: String,
    ) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            self.chat_widget
                .add_error_message("Schedule prompt must not be empty.".to_string());
            return;
        }

        let result = app_server
            .thread_schedule_update(
                thread_id,
                schedule_id.clone(),
                Some(prompt),
                /*schedule*/ None,
                /*status*/ None,
            )
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                self.chat_widget
                    .show_schedule_summary(vec![response.schedule]);
                self.chat_widget.add_info_message(
                    "Schedule updated".to_string(),
                    Some(format!("Updated prompt for {schedule_id}.")),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to update schedule: {err}")),
        }
    }

    pub(super) async fn pause_thread_schedule(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: Option<String>,
    ) {
        let Some(schedule_id) = self
            .resolve_thread_schedule_id(app_server, thread_id, schedule_id, "pause")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_schedule_pause(thread_id, schedule_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                self.chat_widget
                    .show_schedule_summary(vec![response.schedule]);
                self.chat_widget.add_info_message(
                    "Schedule paused".to_string(),
                    Some(format!("Resume it with /schedule resume {schedule_id}.")),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to pause schedule: {err}")),
        }
    }

    pub(super) async fn resume_thread_schedule(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: Option<String>,
    ) {
        let Some(schedule_id) = self
            .resolve_thread_schedule_id(app_server, thread_id, schedule_id, "resume")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_schedule_resume(thread_id, schedule_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                self.chat_widget
                    .show_schedule_summary(vec![response.schedule]);
                self.chat_widget.add_info_message(
                    "Schedule resumed".to_string(),
                    Some(format!("Pause it with /schedule pause {schedule_id}.")),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to resume schedule: {err}")),
        }
    }

    pub(super) async fn delete_thread_schedule(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: Option<String>,
    ) {
        let Some(schedule_id) = self
            .resolve_thread_schedule_id(app_server, thread_id, schedule_id, "delete")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_schedule_delete(thread_id, schedule_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) if response.deleted => self
                .chat_widget
                .add_info_message("Schedule deleted".to_string(), Some(schedule_id)),
            Ok(_) => self.chat_widget.add_info_message(
                "No matching schedule".to_string(),
                Some(format!("Could not find schedule {schedule_id}.")),
            ),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to delete schedule: {err}")),
        }
    }

    pub(super) async fn run_thread_schedule_now(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: Option<String>,
    ) {
        let Some(schedule_id) = self
            .resolve_thread_schedule_id(app_server, thread_id, schedule_id, "run-now")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_schedule_run_now(thread_id, schedule_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => self.chat_widget.add_info_message(
                "Schedule run started".to_string(),
                Some(format!("{} queued for {schedule_id}.", response.run.run_id)),
            ),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to run schedule: {err}")),
        }
    }

    async fn resolve_thread_schedule_id(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        schedule_id: Option<String>,
        action: &'static str,
    ) -> Option<String> {
        if let Some(schedule_id) = schedule_id.filter(|value| !value.trim().is_empty()) {
            return Some(schedule_id);
        }

        let result = app_server.thread_schedule_list(thread_id).await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return None;
        }

        let response = match result {
            Ok(response) => response,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to read schedules: {err}"));
                return None;
            }
        };
        let active_or_paused = response
            .data
            .into_iter()
            .filter(|schedule| !matches!(schedule.status, ThreadScheduleStatus::Expired))
            .collect::<Vec<_>>();

        match active_or_paused.len() {
            0 => {
                self.chat_widget.add_info_message(
                    "No schedules created".to_string(),
                    Some(SCHEDULE_CREATE_HINT.to_string()),
                );
                None
            }
            1 => Some(active_or_paused[0].schedule_id.clone()),
            _ => {
                self.chat_widget
                    .show_schedule_manager(thread_id, active_or_paused);
                self.chat_widget.add_info_message(
                    format!("Choose a schedule to {action}"),
                    Some(format!("Use /schedule {action} <id>.")),
                );
                None
            }
        }
    }
}
