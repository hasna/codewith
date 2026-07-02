use anyhow::Context;
use clap::Parser;
use codex_app_server_protocol::AuthMode;
use codex_backend_client::AccountEntry;
use codex_backend_client::Client as BackendClient;
use codex_core::auth_profile_usage::AuthProfileUsageHealth;
use codex_core::auth_profile_usage::TokenUsageProfileResponse;
use codex_core::auth_profile_usage::usage_health_for_snapshots;
use codex_core::config::AuthProfileAutoSwitchConfig;
use codex_core::config::Config;
use codex_login::AuthManager;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use codex_protocol::account::PlanType as AccountPlanType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_utils_cli::CliConfigOverrides;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::time::timeout;

const USAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Parser)]
pub struct UsageCommand {
    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Inspect a saved auth profile without switching active auth.
    #[arg(long = "auth-profile", value_name = "NAME", conflicts_with_all = ["root", "all"])]
    auth_profile: Option<String>,

    /// Inspect the default root auth without switching active auth.
    #[arg(long, conflicts_with_all = ["auth_profile", "all"])]
    root: bool,

    /// Inspect root auth, saved profiles, and backend accounts/workspaces.
    #[arg(long, conflicts_with_all = ["auth_profile", "root"])]
    all: bool,

    /// Print structured JSON.
    #[arg(long)]
    json: bool,

    /// Include backend token-profile data where available.
    #[arg(long = "include-token-profile")]
    include_token_profile: bool,
}

struct UsageOptions {
    auth_profile: Option<String>,
    root: bool,
    all: bool,
    json: bool,
    include_token_profile: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageReport {
    targets: Vec<UsageTargetReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageTargetReport {
    target: UsageTarget,
    auth_mode: Option<AuthMode>,
    plan: Option<String>,
    redacted_account_id: Option<String>,
    spend_status: UsageSpendSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limits: Option<RateLimitUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_profile: Option<TokenUsageProfileResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_profile_error: Option<UsageError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accounts_error: Option<UsageError>,
    accounts: Vec<BackendAccountUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<UsageError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageTarget {
    display_name: String,
    profile_name: Option<String>,
    subscription_provider: Option<AuthProfileSubscriptionProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackendAccountUsage {
    name: Option<String>,
    structure: String,
    redacted_account_id: String,
    default: bool,
    spend_status: UsageSpendSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limits: Option<RateLimitUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_profile: Option<TokenUsageProfileResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_profile_error: Option<UsageError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<UsageError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitUsage {
    captured_at: i64,
    health: CliUsageHealth,
    snapshots: Vec<RateLimitSnapshot>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliUsageHealth {
    status: CliUsageHealthStatus,
    remaining_percent: Option<f64>,
    resets_at: Option<i64>,
    reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CliUsageHealthStatus {
    Healthy,
    Exhausted,
    Unknown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageError {
    reason: UsageErrorReason,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UsageErrorReason {
    AccountsFetchFailed,
    AccountsFetchTimedOut,
    FetchFailed,
    NoAuth,
    NotCodexBackend,
    RateLimitFetchTimedOut,
    TokenProfileFetchTimedOut,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSpendSummary {
    dollar_spend: UsageSpendAvailability,
    backend_credits: UsageSpendAvailability,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSpendAvailability {
    status: UsageSpendAvailabilityStatus,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum UsageSpendAvailabilityStatus {
    BackendReported,
    Unavailable,
}

pub async fn run_usage(command: UsageCommand) -> anyhow::Result<()> {
    let UsageCommand {
        config_overrides,
        auth_profile,
        root,
        all,
        json,
        include_token_profile,
    } = command;
    let options = UsageOptions {
        auth_profile,
        root,
        all,
        json,
        include_token_profile,
    };
    let config = load_config(config_overrides).await?;
    let profiles =
        codex_login::list_auth_profiles(&config.codex_home, config.cli_auth_credentials_store_mode)
            .context("failed to list auth profiles")?;
    let targets = usage_targets(&config, &profiles, &options)?;
    let mut reports = Vec::new();
    for target in targets {
        reports.push(
            fetch_target_report(&config, target, options.include_token_profile, options.all).await,
        );
    }
    let report = UsageReport { targets: reports };
    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }
    Ok(())
}

async fn load_config(cli_config_overrides: CliConfigOverrides) -> anyhow::Result<Config> {
    let cli_overrides = cli_config_overrides
        .parse_overrides()
        .map_err(|err| anyhow::anyhow!("error parsing -c overrides: {err}"))?;
    Config::load_with_cli_overrides(cli_overrides)
        .await
        .context("error loading configuration")
}

fn usage_targets(
    config: &Config,
    profiles: &[AuthProfile],
    command: &UsageOptions,
) -> anyhow::Result<Vec<UsageTarget>> {
    if command.all {
        let mut targets = vec![UsageTarget::root()];
        targets.extend(profiles.iter().map(UsageTarget::from_profile));
        return Ok(targets);
    }
    if command.root {
        return Ok(vec![UsageTarget::root()]);
    }
    if let Some(profile_name) = command.auth_profile.as_deref() {
        codex_login::validate_auth_profile_name(profile_name)?;
        let Some(profile) = profiles
            .iter()
            .find(|profile| profile.name.as_str() == profile_name)
        else {
            anyhow::bail!("unknown auth profile `{profile_name}`");
        };
        return Ok(vec![UsageTarget::from_profile(profile)]);
    }
    let target = match config.selected_auth_profile.as_deref() {
        Some(profile_name) => profiles
            .iter()
            .find(|profile| profile.name.as_str() == profile_name)
            .map(UsageTarget::from_profile)
            .unwrap_or_else(|| UsageTarget::profile_name(profile_name.to_string())),
        None => UsageTarget::root(),
    };
    Ok(vec![target])
}

async fn fetch_target_report(
    config: &Config,
    target: UsageTarget,
    include_token_profile: bool,
    include_backend_accounts: bool,
) -> UsageTargetReport {
    if target
        .subscription_provider
        .is_some_and(|provider| provider != AuthProfileSubscriptionProvider::ChatGpt)
    {
        return UsageTargetReport::unavailable(target, UsageErrorReason::NotCodexBackend);
    }

    let auth_manager = auth_manager_for_target(config, target.profile_name.clone()).await;
    let Some(auth) = auth_manager.auth().await else {
        return UsageTargetReport::unavailable(target, UsageErrorReason::NoAuth);
    };
    let auth_mode = Some(auth.api_auth_mode());
    let plan = auth.account_plan_type().map(account_plan_type_label);
    let redacted_account_id = auth.get_account_id().as_deref().map(redact_identifier);

    if !auth.uses_codex_backend() {
        return UsageTargetReport {
            target,
            auth_mode,
            plan,
            redacted_account_id,
            spend_status: UsageSpendSummary::account_without_backend_credits(),
            rate_limits: None,
            token_profile: None,
            token_profile_error: None,
            accounts_error: None,
            accounts: Vec::new(),
            error: Some(UsageError {
                reason: UsageErrorReason::NotCodexBackend,
            }),
        };
    }

    let client = match BackendClient::from_auth(config.chatgpt_base_url.clone(), &auth) {
        Ok(client) => client,
        Err(_) => {
            return UsageTargetReport {
                target,
                auth_mode,
                plan,
                redacted_account_id,
                spend_status: UsageSpendSummary::account_without_backend_credits(),
                rate_limits: None,
                token_profile: None,
                token_profile_error: None,
                accounts_error: None,
                accounts: Vec::new(),
                error: Some(UsageError {
                    reason: UsageErrorReason::FetchFailed,
                }),
            };
        }
    };

    let rate_limits = fetch_rate_limits(client.clone(), config).await;
    let (rate_limits, error) = match rate_limits {
        Ok(rate_limits) => (Some(rate_limits), None),
        Err(reason) => (None, Some(UsageError { reason })),
    };
    let (token_profile, token_profile_error) = if include_token_profile && error.is_none() {
        match fetch_token_profile(client.clone()).await {
            Ok(profile) => (Some(profile), None),
            Err(reason) => (None, Some(UsageError { reason })),
        }
    } else {
        (None, None)
    };
    let (accounts, accounts_error) = if include_backend_accounts {
        fetch_backend_accounts(client, config, include_token_profile).await
    } else {
        (Vec::new(), None)
    };
    let spend_status = rate_limits
        .as_ref()
        .map(|rate_limits| UsageSpendSummary::account_from_snapshots(&rate_limits.snapshots))
        .unwrap_or_else(UsageSpendSummary::account_without_backend_credits);

    UsageTargetReport {
        target,
        auth_mode,
        plan,
        redacted_account_id,
        spend_status,
        rate_limits,
        token_profile,
        token_profile_error,
        accounts_error,
        accounts,
        error,
    }
}

async fn auth_manager_for_target(config: &Config, profile: Option<String>) -> Arc<AuthManager> {
    let auth_manager = AuthManager::shared_with_auth_profile(
        config.codex_home.clone().to_path_buf(),
        /*enable_codex_api_key_env*/ true,
        config.cli_auth_credentials_store_mode,
        Some(config.chatgpt_base_url.clone()),
        profile,
    )
    .await;
    auth_manager.set_forced_chatgpt_workspace_id(config.forced_chatgpt_workspace_id.clone());
    auth_manager
}

async fn fetch_rate_limits(
    client: BackendClient,
    config: &Config,
) -> Result<RateLimitUsage, UsageErrorReason> {
    let captured_at = now_unix_secs();
    let snapshots = timeout(USAGE_FETCH_TIMEOUT, client.get_rate_limits_many())
        .await
        .map_err(|_| UsageErrorReason::RateLimitFetchTimedOut)?
        .map_err(|_| UsageErrorReason::FetchFailed)?;
    Ok(RateLimitUsage {
        captured_at,
        health: CliUsageHealth::from_snapshots(&snapshots, &config.auth_profile_auto_switch),
        snapshots,
    })
}

async fn fetch_token_profile(
    client: BackendClient,
) -> Result<TokenUsageProfileResponse, UsageErrorReason> {
    timeout(USAGE_FETCH_TIMEOUT, client.get_token_usage_profile())
        .await
        .map_err(|_| UsageErrorReason::TokenProfileFetchTimedOut)?
        .map(TokenUsageProfileResponse::from)
        .map_err(|_| UsageErrorReason::FetchFailed)
}

async fn fetch_backend_accounts(
    client: BackendClient,
    config: &Config,
    include_token_profile: bool,
) -> (Vec<BackendAccountUsage>, Option<UsageError>) {
    let accounts = match timeout(USAGE_FETCH_TIMEOUT, client.get_accounts_check()).await {
        Ok(Ok(accounts)) => accounts,
        Ok(Err(_)) => {
            return (
                Vec::new(),
                Some(UsageError {
                    reason: UsageErrorReason::AccountsFetchFailed,
                }),
            );
        }
        Err(_) => {
            return (
                Vec::new(),
                Some(UsageError {
                    reason: UsageErrorReason::AccountsFetchTimedOut,
                }),
            );
        }
    };
    let default_account_id = accounts.default_account_id.clone();
    let mut account_reports = Vec::new();
    for account in accounts.accounts {
        let account_client = client.clone().with_chatgpt_account_id(account.id.clone());
        account_reports.push(
            fetch_backend_account_usage(
                account,
                default_account_id.as_deref(),
                account_client,
                config,
                include_token_profile,
            )
            .await,
        );
    }
    (account_reports, None)
}

async fn fetch_backend_account_usage(
    account: AccountEntry,
    default_account_id: Option<&str>,
    client: BackendClient,
    config: &Config,
    include_token_profile: bool,
) -> BackendAccountUsage {
    let rate_limits = fetch_rate_limits(client.clone(), config).await;
    let (rate_limits, error) = match rate_limits {
        Ok(rate_limits) => (Some(rate_limits), None),
        Err(reason) => (None, Some(UsageError { reason })),
    };
    let (token_profile, token_profile_error) = if include_token_profile && error.is_none() {
        match fetch_token_profile(client).await {
            Ok(profile) => (Some(profile), None),
            Err(reason) => (None, Some(UsageError { reason })),
        }
    } else {
        (None, None)
    };
    let spend_status = rate_limits
        .as_ref()
        .map(|rate_limits| UsageSpendSummary::account_from_snapshots(&rate_limits.snapshots))
        .unwrap_or_else(UsageSpendSummary::account_without_backend_credits);

    BackendAccountUsage {
        default: default_account_id == Some(account.id.as_str()),
        redacted_account_id: redact_identifier(&account.id),
        name: account.name,
        structure: account.structure,
        spend_status,
        rate_limits,
        token_profile,
        token_profile_error,
        error,
    }
}

impl UsageTarget {
    fn root() -> Self {
        Self {
            display_name: "root".to_string(),
            profile_name: None,
            subscription_provider: Some(AuthProfileSubscriptionProvider::ChatGpt),
        }
    }

    fn from_profile(profile: &AuthProfile) -> Self {
        Self {
            display_name: profile.name.clone(),
            profile_name: Some(profile.name.clone()),
            subscription_provider: Some(profile.subscription_provider),
        }
    }

    fn profile_name(profile_name: String) -> Self {
        Self {
            display_name: profile_name.clone(),
            profile_name: Some(profile_name),
            subscription_provider: None,
        }
    }
}

impl UsageTargetReport {
    fn unavailable(target: UsageTarget, reason: UsageErrorReason) -> Self {
        Self {
            target,
            auth_mode: None,
            plan: None,
            redacted_account_id: None,
            spend_status: UsageSpendSummary::account_without_backend_credits(),
            rate_limits: None,
            token_profile: None,
            token_profile_error: None,
            accounts_error: None,
            accounts: Vec::new(),
            error: Some(UsageError { reason }),
        }
    }
}

impl CliUsageHealth {
    fn from_snapshots(
        snapshots: &[RateLimitSnapshot],
        config: &AuthProfileAutoSwitchConfig,
    ) -> Self {
        match usage_health_for_snapshots(snapshots, config) {
            AuthProfileUsageHealth::Healthy {
                remaining_percent,
                resets_at,
            } => Self {
                status: CliUsageHealthStatus::Healthy,
                remaining_percent: Some(remaining_percent),
                resets_at,
                reason: None,
            },
            AuthProfileUsageHealth::Exhausted { retry_at } => Self {
                status: CliUsageHealthStatus::Exhausted,
                remaining_percent: Some(0.0),
                resets_at: retry_at,
                reason: None,
            },
            AuthProfileUsageHealth::Unknown => Self {
                status: CliUsageHealthStatus::Unknown,
                remaining_percent: None,
                resets_at: None,
                reason: Some("unsupported_or_missing_usage_windows"),
            },
        }
    }
}

impl UsageSpendSummary {
    fn account_from_snapshots(snapshots: &[RateLimitSnapshot]) -> Self {
        let backend_credits = if snapshots
            .iter()
            .any(|snapshot| snapshot.credits.is_some() || snapshot.individual_limit.is_some())
        {
            UsageSpendAvailability {
                status: UsageSpendAvailabilityStatus::BackendReported,
                reason: "included_in_rate_limit_snapshots",
            }
        } else {
            UsageSpendAvailability::unavailable("no_backend_credit_or_spend_control_status")
        };
        Self {
            dollar_spend: UsageSpendAvailability::unavailable("no_backend_dollar_spend_endpoint"),
            backend_credits,
        }
    }

    fn account_without_backend_credits() -> Self {
        Self {
            dollar_spend: UsageSpendAvailability::unavailable("no_backend_dollar_spend_endpoint"),
            backend_credits: UsageSpendAvailability::unavailable(
                "no_backend_credit_or_spend_control_status",
            ),
        }
    }
}

impl UsageSpendAvailability {
    fn unavailable(reason: &'static str) -> Self {
        Self {
            status: UsageSpendAvailabilityStatus::Unavailable,
            reason,
        }
    }
}

fn print_human_report(report: &UsageReport) {
    for target in &report.targets {
        println!("Target: {}", target.target.display_name);
        if let Some(provider) = target.target.subscription_provider {
            println!("  Provider: {provider}");
        }
        if let Some(auth_mode) = target.auth_mode {
            println!("  Auth mode: {auth_mode}");
        }
        if let Some(plan) = target.plan.as_deref() {
            println!("  Plan: {plan}");
        }
        if let Some(account_id) = target.redacted_account_id.as_deref() {
            println!("  Account: {account_id}");
        }
        print_spend_status("  ", &target.spend_status);
        if let Some(rate_limits) = target.rate_limits.as_ref() {
            print_rate_limits("  ", rate_limits);
        }
        if let Some(token_profile) = target.token_profile.as_ref() {
            print_token_profile("  ", token_profile);
        }
        if let Some(error) = target.error.as_ref() {
            println!("  Error: {:?}", error.reason);
        }
        if let Some(error) = target.token_profile_error.as_ref() {
            println!("  Token profile error: {:?}", error.reason);
        }
        if let Some(error) = target.accounts_error.as_ref() {
            println!("  Accounts error: {:?}", error.reason);
        }
        for account in &target.accounts {
            println!(
                "  Backend account: {}{}",
                account.name.as_deref().unwrap_or("-"),
                if account.default { " (default)" } else { "" }
            );
            println!("    Account: {}", account.redacted_account_id);
            if !account.structure.is_empty() {
                println!("    Structure: {}", account.structure);
            }
            print_spend_status("    ", &account.spend_status);
            if let Some(rate_limits) = account.rate_limits.as_ref() {
                print_rate_limits("    ", rate_limits);
            }
            if let Some(token_profile) = account.token_profile.as_ref() {
                print_token_profile("    ", token_profile);
            }
            if let Some(error) = account.error.as_ref() {
                println!("    Error: {:?}", error.reason);
            }
            if let Some(error) = account.token_profile_error.as_ref() {
                println!("    Token profile error: {:?}", error.reason);
            }
        }
    }
}

fn print_spend_status(indent: &str, spend_status: &UsageSpendSummary) {
    println!(
        "{indent}Dollar spend: {:?} ({})",
        spend_status.dollar_spend.status, spend_status.dollar_spend.reason
    );
    println!(
        "{indent}Backend credits: {:?} ({})",
        spend_status.backend_credits.status, spend_status.backend_credits.reason
    );
}

fn print_rate_limits(indent: &str, rate_limits: &RateLimitUsage) {
    let health = &rate_limits.health;
    let remaining = health
        .remaining_percent
        .map(|remaining| format!("{remaining:.1}%"))
        .unwrap_or_else(|| "-".to_string());
    let resets_at = health
        .resets_at
        .map(|resets_at| resets_at.to_string())
        .unwrap_or_else(|| "-".to_string());
    println!(
        "{indent}Usage health: {:?}, remaining: {remaining}, resetsAt: {resets_at}",
        health.status
    );
    for snapshot in &rate_limits.snapshots {
        let name = snapshot
            .limit_name
            .as_deref()
            .or(snapshot.limit_id.as_deref())
            .unwrap_or("usage");
        println!("{indent}Rate limit: {name}");
        if let Some(primary) = snapshot.primary.as_ref() {
            println!(
                "{indent}  primary used: {:.1}% resetsAt: {}",
                primary.used_percent,
                primary
                    .resets_at
                    .map(|resets_at| resets_at.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
        if let Some(secondary) = snapshot.secondary.as_ref() {
            println!(
                "{indent}  secondary used: {:.1}% resetsAt: {}",
                secondary.used_percent,
                secondary
                    .resets_at
                    .map(|resets_at| resets_at.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
    }
}

fn print_token_profile(indent: &str, token_profile: &TokenUsageProfileResponse) {
    let summary = &token_profile.summary;
    println!(
        "{indent}Token profile: lifetime={}, peakDaily={}",
        optional_i64(summary.lifetime_tokens),
        optional_i64(summary.peak_daily_tokens)
    );
}

fn optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn redact_identifier(value: &str) -> String {
    if value.len() <= 8 {
        return "***".to_string();
    }
    let prefix = value.chars().take(4).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

fn account_plan_type_label(plan_type: AccountPlanType) -> String {
    match plan_type {
        AccountPlanType::Free => "Free",
        AccountPlanType::Go => "Go",
        AccountPlanType::Plus => "Plus",
        AccountPlanType::Pro => "Pro",
        AccountPlanType::ProLite => "Pro Lite",
        AccountPlanType::Team => "Team",
        AccountPlanType::SelfServeBusinessUsageBased => "Self Serve Business Usage Based",
        AccountPlanType::Business => "Business",
        AccountPlanType::EnterpriseCbpUsageBased => "Enterprise CBP Usage Based",
        AccountPlanType::Enterprise => "Enterprise",
        AccountPlanType::Edu => "Edu",
        AccountPlanType::Unknown => "Unknown",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_core::config::AuthProfileAutoSwitchConfig;
    use codex_core::config::AuthProfileAutoSwitchStrategy;
    use codex_protocol::protocol::RateLimitWindow;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn config() -> AuthProfileAutoSwitchConfig {
        AuthProfileAutoSwitchConfig {
            enabled: true,
            profiles: Vec::new(),
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

    async fn test_runtime_config() -> Config {
        let codex_home = TempDir::new().expect("temp dir");
        Config::load_default_with_cli_overrides_for_codex_home(
            codex_home.path().to_path_buf(),
            Vec::new(),
        )
        .await
        .expect("test config")
    }

    #[tokio::test]
    async fn usage_targets_rejects_unknown_auth_profile() {
        let config = test_runtime_config().await;
        let options = UsageOptions {
            auth_profile: Some("missing".to_string()),
            root: false,
            all: false,
            json: false,
            include_token_profile: false,
        };

        let err = usage_targets(
            &config,
            &[profile("work", AuthProfileSubscriptionProvider::ChatGpt)],
            &options,
        )
        .expect_err("unknown profile should fail");

        assert!(err.to_string().contains("unknown auth profile `missing`"));
    }

    #[tokio::test]
    async fn usage_targets_all_includes_root_and_saved_profiles() {
        let config = test_runtime_config().await;
        let options = UsageOptions {
            auth_profile: None,
            root: false,
            all: true,
            json: true,
            include_token_profile: true,
        };

        let targets = usage_targets(
            &config,
            &[
                profile("work", AuthProfileSubscriptionProvider::ChatGpt),
                profile("claude", AuthProfileSubscriptionProvider::ClaudeAi),
            ],
            &options,
        )
        .expect("all targets");

        assert_eq!(
            targets
                .iter()
                .map(|target| target.display_name.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "work", "claude"]
        );
    }

    #[test]
    fn usage_health_maps_rate_limit_snapshots() {
        assert_eq!(
            serde_json::to_value(CliUsageHealth::from_snapshots(
                &[snapshot(10.0, 80.0)],
                &config()
            ))
            .expect("serialize usage health"),
            serde_json::json!({
                "status": "healthy",
                "remainingPercent": 20.0,
                "resetsAt": 100,
                "reason": null
            })
        );
    }

    #[test]
    fn redact_identifier_does_not_return_short_ids_or_full_values() {
        assert_eq!(redact_identifier("short"), "***");
        assert_eq!(redact_identifier("account-123456"), "acco...3456");
    }
}
