use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::to_response;
use app_test_support::write_mock_provider_models_cache;
use app_test_support::write_mock_responses_config_toml;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadExternalAgentEvent;
use codex_app_server_protocol::ThreadExternalAgentEventNotification;
use codex_app_server_protocol::ThreadExternalAgentMode;
use codex_app_server_protocol::ThreadExternalAgentStartParams;
use codex_app_server_protocol::ThreadExternalAgentStartResponse;
use codex_app_server_protocol::ThreadExternalAgentStartStatus;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn thread_external_agent_start_emits_run_event_and_validates_runtime() -> Result<()> {
    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;

    let server = create_mock_responses_server_sequence(vec![]).await;
    write_mock_responses_config_toml(
        codex_home.as_path(),
        &server.uri(),
        &BTreeMap::new(),
        200_000,
        None,
        "mock_provider",
        "compact",
    )?;
    codex_login::save_auth_profile_metadata(
        codex_home.as_path(),
        "cursor-work",
        codex_login::AuthProfileMetadata {
            subscription_provider: codex_login::AuthProfileSubscriptionProvider::Cursor,
        },
    )?;
    write_mock_provider_models_cache(codex_home.as_path())?;

    let mut mcp = McpProcess::new_with_env(
        codex_home.as_path(),
        &[("CODEWITH_AUTH_PROFILE", Some("cursor-work"))],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(start_resp)?;

    let external_agent_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id.clone(),
            runtime_id: "cursor".to_string(),
            task: "inspect the auth wiring".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let external_agent_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(external_agent_id)),
    )
    .await??;
    let response: ThreadExternalAgentStartResponse = to_response(external_agent_resp)?;
    assert_eq!(
        response,
        ThreadExternalAgentStartResponse {
            status: ThreadExternalAgentStartStatus::Started,
            run_id: response.run_id.clone(),
            message: "external-agent run started".to_string(),
        }
    );
    let run_id = response.run_id.expect("external-agent run id");
    let started_notification = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/externalAgent/event"),
    )
    .await??;
    let started: ThreadExternalAgentEventNotification = serde_json::from_value(
        started_notification
            .params
            .expect("external-agent event params"),
    )?;
    assert_eq!(started.thread_id, thread.id);
    assert_eq!(started.run_id, run_id);
    assert_eq!(
        started.event,
        ThreadExternalAgentEvent::RunStarted {
            runtime_id: "cursor".to_string(),
            mode: ThreadExternalAgentMode::Plan,
            task: "inspect the auth wiring".to_string(),
        }
    );

    let grok_alias_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id.clone(),
            runtime_id: "grok".to_string(),
            task: "inspect the auth wiring".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let grok_alias_error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(grok_alias_id)),
    )
    .await??;
    assert_eq!(grok_alias_error.error.code, -32600);
    assert_eq!(
        grok_alias_error.error.message,
        "use runtimeId `grok-build` for Grok Build external-agent runs"
    );

    let empty_task_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id,
            runtime_id: "grok-build".to_string(),
            task: "   ".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let empty_task_error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(empty_task_id)),
    )
    .await??;
    assert_eq!(empty_task_error.error.code, -32600);
    assert_eq!(empty_task_error.error.message, "task must not be empty");

    Ok(())
}
