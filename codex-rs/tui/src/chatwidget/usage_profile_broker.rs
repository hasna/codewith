use crate::legacy_core::config::AuthProfileAutoSwitchConfig;
use crate::legacy_core::config::AuthProfileAutoSwitchStrategy;
use crate::legacy_core::usage_profile_health;
use crate::legacy_core::usage_profile_health::UsageProfileHealth;
use crate::legacy_core::usage_profile_health::UsageProfileRateLimitSnapshot;
use crate::legacy_core::usage_profile_health::UsageProfileRateLimitWindow;
use crate::legacy_core::usage_profile_health::choose_profile_for_auto_switch;
use crate::status::RateLimitSnapshotDisplay;
use chrono::Local;
use codex_app_server_protocol::RateLimitSnapshot;
use codex_app_server_protocol::RateLimitWindow;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::time::Duration;

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

/// Outcome of resolving an auto-switch target for an exhausted window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UsageProfileSwitchOutcome {
    /// A healthier alternate profile is available; switch to it.
    Switch(UsageProfileSwitchTarget),
    /// No alternate profile has available usage right now. `earliest_reset_at` carries the
    /// soonest known reset (unix seconds) across the exhausted candidates, when any is known,
    /// so callers can tell the user when a profile is expected to become usable again.
    NoEligibleProfile { earliest_reset_at: Option<i64> },
}

pub(super) fn exhausted_auto_switch_window(
    snapshot: &RateLimitSnapshot,
    config: &AuthProfileAutoSwitchConfig,
    is_codex_limit: bool,
) -> Option<UsageProfileAutoSwitchWindow> {
    usage_profile_health::exhausted_auto_switch_window(
        &app_server_rate_limit_snapshot(snapshot, is_codex_limit),
        config,
    )
    .map(tui_auto_switch_window)
}

pub(super) fn exhausted_auto_switch_window_for_snapshot(
    snapshot: &RateLimitSnapshot,
    is_codex_limit: bool,
) -> Option<UsageProfileAutoSwitchWindow> {
    usage_profile_health::exhausted_auto_switch_window_for_snapshot(
        &app_server_rate_limit_snapshot(snapshot, is_codex_limit),
    )
    .map(tui_auto_switch_window)
}

pub(super) fn earliest_exhausted_reset_at(
    snapshot: &RateLimitSnapshot,
    is_codex_limit: bool,
    now_unix_secs: i64,
) -> Option<i64> {
    usage_profile_health::earliest_exhausted_reset_at(
        &app_server_rate_limit_snapshot(snapshot, is_codex_limit),
        now_unix_secs,
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
    recently_failed_profiles: &HashSet<String>,
    limit_id: &str,
    window: &UsageProfileAutoSwitchWindow,
) -> UsageProfileSwitchOutcome {
    let ordered = ordered_auth_profiles_for_auto_switch(&config.profiles, saved_profiles);
    let candidates = auth_profile_auto_switch_candidates(selected_auth_profile, &ordered);
    let selection = match config.strategy {
        AuthProfileAutoSwitchStrategy::HighestAvailable
        | AuthProfileAutoSwitchStrategy::Ordered => healthiest_auth_profile_for_auto_switch(
            config,
            cached_snapshots_by_profile,
            recently_failed_profiles,
            &candidates,
            window,
        ),
    };
    match selection.selected_profile {
        Some(profile) => UsageProfileSwitchOutcome::Switch(UsageProfileSwitchTarget {
            profile,
            trigger_key: auto_switch_trigger_key(limit_id, window),
        }),
        None => UsageProfileSwitchOutcome::NoEligibleProfile {
            earliest_reset_at: selection.retry_at,
        },
    }
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
    recently_failed_profiles: &HashSet<String>,
    candidates: &[String],
    window: &UsageProfileAutoSwitchWindow,
) -> usage_profile_health::UsageProfileSelection {
    let freshness = Duration::from_secs(config.heartbeat_freshness_secs);
    let mut health_by_profile = BTreeMap::new();

    for profile in candidates {
        let profile_key = Some(profile.clone());
        let snapshots = cached_snapshots_by_profile.get(&profile_key);
        let mut health =
            auth_profile_usage_health_for_auto_switch(snapshots, window, config, freshness);
        // Do not optimistically switch onto a profile whose most recent usage heartbeat
        // failed: we could not confirm it is available, so treat an otherwise-Unknown
        // profile as unavailable rather than a switch target. The failure backoff clears
        // this after a short window, so the profile is reconsidered once it recovers.
        if recently_failed_profiles.contains(profile)
            && matches!(health, UsageProfileHealth::Unknown)
        {
            health = UsageProfileHealth::Exhausted { retry_at: None };
        }
        health_by_profile.insert(profile.clone(), health);
    }

    choose_profile_for_auto_switch(config, candidates, &health_by_profile)
}

fn auth_profile_usage_health_for_auto_switch(
    snapshots: Option<&BTreeMap<String, RateLimitSnapshotDisplay>>,
    trigger_window: &UsageProfileAutoSwitchWindow,
    config: &AuthProfileAutoSwitchConfig,
    freshness: Duration,
) -> UsageProfileHealth {
    let freshness =
        chrono::Duration::seconds(i64::try_from(freshness.as_secs()).unwrap_or(i64::MAX));
    let shared_snapshots = snapshots
        .map(|snapshots| {
            snapshots
                .iter()
                .map(|(limit_id, snapshot)| {
                    let is_fresh =
                        Local::now().signed_duration_since(snapshot.captured_at) <= freshness;
                    (display_rate_limit_snapshot(limit_id, snapshot), is_fresh)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let is_fresh = shared_snapshots
        .iter()
        .find(|(snapshot, _is_fresh)| {
            snapshot
                .limit_id
                .is_none_or(|limit_id| limit_id.eq_ignore_ascii_case("codex"))
        })
        .map(|(_, is_fresh)| *is_fresh)
        .unwrap_or(true);
    let snapshots = shared_snapshots
        .iter()
        .map(|(snapshot, _is_fresh)| *snapshot)
        .collect::<Vec<_>>();

    usage_profile_health::usage_health_for_snapshots(
        &snapshots,
        config,
        Some(trigger_window.label.as_str()),
        is_fresh,
    )
}

fn dedupe_profile_names(profiles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .filter(|profile| seen.insert(profile.clone()))
        .collect()
}

pub(crate) fn limit_label_for_window(window_minutes: Option<i64>, is_secondary: bool) -> String {
    usage_profile_health::limit_label_for_window(window_minutes, is_secondary).to_string()
}

pub(crate) fn get_limits_duration(windows_minutes: i64) -> Option<String> {
    usage_profile_health::get_limits_duration(windows_minutes).map(str::to_string)
}

pub(crate) fn fallback_limit_label(is_secondary: bool) -> &'static str {
    usage_profile_health::fallback_limit_label(is_secondary)
}

fn app_server_rate_limit_snapshot(
    snapshot: &RateLimitSnapshot,
    is_codex_limit: bool,
) -> UsageProfileRateLimitSnapshot<'_> {
    UsageProfileRateLimitSnapshot {
        limit_id: if is_codex_limit {
            Some("codex")
        } else {
            Some("__non_codex__")
        },
        limit_name: if is_codex_limit {
            snapshot.limit_name.as_deref()
        } else {
            Some("__non_codex__")
        },
        primary: snapshot.primary.as_ref().map(app_server_rate_limit_window),
        secondary: snapshot
            .secondary
            .as_ref()
            .map(app_server_rate_limit_window),
    }
}

fn app_server_rate_limit_window(window: &RateLimitWindow) -> UsageProfileRateLimitWindow {
    UsageProfileRateLimitWindow {
        used_percent: f64::from(window.used_percent),
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn display_rate_limit_snapshot<'a>(
    limit_id: &'a str,
    snapshot: &'a RateLimitSnapshotDisplay,
) -> UsageProfileRateLimitSnapshot<'a> {
    UsageProfileRateLimitSnapshot {
        limit_id: Some(limit_id),
        limit_name: Some(snapshot.limit_name.as_str()),
        primary: snapshot.primary.as_ref().map(display_rate_limit_window),
        secondary: snapshot.secondary.as_ref().map(display_rate_limit_window),
    }
}

fn display_rate_limit_window(
    window: &crate::status::RateLimitWindowDisplay,
) -> UsageProfileRateLimitWindow {
    UsageProfileRateLimitWindow {
        used_percent: window.used_percent,
        window_minutes: window.window_minutes,
        // Preserve the server-reported reset time so an exhausted cached profile reports its
        // `retry_at` (used for the earliest-reset message and to avoid re-picking a profile
        // until it resets) instead of an anonymous exhausted state.
        resets_at: window.resets_at_unix,
    }
}

fn tui_auto_switch_window(
    window: usage_profile_health::UsageProfileAutoSwitchWindow,
) -> UsageProfileAutoSwitchWindow {
    UsageProfileAutoSwitchWindow {
        label: window.label.to_string(),
        resets_at: window.resets_at,
    }
}
