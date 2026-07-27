use crate::auth_profile_usage::AuthProfileUsageHealth;
use crate::auth_profile_usage::AuthProfileUsageRecommendationReason;
use crate::auth_profile_usage::TokenUsageProfileResponse;
use crate::auth_profile_usage::ordered_chatgpt_auth_profiles;
use crate::auth_profile_usage::recommend_auth_profile;
use crate::auth_profile_usage::usage_capture_is_stale;
use crate::auth_profile_usage::usage_health_for_snapshots;
use crate::config::AuthProfileAutoSwitchConfig;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::auth_profile_usage_control_spec::GET_USAGE_TOOL_NAME;
use crate::tools::handlers::auth_profile_usage_control_spec::create_get_usage_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_app_server_protocol::AuthMode;
use codex_backend_client::Client as BackendClient;
use codex_login::AuthProfileSubscriptionProvider;
use codex_login::CodexAuth;
use codex_protocol::account::PlanType as AccountPlanType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::TokenUsageInfo;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use futures::StreamExt;
use futures::TryStreamExt;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::future::Future;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::timeout;

const AUTH_PROFILE_USAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONCURRENT_ACCOUNT_USAGE_FETCHES: usize = 4;
static AUTH_PROFILE_USAGE_CACHE: LazyLock<
    Mutex<BTreeMap<AuthProfileUsageCacheKey, AuthProfileUsageCacheEntry>>,
> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub struct GetUsageHandler;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GetUsageScope {
    Session,
    Account,
    AllAccounts,
    Both,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct GetUsageArgs {
    scope: GetUsageScope,
    #[serde(default)]
    auth_profile: Option<String>,
    #[serde(default)]
    include_token_profile: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetUsageResponse {
    scope: GetUsageScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<SessionUsageResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<AccountUsageResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accounts: Option<Vec<AccountUsageResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recommendation: Option<AccountUsageRecommendationResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionUsageResponse {
    token_usage: Option<TokenUsageInfo>,
    spend_status: UsageSpendSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageResponse {
    target: AccountUsageTarget,
    current: bool,
    include_token_profile: bool,
    spend_status: UsageSpendSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limits: Option<AccountRateLimitUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_profile: Option<TokenUsageProfileResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_profile_error: Option<AccountUsageError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<AccountUsageError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageRecommendationResponse {
    profile_name: Option<String>,
    display_name: String,
    current: bool,
    reason: AuthProfileUsageRecommendationReason,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageTarget {
    profile_name: Option<String>,
    subscription_provider: AuthProfileSubscriptionProvider,
    auth_mode: Option<AuthMode>,
    plan: Option<String>,
    redacted_account_id: Option<String>,
}

#[derive(Clone, Debug)]
struct AccountUsageLookupTarget {
    profile_name: Option<String>,
    subscription_provider: AuthProfileSubscriptionProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountRateLimitUsage {
    captured_at: i64,
    stale_after_secs: u64,
    health: AuthProfileUsageSummary,
    snapshots: Vec<RateLimitSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthProfileUsageSummary {
    status: AuthProfileUsageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured_at: Option<i64>,
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<AuthProfileUsageStatusReason>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum AuthProfileUsageStatus {
    Healthy,
    Exhausted,
    Unknown,
    #[cfg(test)]
    Unavailable,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AuthProfileUsageStatusReason {
    FetchFailed,
    NoAuth,
    NotCodexBackend,
    RateLimitFetchTimedOut,
    TokenProfileFetchTimedOut,
    UnsupportedOrMissingUsageWindows,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageError {
    reason: AuthProfileUsageStatusReason,
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AuthProfileUsageCacheKey {
    codex_home: String,
    base_url: String,
    profile: Option<String>,
    auth_mode: String,
    account_id: String,
}

#[derive(Clone, Debug)]
struct AuthProfileUsageCacheEntry {
    captured_at: i64,
    snapshots: Vec<RateLimitSnapshot>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for GetUsageHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(GET_USAGE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_get_usage_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "get_usage handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: GetUsageArgs = parse_arguments(&arguments)?;
        let current_profile = session.selected_auth_profile().await;
        let target_profile =
            normalize_requested_profile(args.auth_profile.clone(), current_profile.clone())?;
        let response =
            get_usage_response(&session, &turn, args, current_profile, target_profile).await?;
        let response = serde_json::to_string_pretty(&response)
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            response,
            Some(true),
        )))
    }
}

impl CoreToolRuntime for GetUsageHandler {}

async fn get_usage_response(
    session: &Session,
    turn: &TurnContext,
    args: GetUsageArgs,
    current_profile: Option<String>,
    target_profile: Option<String>,
) -> Result<GetUsageResponse, FunctionCallError> {
    let include_session = matches!(args.scope, GetUsageScope::Session | GetUsageScope::Both);
    let include_account = matches!(args.scope, GetUsageScope::Account | GetUsageScope::Both);
    let include_accounts = matches!(args.scope, GetUsageScope::AllAccounts);
    let session_usage = if include_session {
        Some(SessionUsageResponse {
            token_usage: session.token_usage_info().await,
            spend_status: UsageSpendSummary::session(),
        })
    } else {
        None
    };
    let account_usage = if include_account {
        Some(
            fetch_account_usage(
                session,
                turn,
                target_profile,
                current_profile.as_deref(),
                args.include_token_profile,
            )
            .await?,
        )
    } else {
        None
    };
    let (accounts_usage, recommendation) = if include_accounts {
        let (accounts, ordered_profiles) = fetch_all_account_usages(
            session,
            turn,
            current_profile.as_deref(),
            args.include_token_profile,
        )
        .await?;
        let recommendation = Some(account_usage_recommendation(
            &accounts,
            current_profile.as_deref(),
            &turn.config.auth_profile_auto_switch,
            &ordered_profiles,
        ));
        (Some(accounts), recommendation)
    } else {
        (None, None)
    };
    Ok(GetUsageResponse {
        scope: args.scope,
        session: session_usage,
        account: account_usage,
        accounts: accounts_usage,
        recommendation,
    })
}

fn normalize_requested_profile(
    requested_profile: Option<String>,
    current_profile: Option<String>,
) -> Result<Option<String>, FunctionCallError> {
    let Some(profile) = requested_profile else {
        return Ok(current_profile);
    };
    let profile = profile.trim();
    if profile.is_empty() {
        return Ok(None);
    }
    codex_login::validate_auth_profile_name(profile)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    Ok(Some(profile.to_string()))
}

async fn fetch_account_usage(
    session: &Session,
    turn: &TurnContext,
    target_profile: Option<String>,
    current_profile: Option<&str>,
    include_token_profile: bool,
) -> Result<AccountUsageResponse, FunctionCallError> {
    let target = resolve_account_usage_target(turn, target_profile)?;
    fetch_account_usage_unvalidated(
        session,
        turn,
        target,
        current_profile,
        include_token_profile,
    )
    .await
}

async fn fetch_account_usage_unvalidated(
    session: &Session,
    turn: &TurnContext,
    target: AccountUsageLookupTarget,
    current_profile: Option<&str>,
    include_token_profile: bool,
) -> Result<AccountUsageResponse, FunctionCallError> {
    let AccountUsageLookupTarget {
        profile_name: target_profile,
        subscription_provider,
    } = target;
    let current = target_profile.as_deref() == current_profile;
    if subscription_provider != AuthProfileSubscriptionProvider::ChatGpt {
        return Ok(account_unavailable_response(
            target_profile,
            subscription_provider,
            current,
            include_token_profile,
            AuthProfileUsageStatusReason::NotCodexBackend,
        ));
    }
    let scoped_auth_manager = session
        .services
        .auth_manager
        .shared_scoped_auth_profile(target_profile.clone())
        .await;
    let Some(auth) = scoped_auth_manager.auth().await else {
        return Ok(account_unavailable_response(
            target_profile,
            subscription_provider,
            current,
            include_token_profile,
            AuthProfileUsageStatusReason::NoAuth,
        ));
    };
    let target =
        AccountUsageTarget::from_auth(target_profile.clone(), subscription_provider, &auth);
    if !auth.uses_codex_backend() {
        return Ok(AccountUsageResponse {
            target,
            current,
            include_token_profile,
            spend_status: UsageSpendSummary::account_without_backend_credits(),
            rate_limits: None,
            token_profile: None,
            token_profile_error: None,
            error: Some(AccountUsageError {
                reason: AuthProfileUsageStatusReason::NotCodexBackend,
            }),
        });
    }
    let client = match BackendClient::from_auth(turn.config.chatgpt_base_url.clone(), &auth) {
        Ok(client) => client,
        Err(_) => {
            return Ok(AccountUsageResponse {
                target,
                current,
                include_token_profile,
                spend_status: UsageSpendSummary::account_without_backend_credits(),
                rate_limits: None,
                token_profile: None,
                token_profile_error: None,
                error: Some(AccountUsageError {
                    reason: AuthProfileUsageStatusReason::FetchFailed,
                }),
            });
        }
    };

    let captured_at = chrono::Utc::now().timestamp();
    let rate_limits = fetch_rate_limit_snapshots(
        turn,
        target_profile.clone(),
        &auth,
        client.clone(),
        captured_at,
    )
    .await;
    let (rate_limits, error) = match rate_limits {
        Ok(snapshots) => {
            let health = AuthProfileUsageSummary::from_snapshots(
                &snapshots,
                &turn.config.auth_profile_auto_switch,
                captured_at,
            );
            (
                Some(AccountRateLimitUsage {
                    captured_at,
                    stale_after_secs: turn
                        .config
                        .auth_profile_auto_switch
                        .heartbeat_freshness_secs,
                    health,
                    snapshots,
                }),
                None,
            )
        }
        Err(reason) => (None, Some(AccountUsageError { reason })),
    };

    let (token_profile, token_profile_error) = if include_token_profile && error.is_none() {
        match fetch_token_usage_profile(client).await {
            Ok(profile) => (Some(profile), None),
            Err(reason) => (None, Some(AccountUsageError { reason })),
        }
    } else {
        (None, None)
    };
    let spend_status = rate_limits
        .as_ref()
        .map(|limits| UsageSpendSummary::account_from_snapshots(&limits.snapshots))
        .unwrap_or_else(UsageSpendSummary::account_without_backend_credits);

    Ok(AccountUsageResponse {
        target,
        current,
        include_token_profile,
        spend_status,
        rate_limits,
        token_profile,
        token_profile_error,
        error,
    })
}

fn resolve_account_usage_target(
    turn: &TurnContext,
    target_profile: Option<String>,
) -> Result<AccountUsageLookupTarget, FunctionCallError> {
    let Some(profile_name) = target_profile else {
        return Ok(AccountUsageLookupTarget {
            profile_name: None,
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
        });
    };
    let profiles = codex_login::list_auth_profiles(
        &turn.config.codex_home,
        turn.config.cli_auth_credentials_store_mode,
    )
    .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    let profile = profiles
        .iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!("unknown auth profile `{profile_name}`"))
        })?;
    Ok(AccountUsageLookupTarget {
        profile_name: Some(profile.name.clone()),
        subscription_provider: profile.subscription_provider,
    })
}

fn account_unavailable_response(
    target_profile: Option<String>,
    subscription_provider: AuthProfileSubscriptionProvider,
    current: bool,
    include_token_profile: bool,
    reason: AuthProfileUsageStatusReason,
) -> AccountUsageResponse {
    AccountUsageResponse {
        target: AccountUsageTarget {
            profile_name: target_profile,
            subscription_provider,
            auth_mode: None,
            plan: None,
            redacted_account_id: None,
        },
        current,
        include_token_profile,
        spend_status: UsageSpendSummary::account_without_backend_credits(),
        rate_limits: None,
        token_profile: None,
        token_profile_error: None,
        error: Some(AccountUsageError { reason }),
    }
}

async fn fetch_all_account_usages(
    session: &Session,
    turn: &TurnContext,
    current_profile: Option<&str>,
    include_token_profile: bool,
) -> Result<(Vec<AccountUsageResponse>, Vec<Option<String>>), FunctionCallError> {
    let saved_profiles = codex_login::list_auth_profiles(
        &turn.config.codex_home,
        turn.config.cli_auth_credentials_store_mode,
    )
    .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    let ordered_profiles = ordered_chatgpt_auth_profiles(
        &turn.config.auth_profile_auto_switch.profiles,
        &saved_profiles,
    )
    .into_iter()
    .map(Some)
    .collect::<Vec<_>>();
    let mut targets = vec![AccountUsageLookupTarget {
        profile_name: None,
        subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
    }];
    let mut seen = HashSet::from([None]);
    for profile in saved_profiles {
        let profile_name = Some(profile.name);
        if seen.insert(profile_name.clone()) {
            targets.push(AccountUsageLookupTarget {
                profile_name,
                subscription_provider: profile.subscription_provider,
            });
        }
    }

    // Bounded fan-out: `all_accounts` can cover every saved profile, and each target is a
    // separate backend round trip. `buffered` keeps the response ordered while capping how many
    // usage requests are in flight, so listing profiles never turns into a burst against the
    // backend.
    let usages = collect_bounded_account_usages(targets.into_iter().map(|target| {
        fetch_account_usage_unvalidated(
            session,
            turn,
            target,
            current_profile,
            include_token_profile,
        )
    }))
    .await?;
    Ok((usages, ordered_profiles))
}

async fn collect_bounded_account_usages<F>(
    futures: impl IntoIterator<Item = F>,
) -> Result<Vec<AccountUsageResponse>, FunctionCallError>
where
    F: Future<Output = Result<AccountUsageResponse, FunctionCallError>>,
{
    futures::stream::iter(futures)
        .buffered(MAX_CONCURRENT_ACCOUNT_USAGE_FETCHES)
        .try_collect()
        .await
}

fn account_usage_recommendation(
    accounts: &[AccountUsageResponse],
    current_profile: Option<&str>,
    config: &AuthProfileAutoSwitchConfig,
    ordered_profiles: &[Option<String>],
) -> AccountUsageRecommendationResponse {
    let health_by_profile = accounts
        .iter()
        .filter_map(|account| {
            let limits = account.rate_limits.as_ref()?;
            Some((
                account.target.profile_name.clone(),
                auth_profile_usage_health_from_summary(&limits.health),
            ))
        })
        .collect::<Vec<_>>();
    let recommendation = recommend_auth_profile(
        current_profile,
        config.strategy,
        ordered_profiles,
        &health_by_profile,
    );
    let profile_name = recommendation.profile;
    AccountUsageRecommendationResponse {
        display_name: display_name_for_profile(profile_name.as_deref()),
        current: profile_name.as_deref() == current_profile,
        profile_name,
        reason: recommendation.reason,
    }
}

fn auth_profile_usage_health_from_summary(
    summary: &AuthProfileUsageSummary,
) -> AuthProfileUsageHealth {
    if summary.stale {
        return AuthProfileUsageHealth::Unknown;
    }
    match summary.status {
        AuthProfileUsageStatus::Healthy => AuthProfileUsageHealth::Healthy {
            remaining_percent: summary.remaining_percent.unwrap_or(0.0),
            resets_at: summary.resets_at,
        },
        AuthProfileUsageStatus::Exhausted => AuthProfileUsageHealth::Exhausted {
            retry_at: summary.resets_at,
        },
        AuthProfileUsageStatus::Unknown => AuthProfileUsageHealth::Unknown,
        #[cfg(test)]
        AuthProfileUsageStatus::Unavailable => AuthProfileUsageHealth::Unknown,
    }
}

fn display_name_for_profile(profile_name: Option<&str>) -> String {
    profile_name.unwrap_or("Default").to_string()
}

async fn fetch_rate_limit_snapshots(
    turn: &TurnContext,
    target_profile: Option<String>,
    auth: &CodexAuth,
    client: BackendClient,
    captured_at: i64,
) -> Result<Vec<RateLimitSnapshot>, AuthProfileUsageStatusReason> {
    let cache_key = usage_cache_key(turn, target_profile, auth);
    if let Some(cache_key) = cache_key.as_ref()
        && let Some(snapshots) =
            cached_rate_limit_snapshots(cache_key, &turn.config.auth_profile_auto_switch).await
    {
        return Ok(snapshots);
    }

    let snapshots = timeout(
        AUTH_PROFILE_USAGE_FETCH_TIMEOUT,
        client.get_rate_limits_many(),
    )
    .await
    .map_err(|_| AuthProfileUsageStatusReason::RateLimitFetchTimedOut)?
    .map_err(|_| AuthProfileUsageStatusReason::FetchFailed)?;
    if let Some(cache_key) = cache_key {
        AUTH_PROFILE_USAGE_CACHE.lock().await.insert(
            cache_key,
            AuthProfileUsageCacheEntry {
                captured_at,
                snapshots: snapshots.clone(),
            },
        );
    }
    Ok(snapshots)
}

fn usage_cache_key(
    turn: &TurnContext,
    target_profile: Option<String>,
    auth: &CodexAuth,
) -> Option<AuthProfileUsageCacheKey> {
    Some(AuthProfileUsageCacheKey {
        codex_home: turn.config.codex_home.to_string_lossy().into_owned(),
        base_url: turn.config.chatgpt_base_url.clone(),
        profile: target_profile,
        auth_mode: auth.api_auth_mode().to_string(),
        account_id: auth.get_account_id()?,
    })
}

async fn cached_rate_limit_snapshots(
    cache_key: &AuthProfileUsageCacheKey,
    config: &crate::config::AuthProfileAutoSwitchConfig,
) -> Option<Vec<RateLimitSnapshot>> {
    let now = chrono::Utc::now().timestamp();
    let entry = AUTH_PROFILE_USAGE_CACHE
        .lock()
        .await
        .get(cache_key)
        .cloned()?;
    if usage_capture_is_stale(entry.captured_at, now, config.heartbeat_freshness_secs) {
        return None;
    }
    Some(entry.snapshots)
}

async fn fetch_token_usage_profile(
    client: BackendClient,
) -> Result<TokenUsageProfileResponse, AuthProfileUsageStatusReason> {
    timeout(
        AUTH_PROFILE_USAGE_FETCH_TIMEOUT,
        client.get_token_usage_profile(),
    )
    .await
    .map_err(|_| AuthProfileUsageStatusReason::TokenProfileFetchTimedOut)?
    .map(TokenUsageProfileResponse::from)
    .map_err(|_| AuthProfileUsageStatusReason::FetchFailed)
}

impl AccountUsageTarget {
    fn from_auth(
        profile_name: Option<String>,
        subscription_provider: AuthProfileSubscriptionProvider,
        auth: &CodexAuth,
    ) -> Self {
        Self {
            profile_name,
            subscription_provider,
            auth_mode: Some(auth.api_auth_mode()),
            plan: auth.account_plan_type().map(account_plan_type_label),
            redacted_account_id: auth.get_account_id().as_deref().map(redact_identifier),
        }
    }
}

impl AuthProfileUsageSummary {
    fn from_snapshots(
        snapshots: &[RateLimitSnapshot],
        config: &crate::config::AuthProfileAutoSwitchConfig,
        captured_at: i64,
    ) -> Self {
        let health = usage_health_for_snapshots(snapshots, config);
        let now = chrono::Utc::now().timestamp();
        let stale = usage_capture_is_stale(captured_at, now, config.heartbeat_freshness_secs);
        match health {
            AuthProfileUsageHealth::Healthy {
                remaining_percent,
                resets_at,
            } => Self {
                status: AuthProfileUsageStatus::Healthy,
                remaining_percent: Some(remaining_percent),
                resets_at,
                captured_at: Some(captured_at),
                stale,
                reason: None,
            },
            AuthProfileUsageHealth::Exhausted { retry_at } => Self {
                status: AuthProfileUsageStatus::Exhausted,
                remaining_percent: Some(0.0),
                resets_at: retry_at,
                captured_at: Some(captured_at),
                stale,
                reason: None,
            },
            AuthProfileUsageHealth::Unknown => Self {
                status: AuthProfileUsageStatus::Unknown,
                remaining_percent: None,
                resets_at: None,
                captured_at: Some(captured_at),
                stale,
                reason: Some(AuthProfileUsageStatusReason::UnsupportedOrMissingUsageWindows),
            },
        }
    }

    #[cfg(test)]
    fn unavailable(reason: AuthProfileUsageStatusReason) -> Self {
        Self {
            status: AuthProfileUsageStatus::Unavailable,
            remaining_percent: None,
            resets_at: None,
            captured_at: None,
            stale: false,
            reason: Some(reason),
        }
    }
}

impl UsageSpendSummary {
    fn session() -> Self {
        Self {
            dollar_spend: UsageSpendAvailability::unavailable("session_dollar_spend_not_tracked"),
            backend_credits: UsageSpendAvailability::unavailable(
                "session_backend_credit_status_not_tracked",
            ),
        }
    }

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
    use crate::config::AuthProfileAutoSwitchConfig;
    use crate::config::AuthProfileAutoSwitchStrategy;
    use codex_protocol::protocol::CreditsSnapshot;
    use codex_protocol::protocol::RateLimitWindow;
    use pretty_assertions::assert_eq;

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

    #[test]
    fn usage_summary_serializes_sanitized_unavailable_reason() {
        let summary =
            AuthProfileUsageSummary::unavailable(AuthProfileUsageStatusReason::FetchFailed);
        let response = serde_json::to_value(summary).expect("serialize summary");

        assert_eq!(
            response,
            serde_json::json!({
                "status": "unavailable",
                "stale": false,
                "reason": "fetch_failed"
            })
        );
        let serialized = response.to_string();
        assert!(!serialized.contains("refresh_token"));
        assert!(!serialized.contains("access_token"));
    }

    #[test]
    fn usage_summary_maps_snapshots_to_health() {
        let captured_at = chrono::Utc::now().timestamp();
        let response = AuthProfileUsageSummary::from_snapshots(
            &[snapshot(10.0, 80.0)],
            &config(),
            captured_at,
        );
        let response = serde_json::to_value(response).expect("serialize summary");

        assert_eq!(
            response,
            serde_json::json!({
                "status": "healthy",
                "remainingPercent": 20.0,
                "resetsAt": 100,
                "capturedAt": captured_at,
                "stale": false
            })
        );
    }

    #[test]
    fn usage_summary_ignores_empty_credits_when_codex_windows_are_healthy() {
        let captured_at = chrono::Utc::now().timestamp();
        let mut snapshot = snapshot(4.0, 39.0);
        snapshot.credits = Some(CreditsSnapshot {
            has_credits: false,
            unlimited: false,
            balance: Some("0".to_string()),
        });

        let response = AuthProfileUsageSummary::from_snapshots(&[snapshot], &config(), captured_at);
        let response = serde_json::to_value(response).expect("serialize summary");

        assert_eq!(
            response,
            serde_json::json!({
                "status": "healthy",
                "remainingPercent": 61.0,
                "resetsAt": 100,
                "capturedAt": captured_at,
                "stale": false
            })
        );
    }

    fn account_usage(
        profile_name: Option<&str>,
        current: bool,
        health: AuthProfileUsageSummary,
    ) -> AccountUsageResponse {
        AccountUsageResponse {
            target: AccountUsageTarget {
                profile_name: profile_name.map(str::to_string),
                subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
                auth_mode: None,
                plan: None,
                redacted_account_id: None,
            },
            current,
            include_token_profile: false,
            spend_status: UsageSpendSummary::account_without_backend_credits(),
            rate_limits: Some(AccountRateLimitUsage {
                captured_at: 123,
                stale_after_secs: 120,
                health,
                snapshots: Vec::new(),
            }),
            token_profile: None,
            token_profile_error: None,
            error: None,
        }
    }

    #[test]
    fn account_usage_recommendation_serializes_shared_reason_codes() {
        let accounts = vec![
            account_usage(
                Some("work"),
                true,
                AuthProfileUsageSummary {
                    status: AuthProfileUsageStatus::Exhausted,
                    remaining_percent: Some(0.0),
                    resets_at: Some(100),
                    captured_at: Some(123),
                    stale: false,
                    reason: None,
                },
            ),
            account_usage(
                Some("spare"),
                false,
                AuthProfileUsageSummary {
                    status: AuthProfileUsageStatus::Healthy,
                    remaining_percent: Some(80.0),
                    resets_at: Some(200),
                    captured_at: Some(123),
                    stale: false,
                    reason: None,
                },
            ),
        ];

        let recommendation = account_usage_recommendation(
            &accounts,
            Some("work"),
            &config(),
            &[Some("spare".to_string()), Some("work".to_string())],
        );
        let response = serde_json::to_value(recommendation).expect("serialize recommendation");

        assert_eq!(
            response,
            serde_json::json!({
                "profileName": "spare",
                "displayName": "spare",
                "current": false,
                "reason": "selected_highest_remaining"
            })
        );
    }

    #[test]
    fn account_usage_recommendation_respects_auto_switch_candidate_order() {
        let accounts = vec![
            account_usage(
                Some("work"),
                true,
                AuthProfileUsageSummary {
                    status: AuthProfileUsageStatus::Exhausted,
                    remaining_percent: Some(0.0),
                    resets_at: Some(100),
                    captured_at: Some(123),
                    stale: false,
                    reason: None,
                },
            ),
            account_usage(
                Some("excluded"),
                false,
                AuthProfileUsageSummary {
                    status: AuthProfileUsageStatus::Healthy,
                    remaining_percent: Some(99.0),
                    resets_at: Some(200),
                    captured_at: Some(123),
                    stale: false,
                    reason: None,
                },
            ),
            account_usage(
                Some("configured"),
                false,
                AuthProfileUsageSummary {
                    status: AuthProfileUsageStatus::Healthy,
                    remaining_percent: Some(40.0),
                    resets_at: Some(200),
                    captured_at: Some(123),
                    stale: false,
                    reason: None,
                },
            ),
        ];

        let recommendation = account_usage_recommendation(
            &accounts,
            Some("work"),
            &config(),
            &[Some("configured".to_string()), Some("work".to_string())],
        );

        assert_eq!(
            serde_json::to_value(recommendation).expect("serialize recommendation"),
            serde_json::json!({
                "profileName": "configured",
                "displayName": "configured",
                "current": false,
                "reason": "selected_highest_remaining"
            })
        );
    }

    #[test]
    fn non_chatgpt_usage_is_provider_accurate_and_does_not_expose_account_details() {
        let response = account_unavailable_response(
            Some("claude-work".to_string()),
            AuthProfileSubscriptionProvider::ClaudeAi,
            /*current*/ true,
            /*include_token_profile*/ false,
            AuthProfileUsageStatusReason::NotCodexBackend,
        );

        assert_eq!(
            serde_json::to_value(response).expect("serialize unavailable account"),
            serde_json::json!({
                "target": {
                    "profileName": "claude-work",
                    "subscriptionProvider": "claude-ai",
                    "authMode": null,
                    "plan": null,
                    "redactedAccountId": null
                },
                "current": true,
                "includeTokenProfile": false,
                "spendStatus": {
                    "dollarSpend": {
                        "status": "unavailable",
                        "reason": "no_backend_dollar_spend_endpoint"
                    },
                    "backendCredits": {
                        "status": "unavailable",
                        "reason": "no_backend_credit_or_spend_control_status"
                    }
                },
                "error": {
                    "reason": "not_codex_backend"
                }
            })
        );
    }

    #[tokio::test]
    async fn bounded_account_usage_collection_preserves_order_and_caps_fanout() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        use tokio::sync::Semaphore;

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));
        let future_count = MAX_CONCURRENT_ACCOUNT_USAGE_FETCHES + 2;
        let release_after_full_batch = {
            let active = Arc::clone(&active);
            let release = Arc::clone(&release);
            tokio::spawn(async move {
                while active.load(Ordering::SeqCst) < MAX_CONCURRENT_ACCOUNT_USAGE_FETCHES {
                    tokio::task::yield_now().await;
                }
                release.add_permits(future_count);
            })
        };
        let futures = (0..future_count).map(|index| {
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let release = Arc::clone(&release);
            async move {
                let current_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(current_active, Ordering::SeqCst);
                let permit = release.acquire().await.expect("release semaphore");
                permit.forget();
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(account_usage(
                    Some(&format!("profile-{index}")),
                    /*current*/ false,
                    AuthProfileUsageSummary::unavailable(AuthProfileUsageStatusReason::FetchFailed),
                ))
            }
        });

        let accounts = collect_bounded_account_usages(futures)
            .await
            .expect("collect account usage");
        release_after_full_batch
            .await
            .expect("release task should finish");

        let profile_names = accounts
            .into_iter()
            .map(|account| account.target.profile_name)
            .collect::<Vec<_>>();
        let expected_profile_names = (0..future_count)
            .map(|index| Some(format!("profile-{index}")))
            .collect::<Vec<_>>();
        assert_eq!(profile_names, expected_profile_names);
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            MAX_CONCURRENT_ACCOUNT_USAGE_FETCHES
        );
    }

    #[tokio::test]
    async fn cached_rate_limit_snapshots_reuses_fresh_entries_and_expires_stale_entries() {
        let key = AuthProfileUsageCacheKey {
            codex_home: "unit-cache-fetch-reuses-fresh-entries".to_string(),
            base_url: "https://chatgpt.com/backend-api".to_string(),
            profile: Some("work".to_string()),
            auth_mode: "chatgpt".to_string(),
            account_id: "account-123".to_string(),
        };
        AUTH_PROFILE_USAGE_CACHE.lock().await.insert(
            key.clone(),
            AuthProfileUsageCacheEntry {
                captured_at: chrono::Utc::now().timestamp(),
                snapshots: vec![snapshot(10.0, 20.0)],
            },
        );

        assert_eq!(
            cached_rate_limit_snapshots(&key, &config()).await,
            Some(vec![snapshot(10.0, 20.0)])
        );

        AUTH_PROFILE_USAGE_CACHE.lock().await.insert(
            key.clone(),
            AuthProfileUsageCacheEntry {
                captured_at: 1,
                snapshots: vec![snapshot(10.0, 20.0)],
            },
        );
        assert!(cached_rate_limit_snapshots(&key, &config()).await.is_none());
    }

    #[test]
    fn redact_identifier_does_not_return_short_ids_or_full_values() {
        assert_eq!(redact_identifier("short"), "***");
        assert_eq!(redact_identifier("account-123456"), "acco...3456");
    }
}
