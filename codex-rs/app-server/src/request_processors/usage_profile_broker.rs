use super::*;
use codex_core::config::AuthProfileAutoSwitchConfig;
use codex_core::config::AuthProfileAutoSwitchStrategy;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use codex_protocol::protocol::RateLimitSnapshot;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::sync::Mutex as StdMutex;
use std::time::Instant;
use tokio::time::timeout;

const PROFILE_BROKER_RATE_LIMIT_FETCH_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 10);
const PROFILE_BROKER_PROFILE_LEASE_DURATION: Duration = Duration::from_secs(60);
const PROFILE_BROKER_UNKNOWN_HEALTH_BACKOFF: Duration = Duration::from_secs(5 * 60);
const PRIMARY_LIMIT_FALLBACK_LABEL: &str = "usage";
const SECONDARY_LIMIT_FALLBACK_LABEL: &str = "secondary usage";
const FIVE_HOUR_LIMIT_LABEL: &str = "5h";
const WEEKLY_LIMIT_LABEL: &str = "weekly";

static PROFILE_BROKER_PROFILE_LEASES: LazyLock<StdMutex<BTreeMap<String, Instant>>> =
    LazyLock::new(|| StdMutex::new(BTreeMap::new()));
static PROFILE_BROKER_USAGE_HEALTH_CACHE: LazyLock<
    StdMutex<BTreeMap<ProfileBrokerUsageCacheKey, CachedProfileHealth>>,
> = LazyLock::new(|| StdMutex::new(BTreeMap::new()));

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ProfileBrokerUsageCacheKey {
    codex_home: String,
    chatgpt_base_url: String,
    profile: Option<String>,
    on_5h_limit: bool,
    on_weekly_limit: bool,
    heartbeat_interval_secs: u64,
    heartbeat_freshness_secs: u64,
}

#[derive(Clone, Copy, Debug)]
struct CachedProfileHealth {
    health: UsageProfileHealth,
    expires_at: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct UsageProfileBrokerDecision {
    pub(super) selected_profile: Option<String>,
    pub(super) retry_at: Option<i64>,
    pub(super) reason: UsageProfileBrokerDecisionReason,
}

impl UsageProfileBrokerDecision {
    fn no_switch(reason: UsageProfileBrokerDecisionReason) -> Self {
        Self {
            selected_profile: None,
            retry_at: None,
            reason,
        }
    }

    fn selected(profile: String, reason: UsageProfileBrokerDecisionReason) -> Self {
        Self {
            selected_profile: Some(profile),
            retry_at: None,
            reason,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum UsageProfileBrokerDecisionReason {
    AutoSwitchDisabled,
    CurrentProfileAvailable,
    CurrentProfileUnknown,
    ProfileListUnavailable,
    NoCandidateProfiles,
    SelectedHealthyProfile,
    SelectedUnknownProfile,
    NoAvailableProfiles,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum UsageProfileHealth {
    Healthy { remaining_percent: f64 },
    Exhausted { retry_at: Option<i64> },
    Unknown,
}

pub(super) async fn resolve_dispatch_auth_profile(
    auth_manager: &Arc<AuthManager>,
    config: &Config,
    requested_auth_profile: Option<Option<String>>,
) -> UsageProfileBrokerDecision {
    let auto_switch = &config.auth_profile_auto_switch;
    if !auto_switch.enabled {
        return UsageProfileBrokerDecision::no_switch(
            UsageProfileBrokerDecisionReason::AutoSwitchDisabled,
        );
    }

    let current_profile = effective_current_profile(config, requested_auth_profile.as_ref());
    let current_health = fetch_profile_health(auth_manager, config, current_profile).await;
    match current_health {
        UsageProfileHealth::Healthy { .. } => {
            return UsageProfileBrokerDecision::no_switch(
                UsageProfileBrokerDecisionReason::CurrentProfileAvailable,
            );
        }
        UsageProfileHealth::Unknown => {
            return UsageProfileBrokerDecision::no_switch(
                UsageProfileBrokerDecisionReason::CurrentProfileUnknown,
            );
        }
        UsageProfileHealth::Exhausted { .. } => {}
    }

    let saved_profiles = match codex_login::list_auth_profiles(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
    ) {
        Ok(profiles) => profiles,
        Err(err) => {
            warn!("usage profile broker could not list auth profiles: {err}");
            return UsageProfileBrokerDecision::no_switch(
                UsageProfileBrokerDecisionReason::ProfileListUnavailable,
            );
        }
    };

    let locked_profiles = active_profile_leases(Instant::now());
    let candidates = auth_profile_auto_switch_candidates(
        current_profile,
        auto_switch,
        &saved_profiles,
        &locked_profiles,
    );
    if candidates.is_empty() {
        return UsageProfileBrokerDecision::no_switch(
            UsageProfileBrokerDecisionReason::NoCandidateProfiles,
        );
    }

    let mut health_by_profile = BTreeMap::new();
    for profile in &candidates {
        health_by_profile.insert(
            profile.clone(),
            fetch_profile_health(auth_manager, config, Some(profile.as_str())).await,
        );
    }

    let decision = choose_dispatch_auth_profile(auto_switch, &candidates, &health_by_profile);
    if let Some(profile) = decision.selected_profile.as_ref() {
        lease_profile(profile, Instant::now());
    }
    decision
}

fn effective_current_profile<'a>(
    config: &'a Config,
    requested_auth_profile: Option<&'a Option<String>>,
) -> Option<&'a str> {
    match requested_auth_profile {
        Some(Some(profile)) => Some(profile.as_str()),
        Some(None) => None,
        None => config.selected_auth_profile.as_deref(),
    }
}

async fn fetch_profile_health(
    auth_manager: &Arc<AuthManager>,
    config: &Config,
    profile: Option<&str>,
) -> UsageProfileHealth {
    let profile = profile.map(str::to_string);
    let cache_key = profile_health_cache_key(config, profile.as_deref());
    let now = Instant::now();
    if let Some(health) = cached_profile_health(&cache_key, now) {
        return health;
    }

    let health = fetch_profile_health_uncached(auth_manager, config, profile).await;
    cache_profile_health(cache_key, health, &config.auth_profile_auto_switch, now);
    health
}

async fn fetch_profile_health_uncached(
    auth_manager: &Arc<AuthManager>,
    config: &Config,
    profile: Option<String>,
) -> UsageProfileHealth {
    let scoped_auth_manager = auth_manager.shared_scoped_auth_profile(profile).await;
    let health = async {
        let Some(auth) = scoped_auth_manager.auth().await else {
            return UsageProfileHealth::Unknown;
        };
        if !auth.uses_codex_backend() {
            return UsageProfileHealth::Unknown;
        }
        let Ok(client) = BackendClient::from_auth(config.chatgpt_base_url.clone(), &auth) else {
            return UsageProfileHealth::Unknown;
        };
        match client.get_rate_limits_many().await {
            Ok(snapshots) => {
                usage_health_for_snapshots(&snapshots, &config.auth_profile_auto_switch)
            }
            Err(_err) => {
                tracing::debug!("usage profile broker could not fetch rate limits");
                UsageProfileHealth::Unknown
            }
        }
    };

    match timeout(PROFILE_BROKER_RATE_LIMIT_FETCH_TIMEOUT, health).await {
        Ok(health) => health,
        Err(_) => UsageProfileHealth::Unknown,
    }
}

fn profile_health_cache_key(config: &Config, profile: Option<&str>) -> ProfileBrokerUsageCacheKey {
    ProfileBrokerUsageCacheKey {
        codex_home: config.codex_home.display().to_string(),
        chatgpt_base_url: config.chatgpt_base_url.clone(),
        profile: profile.map(str::to_string),
        on_5h_limit: config.auth_profile_auto_switch.on_5h_limit,
        on_weekly_limit: config.auth_profile_auto_switch.on_weekly_limit,
        heartbeat_interval_secs: config.auth_profile_auto_switch.heartbeat_interval_secs,
        heartbeat_freshness_secs: config.auth_profile_auto_switch.heartbeat_freshness_secs,
    }
}

fn cached_profile_health(
    key: &ProfileBrokerUsageCacheKey,
    now: Instant,
) -> Option<UsageProfileHealth> {
    let Ok(mut cache) = PROFILE_BROKER_USAGE_HEALTH_CACHE.lock() else {
        return None;
    };
    cache.retain(|_, cached| cached.expires_at > now);
    cache.get(key).map(|cached| cached.health)
}

fn cache_profile_health(
    key: ProfileBrokerUsageCacheKey,
    health: UsageProfileHealth,
    config: &AuthProfileAutoSwitchConfig,
    now: Instant,
) {
    let Ok(mut cache) = PROFILE_BROKER_USAGE_HEALTH_CACHE.lock() else {
        return;
    };
    cache.insert(
        key,
        CachedProfileHealth {
            health,
            expires_at: now + profile_health_cache_duration(health, config),
        },
    );
}

fn profile_health_cache_duration(
    health: UsageProfileHealth,
    config: &AuthProfileAutoSwitchConfig,
) -> Duration {
    match health {
        UsageProfileHealth::Healthy { .. } | UsageProfileHealth::Exhausted { .. } => {
            Duration::from_secs(config.heartbeat_freshness_secs)
        }
        UsageProfileHealth::Unknown => PROFILE_BROKER_UNKNOWN_HEALTH_BACKOFF
            .max(Duration::from_secs(config.heartbeat_interval_secs)),
    }
}

fn choose_dispatch_auth_profile(
    config: &AuthProfileAutoSwitchConfig,
    candidates: &[String],
    health_by_profile: &BTreeMap<String, UsageProfileHealth>,
) -> UsageProfileBrokerDecision {
    if candidates.is_empty() {
        return UsageProfileBrokerDecision::no_switch(
            UsageProfileBrokerDecisionReason::NoCandidateProfiles,
        );
    }

    let mut retry_at = None;
    match config.strategy {
        AuthProfileAutoSwitchStrategy::Ordered => {
            for candidate in candidates {
                match health_by_profile
                    .get(candidate)
                    .copied()
                    .unwrap_or(UsageProfileHealth::Unknown)
                {
                    UsageProfileHealth::Healthy { .. } => {
                        return UsageProfileBrokerDecision::selected(
                            candidate.clone(),
                            UsageProfileBrokerDecisionReason::SelectedHealthyProfile,
                        );
                    }
                    UsageProfileHealth::Unknown => {
                        return UsageProfileBrokerDecision::selected(
                            candidate.clone(),
                            UsageProfileBrokerDecisionReason::SelectedUnknownProfile,
                        );
                    }
                    UsageProfileHealth::Exhausted {
                        retry_at: profile_retry_at,
                    } => merge_retry_at(&mut retry_at, profile_retry_at),
                }
            }
        }
        AuthProfileAutoSwitchStrategy::HighestAvailable => {
            let mut best: Option<(&str, f64)> = None;
            let mut first_unknown = None;
            for candidate in candidates {
                match health_by_profile
                    .get(candidate)
                    .copied()
                    .unwrap_or(UsageProfileHealth::Unknown)
                {
                    UsageProfileHealth::Healthy { remaining_percent } => {
                        if best
                            .as_ref()
                            .is_none_or(|(_, best_remaining)| remaining_percent > *best_remaining)
                        {
                            best = Some((candidate.as_str(), remaining_percent));
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
            if let Some((profile, _remaining_percent)) = best {
                return UsageProfileBrokerDecision::selected(
                    profile.to_string(),
                    UsageProfileBrokerDecisionReason::SelectedHealthyProfile,
                );
            }
            if let Some(profile) = first_unknown {
                return UsageProfileBrokerDecision::selected(
                    profile.to_string(),
                    UsageProfileBrokerDecisionReason::SelectedUnknownProfile,
                );
            }
        }
    }

    UsageProfileBrokerDecision {
        selected_profile: None,
        retry_at,
        reason: UsageProfileBrokerDecisionReason::NoAvailableProfiles,
    }
}

fn auth_profile_auto_switch_candidates(
    current: Option<&str>,
    config: &AuthProfileAutoSwitchConfig,
    saved_profiles: &[AuthProfile],
    locked_profiles: &HashSet<String>,
) -> Vec<String> {
    let ordered = ordered_auth_profiles_for_auto_switch(&config.profiles, saved_profiles);
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
        .filter(|profile| !locked_profiles.contains(profile.as_str()))
        .cloned()
        .collect()
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

fn usage_health_for_snapshots(
    snapshots: &[RateLimitSnapshot],
    config: &AuthProfileAutoSwitchConfig,
) -> UsageProfileHealth {
    let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
        .or_else(|| snapshots.first())
    else {
        return UsageProfileHealth::Unknown;
    };

    let mut has_enabled_window = false;
    let mut limiting_remaining_percent = 100.0;
    let mut retry_at = None;
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
        if window.used_percent >= 100.0 {
            merge_retry_at(&mut retry_at, window.resets_at);
        }
    }

    if !has_enabled_window {
        return UsageProfileHealth::Unknown;
    }
    if limiting_remaining_percent <= 0.0 || retry_at.is_some() {
        return UsageProfileHealth::Exhausted { retry_at };
    }
    UsageProfileHealth::Healthy {
        remaining_percent: limiting_remaining_percent,
    }
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

fn fallback_limit_label(is_secondary: bool) -> &'static str {
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

fn active_profile_leases(now: Instant) -> HashSet<String> {
    let Ok(mut leases) = PROFILE_BROKER_PROFILE_LEASES.lock() else {
        return HashSet::new();
    };
    leases.retain(|_, expires_at| *expires_at > now);
    leases.keys().cloned().collect()
}

fn lease_profile(profile: &str, now: Instant) {
    let Ok(mut leases) = PROFILE_BROKER_PROFILE_LEASES.lock() else {
        return;
    };
    leases.insert(
        profile.to_string(),
        now + PROFILE_BROKER_PROFILE_LEASE_DURATION,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::AuthMode;
    use codex_protocol::protocol::RateLimitWindow;

    fn chatgpt_profile(name: &str) -> AuthProfile {
        AuthProfile {
            name: name.to_string(),
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
            auth_mode: Some(AuthMode::Chatgpt),
            email: Some(format!("{name}@example.com")),
            account_id: Some(format!("acct-{name}")),
            plan: Some("plus".to_string()),
            active: false,
        }
    }

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

    fn usage_cache_key(
        profile: Option<&str>,
        config: &AuthProfileAutoSwitchConfig,
    ) -> ProfileBrokerUsageCacheKey {
        ProfileBrokerUsageCacheKey {
            codex_home: "/tmp/codewith-usage-broker-test".to_string(),
            chatgpt_base_url: "https://chatgpt.example.test".to_string(),
            profile: profile.map(str::to_string),
            on_5h_limit: config.on_5h_limit,
            on_weekly_limit: config.on_weekly_limit,
            heartbeat_interval_secs: config.heartbeat_interval_secs,
            heartbeat_freshness_secs: config.heartbeat_freshness_secs,
        }
    }

    #[test]
    fn dispatch_candidates_rotate_after_current_profile() {
        let profiles = vec![
            chatgpt_profile("work"),
            chatgpt_profile("second"),
            chatgpt_profile("third"),
        ];

        assert_eq!(
            auth_profile_auto_switch_candidates(
                Some("work"),
                &config(),
                &profiles,
                &HashSet::new(),
            ),
            vec!["second".to_string(), "third".to_string()]
        );
    }

    #[test]
    fn dispatch_candidates_skip_locally_locked_profiles() {
        let profiles = vec![
            chatgpt_profile("work"),
            chatgpt_profile("second"),
            chatgpt_profile("third"),
        ];
        let locked_profiles = HashSet::from(["second".to_string()]);

        assert_eq!(
            auth_profile_auto_switch_candidates(
                Some("work"),
                &config(),
                &profiles,
                &locked_profiles,
            ),
            vec!["third".to_string()]
        );
    }

    #[test]
    fn highest_available_dispatch_selects_healthiest_non_exhausted_profile() {
        let health_by_profile = BTreeMap::from([
            (
                "second".to_string(),
                UsageProfileHealth::Healthy {
                    remaining_percent: 20.0,
                },
            ),
            (
                "third".to_string(),
                UsageProfileHealth::Healthy {
                    remaining_percent: 80.0,
                },
            ),
        ]);

        assert_eq!(
            choose_dispatch_auth_profile(
                &config(),
                &["second".to_string(), "third".to_string()],
                &health_by_profile,
            ),
            UsageProfileBrokerDecision::selected(
                "third".to_string(),
                UsageProfileBrokerDecisionReason::SelectedHealthyProfile,
            )
        );
    }

    #[test]
    fn ordered_dispatch_skips_exhausted_profile_for_unknown_candidate() {
        let mut config = config();
        config.strategy = AuthProfileAutoSwitchStrategy::Ordered;
        let health_by_profile = BTreeMap::from([(
            "second".to_string(),
            UsageProfileHealth::Exhausted {
                retry_at: Some(500),
            },
        )]);

        assert_eq!(
            choose_dispatch_auth_profile(
                &config,
                &["second".to_string(), "third".to_string()],
                &health_by_profile,
            ),
            UsageProfileBrokerDecision::selected(
                "third".to_string(),
                UsageProfileBrokerDecisionReason::SelectedUnknownProfile,
            )
        );
    }

    #[test]
    fn dispatch_reports_retry_time_when_all_candidates_are_exhausted() {
        let health_by_profile = BTreeMap::from([
            (
                "second".to_string(),
                UsageProfileHealth::Exhausted {
                    retry_at: Some(700),
                },
            ),
            (
                "third".to_string(),
                UsageProfileHealth::Exhausted {
                    retry_at: Some(500),
                },
            ),
        ]);

        assert_eq!(
            choose_dispatch_auth_profile(
                &config(),
                &["second".to_string(), "third".to_string()],
                &health_by_profile,
            ),
            UsageProfileBrokerDecision {
                selected_profile: None,
                retry_at: Some(500),
                reason: UsageProfileBrokerDecisionReason::NoAvailableProfiles,
            }
        );
    }

    #[test]
    fn usage_health_uses_enabled_windows_only() {
        let mut config = config();
        config.on_5h_limit = false;
        config.on_weekly_limit = true;
        let snapshots = vec![RateLimitSnapshot {
            limit_id: Some("codex".to_string()),
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 100.0,
                window_minutes: Some(5 * 60),
                resets_at: Some(100),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: 40.0,
                window_minutes: Some(7 * 24 * 60),
                resets_at: Some(200),
            }),
            credits: None,
            individual_limit: None,
            plan_type: None,
            rate_limit_reached_type: None,
        }];

        assert_eq!(
            usage_health_for_snapshots(&snapshots, &config),
            UsageProfileHealth::Healthy {
                remaining_percent: 60.0,
            }
        );
    }

    #[test]
    fn usage_health_cache_reuses_fresh_health_and_expires_stale_health() {
        let config = config();
        let key = usage_cache_key(Some("work"), &config);
        let now = Instant::now();
        let health = UsageProfileHealth::Healthy {
            remaining_percent: 42.0,
        };

        cache_profile_health(key.clone(), health, &config, now);

        assert_eq!(
            cached_profile_health(&key, now + Duration::from_secs(119)),
            Some(health)
        );
        assert_eq!(
            cached_profile_health(&key, now + Duration::from_secs(121)),
            None
        );
    }

    #[test]
    fn usage_health_cache_key_tracks_policy_bits() {
        let mut config = config();
        let key = usage_cache_key(Some("work"), &config);

        config.on_weekly_limit = false;
        assert_ne!(usage_cache_key(Some("work"), &config), key);
    }

    #[test]
    fn unknown_usage_health_is_backed_off_longer_than_default_heartbeat() {
        let mut config = config();

        assert_eq!(
            profile_health_cache_duration(UsageProfileHealth::Unknown, &config),
            Duration::from_secs(300)
        );

        config.heartbeat_interval_secs = 600;
        assert_eq!(
            profile_health_cache_duration(UsageProfileHealth::Unknown, &config),
            Duration::from_secs(600)
        );
    }
}
