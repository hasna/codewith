use super::*;
use codex_app_server_protocol::ActiveSessionCapability;
use codex_app_server_protocol::ActiveSessionPeerKind;
use codex_app_server_protocol::LocalSession;
use codex_app_server_protocol::LocalSessionGitInfo;
use codex_app_server_protocol::LocalSessionPeer;
use codex_app_server_protocol::LocalSessionRedaction;
use codex_app_server_protocol::LocalSessionStatus;
use codex_app_server_protocol::MissionControlCapabilities;
use codex_app_server_protocol::MissionControlOverviewResponse;
use codex_app_server_protocol::MissionControlSession;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadActiveFlag;
use codex_app_server_protocol::ThreadGoal;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanAutoExecute;
use codex_app_server_protocol::ThreadGoalPlanNode;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_app_server_protocol::ThreadPendingInteraction;
use codex_app_server_protocol::ThreadPendingInteractionKind;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionSourceKind;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
use codex_app_server_protocol::ThreadSchedule;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;
use codex_app_server_protocol::ThreadSource;
use codex_app_server_protocol::ToolRequestUserInputParams;
use codex_app_server_protocol::ToolRequestUserInputQuestion;

#[tokio::test]
async fn mission_control_sessions_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());

    assert_chatwidget_snapshot!(
        "mission_control_sessions",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn mission_control_projects_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_projects",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn mission_control_questions_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_questions",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn mission_control_secret_question_is_disabled() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut response = test_overview_response();
    let thread_id = response.sessions[0].session.thread_id.clone();
    response.pending_interactions = vec![test_secret_pending_interaction(&thread_id)];

    chat.show_mission_control_overview(response);
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_secret_question_disabled",
        render_bottom_popup(&chat, /*width*/ 120)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn mission_control_unsupported_interaction_is_disabled() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut response = test_overview_response();
    let thread_id = response.sessions[0].session.thread_id.clone();
    response.pending_interactions = vec![test_unsupported_pending_interaction(&thread_id)];

    chat.show_mission_control_overview(response);
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_unsupported_interaction_disabled",
        render_bottom_popup(&chat, /*width*/ 120)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn mission_control_empty_states_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(empty_overview_response(test_capabilities()));

    assert_chatwidget_snapshot!(
        "mission_control_empty_sessions",
        render_bottom_popup(&chat, /*width*/ 100)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_empty_projects",
        render_bottom_popup(&chat, /*width*/ 100)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_empty_questions",
        render_bottom_popup(&chat, /*width*/ 100)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_empty_work_queue",
        render_bottom_popup(&chat, /*width*/ 100)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_empty_goal_chains",
        render_bottom_popup(&chat, /*width*/ 100)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_empty_schedules",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn mission_control_disabled_capabilities_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let response = empty_overview_response(MissionControlCapabilities {
        local_sessions: false,
        durable_mailbox: false,
        pending_interactions: false,
        goals: false,
        scheduled_tasks: false,
        remote_dispatch: false,
        workflow_mutation: false,
        shell_execution: false,
        filesystem_mutation: false,
    });

    chat.show_mission_control_overview(response);
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_disabled_capabilities_work_queue",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn mission_control_narrow_overflow_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(long_overflow_response());

    assert_chatwidget_snapshot!(
        "mission_control_narrow_overflow_sessions",
        render_bottom_popup(&chat, /*width*/ 60)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_narrow_overflow_questions",
        render_bottom_popup(&chat, /*width*/ 60)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "mission_control_narrow_overflow_goal_chains",
        render_bottom_popup(&chat, /*width*/ 60)
    );
}

#[tokio::test]
async fn mission_control_work_queue_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_work_queue",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn mission_control_goal_chains_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "mission_control_goal_chains",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn mission_control_schedules_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());
    for _ in 0..5 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    }

    assert_chatwidget_snapshot!(
        "mission_control_schedules",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn mission_control_session_row_selects_thread() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let response = test_overview_response();
    let selected_thread_id = response.sessions[0].session.thread_id.clone();

    chat.show_mission_control_overview(response);
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::SelectAgentThread(thread_id)) => {
            assert_eq!(thread_id.to_string(), selected_thread_id);
        }
        other => panic!("expected SelectAgentThread event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn mission_control_not_loaded_session_row_selects_thread() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = "44444444-4444-4444-8444-444444444444";
    let mut local_session = test_local_session(
        thread_id,
        "Archived operator session",
        "/tmp/codewith/archived",
        LocalSessionStatus::NotLoaded,
        None,
        None,
    );
    local_session.runtime_session_id = None;
    local_session.peer = None;
    let mut response = empty_overview_response(test_capabilities());
    response.sessions.push(MissionControlSession {
        session: local_session,
        goal: None,
        goal_plans: Vec::new(),
        schedules: Vec::new(),
    });

    chat.show_mission_control_overview(response);
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::SelectAgentThread(selected_thread_id)) => {
            assert_eq!(selected_thread_id.to_string(), thread_id);
        }
        other => panic!("expected SelectAgentThread event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn mission_control_goal_chain_plan_opens_detail() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let response = test_overview_response();
    let expected_thread_id = response.sessions[0].session.thread_id.clone();
    let expected_plan_id = response.sessions[0].goal_plans[0].plan_id.clone();

    chat.show_mission_control_overview(response);
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::OpenThreadGoalPlanDetail { thread_id, plan }) => {
            assert_eq!(thread_id.to_string(), expected_thread_id);
            assert_eq!(plan.plan_id, expected_plan_id);
        }
        other => panic!("expected OpenThreadGoalPlanDetail event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn mission_control_question_row_opens_answer_prompt() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_mission_control_overview(test_overview_response());
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::OpenMissionControlInteractionAnswer { interaction }) => {
            assert_eq!(interaction.interaction_id, "interaction-1");
            assert_eq!(interaction.kind, ThreadPendingInteractionKind::UserInput);
        }
        other => panic!("expected OpenMissionControlInteractionAnswer event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn mission_control_answer_prompt_submits_user_input_response() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = "11111111-1111-4111-8111-111111111111";

    chat.show_mission_control_answer_prompt(test_pending_interaction(thread_id));
    chat.handle_paste("open-codewith".to_string());
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    match rx.try_recv() {
        Ok(AppEvent::RespondMissionControlInteraction {
            interaction_id,
            thread_id: event_thread_id,
            terminal_status,
            response,
        }) => {
            assert_eq!(interaction_id, "interaction-1");
            assert_eq!(event_thread_id.as_deref(), Some(thread_id));
            assert_eq!(
                terminal_status,
                ThreadPendingInteractionTerminalStatus::Responded
            );
            match response {
                ThreadPendingInteractionResponsePayload::RequestUserInput { answers } => {
                    assert_eq!(
                        answers
                            .get("target_project")
                            .map(|answer| answer.answers.clone()),
                        Some(vec!["open-codewith".to_string()])
                    );
                }
                other => panic!("expected request-user-input response, got {other:?}"),
            }
        }
        other => panic!("expected RespondMissionControlInteraction event, got {other:?}"),
    }
}

#[tokio::test]
async fn mission_control_slash_command_opens_overview() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::MissionControl);

    match rx.try_recv() {
        Ok(AppEvent::OpenMissionControlOverview) => {}
        other => panic!("expected OpenMissionControlOverview event, got {other:?}"),
    }
}

fn test_overview_response() -> MissionControlOverviewResponse {
    let active_thread_id = "11111111-1111-4111-8111-111111111111";
    let idle_thread_id = "22222222-2222-4222-8222-222222222222";
    MissionControlOverviewResponse {
        sessions: vec![
            MissionControlSession {
                session: test_local_session(
                    active_thread_id,
                    "Codewith orchestration",
                    "/tmp/codewith/open-codewith",
                    LocalSessionStatus::Active,
                    Some("mission-control-ui"),
                    Some("main"),
                ),
                goal: Some(test_goal(active_thread_id, ThreadGoalStatus::UsageLimited)),
                goal_plans: vec![test_plan(active_thread_id, ThreadGoalPlanStatus::Active)],
                schedules: vec![test_schedule(
                    active_thread_id,
                    "Check release blockers",
                    ThreadScheduleStatus::Active,
                    ThreadScheduleSpec::Interval {
                        amount: 15,
                        unit: codex_app_server_protocol::ThreadScheduleIntervalUnit::Minutes,
                    },
                )],
            },
            MissionControlSession {
                session: test_local_session(
                    idle_thread_id,
                    "Workflow builder",
                    "/tmp/codewith/workflows",
                    LocalSessionStatus::Idle,
                    Some("workflow-builder"),
                    Some("workflow-chain"),
                ),
                goal: None,
                goal_plans: vec![test_plan(idle_thread_id, ThreadGoalPlanStatus::Blocked)],
                schedules: Vec::new(),
            },
        ],
        pending_interactions: vec![test_pending_interaction(active_thread_id)],
        next_session_cursor: None,
        next_pending_interaction_cursor: None,
        capabilities: test_capabilities(),
    }
}

fn empty_overview_response(
    capabilities: MissionControlCapabilities,
) -> MissionControlOverviewResponse {
    MissionControlOverviewResponse {
        sessions: Vec::new(),
        pending_interactions: Vec::new(),
        next_session_cursor: None,
        next_pending_interaction_cursor: None,
        capabilities,
    }
}

fn long_overflow_response() -> MissionControlOverviewResponse {
    let thread_id = "33333333-3333-4333-8333-333333333333";
    MissionControlOverviewResponse {
        sessions: vec![MissionControlSession {
            session: test_local_session(
                thread_id,
                "Mission Control With A Very Long Operator Session Name That Should Wrap",
                "/tmp/codewith/super-long-project-name-without-natural-breaks-abcdefghijklmnopqrstuvwxyz",
                LocalSessionStatus::Active,
                Some("mission-control-model-with-a-long-name"),
                Some("feature/very-long-branch-name-for-responsive-mission-control-tests"),
            ),
            goal: Some(test_goal(thread_id, ThreadGoalStatus::Active)),
            goal_plans: vec![test_long_plan(thread_id)],
            schedules: vec![test_schedule(
                thread_id,
                "Review the long responsive Mission Control schedule prompt before it wraps",
                ThreadScheduleStatus::Paused,
                ThreadScheduleSpec::Once,
            )],
        }],
        pending_interactions: vec![test_pending_interaction_with_question(
            thread_id,
            ToolRequestUserInputQuestion {
                id: "long_answer".to_string(),
                header: "Long".to_string(),
                question: "Explain the deployment ordering for ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_string(),
                is_other: false,
                is_secret: false,
                options: None,
            },
            "Explain the deployment ordering for ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        )],
        next_session_cursor: Some("next-sessions".to_string()),
        next_pending_interaction_cursor: Some("next-questions".to_string()),
        capabilities: test_capabilities(),
    }
}

fn test_capabilities() -> MissionControlCapabilities {
    MissionControlCapabilities {
        local_sessions: true,
        durable_mailbox: true,
        pending_interactions: true,
        goals: true,
        scheduled_tasks: true,
        remote_dispatch: false,
        workflow_mutation: false,
        shell_execution: false,
        filesystem_mutation: false,
    }
}

fn test_local_session(
    thread_id: &str,
    display_name: &str,
    cwd: &str,
    status: LocalSessionStatus,
    model: Option<&str>,
    branch: Option<&str>,
) -> LocalSession {
    LocalSession {
        thread_id: thread_id.to_string(),
        runtime_session_id: Some(format!("runtime-{display_name}")),
        peer: Some(LocalSessionPeer {
            peer_id: format!("peer-{display_name}"),
            kind: ActiveSessionPeerKind::CodewithSession,
            capabilities: vec![
                ActiveSessionCapability::ReceiveMessage,
                ActiveSessionCapability::QueueMessage,
            ],
            last_seen_at: 1_800,
        }),
        status,
        active_flags: if status == LocalSessionStatus::Active {
            vec![ThreadActiveFlag::WaitingOnUserInput]
        } else {
            Vec::new()
        },
        cwd: AbsolutePathBuf::from_absolute_path(PathBuf::from(cwd)).expect("absolute cwd"),
        display_name: Some(display_name.to_string()),
        agent_path: None,
        model_provider: "openai".to_string(),
        model: model.map(ToString::to_string),
        source: SessionSource::Cli,
        thread_source: Some(ThreadSource::User),
        created_at: 1_700,
        updated_at: 1_800,
        path: None,
        git_info: Some(LocalSessionGitInfo {
            sha: Some("abcdef1234567890".to_string()),
            branch: branch.map(ToString::to_string),
        }),
        redactions: Vec::<LocalSessionRedaction>::new(),
    }
}

fn test_goal(thread_id: &str, status: ThreadGoalStatus) -> ThreadGoal {
    ThreadGoal {
        thread_id: thread_id.to_string(),
        goal_id: format!("goal-{thread_id}"),
        objective: "Build the operator overview without touching workflow internals".to_string(),
        status,
        token_budget: Some(80_000),
        tokens_used: 12_345,
        time_used_seconds: 600,
        created_at: 1_700,
        updated_at: 1_800,
    }
}

fn test_plan(thread_id: &str, status: ThreadGoalPlanStatus) -> ThreadGoalPlan {
    let nodes = vec![
        test_plan_node(
            thread_id,
            "overview",
            ThreadGoalPlanNodeStatus::Active,
            /*ready*/ false,
        ),
        test_plan_node(
            thread_id,
            "questions",
            ThreadGoalPlanNodeStatus::Pending,
            /*ready*/ true,
        ),
    ];
    ThreadGoalPlan {
        plan_id: format!("plan-{thread_id}"),
        thread_id: thread_id.to_string(),
        status,
        auto_execute: ThreadGoalPlanAutoExecute::AiDirected,
        max_tokens: Some(150_000),
        total_tokens_used: 20_000,
        total_time_used_seconds: 1_200,
        remaining_tokens: Some(130_000),
        node_count: 2,
        completed_node_count: 0,
        ready_node_count: 1,
        active_node_count: 1,
        pending_node_count: 1,
        paused_node_count: 0,
        blocked_node_count: if status == ThreadGoalPlanStatus::Blocked {
            1
        } else {
            0
        },
        usage_limited_node_count: 0,
        budget_limited_node_count: 0,
        cancelled_node_count: 0,
        created_at: 1_700,
        updated_at: 1_800,
        nodes,
    }
}

fn test_long_plan(thread_id: &str) -> ThreadGoalPlan {
    let nodes = vec![
        ThreadGoalPlanNode {
            objective: "Investigate an intentionally long responsive mission-control objective with ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_string(),
            depends_on: vec!["bootstrap".to_string(), "collect-all-the-inputs".to_string()],
            ..test_plan_node(
                thread_id,
                "responsive-overflow-validation-with-a-long-key",
                ThreadGoalPlanNodeStatus::Active,
                /*ready*/ false,
            )
        },
        ThreadGoalPlanNode {
            objective: "Execute the next ready node after narrow-width rendering remains readable".to_string(),
            ..test_plan_node(
                thread_id,
                "ready-node-after-overflow",
                ThreadGoalPlanNodeStatus::Pending,
                /*ready*/ true,
            )
        },
    ];
    ThreadGoalPlan {
        plan_id: format!("plan-{thread_id}-responsive-overflow-validation"),
        thread_id: thread_id.to_string(),
        status: ThreadGoalPlanStatus::Active,
        auto_execute: ThreadGoalPlanAutoExecute::ReadyOnly,
        max_tokens: Some(250_000),
        total_tokens_used: 123_456,
        total_time_used_seconds: 4_200,
        remaining_tokens: Some(126_544),
        node_count: 2,
        completed_node_count: 0,
        ready_node_count: 1,
        active_node_count: 1,
        pending_node_count: 1,
        paused_node_count: 0,
        blocked_node_count: 0,
        usage_limited_node_count: 0,
        budget_limited_node_count: 0,
        cancelled_node_count: 0,
        created_at: 1_700,
        updated_at: 1_800,
        nodes,
    }
}

fn test_plan_node(
    thread_id: &str,
    key: &str,
    status: ThreadGoalPlanNodeStatus,
    ready: bool,
) -> ThreadGoalPlanNode {
    ThreadGoalPlanNode {
        node_id: format!("node-{key}"),
        plan_id: format!("plan-{thread_id}"),
        thread_id: thread_id.to_string(),
        assigned_thread_id: thread_id.to_string(),
        key: key.to_string(),
        sequence: 1,
        priority: 0,
        objective: format!("Execute {key}"),
        status,
        ready,
        token_budget: Some(50_000),
        tokens_used: 1_000,
        time_used_seconds: 60,
        projected_goal_id: None,
        depends_on: Vec::new(),
        created_at: 1_700,
        updated_at: 1_800,
    }
}

fn test_schedule(
    thread_id: &str,
    prompt: &str,
    status: ThreadScheduleStatus,
    schedule: ThreadScheduleSpec,
) -> ThreadSchedule {
    ThreadSchedule {
        thread_id: thread_id.to_string(),
        schedule_id: format!("schedule-{thread_id}"),
        prompt: prompt.to_string(),
        prompt_source: ThreadSchedulePromptSource::Inline,
        schedule,
        timezone: "UTC".to_string(),
        status,
        next_run_at: Some(1_900),
        last_run_at: None,
        expires_at: None,
        failure_count: 0,
        lease_expires_at: None,
        created_at: 1_700,
        updated_at: 1_800,
    }
}

fn test_pending_interaction(thread_id: &str) -> ThreadPendingInteraction {
    test_pending_interaction_with_question(
        thread_id,
        ToolRequestUserInputQuestion {
            id: "target_project".to_string(),
            header: "Target".to_string(),
            question: "Which project should receive this task?".to_string(),
            is_other: false,
            is_secret: false,
            options: None,
        },
        "Which project should receive this task?",
    )
}

fn test_secret_pending_interaction(thread_id: &str) -> ThreadPendingInteraction {
    test_pending_interaction_with_question(
        thread_id,
        ToolRequestUserInputQuestion {
            id: "deploy_token".to_string(),
            header: "Secret".to_string(),
            question: "Enter the deployment token.".to_string(),
            is_other: false,
            is_secret: true,
            options: None,
        },
        "Enter the deployment token.",
    )
}

fn test_unsupported_pending_interaction(thread_id: &str) -> ThreadPendingInteraction {
    ThreadPendingInteraction {
        interaction_id: "interaction-approval".to_string(),
        thread_id: thread_id.to_string(),
        source_kind: ThreadPendingInteractionSourceKind::Thread,
        source_id: None,
        turn_id: Some("turn-approval".to_string()),
        worker_request_id: None,
        kind: ThreadPendingInteractionKind::CommandApproval,
        status: ThreadPendingInteractionStatus::Pending,
        request_payload: serde_json::json!({
            "command": "rm -rf /tmp/not-real",
            "cwd": "/tmp/codewith/open-codewith"
        }),
        request_payload_sha256: "sha-approval-request".to_string(),
        request_payload_preview: "Approve command rm -rf /tmp/not-real?".to_string(),
        request_redactions: Vec::new(),
        response_payload: None,
        response_payload_sha256: None,
        response_payload_preview: None,
        response_redactions: Vec::new(),
        no_client_policy: "queue".to_string(),
        timeout_at: None,
        created_at: 1_700,
        delivered_at: Some(1_710),
        responded_at: None,
        terminal_at: None,
        updated_at: 1_710,
    }
}

fn test_pending_interaction_with_question(
    thread_id: &str,
    question: ToolRequestUserInputQuestion,
    preview: &str,
) -> ThreadPendingInteraction {
    let request_payload = serde_json::to_value(ToolRequestUserInputParams {
        thread_id: thread_id.to_string(),
        turn_id: "turn-1".to_string(),
        item_id: "item-1".to_string(),
        questions: vec![question],
    })
    .expect("request payload");
    ThreadPendingInteraction {
        interaction_id: "interaction-1".to_string(),
        thread_id: thread_id.to_string(),
        source_kind: ThreadPendingInteractionSourceKind::Thread,
        source_id: None,
        turn_id: Some("turn-1".to_string()),
        worker_request_id: None,
        kind: ThreadPendingInteractionKind::UserInput,
        status: ThreadPendingInteractionStatus::Pending,
        request_payload,
        request_payload_sha256: "sha-request".to_string(),
        request_payload_preview: preview.to_string(),
        request_redactions: Vec::new(),
        response_payload: None,
        response_payload_sha256: None,
        response_payload_preview: None,
        response_redactions: Vec::new(),
        no_client_policy: "queue".to_string(),
        timeout_at: None,
        created_at: 1_700,
        delivered_at: Some(1_710),
        responded_at: None,
        terminal_at: None,
        updated_at: 1_710,
    }
}
