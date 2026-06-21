use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use chrono::DateTime;
use chrono::Utc;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LocalSessionStatus;
use codex_app_server_protocol::MissionControlDeliveryPolicy;
use codex_app_server_protocol::MissionControlEnqueueInstructionParams;
use codex_app_server_protocol::MissionControlEnqueueInstructionResponse;
use codex_app_server_protocol::MissionControlMailboxReceiptsParams;
use codex_app_server_protocol::MissionControlMailboxReceiptsResponse;
use codex_app_server_protocol::MissionControlOverviewParams;
use codex_app_server_protocol::MissionControlOverviewResponse;
use codex_app_server_protocol::MissionControlRespondInteractionParams;
use codex_app_server_protocol::MissionControlRespondInteractionResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_app_server_protocol::ThreadMailboxMessageStatus;
use codex_app_server_protocol::ThreadPendingInteractionKind;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_protocol::ThreadId;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::time::Instant;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn mission_control_overview_lists_sessions_goal_plans_and_pending_interactions() -> Result<()>
{
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let state_db =
        StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into()).await?;
    let parsed_thread_id = ThreadId::from_string(thread_id.as_str())?;
    state_db
        .thread_goals()
        .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
            thread_id: parsed_thread_id,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            max_tokens: None,
            nodes: vec![codex_state::ThreadGoalPlanNodeCreateParams {
                key: "blocked".to_string(),
                objective: "Wait for coordinator input".to_string(),
                priority: 0,
                token_budget: None,
                depends_on: Vec::new(),
            }],
        })
        .await?;
    state_db
        .thread_schedules()
        .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
            thread_id: parsed_thread_id,
            prompt: "Check release blockers".to_string(),
            prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
            schedule: codex_state::ThreadScheduleSpec::Once,
            timezone: "UTC".to_string(),
            status: codex_state::ThreadScheduleStatus::Active,
            next_run_at: DateTime::<Utc>::from_timestamp(1_900, 0),
            expires_at: None,
        })
        .await?;

    let set_goal_id = mcp
        .send_raw_request(
            "thread/goal/set",
            Some(json!({
                "threadId": thread_id,
                "status": "blocked",
            })),
        )
        .await?;
    let set_goal_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(set_goal_id)),
    )
    .await??;
    let _ = set_goal_resp;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/goalPlan/updated"),
    )
    .await??;

    let overview = mission_control_overview(
        &mut mcp,
        MissionControlOverviewParams {
            limit: Some(10),
            include_goal_plans: true,
            pending_interaction_statuses: Some(vec![ThreadPendingInteractionStatus::Pending]),
            pending_interaction_limit: Some(10),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(overview.next_session_cursor, None);
    assert_eq!(overview.next_pending_interaction_cursor, None);
    assert_eq!(overview.capabilities.local_sessions, true);
    assert_eq!(overview.capabilities.durable_mailbox, true);
    assert_eq!(overview.capabilities.pending_interactions, true);
    assert_eq!(overview.capabilities.goals, true);
    assert_eq!(overview.capabilities.scheduled_tasks, true);
    assert_eq!(overview.capabilities.remote_dispatch, false);
    assert_eq!(overview.capabilities.workflow_mutation, false);
    assert_eq!(overview.capabilities.shell_execution, false);
    assert_eq!(overview.capabilities.filesystem_mutation, false);

    let session = overview
        .sessions
        .iter()
        .find(|session| session.session.thread_id == thread_id)
        .expect("mission-control overview should include the local session");
    assert_eq!(session.session.status, LocalSessionStatus::Idle);
    assert_eq!(
        session.goal.as_ref().map(|goal| goal.objective.as_str()),
        Some("Wait for coordinator input")
    );
    assert_eq!(session.goal_plans.len(), 1);
    assert_eq!(session.goal_plans[0].status, ThreadGoalPlanStatus::Blocked);
    assert_eq!(
        session.goal_plans[0].nodes[0].status,
        ThreadGoalPlanNodeStatus::Blocked
    );
    assert_eq!(session.schedules.len(), 1);
    assert_eq!(session.schedules[0].prompt, "Check release blockers");
    assert_eq!(
        session.schedules[0].status,
        codex_app_server_protocol::ThreadScheduleStatus::Active
    );

    assert_eq!(overview.pending_interactions.len(), 1);
    let pending = &overview.pending_interactions[0];
    assert_eq!(pending.thread_id, thread_id);
    assert_eq!(pending.kind, ThreadPendingInteractionKind::Blocked);
    assert_eq!(pending.status, ThreadPendingInteractionStatus::Pending);
    assert_eq!(
        pending.request_payload["reason"],
        json!("external-goal-blocked")
    );
    Ok(())
}

#[tokio::test]
async fn mission_control_enqueue_instruction_supports_dry_run_idempotency_and_receipts()
-> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;

    let dry_run = mission_control_enqueue_instruction(
        &mut mcp,
        MissionControlEnqueueInstructionParams {
            target_thread_id: thread_id.clone(),
            message: "  decompose yesterday's notes  ".to_string(),
            sender_thread_id: None,
            sender_label: Some("coordinator".to_string()),
            idempotency_key: Some("mission-control-dry-run".to_string()),
            priority: Some(5),
            max_attempts: Some(3),
            expires_at: None,
            resume: true,
            dry_run: true,
        },
    )
    .await?;
    assert_eq!(dry_run.dry_run, true);
    assert_eq!(
        dry_run.delivery_policy,
        MissionControlDeliveryPolicy::ResumeAndTrigger
    );
    assert_eq!(dry_run.preview, "decompose yesterday's notes");
    assert_eq!(dry_run.message, None);
    assert_eq!(dry_run.created, None);

    let first = mission_control_enqueue_instruction(
        &mut mcp,
        MissionControlEnqueueInstructionParams {
            target_thread_id: thread_id.clone(),
            message: "decompose yesterday's notes".to_string(),
            sender_thread_id: None,
            sender_label: Some("coordinator".to_string()),
            idempotency_key: Some("mission-control-enqueue".to_string()),
            priority: Some(5),
            max_attempts: Some(3),
            expires_at: None,
            resume: true,
            dry_run: false,
        },
    )
    .await?;
    let second = mission_control_enqueue_instruction(
        &mut mcp,
        MissionControlEnqueueInstructionParams {
            target_thread_id: thread_id.clone(),
            message: "decompose yesterday's notes".to_string(),
            sender_thread_id: None,
            sender_label: Some("coordinator".to_string()),
            idempotency_key: Some("mission-control-enqueue".to_string()),
            priority: Some(5),
            max_attempts: Some(3),
            expires_at: None,
            resume: true,
            dry_run: false,
        },
    )
    .await?;
    assert_eq!(first.dry_run, false);
    assert_eq!(first.created, Some(true));
    assert_eq!(second.created, Some(false));
    let first_message = first.message.expect("first enqueue should return message");
    let second_message = second
        .message
        .expect("second enqueue should return message");
    assert_eq!(first_message.message_id, second_message.message_id);
    assert_eq!(first_message.status, ThreadMailboxMessageStatus::Queued);
    assert_eq!(
        first.delivery_policy,
        MissionControlDeliveryPolicy::ResumeAndTrigger
    );

    let receipts =
        mission_control_mailbox_receipts(&mut mcp, &thread_id, &first_message.message_id).await?;
    assert_eq!(receipts.data.len(), 1);
    assert_eq!(
        receipts.data[0].kind,
        codex_app_server_protocol::ThreadMailboxReceiptKind::Enqueued
    );
    Ok(())
}

#[tokio::test]
async fn mission_control_overview_paginates_sessions() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let first = start_thread(&mut mcp).await?;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let second = start_thread(&mut mcp).await?;

    let first_page = mission_control_overview(
        &mut mcp,
        MissionControlOverviewParams {
            limit: Some(1),
            session_statuses: Some(vec![LocalSessionStatus::Idle]),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(first_page.sessions.len(), 1);
    assert!(first_page.next_session_cursor.is_some());

    let second_page = mission_control_overview(
        &mut mcp,
        MissionControlOverviewParams {
            cursor: first_page.next_session_cursor,
            limit: Some(1),
            session_statuses: Some(vec![LocalSessionStatus::Idle]),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(second_page.sessions.len(), 1);
    assert_eq!(second_page.next_session_cursor, None);
    assert_ne!(
        first_page.sessions[0].session.thread_id,
        second_page.sessions[0].session.thread_id
    );
    let mut returned = vec![
        first_page.sessions[0].session.thread_id.clone(),
        second_page.sessions[0].session.thread_id.clone(),
    ];
    returned.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(returned, expected);
    Ok(())
}

#[tokio::test]
async fn mission_control_respond_interaction_dry_run_validates_without_mutating() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = start_thread(&mut mcp).await?;
    let goal_id = mcp
        .send_raw_request(
            "thread/goal/set",
            Some(json!({
                "threadId": thread_id,
                "objective": "wait for coordinator",
                "status": "blocked",
            })),
        )
        .await?;
    let goal_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(goal_id)),
    )
    .await??;
    let _ = goal_resp;

    let overview = wait_for_pending_mission_control_overview(
        &mut mcp,
        MissionControlOverviewParams {
            pending_interaction_statuses: Some(vec![ThreadPendingInteractionStatus::Pending]),
            pending_interaction_limit: Some(10),
            ..Default::default()
        },
    )
    .await?;
    let interaction = overview
        .pending_interactions
        .first()
        .expect("blocked goal should create pending interaction")
        .clone();

    let response = ThreadPendingInteractionResponsePayload::Terminal {
        reason: "coordinator recorded the answer".to_string(),
    };
    let dry_run = mission_control_respond_interaction(
        &mut mcp,
        MissionControlRespondInteractionParams {
            interaction_id: interaction.interaction_id.clone(),
            thread_id: Some(thread_id.clone()),
            terminal_status: ThreadPendingInteractionTerminalStatus::Responded,
            response: response.clone(),
            dry_run: true,
        },
    )
    .await?;
    assert_eq!(dry_run.dry_run, true);
    assert_eq!(dry_run.updated, false);
    assert_eq!(
        dry_run.interaction.as_ref().map(|item| item.status),
        Some(ThreadPendingInteractionStatus::Pending)
    );

    let applied = mission_control_respond_interaction(
        &mut mcp,
        MissionControlRespondInteractionParams {
            interaction_id: interaction.interaction_id,
            thread_id: Some(thread_id.clone()),
            terminal_status: ThreadPendingInteractionTerminalStatus::Responded,
            response,
            dry_run: false,
        },
    )
    .await?;
    assert_eq!(applied.dry_run, false);
    assert_eq!(applied.updated, true);
    assert_eq!(
        applied.interaction.as_ref().map(|item| item.status),
        Some(ThreadPendingInteractionStatus::Responded)
    );
    Ok(())
}

async fn init_mcp(codex_home: &Path) -> Result<TestAppServer> {
    let mut mcp = TestAppServer::new_without_managed_config(codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
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
                text: format!("seed mission control {}", thread.id),
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

async fn mission_control_overview(
    mcp: &mut TestAppServer,
    params: MissionControlOverviewParams,
) -> Result<MissionControlOverviewResponse> {
    let request_id = mcp.send_mission_control_overview_request(params).await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<MissionControlOverviewResponse>(resp)
}

async fn wait_for_pending_mission_control_overview(
    mcp: &mut TestAppServer,
    params: MissionControlOverviewParams,
) -> Result<MissionControlOverviewResponse> {
    let started_at = Instant::now();
    loop {
        let overview = mission_control_overview(mcp, params.clone()).await?;
        if !overview.pending_interactions.is_empty() {
            return Ok(overview);
        }
        if started_at.elapsed() > DEFAULT_READ_TIMEOUT {
            anyhow::bail!("timed out waiting for mission-control pending interactions");
        }
        sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn mission_control_enqueue_instruction(
    mcp: &mut TestAppServer,
    params: MissionControlEnqueueInstructionParams,
) -> Result<MissionControlEnqueueInstructionResponse> {
    let request_id = mcp
        .send_mission_control_enqueue_instruction_request(params)
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<MissionControlEnqueueInstructionResponse>(resp)
}

async fn mission_control_mailbox_receipts(
    mcp: &mut TestAppServer,
    thread_id: &str,
    message_id: &str,
) -> Result<MissionControlMailboxReceiptsResponse> {
    let request_id = mcp
        .send_mission_control_mailbox_receipts_request(MissionControlMailboxReceiptsParams {
            target_thread_id: thread_id.to_string(),
            message_id: message_id.to_string(),
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<MissionControlMailboxReceiptsResponse>(resp)
}

async fn mission_control_respond_interaction(
    mcp: &mut TestAppServer,
    params: MissionControlRespondInteractionParams,
) -> Result<MissionControlRespondInteractionResponse> {
    let request_id = mcp
        .send_mission_control_respond_interaction_request(params)
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<MissionControlRespondInteractionResponse>(resp)
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

[features]
goals = true
personality = true

[goals]
auto_execute = "ready-only"

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
