use super::*;

impl App {
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
                    self.config.selected_auth_profile = Some(profile.name.clone());
                    self.chat_widget
                        .set_auth_profile(Some(profile.name.clone()));
                    self.chat_widget
                        .submit_op(AppCommand::override_turn_context_auth_profile(Some(
                            profile.name.clone(),
                        )));
                    self.refresh_status_line();
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
                    self.config.selected_auth_profile = None;
                    self.chat_widget.set_auth_profile(None);
                    self.chat_widget
                        .submit_op(AppCommand::override_turn_context_auth_profile(None));
                    self.refresh_status_line();
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
