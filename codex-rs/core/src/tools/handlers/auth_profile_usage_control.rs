use crate::auth_profile_usage::AuthProfileUsageHealth;
use crate::auth_profile_usage::TokenUsageProfileResponse;
use crate::auth_profile_usage::usage_capture_is_stale;
use crate::auth_profile_usage::usage_health_for_snapshots;
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
use codex_login::CodexAuth;
use codex_protocol::account::PlanType as AccountPlanType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::TokenUsageInfo;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::timeout;

const AUTH_PROFILE_USAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
static AUTH_PROFILE_USAGE_CACHE: LazyLock<
    Mutex<BTreeMap<AuthProfileUsageCacheKey, AuthProfileUsageCacheEntry>>,
> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub struct GetUsageHandler;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GetUsageScope {
    Session,
    Account,
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
struct AccountUsageTarget {
    profile_name: Option<String>,
    auth_mode: Option<AuthMode>,
    plan: Option<String>,
    redacted_account_id: Option<String>,
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
        let target_profile = normalize_requested_profile(
            args.auth_profile.clone(),
            session.selected_auth_profile().await,
        )?;
        let response = get_usage_response(&session, &turn, args, target_profile).await?;
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
    target_profile: Option<String>,
) -> Result<GetUsageResponse, FunctionCallError> {
    let include_session = matches!(args.scope, GetUsageScope::Session | GetUsageScope::Both);
    let include_account = matches!(args.scope, GetUsageScope::Account | GetUsageScope::Both);
    let session_usage = if include_session {
        Some(SessionUsageResponse {
            token_usage: session.token_usage_info().await,
            spend_status: UsageSpendSummary::session(),
        })
    } else {
        None
    };
    let account_usage = if include_account {
        Some(fetch_account_usage(session, turn, target_profile, args.include_token_profile).await?)
    } else {
        None
    };
    Ok(GetUsageResponse {
        scope: args.scope,
        session: session_usage,
        account: account_usage,
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
    include_token_profile: bool,
) -> Result<AccountUsageResponse, FunctionCallError> {
    validate_target_profile(turn, target_profile.as_deref())?;
    let scoped_auth_manager = session
        .services
        .auth_manager
        .shared_scoped_auth_profile(target_profile.clone())
        .await;
    let Some(auth) = scoped_auth_manager.auth().await else {
        return Ok(account_unavailable_response(
            target_profile,
            include_token_profile,
            AuthProfileUsageStatusReason::NoAuth,
        ));
    };
    let target = AccountUsageTarget::from_auth(target_profile.clone(), &auth);
    if !auth.uses_codex_backend() {
        return Ok(AccountUsageResponse {
            target,
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
        include_token_profile,
        spend_status,
        rate_limits,
        token_profile,
        token_profile_error,
        error,
    })
}

fn validate_target_profile(
    turn: &TurnContext,
    target_profile: Option<&str>,
) -> Result<(), FunctionCallError> {
    let Some(profile_name) = target_profile else {
        return Ok(());
    };
    let profiles = codex_login::list_auth_profiles(
        &turn.config.codex_home,
        turn.config.cli_auth_credentials_store_mode,
    )
    .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    if profiles
        .iter()
        .any(|profile| profile.name.as_str() == profile_name)
    {
        Ok(())
    } else {
        Err(FunctionCallError::RespondToModel(format!(
            "unknown auth profile `{profile_name}`"
        )))
    }
}

fn account_unavailable_response(
    target_profile: Option<String>,
    include_token_profile: bool,
    reason: AuthProfileUsageStatusReason,
) -> AccountUsageResponse {
    AccountUsageResponse {
        target: AccountUsageTarget {
            profile_name: target_profile,
            auth_mode: None,
            plan: None,
            redacted_account_id: None,
        },
        include_token_profile,
        spend_status: UsageSpendSummary::account_without_backend_credits(),
        rate_limits: None,
        token_profile: None,
        token_profile_error: None,
        error: Some(AccountUsageError { reason }),
    }
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
    fn from_auth(profile_name: Option<String>, auth: &CodexAuth) -> Self {
        Self {
            profile_name,
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
