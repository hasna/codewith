use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use chrono::Duration as ChronoDuration;
use chrono::Utc;
use codex_app_server::INPUT_TOO_LARGE_ERROR_CODE;
use codex_app_server::INVALID_PARAMS_ERROR_CODE;
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
use codex_app_server_protocol::AskForApproval;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadSettingsUpdateParams;
use codex_app_server_protocol::ThreadSettingsUpdateResponse;
use codex_app_server_protocol::ThreadSource;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_app_server_protocol::WorktreeAttachResponse;
use codex_app_server_protocol::WorktreeCleanupPolicy;
use codex_app_server_protocol::WorktreeCleanupResponse;
use codex_app_server_protocol::WorktreeCreateResponse;
use codex_app_server_protocol::WorktreeDetachResponse;
use codex_app_server_protocol::WorktreeLifecycleStatus;
use codex_app_server_protocol::WorktreeListResponse;
use codex_app_server_protocol::WorktreeMergeCandidateApplyResponse;
use codex_app_server_protocol::WorktreeMergeCandidateDismissResponse;
use codex_app_server_protocol::WorktreeMergeCandidateListResponse;
use codex_app_server_protocol::WorktreeMergeCandidateRefreshResponse;
use codex_app_server_protocol::WorktreeMergeCandidateStatus;
use codex_app_server_protocol::WorktreeOwnerKind;
use codex_app_server_protocol::WorktreeReadResponse;
use codex_app_server_protocol::WorktreeReleaseResponse;
use codex_protocol::ThreadId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use codex_state::BackgroundAgentDesiredState as StateBackgroundAgentDesiredState;
use codex_state::BackgroundAgentExecutionSnapshotParams;
use codex_state::BackgroundAgentPendingInteractionCreateParams;
use codex_state::BackgroundAgentPendingInteractionKind;
use codex_state::BackgroundAgentPendingInteractionStatus as StateBackgroundAgentPendingInteractionStatus;
use codex_state::BackgroundAgentRunCreateParams;
use codex_state::BackgroundAgentRunStatus as StateBackgroundAgentRunStatus;
use codex_state::BackgroundAgentStatusSnapshotParams;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

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
        start
            .execution_snapshot
            .payload
            .pointer("/permissionProfile/type"),
        Some(&json!("managed"))
    );
    assert_eq!(
        start
            .execution_snapshot
            .payload
            .pointer("/permissionProfile/network"),
        Some(&json!("restricted"))
    );
    assert_eq!(
        start
            .execution_snapshot
            .payload
            .pointer("/permissionProfile/file_system/type"),
        Some(&json!("restricted"))
    );
    assert_eq!(
        start.execution_snapshot.payload.get("approvalPolicy"),
        Some(&json!("never"))
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

    let first_events_page =
        agent_events_page(&mut restarted, &agent_id, /*cursor*/ None, Some(1)).await?;
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
    let all_events =
        agent_events_page(&mut restarted, &agent_id, /*cursor*/ None, Some(20)).await?;
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
async fn agent_start_records_initial_goal_objective() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut params = start_params(
        "fix the flaky test",
        Some("initial-goal-start".to_string()),
        codex_home.path(),
    );
    params.initial_goal_objective = Some("Investigate flaky regression".to_string());

    let mut mcp = init_mcp(codex_home.path()).await?;
    let start = start_agent(&mut mcp, params).await?;

    assert_eq!(
        start.execution_snapshot.payload.get("initialGoalObjective"),
        Some(&json!("Investigate flaky regression"))
    );
    assert_eq!(
        start.event.payload.get("initialGoalObjective"),
        Some(&json!("Investigate flaky regression"))
    );

    let retry = start_agent(
        &mut mcp,
        start_params(
            "retry with different params",
            Some("initial-goal-start".to_string()),
            codex_home.path(),
        ),
    )
    .await?;

    assert_eq!(retry.agent.agent_id, start.agent.agent_id);
    assert_eq!(
        retry.execution_snapshot.payload.get("initialGoalObjective"),
        Some(&json!("Investigate flaky regression"))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_rejects_empty_initial_goal_objective() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut params = start_params(
        "fix the flaky test",
        Some("empty-initial-goal-start".to_string()),
        codex_home.path(),
    );
    params.initial_goal_objective = Some(" \t ".to_string());

    let mut mcp = init_mcp(codex_home.path()).await?;
    let error = start_agent_error(&mut mcp, params).await?;

    assert_eq!(error.error.code, -32600);
    assert_eq!(error.error.message, "goal objective must not be empty");
    let list = agent_list(&mut mcp).await?;
    assert!(list.data.is_empty());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_list_pages_beyond_state_default_cap() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    for index in 0..501 {
        let agent_id = format!("paged-run-{index:03}");
        seed_queued_agent_run(
            state_db.as_ref(),
            agent_id.as_str(),
            /*idempotency_key*/ None,
            "paged run",
        )
        .await?;
        state_db
            .update_background_agent_run_status(
                agent_id.as_str(),
                StateBackgroundAgentRunStatus::Completed,
                Some("completed by pagination test"),
            )
            .await?;
    }
    let mut mcp = init_mcp(codex_home.path()).await?;

    let first_id = mcp
        .send_agent_list_request(AgentListParams {
            cursor: None,
            limit: Some(200),
        })
        .await?;
    let first: AgentListResponse = read_response(&mut mcp, first_id).await?;
    assert_eq!(first.data.len(), 200);
    assert_eq!(first.next_cursor, Some("200".to_string()));

    let second_id = mcp
        .send_agent_list_request(AgentListParams {
            cursor: first.next_cursor,
            limit: Some(200),
        })
        .await?;
    let second: AgentListResponse = read_response(&mut mcp, second_id).await?;
    assert_eq!(second.data.len(), 200);
    assert_eq!(second.next_cursor, Some("400".to_string()));

    let third_id = mcp
        .send_agent_list_request(AgentListParams {
            cursor: second.next_cursor,
            limit: Some(200),
        })
        .await?;
    let third: AgentListResponse = read_response(&mut mcp, third_id).await?;
    assert_eq!(third.data.len(), 101);
    assert_eq!(third.next_cursor, None);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_freezes_authority_from_server_config() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("background agent done")?,
    ])
    .await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut params = start_params(
        "verify frozen authority",
        Some("frozen-authority".to_string()),
        codex_home.path(),
    );
    params.cwd = Some("/tmp/client-selected-cwd".to_string());
    params.auth_profile_ref = Some("client-selected-auth-profile".to_string());
    let context = params
        .execution_context
        .as_mut()
        .expect("test params include execution context");
    context.workspace_roots = Some(vec!["/tmp/client-root".to_string()]);
    context.approval_policy = Some(AskForApproval::OnRequest);
    context.permission_profile = Some(json!({"sandbox": "danger-full-access"}));
    context.model = Some("client-model".to_string());
    context.provider = Some("client-provider".to_string());
    context.service_tier = Some("priority".to_string());

    let mut mcp = init_mcp(codex_home.path()).await?;
    let start = start_agent(&mut mcp, params).await?;

    assert_ne!(
        start.execution_snapshot.payload.get("cwd"),
        Some(&json!("/tmp/client-selected-cwd"))
    );
    assert_ne!(
        start.execution_snapshot.payload.get("workspaceRoots"),
        Some(&json!(["/tmp/client-root"]))
    );
    assert_eq!(
        start.execution_snapshot.payload.get("authProfileRef"),
        Some(&JsonValue::Null)
    );
    assert_eq!(
        start.execution_snapshot.payload.get("approvalPolicy"),
        Some(&json!("never"))
    );
    assert_eq!(
        start
            .execution_snapshot
            .payload
            .pointer("/permissionProfile/type"),
        Some(&json!("managed"))
    );
    assert_eq!(
        start
            .execution_snapshot
            .payload
            .pointer("/permissionProfile/network"),
        Some(&json!("restricted"))
    );
    assert_eq!(
        start
            .execution_snapshot
            .payload
            .pointer("/permissionProfile/file_system/type"),
        Some(&json!("restricted"))
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
        start.execution_snapshot.payload.get("serviceTier"),
        Some(&JsonValue::Null)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_uses_validated_managed_worktree_cwd() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("background agent done")?,
    ])
    .await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let create_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "agent-start",
                "startPoint": "HEAD",
            })),
        )
        .await?;
    let created: WorktreeCreateResponse = read_response(&mut mcp, create_request_id).await?;
    let created_worktree_id = created.worktree.worktree_id.clone();
    let created_worktree_path = created.worktree.worktree_path.clone();
    let mut params = start_params(
        "run inside the managed worktree",
        Some("validated-managed-worktree-cwd".to_string()),
        codex_home.path(),
    );
    params.cwd = Some(created_worktree_path.clone());
    let context = params
        .execution_context
        .as_mut()
        .expect("test params include execution context");
    context.workspace_roots = Some(vec!["/tmp/client-root".to_string()]);

    let start = start_agent(&mut mcp, params).await?;

    assert_eq!(
        start.execution_snapshot.payload.get("cwd"),
        Some(&json!(created_worktree_path))
    );
    assert_eq!(
        start.execution_snapshot.payload.get("workspaceRoots"),
        Some(&json!([created_worktree_path]))
    );
    let read_request_id = mcp
        .send_raw_request(
            "worktree/read",
            Some(json!({
                "worktreeId": created_worktree_id,
            })),
        )
        .await?;
    let read: WorktreeReadResponse = read_response(&mut mcp, read_request_id).await?;
    let worktree = read.worktree.expect("managed worktree should still exist");
    assert_eq!(
        Some(start.agent.agent_id.as_str()),
        worktree.owner_agent_run_id.as_deref()
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_rebinds_workspace_write_permissions_to_managed_worktree() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("background agent done")?,
    ])
    .await;
    write_config_with_sandbox_mode_and_extra(
        codex_home.path(),
        server.uri().as_str(),
        "workspace-write",
        r#"
[sandbox_workspace_write]
exclude_tmpdir_env_var = true
exclude_slash_tmp = true
"#,
    )?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let create_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "agent-start-write",
                "startPoint": "HEAD",
            })),
        )
        .await?;
    let created: WorktreeCreateResponse = read_response(&mut mcp, create_request_id).await?;
    let created_worktree_path = created.worktree.worktree_path.clone();
    let mut params = start_params(
        "run inside the managed writable worktree",
        Some("validated-managed-worktree-permissions".to_string()),
        codex_home.path(),
    );
    params.execution_context = None;
    params.cwd = Some(created_worktree_path.clone());

    let start = start_agent(&mut mcp, params).await?;

    let permission_profile: PermissionProfile = serde_json::from_value(
        start
            .execution_snapshot
            .payload
            .get("permissionProfile")
            .expect("execution snapshot should include permissionProfile")
            .clone(),
    )?;
    let file_system_policy = permission_profile.file_system_sandbox_policy();
    let worktree_path = Path::new(created_worktree_path.as_str());
    assert!(
        file_system_policy.can_write_path_with_cwd(worktree_path, worktree_path),
        "managed worktree should be writable, policy: {file_system_policy:?}"
    );
    assert!(
        !file_system_policy.can_write_path_with_cwd(codex_home.path(), worktree_path),
        "base checkout must not stay writable after worktree rebinding, policy: {file_system_policy:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_rejects_shared_repository_managed_worktree_cwd() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_managed_worktree_with_mode(
        state_db.as_ref(),
        "wt-shared-agent-start",
        codex_home.path(),
        codex_state::ManagedWorktreeMode::SharedRepository,
    )
    .await?;
    let shared_worktree_path = codex_home
        .path()
        .join(".codewith")
        .join("worktrees")
        .join("wt-shared-agent-start");
    std::fs::create_dir_all(&shared_worktree_path)?;
    #[cfg(target_os = "macos")]
    assert_ne!(
        shared_worktree_path,
        std::fs::canonicalize(&shared_worktree_path)?,
        "macOS regression coverage requires the /var and /private/var path aliases"
    );

    let mut params = start_params(
        "run inside a shared-repository worktree",
        Some("shared-repository-managed-worktree-cwd".to_string()),
        codex_home.path(),
    );
    params.cwd = Some(shared_worktree_path.display().to_string());

    let mut mcp = init_mcp(codex_home.path()).await?;
    let error = start_agent_error(&mut mcp, params).await?;

    assert_eq!(INVALID_PARAMS_ERROR_CODE, error.error.code);
    assert_eq!(
        "agent/start worktree cwd requires an isolated managed worktree",
        error.error.message
    );
    let list = agent_list(&mut mcp).await?;
    assert!(list.data.is_empty());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_rejects_unvalidated_rollout_path_metadata() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let mut missing_rollout = start_params(
        "resume a thread without rollout",
        Some("missing-rollout-thread".to_string()),
        codex_home.path(),
    );
    missing_rollout.thread_id = Some("00000000-0000-0000-0000-000000000320".to_string());
    let missing_rollout_error = start_agent_error(&mut mcp, missing_rollout).await?;
    assert_eq!(missing_rollout_error.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(
        missing_rollout_error.error.message,
        "agent/start threadId requires rolloutPath"
    );

    let mut missing_thread = start_params(
        "resume an unowned rollout",
        Some("missing-thread-rollout".to_string()),
        codex_home.path(),
    );
    missing_thread.rollout_path = Some(
        codex_home
            .path()
            .join("unowned-rollout.jsonl")
            .display()
            .to_string(),
    );
    let missing_thread_error = start_agent_error(&mut mcp, missing_thread).await?;
    assert_eq!(missing_thread_error.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(
        missing_thread_error.error.message,
        "agent/start rolloutPath requires threadId"
    );

    let mut unknown_thread = start_params(
        "resume an unknown thread rollout",
        Some("unknown-thread-rollout".to_string()),
        codex_home.path(),
    );
    unknown_thread.thread_id = Some("00000000-0000-0000-0000-000000000321".to_string());
    unknown_thread.rollout_path = Some(
        codex_home
            .path()
            .join("different-rollout.jsonl")
            .display()
            .to_string(),
    );
    let unknown_thread_error = start_agent_error(&mut mcp, unknown_thread).await?;
    assert_eq!(unknown_thread_error.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(
        unknown_thread_error.error.message,
        "agent/start rolloutPath requires a known threadId"
    );

    let list = agent_list(&mut mcp).await?;
    assert!(list.data.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_rejects_oversized_prompt_without_persisting_run() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let prompt = "a".repeat(MAX_USER_INPUT_TEXT_CHARS + 1);
    let error = start_agent_error(
        &mut mcp,
        start_params(
            prompt.as_str(),
            Some("oversized-background-agent-prompt".to_string()),
            codex_home.path(),
        ),
    )
    .await?;

    assert_eq!(error.error.code, INVALID_PARAMS_ERROR_CODE);
    assert_eq!(
        error.error.message,
        format!("Input exceeds the maximum length of {MAX_USER_INPUT_TEXT_CHARS} characters.")
    );
    let data = error.error.data.expect("structured oversized input error");
    assert_eq!(data["input_error_code"], INPUT_TOO_LARGE_ERROR_CODE);
    assert_eq!(data["max_chars"], MAX_USER_INPUT_TEXT_CHARS);
    assert_eq!(data["actual_chars"], prompt.chars().count());

    let list = agent_list(&mut mcp).await?;
    assert!(list.data.is_empty());
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
        /*idempotency_key*/ None,
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

    let events = agent_events_page(&mut mcp, agent_id.as_str(), /*cursor*/ None, Some(20)).await?;
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
        /*idempotency_key*/ None,
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

    let read_after_expire = agent_read(&mut mcp, &agent_id).await?;
    let read_agent = read_after_expire
        .agent
        .expect("read should return the seeded agent");
    let read_snapshot = read_after_expire
        .status_snapshot
        .expect("read should return a refreshed status snapshot");
    assert_eq!(read_agent.last_event_seq, read_snapshot.last_event_seq);
    let read_expired = read_after_expire
        .pending_interactions
        .iter()
        .find(|interaction| interaction.interaction_id == "expired-1")
        .expect("expired interaction should be returned by read");
    assert_eq!(read_expired.status, AgentPendingInteractionStatus::Expired);

    let invalid_attach = raw_request_error(
        &mut mcp,
        "agent/attach",
        json!({
            "agentId": agent_id.clone(),
            "cursor": "not-an-event-cursor",
        }),
    )
    .await?;
    assert_eq!(-32600, invalid_attach.error.code);
    assert_eq!(
        "cursor must be an opaque event cursor",
        invalid_attach.error.message
    );
    let approval_before_attach = state_db
        .get_background_agent_pending_interaction("approval-1")
        .await?
        .expect("approval should still exist");
    assert_eq!(
        StateBackgroundAgentPendingInteractionStatus::Pending,
        approval_before_attach.status
    );

    let attach = agent_attach(&mut mcp, &agent_id).await?;
    assert_eq!(attach.effect, AgentLifecycleEffect::ReplayState);
    assert_eq!(
        attach
            .agent
            .as_ref()
            .expect("attach should return agent")
            .last_event_seq,
        attach
            .status_snapshot
            .as_ref()
            .expect("attach should return status snapshot")
            .last_event_seq
    );
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
    assert!(
        attach
            .events
            .iter()
            .any(|event| event.event_type == "interaction.delivered")
    );
    let expired = attach
        .pending_interactions
        .iter()
        .find(|interaction| interaction.interaction_id == "expired-1")
        .expect("expired interaction should be replayed");
    assert_eq!(expired.status, AgentPendingInteractionStatus::Expired);
    let diagnostics = agent_daemon_diagnostics(&mut mcp).await?;
    assert_eq!(1, diagnostics.pending_interaction_count);

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
        AgentPendingInteractionStatus::Expired,
        expired_respond
            .interaction
            .expect("expired interaction should be returned")
            .status
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
    let read_after_respond = agent_read(&mut mcp, &agent_id).await?;
    assert_eq!(
        read_after_respond
            .agent
            .expect("read should return agent")
            .last_event_seq,
        read_after_respond
            .status_snapshot
            .expect("read should return status snapshot")
            .last_event_seq
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
async fn agent_stop_preserves_delete_requested_desired_state() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    let agent_id = "delete-then-stop-run".to_string();
    seed_queued_agent_run(
        state_db.as_ref(),
        agent_id.as_str(),
        /*idempotency_key*/ None,
        "delete then stop",
    )
    .await?;
    drop(state_db);

    let mut mcp = init_mcp(codex_home.path()).await?;
    let delete_id = mcp
        .send_agent_delete_request(AgentDeleteParams {
            agent_id: agent_id.clone(),
        })
        .await?;
    let delete: AgentDeleteResponse = read_response(&mut mcp, delete_id).await?;
    let deleted_agent = delete.agent.expect("delete response should include agent");
    assert_eq!(AgentDesiredState::Deleted, deleted_agent.desired_state);
    assert_eq!(
        AgentRetentionState::DeleteRequested,
        deleted_agent.retention_state
    );

    let stop_id = mcp
        .send_agent_stop_request(AgentStopParams {
            agent_id: agent_id.clone(),
        })
        .await?;
    let stop: AgentStopResponse = read_response(&mut mcp, stop_id).await?;
    let stopped_agent = stop.agent.expect("stop response should include agent");
    assert_eq!(AgentDesiredState::Deleted, stopped_agent.desired_state);
    assert_eq!(
        AgentRetentionState::DeleteRequested,
        stopped_agent.retention_state
    );
    assert!(
        matches!(
            stopped_agent.status,
            AgentRunStatus::Stopping | AgentRunStatus::Cancelled
        ),
        "unexpected stop status: {:?}",
        stopped_agent.status
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_pending_interaction_respond_rejects_invalid_responded_payload() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let state_db = init_state_db(codex_home.path()).await?;
    let agent_id = "respond-validation-run".to_string();
    seed_queued_agent_run(
        state_db.as_ref(),
        agent_id.as_str(),
        /*idempotency_key*/ None,
        "wait for approval",
    )
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

    let error = agent_pending_interaction_respond_error(
        &mut mcp,
        AgentPendingInteractionRespondParams {
            agent_id: agent_id.clone(),
            interaction_id: "approval-1".to_string(),
            response: json!({ "approved": true }),
            terminal_status: AgentPendingInteractionTerminalStatus::Responded,
        },
    )
    .await?;
    assert_eq!(error.error.code, -32600);
    assert!(
        error
            .error
            .message
            .contains("background agent pending interaction response is invalid for approval"),
        "unexpected invalid response error: {}",
        error.error.message
    );
    let pending = state_db
        .get_background_agent_pending_interaction("approval-1")
        .await?
        .expect("interaction should still exist");
    assert_eq!(
        pending.status,
        StateBackgroundAgentPendingInteractionStatus::Pending
    );
    assert_eq!(pending.response_payload_json, None);

    let respond_id = mcp
        .send_agent_pending_interaction_respond_request(AgentPendingInteractionRespondParams {
            agent_id,
            interaction_id: "approval-1".to_string(),
            response: json!({ "decision": "approved" }),
            terminal_status: AgentPendingInteractionTerminalStatus::Responded,
        })
        .await?;
    let respond: AgentPendingInteractionRespondResponse =
        read_response(&mut mcp, respond_id).await?;
    assert!(respond.updated);
    assert_eq!(
        respond.interaction.expect("responded interaction").status,
        AgentPendingInteractionStatus::Responded
    );

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_list_without_current_repo_does_not_return_global_worktrees() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    let other_repo = codex_home.path().join("other-repo");
    create_managed_worktree(state_db.as_ref(), "wt-other-repo", other_repo.as_path()).await?;
    drop(state_db);

    let mut mcp = init_mcp(codex_home.path()).await?;
    let request_id = mcp
        .send_raw_request("worktree/list", Some(json!({})))
        .await?;
    let response: WorktreeListResponse = read_response(&mut mcp, request_id).await?;

    assert_eq!(Vec::<String>::new(), worktree_ids(&response));
    assert_eq!(None, response.policy.current_base_repo_path);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_list_and_read_are_scoped_to_current_repo() -> Result<()> {
    let codex_home = TempDir::new()?;
    let repo_path = codex_home.path().canonicalize()?;
    std::fs::create_dir(repo_path.join(".git"))?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-current", repo_path.as_path()).await?;
    let other_repo = codex_home.path().join("other-repo");
    std::fs::create_dir_all(other_repo.join(".git"))?;
    create_managed_worktree(state_db.as_ref(), "wt-other", other_repo.as_path()).await?;
    state_db
        .managed_worktrees()
        .record_merge_candidate(codex_state::ManagedWorktreeMergeCandidateRecordParams {
            candidate_id: Some("candidate-other-repo".to_string()),
            worktree_id: "wt-other".to_string(),
            target_ref: "HEAD".to_string(),
            target_sha: Some("target-sha".to_string()),
            base_sha: "base-sha".to_string(),
            head_sha: "head-sha".to_string(),
            status: codex_state::ManagedWorktreeMergeCandidateStatus::Open,
            conflict_summary: None,
            test_summary_json: None,
        })
        .await?;
    drop(state_db);

    let mut mcp = init_mcp(codex_home.path()).await?;
    let list_request_id = mcp
        .send_raw_request("worktree/list", Some(json!({})))
        .await?;
    let list_response: WorktreeListResponse = read_response(&mut mcp, list_request_id).await?;
    assert_eq!(vec!["wt-current".to_string()], worktree_ids(&list_response));
    assert_eq!(
        Some(protocol_path(&repo_path)),
        list_response.policy.current_base_repo_path
    );

    let read_other_request_id = mcp
        .send_raw_request(
            "worktree/read",
            Some(json!({
                "worktreeId": "wt-other",
            })),
        )
        .await?;
    let read_other: WorktreeReadResponse = read_response(&mut mcp, read_other_request_id).await?;
    assert_eq!(None, read_other.worktree);
    assert_eq!(
        Some(protocol_path(&repo_path)),
        read_other.policy.current_base_repo_path
    );

    let read_requested_repo_request_id = mcp
        .send_raw_request(
            "worktree/read",
            Some(json!({
                "worktreeId": "wt-other",
                "baseRepoPath": protocol_path(&other_repo),
            })),
        )
        .await?;
    let read_requested_repo: WorktreeReadResponse =
        read_response(&mut mcp, read_requested_repo_request_id).await?;
    assert_eq!(
        Some("wt-other".to_string()),
        read_requested_repo
            .worktree
            .as_ref()
            .map(|worktree| worktree.worktree_id.clone())
    );
    assert_eq!(
        Some(protocol_path(&other_repo)),
        read_requested_repo.policy.current_base_repo_path
    );

    let list_other_candidates_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/list",
            Some(json!({
                "worktreeId": "wt-other",
                "status": "open",
            })),
        )
        .await?;
    let other_candidates: WorktreeMergeCandidateListResponse =
        read_response(&mut mcp, list_other_candidates_request_id).await?;
    assert_eq!(
        Vec::<String>::new(),
        worktree_ids_from_candidates(&other_candidates)
    );

    let apply_other_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/apply",
            Some(json!({
                "candidateId": "candidate-other-repo",
            })),
        )
        .await?;
    let apply_other: WorktreeMergeCandidateApplyResponse =
        read_response(&mut mcp, apply_other_request_id).await?;
    assert_eq!(None, apply_other.candidate);

    let dismiss_other_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/dismiss",
            Some(json!({
                "candidateId": "candidate-other-repo",
            })),
        )
        .await?;
    let dismiss_other: WorktreeMergeCandidateDismissResponse =
        read_response(&mut mcp, dismiss_other_request_id).await?;
    assert_eq!(None, dismiss_other.candidate);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_create_reconcile_and_cleanup_use_real_git_worktrees() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    git(codex_home.path(), &["add", "config.toml"])?;
    git(codex_home.path(), &["commit", "-m", "add test config"])?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let invalid_branch_error = raw_request_error(
        &mut mcp,
        "worktree/create",
        json!({
            "name": "invalid",
            "branch": "-bad",
            "startPoint": "HEAD",
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, invalid_branch_error.error.code);
    assert!(
        invalid_branch_error
            .error
            .message
            .contains("is not a valid git branch name"),
        "unexpected invalid branch error: {}",
        invalid_branch_error.error.message
    );

    let create_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "feature",
                "startPoint": "HEAD",
            })),
        )
        .await?;
    let created: WorktreeCreateResponse = read_response(&mut mcp, create_request_id).await?;
    assert_eq!(
        WorktreeLifecycleStatus::Active,
        created.worktree.lifecycle_status
    );
    assert!(Path::new(created.worktree.worktree_path.as_str()).exists());
    assert!(
        created
            .worktree
            .branch
            .as_deref()
            .is_some_and(|branch| branch.starts_with("codewith/feature-"))
    );
    let retained_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "retained",
                "startPoint": "HEAD",
            })),
        )
        .await?;
    let retained: WorktreeCreateResponse = read_response(&mut mcp, retained_request_id).await?;
    let retained_path = std::path::PathBuf::from(retained.worktree.worktree_path.as_str());
    std::fs::write(retained_path.join("retained.txt"), "committed work\n")?;
    git(retained_path.as_path(), &["add", "retained.txt"])?;
    git(
        retained_path.as_path(),
        &["commit", "-m", "retain committed work"],
    )?;
    let retained_cleanup_request_id = mcp
        .send_raw_request(
            "worktree/cleanup",
            Some(json!({
                "worktreeId": retained.worktree.worktree_id,
                "forceDelete": false,
            })),
        )
        .await?;
    let retained_cleanup: WorktreeCleanupResponse =
        read_response(&mut mcp, retained_cleanup_request_id).await?;
    let retained_after_cleanup = retained_cleanup
        .worktree
        .expect("retained cleanup response should include worktree");
    assert_eq!(
        WorktreeLifecycleStatus::CleanupPending,
        retained_after_cleanup.lifecycle_status
    );
    assert!(retained_path.exists());

    let outside_root_path = codex_home.path().join("outside-root-worktree");
    git(
        codex_home.path(),
        &[
            "worktree",
            "add",
            "-b",
            "codewith/outside-root",
            outside_root_path.to_string_lossy().as_ref(),
            "HEAD",
        ],
    )?;
    #[cfg(target_os = "macos")]
    assert_ne!(
        outside_root_path,
        std::fs::canonicalize(&outside_root_path)?,
        "macOS regression coverage requires the /var and /private/var path aliases"
    );
    let state_db = init_state_db(codex_home.path()).await?;
    state_db
        .managed_worktrees()
        .create_managed_worktree(codex_state::ManagedWorktreeCreateParams {
            worktree_id: Some("outside-root".to_string()),
            identity: Some("test:outside-root".to_string()),
            mode: codex_state::ManagedWorktreeMode::IsolatedWorktree,
            base_repo_path: codex_home.path().to_path_buf(),
            worktree_path: outside_root_path.clone(),
            branch: Some("codewith/outside-root".to_string()),
            base_sha: None,
            head_sha: None,
            status_snapshot_json: json!({"status": "outside-root"}),
            dirty: false,
            cleanup_policy: codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean,
            owner_kind: codex_state::ManagedWorktreeOwnerKind::Manual,
            owner_thread_id: None,
            owner_agent_run_id: None,
            cleanup_after: None,
        })
        .await?;

    let manual_path = codex_home
        .path()
        .join(".codewith")
        .join("worktrees")
        .join("manual-discovered");
    git(
        codex_home.path(),
        &[
            "worktree",
            "add",
            "-b",
            "codewith/manual-discovered",
            manual_path.to_string_lossy().as_ref(),
            "HEAD",
        ],
    )?;
    let manual_protocol_path = protocol_path(std::fs::canonicalize(&manual_path)?.as_path());
    let reconcile_request_id = mcp
        .send_raw_request("worktree/reconcile", Some(json!({})))
        .await?;
    let reconciled: codex_app_server_protocol::WorktreeReconcileResponse =
        read_response(&mut mcp, reconcile_request_id).await?;
    assert_eq!(1, reconciled.discovered);
    assert!(reconciled.updated >= 1);
    assert!(reconciled.data.iter().any(|worktree| {
        worktree.worktree_path == manual_protocol_path
            && worktree
                .identity
                .as_deref()
                .is_some_and(|identity| identity.starts_with("discovered:"))
    }));
    assert!(reconciled.data.iter().any(|worktree| {
        worktree.worktree_id == "outside-root"
            && worktree.lifecycle_status == WorktreeLifecycleStatus::Active
            && worktree.worktree_path == protocol_path(&outside_root_path)
    }));

    let cleanup_request_id = mcp
        .send_raw_request(
            "worktree/cleanup",
            Some(json!({
                "worktreeId": created.worktree.worktree_id,
                "forceDelete": true,
            })),
        )
        .await?;
    let cleanup: WorktreeCleanupResponse = read_response(&mut mcp, cleanup_request_id).await?;
    let cleaned = cleanup
        .worktree
        .expect("cleanup response should include worktree tombstone");
    assert_eq!(WorktreeLifecycleStatus::Deleted, cleaned.lifecycle_status);
    assert!(!Path::new(cleaned.worktree_path.as_str()).exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_release_background_agent_lease_uses_lease_release_path() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_background_agent_git_worktree_lease(
        state_db.as_ref(),
        "agent-run-release",
        "lease-release",
        codex_home.path(),
        StateBackgroundAgentRunStatus::Completed,
    )
    .await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let release_request_id = mcp
        .send_raw_request(
            "worktree/release",
            Some(json!({
                "worktreeId": "lease-release",
                "cleanupPolicy": "retain",
                "forceDelete": false,
            })),
        )
        .await?;
    let released: WorktreeReleaseResponse = read_response(&mut mcp, release_request_id).await?;
    let released_worktree = released
        .worktree
        .expect("release response should include the managed worktree");
    assert_eq!("lease-release", released_worktree.worktree_id);
    assert_eq!(
        WorktreeLifecycleStatus::Released,
        released_worktree.lifecycle_status
    );
    assert_eq!(
        WorktreeOwnerKind::BackgroundAgent,
        released_worktree.owner_kind
    );
    assert_eq!(
        Some("agent-run-release".to_string()),
        released_worktree.owner_agent_run_id
    );

    let lease = state_db
        .get_background_agent_worktree_lease("lease-release")
        .await?
        .expect("background-agent lease should still be readable after release");
    assert!(lease.released_at.is_some());
    assert_eq!(None, lease.deleted_at);
    assert!(!lease.force_delete_requested);
    assert_eq!(Some(&json!(false)), lease.status_snapshot_json.get("dirty"));

    let force_release_request_id = mcp
        .send_raw_request(
            "worktree/release",
            Some(json!({
                "worktreeId": "lease-release",
                "forceDelete": true,
            })),
        )
        .await?;
    let force_released: WorktreeReleaseResponse =
        read_response(&mut mcp, force_release_request_id).await?;
    let force_released_worktree = force_released
        .worktree
        .expect("force release response should include the managed worktree");
    assert_eq!(
        WorktreeLifecycleStatus::CleanupPending,
        force_released_worktree.lifecycle_status
    );
    let lease = state_db
        .get_background_agent_worktree_lease("lease-release")
        .await?
        .expect("background-agent lease should remain readable after force release");
    assert!(lease.force_delete_requested);
    assert_eq!(None, lease.deleted_at);
    let worktree = state_db
        .managed_worktrees()
        .get_managed_worktree("lease-release")
        .await?
        .expect("managed worktree mirror should still exist");
    assert_eq!(
        codex_state::ManagedWorktreeLifecycleStatus::CleanupPending,
        worktree.lifecycle_status
    );
    assert!(worktree.released_at.is_some());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_release_and_cleanup_reject_active_background_agent_lease() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_background_agent_git_worktree_lease(
        state_db.as_ref(),
        "agent-run-active-lease",
        "lease-active",
        codex_home.path(),
        StateBackgroundAgentRunStatus::Running,
    )
    .await?;
    create_background_agent_git_worktree_lease(
        state_db.as_ref(),
        "agent-run-stopping-lease",
        "lease-stopping",
        codex_home.path(),
        StateBackgroundAgentRunStatus::Stopping,
    )
    .await?;
    state_db
        .set_background_agent_desired_state(
            "agent-run-stopping-lease",
            StateBackgroundAgentDesiredState::Stopped,
        )
        .await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let release_error = raw_request_error(
        &mut mcp,
        "worktree/release",
        json!({
            "worktreeId": "lease-active",
            "forceDelete": true,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, release_error.error.code);
    assert!(
        release_error
            .error
            .message
            .contains("active background agent run"),
        "unexpected release error: {}",
        release_error.error.message
    );
    let cleanup_error = raw_request_error(
        &mut mcp,
        "worktree/cleanup",
        json!({
            "worktreeId": "lease-active",
            "forceDelete": true,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, cleanup_error.error.code);
    assert!(
        cleanup_error
            .error
            .message
            .contains("active background agent run"),
        "unexpected cleanup error: {}",
        cleanup_error.error.message
    );
    let lease = state_db
        .get_background_agent_worktree_lease("lease-active")
        .await?
        .expect("active lease should remain");
    assert_eq!(None, lease.released_at);
    assert_eq!(None, lease.deleted_at);
    assert!(Path::new(lease.worktree_path.as_str()).exists());

    let stopping_release_error = raw_request_error(
        &mut mcp,
        "worktree/release",
        json!({
            "worktreeId": "lease-stopping",
            "forceDelete": true,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, stopping_release_error.error.code);
    assert!(
        stopping_release_error
            .error
            .message
            .contains("active background agent run"),
        "unexpected stopping release error: {}",
        stopping_release_error.error.message
    );
    let stopping_cleanup_error = raw_request_error(
        &mut mcp,
        "worktree/cleanup",
        json!({
            "worktreeId": "lease-stopping",
            "forceDelete": true,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, stopping_cleanup_error.error.code);
    assert!(
        stopping_cleanup_error
            .error
            .message
            .contains("active background agent run"),
        "unexpected stopping cleanup error: {}",
        stopping_cleanup_error.error.message
    );
    let lease = state_db
        .get_background_agent_worktree_lease("lease-stopping")
        .await?
        .expect("stopping lease should remain");
    assert_eq!(None, lease.released_at);
    assert_eq!(None, lease.deleted_at);
    assert!(Path::new(lease.worktree_path.as_str()).exists());

    state_db
        .update_background_agent_run_status(
            "agent-run-active-lease",
            StateBackgroundAgentRunStatus::Orphaned,
            Some("supervisor heartbeat stale"),
        )
        .await?;
    let orphan_release_error = raw_request_error(
        &mut mcp,
        "worktree/release",
        json!({
            "worktreeId": "lease-active",
            "forceDelete": true,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, orphan_release_error.error.code);
    assert!(
        orphan_release_error
            .error
            .message
            .contains("active background agent run"),
        "unexpected orphan release error: {}",
        orphan_release_error.error.message
    );
    let orphan_cleanup_error = raw_request_error(
        &mut mcp,
        "worktree/cleanup",
        json!({
            "worktreeId": "lease-active",
            "forceDelete": true,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, orphan_cleanup_error.error.code);
    assert!(
        orphan_cleanup_error
            .error
            .message
            .contains("active background agent run"),
        "unexpected orphan cleanup error: {}",
        orphan_cleanup_error.error.message
    );
    let lease = state_db
        .get_background_agent_worktree_lease("lease-active")
        .await?
        .expect("orphaned lease should remain");
    assert_eq!(None, lease.released_at);
    assert_eq!(None, lease.deleted_at);
    assert!(Path::new(lease.worktree_path.as_str()).exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_cleanup_retains_nonterminal_owner_agent_worktree() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    seed_queued_agent_run(
        state_db.as_ref(),
        "agent-run-cleanup-guard",
        /*idempotency_key*/ None,
        "guard nonterminal owner cleanup",
    )
    .await?;
    state_db
        .set_background_agent_desired_state(
            "agent-run-cleanup-guard",
            StateBackgroundAgentDesiredState::Stopped,
        )
        .await?;
    state_db
        .update_background_agent_run_status(
            "agent-run-cleanup-guard",
            StateBackgroundAgentRunStatus::Stopping,
            Some("stop requested"),
        )
        .await?;
    let base_sha_output = Command::new("git")
        .current_dir(codex_home.path())
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !base_sha_output.status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&base_sha_output.stderr)
        );
    }
    let base_sha = String::from_utf8(base_sha_output.stdout)?
        .trim()
        .to_string();
    let worktree_path = codex_home
        .path()
        .join(".codewith")
        .join("worktrees")
        .join("cleanup-guard");
    let worktree_parent = worktree_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("worktree path has no parent"))?;
    std::fs::create_dir_all(worktree_parent)?;
    git(
        codex_home.path(),
        &[
            "worktree",
            "add",
            "-b",
            "codewith/cleanup-guard",
            worktree_path.to_string_lossy().as_ref(),
            "HEAD",
        ],
    )?;
    state_db
        .managed_worktrees()
        .create_managed_worktree(codex_state::ManagedWorktreeCreateParams {
            worktree_id: Some("cleanup-guard".to_string()),
            identity: Some("test:cleanup-guard".to_string()),
            mode: codex_state::ManagedWorktreeMode::IsolatedWorktree,
            base_repo_path: codex_home.path().to_path_buf(),
            worktree_path: worktree_path.clone(),
            branch: Some("codewith/cleanup-guard".to_string()),
            base_sha: Some(base_sha.clone()),
            head_sha: Some(base_sha),
            status_snapshot_json: json!({"dirty": false, "source": "seed"}),
            dirty: false,
            cleanup_policy: codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean,
            owner_kind: codex_state::ManagedWorktreeOwnerKind::BackgroundAgent,
            owner_thread_id: None,
            owner_agent_run_id: Some("agent-run-cleanup-guard".to_string()),
            cleanup_after: None,
        })
        .await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let cleanup_request_id = mcp
        .send_raw_request(
            "worktree/cleanup",
            Some(json!({
                "worktreeId": "cleanup-guard",
                "forceDelete": true,
            })),
        )
        .await?;
    let cleanup: WorktreeCleanupResponse = read_response(&mut mcp, cleanup_request_id).await?;
    assert_eq!(
        WorktreeLifecycleStatus::CleanupPending,
        cleanup
            .worktree
            .expect("cleanup response should include worktree")
            .lifecycle_status
    );
    assert!(worktree_path.exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_cleanup_background_agent_lease_uses_lease_release_path() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_background_agent_git_worktree_lease(
        state_db.as_ref(),
        "agent-run-cleanup",
        "lease-cleanup",
        codex_home.path(),
        StateBackgroundAgentRunStatus::Completed,
    )
    .await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let cleanup_request_id = mcp
        .send_raw_request(
            "worktree/cleanup",
            Some(json!({
                "worktreeId": "lease-cleanup",
                "forceDelete": false,
            })),
        )
        .await?;
    let cleanup: WorktreeCleanupResponse = read_response(&mut mcp, cleanup_request_id).await?;
    let cleaned = cleanup
        .worktree
        .expect("cleanup response should include the managed worktree");
    assert_eq!("lease-cleanup", cleaned.worktree_id);
    assert_eq!(WorktreeLifecycleStatus::Deleted, cleaned.lifecycle_status);
    assert!(!Path::new(cleaned.worktree_path.as_str()).exists());

    let lease = state_db
        .get_background_agent_worktree_lease("lease-cleanup")
        .await?
        .expect("background-agent lease should remain readable after cleanup");
    assert!(lease.released_at.is_some());
    assert!(lease.deleted_at.is_some());

    let cleanup_again_request_id = mcp
        .send_raw_request(
            "worktree/cleanup",
            Some(json!({
                "worktreeId": "lease-cleanup",
                "forceDelete": false,
            })),
        )
        .await?;
    let cleanup_again: WorktreeCleanupResponse =
        read_response(&mut mcp, cleanup_again_request_id).await?;
    assert_eq!(
        WorktreeLifecycleStatus::Deleted,
        cleanup_again
            .worktree
            .expect("cleanup retry should return tombstone")
            .lifecycle_status
    );

    let release_again_request_id = mcp
        .send_raw_request(
            "worktree/release",
            Some(json!({
                "worktreeId": "lease-cleanup",
            })),
        )
        .await?;
    let release_again: WorktreeReleaseResponse =
        read_response(&mut mcp, release_again_request_id).await?;
    assert_eq!(
        WorktreeLifecycleStatus::Deleted,
        release_again
            .worktree
            .expect("release retry should return tombstone")
            .lifecycle_status
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_create_rejects_subagent_threads() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id =
        start_thread_for_worktree_attach_with_source(&mut mcp, Some(ThreadSource::Subagent))
            .await?;

    let error = raw_request_error(
        &mut mcp,
        "worktree/create",
        json!({
            "name": "subagent-denied",
            "startPoint": "HEAD",
            "threadId": thread_id,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, error.error.code);
    assert_eq!(
        "subagent sessions cannot create managed worktrees",
        error.error.message
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_worktree_mode_blocks_session_create_and_attach() -> Result<()> {
    let codex_home = TempDir::new()?;
    let repo_path = codex_home.path().join("repo");
    std::fs::create_dir_all(repo_path.as_path())?;
    init_git_repo(repo_path.as_path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-shared", repo_path.as_path()).await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread_for_worktree_attach(&mut mcp).await?;
    upsert_thread_for_worktree_attach(state_db.as_ref(), thread_id.as_str(), repo_path.as_path())
        .await?;
    update_thread_worktree_mode(
        &mut mcp,
        thread_id.as_str(),
        codex_protocol::protocol::SessionWorktreeMode::Shared,
    )
    .await?;

    let create_error = raw_request_error(
        &mut mcp,
        "worktree/create",
        json!({
            "name": "shared-denied",
            "startPoint": "HEAD",
            "threadId": thread_id,
            "baseRepoPath": repo_path.display().to_string(),
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, create_error.error.code);
    assert_eq!(
        "managed worktrees are disabled for this session",
        create_error.error.message
    );

    let attach_error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-shared",
            "threadId": thread_id,
            "agentRunId": null,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, attach_error.error.code);
    assert_eq!(
        "managed worktrees are disabled for this session",
        attach_error.error.message
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_request_worktree_mode_requires_attached_worktree_for_turns() -> Result<()> {
    let codex_home = TempDir::new()?;
    init_git_repo(codex_home.path())?;
    let server =
        create_mock_responses_server_sequence(vec![create_final_assistant_message_sse_response(
            "done",
        )?])
        .await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread_for_worktree_attach(&mut mcp).await?;
    upsert_thread_for_worktree_attach(state_db.as_ref(), thread_id.as_str(), codex_home.path())
        .await?;
    update_thread_worktree_mode(
        &mut mcp,
        thread_id.as_str(),
        codex_protocol::protocol::SessionWorktreeMode::PullRequest,
    )
    .await?;

    let turn_request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.clone(),
            input: vec![V2UserInput::Text {
                text: "implement the PR-mode feature".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(turn_request_id)),
    )
    .await??;
    assert_eq!(INVALID_REQUEST_ERROR_CODE, turn_error.error.code);
    assert_eq!(
        "pull-request mode requires an attached active managed worktree before starting a turn",
        turn_error.error.message
    );

    let retain_cleanup_error = raw_request_error(
        &mut mcp,
        "worktree/create",
        json!({
            "name": "pr-retain-denied",
            "startPoint": "HEAD",
            "threadId": thread_id,
            "cleanupPolicy": "retain",
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, retain_cleanup_error.error.code);
    assert_eq!(
        "pull-request mode worktrees use deleteIfClean cleanup policy",
        retain_cleanup_error.error.message
    );

    let create_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "pr-mode",
                "startPoint": "HEAD",
                "threadId": thread_id,
            })),
        )
        .await?;
    let created: WorktreeCreateResponse = read_response(&mut mcp, create_request_id).await?;
    assert_eq!(
        WorktreeCleanupPolicy::DeleteIfClean,
        created.worktree.cleanup_policy
    );
    assert_eq!(WorktreeOwnerKind::MainSession, created.worktree.owner_kind);
    assert_eq!(Some(thread_id.clone()), created.worktree.owner_thread_id);
    assert!(
        created
            .worktree
            .identity
            .as_deref()
            .is_some_and(|identity| identity.starts_with("pr-mode:"))
    );

    let outside_cwd_turn_request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.clone(),
            input: vec![V2UserInput::Text {
                text: "try to keep working from the shared checkout".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let outside_cwd_error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(outside_cwd_turn_request_id)),
    )
    .await??;
    assert_eq!(INVALID_REQUEST_ERROR_CODE, outside_cwd_error.error.code);
    assert!(
        outside_cwd_error
            .error
            .message
            .contains("pull-request mode requires cwd"),
        "unexpected error: {}",
        outside_cwd_error.error.message
    );

    let successful_turn_request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.clone(),
            cwd: Some(std::path::PathBuf::from(
                created.worktree.worktree_path.as_str(),
            )),
            input: vec![V2UserInput::Text {
                text: "now work inside the PR worktree".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let successful_turn_response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(successful_turn_request_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response(successful_turn_response)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_merge_candidate_refresh_and_apply_use_real_git_merge() -> Result<()> {
    let codex_home = TempDir::new()?;
    let repo_path = codex_home.path().join("repo");
    std::fs::create_dir(&repo_path)?;
    init_git_repo(repo_path.as_path())?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    std::fs::write(repo_path.join(".gitignore"), ".codewith/\n")?;
    git(repo_path.as_path(), &["add", ".gitignore"])?;
    git(repo_path.as_path(), &["commit", "-m", "ignore worktrees"])?;
    let base_repo_path = repo_path.display().to_string();

    let mut mcp = init_mcp_with_cwd(codex_home.path(), repo_path.as_path()).await?;
    let create_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "mergeable",
                "startPoint": "HEAD",
                "baseRepoPath": base_repo_path,
            })),
        )
        .await?;
    let created: WorktreeCreateResponse = read_response(&mut mcp, create_request_id).await?;
    let worktree_path = std::path::PathBuf::from(created.worktree.worktree_path.as_str());
    std::fs::write(worktree_path.join("feature.txt"), "merge candidate\n")?;
    git(worktree_path.as_path(), &["add", "feature.txt"])?;
    git(
        worktree_path.as_path(),
        &["commit", "-m", "add merge candidate"],
    )?;

    let refresh_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/refresh",
            Some(json!({
                "worktreeId": created.worktree.worktree_id,
                "targetRef": "HEAD",
            })),
        )
        .await?;
    let refreshed: WorktreeMergeCandidateRefreshResponse =
        read_response(&mut mcp, refresh_request_id).await?;
    assert_eq!(
        WorktreeMergeCandidateStatus::Open,
        refreshed.candidate.status
    );
    let candidate_id = refreshed.candidate.candidate_id.clone();
    let list_open_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/list",
            Some(json!({
                "worktreeId": created.worktree.worktree_id,
                "status": "open",
            })),
        )
        .await?;
    let open_candidates: WorktreeMergeCandidateListResponse =
        read_response(&mut mcp, list_open_request_id).await?;
    assert_eq!(
        vec![candidate_id.clone()],
        open_candidates
            .data
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect::<Vec<_>>()
    );

    let dismiss_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/dismiss",
            Some(json!({
                "candidateId": candidate_id,
            })),
        )
        .await?;
    let dismissed: WorktreeMergeCandidateDismissResponse =
        read_response(&mut mcp, dismiss_request_id).await?;
    assert_eq!(
        Some(WorktreeMergeCandidateStatus::Dismissed),
        dismissed.candidate.map(|candidate| candidate.status)
    );
    let list_dismissed_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/list",
            Some(json!({
                "worktreeId": created.worktree.worktree_id,
                "status": "dismissed",
            })),
        )
        .await?;
    let dismissed_candidates: WorktreeMergeCandidateListResponse =
        read_response(&mut mcp, list_dismissed_request_id).await?;
    assert_eq!(1, dismissed_candidates.data.len());

    let refresh_again_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/refresh",
            Some(json!({
                "worktreeId": created.worktree.worktree_id,
                "targetRef": "HEAD",
            })),
        )
        .await?;
    let refreshed_again: WorktreeMergeCandidateRefreshResponse =
        read_response(&mut mcp, refresh_again_request_id).await?;
    assert_eq!(
        WorktreeMergeCandidateStatus::Open,
        refreshed_again.candidate.status
    );
    let stale_candidate_id = refreshed_again.candidate.candidate_id.clone();
    std::fs::write(worktree_path.join("later.txt"), "later work\n")?;
    git(worktree_path.as_path(), &["add", "later.txt"])?;
    git(worktree_path.as_path(), &["commit", "-m", "add later work"])?;
    let stale_apply_error = raw_request_error(
        &mut mcp,
        "worktree/mergeCandidate/apply",
        json!({
            "candidateId": stale_candidate_id,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, stale_apply_error.error.code);
    assert_eq!(
        "worktree/mergeCandidate/apply source changed; refresh before applying",
        stale_apply_error.error.message
    );

    let final_refresh_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/refresh",
            Some(json!({
                "worktreeId": created.worktree.worktree_id,
                "targetRef": "HEAD",
            })),
        )
        .await?;
    let final_refreshed: WorktreeMergeCandidateRefreshResponse =
        read_response(&mut mcp, final_refresh_request_id).await?;
    assert_eq!(
        WorktreeMergeCandidateStatus::Open,
        final_refreshed.candidate.status
    );
    let candidate_id = final_refreshed.candidate.candidate_id.clone();
    std::fs::write(repo_path.join("target-untracked.txt"), "target dirt\n")?;
    let dirty_target_apply_error = raw_request_error(
        &mut mcp,
        "worktree/mergeCandidate/apply",
        json!({
            "candidateId": candidate_id,
        }),
    )
    .await?;
    assert_eq!(
        INVALID_PARAMS_ERROR_CODE,
        dirty_target_apply_error.error.code
    );
    assert_eq!(
        "worktree/mergeCandidate/apply requires a clean target checkout",
        dirty_target_apply_error.error.message
    );
    std::fs::remove_file(repo_path.join("target-untracked.txt"))?;

    let apply_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/apply",
            Some(json!({
                "candidateId": candidate_id.clone(),
            })),
        )
        .await?;
    let applied: WorktreeMergeCandidateApplyResponse =
        read_response(&mut mcp, apply_request_id).await?;
    assert_eq!(
        Some(WorktreeMergeCandidateStatus::Applied),
        applied.candidate.map(|candidate| candidate.status)
    );
    assert_eq!(
        "merge candidate\n",
        std::fs::read_to_string(repo_path.join("feature.txt"))?.replace("\r\n", "\n")
    );
    assert_eq!(
        "later work\n",
        std::fs::read_to_string(repo_path.join("later.txt"))?
    );
    let race_create_request_id = mcp
        .send_raw_request(
            "worktree/create",
            Some(json!({
                "name": "target-race",
                "startPoint": "HEAD",
            })),
        )
        .await?;
    let race_created: WorktreeCreateResponse =
        read_response(&mut mcp, race_create_request_id).await?;
    let race_worktree_path = std::path::PathBuf::from(race_created.worktree.worktree_path.as_str());
    std::fs::write(race_worktree_path.join("race.txt"), "race candidate\n")?;
    git(race_worktree_path.as_path(), &["add", "race.txt"])?;
    git(
        race_worktree_path.as_path(),
        &["commit", "-m", "add race candidate"],
    )?;
    let race_refresh_request_id = mcp
        .send_raw_request(
            "worktree/mergeCandidate/refresh",
            Some(json!({
                "worktreeId": race_created.worktree.worktree_id,
                "targetRef": "HEAD",
            })),
        )
        .await?;
    let race_refreshed: WorktreeMergeCandidateRefreshResponse =
        read_response(&mut mcp, race_refresh_request_id).await?;
    assert_eq!(
        WorktreeMergeCandidateStatus::Open,
        race_refreshed.candidate.status
    );
    std::fs::write(repo_path.join("main-advanced.txt"), "main advanced\n")?;
    git(repo_path.as_path(), &["add", "main-advanced.txt"])?;
    git(
        repo_path.as_path(),
        &["commit", "-m", "advance merge target"],
    )?;
    let target_changed_apply_error = raw_request_error(
        &mut mcp,
        "worktree/mergeCandidate/apply",
        json!({
            "candidateId": race_refreshed.candidate.candidate_id,
        }),
    )
    .await?;
    assert_eq!(
        INVALID_PARAMS_ERROR_CODE,
        target_changed_apply_error.error.code
    );
    assert!(
        target_changed_apply_error
            .error
            .message
            .starts_with("worktree/mergeCandidate/apply target changed from "),
        "unexpected target changed error: {}",
        target_changed_apply_error.error.message
    );
    assert!(
        target_changed_apply_error
            .error
            .message
            .ends_with("; refresh before applying"),
        "unexpected target changed error: {}",
        target_changed_apply_error.error.message
    );
    assert!(!codex_home.path().join("race.txt").exists());
    assert_eq!(
        "race candidate\n",
        std::fs::read_to_string(race_worktree_path.join("race.txt"))?
    );
    assert_eq!(
        "main advanced\n",
        std::fs::read_to_string(repo_path.join("main-advanced.txt"))?
    );

    let dismiss_applied_error = raw_request_error(
        &mut mcp,
        "worktree/mergeCandidate/dismiss",
        json!({
            "candidateId": candidate_id.clone(),
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, dismiss_applied_error.error.code);
    assert_eq!(
        "worktree/mergeCandidate/dismiss requires an open or blocked candidate",
        dismiss_applied_error.error.message
    );

    let apply_again_error = raw_request_error(
        &mut mcp,
        "worktree/mergeCandidate/apply",
        json!({
            "candidateId": candidate_id,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, apply_again_error.error.code);
    assert_eq!(
        "worktree/mergeCandidate/apply requires an open candidate",
        apply_again_error.error.message
    );

    let dismiss_applied_error = raw_request_error(
        &mut mcp,
        "worktree/mergeCandidate/dismiss",
        json!({
            "candidateId": candidate_id.clone(),
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, dismiss_applied_error.error.code);
    assert_eq!(
        "worktree/mergeCandidate/dismiss requires an open or blocked candidate",
        dismiss_applied_error.error.message
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_attach_assigns_thread_and_rejects_ambiguous_targets() -> Result<()> {
    let codex_home = TempDir::new()?;
    let repo_path = codex_home.path().canonicalize()?;
    std::fs::create_dir(repo_path.join(".git"))?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config(codex_home.path(), server.uri().as_str())?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-attach", repo_path.as_path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-agent-attach", repo_path.as_path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-unknown-agent", repo_path.as_path()).await?;
    seed_queued_agent_run(
        state_db.as_ref(),
        "agent-run-attach",
        /*idempotency_key*/ None,
        "attach this agent to a worktree",
    )
    .await?;
    state_db
        .update_background_agent_run_status(
            "agent-run-attach",
            StateBackgroundAgentRunStatus::WaitingOnUser,
            Some("waiting for attach test"),
        )
        .await?;
    drop(state_db);

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id_string = start_thread_for_worktree_attach(&mut mcp).await?;
    upsert_thread_for_worktree_attach_with_retry(
        codex_home.path(),
        thread_id_string.as_str(),
        repo_path.as_path(),
    )
    .await?;
    let attach_request_id = mcp
        .send_raw_request(
            "worktree/attach",
            Some(json!({
                "worktreeId": "wt-attach",
                "threadId": thread_id_string,
                "agentRunId": null,
            })),
        )
        .await?;
    let attach: WorktreeAttachResponse = read_response(&mut mcp, attach_request_id).await?;
    assert_eq!("wt-attach", attach.worktree.worktree_id);
    assert_eq!(WorktreeOwnerKind::MainSession, attach.worktree.owner_kind);
    assert_eq!(
        Some(thread_id_string.clone()),
        attach.worktree.owner_thread_id
    );
    assert_eq!(None, attach.worktree.owner_agent_run_id);

    let read_request_id = mcp
        .send_raw_request(
            "worktree/read",
            Some(json!({
                "worktreeId": "wt-attach",
            })),
        )
        .await?;
    let read: WorktreeReadResponse = read_response(&mut mcp, read_request_id).await?;
    let worktree = read.worktree.expect("attached worktree should be readable");
    assert_eq!(Some(thread_id_string.clone()), worktree.owner_thread_id);
    assert_eq!(None, worktree.owner_agent_run_id);

    let detach_request_id = mcp
        .send_raw_request(
            "worktree/detach",
            Some(json!({
                "worktreeId": "wt-attach",
                "threadId": thread_id_string,
                "agentRunId": null,
            })),
        )
        .await?;
    let detached: WorktreeDetachResponse = read_response(&mut mcp, detach_request_id).await?;
    let detached = detached
        .worktree
        .expect("detached worktree should be readable");
    assert_eq!(WorktreeOwnerKind::Manual, detached.owner_kind);
    assert_eq!(None, detached.owner_thread_id);
    assert_eq!(None, detached.owner_agent_run_id);

    let agent_attach_request_id = mcp
        .send_raw_request(
            "worktree/attach",
            Some(json!({
                "worktreeId": "wt-agent-attach",
                "threadId": null,
                "agentRunId": "agent-run-attach",
            })),
        )
        .await?;
    let agent_attach: WorktreeAttachResponse =
        read_response(&mut mcp, agent_attach_request_id).await?;
    assert_eq!("wt-agent-attach", agent_attach.worktree.worktree_id);
    assert_eq!(
        WorktreeOwnerKind::BackgroundAgent,
        agent_attach.worktree.owner_kind
    );
    assert_eq!(None, agent_attach.worktree.owner_thread_id);
    assert_eq!(
        Some("agent-run-attach".to_string()),
        agent_attach.worktree.owner_agent_run_id
    );

    let unknown_agent_error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-unknown-agent",
            "threadId": null,
            "agentRunId": "missing-agent-run",
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, unknown_agent_error.error.code);
    assert_eq!(
        "worktree/attach agentRunId `missing-agent-run` does not exist",
        unknown_agent_error.error.message
    );

    let both_error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-attach",
            "threadId": thread_id_string,
            "agentRunId": "agent-run-1",
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, both_error.error.code);
    assert_eq!(
        "worktree/attach accepts only one of threadId or agentRunId",
        both_error.error.message
    );

    let neither_error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-attach",
            "threadId": null,
            "agentRunId": null,
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, neither_error.error.code);
    assert_eq!(
        "worktree/attach requires one of threadId or agentRunId",
        neither_error.error.message
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_attach_rejects_globally_disabled_policy() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config_with_extra(
        codex_home.path(),
        server.uri().as_str(),
        r#"
[worktrees]
enabled = false
"#,
    )?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-disabled", codex_home.path()).await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-disabled",
            "threadId": "00000000-0000-0000-0000-000000000901",
        }),
    )
    .await?;

    assert_eq!(INVALID_PARAMS_ERROR_CODE, error.error.code);
    assert_eq!(
        "managed worktrees are disabled in config",
        error.error.message
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_attach_rejects_disabled_session_classes() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_config_with_extra(
        codex_home.path(),
        server.uri().as_str(),
        r#"
[worktrees.main_sessions]
mode = "off"

[worktrees.sub_sessions]
mode = "off"
"#,
    )?;
    let state_db = init_state_db(codex_home.path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-main-off", codex_home.path()).await?;
    create_managed_worktree(state_db.as_ref(), "wt-sub-off", codex_home.path()).await?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let main_error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-main-off",
            "threadId": "00000000-0000-0000-0000-000000000902",
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, main_error.error.code);
    assert_eq!(
        "main-session worktrees are disabled in config",
        main_error.error.message
    );

    let sub_error = raw_request_error(
        &mut mcp,
        "worktree/attach",
        json!({
            "worktreeId": "wt-sub-off",
            "agentRunId": "agent-worktree-off",
        }),
    )
    .await?;
    assert_eq!(INVALID_PARAMS_ERROR_CODE, sub_error.error.code);
    assert_eq!(
        "sub-session worktrees are disabled in config",
        sub_error.error.message
    );
    Ok(())
}

async fn init_mcp(codex_home: &Path) -> Result<McpProcess> {
    let mut mcp = McpProcess::new(codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
}

async fn init_mcp_with_cwd(codex_home: &Path, cwd: &Path) -> Result<McpProcess> {
    let mut mcp = McpProcess::new_with_cwd(codex_home, cwd).await?;
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

fn init_git_repo(repo_path: &Path) -> Result<()> {
    git(repo_path, &["init"])?;
    git(repo_path, &["config", "core.autocrlf", "false"])?;
    git(repo_path, &["config", "user.email", "codewith@example.com"])?;
    git(repo_path, &["config", "user.name", "Codewith Test"])?;
    std::fs::write(repo_path.join("README.md"), "worktree test\n")?;
    git(repo_path, &["add", "README.md"])?;
    git(repo_path, &["commit", "-m", "initial"])?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn protocol_path(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let path = path.to_string_lossy().into_owned();
    strip_windows_verbatim_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: String) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_owned();
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: String) -> String {
    path
}

async fn create_managed_worktree(
    state_db: &codex_state::StateRuntime,
    worktree_id: &str,
    base_repo_path: &Path,
) -> Result<()> {
    create_managed_worktree_with_mode(
        state_db,
        worktree_id,
        base_repo_path,
        codex_state::ManagedWorktreeMode::IsolatedWorktree,
    )
    .await
}

async fn create_managed_worktree_with_mode(
    state_db: &codex_state::StateRuntime,
    worktree_id: &str,
    base_repo_path: &Path,
    mode: codex_state::ManagedWorktreeMode,
) -> Result<()> {
    state_db
        .managed_worktrees()
        .create_managed_worktree(codex_state::ManagedWorktreeCreateParams {
            worktree_id: Some(worktree_id.to_string()),
            identity: Some(format!("test:{worktree_id}")),
            mode,
            base_repo_path: base_repo_path.to_path_buf(),
            worktree_path: base_repo_path
                .join(".codewith")
                .join("worktrees")
                .join(worktree_id),
            branch: Some(format!("codewith/{worktree_id}")),
            base_sha: None,
            head_sha: None,
            status_snapshot_json: json!({"status": "ready"}),
            dirty: false,
            cleanup_policy: codex_state::ManagedWorktreeCleanupPolicy::DeleteIfClean,
            owner_kind: codex_state::ManagedWorktreeOwnerKind::MainSession,
            owner_thread_id: None,
            owner_agent_run_id: None,
            cleanup_after: None,
        })
        .await?;
    Ok(())
}

async fn create_background_agent_git_worktree_lease(
    state_db: &codex_state::StateRuntime,
    agent_id: &str,
    lease_id: &str,
    codex_home: &Path,
    run_status: StateBackgroundAgentRunStatus,
) -> Result<()> {
    seed_queued_agent_run(
        state_db,
        agent_id,
        /*idempotency_key*/ None,
        "release a leased background-agent worktree",
    )
    .await?;
    let branch = format!("codewith/{agent_id}");
    let base_sha_output = Command::new("git")
        .current_dir(codex_home)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !base_sha_output.status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&base_sha_output.stderr)
        );
    }
    let base_sha = String::from_utf8(base_sha_output.stdout)?
        .trim()
        .to_string();
    let worktree_path = codex_home
        .join(".codewith")
        .join("worktrees")
        .join(agent_id);
    let worktree_parent = worktree_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("worktree path has no parent"))?;
    std::fs::create_dir_all(worktree_parent)?;
    git(
        codex_home,
        &[
            "worktree",
            "add",
            "-b",
            branch.as_str(),
            worktree_path.to_string_lossy().as_ref(),
            "HEAD",
        ],
    )?;
    state_db
        .create_background_agent_worktree_lease(
            &codex_state::BackgroundAgentWorktreeLeaseCreateParams {
                id: lease_id.to_string(),
                run_id: agent_id.to_string(),
                identity: format!("test:{lease_id}"),
                mode: codex_state::BackgroundAgentWorkspaceMode::IsolatedWorktree,
                base_repo_path: codex_home.to_string_lossy().to_string(),
                worktree_path: worktree_path.to_string_lossy().to_string(),
                branch: Some(branch),
                head_sha: Some(base_sha),
                status_snapshot_json: json!({"dirty": false, "source": "seed"}),
                dirty: false,
                cleanup_after: None,
            },
        )
        .await?;
    state_db
        .update_background_agent_run_status(
            agent_id,
            run_status,
            Some("worktree lease test status"),
        )
        .await?;
    Ok(())
}

async fn start_thread_for_worktree_attach(mcp: &mut McpProcess) -> Result<String> {
    start_thread_for_worktree_attach_with_source(mcp, /*thread_source*/ None).await
}

async fn start_thread_for_worktree_attach_with_source(
    mcp: &mut McpProcess,
    thread_source: Option<ThreadSource>,
) -> Result<String> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            thread_source,
            ..Default::default()
        })
        .await?;
    let response: ThreadStartResponse = read_response(mcp, request_id).await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/started"),
    )
    .await??;
    Ok(response.thread.id)
}

async fn upsert_thread_for_worktree_attach_with_retry(
    codex_home: &Path,
    thread_id: &str,
    cwd: &Path,
) -> Result<()> {
    for attempt in 0..5 {
        let result = async {
            let state_db = init_state_db(codex_home).await?;
            upsert_thread_for_worktree_attach(state_db.as_ref(), thread_id, cwd).await
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(err) if sqlite_lock_error(&err) && attempt < 4 => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

async fn update_thread_worktree_mode(
    mcp: &mut McpProcess,
    thread_id: &str,
    worktree_mode: codex_protocol::protocol::SessionWorktreeMode,
) -> Result<()> {
    let request_id = mcp
        .send_thread_settings_update_request(ThreadSettingsUpdateParams {
            thread_id: thread_id.to_string(),
            worktree_mode: Some(worktree_mode),
            ..Default::default()
        })
        .await?;
    let _: ThreadSettingsUpdateResponse = read_response(mcp, request_id).await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/settings/updated"),
    )
    .await??;
    Ok(())
}

async fn upsert_thread_for_worktree_attach(
    state_db: &codex_state::StateRuntime,
    thread_id: &str,
    cwd: &Path,
) -> Result<()> {
    let thread_id = ThreadId::from_string(thread_id)?;
    let rollout_path = state_db
        .codex_home()
        .join(format!("rollout-{thread_id}.jsonl"));
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
        .ok_or_else(|| anyhow::anyhow!("timestamp should parse"))?;
    state_db
        .upsert_thread(&codex_state::ThreadMetadata {
            id: thread_id,
            rollout_path,
            created_at: now,
            updated_at: now,
            source: "cli".to_string(),
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "mock_provider".to_string(),
            model: Some("mock-model".to_string()),
            reasoning_effort: None,
            cwd: cwd.to_path_buf(),
            cli_version: "0.0.0".to_string(),
            title: String::new(),
            preview: Some("worktree attach target".to_string()),
            sandbox_policy: "read-only".to_string(),
            approval_mode: "never".to_string(),
            tokens_used: 0,
            first_user_message: Some("worktree attach target".to_string()),
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
        })
        .await?;
    Ok(())
}

fn sqlite_lock_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("database is locked")
        || message.contains("database is busy")
        || message.contains("code: 5")
        || message.contains("code: 517")
}

fn worktree_ids(response: &WorktreeListResponse) -> Vec<String> {
    response
        .data
        .iter()
        .map(|worktree| worktree.worktree_id.clone())
        .collect()
}

fn worktree_ids_from_candidates(response: &WorktreeMergeCandidateListResponse) -> Vec<String> {
    response
        .data
        .iter()
        .map(|candidate| candidate.worktree_id.clone())
        .collect()
}

fn write_config(codex_home: &Path, server_uri: &str) -> Result<()> {
    write_config_with_extra(codex_home, server_uri, "")
}

fn write_config_with_extra(codex_home: &Path, server_uri: &str, extra_toml: &str) -> Result<()> {
    write_config_with_sandbox_mode_and_extra(codex_home, server_uri, "read-only", extra_toml)
}

fn write_config_with_sandbox_mode_and_extra(
    codex_home: &Path,
    server_uri: &str,
    sandbox_mode: &str,
    extra_toml: &str,
) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "{sandbox_mode}"
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
{extra_toml}
"#,
        ),
    )?;
    Ok(())
}

async fn raw_request_error(
    mcp: &mut McpProcess,
    method: &str,
    params: JsonValue,
) -> Result<JSONRPCError> {
    let request_id = mcp.send_raw_request(method, Some(params)).await?;
    let response = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(response)
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
        initial_goal_objective: None,
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
            approval_policy: Some(AskForApproval::Never),
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

async fn agent_pending_interaction_respond_error(
    mcp: &mut McpProcess,
    params: AgentPendingInteractionRespondParams,
) -> Result<JSONRPCError> {
    let request_id = mcp
        .send_agent_pending_interaction_respond_request(params)
        .await?;
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
