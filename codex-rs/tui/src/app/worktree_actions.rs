use super::App;
use crate::app_server_session::AppServerSession;
use codex_app_server_protocol::ThreadSettingsUpdateParams;
use codex_app_server_protocol::Worktree;
use codex_app_server_protocol::WorktreeLifecycleStatus;
use codex_app_server_protocol::WorktreeReadResponse;
use codex_app_server_protocol::WorktreeSessionMode;
use codex_exec_server::LOCAL_FS;
use codex_git_utils::resolve_root_git_project_for_trust;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::PathBuf;

impl App {
    pub(super) async fn open_worktree_manager(&mut self, app_server: &mut AppServerSession) {
        let base_repo_path = self.current_worktree_base_repo_path().await;
        match app_server.worktree_list(base_repo_path).await {
            Ok(response) => self
                .chat_widget
                .show_worktree_manager(response.data, response.policy),
            Err(err) => self
                .chat_widget
                .add_error_message(format!("Failed to read worktrees: {err}")),
        }
    }

    pub(super) async fn open_worktree_actions(
        &mut self,
        app_server: &mut AppServerSession,
        worktree_id: String,
        base_repo_path: Option<String>,
    ) {
        let Some((worktree_id, response)) = self
            .read_selected_worktree(app_server, Some(worktree_id), base_repo_path, "actions")
            .await
        else {
            return;
        };
        if let Some(worktree) = response.worktree {
            self.chat_widget
                .show_worktree_actions(worktree, response.policy);
        } else {
            self.show_no_matching_worktree(worktree_id);
        }
    }

    pub(super) async fn read_worktree(
        &mut self,
        app_server: &mut AppServerSession,
        worktree_id: Option<String>,
        base_repo_path: Option<String>,
    ) {
        let Some((worktree_id, response)) = self
            .read_selected_worktree(app_server, worktree_id, base_repo_path, "read")
            .await
        else {
            return;
        };
        if let Some(worktree) = response.worktree {
            self.chat_widget.show_worktree_read(worktree);
        } else {
            self.show_no_matching_worktree(worktree_id);
        }
    }

    pub(super) async fn use_worktree(
        &mut self,
        app_server: &mut AppServerSession,
        worktree_id: String,
        base_repo_path: Option<String>,
    ) {
        let Some(thread_id) = self.active_thread_id else {
            self.chat_widget.add_error_message(
                "Cannot use a worktree before the current session is ready".to_string(),
            );
            return;
        };
        let Some((worktree_id, response)) = self
            .read_selected_worktree(app_server, Some(worktree_id), base_repo_path, "use")
            .await
        else {
            return;
        };
        let Some(worktree) = response.worktree else {
            self.show_no_matching_worktree(worktree_id);
            return;
        };
        let disabled_reason = if !response.policy.enabled {
            Some("managed worktrees are disabled in config".to_string())
        } else if response.policy.main_sessions == WorktreeSessionMode::Off {
            Some("main-session worktrees are disabled in config".to_string())
        } else if worktree.lifecycle_status != WorktreeLifecycleStatus::Active {
            Some("only active worktrees can be used by the current session".to_string())
        } else {
            None
        };
        if let Some(reason) = disabled_reason {
            self.chat_widget
                .add_error_message(format!("Cannot use worktree: {reason}"));
            return;
        }

        let short_id = worktree.worktree_id.chars().take(8).collect::<String>();
        let worktree_path = worktree.worktree_path.clone();
        if let Err(err) = app_server
            .worktree_attach(
                worktree.worktree_id.clone(),
                Some(thread_id.to_string()),
                /*agent_run_id*/ None,
            )
            .await
        {
            self.chat_widget
                .add_error_message(format!("Failed to assign worktree: {err}"));
            return;
        }
        let params = ThreadSettingsUpdateParams {
            thread_id: thread_id.to_string(),
            cwd: Some(PathBuf::from(&worktree_path)),
            ..ThreadSettingsUpdateParams::default()
        };
        match app_server.thread_settings_update(params).await {
            Ok(()) => {
                if let Ok(cwd) =
                    AbsolutePathBuf::from_absolute_path_checked(PathBuf::from(&worktree_path))
                {
                    self.config.cwd = cwd;
                }
                self.chat_widget.add_info_message(
                    "Current session using worktree".to_string(),
                    Some(format!("{short_id} at {worktree_path}")),
                );
            }
            Err(err) => {
                let detach_result = app_server
                    .worktree_detach(
                        worktree.worktree_id.clone(),
                        Some(thread_id.to_string()),
                        /*agent_run_id*/ None,
                    )
                    .await;
                let rollback_suffix = match detach_result {
                    Ok(_) => " Assignment was rolled back.".to_string(),
                    Err(detach_err) => format!(" Assignment rollback failed: {detach_err}"),
                };
                self.chat_widget
                    .add_error_message(format!("Failed to use worktree: {err}.{rollback_suffix}"));
            }
        }
    }

    pub(super) async fn read_selected_worktree(
        &mut self,
        app_server: &mut AppServerSession,
        worktree_id: Option<String>,
        base_repo_path: Option<String>,
        action: &'static str,
    ) -> Option<(String, WorktreeReadResponse)> {
        let worktree_id = self
            .resolve_worktree_id(app_server, worktree_id, base_repo_path.clone(), action)
            .await?;
        match app_server
            .worktree_read(worktree_id.clone(), base_repo_path)
            .await
        {
            Ok(response) => Some((worktree_id, response)),
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to read worktree: {err}"));
                None
            }
        }
    }

    async fn resolve_worktree_id(
        &mut self,
        app_server: &mut AppServerSession,
        worktree_id: Option<String>,
        base_repo_path: Option<String>,
        action: &'static str,
    ) -> Option<String> {
        let base_repo_path = match base_repo_path {
            Some(base_repo_path) => Some(base_repo_path),
            None => self.current_worktree_base_repo_path().await,
        };
        let response = match app_server.worktree_list(base_repo_path.clone()).await {
            Ok(response) => response,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to read worktrees: {err}"));
                return None;
            }
        };
        let policy = response.policy;
        let worktrees = response.data;

        if let Some(worktree_id) = worktree_id.filter(|value| !value.trim().is_empty()) {
            let mut matches = worktrees
                .into_iter()
                .filter(|worktree| worktree.worktree_id.starts_with(worktree_id.as_str()))
                .collect::<Vec<_>>();
            return match matches.len() {
                0 => {
                    self.show_no_matching_worktree(worktree_id);
                    None
                }
                1 => Some(matches.remove(0).worktree_id),
                _ => {
                    self.show_worktree_picker(matches, policy, action);
                    self.chat_widget.add_info_message(
                        format!("Choose a worktree to {action}"),
                        Some(worktree_action_usage(action)),
                    );
                    None
                }
            };
        }

        match one_worktree(worktrees) {
            Ok(worktree_id) => Some(worktree_id),
            Err(WorktreeSelectionError::None) => {
                self.chat_widget.add_info_message(
                    "No Codewith-managed worktrees".to_string(),
                    Some("Use /worktree to inspect managed worktree policy.".to_string()),
                );
                None
            }
            Err(WorktreeSelectionError::Multiple(worktrees)) => {
                self.show_worktree_picker(worktrees, policy, action);
                self.chat_widget.add_info_message(
                    format!("Choose a worktree to {action}"),
                    Some(worktree_action_usage(action)),
                );
                None
            }
        }
    }

    async fn current_worktree_base_repo_path(&self) -> Option<String> {
        resolve_root_git_project_for_trust(LOCAL_FS.as_ref(), &self.config.cwd)
            .await
            .map(|path| path.to_string_lossy().to_string())
    }

    fn show_no_matching_worktree(&mut self, worktree_id: String) {
        self.chat_widget.add_info_message(
            "No matching worktree".to_string(),
            Some(format!("Could not find worktree {worktree_id}.")),
        );
    }

    fn show_worktree_picker(
        &mut self,
        worktrees: Vec<Worktree>,
        policy: codex_app_server_protocol::WorktreePolicy,
        action: &'static str,
    ) {
        if action == "read" {
            self.chat_widget
                .show_worktree_read_selector(worktrees, policy);
        } else {
            self.chat_widget.show_worktree_manager(worktrees, policy);
        }
    }
}

enum WorktreeSelectionError {
    None,
    Multiple(Vec<Worktree>),
}

fn one_worktree(mut worktrees: Vec<Worktree>) -> Result<String, WorktreeSelectionError> {
    match worktrees.len() {
        0 => Err(WorktreeSelectionError::None),
        1 => Ok(worktrees.remove(0).worktree_id),
        _ => Err(WorktreeSelectionError::Multiple(worktrees)),
    }
}

fn worktree_action_usage(action: &str) -> String {
    match action {
        "start-agent" => "Use /agent start --worktree <id> <prompt>.".to_string(),
        _ => format!("Use /worktree {action} <id>."),
    }
}
