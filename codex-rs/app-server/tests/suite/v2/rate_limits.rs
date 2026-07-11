use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::AddCreditsNudgeCreditType;
use codex_app_server_protocol::AddCreditsNudgeEmailStatus;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditParams;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditResponse;
use codex_app_server_protocol::GetAccountRateLimitsParams;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LoginAccountResponse;
use codex_app_server_protocol::RateLimitReachedType;
use codex_app_server_protocol::RateLimitSnapshot;
use codex_app_server_protocol::RateLimitWindow;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SendAddCreditsNudgeEmailParams;
use codex_app_server_protocol::SendAddCreditsNudgeEmailResponse;
use codex_app_server_protocol::SpendControlLimitSnapshot;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::AuthProfileMetadata;
use codex_protocol::account::PlanType as AccountPlanType;
use pretty_assertions::assert_eq;
use serial_test::serial;
use serde_json::json;
use std::ffi::OsString;
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
async fn get_account_rate_limits_requires_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "codex account authentication required to read rate limits"
    );

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_requires_chatgpt_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key(&mut mcp, "sk-test-key").await?;

    let request_id = mcp.send_get_account_rate_limits_request().await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "chatgpt authentication required to read rate limits"
    );

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_returns_snapshot() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    let server_url = server.uri();
    write_chatgpt_base_url(codex_home.path(), &server_url)?;

    let primary_reset_timestamp = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:02:00Z")
        .expect("parse primary reset timestamp")
        .timestamp();
    let secondary_reset_timestamp = chrono::DateTime::parse_from_rfc3339("2025-01-01T01:00:00Z")
        .expect("parse secondary reset timestamp")
        .timestamp();
    let response_body = json!({
        "plan_type": "pro",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
            "primary_window": {
                "used_percent": 42,
                "limit_window_seconds": 3600,
                "reset_after_seconds": 120,
                "reset_at": primary_reset_timestamp,
            },
            "secondary_window": {
                "used_percent": 5,
                "limit_window_seconds": 86400,
                "reset_after_seconds": 43200,
                "reset_at": secondary_reset_timestamp,
            }
        },
        "rate_limit_reached_type": {
            "type": "workspace_member_usage_limit_reached",
        },
        "spend_control": {
            "reached": false,
            "individual_limit": {
                "source": "workspace_spend_controls",
                "limit": "25000",
                "used": "8000",
                "remaining": "17000",
                "used_percent": 32,
                "remaining_percent": 68,
                "reset_after_seconds": 43200,
                "reset_at": secondary_reset_timestamp,
            }
        },
        "additional_rate_limits": [
            {
                "limit_name": "codex_other",
                "metered_feature": "codex_other",
                "rate_limit": {
                    "allowed": true,
                    "limit_reached": false,
                    "primary_window": {
                        "used_percent": 88,
                        "limit_window_seconds": 1800,
                        "reset_after_seconds": 600,
                        "reset_at": 1735693200
                    }
                }
            }
        ]
    });

    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let received: GetAccountRateLimitsResponse = to_response(response)?;

    let expected = GetAccountRateLimitsResponse {
        rate_limits: RateLimitSnapshot {
            limit_id: Some("codex".to_string()),
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 42,
                window_duration_mins: Some(60),
                resets_at: Some(primary_reset_timestamp),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: 5,
                window_duration_mins: Some(1440),
                resets_at: Some(secondary_reset_timestamp),
            }),
            credits: None,
            individual_limit: Some(SpendControlLimitSnapshot {
                limit: "25000".to_string(),
                used: "8000".to_string(),
                remaining_percent: 68,
                resets_at: secondary_reset_timestamp,
            }),
            plan_type: Some(AccountPlanType::Pro),
            rate_limit_reached_type: Some(RateLimitReachedType::WorkspaceMemberUsageLimitReached),
        },
        rate_limits_by_limit_id: Some(
            [
                (
                    "codex".to_string(),
                    RateLimitSnapshot {
                        limit_id: Some("codex".to_string()),
                        limit_name: None,
                        primary: Some(RateLimitWindow {
                            used_percent: 42,
                            window_duration_mins: Some(60),
                            resets_at: Some(primary_reset_timestamp),
                        }),
                        secondary: Some(RateLimitWindow {
                            used_percent: 5,
                            window_duration_mins: Some(1440),
                            resets_at: Some(secondary_reset_timestamp),
                        }),
                        credits: None,
                        individual_limit: Some(SpendControlLimitSnapshot {
                            limit: "25000".to_string(),
                            used: "8000".to_string(),
                            remaining_percent: 68,
                            resets_at: secondary_reset_timestamp,
                        }),
                        plan_type: Some(AccountPlanType::Pro),
                        rate_limit_reached_type: Some(
                            RateLimitReachedType::WorkspaceMemberUsageLimitReached,
                        ),
                    },
                ),
                (
                    "codex_other".to_string(),
                    RateLimitSnapshot {
                        limit_id: Some("codex_other".to_string()),
                        limit_name: Some("codex_other".to_string()),
                        primary: Some(RateLimitWindow {
                            used_percent: 88,
                            window_duration_mins: Some(30),
                            resets_at: Some(1735693200),
                        }),
                        secondary: None,
                        credits: None,
                        individual_limit: None,
                        plan_type: Some(AccountPlanType::Pro),
                        rate_limit_reached_type: None,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        rate_limit_reset_credits: None,
    };
    assert_eq!(received, expected);

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_reads_named_auth_profile() -> Result<()> {
    let codex_home = TempDir::new()?;
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
    let server_url = server.uri();
    write_chatgpt_base_url(codex_home.path(), &server_url)?;

    let primary_reset_timestamp = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:02:00Z")
        .expect("parse primary reset timestamp")
        .timestamp();
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .and(header("authorization", "Bearer work-token"))
        .and(header("chatgpt-account-id", "work-account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 9,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 120,
                    "reset_at": primary_reset_timestamp,
                },
            },
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_rate_limits_request_with_params(GetAccountRateLimitsParams {
            auth_profile: Some(Some("work".to_string())),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountRateLimitsResponse = to_response(response)?;
    let expected_snapshot = RateLimitSnapshot {
        limit_id: Some("codex".to_string()),
        limit_name: None,
        primary: Some(RateLimitWindow {
            used_percent: 9,
            window_duration_mins: Some(10080),
            resets_at: Some(primary_reset_timestamp),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        plan_type: Some(AccountPlanType::Pro),
        rate_limit_reached_type: None,
    };
    let expected = GetAccountRateLimitsResponse {
        rate_limits: expected_snapshot.clone(),
        rate_limits_by_limit_id: Some(
            [("codex".to_string(), expected_snapshot)]
                .into_iter()
                .collect(),
        ),
        rate_limit_reset_credits: None,
    };
    assert_eq!(received, expected);

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_reads_root_auth_profile_when_selected_profile_differs()
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
    let server_url = server.uri();
    write_chatgpt_base_url(codex_home.path(), &server_url)?;

    let primary_reset_timestamp = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:02:00Z")
        .expect("parse primary reset timestamp")
        .timestamp();
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .and(header("authorization", "Bearer root-token"))
        .and(header("chatgpt-account-id", "root-account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 17,
                    "limit_window_seconds": 3600,
                    "reset_after_seconds": 120,
                    "reset_at": primary_reset_timestamp,
                },
            },
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
        .send_get_account_rate_limits_request_with_params(GetAccountRateLimitsParams {
            auth_profile: Some(None),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountRateLimitsResponse = to_response(response)?;
    let expected_snapshot = RateLimitSnapshot {
        limit_id: Some("codex".to_string()),
        limit_name: None,
        primary: Some(RateLimitWindow {
            used_percent: 17,
            window_duration_mins: Some(60),
            resets_at: Some(primary_reset_timestamp),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        plan_type: Some(AccountPlanType::Pro),
        rate_limit_reached_type: None,
    };
    let expected = GetAccountRateLimitsResponse {
        rate_limits: expected_snapshot.clone(),
        rate_limits_by_limit_id: Some(
            [("codex".to_string(), expected_snapshot)]
                .into_iter()
                .collect(),
        ),
        rate_limit_reset_credits: None,
    };
    assert_eq!(received, expected);

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_rejects_invalid_auth_profile_name() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_rate_limits_request_with_params(GetAccountRateLimitsParams {
            auth_profile: Some(Some("bad/profile".to_string())),
        })
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

#[tokio::test]
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_requires_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
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
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_requires_chatgpt_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
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

#[tokio::test]
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_rejects_empty_inputs() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
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
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_rejects_invalid_auth_profile_name() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
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
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_uses_named_auth_profile_and_selected_credit_id() -> Result<()> {
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

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            consume_reset_params("redeem-123")
                .with_credit_id("credit-123")
                .with_auth_profile(Some("work")),
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
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_reads_root_auth_profile_when_selected_profile_differs() -> Result<()> {
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
#[serial(rate_limit_reset_timeout_env)]
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

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
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
#[serial(rate_limit_reset_timeout_env)]
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

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
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
        error
            .error
            .message
            .contains("failed to reset usage limits"),
        "unexpected error message: {}",
        error.error.message
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
#[serial(rate_limit_reset_timeout_env)]
async fn consume_rate_limit_reset_credit_timeout_releases_later_request() -> Result<()> {
    let _timeout_guard = EnvVarGuard::set(RESET_TIMEOUT_ENV, "50");
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

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
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

#[tokio::test]
async fn send_add_credits_nudge_email_requires_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::Credits,
        })
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
        "codex account authentication required to notify workspace owner"
    );

    Ok(())
}

#[tokio::test]
async fn send_add_credits_nudge_email_requires_chatgpt_auth() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::UsageLimit,
        })
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
        "chatgpt authentication required to notify workspace owner"
    );

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn send_add_credits_nudge_email_posts_expected_body() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    let server_url = server.uri();
    write_chatgpt_base_url(codex_home.path(), &server_url)?;

    Mock::given(method("POST"))
        .and(path("/api/codex/accounts/send_add_credits_nudge_email"))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .and(wiremock::matchers::body_json(json!({
            "credit_type": "usage_limit",
        })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::UsageLimit,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: SendAddCreditsNudgeEmailResponse = to_response(response)?;

    assert_eq!(received.status, AddCreditsNudgeEmailStatus::Sent);

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn send_add_credits_nudge_email_maps_cooldown() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    let server_url = server.uri();
    write_chatgpt_base_url(codex_home.path(), &server_url)?;

    Mock::given(method("POST"))
        .and(path("/api/codex/accounts/send_add_credits_nudge_email"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::Credits,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: SendAddCreditsNudgeEmailResponse = to_response(response)?;

    assert_eq!(received.status, AddCreditsNudgeEmailStatus::CooldownActive);

    Ok(())
}

#[cfg_attr(target_os = "windows", ignore = "covered by Linux and macOS CI")]
#[tokio::test]
async fn send_add_credits_nudge_email_surfaces_backend_failure() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    let server_url = server.uri();
    write_chatgpt_base_url(codex_home.path(), &server_url)?;

    Mock::given(method("POST"))
        .and(path("/api/codex/accounts/send_add_credits_nudge_email"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::Credits,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("failed to notify workspace owner"),
        "unexpected error message: {}",
        error.error.message
    );
    assert_eq!(error.error.data, None);

    Ok(())
}

fn consume_reset_params(idempotency_key: &str) -> ConsumeAccountRateLimitResetCreditParams {
    ConsumeAccountRateLimitResetCreditParams {
        idempotency_key: idempotency_key.to_string(),
        credit_id: None,
        auth_profile: None,
    }
}

trait ConsumeResetParamsExt {
    fn with_credit_id(self, credit_id: &str) -> Self;
    fn with_auth_profile(self, profile: Option<&str>) -> Self;
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
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
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

fn write_chatgpt_base_url(codex_home: &Path, base_url: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(config_toml, format!("chatgpt_base_url = \"{base_url}\"\n"))
}
