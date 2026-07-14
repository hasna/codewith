//! Auth profile picker for `ChatWidget`.

use super::*;
use crate::status::RATE_LIMIT_STALE_THRESHOLD_MINUTES;
use crate::status::RateLimitSnapshotDisplay;
use crate::status::RateLimitWindowDisplay;
use crate::status::format_status_limit_summary;
use codex_login::AuthProfile;
use codex_login::AuthProfileMetadata;
use codex_login::AuthProfileMoveDirection;
use codex_login::AuthProfileSubscriptionProvider;
use codex_login::CLIENT_ID;
use codex_login::ServerOptions as LoginServerOptions;
use codex_login::delete_auth_profile;
use codex_login::ensure_auth_profile_storage_dir;
use codex_login::list_auth_profiles;
use codex_login::run_login_server;
use codex_login::save_auth_profile_metadata;
use codex_protocol::config_types::ForcedLoginMethod;
use crossterm::event::KeyCode;
use std::time::Duration;
use std::time::Instant;

const AUTH_PROFILE_LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const AUTH_PROFILE_USAGE_HEARTBEAT_FAILURE_BACKOFF: Duration = Duration::from_secs(5 * 60);
const AUTH_PROFILE_POPUP_VIEW_ID: &str = "auth-profile-selection";

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

        let selected_idx = self
            .bottom_pane
            .selected_index_for_active_view(AUTH_PROFILE_POPUP_VIEW_ID);
        let current = self.config.selected_auth_profile.clone();
        let heartbeat_targets = self.auth_profile_usage_refresh_targets_for_profiles(&profiles);
        let params = self.profile_selection_view_params(profiles, current.as_deref(), selected_idx);
        self.bottom_pane.show_selection_view(params);
        for target in heartbeat_targets {
            self.app_event_tx.send(AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::Heartbeat,
                target,
            });
        }
    }

    fn profile_selection_view_params(
        &self,
        profiles: Vec<AuthProfile>,
        current: Option<&str>,
        selected_idx: Option<usize>,
    ) -> SelectionViewParams {
        let mut items = Vec::with_capacity(profiles.len() + 2);
        items.push(self.default_auth_profile_item(current.is_none()));
        items.extend(
            profiles
                .into_iter()
                .map(|profile| self.named_auth_profile_item(profile, current)),
        );
        items.push(self.new_auth_profile_item());

        let initial_selected_idx = selected_idx.map(|idx| idx.min(items.len().saturating_sub(1)));
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Profile".bold()));
        header.push(Line::from("Switch auth for this session.".dim()));

        SelectionViewParams {
            view_id: Some(AUTH_PROFILE_POPUP_VIEW_ID),
            footer_note: Some(auth_profile_popup_action_hint()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            initial_selected_idx,
            ..Default::default()
        }
    }

    pub(crate) fn refresh_profile_popup_if_active(&mut self) -> bool {
        let Some(selected_idx) = self
            .bottom_pane
            .selected_index_for_active_view(AUTH_PROFILE_POPUP_VIEW_ID)
        else {
            return false;
        };

        let profiles = match list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => profiles,
            Err(err) => {
                tracing::warn!("failed to refresh auth profile popup: {err}");
                return false;
            }
        };

        let current = self.config.selected_auth_profile.as_deref();
        let params = self.profile_selection_view_params(profiles, current, Some(selected_idx));
        self.bottom_pane
            .replace_selection_view_if_active(AUTH_PROFILE_POPUP_VIEW_ID, params)
    }

    fn default_auth_profile_item(&self, is_current: bool) -> SelectionItem {
        let usage_hint = self.auth_profile_usage_hint(/*profile*/ None);
        let reset_generation = self.rate_limit_reset_generation;
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: None,
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
                resume_queued_input: false,
                reset_generation,
            });
        })];
        SelectionItem {
            name: "default".to_string(),
            description: Some("Root login".to_string()),
            selected_description: Some(usage_hint),
            is_current,
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    fn new_auth_profile_item(&self) -> SelectionItem {
        let reset_generation = self.rate_limit_reset_generation;
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenAuthProfileLoginPrompt { reset_generation });
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
        let description = Some(auth_profile_description(&profile));
        let selected_description = Some(usage_hint);
        let reset_generation = self.rate_limit_reset_generation;
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: Some(profile_name.clone()),
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
                resume_queued_input: false,
                reset_generation,
            });
        })];
        let relogin_profile_name = profile.name.clone();
        let rename_profile_name = profile.name.clone();
        let delete_profile_name = profile.name.clone();
        let settings_profile_name = profile.name.clone();
        let move_up_profile_name = profile.name.clone();
        let move_down_profile_name = profile.name.clone();
        let relogin_reset_generation = reset_generation;
        let rename_reset_generation = reset_generation;
        let delete_reset_generation = reset_generation;
        let settings_reset_generation = reset_generation;
        let move_up_reset_generation = reset_generation;
        let move_down_reset_generation = reset_generation;
        let shortcut_actions = vec![
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('l')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::ReloginAuthProfile {
                        profile: relogin_profile_name.clone(),
                        reset_generation: relogin_reset_generation,
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('r')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::OpenAuthProfileRenamePrompt {
                        profile: rename_profile_name.clone(),
                        reset_generation: rename_reset_generation,
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('d')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::OpenAuthProfileDeleteConfirm {
                        profile: delete_profile_name.clone(),
                        reset_generation: delete_reset_generation,
                    });
                }),
                dismiss_on_select: true,
            },
            SelectionShortcutAction {
                binding: key_hint::plain(KeyCode::Char('s')),
                action: Box::new(move |tx| {
                    tx.send(AppEvent::OpenAuthProfileSettings {
                        profile: settings_profile_name.clone(),
                        reset_generation: settings_reset_generation,
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
                        reset_generation: move_up_reset_generation,
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
                        reset_generation: move_down_reset_generation,
                    });
                }),
                dismiss_on_select: true,
            },
        ];
        SelectionItem {
            name: profile.name.clone(),
            description,
            selected_description,
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
        let reset_generation = self.rate_limit_reset_generation;
        let rename_reset_generation = reset_generation;
        let delete_reset_generation = reset_generation;

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
                            reset_generation: rename_reset_generation,
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
                            reset_generation: delete_reset_generation,
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
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Choose subscription".bold()));
        header.push(Line::from(
            "Create a profile tied to one provider subscription.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            header: Box::new(header),
            items: auth_profile_subscription_provider_items(),
            initial_selected_idx: Some(0),
            ..Default::default()
        });
    }

    pub(crate) fn open_auth_profile_name_prompt(
        &mut self,
        subscription_provider: AuthProfileSubscriptionProvider,
    ) {
        let tx = self.app_event_tx.clone();
        let provider_label = subscription_provider.label();
        let view = CustomPromptView::new(
            format!("{provider_label} profile"),
            "Profile name".to_string(),
            String::new(),
            Some("Use letters, numbers, dots, dashes, or underscores".to_string()),
            Box::new(move |profile: String| {
                tx.send(AppEvent::LoginNewAuthProfile {
                    profile: profile.trim().to_string(),
                    subscription_provider,
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn auth_profile_usage_refresh_targets(&mut self) -> Vec<RateLimitRefreshTarget> {
        let profiles = match list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => profiles,
            Err(err) => {
                tracing::debug!("failed to list auth profiles for usage heartbeat: {err}");
                Vec::new()
            }
        };
        self.auth_profile_usage_refresh_targets_for_profiles(&profiles)
    }

    fn auth_profile_usage_refresh_targets_for_profiles(
        &mut self,
        profiles: &[AuthProfile],
    ) -> Vec<RateLimitRefreshTarget> {
        let mut targets = Vec::new();
        if self.root_auth_profile_supports_usage()
            && self.should_request_auth_profile_usage_heartbeat(/*profile*/ None)
        {
            targets.push(RateLimitRefreshTarget::Root);
        }

        for profile in profiles
            .iter()
            .filter(|profile| profile_supports_usage(profile))
        {
            if self.should_request_auth_profile_usage_heartbeat(Some(profile.name.as_str())) {
                targets.push(RateLimitRefreshTarget::Named(profile.name.clone()));
            }
        }

        targets
    }

    fn root_auth_profile_supports_usage(&self) -> bool {
        if self.config.selected_auth_profile.is_none() && self.should_prefetch_rate_limits() {
            return true;
        }

        codex_login::load_auth_dot_json(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        )
        .ok()
        .flatten()
        .is_some_and(|auth| auth_dot_json_supports_usage(&auth))
    }

    fn should_request_auth_profile_usage_heartbeat(&mut self, profile: Option<&str>) -> bool {
        let heartbeat_interval =
            Duration::from_secs(self.config.auth_profile_auto_switch.heartbeat_interval_secs);
        let heartbeat_freshness = Duration::from_secs(
            self.config
                .auth_profile_auto_switch
                .heartbeat_freshness_secs,
        );
        if self
            .auth_profile_usage_snapshots(profile)
            .is_some_and(|snapshots| {
                auth_profile_usage_snapshots_are_fresh(snapshots, heartbeat_freshness)
            })
        {
            return false;
        }

        let profile = profile.map(str::to_string);
        if self
            .auth_profile_usage_heartbeat_failed_at_by_profile
            .get(&profile)
            .is_some_and(|failed_at| {
                failed_at.elapsed() < AUTH_PROFILE_USAGE_HEARTBEAT_FAILURE_BACKOFF
            })
        {
            return false;
        }

        if self
            .auth_profile_usage_heartbeat_requested_at_by_profile
            .get(&profile)
            .is_some_and(|requested_at| requested_at.elapsed() < heartbeat_interval)
        {
            return false;
        }

        self.auth_profile_usage_heartbeat_requested_at_by_profile
            .insert(profile, Instant::now());
        true
    }

    pub(crate) fn record_auth_profile_usage_heartbeat_success(&mut self, profile: Option<String>) {
        self.auth_profile_usage_heartbeat_failed_at_by_profile
            .remove(&profile);
    }

    pub(crate) fn record_auth_profile_usage_heartbeat_failure(&mut self, profile: Option<String>) {
        self.auth_profile_usage_heartbeat_failed_at_by_profile
            .insert(profile, Instant::now());
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

    pub(crate) fn start_auth_profile_login(
        &mut self,
        profile: String,
        subscription_provider: AuthProfileSubscriptionProvider,
    ) {
        if let Err(err) = codex_login::validate_auth_profile_name(&profile) {
            self.add_error_message(format!("Invalid auth profile name: {err}"));
            return;
        }

        if subscription_provider == AuthProfileSubscriptionProvider::ChatGpt
            && matches!(
                self.config.forced_login_method,
                Some(ForcedLoginMethod::Api)
            )
        {
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

        if let Err(err) = save_auth_profile_metadata(
            &self.config.codex_home,
            &profile,
            AuthProfileMetadata {
                subscription_provider,
                last_permissions: None,
            },
        ) {
            self.add_error_message(format!("Failed to create auth profile: {err}"));
            return;
        }

        if subscription_provider != AuthProfileSubscriptionProvider::ChatGpt {
            self.app_event_tx.send(AppEvent::AuthProfileLoginCompleted {
                profile,
                success: true,
                error: None,
            });
            return;
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
        let Some(snapshots) = self.auth_profile_usage_snapshots(profile) else {
            return "usage unknown".to_string();
        };
        compact_usage_hint_for_snapshots(snapshots)
    }

    fn auth_profile_usage_snapshots(
        &self,
        profile: Option<&str>,
    ) -> Option<&BTreeMap<String, RateLimitSnapshotDisplay>> {
        let selected_profile = self.config.selected_auth_profile.as_deref();
        let active_profile_matches = selected_profile == profile;
        if active_profile_matches && !self.rate_limit_snapshots_by_limit_id.is_empty() {
            return Some(&self.rate_limit_snapshots_by_limit_id);
        }

        let profile_key = profile.map(str::to_string);
        self.auth_profile_rate_limit_snapshots_by_profile
            .get(&profile_key)
    }
}

fn auth_profile_subscription_provider_items() -> Vec<SelectionItem> {
    [
        (
            AuthProfileSubscriptionProvider::ChatGpt,
            "ChatGPT",
            "Use Codewith browser login with your ChatGPT plan.",
        ),
        (
            AuthProfileSubscriptionProvider::Cursor,
            "Cursor",
            "Tie this profile to your Cursor subscription login.",
        ),
        (
            AuthProfileSubscriptionProvider::Grok,
            "Grok",
            "Tie this profile to your Grok subscription login.",
        ),
    ]
    .into_iter()
    .map(|(subscription_provider, name, description)| {
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenAuthProfileNamePrompt {
                subscription_provider,
            });
        })];
        SelectionItem {
            name: name.to_string(),
            description: Some(description.to_string()),
            selected_description: Some(format!(
                "Create a profile tied to {}.",
                subscription_provider.label()
            )),
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    })
    .collect()
}

fn cleanup_auth_profile_login(
    codex_home: &std::path::Path,
    auth_credentials_store_mode: codex_login::AuthCredentialsStoreMode,
    profile: &str,
) {
    let _ = delete_auth_profile(codex_home, auth_credentials_store_mode, profile);
}

fn auth_profile_description(profile: &AuthProfile) -> String {
    let mut account = profile.subscription_provider.to_string();
    if let Some(plan) = &profile.plan {
        account.push(' ');
        account.push_str(plan);
    } else if let Some(auth_mode) = profile.auth_mode {
        account.push(' ');
        account.push_str(auth_profile_auth_mode_label(auth_mode));
    }
    let mut parts = vec![account];
    if let Some(email) = &profile.email {
        parts.push(email.clone());
    }
    parts.join(" / ")
}

fn auth_profile_auth_mode_label(auth_mode: codex_app_server_protocol::AuthMode) -> &'static str {
    match auth_mode {
        codex_app_server_protocol::AuthMode::ApiKey => "API key",
        codex_app_server_protocol::AuthMode::Chatgpt
        | codex_app_server_protocol::AuthMode::ChatgptAuthTokens
        | codex_app_server_protocol::AuthMode::PersonalAccessToken => "account",
        codex_app_server_protocol::AuthMode::AgentIdentity => "agent identity",
    }
}

fn profile_supports_usage(profile: &AuthProfile) -> bool {
    profile.subscription_provider == AuthProfileSubscriptionProvider::ChatGpt
        && auth_mode_supports_usage(profile.auth_mode)
}

fn auth_mode_supports_usage(auth_mode: Option<codex_app_server_protocol::AuthMode>) -> bool {
    matches!(
        auth_mode,
        Some(
            codex_app_server_protocol::AuthMode::Chatgpt
                | codex_app_server_protocol::AuthMode::ChatgptAuthTokens
                | codex_app_server_protocol::AuthMode::PersonalAccessToken
                | codex_app_server_protocol::AuthMode::AgentIdentity
        )
    )
}

fn auth_dot_json_supports_usage(auth: &codex_login::AuthDotJson) -> bool {
    if let Some(auth_mode) = auth.auth_mode {
        return auth_mode_supports_usage(Some(auth_mode));
    }

    auth.openai_api_key.is_none()
        && (auth.tokens.is_some()
            || auth.personal_access_token.is_some()
            || auth.agent_identity.is_some())
}

fn auth_profile_popup_action_hint() -> Line<'static> {
    Line::from("Enter switch / l relogin / r rename / d delete / s settings / [ up / ] down")
}

fn compact_usage_hint_for_snapshots(
    snapshots: &BTreeMap<String, RateLimitSnapshotDisplay>,
) -> String {
    let Some(snapshot) = usage_snapshot_with_windows(snapshots) else {
        return "usage unknown".to_string();
    };

    compact_usage_hint_for_snapshot(snapshot)
}

fn compact_usage_hint_for_snapshot(snapshot: &RateLimitSnapshotDisplay) -> String {
    let mut hints = Vec::new();
    if let Some(primary) = snapshot.primary.as_ref() {
        hints.push(compact_usage_hint_for_window(
            primary, /*is_secondary*/ false,
        ));
    }
    if let Some(secondary) = snapshot.secondary.as_ref() {
        hints.push(compact_usage_hint_for_window(
            secondary, /*is_secondary*/ true,
        ));
    }

    if hints.is_empty() {
        return "usage unknown".to_string();
    }

    let hint = hints.join(", ");
    if Local::now().signed_duration_since(snapshot.captured_at)
        > chrono::Duration::minutes(RATE_LIMIT_STALE_THRESHOLD_MINUTES)
    {
        format!("stale {hint}")
    } else {
        hint
    }
}

fn auth_profile_usage_snapshots_are_fresh(
    snapshots: &BTreeMap<String, RateLimitSnapshotDisplay>,
    freshness: Duration,
) -> bool {
    let freshness =
        chrono::Duration::seconds(i64::try_from(freshness.as_secs()).unwrap_or(i64::MAX));
    usage_snapshot_with_windows(snapshots).is_some_and(|snapshot| {
        Local::now().signed_duration_since(snapshot.captured_at) <= freshness
    })
}

fn usage_snapshot_with_windows(
    snapshots: &BTreeMap<String, RateLimitSnapshotDisplay>,
) -> Option<&RateLimitSnapshotDisplay> {
    snapshots
        .get("codex")
        .filter(|snapshot| usage_snapshot_has_windows(snapshot))
        .or_else(|| {
            snapshots
                .values()
                .find(|snapshot| usage_snapshot_has_windows(snapshot))
        })
}

fn usage_snapshot_has_windows(snapshot: &RateLimitSnapshotDisplay) -> bool {
    snapshot.primary.is_some() || snapshot.secondary.is_some()
}

fn compact_usage_hint_for_window(window: &RateLimitWindowDisplay, is_secondary: bool) -> String {
    let label = limit_label_for_window(window.window_minutes, is_secondary);
    let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
    format!("{label} {}", format_status_limit_summary(remaining))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use pretty_assertions::assert_eq;

    fn snapshot(
        captured_at: chrono::DateTime<Local>,
        primary: Option<RateLimitWindowDisplay>,
        secondary: Option<RateLimitWindowDisplay>,
    ) -> RateLimitSnapshotDisplay {
        RateLimitSnapshotDisplay {
            limit_name: "test".to_string(),
            captured_at,
            primary,
            secondary,
            credits: None,
            individual_limit: None,
        }
    }

    fn window(used_percent: f64, window_minutes: i64) -> RateLimitWindowDisplay {
        RateLimitWindowDisplay {
            used_percent,
            resets_at: None,
            window_minutes: Some(window_minutes),
        }
    }

    #[test]
    fn compact_usage_hint_skips_empty_codex_snapshot() {
        let now = Local::now();
        let snapshots = BTreeMap::from([
            ("codex".to_string(), snapshot(now, None, None)),
            (
                "codex_model".to_string(),
                snapshot(now, Some(window(42.0, 5 * 60)), None),
            ),
        ]);

        assert_eq!("5h 58% left", compact_usage_hint_for_snapshots(&snapshots));
    }

    #[test]
    fn auth_profile_usage_freshness_requires_displayable_usage() {
        let now = Local::now();
        let snapshots = BTreeMap::from([("codex".to_string(), snapshot(now, None, None))]);

        assert!(!auth_profile_usage_snapshots_are_fresh(
            &snapshots,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn auth_profile_usage_freshness_uses_displayable_snapshot() {
        let now = Local::now();
        let snapshots = BTreeMap::from([
            ("codex".to_string(), snapshot(now, None, None)),
            (
                "codex_model".to_string(),
                snapshot(
                    now - ChronoDuration::seconds(30),
                    Some(window(42.0, 5 * 60)),
                    None,
                ),
            ),
        ]);

        assert!(auth_profile_usage_snapshots_are_fresh(
            &snapshots,
            Duration::from_secs(60)
        ));
    }
}
