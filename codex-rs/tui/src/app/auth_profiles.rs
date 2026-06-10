use super::*;

const AUTH_PROFILE_RELOGIN_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 10 * 60);

impl App {
    pub(super) fn relogin_auth_profile(&mut self, profile: String) {
        if matches!(
            self.config.forced_login_method,
            Some(codex_protocol::config_types::ForcedLoginMethod::Api)
        ) {
            self.chat_widget.add_error_message(
                "ChatGPT login is disabled. Use API key login instead.".to_string(),
            );
            return;
        }

        if let Err(err) = codex_login::load_auth_profile(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
            profile.as_str(),
        ) {
            self.chat_widget
                .add_error_message(format!("Failed to load auth profile `{profile}`: {err}"));
            return;
        }

        let auth_storage_home =
            match codex_login::ensure_auth_profile_storage_dir(&self.config.codex_home, &profile) {
                Ok(path) => path,
                Err(err) => {
                    self.chat_widget.add_error_message(format!(
                        "Failed to prepare auth profile `{profile}` for relogin: {err}"
                    ));
                    return;
                }
            };

        let mut opts = codex_login::ServerOptions::new(
            auth_storage_home,
            codex_login::CLIENT_ID.to_string(),
            self.config.forced_chatgpt_workspace_id.clone(),
            self.config.cli_auth_credentials_store_mode,
        );
        opts.open_browser = false;

        let server = match codex_login::run_login_server(opts) {
            Ok(server) => server,
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to start relogin for auth profile `{profile}`: {err}"
                ));
                return;
            }
        };

        let auth_url = server.auth_url.clone();
        let shutdown_handle = server.cancel_handle();
        if let Err(err) = webbrowser::open(&auth_url) {
            tracing::warn!(
                profile,
                url = %auth_url,
                "failed to open browser for auth profile relogin: {err}"
            );
            self.chat_widget.add_info_message(
                format!(
                    "Relogin for auth profile `{profile}` started. Open this URL to continue: {auth_url}"
                ),
                /*hint*/ None,
            );
        } else {
            self.chat_widget.add_info_message(
                format!("Relogin for auth profile `{profile}` started in your browser."),
                /*hint*/ None,
            );
        }

        let app_event_tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result =
                match tokio::time::timeout(AUTH_PROFILE_RELOGIN_TIMEOUT, server.block_until_done())
                    .await
                {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(err)) => Err(format!("Login server error: {err}")),
                    Err(_) => {
                        shutdown_handle.shutdown();
                        Err("Login timed out".to_string())
                    }
                };
            app_event_tx.send(AppEvent::AuthProfileReloginFinished { profile, result });
        });
    }

    pub(super) fn finish_auth_profile_relogin(
        &mut self,
        profile: String,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                if self.config.selected_auth_profile.as_deref() == Some(profile.as_str()) {
                    self.chat_widget
                        .submit_op(AppCommand::override_turn_context_auth_profile(Some(
                            profile.clone(),
                        )));
                }
                self.chat_widget.add_info_message(
                    format!("Auth profile `{profile}` relogin completed."),
                    /*hint*/ None,
                );
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Auth profile `{profile}` relogin failed: {err}"));
            }
        }
    }

    pub(super) fn rename_auth_profile(&mut self, old_name: String, new_name: String) {
        let was_selected = self.config.selected_auth_profile.as_deref() == Some(old_name.as_str());
        match codex_login::rename_auth_profile(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
            old_name.as_str(),
            new_name.as_str(),
        ) {
            Ok(profile) => {
                if was_selected {
                    self.chat_widget
                        .submit_op(AppCommand::override_turn_context_auth_profile(Some(
                            profile.name.clone(),
                        )));
                }
                self.chat_widget.add_info_message(
                    format!("Auth profile `{old_name}` renamed to `{}`.", profile.name),
                    /*hint*/ None,
                );
                self.chat_widget.open_profile_popup();
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to rename auth profile: {err}"));
            }
        }
    }

    pub(super) fn delete_auth_profile(&mut self, profile: String) {
        let was_selected = self.config.selected_auth_profile.as_deref() == Some(profile.as_str());
        match codex_login::remove_auth_profile(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
            profile.as_str(),
        ) {
            Ok(()) => {
                if was_selected {
                    self.chat_widget
                        .submit_op(AppCommand::override_turn_context_auth_profile(
                            /*auth_profile*/ None,
                        ));
                }
                self.chat_widget.add_info_message(
                    format!("Auth profile `{profile}` deleted."),
                    /*hint*/ None,
                );
                self.chat_widget.open_profile_popup();
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to delete auth profile: {err}"));
            }
        }
    }
}
