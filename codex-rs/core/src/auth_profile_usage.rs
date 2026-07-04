//! Shared auth-profile usage health scoring for Codex/ChatGPT accounts.

use crate::config::AuthProfileAutoSwitchConfig;
use crate::config::AuthProfileAutoSwitchStrategy;
use codex_backend_client::TokenUsageProfile;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use codex_protocol::protocol::RateLimitSnapshot;
use serde::Serialize;
use std::collections::HashSet;

const FIVE_HOUR_LIMIT_LABEL: &str = "5h";
const WEEKLY_LIMIT_LABEL: &str = "weekly";
pub const MAX_TOKEN_PROFILE_DAILY_BUCKETS: usize = 31;

/// Health classification for one profile's Codex usage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuthProfileUsageHealth {
    /// Usage windows are available and have remaining capacity.
    Healthy {
        /// Lowest remaining percentage among enabled Codex windows.
        remaining_percent: f64,
        /// Reset timestamp for the limiting window, if known.
        resets_at: Option<i64>,
    },
    /// At least one enabled Codex window is exhausted.
    Exhausted {
        /// Earliest reset timestamp across exhausted enabled windows, if known.
        retry_at: Option<i64>,
    },
    /// Usage could not be determined. This is intentionally not treated as exhausted.
    Unknown,
}

/// Recommendation result for model-visible profile usage decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthProfileUsageRecommendation {
    /// Recommended profile name. `None` means the default/root auth profile.
    pub profile: Option<String>,
    /// Stable short reason suitable for model-visible JSON.
    pub reason: AuthProfileUsageRecommendationReason,
}

/// Stable recommendation reason codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthProfileUsageRecommendationReason {
    CurrentProfileHealthy,
    CurrentProfileUnknown,
    CurrentProfileUnavailable,
    SelectedHighestRemaining,
    SelectedOrderedHealthy,
    SelectedUnknownFallback,
    NoAvailableProfiles,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageProfileResponse {
    pub summary: TokenUsageProfileSummary,
    pub daily_usage_buckets: Option<Vec<TokenUsageProfileDailyBucket>>,
    pub daily_usage_bucket_count: Option<usize>,
    pub daily_usage_buckets_truncated: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageProfileSummary {
    pub lifetime_tokens: Option<i64>,
    pub peak_daily_tokens: Option<i64>,
    pub longest_running_turn_sec: Option<i64>,
    pub current_streak_days: Option<i64>,
    pub longest_streak_days: Option<i64>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageProfileDailyBucket {
    pub start_date: String,
    pub tokens: i64,
}

impl From<TokenUsageProfile> for TokenUsageProfileResponse {
    fn from(profile: TokenUsageProfile) -> Self {
        let stats = profile.stats;
        let daily_usage_bucket_count = stats.daily_usage_buckets.as_ref().map(Vec::len);
        let daily_usage_buckets_truncated =
            daily_usage_bucket_count.is_some_and(|count| count > MAX_TOKEN_PROFILE_DAILY_BUCKETS);
        Self {
            summary: TokenUsageProfileSummary {
                lifetime_tokens: stats.lifetime_tokens,
                peak_daily_tokens: stats.peak_daily_tokens,
                longest_running_turn_sec: stats.longest_running_turn_sec,
                current_streak_days: stats.current_streak_days,
                longest_streak_days: stats.longest_streak_days,
            },
            daily_usage_buckets: stats.daily_usage_buckets.map(|buckets| {
                let skip = buckets
                    .len()
                    .saturating_sub(MAX_TOKEN_PROFILE_DAILY_BUCKETS);
                buckets
                    .into_iter()
                    .skip(skip)
                    .map(|bucket| TokenUsageProfileDailyBucket {
                        start_date: bucket.start_date,
                        tokens: bucket.tokens,
                    })
                    .collect()
            }),
            daily_usage_bucket_count,
            daily_usage_buckets_truncated,
        }
    }
}

/// Scores rate-limit snapshots using the Codewith auth-profile auto-switch window settings.
///
/// Only the 5h and weekly Codex windows participate. Missing or unsupported windows return
/// [`AuthProfileUsageHealth::Unknown`] rather than exhausted.
pub fn usage_health_for_snapshots(
    snapshots: &[RateLimitSnapshot],
    config: &AuthProfileAutoSwitchConfig,
) -> AuthProfileUsageHealth {
    let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
        .or_else(|| snapshots.first())
    else {
        return AuthProfileUsageHealth::Unknown;
    };

    let mut backend_blocked = backend_usage_is_blocked(snapshot);
    let mut has_enabled_window = false;
    let mut limiting_remaining_percent = 100.0;
    let mut limiting_resets_at = None;
    let mut retry_at = None;
    if let Some(limit) = snapshot.individual_limit.as_ref()
        && limit.remaining_percent <= 0
    {
        merge_retry_at(&mut retry_at, Some(limit.resets_at));
    }
    for (window, is_secondary) in [
        snapshot.secondary.as_ref().map(|window| (window, true)),
        snapshot.primary.as_ref().map(|window| (window, false)),
    ]
    .into_iter()
    .flatten()
    {
        let label = limit_label_for_window(window.window_minutes, is_secondary);
        if !auth_profile_auto_switch_label_enabled(label.as_str(), config) {
            continue;
        }

        has_enabled_window = true;
        let remaining_percent = (100.0 - window.used_percent).clamp(0.0, 100.0);
        if remaining_percent < limiting_remaining_percent {
            limiting_remaining_percent = remaining_percent;
            limiting_resets_at = window.resets_at;
        }
        if window.used_percent >= 100.0 {
            backend_blocked = true;
            merge_retry_at(&mut retry_at, window.resets_at);
        }
    }

    if backend_blocked {
        return AuthProfileUsageHealth::Exhausted { retry_at };
    }
    if !has_enabled_window {
        return AuthProfileUsageHealth::Unknown;
    }
    if limiting_remaining_percent <= 0.0 || retry_at.is_some() {
        return AuthProfileUsageHealth::Exhausted { retry_at };
    }
    AuthProfileUsageHealth::Healthy {
        remaining_percent: limiting_remaining_percent,
        resets_at: limiting_resets_at,
    }
}

/// Returns configured ChatGPT auth profiles in the order used for auth-profile decisions.
pub fn ordered_chatgpt_auth_profiles(
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

/// Chooses a model-visible recommendation from already-fetched usage health.
pub fn recommend_auth_profile(
    current_profile: Option<&str>,
    strategy: AuthProfileAutoSwitchStrategy,
    ordered_profiles: &[Option<String>],
    health_by_profile: &[(Option<String>, AuthProfileUsageHealth)],
) -> AuthProfileUsageRecommendation {
    match health_for_profile(health_by_profile, current_profile) {
        Some(AuthProfileUsageHealth::Healthy { .. }) => {
            return AuthProfileUsageRecommendation {
                profile: current_profile.map(str::to_string),
                reason: AuthProfileUsageRecommendationReason::CurrentProfileHealthy,
            };
        }
        Some(AuthProfileUsageHealth::Unknown) => {
            return AuthProfileUsageRecommendation {
                profile: current_profile.map(str::to_string),
                reason: AuthProfileUsageRecommendationReason::CurrentProfileUnknown,
            };
        }
        Some(AuthProfileUsageHealth::Exhausted { .. }) => {}
        None => {
            return AuthProfileUsageRecommendation {
                profile: current_profile.map(str::to_string),
                reason: AuthProfileUsageRecommendationReason::CurrentProfileUnavailable,
            };
        }
    }

    let candidates = ordered_profiles
        .iter()
        .filter(|profile| profile.as_deref() != current_profile)
        .collect::<Vec<_>>();
    let mut first_unknown = None;

    match strategy {
        AuthProfileAutoSwitchStrategy::Ordered => {
            for candidate in candidates {
                match health_for_profile(health_by_profile, candidate.as_deref()) {
                    Some(AuthProfileUsageHealth::Healthy { .. }) => {
                        return AuthProfileUsageRecommendation {
                            profile: (*candidate).clone(),
                            reason: AuthProfileUsageRecommendationReason::SelectedOrderedHealthy,
                        };
                    }
                    Some(AuthProfileUsageHealth::Unknown) => {
                        return AuthProfileUsageRecommendation {
                            profile: (*candidate).clone(),
                            reason: AuthProfileUsageRecommendationReason::SelectedUnknownFallback,
                        };
                    }
                    Some(AuthProfileUsageHealth::Exhausted { .. }) | None => {}
                }
            }
        }
        AuthProfileAutoSwitchStrategy::HighestAvailable => {
            let mut best: Option<(Option<String>, f64)> = None;
            for candidate in candidates {
                match health_for_profile(health_by_profile, candidate.as_deref()) {
                    Some(AuthProfileUsageHealth::Healthy {
                        remaining_percent, ..
                    }) => {
                        if best
                            .as_ref()
                            .is_none_or(|(_, best_remaining)| remaining_percent > *best_remaining)
                        {
                            best = Some(((*candidate).clone(), remaining_percent));
                        }
                    }
                    Some(AuthProfileUsageHealth::Unknown) => {
                        if first_unknown.is_none() {
                            first_unknown = Some((*candidate).clone());
                        }
                    }
                    Some(AuthProfileUsageHealth::Exhausted { .. }) | None => {}
                }
            }
            if let Some((profile, _remaining_percent)) = best {
                return AuthProfileUsageRecommendation {
                    profile,
                    reason: AuthProfileUsageRecommendationReason::SelectedHighestRemaining,
                };
            }
        }
    }

    if let Some(profile) = first_unknown {
        return AuthProfileUsageRecommendation {
            profile,
            reason: AuthProfileUsageRecommendationReason::SelectedUnknownFallback,
        };
    }

    AuthProfileUsageRecommendation {
        profile: None,
        reason: AuthProfileUsageRecommendationReason::NoAvailableProfiles,
    }
}

/// Returns whether a captured usage sample is stale for a configured freshness window.
pub fn usage_capture_is_stale(captured_at: i64, now: i64, freshness_secs: u64) -> bool {
    now.saturating_sub(captured_at) > i64::try_from(freshness_secs).unwrap_or(i64::MAX)
}

fn health_for_profile(
    health_by_profile: &[(Option<String>, AuthProfileUsageHealth)],
    profile: Option<&str>,
) -> Option<AuthProfileUsageHealth> {
    health_by_profile
        .iter()
        .find(|(candidate, _health)| candidate.as_deref() == profile)
        .map(|(_, health)| *health)
}

fn auth_profile_auto_switch_label_enabled(
    label: &str,
    config: &AuthProfileAutoSwitchConfig,
) -> bool {
    match label {
        FIVE_HOUR_LIMIT_LABEL => config.on_5h_limit,
        WEEKLY_LIMIT_LABEL => config.on_weekly_limit,
        _ => false,
    }
}

fn limit_label_for_window(window_minutes: Option<i64>, is_secondary: bool) -> String {
    window_minutes
        .and_then(get_limits_duration)
        .unwrap_or_else(|| fallback_limit_label(is_secondary).to_string())
}

fn get_limits_duration(windows_minutes: i64) -> Option<String> {
    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_5_HOURS: i64 = 5 * MINUTES_PER_HOUR;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;

    let windows_minutes = windows_minutes.max(0);

    if is_approximate_window(windows_minutes, MINUTES_PER_5_HOURS) {
        Some(FIVE_HOUR_LIMIT_LABEL.to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_WEEK) {
        Some(WEEKLY_LIMIT_LABEL.to_string())
    } else {
        None
    }
}

fn fallback_limit_label(is_secondary: bool) -> &'static str {
    if is_secondary {
        "secondary usage"
    } else {
        "usage"
    }
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes as f64;
    let expected_minutes = expected_minutes as f64;
    minutes >= expected_minutes * 0.95 && minutes <= expected_minutes * 1.05
}

fn dedupe_profile_names(profiles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .filter(|profile| seen.insert(profile.clone()))
        .collect()
}

fn merge_retry_at(current: &mut Option<i64>, candidate: Option<i64>) {
    let Some(candidate) = candidate else {
        return;
    };
    if current.is_none_or(|current| candidate < current) {
        *current = Some(candidate);
    }
}

fn backend_usage_is_blocked(snapshot: &RateLimitSnapshot) -> bool {
    snapshot.rate_limit_reached_type.is_some()
        || snapshot
            .individual_limit
            .as_ref()
            .is_some_and(|limit| limit.remaining_percent <= 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthProfileAutoSwitchConfig;
    use crate::config::AuthProfileAutoSwitchStrategy;
    use codex_app_server_protocol::AuthMode;
    use codex_backend_client::TokenUsageProfileDailyBucket as BackendTokenUsageProfileDailyBucket;
    use codex_backend_client::TokenUsageProfileStats;
    use codex_protocol::protocol::CreditsSnapshot;
    use codex_protocol::protocol::RateLimitReachedType;
    use codex_protocol::protocol::RateLimitWindow;
    use codex_protocol::protocol::SpendControlLimitSnapshot;
    use pretty_assertions::assert_eq;

    fn config() -> AuthProfileAutoSwitchConfig {
        AuthProfileAutoSwitchConfig {
            enabled: true,
            profiles: vec![
                "work".to_string(),
                "second".to_string(),
                "third".to_string(),
            ],
            on_5h_limit: true,
            on_weekly_limit: true,
            strategy: AuthProfileAutoSwitchStrategy::HighestAvailable,
            heartbeat_interval_secs: 60,
            heartbeat_freshness_secs: 120,
        }
    }

    fn snapshot(primary_used: f64, secondary_used: f64) -> RateLimitSnapshot {
        RateLimitSnapshot {
            limit_id: Some("codex".to_string()),
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: primary_used,
                window_minutes: Some(7 * 24 * 60),
                resets_at: Some(200),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: secondary_used,
                window_minutes: Some(5 * 60),
                resets_at: Some(100),
            }),
            credits: None,
            individual_limit: None,
            plan_type: None,
            rate_limit_reached_type: None,
        }
    }

    fn profile(name: &str, provider: AuthProfileSubscriptionProvider) -> AuthProfile {
        AuthProfile {
            name: name.to_string(),
            subscription_provider: provider,
            auth_mode: (provider == AuthProfileSubscriptionProvider::ChatGpt)
                .then_some(AuthMode::Chatgpt),
            email: None,
            account_id: None,
            plan: None,
            active: false,
        }
    }

    #[test]
    fn usage_health_scores_limiting_enabled_window() {
        assert_eq!(
            AuthProfileUsageHealth::Healthy {
                remaining_percent: 20.0,
                resets_at: Some(100),
            },
            usage_health_for_snapshots(&[snapshot(10.0, 80.0)], &config())
        );
    }

    #[test]
    fn usage_health_detects_exhausted_5h_and_weekly_windows() {
        assert_eq!(
            AuthProfileUsageHealth::Exhausted {
                retry_at: Some(100)
            },
            usage_health_for_snapshots(&[snapshot(50.0, 100.0)], &config())
        );
        assert_eq!(
            AuthProfileUsageHealth::Exhausted {
                retry_at: Some(200)
            },
            usage_health_for_snapshots(&[snapshot(100.0, 20.0)], &config())
        );
    }

    #[test]
    fn usage_health_unknown_when_supported_windows_are_missing_or_disabled() {
        let mut disabled_config = config();
        disabled_config.on_5h_limit = false;
        disabled_config.on_weekly_limit = false;

        assert_eq!(
            AuthProfileUsageHealth::Unknown,
            usage_health_for_snapshots(&[snapshot(10.0, 10.0)], &disabled_config)
        );
        assert_eq!(
            AuthProfileUsageHealth::Unknown,
            usage_health_for_snapshots(&[], &config())
        );
    }

    #[test]
    fn usage_health_ignores_credit_snapshot_when_codex_windows_have_capacity() {
        let mut credit_blocked = snapshot(10.0, 20.0);
        credit_blocked.credits = Some(CreditsSnapshot {
            has_credits: false,
            unlimited: false,
            balance: Some("0".to_string()),
        });
        assert_eq!(
            AuthProfileUsageHealth::Healthy {
                remaining_percent: 80.0,
                resets_at: Some(100),
            },
            usage_health_for_snapshots(&[credit_blocked], &config())
        );
    }

    #[test]
    fn usage_health_detects_backend_reached_type_and_spend_control_blocks() {
        let mut spend_control_blocked = snapshot(10.0, 20.0);
        spend_control_blocked.individual_limit = Some(SpendControlLimitSnapshot {
            limit: "100".to_string(),
            used: "100".to_string(),
            remaining_percent: 0,
            resets_at: 300,
        });
        assert_eq!(
            AuthProfileUsageHealth::Exhausted {
                retry_at: Some(300)
            },
            usage_health_for_snapshots(&[spend_control_blocked], &config())
        );

        let mut reached = snapshot(10.0, 20.0);
        reached.rate_limit_reached_type =
            Some(RateLimitReachedType::WorkspaceMemberCreditsDepleted);
        assert_eq!(
            AuthProfileUsageHealth::Exhausted { retry_at: None },
            usage_health_for_snapshots(&[reached], &config())
        );
    }

    #[test]
    fn ordered_profiles_excludes_non_chatgpt_and_unknown_configured_profiles() {
        let profiles = vec![
            profile("work", AuthProfileSubscriptionProvider::ChatGpt),
            profile("claude", AuthProfileSubscriptionProvider::ClaudeAi),
            profile("second", AuthProfileSubscriptionProvider::ChatGpt),
        ];

        assert_eq!(
            vec!["second".to_string(), "work".to_string()],
            ordered_chatgpt_auth_profiles(
                &[
                    "second".to_string(),
                    "missing".to_string(),
                    "work".to_string(),
                ],
                &profiles,
            )
        );
    }

    #[test]
    fn recommendation_prefers_current_healthy_profile() {
        let health = vec![
            (
                Some("work".to_string()),
                AuthProfileUsageHealth::Healthy {
                    remaining_percent: 10.0,
                    resets_at: None,
                },
            ),
            (
                Some("second".to_string()),
                AuthProfileUsageHealth::Healthy {
                    remaining_percent: 90.0,
                    resets_at: None,
                },
            ),
        ];

        assert_eq!(
            AuthProfileUsageRecommendation {
                profile: Some("work".to_string()),
                reason: AuthProfileUsageRecommendationReason::CurrentProfileHealthy,
            },
            recommend_auth_profile(
                Some("work"),
                AuthProfileAutoSwitchStrategy::HighestAvailable,
                &[Some("work".to_string()), Some("second".to_string())],
                &health,
            )
        );
    }

    #[test]
    fn recommendation_uses_configured_strategy_for_alternatives() {
        let health = vec![
            (
                Some("work".to_string()),
                AuthProfileUsageHealth::Exhausted { retry_at: Some(1) },
            ),
            (
                Some("second".to_string()),
                AuthProfileUsageHealth::Healthy {
                    remaining_percent: 20.0,
                    resets_at: None,
                },
            ),
            (
                Some("third".to_string()),
                AuthProfileUsageHealth::Healthy {
                    remaining_percent: 80.0,
                    resets_at: None,
                },
            ),
        ];
        let ordered = [
            Some("work".to_string()),
            Some("second".to_string()),
            Some("third".to_string()),
        ];

        assert_eq!(
            AuthProfileUsageRecommendation {
                profile: Some("third".to_string()),
                reason: AuthProfileUsageRecommendationReason::SelectedHighestRemaining,
            },
            recommend_auth_profile(
                Some("work"),
                AuthProfileAutoSwitchStrategy::HighestAvailable,
                &ordered,
                &health,
            )
        );
        assert_eq!(
            AuthProfileUsageRecommendation {
                profile: Some("second".to_string()),
                reason: AuthProfileUsageRecommendationReason::SelectedOrderedHealthy,
            },
            recommend_auth_profile(
                Some("work"),
                AuthProfileAutoSwitchStrategy::Ordered,
                &ordered,
                &health,
            )
        );
    }

    #[test]
    fn unknown_usage_is_recommendable_fallback_not_exhausted() {
        let health = vec![
            (
                Some("work".to_string()),
                AuthProfileUsageHealth::Exhausted { retry_at: Some(1) },
            ),
            (Some("second".to_string()), AuthProfileUsageHealth::Unknown),
        ];

        assert_eq!(
            AuthProfileUsageRecommendation {
                profile: Some("second".to_string()),
                reason: AuthProfileUsageRecommendationReason::SelectedUnknownFallback,
            },
            recommend_auth_profile(
                Some("work"),
                AuthProfileAutoSwitchStrategy::HighestAvailable,
                &[Some("work".to_string()), Some("second".to_string())],
                &health,
            )
        );
    }

    #[test]
    fn recommendation_keeps_current_profile_when_current_usage_is_unknown() {
        let health = vec![
            (Some("work".to_string()), AuthProfileUsageHealth::Unknown),
            (
                Some("second".to_string()),
                AuthProfileUsageHealth::Healthy {
                    remaining_percent: 90.0,
                    resets_at: None,
                },
            ),
        ];

        assert_eq!(
            AuthProfileUsageRecommendation {
                profile: Some("work".to_string()),
                reason: AuthProfileUsageRecommendationReason::CurrentProfileUnknown,
            },
            recommend_auth_profile(
                Some("work"),
                AuthProfileAutoSwitchStrategy::HighestAvailable,
                &[Some("work".to_string()), Some("second".to_string())],
                &health,
            )
        );
    }

    #[test]
    fn recommendation_keeps_current_profile_when_current_usage_is_unavailable() {
        let health = vec![(
            Some("second".to_string()),
            AuthProfileUsageHealth::Healthy {
                remaining_percent: 90.0,
                resets_at: None,
            },
        )];

        assert_eq!(
            AuthProfileUsageRecommendation {
                profile: Some("work".to_string()),
                reason: AuthProfileUsageRecommendationReason::CurrentProfileUnavailable,
            },
            recommend_auth_profile(
                Some("work"),
                AuthProfileAutoSwitchStrategy::HighestAvailable,
                &[Some("work".to_string()), Some("second".to_string())],
                &health,
            )
        );
    }

    #[test]
    fn token_usage_profile_response_maps_backend_profile() {
        let response = TokenUsageProfileResponse::from(TokenUsageProfile {
            stats: TokenUsageProfileStats {
                lifetime_tokens: Some(123),
                peak_daily_tokens: Some(45),
                longest_running_turn_sec: Some(67),
                current_streak_days: Some(8),
                longest_streak_days: Some(9),
                daily_usage_buckets: Some(vec![BackendTokenUsageProfileDailyBucket {
                    start_date: "2026-05-29".to_string(),
                    tokens: 10,
                }]),
            },
        });

        assert_eq!(
            response,
            TokenUsageProfileResponse {
                summary: TokenUsageProfileSummary {
                    lifetime_tokens: Some(123),
                    peak_daily_tokens: Some(45),
                    longest_running_turn_sec: Some(67),
                    current_streak_days: Some(8),
                    longest_streak_days: Some(9),
                },
                daily_usage_buckets: Some(vec![TokenUsageProfileDailyBucket {
                    start_date: "2026-05-29".to_string(),
                    tokens: 10,
                }]),
                daily_usage_bucket_count: Some(1),
                daily_usage_buckets_truncated: false,
            }
        );
    }

    #[test]
    fn token_usage_profile_response_caps_daily_buckets() {
        let response = TokenUsageProfileResponse::from(TokenUsageProfile {
            stats: TokenUsageProfileStats {
                lifetime_tokens: None,
                peak_daily_tokens: None,
                longest_running_turn_sec: None,
                current_streak_days: None,
                longest_streak_days: None,
                daily_usage_buckets: Some(
                    (0..35)
                        .map(|day| BackendTokenUsageProfileDailyBucket {
                            start_date: format!("2026-05-{day:02}"),
                            tokens: i64::from(day),
                        })
                        .collect(),
                ),
            },
        });

        let buckets = response.daily_usage_buckets.expect("buckets");
        assert_eq!(buckets.len(), MAX_TOKEN_PROFILE_DAILY_BUCKETS);
        assert_eq!(buckets[0].start_date, "2026-05-04");
        assert_eq!(response.daily_usage_bucket_count, Some(35));
        assert!(response.daily_usage_buckets_truncated);
    }

    #[test]
    fn stale_capture_respects_freshness_window() {
        assert!(!usage_capture_is_stale(100, 159, 60));
        assert!(usage_capture_is_stale(100, 161, 60));
    }
}
