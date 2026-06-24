use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RemoteDispatchCapability;
use codex_app_server_protocol::RemoteDispatchDenialReason;
use codex_app_server_protocol::RemoteDispatchNegotiateResponse;
use codex_app_server_protocol::RemoteDispatchRequestStatus;
use codex_app_server_protocol::RemoteDispatchSubmitResponse;
use codex_app_server_protocol::RequestId;
use codex_protocol::ThreadId;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const THREAD_ID: &str = "00000000-0000-4000-8000-000000000301";

#[tokio::test]
async fn remote_dispatch_jsonrpc_enforces_trust_capabilities_and_expiry() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let state_db =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    upsert_machine(
        &state_db,
        "local",
        codex_state::MachineTrustState::Local,
        codex_state::MachineEnrollmentState::Local,
    )
    .await?;
    upsert_machine(
        &state_db,
        "trusted",
        codex_state::MachineTrustState::Trusted,
        codex_state::MachineEnrollmentState::Manual,
    )
    .await?;
    upsert_machine(
        &state_db,
        "untrusted",
        codex_state::MachineTrustState::Untrusted,
        codex_state::MachineEnrollmentState::Manual,
    )
    .await?;

    let negotiation: RemoteDispatchNegotiateResponse = send_and_read(
        &mut mcp,
        "remoteDispatch/negotiate",
        json!({
            "sourceMachineId": "trusted",
            "targetMachineId": "local",
            "protocolVersion": 1,
            "requestedCapabilities": null,
        }),
    )
    .await?;
    assert_eq!(Some("local".to_string()), negotiation.local_machine_id);
    assert!(!negotiation.supported_capabilities.is_empty());
    assert!(
        !negotiation
            .supported_capabilities
            .contains(&RemoteDispatchCapability::MailboxReceiptRead)
    );
    assert!(
        !negotiation
            .supported_capabilities
            .contains(&RemoteDispatchCapability::AuditReceipts)
    );

    let untrusted = remote_submit(
        &mut mcp,
        "untrusted",
        "local",
        /*capability_version*/ None,
        /*expires_at*/ None,
    )
    .await?;
    assert_eq!(RemoteDispatchRequestStatus::Denied, untrusted.status);
    assert_eq!(
        Some(RemoteDispatchDenialReason::UntrustedMachine),
        untrusted.receipt.denial.map(|denial| denial.reason)
    );

    let stale_capability = remote_submit(
        &mut mcp,
        "trusted",
        "local",
        Some("0"),
        /*expires_at*/ None,
    )
    .await?;
    assert_eq!(RemoteDispatchRequestStatus::Denied, stale_capability.status);
    assert_eq!(
        Some(RemoteDispatchDenialReason::CapabilityMismatch),
        stale_capability.receipt.denial.map(|denial| denial.reason)
    );

    let expired = remote_submit(
        &mut mcp,
        "trusted",
        "local",
        /*capability_version*/ None,
        Some(1),
    )
    .await?;
    assert_eq!(RemoteDispatchRequestStatus::Denied, expired.status);
    assert_eq!(
        Some(RemoteDispatchDenialReason::ExpiredRequest),
        expired.receipt.denial.map(|denial| denial.reason)
    );

    Ok(())
}

#[tokio::test]
async fn remote_dispatch_jsonrpc_rejects_respond_interaction_status_payload_mismatch() -> Result<()>
{
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let state_db =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    upsert_machine(
        &state_db,
        "local",
        codex_state::MachineTrustState::Local,
        codex_state::MachineEnrollmentState::Local,
    )
    .await?;
    upsert_machine(
        &state_db,
        "trusted",
        codex_state::MachineTrustState::Trusted,
        codex_state::MachineEnrollmentState::Manual,
    )
    .await?;
    upsert_thread(&state_db, THREAD_ID, "remote pending interaction").await?;
    create_user_input_interaction(&state_db, "interaction-remote-status-mismatch").await?;

    let request_id = mcp
        .send_raw_request(
            "remoteDispatch/submit",
            Some(json!({
                "requestId": "request-respond-mismatch",
                "sourceMachineId": "trusted",
                "targetMachineId": "local",
                "idempotencyKey": "idem-respond-mismatch",
                "operation": {
                    "type": "respondInteraction",
                    "params": {
                        "interactionId": "interaction-remote-status-mismatch",
                        "threadId": THREAD_ID,
                        "terminalStatus": "denied",
                        "response": {
                            "type": "requestUserInput",
                            "answers": {},
                        },
                    },
                },
                "requestedAt": null,
                "expiresAt": null,
                "capabilityVersion": null,
                "dryRun": true,
            })),
        )
        .await?;

    let err = read_error(&mut mcp, request_id).await?;
    assert!(
        err.error
            .message
            .contains("pending interaction terminalStatus must be responded"),
        "{err:?}"
    );
    let stored = state_db
        .get_thread_pending_interaction("interaction-remote-status-mismatch")
        .await?
        .expect("pending interaction should remain");
    assert_eq!(
        codex_state::PendingInteractionStatus::Pending,
        stored.status
    );
    Ok(())
}

async fn remote_submit(
    mcp: &mut TestAppServer,
    source_machine_id: &str,
    target_machine_id: &str,
    capability_version: Option<&str>,
    expires_at: Option<i64>,
) -> Result<RemoteDispatchSubmitResponse> {
    send_and_read(
        mcp,
        "remoteDispatch/submit",
        json!({
            "requestId": format!("request-{source_machine_id}-{target_machine_id}"),
            "sourceMachineId": source_machine_id,
            "targetMachineId": target_machine_id,
            "idempotencyKey": format!("idem-{source_machine_id}-{target_machine_id}"),
            "operation": {
                "type": "enqueueInstruction",
                "params": {
                    "targetThreadId": "00000000-0000-4000-8000-000000000201",
                    "message": "remote hello",
                    "senderThreadId": null,
                    "senderLabel": null,
                    "priority": null,
                    "maxAttempts": null,
                    "expiresAt": null,
                    "resume": false,
                },
            },
            "requestedAt": null,
            "expiresAt": expires_at,
            "capabilityVersion": capability_version,
            "dryRun": false,
        }),
    )
    .await
}

async fn upsert_machine(
    state_db: &StateRuntime,
    machine_id: &str,
    trust_state: codex_state::MachineTrustState,
    enrollment_state: codex_state::MachineEnrollmentState,
) -> Result<()> {
    state_db
        .machine_registry()
        .upsert_machine(codex_state::MachineRegistryUpsertParams {
            machine_id: Some(machine_id.to_string()),
            installation_id: None,
            display_name: Some(machine_id.to_string()),
            trust_state,
            enrollment_state,
            health_state: codex_state::MachineHealthState::Online,
            source_kind: codex_state::MachineSourceKind::Manual,
            adapter_name: None,
            capabilities_json: json!({}),
            endpoints: Vec::new(),
            last_seen_at: None,
        })
        .await?;
    Ok(())
}

async fn upsert_thread(state_db: &StateRuntime, thread_id: &str, preview: &str) -> Result<()> {
    let thread_id = ThreadId::from_string(thread_id)?;
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
        .ok_or_else(|| anyhow::anyhow!("timestamp should parse"))?;
    let codex_home = state_db.codex_home();
    state_db
        .upsert_thread(&codex_state::ThreadMetadata {
            id: thread_id,
            rollout_path: codex_home.join(format!("rollout-{thread_id}.jsonl")),
            created_at: now,
            updated_at: now,
            source: "cli".to_string(),
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test-provider".to_string(),
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            cwd: codex_home.join("workspace"),
            cli_version: "0.0.0".to_string(),
            title: String::new(),
            preview: Some(preview.to_string()),
            sandbox_policy: "read-only".to_string(),
            approval_mode: "on-request".to_string(),
            tokens_used: 0,
            first_user_message: Some(preview.to_string()),
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
        })
        .await?;
    Ok(())
}

async fn create_user_input_interaction(
    state_db: &StateRuntime,
    interaction_id: &str,
) -> Result<()> {
    state_db
        .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
            interaction_id: interaction_id.to_string(),
            thread_id: ThreadId::from_string(THREAD_ID)?,
            source_kind: codex_state::PendingInteractionSourceKind::Thread,
            source_id: None,
            turn_id: Some("turn-1".to_string()),
            worker_request_id: Some(interaction_id.to_string()),
            server_request_id_json: None,
            kind: codex_state::PendingInteractionKind::UserInput,
            request_payload_json: json!({
                "type": "requestUserInput",
                "questions": [{
                    "id": "decision",
                    "question": "Proceed?",
                }],
            }),
            request_payload_preview: "Proceed?".to_string(),
            request_redactions_json: json!([]),
            no_client_policy: "record-and-wait-for-coordinator".to_string(),
            timeout_at: None,
        })
        .await?;
    Ok(())
}

async fn send_and_read<T>(
    mcp: &mut TestAppServer,
    method: &str,
    params: serde_json::Value,
) -> Result<T>
where
    T: DeserializeOwned,
{
    let request_id = mcp.send_raw_request(method, Some(params)).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(response)
}

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCError> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}
