use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_fake_rollout;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::rollout_path;
use app_test_support::to_response;
use chrono::DateTime;
use chrono::Utc;
use codex_app_server_protocol::ActiveSessionMessageDelivery;
use codex_app_server_protocol::ActiveSessionSendParams;
use codex_app_server_protocol::ActiveSessionSendResponse;
use codex_app_server_protocol::ActiveSessionSendStatus;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ThreadMailboxAckParams;
use codex_app_server_protocol::ThreadMailboxAckResponse;
use codex_app_server_protocol::ThreadMailboxClaimParams;
use codex_app_server_protocol::ThreadMailboxClaimResponse;
use codex_app_server_protocol::ThreadMailboxEnqueueParams;
use codex_app_server_protocol::ThreadMailboxEnqueueResponse;
use codex_app_server_protocol::ThreadMailboxFailDisposition;
use codex_app_server_protocol::ThreadMailboxFailParams;
use codex_app_server_protocol::ThreadMailboxFailResponse;
use codex_app_server_protocol::ThreadMailboxListParams;
use codex_app_server_protocol::ThreadMailboxListResponse;
use codex_app_server_protocol::ThreadMailboxMessageKind;
use codex_app_server_protocol::ThreadMailboxMessageStatus;
use codex_app_server_protocol::ThreadMailboxMessageSummary;
use codex_app_server_protocol::ThreadMailboxReadParams;
use codex_app_server_protocol::ThreadMailboxReadResponse;
use codex_app_server_protocol::ThreadMailboxReceiptKind;
use codex_app_server_protocol::ThreadMailboxReceiptsListParams;
use codex_app_server_protocol::ThreadMailboxReceiptsListResponse;
use codex_app_server_protocol::ThreadMailboxRedaction;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_core::context::MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES;
use codex_core::context::MAX_MAILBOX_STORED_PAYLOAD_BYTES;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::SessionSource;
use codex_state::StateRuntime;
use codex_state::ThreadMetadataBuilder;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::time::Instant;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn thread_mailbox_enqueue_list_read_claim_ack_and_receipts() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;

    let first = enqueue_message(&mut mcp, &thread_id, "daily-thought-1").await?;
    let second = enqueue_message(&mut mcp, &thread_id, "daily-thought-1").await?;
    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.message.message_id, second.message.message_id);
    assert_eq!(first.message.status, ThreadMailboxMessageStatus::Queued);
    assert_eq!(
        first.message.redactions,
        vec![
            ThreadMailboxRedaction::MessageBody,
            ThreadMailboxRedaction::IdempotencyKey,
        ]
    );

    let listed = list_messages(&mut mcp, &thread_id).await?;
    assert_eq!(listed.next_cursor, None);
    assert_eq!(listed.data, vec![first.message.clone()]);

    let read = read_message(&mut mcp, &thread_id, &first.message.message_id).await?;
    assert_eq!(read.message.summary, first.message);
    assert_eq!(read.message.message, json!({ "text": "decompose this" }));

    let claim = claim_message(&mut mcp, &thread_id).await?;
    let claim = claim.claim.expect("mailbox message should be claimed");
    assert_eq!(
        claim.message.summary.status,
        ThreadMailboxMessageStatus::Claimed
    );
    assert_eq!(claim.message.message, json!({ "text": "decompose this" }));
    assert_eq!(claim.attempt.attempt_number, 1);

    let ack = ack_message(&mut mcp, &thread_id, &claim).await?;
    assert_eq!(ack.message.status, ThreadMailboxMessageStatus::Acknowledged);

    let receipts = list_receipts(&mut mcp, &thread_id, &ack.message.message_id).await?;
    assert_eq!(receipts.data.len(), 3);
    assert_eq!(
        receipts
            .data
            .into_iter()
            .map(|receipt| receipt.kind)
            .collect::<Vec<_>>(),
        vec![
            codex_app_server_protocol::ThreadMailboxReceiptKind::Enqueued,
            codex_app_server_protocol::ThreadMailboxReceiptKind::Claimed,
            codex_app_server_protocol::ThreadMailboxReceiptKind::Acknowledged,
        ]
    );

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_enqueue_rejects_oversized_context_payload() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let err = enqueue_message_with_payload_error(
        &mut mcp,
        &thread_id,
        json!({ "text": "x".repeat(MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES + 1) }),
    )
    .await?;

    assert_eq!(
        err.error.message,
        format!(
            "mailbox message rendered for model context must not exceed {MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES} bytes"
        )
    );
    assert_eq!(list_messages(&mut mcp, &thread_id).await?.data, Vec::new());

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_enqueue_rejects_oversized_stored_payload() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let err = enqueue_message_with_payload_error(
        &mut mcp,
        &thread_id,
        json!({
            "text": "small",
            "metadata": "x".repeat(MAX_MAILBOX_STORED_PAYLOAD_BYTES + 1),
        }),
    )
    .await?;

    assert_eq!(
        err.error.message,
        format!("mailbox message payload must not exceed {MAX_MAILBOX_STORED_PAYLOAD_BYTES} bytes")
    );
    assert_eq!(list_messages(&mut mcp, &thread_id).await?.data, Vec::new());

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_fail_does_not_leak_raw_error_in_summaries() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let enqueued = enqueue_message(&mut mcp, &thread_id, "leaky-error").await?;
    let claim = claim_message(&mut mcp, &thread_id)
        .await?
        .claim
        .expect("mailbox message should be claimed");
    let raw_error = "RAW_WORKFLOW_SECRET_SHOULD_NOT_LEAK";

    let failed = fail_message(&mut mcp, &thread_id, &claim, raw_error).await?;
    assert_eq!(failed.message.status, ThreadMailboxMessageStatus::Failed);
    assert_eq!(failed.message.last_error, None);
    assert!(!serde_json::to_string(&failed)?.contains(raw_error));

    let listed = list_messages(&mut mcp, &thread_id).await?;
    assert_eq!(listed.data.len(), 1);
    assert_eq!(listed.data[0].message_id, enqueued.message.message_id);
    assert_eq!(listed.data[0].last_error, None);
    assert!(!serde_json::to_string(&listed)?.contains(raw_error));

    let read = read_message(&mut mcp, &thread_id, &enqueued.message.message_id).await?;
    assert_eq!(read.message.summary.last_error, None);
    assert!(!serde_json::to_string(&read)?.contains(raw_error));

    Ok(())
}

#[tokio::test]
async fn active_session_offline_send_does_not_create_mailbox_message() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let thread_id = {
        let mut mcp = init_mcp(codex_home.path()).await?;
        start_thread(&mut mcp).await?
    };

    let mut mcp = init_mcp(codex_home.path()).await?;
    let send_id = mcp
        .send_active_session_send_request(ActiveSessionSendParams {
            target_thread_id: Some(thread_id.clone()),
            target_peer_id: None,
            message: "hello offline peer".to_string(),
            sender_thread_id: None,
            sender_label: None,
            delivery: Some(ActiveSessionMessageDelivery::TriggerTurn),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(send_id)),
    )
    .await??;
    let send_response = to_response::<ActiveSessionSendResponse>(resp)?;
    assert_eq!(send_response.status, ActiveSessionSendStatus::NotLoaded);

    let listed = list_messages(&mut mcp, &thread_id).await?;
    assert_eq!(listed.data, Vec::new());

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_delivers_due_message_to_loaded_thread() -> Result<()> {
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
    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
    )?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let enqueued = enqueue_auto_dispatch_message_with_sender(
        &mut mcp,
        &thread_id,
        "dispatcher-live",
        Some(thread_id.clone()),
    )
    .await?;

    let acknowledged = wait_for_message_matching(
        &mut mcp,
        &thread_id,
        &enqueued.message.message_id,
        |message| message.status == ThreadMailboxMessageStatus::Acknowledged,
    )
    .await?;
    assert_eq!(acknowledged.attempt_count, 1);
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 2).await?;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    let messages = inter_agent_messages_in_request(&requests[1].body_json());
    assert_eq!(
        messages,
        vec![InterAgentCommunication {
            author: codex_protocol::AgentPath::root(),
            recipient: codex_protocol::AgentPath::root(),
            other_recipients: Vec::new(),
            content: format!(
                "Durable mailbox message {} from unverified sender thread {} with label \"coordinator\":\n\ndecompose this",
                enqueued.message.message_id, thread_id
            ),
            encrypted_content: None,
            trigger_turn: true,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_delivers_to_live_thread_in_separate_app_server_process()
-> Result<()> {
    let server = responses::start_mock_server().await;
    let first_body = responses::sse(vec![
        responses::ev_response_created("resp-cross-process-seed"),
        responses::ev_assistant_message("msg-cross-process-seed", "Seed done"),
        responses::ev_completed("resp-cross-process-seed"),
    ]);
    let second_body = responses::sse(vec![
        responses::ev_response_created("resp-cross-process-mailbox"),
        responses::ev_assistant_message("msg-cross-process-mailbox", "Mailbox done"),
        responses::ev_completed("resp-cross-process-mailbox"),
    ]);
    let response_mock = responses::mount_sse_sequence(&server, vec![first_body, second_body]).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
    )?;

    let mut target_mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut target_mcp).await?;
    wait_for_fresh_local_active_session(codex_home.path(), &thread_id).await?;

    let mut sender_mcp = init_mcp(codex_home.path()).await?;
    let enqueued =
        enqueue_auto_dispatch_message(&mut sender_mcp, &thread_id, "dispatcher-cross-process")
            .await?;
    let acknowledged = wait_for_message_matching(
        &mut sender_mcp,
        &thread_id,
        &enqueued.message.message_id,
        |message| message.status == ThreadMailboxMessageStatus::Acknowledged,
    )
    .await?;
    assert_eq!(acknowledged.attempt_count, 1);
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 2).await?;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    let messages = inter_agent_messages_in_request(&requests[1].body_json());
    assert_eq!(
        messages,
        vec![InterAgentCommunication {
            author: codex_protocol::AgentPath::root(),
            recipient: codex_protocol::AgentPath::root(),
            other_recipients: Vec::new(),
            content: format!(
                "Durable mailbox message {} from external sender \"coordinator\":\n\ndecompose this",
                enqueued.message.message_id
            ),
            encrypted_content: None,
            trigger_turn: true,
        }]
    );

    let receipts = list_receipts(&mut sender_mcp, &thread_id, &enqueued.message.message_id).await?;
    let acknowledged_receipt = receipts
        .data
        .iter()
        .find(|receipt| receipt.kind == ThreadMailboxReceiptKind::Acknowledged)
        .expect("acknowledged receipt");
    assert_eq!(
        acknowledged_receipt
            .payload
            .as_ref()
            .and_then(|payload| payload.get("delivery")),
        Some(&json!("live"))
    );
    assert_eq!(
        acknowledged_receipt
            .payload
            .as_ref()
            .and_then(|payload| payload.get("triggerTurn")),
        Some(&json!(true))
    );

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_does_not_steal_live_target_from_dispatch_disabled_process()
-> Result<()> {
    let server = responses::start_mock_server().await;
    let seed_body = responses::sse(vec![
        responses::ev_response_created("resp-disabled-target-seed"),
        responses::ev_assistant_message("msg-disabled-target-seed", "Seed done"),
        responses::ev_completed("resp-disabled-target-seed"),
    ]);
    let response_mock = responses::mount_sse_once(&server, seed_body).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ false,
    )?;

    let mut target_mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut target_mcp).await?;

    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
    )?;
    let mut sender_mcp = init_mcp(codex_home.path()).await?;
    let enqueued = enqueue_auto_dispatch_message_with_max_attempts(
        &mut sender_mcp,
        &thread_id,
        "dispatcher-disabled-foreign-owner",
        /*max_attempts*/ 1,
    )
    .await?;
    sleep(std::time::Duration::from_secs(2)).await;

    let read = read_message(&mut sender_mcp, &thread_id, &enqueued.message.message_id).await?;
    assert_eq!(
        read.message.summary.status,
        ThreadMailboxMessageStatus::Queued
    );
    assert_eq!(read.message.summary.attempt_count, 0);
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 1).await?;

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_resumes_unloaded_thread_when_requested() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_body = responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-2", "Done"),
        responses::ev_completed("resp-2"),
    ]);
    let response_mock = responses::mount_sse_once(&server, response_body).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
    )?;

    let thread_id = seed_unloaded_thread(codex_home.path()).await?;
    let mut mcp = init_mcp(codex_home.path()).await?;
    let enqueued = enqueue_message_with_payload_and_max_attempts(
        &mut mcp,
        &thread_id,
        "dispatcher-resume",
        json!({
            "text": "decompose this",
            "delivery": "resumeAndTrigger",
        }),
        /*max_attempts*/ 3,
    )
    .await?;

    let acknowledged = wait_for_message_matching(
        &mut mcp,
        &thread_id,
        &enqueued.message.message_id,
        |message| message.status == ThreadMailboxMessageStatus::Acknowledged,
    )
    .await?;
    assert_eq!(acknowledged.attempt_count, 1);
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 1).await?;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 1);
    let messages = inter_agent_messages_in_request(&requests[0].body_json());
    assert_eq!(
        messages,
        vec![InterAgentCommunication {
            author: codex_protocol::AgentPath::root(),
            recipient: codex_protocol::AgentPath::root(),
            other_recipients: Vec::new(),
            content: format!(
                "Durable mailbox message {} from external sender \"coordinator\":\n\ndecompose this",
                enqueued.message.message_id
            ),
            encrypted_content: None,
            trigger_turn: true,
        }]
    );

    let receipts = list_receipts(&mut mcp, &thread_id, &enqueued.message.message_id).await?;
    let acknowledged_receipt = receipts
        .data
        .iter()
        .find(|receipt| receipt.kind == ThreadMailboxReceiptKind::Acknowledged)
        .expect("acknowledged receipt");
    assert_eq!(
        acknowledged_receipt
            .payload
            .as_ref()
            .and_then(|payload| payload.get("delivery")),
        Some(&json!("resumed"))
    );
    assert_eq!(
        acknowledged_receipt
            .payload
            .as_ref()
            .and_then(|payload| payload.get("triggerTurn")),
        Some(&json!(true))
    );

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_resume_preserves_persisted_permissions() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_body = responses::sse(vec![
        responses::ev_response_created("resp-permission-resume"),
        responses::ev_assistant_message("msg-permission-resume", "Done"),
        responses::ev_completed("resp-permission-resume"),
    ]);
    let response_mock = responses::mount_sse_once(&server, response_body).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_mailbox_dispatcher_and_sandbox(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
        "danger-full-access",
    )?;

    let persisted_permission_profile = PermissionProfile::read_only();
    let persisted_sandbox_policy = serde_json::to_string(&persisted_permission_profile)?;
    let thread_id = seed_unloaded_thread_with_persisted_sandbox_policy(
        codex_home.path(),
        Some(persisted_sandbox_policy),
    )
    .await?;
    let mut mcp = init_mcp(codex_home.path()).await?;
    let enqueued = enqueue_message_with_payload_and_max_attempts(
        &mut mcp,
        &thread_id,
        "dispatcher-resume-permissions",
        json!({
            "text": "decompose this",
            "delivery": "resumeAndTrigger",
        }),
        /*max_attempts*/ 3,
    )
    .await?;

    let acknowledged = wait_for_message_matching(
        &mut mcp,
        &thread_id,
        &enqueued.message.message_id,
        |message| message.status == ThreadMailboxMessageStatus::Acknowledged,
    )
    .await?;
    assert_eq!(acknowledged.attempt_count, 1);
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 1).await?;

    let state_db =
        StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into()).await?;
    let thread_metadata = state_db
        .get_thread(ThreadId::from_string(&thread_id)?)
        .await?
        .expect("thread metadata should persist");
    let resumed_permission_profile: PermissionProfile =
        serde_json::from_str(&thread_metadata.sandbox_policy)?;
    assert_eq!(persisted_permission_profile, resumed_permission_profile);

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_resumes_unloaded_thread_by_default() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_body = responses::sse(vec![
        responses::ev_response_created("resp-default-dispatch"),
        responses::ev_assistant_message("msg-default-dispatch", "Done"),
        responses::ev_completed("resp-default-dispatch"),
    ]);
    let response_mock = responses::mount_sse_once(&server, response_body).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_default_features(codex_home.path(), &server.uri())?;

    let thread_id = seed_unloaded_thread(codex_home.path()).await?;
    let mut mcp = init_mcp(codex_home.path()).await?;
    let enqueued = enqueue_message_with_payload_and_max_attempts(
        &mut mcp,
        &thread_id,
        "dispatcher-resume-default",
        json!({
            "text": "decompose this",
            "delivery": "resumeAndTrigger",
        }),
        /*max_attempts*/ 3,
    )
    .await?;

    let acknowledged = wait_for_message_matching(
        &mut mcp,
        &thread_id,
        &enqueued.message.message_id,
        |message| message.status == ThreadMailboxMessageStatus::Acknowledged,
    )
    .await?;
    assert_eq!(acknowledged.attempt_count, 1);
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 1).await?;

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_leaves_plain_rows_for_manual_claim_by_default() -> Result<()> {
    let server = responses::start_mock_server().await;
    let seed_body = responses::sse(vec![
        responses::ev_response_created("resp-manual-seed"),
        responses::ev_assistant_message("msg-manual-seed", "Seed done"),
        responses::ev_completed("resp-manual-seed"),
    ]);
    let response_mock = responses::mount_sse_once(&server, seed_body).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_default_features(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let enqueued = enqueue_message(&mut mcp, &thread_id, "manual-default").await?;
    sleep(std::time::Duration::from_secs(2)).await;

    let read = read_message(&mut mcp, &thread_id, &enqueued.message.message_id).await?;
    assert_eq!(
        read.message.summary.status,
        ThreadMailboxMessageStatus::Queued
    );
    assert_eq!(read.message.summary.attempt_count, 0);

    let claim = claim_message(&mut mcp, &thread_id)
        .await?
        .claim
        .expect("plain mailbox row should remain manually claimable");
    assert_eq!(
        claim.message.summary.message_id,
        enqueued.message.message_id
    );
    wait_for_response_mock_request_count(&response_mock, /*expected_count*/ 1).await?;

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_queues_trigger_turn_while_target_busy() -> Result<()> {
    let server = responses::start_mock_server().await;
    let first_body = create_request_user_input_body("call-busy")?;
    let second_body = responses::sse(vec![
        responses::ev_response_created("resp-busy-final"),
        responses::ev_assistant_message("msg-busy-final", "First turn done"),
        responses::ev_completed("resp-busy-final"),
    ]);
    let third_body = responses::sse(vec![
        responses::ev_response_created("resp-mailbox"),
        responses::ev_assistant_message("msg-mailbox", "Mailbox turn done"),
        responses::ev_completed("resp-mailbox"),
    ]);
    let response_mock =
        responses::mount_sse_sequence(&server, vec![first_body, second_body, third_body]).await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
    )?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread_without_turn(&mut mcp).await?;
    let turn_start_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.clone(),
            client_user_message_id: None,
            input: vec![UserInput::Text {
                text: "ask for input".to_string(),
                text_elements: Vec::new(),
            }],
            collaboration_mode: Some(CollaborationMode {
                mode: ModeKind::Plan,
                settings: Settings {
                    model: "mock-model".to_string(),
                    reasoning_effort: None,
                    developer_instructions: None,
                },
            }),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_start_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response(turn_resp)?;

    let server_req = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_request_message(),
    )
    .await??;
    let ServerRequest::ToolRequestUserInput { request_id, .. } = server_req else {
        anyhow::bail!("expected ToolRequestUserInput request, got: {server_req:?}");
    };

    let enqueued = enqueue_auto_dispatch_message(&mut mcp, &thread_id, "dispatcher-busy").await?;
    let acknowledged = wait_for_message_matching(
        &mut mcp,
        &thread_id,
        &enqueued.message.message_id,
        |message| message.status == ThreadMailboxMessageStatus::Acknowledged,
    )
    .await?;
    assert_eq!(acknowledged.attempt_count, 1);
    assert_eq!(response_mock.requests().len(), 1);

    mcp.send_response(
        request_id,
        json!({
            "answers": {
                "confirm_path": { "answers": ["yes"] }
            }
        }),
    )
    .await?;

    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 3);
    let messages = inter_agent_messages_in_request(&requests[2].body_json());
    assert_eq!(
        messages,
        vec![InterAgentCommunication {
            author: codex_protocol::AgentPath::root(),
            recipient: codex_protocol::AgentPath::root(),
            other_recipients: Vec::new(),
            content: format!(
                "Durable mailbox message {} from external sender \"coordinator\":\n\ndecompose this",
                enqueued.message.message_id
            ),
            encrypted_content: None,
            trigger_turn: true,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn thread_mailbox_dispatcher_retries_and_poisons_offline_targets() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml_with_mailbox_dispatcher(
        codex_home.path(),
        &server.uri(),
        /*mailbox_dispatcher_enabled*/ true,
    )?;

    let (retry_thread_id, poison_thread_id) = {
        let mut setup_mcp = init_mcp(codex_home.path()).await?;
        let retry_thread_id = start_thread(&mut setup_mcp).await?;
        let poison_thread_id = start_thread(&mut setup_mcp).await?;
        (retry_thread_id, poison_thread_id)
    };

    let mut mcp = init_mcp(codex_home.path()).await?;
    let retry = enqueue_auto_dispatch_message_with_max_attempts(
        &mut mcp,
        &retry_thread_id,
        "dispatcher-retry",
        /*max_attempts*/ 2,
    )
    .await?;
    let poison = enqueue_auto_dispatch_message_with_max_attempts(
        &mut mcp,
        &poison_thread_id,
        "dispatcher-poison",
        /*max_attempts*/ 1,
    )
    .await?;

    let retried = wait_for_message_matching(
        &mut mcp,
        &retry_thread_id,
        &retry.message.message_id,
        |message| {
            message.status == ThreadMailboxMessageStatus::Queued && message.attempt_count == 1
        },
    )
    .await?;
    assert_eq!(retried.last_error, None);

    let poisoned = wait_for_message_matching(
        &mut mcp,
        &poison_thread_id,
        &poison.message.message_id,
        |message| {
            message.status == ThreadMailboxMessageStatus::Poisoned && message.attempt_count == 1
        },
    )
    .await?;
    assert_eq!(poisoned.last_error, None);

    Ok(())
}

async fn init_mcp(codex_home: &Path) -> Result<TestAppServer> {
    let mut mcp = TestAppServer::new(codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
}

async fn enqueue_message(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_message_with_max_attempts(mcp, thread_id, idempotency_key, /*max_attempts*/ 3).await
}

async fn enqueue_message_with_max_attempts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
    max_attempts: u32,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_message_with_sender_and_payload_and_max_attempts(
        mcp,
        thread_id,
        idempotency_key,
        /*sender_thread_id*/ None,
        json!({ "text": "decompose this" }),
        max_attempts,
    )
    .await
}

async fn enqueue_auto_dispatch_message(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_auto_dispatch_message_with_max_attempts(
        mcp,
        thread_id,
        idempotency_key,
        /*max_attempts*/ 3,
    )
    .await
}

async fn enqueue_auto_dispatch_message_with_max_attempts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
    max_attempts: u32,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_auto_dispatch_message_with_sender_and_max_attempts(
        mcp,
        thread_id,
        idempotency_key,
        /*sender_thread_id*/ None,
        max_attempts,
    )
    .await
}

async fn enqueue_auto_dispatch_message_with_sender(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
    sender_thread_id: Option<String>,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_auto_dispatch_message_with_sender_and_max_attempts(
        mcp,
        thread_id,
        idempotency_key,
        sender_thread_id,
        /*max_attempts*/ 3,
    )
    .await
}

async fn enqueue_auto_dispatch_message_with_sender_and_max_attempts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
    sender_thread_id: Option<String>,
    max_attempts: u32,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_message_with_sender_and_payload_and_max_attempts(
        mcp,
        thread_id,
        idempotency_key,
        sender_thread_id,
        json!({ "text": "decompose this", "delivery": "liveOnly" }),
        max_attempts,
    )
    .await
}

async fn enqueue_message_with_payload_and_max_attempts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
    payload: Value,
    max_attempts: u32,
) -> Result<ThreadMailboxEnqueueResponse> {
    enqueue_message_with_sender_and_payload_and_max_attempts(
        mcp,
        thread_id,
        idempotency_key,
        /*sender_thread_id*/ None,
        payload,
        max_attempts,
    )
    .await
}

async fn enqueue_message_with_sender_and_payload_and_max_attempts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    idempotency_key: &str,
    sender_thread_id: Option<String>,
    payload: Value,
    max_attempts: u32,
) -> Result<ThreadMailboxEnqueueResponse> {
    let request_id = mcp
        .send_thread_mailbox_enqueue_request(ThreadMailboxEnqueueParams {
            target_thread_id: thread_id.to_string(),
            sender_thread_id,
            sender_label: Some("coordinator".to_string()),
            idempotency_key: Some(idempotency_key.to_string()),
            kind: ThreadMailboxMessageKind::UserInstruction,
            message: payload,
            preview: Some("decompose this".to_string()),
            priority: Some(5),
            max_attempts: Some(max_attempts),
            next_attempt_at: None,
            expires_at: None,
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxEnqueueResponse>(resp)
}

async fn enqueue_message_with_payload_error(
    mcp: &mut TestAppServer,
    thread_id: &str,
    payload: Value,
) -> Result<JSONRPCError> {
    let request_id = mcp
        .send_thread_mailbox_enqueue_request(ThreadMailboxEnqueueParams {
            target_thread_id: thread_id.to_string(),
            sender_thread_id: None,
            sender_label: Some("coordinator".to_string()),
            idempotency_key: Some("oversized-context-payload".to_string()),
            kind: ThreadMailboxMessageKind::UserInstruction,
            message: payload,
            preview: Some("oversized".to_string()),
            priority: Some(5),
            max_attempts: Some(3),
            next_attempt_at: None,
            expires_at: None,
        })
        .await?;
    let err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(err)
}

async fn wait_for_message_matching(
    mcp: &mut TestAppServer,
    thread_id: &str,
    message_id: &str,
    predicate: impl Fn(&ThreadMailboxMessageSummary) -> bool,
) -> Result<ThreadMailboxMessageSummary> {
    let started_at = Instant::now();
    loop {
        let read = read_message(mcp, thread_id, message_id).await?;
        if predicate(&read.message.summary) {
            return Ok(read.message.summary);
        }
        if started_at.elapsed() > DEFAULT_READ_TIMEOUT {
            anyhow::bail!(
                "timed out waiting for mailbox message {message_id} to match predicate; last status {:?}",
                read.message.summary.status
            );
        }
        sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn wait_for_response_mock_request_count(
    response_mock: &responses::ResponseMock,
    expected_count: usize,
) -> Result<()> {
    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let request_count = response_mock.requests().len();
            if request_count == expected_count {
                return Ok::<(), anyhow::Error>(());
            }
            if request_count > expected_count {
                anyhow::bail!(
                    "expected exactly {expected_count} response requests, got {request_count}"
                );
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

async fn list_messages(
    mcp: &mut TestAppServer,
    thread_id: &str,
) -> Result<ThreadMailboxListResponse> {
    let request_id = mcp
        .send_thread_mailbox_list_request(ThreadMailboxListParams {
            target_thread_id: thread_id.to_string(),
            statuses: None,
            cursor: None,
            limit: None,
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxListResponse>(resp)
}

async fn read_message(
    mcp: &mut TestAppServer,
    thread_id: &str,
    message_id: &str,
) -> Result<ThreadMailboxReadResponse> {
    let request_id = mcp
        .send_thread_mailbox_read_request(ThreadMailboxReadParams {
            target_thread_id: thread_id.to_string(),
            message_id: message_id.to_string(),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxReadResponse>(resp)
}

async fn claim_message(
    mcp: &mut TestAppServer,
    thread_id: &str,
) -> Result<ThreadMailboxClaimResponse> {
    let request_id = mcp
        .send_thread_mailbox_claim_request(ThreadMailboxClaimParams {
            target_thread_id: thread_id.to_string(),
            lease_owner: Some("test-dispatcher".to_string()),
            lease_seconds: Some(600),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxClaimResponse>(resp)
}

async fn ack_message(
    mcp: &mut TestAppServer,
    thread_id: &str,
    claim: &codex_app_server_protocol::ThreadMailboxClaim,
) -> Result<ThreadMailboxAckResponse> {
    let request_id = mcp
        .send_thread_mailbox_ack_request(ThreadMailboxAckParams {
            target_thread_id: thread_id.to_string(),
            message_id: claim.message.summary.message_id.clone(),
            attempt_id: claim.attempt.attempt_id.clone(),
            lease_id: claim.attempt.lease_id.clone(),
            receipt: Some(json!({ "handled": true })),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxAckResponse>(resp)
}

async fn fail_message(
    mcp: &mut TestAppServer,
    thread_id: &str,
    claim: &codex_app_server_protocol::ThreadMailboxClaim,
    error: &str,
) -> Result<ThreadMailboxFailResponse> {
    let request_id = mcp
        .send_thread_mailbox_fail_request(ThreadMailboxFailParams {
            target_thread_id: thread_id.to_string(),
            message_id: claim.message.summary.message_id.clone(),
            attempt_id: claim.attempt.attempt_id.clone(),
            lease_id: claim.attempt.lease_id.clone(),
            disposition: ThreadMailboxFailDisposition::Terminal,
            error: error.to_string(),
            retry_at: None,
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxFailResponse>(resp)
}

async fn list_receipts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    message_id: &str,
) -> Result<ThreadMailboxReceiptsListResponse> {
    let request_id = mcp
        .send_thread_mailbox_receipts_list_request(ThreadMailboxReceiptsListParams {
            target_thread_id: thread_id.to_string(),
            message_id: message_id.to_string(),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadMailboxReceiptsListResponse>(resp)
}

async fn start_thread(mcp: &mut TestAppServer) -> Result<String> {
    let thread_id = start_thread_without_turn(mcp).await?;
    let turn_start_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.clone(),
            client_user_message_id: None,
            input: vec![UserInput::Text {
                text: format!("seed mailbox {thread_id}"),
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
    Ok(thread_id)
}

async fn start_thread_without_turn(mcp: &mut TestAppServer) -> Result<String> {
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

async fn wait_for_fresh_local_active_session(codex_home: &Path, thread_id: &str) -> Result<()> {
    let state_db = StateRuntime::init(codex_home.to_path_buf(), "mock_provider".into()).await?;
    let thread_id = ThreadId::from_string(thread_id)?;
    let thread_id_display = thread_id.to_string();
    let started_at = Instant::now();
    loop {
        let fresh_after = Utc::now() - chrono::Duration::seconds(5);
        if state_db
            .local_active_sessions()
            .get_fresh_session(thread_id, fresh_after)
            .await?
            .is_some()
        {
            return Ok(());
        }
        if started_at.elapsed() > DEFAULT_READ_TIMEOUT {
            anyhow::bail!("timed out waiting for fresh local active session {thread_id_display}");
        }
        sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn create_request_user_input_body(call_id: &str) -> Result<String> {
    let tool_call_arguments = serde_json::to_string(&json!({
        "questions": [{
            "id": "confirm_path",
            "header": "Confirm",
            "question": "Proceed with the plan?",
            "options": [{
                "label": "Yes (Recommended)",
                "description": "Continue the current plan."
            }, {
                "label": "No",
                "description": "Stop and revisit the approach."
            }]
        }]
    }))?;
    Ok(responses::sse(vec![
        responses::ev_response_created("resp-busy-request"),
        responses::ev_function_call(call_id, "request_user_input", &tool_call_arguments),
        responses::ev_completed("resp-busy-request"),
    ]))
}

async fn seed_unloaded_thread(codex_home: &Path) -> Result<String> {
    seed_unloaded_thread_with_persisted_sandbox_policy(codex_home, /*sandbox_policy*/ None).await
}

async fn seed_unloaded_thread_with_persisted_sandbox_policy(
    codex_home: &Path,
    sandbox_policy: Option<String>,
) -> Result<String> {
    const FILENAME_TS: &str = "2025-02-03T10-00-00";
    const META_RFC3339: &str = "2025-02-03T10:00:00Z";
    let thread_id = create_fake_rollout(
        codex_home,
        FILENAME_TS,
        META_RFC3339,
        "seed mailbox persisted thread",
        Some("mock_provider"),
        /*git_info*/ None,
    )?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.to_path_buf(), "mock_provider".into()).await?;
    let parsed_thread_id = ThreadId::from_string(&thread_id)?;
    let created_at = DateTime::parse_from_rfc3339(META_RFC3339)?.with_timezone(&Utc);
    let mut builder = ThreadMetadataBuilder::new(
        parsed_thread_id,
        rollout_path(codex_home, FILENAME_TS, &thread_id),
        created_at,
        SessionSource::Cli,
    );
    builder.updated_at = Some(created_at);
    builder.model_provider = Some("mock_provider".to_string());
    builder.cwd = codex_home.to_path_buf();
    builder.cli_version = Some("0.0.0".to_string());
    let mut metadata = builder.build("mock_provider");
    if let Some(sandbox_policy) = sandbox_policy {
        metadata.sandbox_policy = sandbox_policy;
    }
    state_db.upsert_thread(&metadata).await?;
    Ok(thread_id)
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    create_config_toml_with_mailbox_dispatcher(
        codex_home, server_uri, /*mailbox_dispatcher_enabled*/ false,
    )
}

fn create_config_toml_with_default_features(
    codex_home: &Path,
    server_uri: &str,
) -> std::io::Result<()> {
    write_config_toml(
        codex_home,
        server_uri,
        /*mailbox_dispatcher_enabled*/ None,
        "read-only",
    )
}

fn create_config_toml_with_mailbox_dispatcher(
    codex_home: &Path,
    server_uri: &str,
    mailbox_dispatcher_enabled: bool,
) -> std::io::Result<()> {
    create_config_toml_with_mailbox_dispatcher_and_sandbox(
        codex_home,
        server_uri,
        mailbox_dispatcher_enabled,
        "read-only",
    )
}

fn create_config_toml_with_mailbox_dispatcher_and_sandbox(
    codex_home: &Path,
    server_uri: &str,
    mailbox_dispatcher_enabled: bool,
    sandbox_mode: &str,
) -> std::io::Result<()> {
    write_config_toml(
        codex_home,
        server_uri,
        Some(mailbox_dispatcher_enabled),
        sandbox_mode,
    )
}

fn write_config_toml(
    codex_home: &Path,
    server_uri: &str,
    mailbox_dispatcher_enabled: Option<bool>,
    sandbox_mode: &str,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    let feature_config = mailbox_dispatcher_enabled
        .map(|enabled| format!("\n[features]\nmailbox_dispatcher = {enabled}\n"))
        .unwrap_or_default();
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "{sandbox_mode}"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#,
        ) + feature_config.as_str(),
    )
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
