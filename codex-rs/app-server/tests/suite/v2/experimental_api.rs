use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use codex_app_server_protocol::AskForApproval;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LocalSessionListParams;
use codex_app_server_protocol::MockExperimentalMethodParams;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadMailboxAckParams;
use codex_app_server_protocol::ThreadMailboxClaimParams;
use codex_app_server_protocol::ThreadMailboxEnqueueParams;
use codex_app_server_protocol::ThreadMailboxFailDisposition;
use codex_app_server_protocol::ThreadMailboxFailParams;
use codex_app_server_protocol::ThreadMailboxListParams;
use codex_app_server_protocol::ThreadMailboxMessageKind;
use codex_app_server_protocol::ThreadMailboxReadParams;
use codex_app_server_protocol::ThreadMailboxReceiptsListParams;
use codex_app_server_protocol::ThreadMemoryMode;
use codex_app_server_protocol::ThreadMemoryModeSetParams;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartTransport;
use codex_app_server_protocol::ThreadSettingsUpdateParams;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_protocol::protocol::RealtimeOutputModality;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn mock_experimental_method_requires_experimental_api_capability() -> Result<()> {
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

    let request_id = mcp
        .send_mock_experimental_method_request(MockExperimentalMethodParams::default())
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "mock/experimentalMethod");
    Ok(())
}

#[tokio::test]
async fn local_session_list_requires_experimental_api_capability() -> Result<()> {
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

    let request_id = mcp
        .send_local_session_list_request(LocalSessionListParams::default())
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "localSession/list");
    Ok(())
}

#[tokio::test]
async fn thread_mailbox_methods_require_experimental_api_capability() -> Result<()> {
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

    let thread_id = "00000000-0000-4000-8000-000000000001".to_string();
    let message_id = "mailbox-message".to_string();
    let attempt_id = "attempt".to_string();
    let lease_id = "lease".to_string();
    let requests = vec![
        (
            mcp.send_thread_mailbox_enqueue_request(ThreadMailboxEnqueueParams {
                target_thread_id: thread_id.clone(),
                sender_thread_id: None,
                sender_label: None,
                idempotency_key: Some("key".to_string()),
                kind: ThreadMailboxMessageKind::UserInstruction,
                message: json!({ "text": "hello" }),
                preview: None,
                priority: None,
                max_attempts: None,
                next_attempt_at: None,
                expires_at: None,
            })
            .await?,
            "thread/mailbox/enqueue",
        ),
        (
            mcp.send_thread_mailbox_list_request(ThreadMailboxListParams {
                target_thread_id: thread_id.clone(),
                statuses: None,
                cursor: None,
                limit: None,
            })
            .await?,
            "thread/mailbox/list",
        ),
        (
            mcp.send_thread_mailbox_read_request(ThreadMailboxReadParams {
                target_thread_id: thread_id.clone(),
                message_id: message_id.clone(),
            })
            .await?,
            "thread/mailbox/read",
        ),
        (
            mcp.send_thread_mailbox_claim_request(ThreadMailboxClaimParams {
                target_thread_id: thread_id.clone(),
                lease_owner: None,
                lease_seconds: None,
            })
            .await?,
            "thread/mailbox/claim",
        ),
        (
            mcp.send_thread_mailbox_ack_request(ThreadMailboxAckParams {
                target_thread_id: thread_id.clone(),
                message_id: message_id.clone(),
                attempt_id: attempt_id.clone(),
                lease_id: lease_id.clone(),
                receipt: None,
            })
            .await?,
            "thread/mailbox/ack",
        ),
        (
            mcp.send_thread_mailbox_fail_request(ThreadMailboxFailParams {
                target_thread_id: thread_id.clone(),
                message_id: message_id.clone(),
                attempt_id,
                lease_id,
                disposition: ThreadMailboxFailDisposition::Terminal,
                error: "failed".to_string(),
                retry_at: None,
            })
            .await?,
            "thread/mailbox/fail",
        ),
        (
            mcp.send_thread_mailbox_receipts_list_request(ThreadMailboxReceiptsListParams {
                target_thread_id: thread_id,
                message_id,
            })
            .await?,
            "thread/mailbox/receipts/list",
        ),
    ];

    for (request_id, method) in requests {
        let error = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_experimental_capability_error(error, method);
    }

    Ok(())
}

#[tokio::test]
async fn promoted_coordination_entrypoints_do_not_require_experimental_api_capability() -> Result<()>
{
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

    for (method, params) in [
        ("missionControl/overview", json!({})),
        (
            "worktree/list",
            json!({
                "baseRepoPath": null,
                "includeDeleted": null,
                "cursor": null,
                "limit": null,
            }),
        ),
    ] {
        let request_id = mcp.send_raw_request(method, Some(params)).await?;
        let _response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;
    }

    Ok(())
}

#[tokio::test]
async fn coordination_methods_require_experimental_api_capability() -> Result<()> {
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

    let thread_id = "00000000-0000-4000-8000-000000000001";
    let requests = vec![
        (
            mcp.send_raw_request(
                "thread/workflow/create",
                Some(json!({
                    "threadId": thread_id,
                    "yaml": "schema_version: workflow.codex.codewith/v0",
                })),
            )
            .await?,
            "thread/workflow/create",
        ),
        (
            mcp.send_raw_request(
                "thread/workflow/get",
                Some(json!({
                    "threadId": thread_id,
                    "workflowRecordId": "workflow-1",
                })),
            )
            .await?,
            "thread/workflow/get",
        ),
        (
            mcp.send_raw_request(
                "thread/workflow/list",
                Some(json!({
                    "threadId": thread_id,
                    "cursor": null,
                    "limit": null,
                })),
            )
            .await?,
            "thread/workflow/list",
        ),
        (
            mcp.send_raw_request(
                "thread/pendingInteraction/list",
                Some(json!({
                    "threadId": thread_id,
                    "statuses": null,
                    "kinds": null,
                    "cursor": null,
                    "limit": null,
                })),
            )
            .await?,
            "thread/pendingInteraction/list",
        ),
        (
            mcp.send_raw_request(
                "thread/pendingInteraction/read",
                Some(json!({
                    "interactionId": "interaction-1",
                    "threadId": thread_id,
                })),
            )
            .await?,
            "thread/pendingInteraction/read",
        ),
        (
            mcp.send_raw_request(
                "thread/pendingInteraction/respond",
                Some(json!({
                    "interactionId": "interaction-1",
                    "threadId": thread_id,
                    "terminalStatus": "responded",
                    "response": {
                        "type": "terminal",
                        "reason": "done",
                    },
                    "dryRun": true,
                })),
            )
            .await?,
            "thread/pendingInteraction/respond",
        ),
        (
            mcp.send_raw_request(
                "remoteDispatch/negotiate",
                Some(json!({
                    "sourceMachineId": "source",
                    "targetMachineId": "target",
                    "protocolVersion": 1,
                    "requestedCapabilities": null,
                })),
            )
            .await?,
            "remoteDispatch/negotiate",
        ),
        (
            mcp.send_raw_request(
                "remoteDispatch/submit",
                Some(json!({
                    "requestId": "request-1",
                    "sourceMachineId": "source",
                    "targetMachineId": "target",
                    "idempotencyKey": "idem-1",
                    "operation": {
                        "type": "enqueueInstruction",
                        "params": {
                            "targetThreadId": thread_id,
                            "message": "hello",
                            "senderThreadId": null,
                            "senderLabel": null,
                            "priority": null,
                            "maxAttempts": null,
                            "expiresAt": null,
                            "resume": false,
                        },
                    },
                    "requestedAt": null,
                    "expiresAt": null,
                    "capabilityVersion": null,
                    "dryRun": false,
                })),
            )
            .await?,
            "remoteDispatch/submit",
        ),
        (
            mcp.send_raw_request(
                "remoteDispatch/receipt/read",
                Some(json!({
                    "requestId": "request-1",
                    "idempotencyKey": null,
                    "sourceMachineId": "source",
                    "targetMachineId": "target",
                })),
            )
            .await?,
            "remoteDispatch/receipt/read",
        ),
        (
            mcp.send_raw_request(
                "machineRegistry/list",
                Some(json!({
                    "includeDisabled": false,
                    "includeForgotten": false,
                    "cursor": null,
                    "limit": null,
                })),
            )
            .await?,
            "machineRegistry/list",
        ),
        (
            mcp.send_raw_request(
                "machineRegistry/read",
                Some(json!({
                    "machineId": "machine-1",
                })),
            )
            .await?,
            "machineRegistry/read",
        ),
        (
            mcp.send_raw_request(
                "machineRegistry/upsert",
                Some(json!({
                    "machineId": "machine-1",
                    "installationId": null,
                    "displayName": "Machine One",
                    "capabilities": {},
                    "endpoints": [],
                    "lastSeenAt": null,
                })),
            )
            .await?,
            "machineRegistry/upsert",
        ),
        (
            mcp.send_raw_request(
                "machineRegistry/updateTrust",
                Some(json!({
                    "machineId": "machine-1",
                    "trustState": "trusted",
                })),
            )
            .await?,
            "machineRegistry/updateTrust",
        ),
        (
            mcp.send_raw_request(
                "machineRegistry/disable",
                Some(json!({
                    "machineId": "machine-1",
                })),
            )
            .await?,
            "machineRegistry/disable",
        ),
        (
            mcp.send_raw_request(
                "machineRegistry/forget",
                Some(json!({
                    "machineId": "machine-1",
                })),
            )
            .await?,
            "machineRegistry/forget",
        ),
    ];

    for (request_id, method) in requests {
        let error = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_experimental_capability_error(error, method);
    }

    Ok(())
}

#[tokio::test]
async fn realtime_conversation_start_requires_experimental_api_capability() -> Result<()> {
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

    let request_id = mcp
        .send_thread_realtime_start_request(ThreadRealtimeStartParams {
            thread_id: "thr_123".to_string(),
            output_modality: RealtimeOutputModality::Audio,
            prompt: Some(Some("hello".to_string())),
            realtime_session_id: None,
            transport: None,
            voice: None,
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "thread/realtime/start");
    Ok(())
}

#[tokio::test]
async fn thread_memory_mode_set_requires_experimental_api_capability() -> Result<()> {
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

    let request_id = mcp
        .send_thread_memory_mode_set_request(ThreadMemoryModeSetParams {
            thread_id: "thr_123".to_string(),
            mode: ThreadMemoryMode::Disabled,
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "thread/memoryMode/set");
    Ok(())
}

#[tokio::test]
async fn thread_settings_update_requires_experimental_api_capability() -> Result<()> {
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

    let request_id = mcp
        .send_thread_settings_update_request(ThreadSettingsUpdateParams {
            thread_id: "thr_123".to_string(),
            ..Default::default()
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "thread/settings/update");
    Ok(())
}

#[tokio::test]
async fn realtime_webrtc_start_requires_experimental_api_capability() -> Result<()> {
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

    let request_id = mcp
        .send_thread_realtime_start_request(ThreadRealtimeStartParams {
            thread_id: "thr_123".to_string(),
            output_modality: RealtimeOutputModality::Audio,
            prompt: Some(Some("hello".to_string())),
            realtime_session_id: None,
            transport: Some(ThreadRealtimeStartTransport::Webrtc {
                sdp: "v=offer\r\n".to_string(),
            }),
            voice: None,
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "thread/realtime/start");
    Ok(())
}

#[tokio::test]
async fn thread_start_mock_field_requires_experimental_api_capability() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

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

    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            mock_experimental_field: Some("mock".to_string()),
            ..Default::default()
        })
        .await?;

    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "thread/start.mockExperimentalField");
    Ok(())
}

#[tokio::test]
async fn thread_start_without_dynamic_tools_allows_without_experimental_api_capability()
-> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

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

    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadStartResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn thread_start_granular_approval_policy_requires_experimental_api_capability() -> Result<()>
{
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

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

    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            approval_policy: Some(AskForApproval::Granular {
                sandbox_approval: true,
                rules: false,
                skill_approval: false,
                request_permissions: true,
                mcp_elicitations: false,
            }),
            ..Default::default()
        })
        .await?;

    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_experimental_capability_error(error, "askForApproval.granular");
    Ok(())
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
