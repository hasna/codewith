use std::collections::HashSet;
use std::sync::Arc;

use crate::session::session::Session;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use codex_login::list_auth_profiles;
use codex_protocol::error::UsageLimitReachedError;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;

const FIVE_HOUR_WINDOW_MINUTES: i64 = 5 * 60;
const WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;

#[derive(Debug, Default)]
pub(crate) struct AuthProfileAutoSwitchTurnState {
    attempted_profiles: HashSet<Option<String>>,
}

impl AuthProfileAutoSwitchTurnState {
    pub(crate) async fn next_profile_for_usage_limit(
        &mut self,
        sess: &Arc<Session>,
        err: &UsageLimitReachedError,
    ) -> Option<String> {
        let snapshot = {
            let state = sess.state.lock().await;
            state
                .session_configuration
                .original_config_do_not_use
                .auth_profile_auto_switch
                .enabled
                .then(|| {
                    (
                        state
                            .session_configuration
                            .original_config_do_not_use
                            .auth_profile_auto_switch
                            .clone(),
                        state
                            .session_configuration
                            .original_config_do_not_use
                            .clone(),
                    )
                })
        }?;
        let (auto_switch_config, config) = snapshot;

        if !usage_limit_matches_config(&auto_switch_config, err.rate_limits.as_deref()) {
            return None;
        }

        let current_profile = sess.selected_auth_profile().await;
        self.attempted_profiles.insert(current_profile.clone());

        let profiles =
            match list_auth_profiles(&config.codex_home, config.cli_auth_credentials_store_mode) {
                Ok(profiles) => profiles,
                Err(err) => {
                    tracing::warn!("failed to list auth profiles for auto-switch: {err}");
                    return None;
                }
            };
        let ordered = ordered_auth_profiles(&auto_switch_config.profiles, &profiles);
        next_untried_profile(
            current_profile.as_deref(),
            &ordered,
            &self.attempted_profiles,
        )
    }
}

fn usage_limit_matches_config(
    config: &crate::config::AuthProfileAutoSwitchConfig,
    snapshot: Option<&RateLimitSnapshot>,
) -> bool {
    if !config.enabled {
        return false;
    }

    let Some(snapshot) = snapshot else {
        return true;
    };
    if !is_codex_limit(snapshot) {
        return false;
    }

    let mut saw_supported_window = false;
    for window in [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
    {
        if window.used_percent < 100.0 {
            continue;
        }
        match auto_switch_window(window) {
            Some(AutoSwitchWindow::FiveHour) => {
                saw_supported_window = true;
                if config.on_5h_limit {
                    return true;
                }
            }
            Some(AutoSwitchWindow::Weekly) => {
                saw_supported_window = true;
                if config.on_weekly_limit {
                    return true;
                }
            }
            None => {}
        }
    }

    !saw_supported_window
}

fn is_codex_limit(snapshot: &RateLimitSnapshot) -> bool {
    let id_is_codex = snapshot
        .limit_id
        .as_deref()
        .is_none_or(|limit_id| limit_id.eq_ignore_ascii_case("codex"));
    let name_is_codex = snapshot
        .limit_name
        .as_deref()
        .is_none_or(|limit_name| limit_name.eq_ignore_ascii_case("codex"));
    id_is_codex && name_is_codex
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoSwitchWindow {
    FiveHour,
    Weekly,
}

fn auto_switch_window(window: &RateLimitWindow) -> Option<AutoSwitchWindow> {
    match window.window_minutes {
        Some(FIVE_HOUR_WINDOW_MINUTES) => Some(AutoSwitchWindow::FiveHour),
        Some(WEEKLY_WINDOW_MINUTES) => Some(AutoSwitchWindow::Weekly),
        _ => None,
    }
}

fn ordered_auth_profiles(
    configured_profiles: &[String],
    saved_profiles: &[AuthProfile],
) -> Vec<String> {
    let saved_profiles = saved_profiles
        .iter()
        .filter(|profile| {
            profile.subscription_provider == AuthProfileSubscriptionProvider::ChatGpt
                && profile.auth_mode.is_some()
        })
        .collect::<Vec<_>>();
    let saved_names = saved_profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<HashSet<_>>();

    let ordered = if configured_profiles.is_empty() {
        saved_profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect::<Vec<_>>()
    } else {
        configured_profiles
            .iter()
            .filter(|profile| saved_names.contains(profile.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    dedupe_profile_names(ordered)
}

fn dedupe_profile_names(profiles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .filter(|profile| seen.insert(profile.clone()))
        .collect()
}

fn next_untried_profile(
    current_profile: Option<&str>,
    ordered: &[String],
    attempted_profiles: &HashSet<Option<String>>,
) -> Option<String> {
    if ordered.is_empty() {
        return None;
    }

    let start = current_profile
        .and_then(|current| ordered.iter().position(|profile| profile == current))
        .map(|index| index + 1)
        .unwrap_or(0);
    ordered
        .iter()
        .cycle()
        .skip(start)
        .take(ordered.len())
        .find(|profile| {
            current_profile != Some(profile.as_str())
                && !attempted_profiles.contains(&Some((*profile).clone()))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthProfileAutoSwitchConfig;
    use codex_app_server_protocol::AuthMode;
    use codex_protocol::protocol::RateLimitSnapshot;
    use codex_protocol::protocol::RateLimitWindow;
    use pretty_assertions::assert_eq;

    fn profile(name: &str) -> AuthProfile {
        AuthProfile {
            name: name.to_string(),
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
            auth_mode: Some(AuthMode::Chatgpt),
            email: None,
            account_id: None,
            plan: None,
            active: false,
        }
    }

    fn codex_snapshot(window_minutes: i64) -> RateLimitSnapshot {
        RateLimitSnapshot {
            limit_id: Some("codex".to_string()),
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 100.0,
                window_minutes: Some(window_minutes),
                resets_at: Some(123),
            }),
            secondary: None,
            credits: None,
            individual_limit: None,
            plan_type: None,
            rate_limit_reached_type: None,
        }
    }

    #[test]
    fn next_untried_profile_walks_order_without_repeating_attempted_profiles() {
        let ordered = vec![
            "account001".to_string(),
            "account002".to_string(),
            "account003".to_string(),
        ];
        let attempted = HashSet::from([
            Some("account001".to_string()),
            Some("account002".to_string()),
        ]);

        assert_eq!(
            Some("account003".to_string()),
            next_untried_profile(Some("account002"), &ordered, &attempted)
        );

        let attempted = HashSet::from([
            Some("account001".to_string()),
            Some("account002".to_string()),
            Some("account003".to_string()),
        ]);
        assert_eq!(
            None,
            next_untried_profile(Some("account003"), &ordered, &attempted)
        );
    }

    #[test]
    fn configured_order_filters_unknown_profiles_and_dedupes() {
        let saved = vec![profile("account001"), profile("account002")];
        let configured = vec![
            "account002".to_string(),
            "missing".to_string(),
            "account002".to_string(),
            "account001".to_string(),
        ];

        assert_eq!(
            vec!["account002".to_string(), "account001".to_string()],
            ordered_auth_profiles(&configured, &saved)
        );
    }

    #[test]
    fn usage_limit_matching_respects_enabled_windows() {
        let mut config = AuthProfileAutoSwitchConfig {
            enabled: true,
            ..Default::default()
        };

        assert!(usage_limit_matches_config(
            &config,
            Some(&codex_snapshot(FIVE_HOUR_WINDOW_MINUTES))
        ));

        config.on_5h_limit = false;
        assert!(!usage_limit_matches_config(
            &config,
            Some(&codex_snapshot(FIVE_HOUR_WINDOW_MINUTES))
        ));

        assert!(usage_limit_matches_config(
            &config,
            Some(&codex_snapshot(WEEKLY_WINDOW_MINUTES))
        ));

        config.on_weekly_limit = false;
        assert!(!usage_limit_matches_config(
            &config,
            Some(&codex_snapshot(WEEKLY_WINDOW_MINUTES))
        ));
    }
}
