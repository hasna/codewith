use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use chrono::Duration as ChronoDuration;
use chrono::Utc;
use codex_app_server_protocol::AgentAttachParams;
use codex_app_server_protocol::AgentAttachResponse;
use codex_app_server_protocol::AgentDaemonDiagnosticsParams;
use codex_app_server_protocol::AgentDaemonDiagnosticsResponse;
use codex_app_server_protocol::AgentDeleteParams;
use codex_app_server_protocol::AgentDeleteResponse;
use codex_app_server_protocol::AgentDesiredState;
use codex_app_server_protocol::AgentDetachParams;
use codex_app_server_protocol::AgentDetachResponse;
use codex_app_server_protocol::AgentEventsListParams;
use codex_app_server_protocol::AgentEventsListResponse;
use codex_app_server_protocol::AgentExecutionContextParams;
use codex_app_server_protocol::AgentLifecycleEffect;
use codex_app_server_protocol::AgentListParams;
use codex_app_server_protocol::AgentListResponse;
use codex_app_server_protocol::AgentPendingInteractionRespondParams;
use codex_app_server_protocol::AgentPendingInteractionRespondResponse;
use codex_app_server_protocol::AgentPendingInteractionStatus;
use codex_app_server_protocol::AgentPendingInteractionTerminalStatus;
use codex_app_server_protocol::AgentReadParams;
use codex_app_server_protocol::AgentReadResponse;
use codex_app_server_protocol::AgentRetentionState;
use codex_app_server_protocol::AgentRunStatus;
use codex_app_server_protocol::AgentStartParams;
use codex_app_server_protocol::AgentStartResponse;
use codex_app_server_protocol::AgentStopParams;
use codex_app_server_protocol::AgentStopResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_state::BackgroundAgentDesiredState as StateBackgroundAgentDesiredState;
use codex_state::BackgroundAgentExecutionSnapshotParams;
use codex_state::BackgroundAgentPendingInteractionCreateParams;
use codex_state::BackgroundAgentPendingInteractionKind;
use codex_state::BackgroundAgentRunCreateParams;
use codex_state::BackgroundAgentRunStatus as StateBackgroundAgentRunStatus;
use codex_state::BackgroundAgentStatusSnapshotParams;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_list_read_and_events_survive_app_server_restart() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server =
        create_mock_responses_server_sequence(vec![create_final_assistant_message_sse_response(
            "background agent done",
        )?])
        .await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let start = start_agent(
        &mut mcp,
        start_params(
            "build the background agent plan",
            Some("durable-start-list-read".to_string()),
            codex_home.path(),
        ),
    )
    .await?;
    let agent_id = start.agent.agent_id.clone();

    assert_eq!(start.agent.status, AgentRunStatus::Queued);
    assert_eq!(start.agent.desired_state, AgentDesiredState::Running);
    assert_eq!(start.status_snapshot.status, AgentRunStatus::Queued);
    assert_eq!(
        start.execution_snapshot.snapshot_kind,
        "initial_execution_context"
    );
    assert_eq!(
        start.execution_snapshot.recovery_policy,
        "abort_mid_turn_resume_at_safe_boundary"
    );
    assert_eq!(
        start.execution_snapshot.payload.get("model"),
        Some(&json!("mock-model"))
    );
    assert_eq!(
        start.execution_snapshot.payload.get("provider"),
        Some(&json!("mock_provider"))
    );
    assert_eq!(
        start.execution_snapshot.payload.get("permissionProfile"),
        Some(&json!({"sandbox": "read-only"}))
    );
    assert_eq!(start.event.seq, 1);
    assert_eq!(start.event.event_type, "agent.started");
    assert_eq!(
        start.event.payload.get("prompt"),
        Some(&json!("build the background agent plan"))
    );
    let completed =
        wait_for_agent_status(&mut mcp, agent_id.as_str(), AgentRunStatus::Completed).await?;
    assert!(
        completed
            .agent
            .expect("completed agent")
            .thread_id
            .is_some()
    );
    let state_db = init_state_db(codex_home.path()).await?;
    state_db
        .append_background_agent_event(
            agent_id.as_str(),
            "agent.progress",
            &json!({"summary": "working"}),
        )
        .await?;

    let list = agent_list(&mut mcp).await?;
    assert_eq!(list.data.len(), 1);
    assert_eq!(list.data[0].agent_id, agent_id);

    drop(mcp);

    let mut restarted = init_mcp(codex_home.path()).await?;
    let read = agent_read(&mut restarted, &agent_id).await?;
    assert_eq!(read.agent.expect("agent after restart").agent_id, agent_id);
    let execution_snapshot = read
        .execution_snapshot
        .expect("execution snapshot after restart");
    assert_eq!(execution_snapshot.snapshot_kind, "worker_thread_bound");
    assert!(execution_snapshot.payload.get("threadId").is_some());
    assert_eq!(
        read.status_snapshot.expect("snapshot after restart").status,
        AgentRunStatus::Completed
    );

    let first_events_page = agent_events_page(&mut restarted, &agent_id, None, Some(1)).await?;
    assert_eq!(first_events_page.data.len(), 1);
    assert_eq!(first_events_page.data[0].event_type, "agent.started");
    assert_eq!(first_events_page.next_cursor, Some("event:1".to_string()));

    let second_events_page = agent_events_page(
        &mut restarted,
        &agent_id,
        first_events_page.next_cursor,
        Some(1),
    )
    .await?;
    assert_eq!(second_events_page.data.len(), 1);
    assert_eq!(
        second_events_page.data[0].event_type,
        "agent.workerStarting"
    );
    assert_eq!(second_events_page.next_cursor, Some("event:2".to_string()));
    let all_events = agent_events_page(&mut restarted, &agent_id, None, Some(20)).await?;
    let event_types = all_events
        .data
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"agent.workerRunning"));
    assert!(event_types.contains(&"agent.completed"));
    assert!(event_types.contains(&"agent.progress"));

    state_db
        .compact_background_agent_events_before_seq(agent_id.as_str(), /*before_seq*/ 3)
        .await?;
    let stale_cursor_error = agent_events_error(
        &mut restarted,
        &agent_id,
        Some("event:1".to_string()),
        Some(1),
    )
    .await?;
    assert_eq!(stale_cursor_error.error.code, -32600);
    assert!(
        stale_cursor_error
            .error
            .message
            .contains("background agent event cursor has been compacted"),
        "unexpected stale cursor error: {}",
        stale_cursor_error.error.message
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_periodically_starts_durable_queued_runs() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server =
        create_mock_responses_server_sequence(vec![create_final_assistant_message_sse_response(
            "periodic background agent done",
        )?])
        .await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let state_db = init_state_db(codex_home.path()).await?;
    let agent_id = "periodic-supervisor-run".to_string();
    seed_queued_agent_run(
        state_db.as_ref(),
        agent_id.as_str(),
        None,
        "picked up by periodic supervisor",
    )
    .await?;
    state_db
        .create_background_agent_execution_snapshot(&BackgroundAgentExecutionSnapshotParams {
            run_id: agent_id.clone(),
            snapshot_kind: "initial_execution_context".to_string(),
            payload_json: json!({
                "snapshotSource": "state-seeded-test",
                "cwd": codex_home.path().display().to_string(),
                "workspaceRoots": [codex_home.path().display().to_string()],
                "model": "mock-model",
                "provider": "mock_provider",
                "recoveryPolicy": "abort_mid_turn_resume_at_safe_boundary",
            }),
            recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
            config_fingerprint: Some("cfg-test".to_string()),
        })
        .await?;
    state_db
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: agent_id.clone(),
            seq: 1,
            status: StateBackgroundAgentRunStatus::Queued,
            desired_state: StateBackgroundAgentDesiredState::Running,
            summary: Some("Queued".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 1,
            payload_json: json!({"phase": "queued"}),
        })
        .await?;

    let completed =
        wait_for_agent_status(&mut mcp, agent_id.as_str(), AgentRunStatus::Completed).await?;
    let agent = completed.agent.expect("completed periodic agent");
    assert_eq!(agent.agent_id, agent_id);
    assert!(agent.thread_id.is_some());

    let events = agent_events_page(&mut mcp, agent_id.as_str(), None, Some(20)).await?;
    let event_types = events
        .data
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"agent.workerStarting"));
    assert!(event_types.contains(&"agent.workerRunning"));
    assert!(event_types.contains(&"agent.completed"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_lifecycle_and_pending_interaction_flow() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let state_db = init_state_db(codex_home.path()).await?;
    let agent_id = "lifecycle-run".to_string();
    seed_queued_agent_run(
        state_db.as_ref(),
        agent_id.as_str(),
        None,
        "wait for approval",
    )
    .await?;
    state_db
        .create_background_agent_execution_snapshot(&BackgroundAgentExecutionSnapshotParams {
            run_id: agent_id.clone(),
            snapshot_kind: "initial_execution_context".to_string(),
            payload_json: json!({
                "snapshotSource": "agent/start",
                "cwd": codex_home.path().display().to_string(),
                "sandboxPolicy": {"mode": "read-only"},
                "model": "mock-model",
                "provider": "mock_provider",
                "recoveryPolicy": "abort_mid_turn_resume_at_safe_boundary",
            }),
            recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
            config_fingerprint: Some("cfg-test".to_string()),
        })
        .await?;
    state_db
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: agent_id.clone(),
            seq: 1,
            status: StateBackgroundAgentRunStatus::Queued,
            desired_state: StateBackgroundAgentDesiredState::Running,
            summary: Some("Queued".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 1,
            payload_json: json!({"phase": "queued"}),
        })
        .await?;
    state_db
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "approval-1".to_string(),
                run_id: agent_id.clone(),
                worker_request_id: Some("worker-request-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({ "command": "deploy" }),
                no_client_policy: "deny".to_string(),
                timeout_at: None,
            },
        )
        .await?;
    state_db
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "expired-1".to_string(),
                run_id: agent_id.clone(),
                worker_request_id: Some("worker-request-expired-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::UserInput,
                request_payload_json: json!({ "prompt": "continue?" }),
                no_client_policy: "cancel".to_string(),
                timeout_at: Some(Utc::now() - ChronoDuration::seconds(1)),
            },
        )
        .await?;

    let attach = agent_attach(&mut mcp, &agent_id).await?;
    assert_eq!(attach.effect, AgentLifecycleEffect::ReplayState);
    assert_eq!(
        attach
            .execution_snapshot
            .expect("attach execution snapshot")
            .payload
            .get("sandboxPolicy"),
        Some(&json!({"mode": "read-only"}))
    );
    assert_eq!(attach.pending_interactions.len(), 2);
    let approval = attach
        .pending_interactions
        .iter()
        .find(|interaction| interaction.interaction_id == "approval-1")
        .expect("approval interaction should be replayed");
    assert_eq!(approval.status, AgentPendingInteractionStatus::Delivered);
    let expired = attach
        .pending_interactions
        .iter()
        .find(|interaction| interaction.interaction_id == "expired-1")
        .expect("expired interaction should be replayed");
    assert_eq!(expired.status, AgentPendingInteractionStatus::Expired);

    let expired_respond_id = mcp
        .send_agent_pending_interaction_respond_request(AgentPendingInteractionRespondParams {
            agent_id: agent_id.clone(),
            interaction_id: "expired-1".to_string(),
            response: json!({ "answer": "late" }),
            terminal_status: AgentPendingInteractionTerminalStatus::Responded,
        })
        .await?;
    let expired_respond: AgentPendingInteractionRespondResponse =
        read_response(&mut mcp, expired_respond_id).await?;
    assert!(!expired_respond.updated);
    assert_eq!(
        expired_respond
            .interaction
            .expect("expired interaction should be returned")
            .status,
        AgentPendingInteractionStatus::Expired
    );

    let respond_id = mcp
        .send_agent_pending_interaction_respond_request(AgentPendingInteractionRespondParams {
            agent_id: agent_id.clone(),
            interaction_id: "approval-1".to_string(),
            response: json!({ "approved": false }),
            terminal_status: AgentPendingInteractionTerminalStatus::Denied,
        })
        .await?;
    let respond: AgentPendingInteractionRespondResponse =
        read_response(&mut mcp, respond_id).await?;
    assert!(respond.updated);
    assert_eq!(
        respond.interaction.expect("responded interaction").status,
        AgentPendingInteractionStatus::Denied
    );

    let detach_id = mcp
        .send_agent_detach_request(AgentDetachParams {
            agent_id: agent_id.clone(),
        })
        .await?;
    let detach: AgentDetachResponse = read_response(&mut mcp, detach_id).await?;
    assert_eq!(detach.effect, AgentLifecycleEffect::RemoveSubscriberOnly);
    assert_eq!(
        detach.agent.expect("detached agent").desired_state,
        AgentDesiredState::Running
    );

    let stop_id = mcp
        .send_agent_stop_request(AgentStopParams {
            agent_id: agent_id.clone(),
        })
        .await?;
    let stop: AgentStopResponse = read_response(&mut mcp, stop_id).await?;
    let stopped_agent = stop.agent.expect("stopped agent");
    assert_eq!(stop.effect, AgentLifecycleEffect::RequestWorkerStop);
    assert_eq!(stopped_agent.desired_state, AgentDesiredState::Stopped);
    assert_eq!(stopped_agent.status, AgentRunStatus::Cancelled);
    assert_eq!(
        stopped_agent.status_reason.as_deref(),
        Some("stop requested before worker claim")
    );

    let delete_id = mcp
        .send_agent_delete_request(AgentDeleteParams {
            agent_id: agent_id.clone(),
        })
        .await?;
    let delete: AgentDeleteResponse = read_response(&mut mcp, delete_id).await?;
    let deleted_agent = delete.agent.expect("deleted agent");
    assert!(delete.deleted);
    assert_eq!(delete.effect, AgentLifecycleEffect::MarkDeleteRequested);
    assert_eq!(deleted_agent.desired_state, AgentDesiredState::Deleted);
    assert_eq!(
        deleted_agent.retention_state,
        AgentRetentionState::DeleteRequested
    );

    let diagnostics = agent_daemon_diagnostics(&mut mcp).await?;
    assert!(diagnostics.state_store_available);
    assert_eventually_no_pending_interactions(&mut mcp).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_diagnostics_reports_quota_and_overloaded_admission() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server =
        create_mock_responses_server_sequence(vec![create_final_assistant_message_sse_response(
            "accepted after slot",
        )?])
        .await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let state_db = init_state_db(codex_home.path()).await?;
    for index in 0..8 {
        seed_queued_agent_run(
            state_db.as_ref(),
            &format!("quota-run-{index}"),
            Some(format!("quota-idempotency-{index}")),
            &format!("quota run {index}"),
        )
        .await?;
    }
    let first_agent_id = "quota-run-0".to_string();

    let initial = agent_daemon_diagnostics(&mut mcp).await?;
    assert!(initial.state_store_available);
    assert!(initial.max_active_runs_per_user >= 2);
    assert_eq!(initial.max_active_runs_per_user, 8);

    let full = initial;
    assert_eq!(full.active_run_count, full.max_active_runs_per_user);
    assert_eq!(full.queued_run_count, full.max_active_runs_per_user);
    assert_eq!(full.available_active_run_slots, 0);
    assert!(!full.admission_allowed);
    assert_eq!(
        full.backpressure_reasons,
        vec!["active_run_limit".to_string()]
    );
    assert_eq!(
        run_status_count(&full, AgentRunStatus::Queued),
        full.max_active_runs_per_user
    );

    let rejected = start_agent_error(
        &mut mcp,
        start_params(
            "new run should be rejected while full",
            Some("quota-idempotency-overflow".to_string()),
            codex_home.path(),
        ),
    )
    .await?;
    assert_eq!(rejected.error.code, -32001);
    assert!(
        rejected
            .error
            .message
            .contains("background agent queue is overloaded"),
        "unexpected overloaded error: {}",
        rejected.error.message
    );

    state_db
        .update_background_agent_run_status(
            first_agent_id.as_str(),
            StateBackgroundAgentRunStatus::Completed,
            Some("completed by quota test"),
        )
        .await?;

    let available = agent_daemon_diagnostics(&mut mcp).await?;
    assert_eq!(
        available.active_run_count,
        available.max_active_runs_per_user - 1
    );
    assert_eq!(available.available_active_run_slots, 1);
    assert!(available.admission_allowed);
    assert!(available.backpressure_reasons.is_empty());

    let accepted = start_agent(
        &mut mcp,
        start_params(
            "new run after a slot opens",
            Some("quota-idempotency-after-slot".to_string()),
            codex_home.path(),
        ),
    )
    .await?;
    assert_ne!(accepted.agent.agent_id, first_agent_id);
    wait_for_agent_status(
        &mut mcp,
        accepted.agent.agent_id.as_str(),
        AgentRunStatus::Completed,
    )
    .await?;

    let retry = start_agent(
        &mut mcp,
        start_params(
            "idempotent retry is not new pressure",
            Some("quota-idempotency-0".to_string()),
            codex_home.path(),
        ),
    )
    .await?;
    assert_eq!(retry.agent.agent_id, first_agent_id);

    Ok(())
}

async fn init_mcp(codex_home: &Path) -> Result<McpProcess> {
    let mut mcp = McpProcess::new(codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
}

async fn init_state_db(codex_home: &Path) -> Result<Arc<codex_state::StateRuntime>> {
    let state_db =
        codex_state::StateRuntime::init(codex_home.to_path_buf(), "mock_provider".into()).await?;
    state_db
        .mark_backfill_complete(/*last_watermark*/ None)
        .await?;
    Ok(state_db)
}

fn write_config(codex_home: &Path, server_uri: &str) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"
model_provider = "mock_provider"
suppress_unstable_features_warning = true

[features]
sqlite = true

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )?;
    Ok(())
}

async fn wait_for_agent_status(
    mcp: &mut McpProcess,
    agent_id: &str,
    expected: AgentRunStatus,
) -> Result<AgentReadResponse> {
    let deadline = tokio::time::Instant::now() + DEFAULT_READ_TIMEOUT;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    loop {
        let list = agent_list(mcp).await?;
        if let Some(agent) = list.data.iter().find(|agent| agent.agent_id == agent_id) {
            if agent.status == expected {
                return agent_read(mcp, agent_id).await;
            }
            if matches!(
                agent.status,
                AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
            ) {
                let read = agent_read(mcp, agent_id).await?;
                anyhow::bail!(
                    "agent {agent_id} reached terminal status {:?}, expected {:?}: {:?}",
                    agent.status,
                    expected,
                    read.status_snapshot
                );
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for agent {agent_id} status {expected:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn assert_eventually_no_pending_interactions(mcp: &mut McpProcess) -> Result<()> {
    let deadline = tokio::time::Instant::now() + DEFAULT_READ_TIMEOUT;
    loop {
        let diagnostics = agent_daemon_diagnostics(mcp).await?;
        if diagnostics.pending_interaction_count == 0 {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for background-agent pending interactions to settle");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn seed_queued_agent_run(
    state_db: &codex_state::StateRuntime,
    agent_id: &str,
    idempotency_key: Option<String>,
    prompt: &str,
) -> Result<()> {
    state_db
        .create_background_agent_run(&BackgroundAgentRunCreateParams {
            id: agent_id.to_string(),
            idempotency_key,
            request_id: None,
            source: "quota-test".to_string(),
            prompt_snapshot_ref: format!("inline:{agent_id}:prompt"),
            input_snapshot_ref: None,
            thread_id: None,
            thread_store_kind: "background-agent".to_string(),
            thread_store_id: None,
            rollout_path: None,
            parent_thread_id: None,
            parent_agent_run_id: None,
            spawn_linkage_json: None,
            auth_profile_ref: None,
            status_reason: Some("queued by quota test".to_string()),
            config_fingerprint: Some("cfg-test".to_string()),
            version_fingerprint: Some("version-test".to_string()),
        })
        .await?;
    state_db
        .append_background_agent_event(
            agent_id,
            "agent.started",
            &json!({
                "cwd": null,
                "prompt": prompt,
                "promptSnapshotRef": format!("inline:{agent_id}:prompt"),
            }),
        )
        .await?;
    Ok(())
}

fn start_params(
    prompt: &str,
    idempotency_key: Option<String>,
    codex_home: &Path,
) -> AgentStartParams {
    AgentStartParams {
        prompt: prompt.to_string(),
        cwd: Some(codex_home.display().to_string()),
        idempotency_key,
        request_id: None,
        source: Some("app-server-test".to_string()),
        prompt_snapshot_ref: None,
        input_snapshot_ref: None,
        thread_id: None,
        thread_store_kind: None,
        thread_store_id: None,
        rollout_path: None,
        parent_thread_id: None,
        parent_agent_run_id: None,
        spawn_linkage: None,
        auth_profile_ref: None,
        config_fingerprint: Some("cfg-test".to_string()),
        version_fingerprint: Some("version-test".to_string()),
        execution_context: Some(Box::new(AgentExecutionContextParams {
            workspace_roots: Some(vec![codex_home.display().to_string()]),
            permission_profile: Some(json!({"sandbox": "read-only"})),
            sandbox_policy: Some(json!({"mode": "read-only"})),
            network_policy: Some(json!({"enabled": false})),
            model: Some("mock-model".to_string()),
            provider: Some("mock_provider".to_string()),
            service_tier: Some("default".to_string()),
            mcp_tool_allowlist: Some(vec!["shell".to_string(), "apply_patch".to_string()]),
            env_snapshot_policy: Some("inherit-minimal".to_string()),
            shell_snapshot: Some(json!({"shell": "bash"})),
            config_source_hashes: Some(json!({"config.toml": "cfg-test"})),
            max_runtime_seconds: Some(3600),
            max_tokens: Some(200_000),
            recovery_policy: Some("abort_mid_turn_resume_at_safe_boundary".to_string()),
        })),
    }
}

async fn start_agent(mcp: &mut McpProcess, params: AgentStartParams) -> Result<AgentStartResponse> {
    let request_id = mcp.send_agent_start_request(params).await?;
    read_response(mcp, request_id).await
}

async fn start_agent_error(mcp: &mut McpProcess, params: AgentStartParams) -> Result<JSONRPCError> {
    let request_id = mcp.send_agent_start_request(params).await?;
    let response = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(response)
}

async fn agent_daemon_diagnostics(mcp: &mut McpProcess) -> Result<AgentDaemonDiagnosticsResponse> {
    let request_id = mcp
        .send_agent_daemon_diagnostics_request(AgentDaemonDiagnosticsParams {})
        .await?;
    read_response(mcp, request_id).await
}

fn run_status_count(diagnostics: &AgentDaemonDiagnosticsResponse, status: AgentRunStatus) -> i64 {
    diagnostics
        .runs_by_status
        .iter()
        .find(|count| count.status == status)
        .map(|count| count.count)
        .unwrap_or_default()
}

async fn agent_list(mcp: &mut McpProcess) -> Result<AgentListResponse> {
    let request_id = mcp
        .send_agent_list_request(AgentListParams {
            cursor: None,
            limit: Some(10),
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn agent_read(mcp: &mut McpProcess, agent_id: &str) -> Result<AgentReadResponse> {
    let request_id = mcp
        .send_agent_read_request(AgentReadParams {
            agent_id: agent_id.to_string(),
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn agent_attach(mcp: &mut McpProcess, agent_id: &str) -> Result<AgentAttachResponse> {
    let request_id = mcp
        .send_agent_attach_request(AgentAttachParams {
            agent_id: agent_id.to_string(),
            cursor: None,
            limit: Some(10),
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn agent_events_page(
    mcp: &mut McpProcess,
    agent_id: &str,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<AgentEventsListResponse> {
    let request_id = mcp
        .send_agent_events_list_request(AgentEventsListParams {
            agent_id: agent_id.to_string(),
            cursor,
            limit,
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn agent_events_error(
    mcp: &mut McpProcess,
    agent_id: &str,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<JSONRPCError> {
    let request_id = mcp
        .send_agent_events_list_request(AgentEventsListParams {
            agent_id: agent_id.to_string(),
            cursor,
            limit,
        })
        .await?;
    let response = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(response)
}

async fn read_response<T: DeserializeOwned>(mcp: &mut McpProcess, request_id: i64) -> Result<T> {
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(response)
}
