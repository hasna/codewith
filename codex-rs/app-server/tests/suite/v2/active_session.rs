use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::ActiveSessionListParams;
use codex_app_server_protocol::ActiveSessionListResponse;
use codex_app_server_protocol::ActiveSessionMessageDelivery;
use codex_app_server_protocol::ActiveSessionPeerKind;
use codex_app_server_protocol::ActiveSessionSendParams;
use codex_app_server_protocol::ActiveSessionSendResponse;
use codex_app_server_protocol::ActiveSessionSendStatus;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn active_session_list_returns_loaded_thread_peers() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let first = start_thread(&mut mcp).await?;
    let second = start_thread(&mut mcp).await?;
    let mut expected = [first.clone(), second.clone()];
    expected.sort();

    let list_id = mcp
        .send_active_session_list_request(ActiveSessionListParams::default())
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let ActiveSessionListResponse {
        mut data,
        next_cursor,
    } = to_response::<ActiveSessionListResponse>(resp)?;
    data.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));

    assert_eq!(data.len(), 2);
    assert_eq!(data[0].thread_id, expected[0]);
    assert_eq!(data[0].peer_id, expected[0]);
    assert_eq!(data[0].kind, ActiveSessionPeerKind::CodewithSession);
    assert_eq!(data[1].thread_id, expected[1]);
    assert_eq!(next_cursor, None);

    Ok(())
}

#[tokio::test]
async fn active_session_list_paginates_by_peer_id() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let first = start_thread(&mut mcp).await?;
    let second = start_thread(&mut mcp).await?;
    let mut expected = [first, second];
    expected.sort();

    let list_id = mcp
        .send_active_session_list_request(ActiveSessionListParams {
            cursor: None,
            limit: Some(1),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let ActiveSessionListResponse {
        data: first_page,
        next_cursor,
    } = to_response::<ActiveSessionListResponse>(resp)?;
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].peer_id, expected[0]);
    assert_eq!(next_cursor, Some(expected[0].clone()));

    let list_id = mcp
        .send_active_session_list_request(ActiveSessionListParams {
            cursor: next_cursor,
            limit: Some(1),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let ActiveSessionListResponse {
        data: second_page,
        next_cursor,
    } = to_response::<ActiveSessionListResponse>(resp)?;
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].peer_id, expected[1]);
    assert_eq!(next_cursor, None);

    Ok(())
}

#[tokio::test]
async fn active_session_send_delivers_to_loaded_thread() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let sender_thread_id = start_thread(&mut mcp).await?;
    let target_thread_id = start_thread(&mut mcp).await?;

    let send_id = mcp
        .send_active_session_send_request(ActiveSessionSendParams {
            target_thread_id: target_thread_id.clone(),
            message: "hello active peer".to_string(),
            sender_thread_id: Some(sender_thread_id.clone()),
            sender_label: Some("integration test".to_string()),
            delivery: Some(ActiveSessionMessageDelivery::QueueOnly),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(send_id)),
    )
    .await??;
    let response = to_response::<ActiveSessionSendResponse>(resp)?;

    assert_eq!(response.status, ActiveSessionSendStatus::Delivered);
    assert_eq!(response.target_thread_id, target_thread_id);
    assert_eq!(response.sender_thread_id, Some(sender_thread_id));
    assert_eq!(response.reason, None);
    assert!(!response.message_id.is_empty());

    Ok(())
}

#[tokio::test]
async fn active_session_send_rejects_unloaded_target_without_resuming() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let missing_thread_id = "00000000-0000-4000-8000-000000000001".to_string();
    let send_id = mcp
        .send_active_session_send_request(ActiveSessionSendParams {
            target_thread_id: missing_thread_id.clone(),
            message: "hello inactive peer".to_string(),
            sender_thread_id: None,
            sender_label: None,
            delivery: Some(ActiveSessionMessageDelivery::QueueOnly),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(send_id)),
    )
    .await??;
    let response = to_response::<ActiveSessionSendResponse>(resp)?;

    assert_eq!(response.status, ActiveSessionSendStatus::NotLoaded);
    assert_eq!(response.target_thread_id, missing_thread_id);
    assert_eq!(response.sender_thread_id, None);
    assert_eq!(
        response.reason,
        Some("target thread is not currently loaded; inactive delivery is deferred".to_string())
    );

    Ok(())
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}

async fn start_thread(mcp: &mut McpProcess) -> Result<String> {
    let req_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("gpt-5.2".to_string()),
            ..Default::default()
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(req_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(resp)?;
    Ok(thread.id)
}
