//! Auth profile picker for `ChatWidget`.

use super::*;
use crate::status::RATE_LIMIT_STALE_THRESHOLD_MINUTES;
use crate::status::RateLimitSnapshotDisplay;
use crate::status::RateLimitWindowDisplay;
use crate::status::format_status_limit_summary;
use codex_login::AuthProfile;
use codex_login::AuthProfileMoveDirection;
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
        let rename_profile_name = profile.name.clone();
        let delete_profile_name = profile.name.clone();
        let move_up_profile_name = profile.name.clone();
        let move_down_profile_name = profile.name.clone();
        let shortcut_actions = vec![
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
                "Enter switch / r rename / d delete / [ up / ] down",
                &usage_hint,
            )),
            is_current: current == Some(profile.name.as_str()),
            actions,
            shortcut_actions,
            dismiss_on_select: true,
            ..Default::default()
        }
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
