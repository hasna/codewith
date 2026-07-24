use std::collections::BTreeMap;
use std::time::Duration;

use crate::config::AuthProfileAutoSwitchConfig;
use crate::config::AuthProfileAutoSwitchStrategy;

pub const PRIMARY_LIMIT_FALLBACK_LABEL: &str = "usage";
pub const SECONDARY_LIMIT_FALLBACK_LABEL: &str = "secondary usage";
pub const FIVE_HOUR_LIMIT_LABEL: &str = "5h";
pub const WEEKLY_LIMIT_LABEL: &str = "weekly";

const DAILY_LIMIT_LABEL: &str = "daily";
const MONTHLY_LIMIT_LABEL: &str = "monthly";
const ANNUAL_LIMIT_LABEL: &str = "annual";
const MINUTES_PER_HOUR: i64 = 60;
const MINUTES_PER_5_HOURS: i64 = 5 * MINUTES_PER_HOUR;
const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;
const MINUTES_PER_YEAR: i64 = 365 * MINUTES_PER_DAY;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageProfileRateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageProfileRateLimitSnapshot<'a> {
    pub limit_id: Option<&'a str>,
    pub limit_name: Option<&'a str>,
    pub primary: Option<UsageProfileRateLimitWindow>,
    pub secondary: Option<UsageProfileRateLimitWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageProfileAutoSwitchWindow {
    pub label: &'static str,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageProfileScore {
    pub trigger_remaining_percent: f64,
    pub limiting_remaining_percent: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UsageProfileHealth {
    Healthy(UsageProfileScore),
    Exhausted { retry_at: Option<i64> },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageProfileSelection {
    pub selected_profile: Option<String>,
    pub retry_at: Option<i64>,
    pub reason: UsageProfileSelectionReason,
}

impl UsageProfileSelection {
    fn selected(profile: String, reason: UsageProfileSelectionReason) -> Self {
        Self {
            selected_profile: Some(profile),
            retry_at: None,
            reason,
        }
    }

    pub fn no_candidates() -> Self {
        Self {
            selected_profile: None,
            retry_at: None,
            reason: UsageProfileSelectionReason::NoCandidateProfiles,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageProfileSelectionReason {
    NoCandidateProfiles,
    SelectedHealthyProfile,
    SelectedUnknownProfile,
    NoAvailableProfiles,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsageProfileCooldownKey {
    pub profile: Option<String>,
    pub limit_id: String,
    pub window_label: String,
    pub resets_at: Option<i64>,
}

impl UsageProfileCooldownKey {
    pub fn new(
        profile: Option<String>,
        limit_id: impl Into<String>,
        window: UsageProfileAutoSwitchWindow,
    ) -> Self {
        Self {
            profile,
            limit_id: limit_id.into(),
            window_label: window.label.to_string(),
            resets_at: window.resets_at,
        }
    }
}

pub fn usage_limit_matches_auto_switch_config(
    config: &AuthProfileAutoSwitchConfig,
    snapshot: Option<&UsageProfileRateLimitSnapshot<'_>>,
) -> bool {
    if !config.enabled {
        return false;
    }

    let Some(snapshot) = snapshot else {
        return true;
    };
    exhausted_auto_switch_window(snapshot, config).is_some()
}

pub fn exhausted_auto_switch_window(
    snapshot: &UsageProfileRateLimitSnapshot<'_>,
    config: &AuthProfileAutoSwitchConfig,
) -> Option<UsageProfileAutoSwitchWindow> {
    if !config.enabled || !is_codex_limit(snapshot) {
        return None;
    }

    [snapshot.secondary, snapshot.primary]
        .into_iter()
        .flatten()
        .filter_map(exhausted_auto_switch_window_for_limit)
        .find(|window| auth_profile_auto_switch_label_enabled(window.label, config))
}

pub fn exhausted_auto_switch_window_for_snapshot(
    snapshot: &UsageProfileRateLimitSnapshot<'_>,
) -> Option<UsageProfileAutoSwitchWindow> {
    if !is_codex_limit(snapshot) {
        return None;
    }

    [snapshot.secondary, snapshot.primary]
        .into_iter()
        .flatten()
        .find_map(exhausted_auto_switch_window_for_limit)
}

/// Earliest future reset timestamp (unix seconds) among exhausted (`used_percent >= 100`)
/// codex windows in `snapshot`.
///
/// Returns `None` when the limit is not codex, nothing is exhausted, or every exhausted
/// window's reset is missing or already in the past. Callers use this to suppress usage
/// heartbeats for a profile that is already capped until it resets, instead of re-polling
/// the usage endpoint every heartbeat interval (which just hammers a limit we already know
/// about). When the reset is unknown, `None` lets the caller fall back to the normal
/// interval/backoff so heartbeats are never permanently blocked.
pub fn earliest_exhausted_reset_at(
    snapshot: &UsageProfileRateLimitSnapshot<'_>,
    now_unix_secs: i64,
) -> Option<i64> {
    if !is_codex_limit(snapshot) {
        return None;
    }

    [snapshot.secondary, snapshot.primary]
        .into_iter()
        .flatten()
        .filter(|window| window.used_percent >= 100.0)
        .filter_map(|window| window.resets_at)
        .filter(|reset_at| *reset_at > now_unix_secs)
        .min()
}

pub fn usage_health_for_snapshots(
    snapshots: &[UsageProfileRateLimitSnapshot<'_>],
    config: &AuthProfileAutoSwitchConfig,
    trigger_window_label: Option<&str>,
    is_fresh: bool,
) -> UsageProfileHealth {
    if !is_fresh {
        return UsageProfileHealth::Unknown;
    }

    let Some(snapshot) = snapshots.iter().find(|snapshot| is_codex_limit(snapshot)) else {
        return UsageProfileHealth::Unknown;
    };

    let mut trigger_remaining_percent = None;
    let mut limiting_remaining_percent = 100.0;
    let mut retry_at = None;
    let mut has_enabled_window = false;
    for (window, is_secondary) in [
        snapshot.secondary.map(|window| (window, true)),
        snapshot.primary.map(|window| (window, false)),
    ]
    .into_iter()
    .flatten()
    {
        let label = limit_label_for_window(window.window_minutes, is_secondary);
        if !auth_profile_auto_switch_label_enabled(label, config) {
            continue;
        }

        has_enabled_window = true;
        let remaining_percent = (100.0 - window.used_percent).clamp(0.0, 100.0);
        limiting_remaining_percent = f64::min(limiting_remaining_percent, remaining_percent);
        if trigger_window_label.is_none_or(|trigger_label| trigger_label == label) {
            trigger_remaining_percent = Some(
                trigger_remaining_percent.map_or(remaining_percent, |current| {
                    f64::min(current, remaining_percent)
                }),
            );
        }
        if window.used_percent >= 100.0 {
            merge_retry_at(&mut retry_at, window.resets_at);
        }
    }

    if !has_enabled_window {
        return UsageProfileHealth::Unknown;
    }
    let Some(trigger_remaining_percent) = trigger_remaining_percent else {
        return UsageProfileHealth::Unknown;
    };
    if limiting_remaining_percent <= 0.0 || retry_at.is_some() {
        return UsageProfileHealth::Exhausted { retry_at };
    }

    UsageProfileHealth::Healthy(UsageProfileScore {
        trigger_remaining_percent,
        limiting_remaining_percent,
    })
}

pub fn choose_profile_for_auto_switch(
    config: &AuthProfileAutoSwitchConfig,
    candidates: &[String],
    health_by_profile: &BTreeMap<String, UsageProfileHealth>,
) -> UsageProfileSelection {
    if candidates.is_empty() {
        return UsageProfileSelection::no_candidates();
    }

    let mut retry_at = None;
    match config.strategy {
        AuthProfileAutoSwitchStrategy::Ordered => {
            // Respect the configured order, but never optimistically switch to an
            // Unknown profile when a later candidate is known to be healthy: a
            // known-healthy profile is always a safer switch target than one whose
            // usage we have not confirmed.
            let mut first_healthy = None;
            let mut first_unknown = None;
            for candidate in candidates {
                match health_by_profile
                    .get(candidate)
                    .copied()
                    .unwrap_or(UsageProfileHealth::Unknown)
                {
                    UsageProfileHealth::Healthy(_) => {
                        if first_healthy.is_none() {
                            first_healthy = Some(candidate.as_str());
                        }
                    }
                    UsageProfileHealth::Unknown => {
                        if first_unknown.is_none() {
                            first_unknown = Some(candidate.as_str());
                        }
                    }
                    UsageProfileHealth::Exhausted {
                        retry_at: profile_retry_at,
                    } => merge_retry_at(&mut retry_at, profile_retry_at),
                }
            }
            if let Some(profile) = first_healthy {
                return UsageProfileSelection::selected(
                    profile.to_string(),
                    UsageProfileSelectionReason::SelectedHealthyProfile,
                );
            }
            if let Some(profile) = first_unknown {
                return UsageProfileSelection::selected(
                    profile.to_string(),
                    UsageProfileSelectionReason::SelectedUnknownProfile,
                );
            }
        }
        AuthProfileAutoSwitchStrategy::HighestAvailable => {
            let mut best: Option<(&str, UsageProfileScore)> = None;
            let mut first_unknown = None;
            for candidate in candidates {
                match health_by_profile
                    .get(candidate)
                    .copied()
                    .unwrap_or(UsageProfileHealth::Unknown)
                {
                    UsageProfileHealth::Healthy(score) => {
                        if best.as_ref().is_none_or(|(_, best_score)| {
                            usage_profile_score_is_better(score, *best_score)
                        }) {
                            best = Some((candidate.as_str(), score));
                        }
                    }
                    UsageProfileHealth::Unknown => {
                        if first_unknown.is_none() {
                            first_unknown = Some(candidate.as_str());
                        }
                    }
                    UsageProfileHealth::Exhausted {
                        retry_at: profile_retry_at,
                    } => merge_retry_at(&mut retry_at, profile_retry_at),
                }
            }
            if let Some((profile, _score)) = best {
                return UsageProfileSelection::selected(
                    profile.to_string(),
                    UsageProfileSelectionReason::SelectedHealthyProfile,
                );
            }
            if let Some(profile) = first_unknown {
                return UsageProfileSelection::selected(
                    profile.to_string(),
                    UsageProfileSelectionReason::SelectedUnknownProfile,
                );
            }
        }
    }

    UsageProfileSelection {
        selected_profile: None,
        retry_at,
        reason: UsageProfileSelectionReason::NoAvailableProfiles,
    }
}

pub fn limit_label_for_window(window_minutes: Option<i64>, is_secondary: bool) -> &'static str {
    window_minutes
        .and_then(get_limits_duration)
        .unwrap_or_else(|| fallback_limit_label(is_secondary))
}

pub fn get_limits_duration(windows_minutes: i64) -> Option<&'static str> {
    let windows_minutes = windows_minutes.max(0);

    if is_approximate_window(windows_minutes, MINUTES_PER_5_HOURS) {
        Some(FIVE_HOUR_LIMIT_LABEL)
    } else if is_approximate_window(windows_minutes, MINUTES_PER_DAY) {
        Some(DAILY_LIMIT_LABEL)
    } else if is_approximate_window(windows_minutes, MINUTES_PER_WEEK) {
        Some(WEEKLY_LIMIT_LABEL)
    } else if is_approximate_window(windows_minutes, MINUTES_PER_MONTH) {
        Some(MONTHLY_LIMIT_LABEL)
    } else if is_approximate_window(windows_minutes, MINUTES_PER_YEAR) {
        Some(ANNUAL_LIMIT_LABEL)
    } else {
        None
    }
}

pub fn fallback_limit_label(is_secondary: bool) -> &'static str {
    if is_secondary {
        SECONDARY_LIMIT_FALLBACK_LABEL
    } else {
        PRIMARY_LIMIT_FALLBACK_LABEL
    }
}

pub fn cooldown_duration_for_reset(
    resets_at: Option<i64>,
    now_unix_secs: i64,
    reset_retry_buffer_secs: u64,
    fallback: Duration,
) -> Duration {
    let Some(resets_at) = resets_at else {
        return fallback;
    };
    let Some(wait_secs) = resets_at.checked_sub(now_unix_secs) else {
        return fallback;
    };
    if wait_secs <= 0 {
        return fallback;
    }

    let wait_secs = u64::try_from(wait_secs).unwrap_or(u64::MAX);
    let wait_secs = wait_secs.saturating_add(reset_retry_buffer_secs);
    Duration::from_secs(wait_secs)
}

pub fn merge_retry_at(current: &mut Option<i64>, candidate: Option<i64>) {
    let Some(candidate) = candidate else {
        return;
    };
    if current.is_none_or(|current| candidate < current) {
        *current = Some(candidate);
    }
}

pub fn auth_profile_auto_switch_label_enabled(
    label: &str,
    config: &AuthProfileAutoSwitchConfig,
) -> bool {
    match label {
        FIVE_HOUR_LIMIT_LABEL => config.on_5h_limit,
        WEEKLY_LIMIT_LABEL => config.on_weekly_limit,
        _ => false,
    }
}

fn exhausted_auto_switch_window_for_limit(
    window: UsageProfileRateLimitWindow,
) -> Option<UsageProfileAutoSwitchWindow> {
    if window.used_percent < 100.0 {
        return None;
    }

    let label = get_limits_duration(window.window_minutes?)?;
    matches!(label, FIVE_HOUR_LIMIT_LABEL | WEEKLY_LIMIT_LABEL).then_some(
        UsageProfileAutoSwitchWindow {
            label,
            resets_at: window.resets_at,
        },
    )
}

fn is_codex_limit(snapshot: &UsageProfileRateLimitSnapshot<'_>) -> bool {
    if let Some(limit_id) = snapshot.limit_id {
        return limit_id.eq_ignore_ascii_case("codex");
    }

    snapshot
        .limit_name
        .is_none_or(|limit_name| limit_name.eq_ignore_ascii_case("codex"))
}

fn usage_profile_score_is_better(
    candidate: UsageProfileScore,
    current_best: UsageProfileScore,
) -> bool {
    candidate.trigger_remaining_percent > current_best.trigger_remaining_percent
        || (candidate.trigger_remaining_percent == current_best.trigger_remaining_percent
            && candidate.limiting_remaining_percent > current_best.limiting_remaining_percent)
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes as f64;
    let expected_minutes = expected_minutes as f64;
    minutes >= expected_minutes * 0.95 && minutes <= expected_minutes * 1.05
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AuthProfileAutoSwitchConfig {
        AuthProfileAutoSwitchConfig {
            enabled: true,
            ..Default::default()
        }
    }

    fn window(
        used_percent: f64,
        window_minutes: i64,
        resets_at: Option<i64>,
    ) -> UsageProfileRateLimitWindow {
        UsageProfileRateLimitWindow {
            used_percent,
            window_minutes: Some(window_minutes),
            resets_at,
        }
    }

    fn snapshot(
        primary: Option<UsageProfileRateLimitWindow>,
        secondary: Option<UsageProfileRateLimitWindow>,
    ) -> UsageProfileRateLimitSnapshot<'static> {
        UsageProfileRateLimitSnapshot {
            limit_id: Some("codex"),
            limit_name: None,
            primary,
            secondary,
        }
    }

    #[test]
    fn usage_health_scores_healthy_snapshot() {
        assert_eq!(
            UsageProfileHealth::Healthy(UsageProfileScore {
                trigger_remaining_percent: 65.0,
                limiting_remaining_percent: 65.0,
            }),
            usage_health_for_snapshots(
                &[snapshot(
                    Some(window(35.0, MINUTES_PER_5_HOURS, Some(100))),
                    None,
                )],
                &config(),
                Some(FIVE_HOUR_LIMIT_LABEL),
                true,
            )
        );
    }

    #[test]
    fn usage_health_marks_exhausted_snapshot() {
        assert_eq!(
            UsageProfileHealth::Exhausted {
                retry_at: Some(500),
            },
            usage_health_for_snapshots(
                &[snapshot(
                    Some(window(100.0, MINUTES_PER_5_HOURS, Some(500))),
                    None,
                )],
                &config(),
                Some(FIVE_HOUR_LIMIT_LABEL),
                true,
            )
        );
    }

    #[test]
    fn usage_health_unknown_for_missing_snapshot() {
        assert_eq!(
            UsageProfileHealth::Unknown,
            usage_health_for_snapshots(&[], &config(), Some(FIVE_HOUR_LIMIT_LABEL), true)
        );
    }

    #[test]
    fn usage_health_uses_five_hour_only_when_weekly_disabled() {
        let mut config = config();
        config.on_weekly_limit = false;

        assert_eq!(
            UsageProfileHealth::Healthy(UsageProfileScore {
                trigger_remaining_percent: 75.0,
                limiting_remaining_percent: 75.0,
            }),
            usage_health_for_snapshots(
                &[snapshot(
                    Some(window(100.0, MINUTES_PER_WEEK, Some(900))),
                    Some(window(25.0, MINUTES_PER_5_HOURS, Some(100))),
                )],
                &config,
                Some(FIVE_HOUR_LIMIT_LABEL),
                true,
            )
        );
    }

    #[test]
    fn usage_health_uses_weekly_only_when_five_hour_disabled() {
        let mut config = config();
        config.on_5h_limit = false;

        assert_eq!(
            UsageProfileHealth::Healthy(UsageProfileScore {
                trigger_remaining_percent: 55.0,
                limiting_remaining_percent: 55.0,
            }),
            usage_health_for_snapshots(
                &[snapshot(
                    Some(window(45.0, MINUTES_PER_WEEK, Some(900))),
                    Some(window(100.0, MINUTES_PER_5_HOURS, Some(100))),
                )],
                &config,
                Some(WEEKLY_LIMIT_LABEL),
                true,
            )
        );
    }

    #[test]
    fn usage_health_scores_both_windows_by_most_limiting_window() {
        assert_eq!(
            UsageProfileHealth::Healthy(UsageProfileScore {
                trigger_remaining_percent: 80.0,
                limiting_remaining_percent: 35.0,
            }),
            usage_health_for_snapshots(
                &[snapshot(
                    Some(window(65.0, MINUTES_PER_WEEK, Some(900))),
                    Some(window(20.0, MINUTES_PER_5_HOURS, Some(100))),
                )],
                &config(),
                Some(FIVE_HOUR_LIMIT_LABEL),
                true,
            )
        );
    }

    #[test]
    fn usage_health_treats_stale_snapshot_as_unknown() {
        assert_eq!(
            UsageProfileHealth::Unknown,
            usage_health_for_snapshots(
                &[snapshot(
                    Some(window(35.0, MINUTES_PER_5_HOURS, Some(100))),
                    None,
                )],
                &config(),
                Some(FIVE_HOUR_LIMIT_LABEL),
                false,
            )
        );
    }

    #[test]
    fn usage_health_ignores_unknown_non_codex_limit_ids() {
        let snapshot = UsageProfileRateLimitSnapshot {
            limit_id: Some("not-codex"),
            limit_name: None,
            primary: Some(window(100.0, MINUTES_PER_5_HOURS, Some(500))),
            secondary: None,
        };

        assert_eq!(
            UsageProfileHealth::Unknown,
            usage_health_for_snapshots(&[snapshot], &config(), Some(FIVE_HOUR_LIMIT_LABEL), true,)
        );
    }

    #[test]
    fn usage_health_trusts_codex_limit_id_with_display_name() {
        let snapshot = UsageProfileRateLimitSnapshot {
            limit_id: Some("codex"),
            limit_name: Some("gpt-5.4-codex"),
            primary: Some(window(35.0, MINUTES_PER_5_HOURS, Some(100))),
            secondary: None,
        };

        assert_eq!(
            UsageProfileHealth::Healthy(UsageProfileScore {
                trigger_remaining_percent: 65.0,
                limiting_remaining_percent: 65.0,
            }),
            usage_health_for_snapshots(&[snapshot], &config(), Some(FIVE_HOUR_LIMIT_LABEL), true,)
        );
    }

    #[test]
    fn usage_health_treats_non_codex_limit_id_as_authoritative() {
        let snapshot = UsageProfileRateLimitSnapshot {
            limit_id: Some("codex_model"),
            limit_name: Some("codex"),
            primary: Some(window(35.0, MINUTES_PER_5_HOURS, Some(100))),
            secondary: None,
        };

        assert_eq!(
            UsageProfileHealth::Unknown,
            usage_health_for_snapshots(&[snapshot], &config(), Some(FIVE_HOUR_LIMIT_LABEL), true,)
        );
    }

    #[test]
    fn highest_available_selects_healthy_before_unknown() {
        let health_by_profile = BTreeMap::from([
            ("unknown".to_string(), UsageProfileHealth::Unknown),
            (
                "healthy".to_string(),
                UsageProfileHealth::Healthy(UsageProfileScore {
                    trigger_remaining_percent: 30.0,
                    limiting_remaining_percent: 30.0,
                }),
            ),
        ]);

        assert_eq!(
            UsageProfileSelection {
                selected_profile: Some("healthy".to_string()),
                retry_at: None,
                reason: UsageProfileSelectionReason::SelectedHealthyProfile,
            },
            choose_profile_for_auto_switch(
                &config(),
                &["unknown".to_string(), "healthy".to_string()],
                &health_by_profile,
            )
        );
    }

    fn ordered_config() -> AuthProfileAutoSwitchConfig {
        AuthProfileAutoSwitchConfig {
            enabled: true,
            strategy: AuthProfileAutoSwitchStrategy::Ordered,
            ..Default::default()
        }
    }

    #[test]
    fn ordered_prefers_healthy_over_earlier_unknown() {
        let health_by_profile = BTreeMap::from([
            ("first".to_string(), UsageProfileHealth::Unknown),
            (
                "second".to_string(),
                UsageProfileHealth::Healthy(UsageProfileScore {
                    trigger_remaining_percent: 40.0,
                    limiting_remaining_percent: 40.0,
                }),
            ),
        ]);

        assert_eq!(
            UsageProfileSelection {
                selected_profile: Some("second".to_string()),
                retry_at: None,
                reason: UsageProfileSelectionReason::SelectedHealthyProfile,
            },
            choose_profile_for_auto_switch(
                &ordered_config(),
                &["first".to_string(), "second".to_string()],
                &health_by_profile,
            )
        );
    }

    #[test]
    fn ordered_keeps_first_healthy_in_configured_order() {
        let health_by_profile = BTreeMap::from([
            (
                "first".to_string(),
                UsageProfileHealth::Healthy(UsageProfileScore {
                    trigger_remaining_percent: 20.0,
                    limiting_remaining_percent: 20.0,
                }),
            ),
            (
                "second".to_string(),
                UsageProfileHealth::Healthy(UsageProfileScore {
                    trigger_remaining_percent: 90.0,
                    limiting_remaining_percent: 90.0,
                }),
            ),
        ]);

        assert_eq!(
            UsageProfileSelection {
                selected_profile: Some("first".to_string()),
                retry_at: None,
                reason: UsageProfileSelectionReason::SelectedHealthyProfile,
            },
            choose_profile_for_auto_switch(
                &ordered_config(),
                &["first".to_string(), "second".to_string()],
                &health_by_profile,
            )
        );
    }

    #[test]
    fn ordered_falls_back_to_first_unknown_when_no_healthy() {
        let health_by_profile = BTreeMap::from([(
            "first".to_string(),
            UsageProfileHealth::Exhausted {
                retry_at: Some(500),
            },
        )]);

        assert_eq!(
            UsageProfileSelection {
                selected_profile: Some("second".to_string()),
                retry_at: None,
                reason: UsageProfileSelectionReason::SelectedUnknownProfile,
            },
            choose_profile_for_auto_switch(
                &ordered_config(),
                &["first".to_string(), "second".to_string()],
                &health_by_profile,
            )
        );
    }

    #[test]
    fn earliest_exhausted_reset_at_returns_future_reset() {
        assert_eq!(
            Some(1_000),
            earliest_exhausted_reset_at(
                &snapshot(
                    Some(window(100.0, MINUTES_PER_5_HOURS, Some(1_000))),
                    Some(window(100.0, MINUTES_PER_WEEK, Some(2_000))),
                ),
                500,
            )
        );
    }

    #[test]
    fn earliest_exhausted_reset_at_ignores_unexhausted_and_past_resets() {
        // Not exhausted -> None.
        assert_eq!(
            None,
            earliest_exhausted_reset_at(
                &snapshot(Some(window(80.0, MINUTES_PER_5_HOURS, Some(1_000))), None),
                500,
            )
        );
        // Exhausted but reset already elapsed -> None (fall back to normal interval).
        assert_eq!(
            None,
            earliest_exhausted_reset_at(
                &snapshot(Some(window(100.0, MINUTES_PER_5_HOURS, Some(400))), None),
                500,
            )
        );
        // Exhausted but reset unknown -> None.
        assert_eq!(
            None,
            earliest_exhausted_reset_at(
                &snapshot(Some(window(100.0, MINUTES_PER_5_HOURS, None)), None),
                500,
            )
        );
    }

    #[test]
    fn earliest_exhausted_reset_at_ignores_non_codex_limits() {
        let snapshot = UsageProfileRateLimitSnapshot {
            limit_id: Some("not-codex"),
            limit_name: None,
            primary: Some(window(100.0, MINUTES_PER_5_HOURS, Some(1_000))),
            secondary: None,
        };

        assert_eq!(None, earliest_exhausted_reset_at(&snapshot, 500));
    }

    /// Health derived from a single primary 5h window at `used_percent`, resetting at `resets_at`.
    fn health_from_5h(used_percent: f64, resets_at: Option<i64>) -> UsageProfileHealth {
        usage_health_for_snapshots(
            &[snapshot(
                Some(window(used_percent, MINUTES_PER_5_HOURS, resets_at)),
                None,
            )],
            &config(),
            Some(FIVE_HOUR_LIMIT_LABEL),
            /*is_fresh*/ true,
        )
    }

    #[test]
    fn highest_available_picks_best_remaining_and_skips_exhausted() {
        // `dead` is exhausted (must be skipped), `low` has 20% headroom, `high` has 70%.
        // HighestAvailable must select `high` even though `low`/`dead` come first in order.
        let health_by_profile = BTreeMap::from([
            ("dead".to_string(), health_from_5h(100.0, Some(900))),
            ("low".to_string(), health_from_5h(80.0, Some(100))),
            ("high".to_string(), health_from_5h(30.0, Some(200))),
        ]);

        let selection = choose_profile_for_auto_switch(
            &config(),
            &["dead".to_string(), "low".to_string(), "high".to_string()],
            &health_by_profile,
        );

        assert_eq!(selection.selected_profile.as_deref(), Some("high"));
        assert_eq!(
            selection.reason,
            UsageProfileSelectionReason::SelectedHealthyProfile
        );
    }

    #[test]
    fn all_exhausted_snapshots_report_earliest_reset_gracefully() {
        // Every candidate is exhausted: no profile is selectable, and the soonest reset
        // (900, earlier than 2_000) is surfaced so the caller can tell the user when a
        // profile becomes usable again.
        let health_by_profile = BTreeMap::from([
            ("later".to_string(), health_from_5h(100.0, Some(2_000))),
            ("soonest".to_string(), health_from_5h(100.0, Some(900))),
        ]);

        let selection = choose_profile_for_auto_switch(
            &config(),
            &["later".to_string(), "soonest".to_string()],
            &health_by_profile,
        );

        assert_eq!(
            selection,
            UsageProfileSelection {
                selected_profile: None,
                retry_at: Some(900),
                reason: UsageProfileSelectionReason::NoAvailableProfiles,
            }
        );
    }

    #[test]
    fn exhausted_candidates_merge_earliest_retry_timestamp() {
        let health_by_profile = BTreeMap::from([
            (
                "first".to_string(),
                UsageProfileHealth::Exhausted {
                    retry_at: Some(700),
                },
            ),
            (
                "second".to_string(),
                UsageProfileHealth::Exhausted {
                    retry_at: Some(500),
                },
            ),
        ]);

        assert_eq!(
            UsageProfileSelection {
                selected_profile: None,
                retry_at: Some(500),
                reason: UsageProfileSelectionReason::NoAvailableProfiles,
            },
            choose_profile_for_auto_switch(
                &config(),
                &["first".to_string(), "second".to_string()],
                &health_by_profile,
            )
        );
    }
}
