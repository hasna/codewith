//! Auth profile picker for `ChatWidget`.

use super::*;
use codex_login::AuthProfile;
use codex_login::list_auth_profiles;
use crossterm::event::KeyCode;

impl ChatWidget {
    pub(crate) fn open_profile_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Profile selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let profiles = match list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => profiles,
            Err(err) => {
                self.add_error_message(format!("Failed to load auth profiles: {err}"));
                return;
            }
        };

        let current = self.config.selected_auth_profile.as_deref();
        let mut items = Vec::with_capacity(profiles.len() + 1);
        items.push(self.default_auth_profile_item(current.is_none()));
        items.extend(
            profiles
                .into_iter()
                .map(|profile| self.named_auth_profile_item(profile, current)),
        );

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Profile".bold()));
        header.push(Line::from("Switch auth for this session.".dim()));
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            ..Default::default()
        });
    }

    fn default_auth_profile_item(&self, is_current: bool) -> SelectionItem {
        let actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: None,
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
                resume_queued_input: false,
            });
        })];
        SelectionItem {
            name: "default".to_string(),
            description: Some("Root login".to_string()),
            selected_description: Some("Use the default auth store".to_string()),
            is_current,
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    fn named_auth_profile_item(
        &self,
        profile: AuthProfile,
        current: Option<&str>,
    ) -> SelectionItem {
        let profile_name = profile.name.clone();
        let description = auth_profile_description(&profile);
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: Some(profile_name.clone()),
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
                resume_queued_input: false,
            });
        })];
        let rename_profile_name = profile.name.clone();
        let delete_profile_name = profile.name.clone();
        let relogin_profile_name = profile.name.clone();
        let shortcut_actions = vec![
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('l')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::ReloginAuthProfile {
                        profile: relogin_profile_name.clone(),
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('r')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::ReloginAuthProfile {
                        profile: relogin_profile_name.clone(),
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('s')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::OpenAuthProfileSettings {
                        profile: settings_profile_name.clone(),
                    });
                }),
                dismiss_on_select: true,
            },
        ];
        SelectionItem {
            name: profile.name.clone(),
            description,
            selected_description: Some(
                "Enter switch / l relogin / r rename / d delete".to_string(),
            ),
            is_current: current == Some(profile.name.as_str()),
            actions,
            shortcut_actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    pub(crate) fn open_auth_profile_settings_popup(&mut self, profile: String) {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Profile settings".bold()));
        header.push(Line::from(
            format!("Manage auth profile `{profile}`.").dim(),
        ));

        let rename_profile = profile.clone();
        let delete_profile = profile.clone();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            header: Box::new(header),
            items: vec![
                SelectionItem {
                    name: "Rename profile".to_string(),
                    description: Some(format!("Rename `{profile}`.")),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::OpenAuthProfileRenamePrompt {
                            profile: rename_profile.clone(),
                        });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Delete profile".to_string(),
                    description: Some(format!("Remove `{profile}` from saved auth profiles.")),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::OpenAuthProfileDeleteConfirm {
                            profile: delete_profile.clone(),
                        });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Cancel".to_string(),
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            initial_selected_idx: Some(0),
            ..Default::default()
        });
    }

    pub(crate) fn open_auth_profile_rename_prompt(&mut self, profile: String) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Rename profile".to_string(),
            "Type a new profile name and press Enter".to_string(),
            profile.clone(),
            Some(format!("Current: {profile}")),
            Box::new(move |new_name: String| {
                if let Err(err) = codex_login::validate_auth_profile_name(&new_name) {
                    tx.send(AppEvent::InsertHistoryCell(Box::new(
                        history_cell::new_error_event(format!("Invalid auth profile name: {err}")),
                    )));
                    return;
                }
                tx.send(AppEvent::RenameAuthProfile {
                    old_name: profile.clone(),
                    new_name,
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn open_auth_profile_delete_confirm(&mut self, profile: String) {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Delete profile".bold()));
        header.push(Line::from(
            format!("Delete auth profile `{profile}`?").dim(),
        ));
        header.push(Line::from("This removes only the saved profile.".dim()));

        let delete_profile = profile.clone();
        let delete_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::DeleteAuthProfile {
                profile: delete_profile.clone(),
            });
        })];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            header: Box::new(header),
            items: vec![
                SelectionItem {
                    name: "Delete".to_string(),
                    description: Some(format!("Remove `{profile}` from saved auth profiles.")),
                    actions: delete_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Cancel".to_string(),
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            initial_selected_idx: Some(1),
            ..Default::default()
        });
    }
}

fn auth_profile_description(profile: &AuthProfile) -> Option<String> {
    let mut parts = vec![profile.auth_mode.to_string()];
    if let Some(plan) = &profile.plan {
        parts.push(plan.clone());
    }
    if let Some(email) = &profile.email {
        parts.push(email.clone());
    }
    Some(parts.join(" / "))
}
