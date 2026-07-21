//! Validation, backend I/O, and wire mapping for usage-limit reset credits.

use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use chrono::DateTime;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RateLimitResetCredit;
use codex_app_server_protocol::RateLimitResetCreditStatus;
use codex_app_server_protocol::RateLimitResetCreditsSummary;
use codex_app_server_protocol::RateLimitResetType;
use codex_backend_client::Client as BackendClient;
use codex_backend_client::ConsumeRateLimitResetCreditCode;
use codex_backend_client::RateLimitResetCreditDetails as BackendRateLimitResetCreditDetails;
use codex_backend_client::RateLimitResetCreditsDetails as BackendRateLimitResetCreditsDetails;
use codex_backend_client::RateLimitResetCreditsSummary as BackendRateLimitResetCreditsSummary;
use codex_model_provider_info::provider_base_url_is_loopback;
use codex_model_provider_info::provider_base_url_matches;
use tokio::time::Duration;

const CHATGPT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api";
const ACCOUNT_RATE_LIMIT_RESET_CONSUME_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 10);
const ACCOUNT_RATE_LIMIT_RESET_DETAILS_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 5);
const ACCOUNT_RATE_LIMIT_RESET_CONSUME_TIMEOUT_MS_ENV_VAR: &str =
    "CODEX_TEST_ACCOUNT_RATE_LIMIT_RESET_TIMEOUT_MS";
pub(super) fn ensure_canonical_backend(
    base_url: &str,
    allow_loopback_for_tests: bool,
) -> Result<(), JSONRPCErrorError> {
    if backend_supports_usage_resets(base_url, allow_loopback_for_tests) {
        return Ok(());
    }
    Err(invalid_request(
        "usage limit resets require the canonical ChatGPT backend",
    ))
}

pub(super) fn backend_supports_usage_resets(
    base_url: &str,
    allow_loopback_for_tests: bool,
) -> bool {
    provider_base_url_matches(base_url, CHATGPT_BACKEND_BASE_URL)
        || (cfg!(debug_assertions)
            && allow_loopback_for_tests
            && provider_base_url_is_loopback(base_url))
}

pub(super) fn validated_idempotency_key(value: &str) -> Result<&str, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request(
            "idempotencyKey is required to reset usage limits",
        ));
    }
    Ok(value)
}

pub(super) fn validated_credit_id(value: Option<&str>) -> Result<Option<&str>, JSONRPCErrorError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request("creditId must be non-empty when provided"));
    }
    Ok(Some(value))
}

pub(super) async fn consume_credit(
    client: &BackendClient,
    idempotency_key: &str,
    credit_id: Option<&str>,
) -> Result<ConsumeAccountRateLimitResetCreditOutcome, JSONRPCErrorError> {
    let response = tokio::time::timeout(
        consume_timeout(),
        client.consume_rate_limit_reset_credit(idempotency_key, credit_id),
    )
    .await
    .map_err(|_| internal_error("usage limit reset request timed out"))?
    .map_err(|err| internal_error(format!("failed to reset usage limits: {err}")))?;

    Ok(outcome_from_backend(response.code))
}

pub(super) async fn enrich_summary(
    client: &BackendClient,
    summary: Option<BackendRateLimitResetCreditsSummary>,
) -> Option<RateLimitResetCreditsSummary> {
    // Banked usage-limit resets live in the dedicated reset-credit ledger
    // (`/rate-limit-reset-credits`), which is the source of truth for how many
    // resets are available. The inline `available_count` returned by the usage
    // endpoint can lag that ledger and report 0 while banked resets still exist,
    // so we must always reconcile against the authoritative ledger for accounts
    // that reach this path (only the canonical ChatGPT backend does). Trusting
    // the inline count is exactly what made `/usage` show "no usage limit resets
    // available" while banked resets were still redeemable.
    //
    // Fall back to the count-only summary only when the ledger fetch fails, so a
    // transient ledger error still surfaces the coarse count instead of hiding
    // resets entirely.
    if let Some(details) = detailed_credits(client).await {
        return Some(details);
    }

    summary.map(summary_from_backend)
}

fn outcome_from_backend(
    code: ConsumeRateLimitResetCreditCode,
) -> ConsumeAccountRateLimitResetCreditOutcome {
    match code {
        ConsumeRateLimitResetCreditCode::Reset => ConsumeAccountRateLimitResetCreditOutcome::Reset,
        ConsumeRateLimitResetCreditCode::NothingToReset => {
            ConsumeAccountRateLimitResetCreditOutcome::NothingToReset
        }
        ConsumeRateLimitResetCreditCode::NoCredit => {
            ConsumeAccountRateLimitResetCreditOutcome::NoCredit
        }
        ConsumeRateLimitResetCreditCode::AlreadyRedeemed => {
            ConsumeAccountRateLimitResetCreditOutcome::AlreadyRedeemed
        }
        ConsumeRateLimitResetCreditCode::Unknown => {
            ConsumeAccountRateLimitResetCreditOutcome::Unknown
        }
    }
}

fn summary_from_backend(
    summary: BackendRateLimitResetCreditsSummary,
) -> RateLimitResetCreditsSummary {
    RateLimitResetCreditsSummary {
        available_count: summary.available_count,
        credits: None,
    }
}

async fn detailed_credits(client: &BackendClient) -> Option<RateLimitResetCreditsSummary> {
    let details = match tokio::time::timeout(
        ACCOUNT_RATE_LIMIT_RESET_DETAILS_TIMEOUT,
        client.list_rate_limit_reset_credits(),
    )
    .await
    {
        Ok(Ok(details)) => details,
        Ok(Err(err)) => {
            tracing::warn!(
                "failed to fetch usage-limit reset details; falling back to the usage count: {err}"
            );
            return None;
        }
        Err(_) => {
            tracing::warn!(
                "usage-limit reset detail request timed out; falling back to the usage count"
            );
            return None;
        }
    };

    match details_from_backend(details) {
        Ok(summary) => Some(summary),
        Err(err) => {
            tracing::warn!(
                "failed to parse usage-limit reset details; falling back to the usage count: {err}"
            );
            None
        }
    }
}

fn details_from_backend(
    details: BackendRateLimitResetCreditsDetails,
) -> Result<RateLimitResetCreditsSummary, String> {
    let credits = details
        .credits
        .into_iter()
        .map(credit_from_backend)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RateLimitResetCreditsSummary {
        available_count: details.available_count,
        credits: Some(credits),
    })
}

fn credit_from_backend(
    credit: BackendRateLimitResetCreditDetails,
) -> Result<RateLimitResetCredit, String> {
    let reset_type = match credit.reset_type.as_str() {
        "codex_rate_limits" => RateLimitResetType::CodexRateLimits,
        _ => RateLimitResetType::Unknown,
    };
    let status = match credit.status.as_str() {
        "available" => RateLimitResetCreditStatus::Available,
        "redeeming" => RateLimitResetCreditStatus::Redeeming,
        "redeemed" => RateLimitResetCreditStatus::Redeemed,
        _ => RateLimitResetCreditStatus::Unknown,
    };
    let granted_at = timestamp(&credit.granted_at)
        .map_err(|err| format!("invalid granted_at for credit `{}`: {err}", credit.id))?;
    let expires_at = credit
        .expires_at
        .as_deref()
        .map(timestamp)
        .transpose()
        .map_err(|err| format!("invalid expires_at for credit `{}`: {err}", credit.id))?;

    Ok(RateLimitResetCredit {
        id: credit.id,
        reset_type,
        status,
        granted_at,
        expires_at,
        title: credit.title,
        description: credit.description,
    })
}

fn timestamp(timestamp: &str) -> Result<i64, String> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|timestamp| timestamp.timestamp())
        .map_err(|err| format!("failed to parse timestamp `{timestamp}`: {err}"))
}

fn consume_timeout() -> Duration {
    if let Ok(value) = std::env::var(ACCOUNT_RATE_LIMIT_RESET_CONSUME_TIMEOUT_MS_ENV_VAR)
        && let Ok(ms) = value.parse::<u64>()
        && ms > 0
    {
        return Duration::from_millis(ms);
    }
    ACCOUNT_RATE_LIMIT_RESET_CONSUME_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn maps_unknown_backend_values_without_claiming_the_credit_is_usable() {
        let credit = credit_from_backend(BackendRateLimitResetCreditDetails {
            id: "credit-1".to_string(),
            reset_type: "future_reset_type".to_string(),
            status: "future_status".to_string(),
            granted_at: "2026-07-01T00:00:00Z".to_string(),
            expires_at: None,
            title: None,
            description: None,
        })
        .expect("timestamps are valid");

        assert_eq!(credit.reset_type, RateLimitResetType::Unknown);
        assert_eq!(credit.status, RateLimitResetCreditStatus::Unknown);
    }

    #[test]
    fn rejects_empty_reset_identifiers() {
        assert!(validated_idempotency_key(" ").is_err());
        assert!(validated_credit_id(Some(" ")).is_err());
    }

    #[test]
    fn accepts_only_the_canonical_backend_without_the_test_escape() {
        assert!(
            ensure_canonical_backend(
                "https://chatgpt.com/backend-api/",
                /*allow_loopback_for_tests*/ false,
            )
            .is_ok()
        );
        assert!(
            ensure_canonical_backend(
                "https://example.test/backend-api",
                /*allow_loopback_for_tests*/ false,
            )
            .is_err()
        );
    }
}
