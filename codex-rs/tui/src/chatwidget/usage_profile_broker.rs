use crate::legacy_core::config::AuthProfileAutoSwitchConfig;
use crate::legacy_core::config::AuthProfileAutoSwitchStrategy;
use crate::status::RateLimitSnapshotDisplay;
use chrono::Local;
use codex_app_server_protocol::RateLimitSnapshot;
use codex_app_server_protocol::RateLimitWindow;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::time::Duration;

const PRIMARY_LIMIT_FALLBACK_LABEL: &str = "usage";
const SECONDARY_LIMIT_FALLBACK_LABEL: &str = "secondary usage";
const FIVE_HOUR_LIMIT_LABEL: &str = "5h";
const WEEKLY_LIMIT_LABEL: &str = "weekly";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UsageProfileAutoSwitchWindow {
    pub(super) label: String,
    pub(super) resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UsageProfileSwitchTarget {
    pub(super) profile: String,
    pub(super) trigger_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct UsageProfileScore {
    trigger_remaining_percent: f64,
    limiting_remaining_percent: f64,
}

enum UsageProfileHealth {
    Healthy(UsageProfileScore),
    Exhausted,
    Unknown,
}

pub(super) fn exhausted_auto_switch_window(
    snapshot: &RateLimitSnapshot,
    config: &AuthProfileAutoSwitchConfig,
    is_codex_limit: bool,
) -> Option<UsageProfileAutoSwitchWindow> {
    if !config.enabled || !is_codex_limit {
        return None;
    }

    [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(exhausted_auto_switch_window_for_limit)
        .find(|window| auth_profile_auto_switch_window_enabled(window, config))
}

pub(super) fn exhausted_auto_switch_window_for_snapshot(
    snapshot: &RateLimitSnapshot,
    is_codex_limit: bool,
) -> Option<UsageProfileAutoSwitchWindow> {
    if !is_codex_limit {
        return None;
    }

    [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
        .find_map(exhausted_auto_switch_window_for_limit)
}

fn exhausted_auto_switch_window_for_limit(
    window: &RateLimitWindow,
) -> Option<UsageProfileAutoSwitchWindow> {
    if window.used_percent < 100 {
        return None;
    }
    let label = get_limits_duration(window.window_duration_mins?)?;
    matches!(label.as_str(), FIVE_HOUR_LIMIT_LABEL | WEEKLY_LIMIT_LABEL).then_some(
        UsageProfileAutoSwitchWindow {
            label,
            resets_at: window.resets_at,
        },
    )
}

pub(super) fn auto_switch_trigger_key(
    limit_id: &str,
    window: &UsageProfileAutoSwitchWindow,
) -> String {
    let resets_at = window
        .resets_at
        .map(|reset| reset.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!("{limit_id}:{}:{resets_at}", window.label)
}

pub(super) fn auth_profile_auto_switch_target(
    config: &AuthProfileAutoSwitchConfig,
    selected_auth_profile: Option<&str>,
    saved_profiles: &[AuthProfile],
    cached_snapshots_by_profile: &BTreeMap<
        Option<String>,
        BTreeMap<String, RateLimitSnapshotDisplay>,
    >,
    limit_id: &str,
    window: &UsageProfileAutoSwitchWindow,
) -> Option<UsageProfileSwitchTarget> {
    let ordered = ordered_auth_profiles_for_auto_switch(&config.profiles, saved_profiles);
    let candidates = auth_profile_auto_switch_candidates(selected_auth_profile, &ordered);
    let profile = match config.strategy {
        AuthProfileAutoSwitchStrategy::HighestAvailable => healthiest_auth_profile_for_auto_switch(
            config,
            cached_snapshots_by_profile,
            &candidates,
            window,
        ),
        AuthProfileAutoSwitchStrategy::Ordered => candidates.first().cloned(),
    }?;
    Some(UsageProfileSwitchTarget {
        profile,
        trigger_key: auto_switch_trigger_key(limit_id, window),
    })
}

fn ordered_auth_profiles_for_auto_switch(
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

fn auth_profile_auto_switch_candidates(current: Option<&str>, ordered: &[String]) -> Vec<String> {
    if ordered.is_empty() {
        return Vec::new();
    }

    let start = current
        .and_then(|current| ordered.iter().position(|profile| profile == current))
        .map(|index| index + 1)
        .unwrap_or(0);
    ordered
        .iter()
        .cycle()
        .skip(start)
        .take(ordered.len())
        .filter(|profile| current != Some(profile.as_str()))
        .cloned()
        .collect()
}

fn healthiest_auth_profile_for_auto_switch(
    config: &AuthProfileAutoSwitchConfig,
    cached_snapshots_by_profile: &BTreeMap<
        Option<String>,
        BTreeMap<String, RateLimitSnapshotDisplay>,
    >,
    candidates: &[String],
    window: &UsageProfileAutoSwitchWindow,
) -> Option<String> {
    let freshness = Duration::from_secs(config.heartbeat_freshness_secs);
    let mut best: Option<(String, UsageProfileScore)> = None;
    let mut first_unknown = None;

    for profile in candidates {
        let profile_key = Some(profile.clone());
        let snapshots = cached_snapshots_by_profile.get(&profile_key);
        match auth_profile_usage_health_for_auto_switch(snapshots, window, config, freshness) {
            UsageProfileHealth::Healthy(score) => {
                if best
                    .as_ref()
                    .is_none_or(|(_, best_score)| usage_profile_score_is_better(score, *best_score))
                {
                    best = Some((profile.clone(), score));
                }
            }
            UsageProfileHealth::Exhausted => {}
            UsageProfileHealth::Unknown => {
                if first_unknown.is_none() {
                    first_unknown = Some(profile.clone());
                }
            }
        }
    }

    best.map(|(profile, _score)| profile).or(first_unknown)
}

fn auth_profile_usage_health_for_auto_switch(
    snapshots: Option<&BTreeMap<String, RateLimitSnapshotDisplay>>,
    trigger_window: &UsageProfileAutoSwitchWindow,
    config: &AuthProfileAutoSwitchConfig,
    freshness: Duration,
) -> UsageProfileHealth {
    let Some(snapshot) = snapshots
        .and_then(|snapshots| snapshots.get("codex").or_else(|| snapshots.values().next()))
    else {
        return UsageProfileHealth::Unknown;
    };

    let freshness =
        chrono::Duration::seconds(i64::try_from(freshness.as_secs()).unwrap_or(i64::MAX));
    if Local::now().signed_duration_since(snapshot.captured_at) > freshness {
        return UsageProfileHealth::Unknown;
    }

    let mut trigger_remaining_percent = None;
    let mut limiting_remaining_percent = 100.0;
    let mut has_enabled_window = false;
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
        limiting_remaining_percent = f64::min(limiting_remaining_percent, remaining_percent);
        if label == trigger_window.label {
            trigger_remaining_percent = Some(remaining_percent);
        }
    }

    let Some(trigger_remaining_percent) = trigger_remaining_percent else {
        return UsageProfileHealth::Unknown;
    };
    if !has_enabled_window || trigger_remaining_percent <= 0.0 || limiting_remaining_percent <= 0.0
    {
        return UsageProfileHealth::Exhausted;
    }

    UsageProfileHealth::Healthy(UsageProfileScore {
        trigger_remaining_percent,
        limiting_remaining_percent,
    })
}

fn auth_profile_auto_switch_window_enabled(
    window: &UsageProfileAutoSwitchWindow,
    config: &AuthProfileAutoSwitchConfig,
) -> bool {
    auth_profile_auto_switch_label_enabled(window.label.as_str(), config)
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

fn usage_profile_score_is_better(
    candidate: UsageProfileScore,
    current_best: UsageProfileScore,
) -> bool {
    candidate.trigger_remaining_percent > current_best.trigger_remaining_percent
        || (candidate.trigger_remaining_percent == current_best.trigger_remaining_percent
            && candidate.limiting_remaining_percent > current_best.limiting_remaining_percent)
}

fn dedupe_profile_names(profiles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .filter(|profile| seen.insert(profile.clone()))
        .collect()
}

pub(crate) fn limit_label_for_window(window_minutes: Option<i64>, is_secondary: bool) -> String {
    window_minutes
        .and_then(get_limits_duration)
        .unwrap_or_else(|| fallback_limit_label(is_secondary).to_string())
}

pub(crate) fn get_limits_duration(windows_minutes: i64) -> Option<String> {
    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_5_HOURS: i64 = 5 * MINUTES_PER_HOUR;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
    const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;
    const MINUTES_PER_YEAR: i64 = 365 * MINUTES_PER_DAY;

    let windows_minutes = windows_minutes.max(0);

    if is_approximate_window(windows_minutes, MINUTES_PER_5_HOURS) {
        Some("5h".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_DAY) {
        Some("daily".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_WEEK) {
        Some("weekly".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_MONTH) {
        Some("monthly".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_YEAR) {
        Some("annual".to_string())
    } else {
        None
    }
}

pub(crate) fn fallback_limit_label(is_secondary: bool) -> &'static str {
    if is_secondary {
        SECONDARY_LIMIT_FALLBACK_LABEL
    } else {
        PRIMARY_LIMIT_FALLBACK_LABEL
    }
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes as f64;
    let expected_minutes = expected_minutes as f64;
    minutes >= expected_minutes * 0.95 && minutes <= expected_minutes * 1.05
}
