use super::*;
use codex_core::config::AuthProfileAutoSwitchConfig;
use codex_core::usage_profile_health::UsageProfileCooldownKey;
use codex_core::usage_profile_health::UsageProfileHealth;
use codex_core::usage_profile_health::UsageProfileRateLimitSnapshot;
use codex_core::usage_profile_health::UsageProfileRateLimitWindow;
use codex_core::usage_profile_health::choose_profile_for_auto_switch;
use codex_core::usage_profile_health::cooldown_duration_for_reset;
use codex_core::usage_profile_health::exhausted_auto_switch_window;
use codex_core::usage_profile_health::usage_health_for_snapshots;
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

static PROFILE_BROKER_PROFILE_LEASES: LazyLock<StdMutex<BTreeMap<String, Instant>>> =
    LazyLock::new(|| StdMutex::new(BTreeMap::new()));
static PROFILE_BROKER_EXHAUSTED_PROFILE_COOLDOWNS: LazyLock<
    StdMutex<BTreeMap<UsageProfileCooldownKey, Instant>>,
> = LazyLock::new(|| StdMutex::new(BTreeMap::new()));

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

#[derive(Clone, Debug, PartialEq)]
struct FetchedProfileHealth {
    health: UsageProfileHealth,
    exhausted_cooldown: Option<UsageProfileCooldownKey>,
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
    match current_health.health {
        UsageProfileHealth::Healthy(_) => {
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

    let locked_profiles = active_profile_locks(Instant::now());
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
        let fetched = fetch_profile_health(auth_manager, config, Some(profile.as_str())).await;
        if let Some(cooldown_key) = fetched.exhausted_cooldown {
            lease_exhausted_profile(cooldown_key, config, Instant::now());
        }
        health_by_profile.insert(profile.clone(), fetched.health);
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
) -> FetchedProfileHealth {
    let profile = profile.map(str::to_string);
    let scoped_auth_manager = auth_manager
        .shared_scoped_auth_profile(profile.clone())
        .await;
    let health = async {
        let Some(auth) = scoped_auth_manager.auth().await else {
            return FetchedProfileHealth::unknown();
        };
        if !auth.uses_codex_backend() {
            return FetchedProfileHealth::unknown();
        }
        let Ok(client) = BackendClient::from_auth(config.chatgpt_base_url.clone(), &auth) else {
            return FetchedProfileHealth::unknown();
        };
        match client.get_rate_limits_many().await {
            Ok(snapshots) => {
                let shared_snapshots = snapshots
                    .iter()
                    .map(core_rate_limit_snapshot)
                    .collect::<Vec<_>>();
                FetchedProfileHealth {
                    health: usage_health_for_snapshots(
                        &shared_snapshots,
                        &config.auth_profile_auto_switch,
                        None,
                        /*is_fresh*/ true,
                    ),
                    exhausted_cooldown: exhausted_profile_cooldown_key(
                        profile.as_deref(),
                        &shared_snapshots,
                        &config.auth_profile_auto_switch,
                    ),
                }
            }
            Err(_err) => {
                tracing::debug!("usage profile broker could not fetch rate limits");
                FetchedProfileHealth::unknown()
            }
        }
    };

    match timeout(PROFILE_BROKER_RATE_LIMIT_FETCH_TIMEOUT, health).await {
        Ok(health) => health,
        Err(_) => FetchedProfileHealth::unknown(),
    }
}

fn choose_dispatch_auth_profile(
    config: &AuthProfileAutoSwitchConfig,
    candidates: &[String],
    health_by_profile: &BTreeMap<String, UsageProfileHealth>,
) -> UsageProfileBrokerDecision {
    let selection = choose_profile_for_auto_switch(config, candidates, health_by_profile);
    if let Some(profile) = selection.selected_profile {
        return UsageProfileBrokerDecision::selected(
            profile,
            match selection.reason {
                codex_core::usage_profile_health::UsageProfileSelectionReason::SelectedHealthyProfile => {
                    UsageProfileBrokerDecisionReason::SelectedHealthyProfile
                }
                codex_core::usage_profile_health::UsageProfileSelectionReason::SelectedUnknownProfile => {
                    UsageProfileBrokerDecisionReason::SelectedUnknownProfile
                }
                codex_core::usage_profile_health::UsageProfileSelectionReason::NoCandidateProfiles
                | codex_core::usage_profile_health::UsageProfileSelectionReason::NoAvailableProfiles => {
                    UsageProfileBrokerDecisionReason::NoAvailableProfiles
                }
            },
        );
    }

    match selection.reason {
        codex_core::usage_profile_health::UsageProfileSelectionReason::NoCandidateProfiles => {
            UsageProfileBrokerDecision::no_switch(
                UsageProfileBrokerDecisionReason::NoCandidateProfiles,
            )
        }
        codex_core::usage_profile_health::UsageProfileSelectionReason::NoAvailableProfiles
        | codex_core::usage_profile_health::UsageProfileSelectionReason::SelectedHealthyProfile
        | codex_core::usage_profile_health::UsageProfileSelectionReason::SelectedUnknownProfile => {
            UsageProfileBrokerDecision {
                selected_profile: None,
                retry_at: selection.retry_at,
                reason: UsageProfileBrokerDecisionReason::NoAvailableProfiles,
            }
        }
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

impl FetchedProfileHealth {
    fn unknown() -> Self {
        Self {
            health: UsageProfileHealth::Unknown,
            exhausted_cooldown: None,
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

fn exhausted_profile_cooldown_key(
    profile: Option<&str>,
    snapshots: &[UsageProfileRateLimitSnapshot<'_>],
    config: &AuthProfileAutoSwitchConfig,
) -> Option<UsageProfileCooldownKey> {
    snapshots.iter().find_map(|snapshot| {
        exhausted_auto_switch_window(snapshot, config).map(|window| {
            UsageProfileCooldownKey::new(
                profile.map(str::to_string),
                snapshot.limit_id.unwrap_or("codex"),
                window,
            )
        })
    })
}

fn dedupe_profile_names(profiles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .filter(|profile| seen.insert(profile.clone()))
        .collect()
}

fn active_profile_locks(now: Instant) -> HashSet<String> {
    let mut locked_profiles = active_profile_leases(now);
    locked_profiles.extend(active_exhausted_profile_cooldowns(now));
    locked_profiles
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

fn active_exhausted_profile_cooldowns(now: Instant) -> HashSet<String> {
    let Ok(mut cooldowns) = PROFILE_BROKER_EXHAUSTED_PROFILE_COOLDOWNS.lock() else {
        return HashSet::new();
    };
    cooldowns.retain(|_, expires_at| *expires_at > now);
    cooldowns
        .keys()
        .filter_map(|key| key.profile.clone())
        .collect()
}

fn lease_exhausted_profile(key: UsageProfileCooldownKey, config: &Config, now: Instant) {
    let Ok(mut cooldowns) = PROFILE_BROKER_EXHAUSTED_PROFILE_COOLDOWNS.lock() else {
        return;
    };
    let cooldown = cooldown_duration_for_reset(
        key.resets_at,
        Utc::now().timestamp(),
        config.usage_self_heal.reset_retry_buffer_secs,
        PROFILE_BROKER_PROFILE_LEASE_DURATION,
    );
    cooldowns.insert(key, now + cooldown);
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::AuthMode;
    use codex_core::config::AuthProfileAutoSwitchStrategy;
    use codex_core::usage_profile_health::UsageProfileScore;
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

    fn health(remaining_percent: f64) -> UsageProfileHealth {
        UsageProfileHealth::Healthy(UsageProfileScore {
            trigger_remaining_percent: remaining_percent,
            limiting_remaining_percent: remaining_percent,
        })
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
    fn dispatch_profile_leases_exhaust_candidates_until_lease_expiry() {
        PROFILE_BROKER_PROFILE_LEASES
            .lock()
            .expect("profile leases lock")
            .clear();

        let mut config = config();
        config.strategy = AuthProfileAutoSwitchStrategy::Ordered;
        let profiles = vec![
            chatgpt_profile("work"),
            chatgpt_profile("second"),
            chatgpt_profile("third"),
        ];
        let health_by_profile = BTreeMap::from([
            ("second".to_string(), health(20.0)),
            ("third".to_string(), health(80.0)),
        ]);
        let now = Instant::now();

        let first_candidates = auth_profile_auto_switch_candidates(
            Some("work"),
            &config,
            &profiles,
            &active_profile_leases(now),
        );
        assert_eq!(
            vec!["second".to_string(), "third".to_string()],
            first_candidates
        );
        let first = choose_dispatch_auth_profile(&config, &first_candidates, &health_by_profile);
        assert_eq!(
            UsageProfileBrokerDecision::selected(
                "second".to_string(),
                UsageProfileBrokerDecisionReason::SelectedHealthyProfile,
            ),
            first
        );
        lease_profile(
            first
                .selected_profile
                .as_deref()
                .expect("first candidate should be selected"),
            now,
        );

        let second_candidates = auth_profile_auto_switch_candidates(
            Some("work"),
            &config,
            &profiles,
            &active_profile_leases(now),
        );
        assert_eq!(vec!["third".to_string()], second_candidates);
        let second = choose_dispatch_auth_profile(&config, &second_candidates, &health_by_profile);
        assert_eq!(
            UsageProfileBrokerDecision::selected(
                "third".to_string(),
                UsageProfileBrokerDecisionReason::SelectedHealthyProfile,
            ),
            second
        );
        lease_profile(
            second
                .selected_profile
                .as_deref()
                .expect("second candidate should be selected"),
            now,
        );

        let exhausted_candidates = auth_profile_auto_switch_candidates(
            Some("work"),
            &config,
            &profiles,
            &active_profile_leases(now),
        );
        assert_eq!(Vec::<String>::new(), exhausted_candidates);
        assert_eq!(
            UsageProfileBrokerDecision::no_switch(
                UsageProfileBrokerDecisionReason::NoCandidateProfiles
            ),
            choose_dispatch_auth_profile(&config, &exhausted_candidates, &health_by_profile)
        );

        let after_expiry = now + PROFILE_BROKER_PROFILE_LEASE_DURATION + Duration::from_millis(1);
        assert_eq!(
            HashSet::<String>::new(),
            active_profile_leases(after_expiry)
        );
        assert_eq!(
            vec!["second".to_string(), "third".to_string()],
            auth_profile_auto_switch_candidates(
                Some("work"),
                &config,
                &profiles,
                &active_profile_leases(after_expiry),
            )
        );

        PROFILE_BROKER_PROFILE_LEASES
            .lock()
            .expect("profile leases lock")
            .clear();
    }

    #[test]
    fn highest_available_dispatch_selects_healthiest_non_exhausted_profile() {
        let health_by_profile = BTreeMap::from([
            ("second".to_string(), health(20.0)),
            ("third".to_string(), health(80.0)),
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
        let snapshots = [RateLimitSnapshot {
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
        let shared_snapshots = snapshots
            .iter()
            .map(core_rate_limit_snapshot)
            .collect::<Vec<_>>();

        assert_eq!(
            usage_health_for_snapshots(
                &shared_snapshots,
                &config,
                /*trigger_window_label*/ None,
                /*is_fresh*/ true,
            ),
            health(60.0)
        );
    }
}
