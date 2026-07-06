use super::App;
use crate::app_server_session::AppServerSession;
use codex_protocol::ThreadId;

const MONITOR_CREATE_HINT: &str = "Create one with /monitor <request>.";

impl App {
    pub(super) async fn open_thread_monitor_manager(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) {
        let result = app_server.thread_monitor_list(thread_id).await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => self
                .chat_widget
                .show_monitor_manager(thread_id, response.data),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read monitors: {err}")),
        }
    }

    pub(super) async fn open_thread_monitor_actions(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        monitor_id: String,
    ) {
        let result = app_server
            .thread_monitor_read(
                thread_id,
                monitor_id.clone(),
                /*cursor*/ None,
                /*limit*/ None,
            )
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if let Some(monitor) = response.monitor {
                    self.chat_widget.show_monitor_actions(thread_id, monitor);
                } else {
                    self.chat_widget.add_info_message(
                        "No matching monitor".to_string(),
                        Some(format!("Could not find monitor {monitor_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read monitor: {err}")),
        }
    }

    pub(super) async fn read_thread_monitor(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        monitor_id: Option<String>,
    ) {
        let Some(monitor_id) = self
            .resolve_thread_monitor_id(app_server, thread_id, monitor_id, "read")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_monitor_read(
                thread_id,
                monitor_id.clone(),
                /*cursor*/ None,
                /*limit*/ None,
            )
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if let Some(monitor) = response.monitor {
                    self.chat_widget
                        .show_monitor_events(monitor, response.events);
                } else {
                    self.chat_widget.add_info_message(
                        "No matching monitor".to_string(),
                        Some(format!("Could not find monitor {monitor_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read monitor output: {err}")),
        }
    }

    pub(super) async fn stop_thread_monitor(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        monitor_id: Option<String>,
    ) {
        let Some(monitor_id) = self
            .resolve_thread_monitor_id(app_server, thread_id, monitor_id, "stop")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_monitor_stop(thread_id, monitor_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                self.chat_widget
                    .show_monitor_summary(vec![response.monitor]);
                self.chat_widget.add_info_message(
                    "Monitor stopped".to_string(),
                    Some(format!("Restart it with /monitor restart {monitor_id}.")),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to stop monitor: {err}")),
        }
    }

    pub(super) async fn restart_thread_monitor(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        monitor_id: Option<String>,
    ) {
        let Some(monitor_id) = self
            .resolve_thread_monitor_id(app_server, thread_id, monitor_id, "restart")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_monitor_restart(thread_id, monitor_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                self.chat_widget
                    .show_monitor_summary(vec![response.monitor]);
                self.chat_widget.add_info_message(
                    "Monitor restarted".to_string(),
                    Some(format!("Read output with /monitor read {monitor_id}.")),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to restart monitor: {err}")),
        }
    }

    pub(super) async fn delete_thread_monitor(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        monitor_id: Option<String>,
    ) {
        let Some(monitor_id) = self
            .resolve_thread_monitor_id(app_server, thread_id, monitor_id, "delete")
            .await
        else {
            return;
        };
        let result = app_server
            .thread_monitor_delete(thread_id, monitor_id.clone())
            .await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => {
                if response.deleted {
                    self.chat_widget.add_info_message(
                        "Monitor deleted".to_string(),
                        Some(format!("Deleted monitor {}.", response.monitor_id)),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "Monitor not found".to_string(),
                        Some(format!("Could not find monitor {monitor_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to delete monitor: {err}")),
        }
    }

    async fn resolve_thread_monitor_id(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        monitor_id: Option<String>,
        action: &'static str,
    ) -> Option<String> {
        if let Some(monitor_id) = monitor_id.filter(|value| !value.trim().is_empty()) {
            let result = app_server
                .thread_monitor_read(
                    thread_id,
                    monitor_id.clone(),
                    /*cursor*/ None,
                    /*limit*/ None,
                )
                .await;
            if self.current_displayed_thread_id() != Some(thread_id) {
                return None;
            }

            match result {
                Ok(response) if response.monitor.is_some() => return Some(monitor_id),
                Ok(_) => {
                    self.chat_widget.add_info_message(
                        "No matching monitor".to_string(),
                        Some(format!("Could not find monitor {monitor_id}.")),
                    );
                    return None;
                }
                Err(err) => {
                    self.chat_widget
                        .add_error_message(format!("Failed to read monitor: {err}"));
                    return None;
                }
            }
        }

        let result = app_server.thread_monitor_list(thread_id).await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return None;
        }

        let response = match result {
            Ok(response) => response,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to read monitors: {err}"));
                return None;
            }
        };
        let monitors = response.data;

        match monitors.len() {
            0 => {
                self.chat_widget.add_info_message(
                    "No monitors created".to_string(),
                    Some(MONITOR_CREATE_HINT.to_string()),
                );
                None
            }
            1 => Some(monitors[0].monitor_id.clone()),
            _ => {
                self.chat_widget.show_monitor_manager(thread_id, monitors);
                self.chat_widget.add_info_message(
                    format!("Choose a monitor to {action}"),
                    Some(format!("Use /monitor {action} <id>.")),
                );
                None
            }
        }
    }
}
