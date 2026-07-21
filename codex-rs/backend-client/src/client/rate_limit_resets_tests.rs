use super::*;
use crate::types::ConsumeRateLimitResetCreditCode;
use crate::types::RateLimitResetCreditsDetails;
use crate::types::RateLimitResetCreditsSummary;
use pretty_assertions::assert_eq;

#[test]
fn rate_limit_reset_contract_uses_expected_paths_and_payloads() {
    assert_eq!(
        test_client("https://example.test", PathStyle::CodexApi).rate_limit_status_url(),
        "https://example.test/api/codex/usage"
    );
    assert_eq!(
        test_client("https://example.test", PathStyle::CodexApi)
            .consume_rate_limit_reset_credit_url(),
        "https://example.test/api/codex/rate-limit-reset-credits/consume"
    );
    assert_eq!(
        test_client("https://example.test", PathStyle::CodexApi).rate_limit_reset_credits_url(),
        "https://example.test/api/codex/rate-limit-reset-credits"
    );
    assert_eq!(
        test_client("https://chatgpt.com/backend-api", PathStyle::ChatGptApi)
            .rate_limit_status_url(),
        "https://chatgpt.com/backend-api/wham/usage"
    );
    assert_eq!(
        test_client("https://chatgpt.com/backend-api", PathStyle::ChatGptApi)
            .consume_rate_limit_reset_credit_url(),
        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume"
    );

    assert_eq!(
        serde_json::to_value(ConsumeRateLimitResetCreditRequest {
            redeem_request_id: "redeem-123",
            credit_id: None,
        })
        .unwrap(),
        serde_json::json!({ "redeem_request_id": "redeem-123" })
    );
    assert_eq!(
        serde_json::to_value(ConsumeRateLimitResetCreditRequest {
            redeem_request_id: "redeem-456",
            credit_id: Some("credit-123"),
        })
        .unwrap(),
        serde_json::json!({
            "redeem_request_id": "redeem-456",
            "credit_id": "credit-123",
        })
    );

    let status: RateLimitStatusWithResetCredits = serde_json::from_value(serde_json::json!({
        "plan_type": "plus",
        "rate_limit_reset_credits": { "available_count": 3 }
    }))
    .unwrap();
    assert_eq!(
        status.rate_limit_reset_credits,
        Some(RateLimitResetCreditsSummary { available_count: 3 })
    );

    let details: RateLimitResetCreditsDetails = serde_json::from_value(serde_json::json!({
        "available_count": 2,
        "credits": [
            {
                "id": "credit-later",
                "reset_type": "codex_rate_limits",
                "status": "available",
                "granted_at": "2026-07-01T00:00:00Z",
                "expires_at": "2026-08-01T00:00:00Z",
                "title": "Weekly reset",
                "description": null
            },
            {
                "id": "credit-earlier",
                "reset_type": "codex_rate_limits",
                "status": "available",
                "granted_at": "2026-07-02T00:00:00Z",
                "expires_at": "2026-07-20T00:00:00Z",
                "title": null,
                "description": null
            }
        ]
    }))
    .unwrap();
    assert_eq!(details.available_count, 2);
    assert_eq!(details.credits[1].id, "credit-earlier");

    let response: ConsumeRateLimitResetCreditResponse = serde_json::from_value(serde_json::json!({
        "code": "reset",
        "windows_reset": 2
    }))
    .unwrap();
    assert_eq!(response.code, ConsumeRateLimitResetCreditCode::Reset);
    assert_eq!(response.windows_reset, Some(2));
}

fn test_client(base_url: &str, path_style: PathStyle) -> Client {
    Client {
        base_url: base_url.to_string(),
        http: reqwest::Client::new(),
        auth_provider: codex_model_provider::unauthenticated_auth_provider(),
        user_agent: None,
        chatgpt_account_id: None,
        chatgpt_account_is_fedramp: false,
        path_style,
    }
}
