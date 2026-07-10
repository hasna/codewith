use std::collections::HashMap;
use std::process::Command;

use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;
use codex_config::types::OAuthCredentialsStoreMode;
use codex_mcp::EffectiveMcpServer;
use codex_mcp::McpCredentialPolicy;
use codex_mcp::compute_auth_statuses;
use codex_protocol::protocol::McpAuthStatus;
use codex_rmcp_client::StoredOAuthTokens;
use codex_rmcp_client::WrappedOAuthTokenResponse;
use codex_rmcp_client::save_oauth_tokens;
use oauth2::AccessToken;
use oauth2::basic::BasicTokenType;
use rmcp::transport::auth::OAuthTokenResponse;
use rmcp::transport::auth::VendorExtraTokenFields;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

const SERVER_NAME: &str = "infinity-policy-bridge";
const SERVER_URL_ENV: &str = "MCP_INFINITY_AUTH_STATUS_SERVER_URL";

#[tokio::test]
async fn infinity_agent_policy_auth_status_skips_stored_tokens_and_discovery() -> anyhow::Result<()>
{
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let codex_home = TempDir::new()?;
    let server_url = format!("{}/mcp", server.uri());
    let status = Command::new(std::env::current_exe()?)
        .args([
            "infinity_agent_policy_auth_status_child",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("CODEX_HOME", codex_home.path())
        .env(SERVER_URL_ENV, server_url)
        .status()?;
    assert!(status.success(), "auth-status child failed: {status}");
    server.verify().await;
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "spawned by infinity_agent_policy_auth_status_skips_stored_tokens_and_discovery"]
async fn infinity_agent_policy_auth_status_child() -> anyhow::Result<()> {
    let server_url = std::env::var(SERVER_URL_ENV)?;
    let response = OAuthTokenResponse::new(
        AccessToken::new("stored-token-must-not-be-read".to_string()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    save_oauth_tokens(
        SERVER_NAME,
        &StoredOAuthTokens {
            server_name: SERVER_NAME.to_string(),
            url: server_url.clone(),
            client_id: "stored-client-must-not-be-read".to_string(),
            token_response: WrappedOAuthTokenResponse(response),
            expires_at: None,
        },
        OAuthCredentialsStoreMode::File,
    )?;

    let servers = HashMap::from([(
        SERVER_NAME.to_string(),
        EffectiveMcpServer::configured(McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: server_url,
                bearer_token_env_var: None,
                http_headers: None,
                env_http_headers: None,
            },
            environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
            enabled: true,
            required: true,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        }),
    )]);
    let statuses = compute_auth_statuses(
        servers.iter(),
        OAuthCredentialsStoreMode::File,
        /*auth*/ None,
        McpCredentialPolicy::Forbid,
    )
    .await;

    assert_eq!(
        statuses
            .get(SERVER_NAME)
            .expect("restricted bridge auth status")
            .auth_status,
        McpAuthStatus::Unsupported
    );
    Ok(())
}
