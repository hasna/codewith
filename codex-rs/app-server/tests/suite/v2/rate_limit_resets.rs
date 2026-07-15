use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditParams;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditResponse;
use codex_app_server_protocol::GetAccountRateLimitsParams;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LoginAccountResponse;
use codex_app_server_protocol::RateLimitResetCreditStatus;
use codex_app_server_protocol::RateLimitResetCreditsSummary;
use codex_app_server_protocol::RateLimitResetType;
use codex_app_server_protocol::RequestId;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::AuthProfileMetadata;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const INTERNAL_ERROR_CODE: i64 = -32603;
const RESET_TIMEOUT_ENV: &str = "CODEX_TEST_ACCOUNT_RATE_LIMIT_RESET_TIMEOUT_MS";

#[tokio::test]
async fn rate_limit_read_skips_reset_details_when_summary_has_no_available_credits() -> Result<()> {
    let (codex_home, server) = rate_limit_test_server().await?;
    mount_usage_response(&server, Some(0)).await;
    Mock::given(method("GET"))
        .and(path("/api/codex/rate-limit-reset-credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "available_count": 1,
            "credits": [],
        })))
        .expect(0)
        .mount(&server)
        .await;

    let response = read_rate_limits(codex_home.path()).await?;

    assert_eq!(
        response.rate_limit_reset_credits,
        Some(RateLimitResetCreditsSummary {
            available_count: 0,
            credits: None,
        })
    );
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn rate_limit_read_can_force_reset_details_after_last_credit_is_redeemed() -> Result<()> {
    let (codex_home, server) = rate_limit_test_server().await?;
    mount_usage_response(&server, Some(0)).await;
    Mock::given(method("GET"))
        .and(path("/api/codex/rate-limit-reset-credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "available_count": 0,
            "credits": [{
                "id": "credit-1",
                "reset_type": "codex_rate_limits",
                "status": "redeemed",
                "granted_at": "2026-07-01T00:00:00Z",
                "expires_at": null,
                "title": null,
                "description": null
            }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let response = read_rate_limits_with_params(
        codex_home.path(),
        GetAccountRateLimitsParams {
            include_reset_credit_details: true,
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(
        response.rate_limit_reset_credits,
        Some(RateLimitResetCreditsSummary {
            available_count: 0,
            credits: Some(vec![codex_app_server_protocol::RateLimitResetCredit {
                id: "credit-1".to_string(),
                reset_type: RateLimitResetType::CodexRateLimits,
                status: RateLimitResetCreditStatus::Redeemed,
                granted_at: 1_782_864_000,
                expires_at: None,
                title: None,
                description: None,
            }]),
        })
    );
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn rate_limit_read_fetches_reset_details_when_summary_has_available_credits() -> Result<()> {
    let (codex_home, server) = rate_limit_test_server().await?;
    mount_usage_response(&server, Some(2)).await;
    Mock::given(method("GET"))
        .and(path("/api/codex/rate-limit-reset-credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "available_count": 2,
            "credits": [
                {
                    "id": "credit-1",
                    "reset_type": "codex_rate_limits",
                    "status": "available",
                    "granted_at": "2026-07-01T00:00:00Z",
                    "expires_at": "2026-08-01T00:00:00Z",
                    "title": "Weekly reset",
                    "description": null
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let response = read_rate_limits(codex_home.path()).await?;

    let summary = response
        .rate_limit_reset_credits
        .expect("reset details should be present");
    assert_eq!(summary.available_count, 2);
    let credits = summary.credits.expect("detail rows should be present");
    assert_eq!(credits.len(), 1);
    assert_eq!(credits[0].id, "credit-1");
    assert_eq!(credits[0].reset_type, RateLimitResetType::CodexRateLimits);
    assert_eq!(credits[0].status, RateLimitResetCreditStatus::Available);
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn rate_limit_read_preserves_count_only_summary_when_reset_details_fail() -> Result<()> {
    let (codex_home, server) = rate_limit_test_server().await?;
    mount_usage_response(&server, Some(3)).await;
    Mock::given(method("GET"))
        .and(path("/api/codex/rate-limit-reset-credits"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .expect(1)
        .mount(&server)
        .await;

    let response = read_rate_limits(codex_home.path()).await?;

    assert_eq!(
        response.rate_limit_reset_credits,
        Some(RateLimitResetCreditsSummary {
            available_count: 3,
            credits: None,
        })
    );
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn consume_rate_limit_reset_credit_requires_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("redeem-123"))
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "codex account authentication required to reset usage limits"
    );

    Ok(())
}

#[tokio::test]
async fn consume_rate_limit_reset_credit_requires_chatgpt_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("redeem-123"))
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "chatgpt authentication required to reset usage limits"
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_without_account_id_never_posts() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token").plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;
    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("redeem-123"))
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "chatgpt account identity required to reset usage limits"
    );
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn consume_rate_limit_reset_credit_rejects_empty_inputs() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params(" "))
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "idempotencyKey is required to reset usage limits"
    );

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            consume_reset_params("redeem-123").with_credit_id(" "),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "creditId must be non-empty when provided"
    );

    Ok(())
}

#[tokio::test]
async fn consume_rate_limit_reset_credit_rejects_invalid_auth_profile_name() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            consume_reset_params("redeem-123").with_auth_profile(Some("bad/profile")),
        )
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "invalid auth profile: invalid auth profile name `bad/profile`; use letters, numbers, dots, dashes, or underscores, and start with a letter or number"
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_rejects_changed_account_without_backend_post() -> Result<()>
{
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("current-token")
            .account_id("current-account")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            consume_reset_params("redeem-123").with_expected_account("previous-account"),
        )
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: ConsumeAccountRateLimitResetCreditResponse = to_response(response)?;
    assert_opaque_account_identity(&received.account_identity_fingerprint, "current-account");
    let account_identity_fingerprint = received.account_identity_fingerprint.clone();

    assert_eq!(
        received,
        ConsumeAccountRateLimitResetCreditResponse {
            outcome: ConsumeAccountRateLimitResetCreditOutcome::AccountChanged,
            account_identity_fingerprint,
        }
    );
    server.verify().await;

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_uses_named_auth_profile_and_selected_credit_id()
-> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("root-token")
            .account_id("root-account")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;
    codex_login::save_auth_profile_metadata(
        codex_home.path(),
        "work",
        AuthProfileMetadata::default(),
    )?;
    let work_profile_home = codex_login::auth_profile_storage_dir(codex_home.path(), "work")?;
    write_chatgpt_auth(
        &work_profile_home,
        ChatGptAuthFixture::new("work-token")
            .account_id("work-account")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    mount_usage_response(&server, None).await;

    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .and(header("authorization", "Bearer work-token"))
        .and(header("chatgpt-account-id", "work-account"))
        .and(wiremock::matchers::body_json(json!({
            "redeem_request_id": "redeem-123",
            "credit_id": "credit-123",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "reset",
            "windows_reset": 2,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_rate_limits_request_with_params(GetAccountRateLimitsParams {
            auth_profile: Some(Some("work".to_string())),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let rate_limits: GetAccountRateLimitsResponse = to_response(response)?;
    let account_identity_fingerprint = rate_limits
        .account_identity_fingerprint
        .expect("account identity fingerprint should be present");
    assert_opaque_account_identity(&account_identity_fingerprint, "work-account");

    let mut params = consume_reset_params("redeem-123")
        .with_credit_id("credit-123")
        .with_auth_profile(Some("work"));
    params.expected_account_identity_fingerprint = Some(account_identity_fingerprint);

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(params)
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: ConsumeAccountRateLimitResetCreditResponse = to_response(response)?;

    assert_eq!(
        received.outcome,
        ConsumeAccountRateLimitResetCreditOutcome::Reset
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_reads_root_auth_profile_when_selected_profile_differs()
-> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("root-token")
            .account_id("root-account")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;
    codex_login::save_auth_profile_metadata(
        codex_home.path(),
        "work",
        AuthProfileMetadata::default(),
    )?;
    let work_profile_home = codex_login::auth_profile_storage_dir(codex_home.path(), "work")?;
    write_chatgpt_auth(
        &work_profile_home,
        ChatGptAuthFixture::new("work-token")
            .account_id("work-account")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;

    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .and(header("authorization", "Bearer root-token"))
        .and(header("chatgpt-account-id", "root-account"))
        .and(wiremock::matchers::body_json(json!({
            "redeem_request_id": "root-redeem",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "reset",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut mcp = TestAppServer::new_with_env(
        codex_home.path(),
        &[
            ("CODEWITH_AUTH_PROFILE", Some("work")),
            ("OPENAI_API_KEY", None),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            consume_reset_params("root-redeem").with_auth_profile(None),
        )
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: ConsumeAccountRateLimitResetCreditResponse = to_response(response)?;

    assert_eq!(
        received.outcome,
        ConsumeAccountRateLimitResetCreditOutcome::Reset
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_maps_no_credit_outcome() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "no_credit",
        })))
        .mount(&server)
        .await;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("redeem-123"))
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: ConsumeAccountRateLimitResetCreditResponse = to_response(response)?;

    assert_eq!(
        received.outcome,
        ConsumeAccountRateLimitResetCreditOutcome::NoCredit
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_surfaces_backend_failure() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let mut mcp = test_app_server(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("redeem-123"))
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error.error.message.contains("failed to reset usage limits"),
        "unexpected error message: {}",
        error.error.message
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn consume_rate_limit_reset_credit_timeout_releases_later_request() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .and(wiremock::matchers::body_json(json!({
            "redeem_request_id": "slow-redeem",
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(300))
                .set_body_json(json!({ "code": "reset" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/codex/rate-limit-reset-credits/consume"))
        .and(wiremock::matchers::body_json(json!({
            "redeem_request_id": "fast-redeem",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "already_redeemed",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut mcp = TestAppServer::new_with_env(
        codex_home.path(),
        &[
            ("CODEWITH_AUTH_PROFILE", None),
            ("CODEX_AUTH_PROFILE", None),
            ("OPENAI_API_KEY", None),
            (RESET_TIMEOUT_ENV, Some("50")),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("slow-redeem"))
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert_eq!(error.error.message, "usage limit reset request timed out");

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(consume_reset_params("fast-redeem"))
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: ConsumeAccountRateLimitResetCreditResponse = to_response(response)?;
    assert_eq!(
        received.outcome,
        ConsumeAccountRateLimitResetCreditOutcome::AlreadyRedeemed
    );

    Ok(())
}

fn consume_reset_params(idempotency_key: &str) -> ConsumeAccountRateLimitResetCreditParams {
    ConsumeAccountRateLimitResetCreditParams {
        idempotency_key: idempotency_key.to_string(),
        credit_id: None,
        auth_profile: None,
        expected_account_identity_fingerprint: None,
    }
}

fn assert_opaque_account_identity(fingerprint: &str, raw_account_id: &str) {
    assert!(fingerprint.starts_with("opaque:"));
    assert_eq!(fingerprint.len(), "opaque:".len() + 64);
    assert!(!fingerprint.contains(raw_account_id));
}

trait ConsumeResetParamsExt {
    fn with_credit_id(self, credit_id: &str) -> Self;
    fn with_auth_profile(self, profile: Option<&str>) -> Self;
    fn with_expected_account(self, account_id: &str) -> Self;
}

impl ConsumeResetParamsExt for ConsumeAccountRateLimitResetCreditParams {
    fn with_credit_id(mut self, credit_id: &str) -> Self {
        self.credit_id = Some(credit_id.to_string());
        self
    }

    fn with_auth_profile(mut self, profile: Option<&str>) -> Self {
        self.auth_profile = Some(profile.map(str::to_string));
        self
    }

    fn with_expected_account(mut self, account_id: &str) -> Self {
        self.expected_account_identity_fingerprint =
            Some(codex_login::account_identity_fingerprint(account_id));
        self
    }
}

async fn login_with_api_key(mcp: &mut TestAppServer, api_key: &str) -> Result<()> {
    let request_id = mcp.send_login_account_api_key_request(api_key).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let login: LoginAccountResponse = to_response(response)?;
    assert_eq!(login, LoginAccountResponse::ApiKey {});

    Ok(())
}

async fn test_app_server(codex_home: &Path) -> Result<TestAppServer> {
    TestAppServer::new_with_env(
        codex_home,
        &[
            ("CODEWITH_AUTH_PROFILE", None),
            ("CODEX_AUTH_PROFILE", None),
            ("OPENAI_API_KEY", None),
        ],
    )
    .await
}

async fn rate_limit_test_server() -> Result<(TempDir, MockServer)> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;
    let server = MockServer::start().await;
    write_chatgpt_base_url(codex_home.path(), &server.uri())?;
    Ok((codex_home, server))
}

async fn mount_usage_response(server: &MockServer, available_count: Option<i64>) {
    let mut response = json!({
        "plan_type": "pro",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
            "primary_window": {
                "used_percent": 100,
                "limit_window_seconds": 604800,
                "reset_after_seconds": 3600,
                "reset_at": 1783987200
            }
        }
    });
    if let Some(available_count) = available_count {
        response["rate_limit_reset_credits"] = json!({
            "available_count": available_count,
        });
    }
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(server)
        .await;
}

async fn read_rate_limits(codex_home: &Path) -> Result<GetAccountRateLimitsResponse> {
    read_rate_limits_with_params(codex_home, GetAccountRateLimitsParams::default()).await
}

async fn read_rate_limits_with_params(
    codex_home: &Path,
    params: GetAccountRateLimitsParams,
) -> Result<GetAccountRateLimitsResponse> {
    let mut mcp = TestAppServer::new_with_env(
        codex_home,
        &[
            ("CODEWITH_AUTH_PROFILE", None),
            ("CODEX_AUTH_PROFILE", None),
            ("OPENAI_API_KEY", None),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let request_id = mcp
        .send_get_account_rate_limits_request_with_params(params)
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(response)
}

fn write_chatgpt_base_url(codex_home: &Path, base_url: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!("chatgpt_base_url = \"{base_url}\"\ncli_auth_credentials_store = \"file\"\n"),
    )
}
