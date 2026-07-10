mod streamable_http_test_support;

use std::sync::Arc;
use std::time::Duration;

use codex_config::types::OAuthCredentialsStoreMode;
use codex_exec_server::Environment;
use codex_exec_server::NoRedirectReqwestHttpClient;
use codex_rmcp_client::RmcpClient;
use codex_rmcp_client::StoredOAuthTokens;
use codex_rmcp_client::WrappedOAuthTokenResponse;
use codex_rmcp_client::save_oauth_tokens;
use oauth2::AccessToken;
use oauth2::RefreshToken;
use oauth2::basic::BasicTokenType;
use rmcp::transport::auth::OAuthTokenResponse;
use rmcp::transport::auth::VendorExtraTokenFields;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::process::Command;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use streamable_http_test_support::initialize_client;

const SERVER_NAME: &str = "test-streamable-http-oauth-startup";
const EXPIRED_ACCESS_TOKEN: &str = "expired-access-token";
const REFRESH_TOKEN: &str = "valid-refresh-token";
const REFRESHED_ACCESS_TOKEN: &str = "refreshed-access-token";
const CHILD_SERVER_URL_ENV: &str = "MCP_TEST_OAUTH_STARTUP_SERVER_URL";
const NO_AUTH_SERVER_NAME: &str = "test-streamable-http-no-auth";
const NO_AUTH_ACCESS_TOKEN: &str = "must-never-be-sent";
const NO_AUTH_CHILD_SERVER_URL_ENV: &str = "MCP_TEST_NO_AUTH_SERVER_URL";
const NO_AUTH_REDIRECT_TARGET_ENV: &str = "MCP_TEST_NO_AUTH_REDIRECT_URL";

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn refreshes_expired_persisted_token_before_initialize() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_endpoint": format!("{}/oauth/authorize", server.uri()),
            "token_endpoint": format!("{}/oauth/token", server.uri()),
            "scopes_supported": [""],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains(format!(
            "refresh_token={REFRESH_TOKEN}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": REFRESHED_ACCESS_TOKEN,
            "token_type": "Bearer",
            "expires_in": 7200,
            "refresh_token": REFRESH_TOKEN,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header(
            "authorization",
            format!("Bearer {REFRESHED_ACCESS_TOKEN}"),
        ))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().expect("valid JSON-RPC request");
            match body.get("method").and_then(Value::as_str) {
                Some("initialize") => ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": body.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "protocolVersion": body
                            .pointer("/params/protocolVersion")
                            .cloned()
                            .unwrap_or_else(|| json!("2025-06-18")),
                        "capabilities": {},
                        "serverInfo": {
                            "name": "oauth-startup-test",
                            "version": "0.0.0-test",
                        },
                    },
                })),
                Some("notifications/initialized") => ResponseTemplate::new(202),
                method => ResponseTemplate::new(400)
                    .set_body_string(format!("unexpected JSON-RPC method: {method:?}")),
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let codex_home = TempDir::new()?;
    let server_url = format!("{}/mcp", server.uri());

    // Credential storage resolves CODEX_HOME from the process environment.
    // Run the client half of the test in an ignored helper test so it can use
    // an isolated home without mutating the parent test runner's environment.
    let status = Command::new(std::env::current_exe()?)
        .args(["oauth_startup_child", "--exact", "--ignored", "--nocapture"])
        .env("CODEX_HOME", codex_home.path())
        .env(CHILD_SERVER_URL_ENV, server_url)
        .status()
        .await?;
    assert!(status.success(), "OAuth startup child failed: {status}");
    server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[ignore = "spawned by refreshes_expired_persisted_token_before_initialize"]
async fn oauth_startup_child() -> anyhow::Result<()> {
    let server_url = std::env::var(CHILD_SERVER_URL_ENV)?;

    // Save an expired access token with a valid refresh token so startup must
    // refresh before sending the initialize request.
    let mut response = OAuthTokenResponse::new(
        AccessToken::new(EXPIRED_ACCESS_TOKEN.to_string()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    response.set_refresh_token(Some(RefreshToken::new(REFRESH_TOKEN.to_string())));
    response.set_expires_in(Some(&Duration::from_secs(7200)));
    let tokens = StoredOAuthTokens {
        server_name: SERVER_NAME.to_string(),
        url: server_url.clone(),
        client_id: "test-client-id".to_string(),
        token_response: WrappedOAuthTokenResponse(response),
        expires_at: Some(0),
    };
    save_oauth_tokens(SERVER_NAME, &tokens, OAuthCredentialsStoreMode::File)?;

    // This mirrors create_client's transport and initialization setup, except
    // it omits the direct bearer token. Supplying that token would bypass the
    // persisted OAuth credentials and the startup refresh under test.
    let client = RmcpClient::new_streamable_http_client(
        SERVER_NAME,
        &server_url,
        /*bearer_token*/ None,
        /*http_headers*/ None,
        /*env_http_headers*/ None,
        OAuthCredentialsStoreMode::File,
        Environment::default_for_tests().get_http_client(),
        /*auth_provider*/ None,
    )
    .await?;

    initialize_client(&client).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn infinity_agent_policy_no_auth_transport_ignores_stored_oauth_proxy_and_custom_ca()
-> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().expect("valid JSON-RPC request");
            match body.get("method").and_then(Value::as_str) {
                Some("initialize") => ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": body.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "protocolVersion": body
                            .pointer("/params/protocolVersion")
                            .cloned()
                            .unwrap_or_else(|| json!("2025-06-18")),
                        "capabilities": {},
                        "serverInfo": {
                            "name": "no-auth-startup-test",
                            "version": "0.0.0-test",
                        },
                    },
                })),
                Some("notifications/initialized") => ResponseTemplate::new(202),
                method => ResponseTemplate::new(400)
                    .set_body_string(format!("unexpected JSON-RPC method: {method:?}")),
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let proxy = MockServer::start().await;
    let codex_home = TempDir::new()?;
    let server_url = format!("{}/mcp", server.uri());
    let missing_ca = codex_home.path().join("ambient-ca-must-not-be-read.pem");
    let missing_ssl_ca = codex_home
        .path()
        .join("ambient-ssl-ca-must-not-be-read.pem");
    let status = Command::new(std::env::current_exe()?)
        .args([
            "no_auth_startup_child",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("CODEX_HOME", codex_home.path())
        .env(NO_AUTH_CHILD_SERVER_URL_ENV, server_url)
        .env("HTTP_PROXY", proxy.uri())
        .env("HTTPS_PROXY", proxy.uri())
        .env("ALL_PROXY", proxy.uri())
        .env("http_proxy", proxy.uri())
        .env("https_proxy", proxy.uri())
        .env("all_proxy", proxy.uri())
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .env("CODEX_CA_CERTIFICATE", missing_ca)
        .env("SSL_CERT_FILE", missing_ssl_ca)
        .status()
        .await?;
    assert!(status.success(), "no-auth startup child failed: {status}");
    server.verify().await;

    let proxy_requests = proxy.received_requests().await.unwrap_or_default();
    assert!(
        proxy_requests.is_empty(),
        "credential-forbidden transport must not use ambient proxies"
    );
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        2,
        "only MCP initialization may hit the bridge"
    );
    for request in requests {
        assert_eq!(request.url.path(), "/mcp");
        for forbidden_header in [
            "authorization",
            "proxy-authorization",
            "chatgpt-account-id",
            "x-openai-fedramp",
        ] {
            assert!(
                !request.headers.contains_key(forbidden_header),
                "credential-forbidden transport sent {forbidden_header}"
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[ignore = "spawned by infinity_agent_policy_no_auth_transport_ignores_stored_oauth_proxy_and_custom_ca"]
async fn no_auth_startup_child() -> anyhow::Result<()> {
    let server_url = std::env::var(NO_AUTH_CHILD_SERVER_URL_ENV)?;
    let response = OAuthTokenResponse::new(
        AccessToken::new(NO_AUTH_ACCESS_TOKEN.to_string()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    let tokens = StoredOAuthTokens {
        server_name: NO_AUTH_SERVER_NAME.to_string(),
        url: server_url.clone(),
        client_id: "must-not-be-read".to_string(),
        token_response: WrappedOAuthTokenResponse(response),
        expires_at: None,
    };
    save_oauth_tokens(
        NO_AUTH_SERVER_NAME,
        &tokens,
        OAuthCredentialsStoreMode::File,
    )?;

    let client = RmcpClient::new_unauthenticated_streamable_http_client(
        &server_url,
        Arc::new(NoRedirectReqwestHttpClient),
    )
    .await?;
    initialize_client(&client).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn infinity_agent_policy_no_auth_transport_does_not_follow_redirects() -> anyhow::Result<()> {
    let target = MockServer::start().await;
    let redirector = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/capture", target.uri())),
        )
        .expect(1)
        .mount(&redirector)
        .await;

    let status = Command::new(std::env::current_exe()?)
        .args([
            "no_auth_redirect_child",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env(
            NO_AUTH_REDIRECT_TARGET_ENV,
            format!("{}/mcp", redirector.uri()),
        )
        .status()
        .await?;
    assert!(status.success(), "no-auth redirect child failed: {status}");
    redirector.verify().await;
    assert!(
        target
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "credential-forbidden transport followed a redirect"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[ignore = "spawned by infinity_agent_policy_no_auth_transport_does_not_follow_redirects"]
async fn no_auth_redirect_child() -> anyhow::Result<()> {
    let server_url = std::env::var(NO_AUTH_REDIRECT_TARGET_ENV)?;
    let client = RmcpClient::new_unauthenticated_streamable_http_client(
        &server_url,
        Arc::new(NoRedirectReqwestHttpClient),
    )
    .await?;
    assert!(
        initialize_client(&client).await.is_err(),
        "redirect response must not initialize the MCP client"
    );
    Ok(())
}
