use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadContinueParams;
use codex_app_server_protocol::ThreadContinueResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_core::RolloutRecorder;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::RolloutItem;
use core_test_support::responses;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn thread_continue_summarizes_source_into_captured_destination_once() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("source-response"),
                responses::ev_assistant_message("source-message", "Source work completed"),
                responses::ev_completed("source-response"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("continue-response"),
                responses::ev_assistant_message("continue-message", "Concise source handoff"),
                responses::ev_completed("continue-response"),
            ]),
        ],
    )
    .await;

    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let source = start_thread(&mut mcp).await?;
    let turn_request = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: source.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "Implement the source feature".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_request)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let destination = start_thread(&mut mcp).await?;
    let continue_request = mcp
        .send_thread_continue_request(ThreadContinueParams {
            destination_thread_id: destination.id.clone(),
            source_thread_id: source.id.clone(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(continue_request)),
    )
    .await??;
    let response = to_response::<ThreadContinueResponse>(response)?;
    assert_eq!(response.summary, "Concise source handoff");

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body_json()["model"], "mock-model");
    let continuation_input = requests[1].input();
    assert!(response_item_text_present(
        &continuation_input,
        "Implement the source feature"
    ));
    assert!(response_item_text_present(
        &continuation_input,
        "Source work completed"
    ));

    let rollout_path = destination
        .path
        .as_ref()
        .context("destination rollout path missing")?;
    let history = RolloutRecorder::get_rollout_history(rollout_path).await?;
    let InitialHistory::Resumed(history) = history else {
        panic!("expected resumed destination rollout history");
    };
    let response_item_count = history
        .history
        .iter()
        .filter(|item| {
            matches!(
                item,
                RolloutItem::ResponseItem(ResponseItem::Message {
                    role,
                    content,
                    ..
                }) if role == "assistant"
                    && matches!(
                        content.as_slice(),
                        [ContentItem::OutputText { text }] if text == "Concise source handoff"
                    )
            )
        })
        .count();
    let agent_message_count = history
        .history
        .iter()
        .filter(|item| {
            matches!(
                item,
                RolloutItem::EventMsg(EventMsg::AgentMessage(event))
                    if event.message == "Concise source handoff"
            )
        })
        .count();
    assert_eq!(response_item_count, 1);
    assert_eq!(agent_message_count, 1);

    Ok(())
}

async fn start_thread(mcp: &mut TestAppServer) -> Result<codex_app_server_protocol::Thread> {
    let request = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request)),
    )
    .await??;
    Ok(to_response::<ThreadStartResponse>(response)?.thread)
}

fn response_item_text_present(items: &[serde_json::Value], expected: &str) -> bool {
    items.iter().any(|item| {
        item.get("content")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|content| {
                content.iter().any(|content| {
                    content.get("text").and_then(serde_json::Value::as_str) == Some(expected)
                })
            })
    })
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider"
base_url = "{server_uri}/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
