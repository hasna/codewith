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

/// Process exit code used when the command itself ran fine, but at least one
/// inspected target could NOT be verified against the provider — dead auth, a
/// failed or timed-out fetch, or a provider this command cannot check.
///
/// This exists because `usage` is the probe callers reach for to ask "is this
/// auth profile healthy". Before this code existed the answer was always exit
/// 0: an unknown profile name failed loudly (exit 1) while a profile whose auth
/// was dead succeeded quietly, so the exit code discriminated name resolution
/// only and never auth. `usage --auth-profile P && use-it` is now meaningful.
pub const USAGE_EXIT_TARGET_UNVERIFIED: i32 = 2;

/// Explanation attached to every unverified target, because the report body
/// still carries a plausible `plan` and `redactedAccountId` in that case: both
/// are read from the auth file on this machine BEFORE any request is made, so
/// they are present and real-looking even when the provider never answered.
const LOCAL_FILE_PROVENANCE_NOTE: &str = concat!(
    "The plan and account above are read from the LOCAL auth file on this machine, ",
    "not from the provider. They are NOT evidence that this profile's auth works."
);

const USAGE_EXIT_CODE_HELP: &str = "Exit codes:
  0  every inspected target was verified against the provider
  1  the command could not run (bad flags, unknown auth profile, bad config)
  2  the command ran, but at least one target could NOT be verified: dead or
     rejected auth, a failed or timed-out fetch, or a provider this command
     cannot check

Exit 2 exists because a report body is still populated when the provider never
answered: `plan` and `redactedAccountId` are read from the LOCAL auth file on
this machine before any request is made, so a dead profile prints a plausible
plan and account. Check the exit code, or the STATUS lines, or `.ok` in JSON --
never the presence of a plan.";

#[derive(Debug, Parser)]
#[command(after_long_help = USAGE_EXIT_CODE_HELP)]
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
    /// True only when every inspected target was verified against the provider.
    /// Mirrors the process exit code so a JSON consumer does not have to know to
    /// look inside `targets[..].error`.
    ok: bool,
    targets: Vec<UsageTargetReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageTargetReport {
    /// True when this target was actually verified against the provider.
    ///
    /// Deliberately initialised to `false` by every constructor and set only by
    /// [`UsageReport::new`]. If the normalisation step is ever skipped the
    /// output reports "not verified", which is the fail-safe direction for a
    /// health probe.
    ok: bool,
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
    let report = UsageReport::new(reports);
    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }

    // Both output routes are covered on purpose. The exit code makes
    // `usage --auth-profile P && use-it` correct for scripts; the stderr line
    // and the human STATUS block make the failure visible to a person who is
    // reading output rather than checking `$?`.
    if !report.ok {
        for line in report.failure_summary_lines() {
            eprintln!("{line}");
        }
        // `println!` writes through a LineWriter, but flush explicitly so no
        // buffered report can be lost to `process::exit`.
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = std::io::Write::flush(&mut std::io::stderr());
        std::process::exit(USAGE_EXIT_TARGET_UNVERIFIED);
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
            ok: false,
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
                ok: false,
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
        ok: false,
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

impl UsageReport {
    /// Single normalisation point for `ok`. Every `UsageTargetReport`
    /// constructor leaves `ok` false, so verification can only be asserted
    /// here, from the one field that records whether the provider answered.
    fn new(mut targets: Vec<UsageTargetReport>) -> Self {
        for target in &mut targets {
            target.ok = target.error.is_none();
        }
        let ok = targets.iter().all(|target| target.ok);
        Self { ok, targets }
    }

    /// One line per unverified target, for stderr. Kept separate from the human
    /// report so it is emitted in `--json` mode too, where stdout must stay
    /// parseable.
    fn failure_summary_lines(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter_map(|target| {
                let reason = target.error.as_ref()?.reason;
                Some(format!(
                    "codewith usage: NOT VERIFIED: `{}` could not be checked against the provider ({}). {}",
                    target.target.display_name,
                    reason.as_str(),
                    LOCAL_FILE_PROVENANCE_NOTE
                ))
            })
            .collect()
    }
}

impl UsageErrorReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::AccountsFetchFailed => "accounts_fetch_failed",
            Self::AccountsFetchTimedOut => "accounts_fetch_timed_out",
            Self::FetchFailed => "fetch_failed",
            Self::NoAuth => "no_auth",
            Self::NotCodexBackend => "not_codex_backend",
            Self::RateLimitFetchTimedOut => "rate_limit_fetch_timed_out",
            Self::TokenProfileFetchTimedOut => "token_profile_fetch_timed_out",
        }
    }

    /// Plain-language cause, so the human report does not require the reader to
    /// know what the enum variant means.
    fn explanation(self) -> &'static str {
        match self {
            Self::AccountsFetchFailed => "the backend rejected the accounts request",
            Self::AccountsFetchTimedOut => "the accounts request timed out",
            Self::FetchFailed => {
                "the provider request failed - the auth for this target is dead, rejected, or unreachable"
            }
            Self::NoAuth => "there is no usable auth stored for this target",
            Self::NotCodexBackend => {
                "this provider cannot be checked by this command, so its health is UNKNOWN, not healthy"
            }
            Self::RateLimitFetchTimedOut => "the rate-limit request timed out",
            Self::TokenProfileFetchTimedOut => "the token-profile request timed out",
        }
    }
}

impl UsageTargetReport {
    fn unavailable(target: UsageTarget, reason: UsageErrorReason) -> Self {
        Self {
            ok: false,
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
        // A target whose provider fetch failed still has a `plan` and an
        // account id, both read from the local auth file before any request was
        // made. Tag them at the point they are printed: an unqualified
        // "Plan: Pro" on a dead profile is precisely what has been read as
        // proof that the profile works.
        let local_only = if target.ok { "" } else { "  [local file]" };
        println!("Target: {}", target.target.display_name);
        if let Some(provider) = target.target.subscription_provider {
            println!("  Provider: {provider}");
        }
        if let Some(auth_mode) = target.auth_mode {
            println!("  Auth mode: {auth_mode}{local_only}");
        }
        if let Some(plan) = target.plan.as_deref() {
            println!("  Plan: {plan}{local_only}");
        }
        if let Some(account_id) = target.redacted_account_id.as_deref() {
            println!("  Account: {account_id}{local_only}");
        }
        print_spend_status("  ", &target.spend_status);
        if let Some(rate_limits) = target.rate_limits.as_ref() {
            print_rate_limits("  ", rate_limits);
        }
        if let Some(token_profile) = target.token_profile.as_ref() {
            print_token_profile("  ", token_profile);
        }
        print_target_status("  ", target);
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

/// The verdict lines. This is what a reader is meant to see; everything above
/// them is detail. They never say "verified" for a target the provider did not
/// confirm.
fn target_status_lines(target: &UsageTargetReport) -> Vec<String> {
    match target.error.as_ref() {
        None => vec!["STATUS: VERIFIED - the provider answered for this target".to_string()],
        Some(error) => {
            let reason = error.reason;
            vec![
                format!(
                    "STATUS: NOT VERIFIED - {} ({})",
                    reason.explanation(),
                    reason.as_str()
                ),
                format!("STATUS: {LOCAL_FILE_PROVENANCE_NOTE}"),
            ]
        }
    }
}

fn print_target_status(indent: &str, target: &UsageTargetReport) {
    for line in target_status_lines(target) {
        println!("{indent}{line}");
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

    /// A target report shaped exactly like the one a DEAD auth profile produces:
    /// `plan` and `redacted_account_id` populated from the local auth file,
    /// `error` set because the provider never answered.
    fn dead_auth_target_report(name: &str) -> UsageTargetReport {
        UsageTargetReport {
            ok: false,
            target: UsageTarget::profile_name(name.to_string()),
            auth_mode: Some(AuthMode::Chatgpt),
            plan: Some("Pro".to_string()),
            redacted_account_id: Some("acct...7890".to_string()),
            spend_status: UsageSpendSummary::account_without_backend_credits(),
            rate_limits: None,
            token_profile: None,
            token_profile_error: None,
            accounts_error: None,
            accounts: Vec::new(),
            error: Some(UsageError {
                reason: UsageErrorReason::FetchFailed,
            }),
        }
    }

    fn verified_target_report(name: &str) -> UsageTargetReport {
        UsageTargetReport {
            error: None,
            ..dead_auth_target_report(name)
        }
    }

    #[test]
    fn report_is_not_ok_when_a_target_failed_even_though_its_body_looks_complete() {
        let report = UsageReport::new(vec![dead_auth_target_report("account012")]);

        assert!(
            !report.ok,
            "a target the provider never confirmed is not ok"
        );
        assert!(!report.targets[0].ok);

        // The body a dead profile returns is deliberately asserted here: a
        // plausible plan and account id are what made four separate agents read
        // rc=0 as proof the profile worked. They come from the local auth file.
        let value = serde_json::to_value(&report).expect("serialize report");
        assert_eq!(value["ok"], serde_json::json!(false));
        assert_eq!(value["targets"][0]["ok"], serde_json::json!(false));
        assert_eq!(value["targets"][0]["plan"], serde_json::json!("Pro"));
        assert_eq!(
            value["targets"][0]["redactedAccountId"],
            serde_json::json!("acct...7890")
        );
        assert_eq!(
            value["targets"][0]["error"]["reason"],
            serde_json::json!("fetch_failed")
        );
    }

    #[test]
    fn report_is_ok_only_when_every_target_was_verified() {
        let all_good = UsageReport::new(vec![
            verified_target_report("account001"),
            verified_target_report("account011"),
        ]);
        assert!(all_good.ok);
        assert!(all_good.targets.iter().all(|target| target.ok));
        assert!(all_good.failure_summary_lines().is_empty());

        let mixed = UsageReport::new(vec![
            verified_target_report("account001"),
            dead_auth_target_report("account012"),
        ]);
        assert!(
            !mixed.ok,
            "one unverified target makes the whole run not ok"
        );
        assert!(mixed.targets[0].ok);
        assert!(!mixed.targets[1].ok);
    }

    #[test]
    fn failure_summary_names_the_target_and_the_local_file_provenance() {
        let report = UsageReport::new(vec![dead_auth_target_report("account012")]);
        let lines = report.failure_summary_lines();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("NOT VERIFIED"), "{}", lines[0]);
        assert!(lines[0].contains("account012"), "{}", lines[0]);
        assert!(lines[0].contains("fetch_failed"), "{}", lines[0]);
        assert!(lines[0].contains("LOCAL auth file"), "{}", lines[0]);
    }

    #[test]
    fn status_lines_never_claim_verification_for_a_failed_target() {
        let failed = target_status_lines(&dead_auth_target_report("account012")).join("\n");
        assert!(failed.contains("STATUS: NOT VERIFIED"), "{failed}");
        assert!(failed.contains("fetch_failed"), "{failed}");
        assert!(failed.contains("LOCAL auth file"), "{failed}");

        let verified = target_status_lines(&verified_target_report("account001")).join("\n");
        assert_eq!(
            verified, "STATUS: VERIFIED - the provider answered for this target",
            "a verified target gets exactly one unambiguous line"
        );
    }

    #[test]
    fn not_codex_backend_is_reported_as_unverified_rather_than_healthy() {
        let report = UsageReport::new(vec![UsageTargetReport::unavailable(
            UsageTarget::profile_name("claude".to_string()),
            UsageErrorReason::NotCodexBackend,
        )]);

        assert!(
            !report.ok,
            "a provider we cannot check is not a healthy one"
        );
        let lines = target_status_lines(&report.targets[0]).join("\n");
        assert!(lines.contains("UNKNOWN, not healthy"), "{lines}");
    }

    #[test]
    fn every_error_reason_has_a_wire_name_matching_its_serialized_form() {
        // `as_str` feeds the human and stderr output while serde feeds JSON;
        // this keeps a reader grepping for one from missing the other.
        for reason in [
            UsageErrorReason::AccountsFetchFailed,
            UsageErrorReason::AccountsFetchTimedOut,
            UsageErrorReason::FetchFailed,
            UsageErrorReason::NoAuth,
            UsageErrorReason::NotCodexBackend,
            UsageErrorReason::RateLimitFetchTimedOut,
            UsageErrorReason::TokenProfileFetchTimedOut,
        ] {
            assert_eq!(
                serde_json::to_value(reason).expect("serialize reason"),
                serde_json::json!(reason.as_str())
            );
            assert!(!reason.explanation().is_empty());
        }
    }

    #[test]
    fn redact_identifier_does_not_return_short_ids_or_full_values() {
        assert_eq!(redact_identifier("short"), "***");
        assert_eq!(redact_identifier("account-123456"), "acco...3456");
    }
}
