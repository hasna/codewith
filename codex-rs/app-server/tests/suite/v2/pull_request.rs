use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::PullRequestListResponse;
use codex_app_server_protocol::PullRequestOverviewResponse;
use codex_app_server_protocol::PullRequestQueryState;
use codex_app_server_protocol::PullRequestReadResponse;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn pull_request_overview_reports_unavailable_in_band() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "pullRequest/overview",
            Some(json!({ "baseRepoPath": null })),
        )
        .await?;
    let response = read_json_response(&mut mcp, request_id).await?;
    let overview: PullRequestOverviewResponse = to_response(response)?;
    assert_eq!(PullRequestQueryState::Unavailable, overview.query_state);
    assert_eq!(None, overview.overview);
    assert!(overview.message.is_some());

    Ok(())
}

#[tokio::test]
async fn pull_request_list_reports_unavailable_in_band() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "pullRequest/list",
            Some(json!({
                "baseRepoPath": null,
                "state": null,
                "cursor": null,
                "limit": null,
            })),
        )
        .await?;
    let response = read_json_response(&mut mcp, request_id).await?;
    let list: PullRequestListResponse = to_response(response)?;
    assert_eq!(PullRequestQueryState::Unavailable, list.query_state);
    assert!(list.data.is_empty());
    assert_eq!(None, list.next_cursor);
    assert!(list.message.is_some());

    Ok(())
}

#[tokio::test]
async fn pull_request_read_reports_unavailable_in_band() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "pullRequest/read",
            Some(json!({ "number": 7, "baseRepoPath": null })),
        )
        .await?;
    let response = read_json_response(&mut mcp, request_id).await?;
    let read: PullRequestReadResponse = to_response(response)?;
    assert_eq!(PullRequestQueryState::Unavailable, read.query_state);
    assert_eq!(None, read.pull_request);
    assert!(read.message.is_some());

    Ok(())
}

#[tokio::test]
async fn pull_request_methods_require_experimental_api_capability() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;

    let init = mcp
        .initialize_with_capabilities(
            default_client_info(),
            Some(InitializeCapabilities {
                experimental_api: false,
                request_attestation: false,
                opt_out_notification_methods: None,
            }),
        )
        .await?;
    let JSONRPCMessage::Response(_) = init else {
        anyhow::bail!("expected initialize response, got {init:?}");
    };

    let requests = vec![
        (
            mcp.send_raw_request(
                "pullRequest/overview",
                Some(json!({ "baseRepoPath": null })),
            )
            .await?,
            "pullRequest/overview",
        ),
        (
            mcp.send_raw_request(
                "pullRequest/list",
                Some(json!({
                    "baseRepoPath": null,
                    "state": null,
                    "cursor": null,
                    "limit": null,
                })),
            )
            .await?,
            "pullRequest/list",
        ),
        (
            mcp.send_raw_request(
                "pullRequest/read",
                Some(json!({ "number": 1, "baseRepoPath": null })),
            )
            .await?,
            "pullRequest/read",
        ),
    ];

    for (request_id, method) in requests {
        let error = read_error(&mut mcp, request_id).await?;
        assert_experimental_capability_error(error, method);
    }

    Ok(())
}

async fn read_json_response(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCResponse> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await?
}

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCError> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: DEFAULT_CLIENT_NAME.to_string(),
        title: None,
        version: "0.1.0".to_string(),
    }
}

fn assert_experimental_capability_error(error: JSONRPCError, reason: &str) {
    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        format!("{reason} requires experimentalApi capability")
    );
    assert_eq!(error.error.data, None);
}
