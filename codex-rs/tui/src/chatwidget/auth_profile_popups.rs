//! Auth profile picker for `ChatWidget`.

use super::*;
use crate::status::RATE_LIMIT_STALE_THRESHOLD_MINUTES;
use crate::status::RateLimitSnapshotDisplay;
use crate::status::RateLimitWindowDisplay;
use crate::status::format_status_limit_summary;
use codex_login::AuthProfile;
use codex_login::AuthProfileMoveDirection;
use codex_login::CLIENT_ID;
use codex_login::ServerOptions as LoginServerOptions;
use codex_login::delete_auth_profile;
use codex_login::ensure_auth_profile_storage_dir;
use codex_login::list_auth_profiles;
use codex_login::run_login_server;
use codex_protocol::config_types::ForcedLoginMethod;
use crossterm::event::KeyCode;
use std::time::Duration;

const AUTH_PROFILE_LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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
        let mut items = Vec::with_capacity(profiles.len() + 2);
        items.push(self.default_auth_profile_item(current.is_none()));
        items.extend(
            profiles
                .into_iter()
                .map(|profile| self.named_auth_profile_item(profile, current)),
        );
        items.push(self.new_auth_profile_item());

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
        let usage_hint = self.auth_profile_usage_hint(None);
        let actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: None,
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
                resume_queued_input: false,
            });
        })];
        SelectionItem {
            name: "default".to_string(),
            description: Some(auth_profile_description_with_usage(
                "Root login",
                &usage_hint,
            )),
            selected_description: Some(auth_profile_description_with_usage(
                "Use the default auth store",
                &usage_hint,
            )),
            is_current,
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    fn new_auth_profile_item(&self) -> SelectionItem {
        let actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::OpenAuthProfileLoginPrompt);
        })];
        SelectionItem {
            name: "Log in new profile".to_string(),
            description: Some("Create a saved auth profile".to_string()),
            selected_description: Some("Enter name and start browser login".to_string()),
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
        let usage_hint = self.auth_profile_usage_hint(Some(profile.name.as_str()));
        let description = Some(auth_profile_description_with_usage(
            &auth_profile_description(&profile),
            &usage_hint,
        ));
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: Some(profile_name.clone()),
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
                resume_queued_input: false,
            });
        })];
        let relogin_profile_name = profile.name.clone();
        let rename_profile_name = profile.name.clone();
        let delete_profile_name = profile.name.clone();
        let settings_profile_name = profile.name.clone();
        let move_up_profile_name = profile.name.clone();
        let move_down_profile_name = profile.name.clone();
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
                    tx.send(AppEvent::OpenAuthProfileRenamePrompt {
                        profile: rename_profile_name.clone(),
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('d')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::OpenAuthProfileDeleteConfirm {
                        profile: delete_profile_name.clone(),
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
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('[')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::MoveAuthProfile {
                        profile: move_up_profile_name.clone(),
                        direction: AuthProfileMoveDirection::Up,
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char(']')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::MoveAuthProfile {
                        profile: move_down_profile_name.clone(),
                        direction: AuthProfileMoveDirection::Down,
                    });
                }),
                dismiss_on_select: true,
            },
        ];
        SelectionItem {
            name: profile.name.clone(),
            description,
            selected_description: Some(auth_profile_description_with_usage(
                "Enter switch / l relogin / r rename / d delete / s settings / [ up / ] down",
                &usage_hint,
            )),
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

    pub(crate) fn open_auth_profile_login_prompt(&mut self) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Log in new profile".to_string(),
            "Profile name".to_string(),
            String::new(),
            Some("Use letters, numbers, dots, dashes, or underscores".to_string()),
            Box::new(move |profile: String| {
                tx.send(AppEvent::LoginNewAuthProfile {
                    profile: profile.trim().to_string(),
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

    pub(crate) fn start_auth_profile_login(&mut self, profile: String) {
        if let Err(err) = codex_login::validate_auth_profile_name(&profile) {
            self.add_error_message(format!("Invalid auth profile name: {err}"));
            return;
        }

        if matches!(
            self.config.forced_login_method,
            Some(ForcedLoginMethod::Api)
        ) {
            self.add_error_message(
                "ChatGPT browser login is disabled. Use `codewith login --auth-profile <name> --with-api-key` for this profile.".to_string(),
            );
            return;
        }

        match list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => {
                if profiles.iter().any(|existing| existing.name == profile) {
                    self.add_error_message(format!("Auth profile `{profile}` already exists."));
                    return;
                }
            }
            Err(err) => {
                self.add_error_message(format!("Failed to load auth profiles: {err}"));
                return;
            }
        }

        let auth_profile_home =
            match ensure_auth_profile_storage_dir(&self.config.codex_home, &profile) {
                Ok(auth_profile_home) => auth_profile_home,
                Err(err) => {
                    self.add_error_message(format!("Failed to create auth profile: {err}"));
                    return;
                }
            };

        let opts = LoginServerOptions {
            open_browser: true,
            ..LoginServerOptions::new(
                auth_profile_home,
                CLIENT_ID.to_string(),
                self.config.forced_chatgpt_workspace_id.clone(),
                self.config.cli_auth_credentials_store_mode,
            )
        };
        let server = match run_login_server(opts) {
            Ok(server) => server,
            Err(err) => {
                cleanup_auth_profile_login(
                    &self.config.codex_home,
                    self.config.cli_auth_credentials_store_mode,
                    &profile,
                );
                self.add_error_message(format!("Failed to start auth profile login: {err}"));
                return;
            }
        };

        let auth_url = server.auth_url.clone();
        self.add_info_message(
            format!("Complete browser login for auth profile `{profile}`: {auth_url}"),
            /*hint*/ None,
        );

        let tx = self.app_event_tx.clone();
        let codex_home = self.config.codex_home.clone();
        let auth_credentials_store_mode = self.config.cli_auth_credentials_store_mode;
        let shutdown_handle = server.cancel_handle();
        tokio::spawn(async move {
            let result =
                tokio::time::timeout(AUTH_PROFILE_LOGIN_TIMEOUT, server.block_until_done()).await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(format!("Login server error: {err}")),
                Err(_elapsed) => {
                    shutdown_handle.shutdown();
                    Some("Login timed out".to_string())
                }
            };
            if error.is_some() {
                cleanup_auth_profile_login(&codex_home, auth_credentials_store_mode, &profile);
            }
            tx.send(AppEvent::AuthProfileLoginCompleted {
                profile,
                success: error.is_none(),
                error,
            });
        });
    }

    fn auth_profile_usage_hint(&self, profile: Option<&str>) -> String {
        let selected_profile = self.config.selected_auth_profile.as_deref();
        let active_profile_matches = selected_profile == profile;
        let snapshots =
            if active_profile_matches && !self.rate_limit_snapshots_by_limit_id.is_empty() {
                Some(&self.rate_limit_snapshots_by_limit_id)
            } else {
                let profile_key = profile.map(str::to_string);
                self.auth_profile_rate_limit_snapshots_by_profile
                    .get(&profile_key)
            };

        let Some(snapshots) = snapshots else {
            return "usage unknown".to_string();
        };
        compact_usage_hint_for_snapshots(snapshots)
    }
}

fn cleanup_auth_profile_login(
    codex_home: &std::path::Path,
    auth_credentials_store_mode: codex_login::AuthCredentialsStoreMode,
    profile: &str,
) {
    let _ = delete_auth_profile(codex_home, auth_credentials_store_mode, profile);
}

fn auth_profile_description(profile: &AuthProfile) -> String {
    let mut parts = vec![profile.auth_mode.to_string()];
    if let Some(plan) = &profile.plan {
        parts.push(plan.clone());
    }
    if let Some(email) = &profile.email {
        parts.push(email.clone());
    }
    parts.join(" / ")
}

fn auth_profile_description_with_usage(description: &str, usage_hint: &str) -> String {
    format!("{description} / {usage_hint}")
}

fn compact_usage_hint_for_snapshots(
    snapshots: &BTreeMap<String, RateLimitSnapshotDisplay>,
) -> String {
    let Some(snapshot) = snapshots.get("codex").or_else(|| snapshots.values().next()) else {
        return "usage unknown".to_string();
    };

    compact_usage_hint_for_snapshot(snapshot)
}

fn compact_usage_hint_for_snapshot(snapshot: &RateLimitSnapshotDisplay) -> String {
    let mut hints = Vec::new();
    if let Some(secondary) = snapshot.secondary.as_ref() {
        hints.push(compact_usage_hint_for_window(
            secondary, /*is_secondary*/ true,
        ));
    }
    if let Some(primary) = snapshot.primary.as_ref() {
        hints.push(compact_usage_hint_for_window(
            primary, /*is_secondary*/ false,
        ));
    }

    if hints.is_empty() {
        return "usage unknown".to_string();
    }

    let hint = hints.join(" / ");
    if Local::now().signed_duration_since(snapshot.captured_at)
        > chrono::Duration::minutes(RATE_LIMIT_STALE_THRESHOLD_MINUTES)
    {
        format!("stale {hint}")
    } else {
        hint
    }
}

fn compact_usage_hint_for_window(window: &RateLimitWindowDisplay, is_secondary: bool) -> String {
    let label = limit_label_for_window(window.window_minutes, is_secondary);
    let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
    format!("{label} {}", format_status_limit_summary(remaining))
}
