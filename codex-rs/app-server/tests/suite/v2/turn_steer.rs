#![cfg(unix)]

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::create_shell_command_sse_response;
use app_test_support::to_response;
use app_test_support::write_mock_responses_config_toml_with_chatgpt_base_url;
use codex_app_server::INPUT_TOO_LARGE_ERROR_CODE;
use codex_app_server::INVALID_PARAMS_ERROR_CODE;
use codex_app_server_protocol::AdditionalContextEntry;
use codex_app_server_protocol::AdditionalContextKind;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use serde_json::Value;
use std::collections::HashMap;
use tempfile::TempDir;
use tokio::time::timeout;

use super::analytics::mount_analytics_capture;
use super::analytics::wait_for_analytics_event;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_ADDITIONAL_CONTEXT_ENTRIES: usize = 16;
const MAX_ADDITIONAL_CONTEXT_KEY_CHARS: usize = 128;
const MAX_ADDITIONAL_CONTEXT_VALUE_BYTES: usize = 64 * 1024;
const MAX_ADDITIONAL_CONTEXT_TOTAL_BYTES: usize = 128 * 1024;

fn untrusted_context_entry(value: impl Into<String>) -> AdditionalContextEntry {
    AdditionalContextEntry {
        value: value.into(),
        kind: AdditionalContextKind::Untrusted,
        source: None,
    }
}

async fn expect_turn_steer_additional_context_error(
    mcp: &mut TestAppServer,
    thread_id: &str,
    turn_id: &str,
    additional_context: HashMap<String, AdditionalContextEntry>,
) -> Result<JSONRPCError> {
    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread_id.to_string(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "steer".to_string(),
                text_elements: Vec::new(),
            }],
            responsesapi_client_metadata: None,
            additional_context: Some(additional_context),
            expected_turn_id: turn_id.to_string(),
        })
        .await?;
    let err = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(steer_req)),
    )
    .await??;
    Ok(err)
}

fn assert_additional_context_too_large_error(
    err: JSONRPCError,
    message: &str,
    limit_name: &str,
) -> Value {
    assert_eq!(err.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(err.error.message, message);
    let Some(data) = err.error.data else {
        panic!("expected structured error data");
    };
    assert_eq!(data["input_error_code"], INPUT_TOO_LARGE_ERROR_CODE);
    assert_eq!(data["input_field"], "additionalContext");
    assert_eq!(data["limit_name"], limit_name);
    data
}

#[tokio::test]
async fn turn_steer_requires_active_turn() -> Result<()> {
    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;

    let server = create_mock_responses_server_sequence(vec![]).await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        &codex_home,
        &server.uri(),
        &server.uri(),
    )?;
    mount_analytics_capture(&server, &codex_home).await?;

    let mut mcp = TestAppServer::new_without_managed_config(&codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread.id.clone(),
            client_user_message_id: Some("client-steer-message-1".to_string()),
            input: vec![V2UserInput::Text {
                text: "steer".to_string(),
                text_elements: Vec::new(),
            }],
            responsesapi_client_metadata: None,
            additional_context: None,
            expected_turn_id: "turn-does-not-exist".to_string(),
        })
        .await?;
    let steer_err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(steer_req)),
    )
    .await??;
    assert_eq!(steer_err.error.code, -32600);

    let event =
        wait_for_analytics_event(&server, DEFAULT_READ_TIMEOUT, "codex_turn_steer_event").await?;
    assert_eq!(event["event_params"]["thread_id"], thread.id);
    assert_eq!(event["event_params"]["result"], "rejected");
    assert_eq!(event["event_params"]["num_input_images"], 0);
    assert_eq!(
        event["event_params"]["expected_turn_id"],
        "turn-does-not-exist"
    );
    assert_eq!(
        event["event_params"]["accepted_turn_id"],
        serde_json::Value::Null
    );
    assert_eq!(event["event_params"]["rejection_reason"], "no_active_turn");

    Ok(())
}

#[tokio::test]
async fn turn_steer_rejects_oversized_text_input() -> Result<()> {
    #[cfg(target_os = "windows")]
    let shell_command = vec![
        "powershell".to_string(),
        "-Command".to_string(),
        "Start-Sleep -Seconds 10".to_string(),
    ];
    #[cfg(not(target_os = "windows"))]
    let shell_command = vec!["sleep".to_string(), "10".to_string()];

    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;
    let working_directory = tmp.path().join("workdir");
    std::fs::create_dir(&working_directory)?;

    let server =
        create_mock_responses_server_sequence_unchecked(vec![create_shell_command_sse_response(
            shell_command.clone(),
            Some(&working_directory),
            Some(10_000),
            "call_sleep",
        )?])
        .await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        &codex_home,
        &server.uri(),
        &server.uri(),
    )?;
    mount_analytics_capture(&server, &codex_home).await?;

    let mut mcp = TestAppServer::new_without_managed_config(&codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "run sleep".to_string(),
                text_elements: Vec::new(),
            }],
            cwd: Some(working_directory.clone()),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let TurnStartResponse { turn } = to_response::<TurnStartResponse>(turn_resp)?;

    let _task_started: JSONRPCNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    let oversized_input = "x".repeat(MAX_USER_INPUT_TEXT_CHARS + 1);
    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: oversized_input.clone(),
                text_elements: Vec::new(),
            }],
            responsesapi_client_metadata: None,
            additional_context: None,
            expected_turn_id: turn.id.clone(),
        })
        .await?;
    let steer_err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(steer_req)),
    )
    .await??;

    assert_eq!(steer_err.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(
        steer_err.error.message,
        format!("Input exceeds the maximum length of {MAX_USER_INPUT_TEXT_CHARS} characters.")
    );
    let data = steer_err
        .error
        .data
        .expect("expected structured error data");
    assert_eq!(data["input_error_code"], INPUT_TOO_LARGE_ERROR_CODE);
    assert_eq!(data["max_chars"], MAX_USER_INPUT_TEXT_CHARS);
    assert_eq!(data["actual_chars"], oversized_input.chars().count());

    mcp.interrupt_turn_and_wait_for_aborted(thread.id, turn.id, DEFAULT_READ_TIMEOUT)
        .await?;

    Ok(())
}

#[tokio::test]
async fn turn_steer_rejects_additional_context_application_and_limits() -> Result<()> {
    #[cfg(target_os = "windows")]
    let shell_command = vec![
        "powershell".to_string(),
        "-Command".to_string(),
        "Start-Sleep -Seconds 10".to_string(),
    ];
    #[cfg(not(target_os = "windows"))]
    let shell_command = vec!["sleep".to_string(), "10".to_string()];

    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;
    let working_directory = tmp.path().join("workdir");
    std::fs::create_dir(&working_directory)?;

    let server =
        create_mock_responses_server_sequence_unchecked(vec![create_shell_command_sse_response(
            shell_command.clone(),
            Some(&working_directory),
            Some(10_000),
            "call_sleep",
        )?])
        .await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        &codex_home,
        &server.uri(),
        &server.uri(),
    )?;
    mount_analytics_capture(&server, &codex_home).await?;

    let mut mcp = TestAppServer::new_without_managed_config(&codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "run sleep".to_string(),
                text_elements: Vec::new(),
            }],
            cwd: Some(working_directory.clone()),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let TurnStartResponse { turn } = to_response::<TurnStartResponse>(turn_resp)?;

    let _task_started: JSONRPCNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    let application_err = expect_turn_steer_additional_context_error(
        &mut mcp,
        &thread.id,
        &turn.id,
        HashMap::from([(
            "custom_source".to_string(),
            AdditionalContextEntry {
                value: "source value".to_string(),
                kind: AdditionalContextKind::Application,
                source: None,
            },
        )]),
    )
    .await?;
    assert_eq!(application_err.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(
        application_err.error.message,
        "additionalContext kind `application` is reserved for trusted server-owned context and \
         cannot be supplied by turn/start or turn/steer clients"
    );
    assert!(
        application_err.error.data.is_none(),
        "application kind rejection should not be reported as an input-too-large error"
    );

    let too_many_entries = (0..=MAX_ADDITIONAL_CONTEXT_ENTRIES)
        .map(|index| {
            (
                format!("source_{index}"),
                untrusted_context_entry(format!("value {index}")),
            )
        })
        .collect::<HashMap<_, _>>();
    let too_many_err = expect_turn_steer_additional_context_error(
        &mut mcp,
        &thread.id,
        &turn.id,
        too_many_entries,
    )
    .await?;
    let data = assert_additional_context_too_large_error(
        too_many_err,
        &format!(
            "additionalContext must not contain more than \
             {MAX_ADDITIONAL_CONTEXT_ENTRIES} entries."
        ),
        "max_entries",
    );
    assert_eq!(data["max_entries"], MAX_ADDITIONAL_CONTEXT_ENTRIES);
    assert_eq!(data["actual_entries"], MAX_ADDITIONAL_CONTEXT_ENTRIES + 1);

    let long_key = "k".repeat(MAX_ADDITIONAL_CONTEXT_KEY_CHARS + 1);
    let long_key_err = expect_turn_steer_additional_context_error(
        &mut mcp,
        &thread.id,
        &turn.id,
        HashMap::from([(long_key, untrusted_context_entry("value"))]),
    )
    .await?;
    let data = assert_additional_context_too_large_error(
        long_key_err,
        &format!(
            "additionalContext keys must not exceed \
             {MAX_ADDITIONAL_CONTEXT_KEY_CHARS} characters."
        ),
        "max_key_chars",
    );
    assert_eq!(data["max_chars"], MAX_ADDITIONAL_CONTEXT_KEY_CHARS);
    assert_eq!(data["actual_chars"], MAX_ADDITIONAL_CONTEXT_KEY_CHARS + 1);

    let oversized_value = "x".repeat(MAX_ADDITIONAL_CONTEXT_VALUE_BYTES + 1);
    let oversized_value_err = expect_turn_steer_additional_context_error(
        &mut mcp,
        &thread.id,
        &turn.id,
        HashMap::from([(
            "source".to_string(),
            untrusted_context_entry(oversized_value),
        )]),
    )
    .await?;
    let data = assert_additional_context_too_large_error(
        oversized_value_err,
        &format!(
            "additionalContext values must not exceed \
             {MAX_ADDITIONAL_CONTEXT_VALUE_BYTES} bytes."
        ),
        "max_value_bytes",
    );
    assert_eq!(data["max_bytes"], MAX_ADDITIONAL_CONTEXT_VALUE_BYTES);
    assert_eq!(data["actual_bytes"], MAX_ADDITIONAL_CONTEXT_VALUE_BYTES + 1);

    let aggregate_context = (0..3)
        .map(|index| {
            (
                format!("source_{index}"),
                untrusted_context_entry("x".repeat(44 * 1024)),
            )
        })
        .collect::<HashMap<_, _>>();
    let aggregate_err = expect_turn_steer_additional_context_error(
        &mut mcp,
        &thread.id,
        &turn.id,
        aggregate_context,
    )
    .await?;
    let data = assert_additional_context_too_large_error(
        aggregate_err,
        &format!(
            "additionalContext must not exceed \
             {MAX_ADDITIONAL_CONTEXT_TOTAL_BYTES} aggregate bytes."
        ),
        "max_total_bytes",
    );
    assert_eq!(data["max_bytes"], MAX_ADDITIONAL_CONTEXT_TOTAL_BYTES);
    assert!(
        data["actual_bytes"]
            .as_u64()
            .is_some_and(|actual| actual > MAX_ADDITIONAL_CONTEXT_TOTAL_BYTES as u64)
    );

    let requests = server
        .received_requests()
        .await
        .context("failed to fetch received requests")?;
    let response_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    for request in response_requests {
        let body = String::from_utf8_lossy(&request.body);
        assert!(
            !body.contains("steer") && !body.contains("source value"),
            "rejected turn/steer additionalContext should not be submitted to the model"
        );
    }

    mcp.interrupt_turn_and_wait_for_aborted(thread.id, turn.id, DEFAULT_READ_TIMEOUT)
        .await?;

    Ok(())
}

#[tokio::test]
async fn turn_steer_returns_active_turn_id() -> Result<()> {
    #[cfg(target_os = "windows")]
    let shell_command = vec![
        "powershell".to_string(),
        "-Command".to_string(),
        "Start-Sleep -Seconds 2".to_string(),
    ];
    #[cfg(not(target_os = "windows"))]
    let shell_command = vec!["sleep".to_string(), "2".to_string()];

    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;
    let working_directory = tmp.path().join("workdir");
    std::fs::create_dir(&working_directory)?;

    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_shell_command_sse_response(
            shell_command.clone(),
            Some(&working_directory),
            Some(10_000),
            "call_sleep",
        )?,
        app_test_support::create_final_assistant_message_sse_response("Done")?,
    ])
    .await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        &codex_home,
        &server.uri(),
        &server.uri(),
    )?;
    mount_analytics_capture(&server, &codex_home).await?;

    let mut mcp = TestAppServer::new_without_managed_config(&codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "run sleep".to_string(),
                text_elements: Vec::new(),
            }],
            cwd: Some(working_directory.clone()),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let TurnStartResponse { turn } = to_response::<TurnStartResponse>(turn_resp)?;

    let _task_started: JSONRPCNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread.id.clone(),
            client_user_message_id: Some("client-steer-message-1".to_string()),
            input: vec![V2UserInput::Text {
                text: "steer".to_string(),
                text_elements: Vec::new(),
            }],
            responsesapi_client_metadata: None,
            additional_context: None,
            expected_turn_id: turn.id.clone(),
        })
        .await?;
    let steer_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(steer_req)),
    )
    .await??;
    let steer: TurnSteerResponse = to_response::<TurnSteerResponse>(steer_resp)?;
    assert_eq!(steer.turn_id, turn.id);

    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let notification = mcp
                .read_stream_until_notification_message("item/started")
                .await?;
            let params = notification.params.expect("item/started params");
            let item_started: ItemStartedNotification =
                serde_json::from_value(params).expect("deserialize item/started notification");
            let ThreadItem::UserMessage {
                client_id, content, ..
            } = item_started.item
            else {
                continue;
            };
            if client_id == Some("client-steer-message-1".to_string()) {
                assert_eq!(
                    content,
                    vec![V2UserInput::Text {
                        text: "steer".to_string(),
                        text_elements: Vec::new(),
                    }]
                );
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await??;

    let event =
        wait_for_analytics_event(&server, DEFAULT_READ_TIMEOUT, "codex_turn_steer_event").await?;
    assert_eq!(event["event_params"]["thread_id"], thread.id);
    assert_eq!(event["event_params"]["session_id"], thread.session_id);
    assert_eq!(event["event_params"]["result"], "accepted");
    assert_eq!(event["event_params"]["num_input_images"], 0);
    assert_eq!(event["event_params"]["expected_turn_id"], turn.id);
    assert_eq!(event["event_params"]["accepted_turn_id"], turn.id);
    assert_eq!(
        event["event_params"]["rejection_reason"],
        serde_json::Value::Null
    );

    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    Ok(())
}

#[tokio::test]
async fn turn_steer_rejects_context_only_input_without_merging_context() -> Result<()> {
    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;
    let working_directory = tmp.path().join("workdir");
    std::fs::create_dir(&working_directory)?;

    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_shell_command_sse_response(
            vec!["sleep".to_string(), "1".to_string()],
            Some(&working_directory),
            Some(10_000),
            "call_sleep",
        )?,
        app_test_support::create_final_assistant_message_sse_response("Done")?,
    ])
    .await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        &codex_home,
        &server.uri(),
        &server.uri(),
    )?;
    mount_analytics_capture(&server, &codex_home).await?;

    let mut mcp = TestAppServer::new_without_managed_config(&codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "run sleep".to_string(),
                text_elements: Vec::new(),
            }],
            cwd: Some(working_directory),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let TurnStartResponse { turn } = to_response::<TurnStartResponse>(turn_resp)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    let additional_context = Some(HashMap::from([(
        "browser_info".to_string(),
        AdditionalContextEntry {
            value: "tab one".to_string(),
            kind: AdditionalContextKind::Untrusted,
            source: None,
        },
    )]));
    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: Vec::new(),
            responsesapi_client_metadata: None,
            additional_context,
            expected_turn_id: turn.id,
        })
        .await?;
    let steer_error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(steer_req)),
    )
    .await??;
    assert_eq!(steer_error.error.code, -32600);
    assert_eq!(steer_error.error.message, "input must not be empty");

    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = server
        .received_requests()
        .await
        .context("failed to fetch received requests")?;
    let response_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(response_requests.len(), 2);
    let body = response_requests[1]
        .body_json::<Value>()
        .context("request body should be JSON")?;
    assert!(
        !body
            .to_string()
            .contains("<external_browser_info>tab one</external_browser_info>")
    );

    Ok(())
}
