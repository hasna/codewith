use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::ActiveSessionPeerKind;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LocalSession;
use codex_app_server_protocol::LocalSessionListParams;
use codex_app_server_protocol::LocalSessionListResponse;
use codex_app_server_protocol::LocalSessionRedaction;
use codex_app_server_protocol::LocalSessionStatus;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn local_session_list_returns_loaded_sessions_with_peer_metadata() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let first = start_thread(&mut mcp).await?;
    let second = start_thread(&mut mcp).await?;

    let mut response = list_local_sessions(&mut mcp, LocalSessionListParams::default()).await?;
    response.data.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    let mut expected = [first, second];
    expected.sort();

    assert_eq!(response.next_cursor, None);
    assert_eq!(response.data.len(), 2);
    assert_loaded_local_session(&response.data[0], expected[0].as_str());
    assert_loaded_local_session(&response.data[1], expected[1].as_str());

    Ok(())
}

#[tokio::test]
async fn local_session_list_returns_persisted_sessions_without_resuming_them() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let thread_id = {
        let mut mcp = init_mcp(codex_home.path()).await?;
        start_thread(&mut mcp).await?
    };

    let mut mcp = init_mcp(codex_home.path()).await?;
    let loaded = list_loaded_threads(&mut mcp).await?;
    assert_eq!(loaded.data, Vec::<String>::new());

    let response = list_local_sessions(&mut mcp, LocalSessionListParams::default()).await?;
    assert_eq!(response.next_cursor, None);
    assert_eq!(response.data.len(), 1);
    let session = &response.data[0];
    assert_eq!(session.thread_id, thread_id);
    assert_eq!(session.status, LocalSessionStatus::NotLoaded);
    assert_eq!(session.peer, None);
    assert_eq!(session.runtime_session_id, None);
    assert_eq!(session.active_flags, Vec::new());

    Ok(())
}

#[tokio::test]
async fn local_session_list_paginates_and_filters_statuses() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let first = start_thread(&mut mcp).await?;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let second = start_thread(&mut mcp).await?;

    let first_page = list_local_sessions(
        &mut mcp,
        LocalSessionListParams {
            limit: Some(1),
            statuses: Some(vec![LocalSessionStatus::Idle]),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(first_page.data.len(), 1);
    let first_page_thread_id = first_page.data[0].thread_id.clone();
    assert!(
        first_page_thread_id == first || first_page_thread_id == second,
        "unexpected first page thread id {first_page_thread_id}"
    );
    assert_eq!(first_page.data[0].status, LocalSessionStatus::Idle);
    assert!(first_page.next_cursor.is_some());

    let second_page = list_local_sessions(
        &mut mcp,
        LocalSessionListParams {
            cursor: first_page.next_cursor,
            limit: Some(1),
            statuses: Some(vec![LocalSessionStatus::Idle]),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(second_page.data.len(), 1);
    assert_ne!(second_page.data[0].thread_id, first_page_thread_id);
    assert_eq!(second_page.data[0].status, LocalSessionStatus::Idle);
    assert_eq!(second_page.next_cursor, None);

    Ok(())
}

#[tokio::test]
async fn local_session_list_filters_unloaded_status_after_restart() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let thread_id = {
        let mut mcp = init_mcp(codex_home.path()).await?;
        start_thread(&mut mcp).await?
    };

    let mut mcp = init_mcp(codex_home.path()).await?;
    let response = list_local_sessions(
        &mut mcp,
        LocalSessionListParams {
            statuses: Some(vec![LocalSessionStatus::NotLoaded]),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(response.data.len(), 1);
    assert_eq!(response.data[0].thread_id, thread_id);
    assert_eq!(response.data[0].status, LocalSessionStatus::NotLoaded);

    Ok(())
}

fn assert_loaded_local_session(session: &LocalSession, expected_thread_id: &str) {
    assert_eq!(session.thread_id, expected_thread_id);
    assert_eq!(session.status, LocalSessionStatus::Idle);
    assert_eq!(session.active_flags, Vec::new());
    assert!(session.runtime_session_id.is_some());
    let Some(peer) = session.peer.as_ref() else {
        panic!("loaded session peer missing");
    };
    assert_eq!(peer.peer_id, expected_thread_id);
    assert_eq!(peer.kind, ActiveSessionPeerKind::CodewithSession);
    assert_eq!(peer.capabilities.len(), 3);
    assert_eq!(
        session.redactions,
        vec![LocalSessionRedaction::ProcessDetails]
    );
    assert_eq!(session.model_provider, "mock_provider");
}

async fn init_mcp(codex_home: &Path) -> Result<TestAppServer> {
    let mut mcp = TestAppServer::new(codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
}

async fn list_local_sessions(
    mcp: &mut TestAppServer,
    params: LocalSessionListParams,
) -> Result<LocalSessionListResponse> {
    let request_id = mcp.send_local_session_list_request(params).await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<LocalSessionListResponse>(resp)
}

async fn list_loaded_threads(mcp: &mut TestAppServer) -> Result<ThreadLoadedListResponse> {
    let request_id = mcp
        .send_thread_loaded_list_request(ThreadLoadedListParams::default())
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadLoadedListResponse>(resp)
}

async fn start_thread(mcp: &mut TestAppServer) -> Result<String> {
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
    let turn_start_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![UserInput::Text {
                text: format!("seed local session {}", thread.id),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_start_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response::<TurnStartResponse>(turn_resp)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    Ok(thread.id)
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
