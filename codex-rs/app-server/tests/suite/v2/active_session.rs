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
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_protocol::protocol::InterAgentCommunication;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
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
    let sender_thread_id_input = sender_thread_id.to_ascii_uppercase();
    let target_thread_id_input = target_thread_id.to_ascii_uppercase();

    let send_id = mcp
        .send_active_session_send_request(ActiveSessionSendParams {
            target_thread_id: target_thread_id_input,
            message: "hello active peer".to_string(),
            sender_thread_id: Some(sender_thread_id_input),
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
async fn active_session_send_queue_only_reaches_target_next_turn_mailbox() -> Result<()> {
    let server = responses::start_mock_server().await;
    let first_body = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-1"),
    ]);
    let second_body = responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-2", "Done"),
        responses::ev_completed("resp-2"),
    ]);
    let response_mock = responses::mount_sse_sequence(&server, vec![first_body, second_body]).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let sender_thread_id = start_thread(&mut mcp).await?;
    let target_thread_id = start_thread(&mut mcp).await?;
    let send_response = send_active_session_message(
        &mut mcp,
        ActiveSessionSendParams {
            target_thread_id: target_thread_id.clone(),
            message: "queued delivery evidence".to_string(),
            sender_thread_id: Some(sender_thread_id.clone()),
            sender_label: Some("queue-only test".to_string()),
            delivery: Some(ActiveSessionMessageDelivery::QueueOnly),
        },
    )
    .await?;
    assert_eq!(send_response.status, ActiveSessionSendStatus::Delivered);

    let turn_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: target_thread_id,
            client_user_message_id: None,
            input: vec![UserInput::Text {
                text: "process queued message".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response::<TurnStartResponse>(resp)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = response_mock.requests();
    assert_eq!(
        requests.len(),
        2,
        "expected queued mail to drive a follow-up request"
    );
    assert_eq!(
        inter_agent_messages_in_request(&requests[0].body_json()),
        Vec::new()
    );
    let messages = inter_agent_messages_in_request(&requests[1].body_json());
    assert_eq!(
        messages,
        vec![InterAgentCommunication {
            author: codex_protocol::AgentPath::root(),
            recipient: codex_protocol::AgentPath::root(),
            other_recipients: Vec::new(),
            content: format!(
                "Active session message {} from queue-only test:\n\nqueued delivery evidence",
                send_response.message_id
            ),
            trigger_turn: false,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn active_session_send_trigger_turn_wakes_target_mailbox() -> Result<()> {
    let server = responses::start_mock_server().await;
    let body = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-1"),
    ]);
    let response_mock = responses::mount_sse_once(&server, body).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let target_thread_id = start_thread(&mut mcp).await?;
    let send_response = send_active_session_message(
        &mut mcp,
        ActiveSessionSendParams {
            target_thread_id,
            message: "wake delivery evidence".to_string(),
            sender_thread_id: None,
            sender_label: Some("trigger-turn test".to_string()),
            delivery: Some(ActiveSessionMessageDelivery::TriggerTurn),
        },
    )
    .await?;
    assert_eq!(send_response.status, ActiveSessionSendStatus::Delivered);
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let messages = inter_agent_messages_in_request(&response_mock.single_request().body_json());
    assert_eq!(
        messages,
        vec![InterAgentCommunication {
            author: codex_protocol::AgentPath::root(),
            recipient: codex_protocol::AgentPath::root(),
            other_recipients: Vec::new(),
            content: format!(
                "Active session message {} from trigger-turn test:\n\nwake delivery evidence",
                send_response.message_id
            ),
            trigger_turn: true,
        }]
    );

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
        Some(
            "target thread is not currently loaded; no offline delivery was attempted".to_string()
        )
    );

    Ok(())
}

#[tokio::test]
async fn active_session_send_returns_not_loaded_for_offline_thread_without_resuming() -> Result<()>
{
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let offline_thread_id = {
        let mut mcp = McpProcess::new(codex_home.path()).await?;
        timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
        start_thread(&mut mcp).await?
    };

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let send_response = send_active_session_message(
        &mut mcp,
        ActiveSessionSendParams {
            target_thread_id: offline_thread_id.clone(),
            message: "hello offline peer".to_string(),
            sender_thread_id: None,
            sender_label: None,
            delivery: Some(ActiveSessionMessageDelivery::TriggerTurn),
        },
    )
    .await?;

    assert_eq!(send_response.status, ActiveSessionSendStatus::NotLoaded);
    assert_eq!(send_response.target_thread_id, offline_thread_id);
    assert_eq!(send_response.sender_thread_id, None);
    assert_eq!(
        send_response.reason,
        Some(
            "target thread is not currently loaded; no offline delivery was attempted".to_string()
        )
    );

    let list_id = mcp
        .send_active_session_list_request(ActiveSessionListParams::default())
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let ActiveSessionListResponse { data, next_cursor } =
        to_response::<ActiveSessionListResponse>(resp)?;
    assert_eq!(data, Vec::new());
    assert_eq!(next_cursor, None);

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

async fn send_active_session_message(
    mcp: &mut McpProcess,
    params: ActiveSessionSendParams,
) -> Result<ActiveSessionSendResponse> {
    let send_id = mcp.send_active_session_send_request(params).await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(send_id)),
    )
    .await??;
    to_response::<ActiveSessionSendResponse>(resp)
}

fn inter_agent_messages_in_request(body: &Value) -> Vec<InterAgentCommunication> {
    body.get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .filter(|item| item.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|span| {
            matches!(
                span.get("type").and_then(Value::as_str),
                Some("input_text" | "output_text")
            )
        })
        .filter_map(|span| span.get("text").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<InterAgentCommunication>(text).ok())
        .collect()
}
