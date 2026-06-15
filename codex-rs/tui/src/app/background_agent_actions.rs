use super::App;
use crate::app_server_session::AppServerSession;
use codex_protocol::ThreadId;

const BACKGROUND_AGENT_CREATE_HINT: &str = "Start one with /agent start <prompt>.";

impl App {
    pub(super) async fn open_background_agent_manager(
        &mut self,
        app_server: &mut AppServerSession,
    ) {
        match app_server.agent_list().await {
            Ok(response) => self
                .chat_widget
                .show_background_agent_manager(response.data),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read background agents: {err}")),
        }
    }

    pub(super) async fn open_background_agent_actions(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: String,
    ) {
        match app_server.agent_read(agent_id.clone()).await {
            Ok(response) => {
                if let Some(agent) = response.agent {
                    self.chat_widget.show_background_agent_actions(agent);
                } else {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read background agent: {err}")),
        }
    }

    pub(super) async fn start_background_agent(
        &mut self,
        app_server: &mut AppServerSession,
        prompt: String,
    ) {
        let cwd = Some(self.config.cwd.to_string_lossy().to_string());
        let parent_thread_id = self.active_thread_id;
        let auth_profile_ref = self.config.selected_auth_profile.clone();
        match app_server
            .agent_start(prompt, cwd, parent_thread_id, auth_profile_ref)
            .await
        {
            Ok(response) => {
                let agent_id = response.agent.agent_id.clone();
                self.chat_widget
                    .show_background_agent_summary(vec![response.agent]);
                self.chat_widget.add_info_message(
                    "Background agent started".to_string(),
                    Some(format!(
                        "Use /agent attach {} to open its session or replay its events.",
                        short_background_agent_id(&agent_id)
                    )),
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to start background agent: {err}")),
        }
    }

    pub(super) async fn read_background_agent(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
    ) {
        let Some(agent_id) = self
            .resolve_background_agent_id(app_server, agent_id, "read")
            .await
        else {
            return;
        };
        match app_server.agent_read(agent_id.clone()).await {
            Ok(response) => {
                if response.agent.is_some() {
                    self.chat_widget.show_background_agent_read(response);
                } else {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read background agent: {err}")),
        }
    }

    pub(super) async fn attach_background_agent(
        &mut self,
        tui: &mut crate::tui::Tui,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
    ) {
        let Some(agent_id) = self
            .resolve_background_agent_id(app_server, agent_id, "attach")
            .await
        else {
            return;
        };
        match app_server.agent_attach(agent_id.clone()).await {
            Ok(response) => {
                if let Some(agent) = response.agent.clone() {
                    if let Some(thread_id) = agent
                        .thread_id
                        .as_deref()
                        .and_then(|id| ThreadId::from_string(id).ok())
                    {
                        if let Err(err) = self
                            .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                            .await
                        {
                            self.chat_widget.add_error_message(format!(
                                "Failed to open background agent session: {err}"
                            ));
                            self.chat_widget.show_background_agent_attach(response);
                            return;
                        }
                        self.chat_widget.add_info_message(
                            "Background agent attached".to_string(),
                            Some(format!(
                                "Opened session for {}. Use /agent detach {} when you are done.",
                                short_background_agent_id(&agent.agent_id),
                                short_background_agent_id(&agent.agent_id)
                            )),
                        );
                        return;
                    }
                    self.chat_widget.show_background_agent_attach(response);
                    self.chat_widget.add_info_message(
                        "Background agent attached".to_string(),
                        Some(format!(
                            "Use /agent detach {} when you are done.",
                            short_background_agent_id(&agent.agent_id)
                        )),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to attach background agent: {err}")),
        }
    }

    pub(super) async fn show_background_agent_logs(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
    ) {
        let Some(agent_id) = self
            .resolve_background_agent_id(app_server, agent_id, "logs")
            .await
        else {
            return;
        };
        match app_server.agent_events_list(agent_id.clone()).await {
            Ok(response) => self
                .chat_widget
                .show_background_agent_logs(agent_id, response),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read background agent logs: {err}")),
        }
    }

    pub(super) async fn detach_background_agent(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
    ) {
        let Some(agent_id) = self
            .resolve_background_agent_id(app_server, agent_id, "detach")
            .await
        else {
            return;
        };
        match app_server.agent_detach(agent_id.clone()).await {
            Ok(response) => {
                if let Some(agent) = response.agent {
                    self.chat_widget.add_info_message(
                        "Background agent detached".to_string(),
                        Some(format!(
                            "Detached from {}.",
                            short_background_agent_id(&agent.agent_id)
                        )),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to detach background agent: {err}")),
        }
    }

    pub(super) async fn stop_background_agent(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
    ) {
        let Some(agent_id) = self
            .resolve_background_agent_id(app_server, agent_id, "stop")
            .await
        else {
            return;
        };
        match app_server.agent_stop(agent_id.clone()).await {
            Ok(response) => {
                if let Some(agent) = response.agent {
                    self.chat_widget.show_background_agent_summary(vec![agent]);
                    self.chat_widget.add_info_message(
                        "Background agent stop requested".to_string(),
                        Some(format!(
                            "Read it with /agent read {}.",
                            short_background_agent_id(&agent_id)
                        )),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to stop background agent: {err}")),
        }
    }

    pub(super) async fn delete_background_agent(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
    ) {
        let Some(agent_id) = self
            .resolve_background_agent_id(app_server, agent_id, "delete")
            .await
        else {
            return;
        };
        match app_server.agent_delete(agent_id.clone()).await {
            Ok(response) => {
                if response.deleted {
                    self.chat_widget.add_info_message(
                        "Background agent deleted".to_string(),
                        Some(format!("Deleted background agent {agent_id}.")),
                    );
                } else if let Some(agent) = response.agent {
                    self.chat_widget.show_background_agent_summary(vec![agent]);
                    self.chat_widget.add_info_message(
                        "Background agent delete requested".to_string(),
                        Some(format!("Deletion is pending for {agent_id}.")),
                    );
                } else {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                }
            }
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to delete background agent: {err}")),
        }
    }

    pub(super) async fn show_background_agent_diagnostics(
        &mut self,
        app_server: &mut AppServerSession,
    ) {
        match app_server.agent_daemon_diagnostics().await {
            Ok(response) => self.chat_widget.show_background_agent_diagnostics(response),
            Err(err) => self.chat_widget.add_error_message(format!(
                "Failed to read background-agent diagnostics: {err}"
            )),
        }
    }

    async fn resolve_background_agent_id(
        &mut self,
        app_server: &mut AppServerSession,
        agent_id: Option<String>,
        action: &'static str,
    ) -> Option<String> {
        if let Some(agent_id) = agent_id.filter(|value| !value.trim().is_empty()) {
            match app_server.agent_read(agent_id.clone()).await {
                Ok(response) if response.agent.is_some() => return Some(agent_id),
                Ok(_) => {}
                Err(err) => {
                    self.chat_widget
                        .add_error_message(format!("Failed to read background agent: {err}"));
                    return None;
                }
            }

            let result = app_server.agent_list().await;
            let response = match result {
                Ok(response) => response,
                Err(err) => {
                    self.chat_widget
                        .add_error_message(format!("Failed to read background agents: {err}"));
                    return None;
                }
            };
            let mut matches = response
                .data
                .into_iter()
                .filter(|agent| agent.agent_id.starts_with(agent_id.as_str()))
                .collect::<Vec<_>>();
            return match matches.len() {
                0 => {
                    self.chat_widget.add_info_message(
                        "No matching background agent".to_string(),
                        Some(format!("Could not find background agent {agent_id}.")),
                    );
                    None
                }
                1 => Some(matches.remove(0).agent_id),
                _ => {
                    self.chat_widget.show_background_agent_manager(matches);
                    self.chat_widget.add_info_message(
                        format!("Choose a background agent to {action}"),
                        Some(format!("Use /agent {action} <id>.")),
                    );
                    None
                }
            };
        }

        let result = app_server.agent_list().await;
        let response = match result {
            Ok(response) => response,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to read background agents: {err}"));
                return None;
            }
        };
        let agents = response.data;

        match agents.len() {
            0 => {
                self.chat_widget.add_info_message(
                    "No background agents created".to_string(),
                    Some(BACKGROUND_AGENT_CREATE_HINT.to_string()),
                );
                None
            }
            1 => Some(agents[0].agent_id.clone()),
            _ => {
                self.chat_widget.show_background_agent_manager(agents);
                self.chat_widget.add_info_message(
                    format!("Choose a background agent to {action}"),
                    Some(format!("Use /agent {action} <id>.")),
                );
                None
            }
        }
    }
}

fn short_background_agent_id(agent_id: &str) -> String {
    agent_id.chars().take(8).collect()
}
