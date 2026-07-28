use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::session::session::Session;
use crate::usage_profile_health::UsageProfileAutoSwitchWindow;
use crate::usage_profile_health::UsageProfileCooldownKey;
use crate::usage_profile_health::UsageProfileHealth;
use crate::usage_profile_health::UsageProfileRateLimitSnapshot;
use crate::usage_profile_health::UsageProfileRateLimitWindow;
use crate::usage_profile_health::choose_profile_for_auto_switch;
use crate::usage_profile_health::exhausted_auto_switch_window;
use crate::usage_profile_health::usage_health_for_snapshots;
use crate::usage_profile_health::usage_limit_matches_auto_switch_config;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use codex_login::list_auth_profiles;
use codex_protocol::error::UsageLimitReachedError;
use codex_protocol::protocol::RateLimitSnapshot;

#[derive(Debug, Default)]
pub(crate) struct AuthProfileAutoSwitchTurnState {
    attempted_profiles: HashSet<Option<String>>,
    known_health_by_profile: BTreeMap<String, UsageProfileHealth>,
    exhausted_profile_cooldowns: HashSet<UsageProfileCooldownKey>,
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

        let rate_limit_snapshot = err.rate_limits.as_deref().map(core_rate_limit_snapshot);
        if !usage_limit_matches_auto_switch_config(
            &auto_switch_config,
            rate_limit_snapshot.as_ref(),
        ) {
            return None;
        }

        let current_profile = sess.selected_auth_profile().await;
        self.attempted_profiles.insert(current_profile.clone());
        self.record_profile_health(
            current_profile.as_ref(),
            rate_limit_snapshot.as_ref(),
            &auto_switch_config,
        );

        let profiles =
            match list_auth_profiles(&config.codex_home, config.cli_auth_credentials_store_mode) {
                Ok(profiles) => profiles,
                Err(err) => {
                    tracing::warn!("failed to list auth profiles for auto-switch: {err}");
                    return None;
                }
            };
        let ordered = ordered_auth_profiles(&auto_switch_config.profiles, &profiles);
        next_profile_from_known_health(
            &auto_switch_config,
            current_profile.as_deref(),
            &ordered,
            &self.attempted_profiles,
            &self.exhausted_profile_cooldowns,
            &self.known_health_by_profile,
        )
    }

    fn record_profile_health(
        &mut self,
        profile: Option<&String>,
        snapshot: Option<&UsageProfileRateLimitSnapshot<'_>>,
        config: &crate::config::AuthProfileAutoSwitchConfig,
    ) {
        let (Some(profile), Some(snapshot)) = (profile, snapshot) else {
            return;
        };
        let trigger_window = exhausted_auto_switch_window(snapshot, config);
        let health = usage_health_for_snapshots(
            &[*snapshot],
            config,
            trigger_window.map(|window| window.label),
            /*is_fresh*/ true,
        );
        self.known_health_by_profile.insert(profile.clone(), health);
        if let Some(window) = trigger_window {
            self.exhausted_profile_cooldowns
                .insert(profile_cooldown_key(profile, snapshot, window));
        }
    }
}

fn core_rate_limit_snapshot(snapshot: &RateLimitSnapshot) -> UsageProfileRateLimitSnapshot<'_> {
    UsageProfileRateLimitSnapshot {
        limit_id: snapshot.limit_id.as_deref(),
        limit_name: snapshot.limit_name.as_deref(),
        primary: snapshot.primary.as_ref().map(core_rate_limit_window),
        secondary: snapshot.secondary.as_ref().map(core_rate_limit_window),
    }
}

fn core_rate_limit_window(
    window: &codex_protocol::protocol::RateLimitWindow,
) -> UsageProfileRateLimitWindow {
    UsageProfileRateLimitWindow {
        used_percent: window.used_percent,
        window_minutes: window.window_minutes,
        resets_at: window.resets_at,
    }
}

fn profile_cooldown_key(
    profile: &str,
    snapshot: &UsageProfileRateLimitSnapshot<'_>,
    window: UsageProfileAutoSwitchWindow,
) -> UsageProfileCooldownKey {
    UsageProfileCooldownKey::new(
        Some(profile.to_string()),
        snapshot.limit_id.unwrap_or("codex"),
        window,
    )
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

fn next_profile_from_known_health(
    config: &crate::config::AuthProfileAutoSwitchConfig,
    current_profile: Option<&str>,
    ordered: &[String],
    attempted_profiles: &HashSet<Option<String>>,
    exhausted_profile_cooldowns: &HashSet<UsageProfileCooldownKey>,
    known_health_by_profile: &BTreeMap<String, UsageProfileHealth>,
) -> Option<String> {
    let candidates = auth_profile_candidates(
        current_profile,
        ordered,
        attempted_profiles,
        exhausted_profile_cooldowns,
    );
    choose_profile_for_auto_switch(config, &candidates, known_health_by_profile).selected_profile
}

fn auth_profile_candidates(
    current_profile: Option<&str>,
    ordered: &[String],
    attempted_profiles: &HashSet<Option<String>>,
    exhausted_profile_cooldowns: &HashSet<UsageProfileCooldownKey>,
) -> Vec<String> {
    next_untried_profiles(current_profile, ordered, attempted_profiles)
        .into_iter()
        .filter(|profile| !profile_has_exhausted_cooldown(profile, exhausted_profile_cooldowns))
        .collect()
}

fn next_untried_profiles(
    current_profile: Option<&str>,
    ordered: &[String],
    attempted_profiles: &HashSet<Option<String>>,
) -> Vec<String> {
    if ordered.is_empty() {
        return Vec::new();
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
        .filter(|profile| {
            current_profile != Some(profile.as_str())
                && !attempted_profiles.contains(&Some((*profile).clone()))
        })
        .cloned()
        .collect()
}

fn profile_has_exhausted_cooldown(
    profile: &str,
    exhausted_profile_cooldowns: &HashSet<UsageProfileCooldownKey>,
) -> bool {
    exhausted_profile_cooldowns
        .iter()
        .any(|cooldown| cooldown.profile.as_deref() == Some(profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthProfileAutoSwitchConfig;
    use crate::config::AuthProfileAutoSwitchStrategy;
    use crate::usage_profile_health::FIVE_HOUR_LIMIT_LABEL;
    use crate::usage_profile_health::UsageProfileScore;
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

    fn health(remaining_percent: f64) -> UsageProfileHealth {
        UsageProfileHealth::Healthy(UsageProfileScore {
            trigger_remaining_percent: remaining_percent,
            limiting_remaining_percent: remaining_percent,
        })
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
            vec!["account003".to_string()],
            next_untried_profiles(Some("account002"), &ordered, &attempted)
        );

        let attempted = HashSet::from([
            Some("account001".to_string()),
            Some("account002".to_string()),
            Some("account003".to_string()),
        ]);
        assert_eq!(
            Vec::<String>::new(),
            next_untried_profiles(Some("account003"), &ordered, &attempted)
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

        let five_hour_snapshot = codex_snapshot(5 * 60);
        let five_hour = core_rate_limit_snapshot(&five_hour_snapshot);
        assert!(usage_limit_matches_config(&config, Some(&five_hour)));

        config.on_5h_limit = false;
        let five_hour_snapshot = codex_snapshot(5 * 60);
        let five_hour = core_rate_limit_snapshot(&five_hour_snapshot);
        assert!(!usage_limit_matches_config(&config, Some(&five_hour)));

        let weekly_snapshot = codex_snapshot(7 * 24 * 60);
        let weekly = core_rate_limit_snapshot(&weekly_snapshot);
        assert!(usage_limit_matches_config(&config, Some(&weekly)));

        config.on_weekly_limit = false;
        let weekly_snapshot = codex_snapshot(7 * 24 * 60);
        let weekly = core_rate_limit_snapshot(&weekly_snapshot);
        assert!(!usage_limit_matches_config(&config, Some(&weekly)));
    }

    fn usage_limit_matches_config(
        config: &AuthProfileAutoSwitchConfig,
        snapshot: Option<&UsageProfileRateLimitSnapshot<'_>>,
    ) -> bool {
        usage_limit_matches_auto_switch_config(config, snapshot)
    }

    #[test]
    fn highest_available_skips_known_exhausted_profile_and_prefers_known_healthy_profile() {
        let mut config = AuthProfileAutoSwitchConfig {
            enabled: true,
            strategy: AuthProfileAutoSwitchStrategy::HighestAvailable,
            ..Default::default()
        };
        config.profiles = vec![
            "account001".to_string(),
            "account002".to_string(),
            "account003".to_string(),
        ];
        let ordered = config.profiles.clone();
        let attempted = HashSet::from([Some("account001".to_string())]);
        let exhausted_profile_cooldowns = HashSet::from([UsageProfileCooldownKey {
            profile: Some("account002".to_string()),
            limit_id: "codex".to_string(),
            window_label: FIVE_HOUR_LIMIT_LABEL.to_string(),
            resets_at: Some(123),
        }]);
        let known_health_by_profile = BTreeMap::from([
            (
                "account002".to_string(),
                UsageProfileHealth::Exhausted {
                    retry_at: Some(123),
                },
            ),
            ("account003".to_string(), health(/*remaining_percent*/ 70.0)),
        ]);

        assert_eq!(
            Some("account003".to_string()),
            next_profile_from_known_health(
                &config,
                Some("account001"),
                &ordered,
                &attempted,
                &exhausted_profile_cooldowns,
                &known_health_by_profile,
            )
        );
    }
}
