use super::*;
use codex_app_server_protocol::ThreadGoalListResponse;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanAutoExecute;
use codex_app_server_protocol::ThreadGoalPlanNode;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;

#[tokio::test]
async fn goal_menu_active_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_summary(test_goal(
        thread_id,
        AppThreadGoalStatus::Active,
        /*token_budget*/ Some(80_000),
    ));

    assert_chatwidget_snapshot!("goal_menu_active", rendered_goal_summary(&mut rx));
}

#[tokio::test]
async fn goal_menu_paused_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_summary(test_goal(
        thread_id,
        AppThreadGoalStatus::Paused,
        /*token_budget*/ None,
    ));

    assert_chatwidget_snapshot!("goal_menu_paused", rendered_goal_summary(&mut rx));
}

#[tokio::test]
async fn goal_menu_blocked_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_summary(test_goal(
        thread_id,
        AppThreadGoalStatus::Blocked,
        /*token_budget*/ None,
    ));

    assert_chatwidget_snapshot!("goal_menu_blocked", rendered_goal_summary(&mut rx));
}

#[tokio::test]
async fn goal_menu_usage_limited_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_summary(test_goal(
        thread_id,
        AppThreadGoalStatus::UsageLimited,
        /*token_budget*/ None,
    ));

    assert_chatwidget_snapshot!("goal_menu_usage_limited", rendered_goal_summary(&mut rx));
}

#[tokio::test]
async fn goal_menu_budget_limited_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_summary(test_goal(
        thread_id,
        AppThreadGoalStatus::BudgetLimited,
        /*token_budget*/ Some(80_000),
    ));

    assert_chatwidget_snapshot!("goal_menu_budget_limited", rendered_goal_summary(&mut rx));
}

#[tokio::test]
async fn resume_paused_goal_prompt_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_resume_paused_goal_prompt(
        thread_id,
        "Keep improving the bare goal command until it feels calm and useful.".to_string(),
    );

    assert_chatwidget_snapshot!(
        "resume_paused_goal_prompt",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn goal_edit_prompt_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_edit_prompt(
        thread_id,
        test_goal(
            thread_id,
            AppThreadGoalStatus::Active,
            /*token_budget*/ Some(80_000),
        ),
    );

    assert_chatwidget_snapshot!(
        "goal_edit_prompt",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn goal_manager_with_plan_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    let thread_id_string = thread_id.to_string();

    chat.show_goal_manager(
        thread_id,
        ThreadGoalListResponse {
            goal: Some(test_goal(
                thread_id,
                AppThreadGoalStatus::Active,
                /*token_budget*/ Some(80_000),
            )),
            next_cursor: None,
            goal_plans: vec![test_plan(
                &thread_id_string,
                ThreadGoalPlanStatus::Active,
                ThreadGoalPlanAutoExecute::AiDirected,
                vec![
                    test_plan_node(
                        "discover",
                        "Map existing goal persistence and UI surfaces",
                        ThreadGoalPlanNodeStatus::Active,
                        /*depends_on*/ Vec::new(),
                        &thread_id_string,
                    ),
                    test_plan_node(
                        "ship",
                        "Implement durable dependent goal execution",
                        ThreadGoalPlanNodeStatus::Pending,
                        vec!["discover".to_string()],
                        &thread_id_string,
                    ),
                ],
            )],
        },
    );

    assert_chatwidget_snapshot!(
        "goal_manager_with_plan",
        render_bottom_popup(&chat, /*width*/ 110)
    );
}

#[tokio::test]
async fn goal_manager_cancel_current_goal_row_cancels_goal() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_manager(
        thread_id,
        ThreadGoalListResponse {
            goal: Some(test_goal(
                thread_id,
                AppThreadGoalStatus::Active,
                /*token_budget*/ Some(80_000),
            )),
            next_cursor: None,
            goal_plans: Vec::new(),
        },
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::SetThreadGoalStatus {
            thread_id: event_thread_id,
            status,
        }) => {
            assert_eq!(event_thread_id, thread_id);
            assert_eq!(status, AppThreadGoalStatus::Cancelled);
        }
        other => panic!("expected SetThreadGoalStatus event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn goal_plan_detail_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    let thread_id_string = thread_id.to_string();

    chat.show_goal_plan_detail(
        thread_id,
        test_plan(
            &thread_id_string,
            ThreadGoalPlanStatus::Active,
            ThreadGoalPlanAutoExecute::AiDirected,
            vec![
                test_plan_node(
                    "discover",
                    "Map existing goal persistence and UI surfaces",
                    ThreadGoalPlanNodeStatus::Active,
                    /*depends_on*/ Vec::new(),
                    &thread_id_string,
                ),
                test_plan_node(
                    "ship",
                    "Implement durable dependent goal execution",
                    ThreadGoalPlanNodeStatus::Pending,
                    vec!["discover".to_string()],
                    &thread_id_string,
                ),
            ],
        ),
    );

    assert_chatwidget_snapshot!(
        "goal_plan_detail",
        render_bottom_popup(&chat, /*width*/ 110)
    );
}

#[tokio::test]
async fn goal_manager_plan_row_opens_plan_detail() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    let thread_id_string = thread_id.to_string();
    let plan = test_plan(
        &thread_id_string,
        ThreadGoalPlanStatus::Active,
        ThreadGoalPlanAutoExecute::ReadyOnly,
        vec![test_plan_node(
            "implement",
            "Implement durable dependent goal execution",
            ThreadGoalPlanNodeStatus::Pending,
            Vec::new(),
            &thread_id_string,
        )],
    );

    chat.show_goal_manager(
        thread_id,
        ThreadGoalListResponse {
            goal: None,
            next_cursor: None,
            goal_plans: vec![plan.clone()],
        },
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::OpenThreadGoalPlanDetail {
            thread_id: event_thread_id,
            plan: event_plan,
        }) => {
            assert_eq!(event_thread_id, thread_id);
            assert_eq!(event_plan, plan);
        }
        other => panic!("expected OpenThreadGoalPlanDetail event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn goal_plan_detail_ready_node_activates_selected_goal() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    let thread_id_string = thread_id.to_string();

    chat.show_goal_plan_detail(
        thread_id,
        test_plan(
            &thread_id_string,
            ThreadGoalPlanStatus::Active,
            ThreadGoalPlanAutoExecute::ReadyOnly,
            vec![test_plan_node(
                "implement",
                "Implement durable dependent goal execution",
                ThreadGoalPlanNodeStatus::Pending,
                Vec::new(),
                &thread_id_string,
            )],
        ),
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::ActivateThreadGoalPlanNode {
            thread_id: event_thread_id,
            node_id,
        }) => {
            assert_eq!(event_thread_id, thread_id);
            assert_eq!(node_id, "node_implement");
        }
        other => panic!("expected ActivateThreadGoalPlanNode event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn goal_plan_detail_ready_node_assigned_elsewhere_has_no_activation_action() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    let thread_id_string = thread_id.to_string();
    let delegate_thread_id_string = ThreadId::new().to_string();

    chat.show_goal_plan_detail(
        thread_id,
        test_plan(
            &thread_id_string,
            ThreadGoalPlanStatus::Active,
            ThreadGoalPlanAutoExecute::ReadyOnly,
            vec![test_plan_node(
                "implement",
                "Implement durable delegated goal execution",
                ThreadGoalPlanNodeStatus::Pending,
                Vec::new(),
                &delegate_thread_id_string,
            )],
        ),
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert!(rx.try_recv().is_err());
    assert!(!chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn goal_edit_prompt_submits_preserved_status_and_budget() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_goal_edit_prompt(
        thread_id,
        test_goal(
            thread_id,
            AppThreadGoalStatus::Paused,
            /*token_budget*/ Some(80_000),
        ),
    );
    chat.handle_paste(" with clearer wording".to_string());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::SetThreadGoalObjective {
            thread_id: event_thread_id,
            objective,
            mode:
                crate::app_event::ThreadGoalSetMode::UpdateExisting {
                    status,
                    token_budget,
                },
        }) => {
            assert_eq!(event_thread_id, thread_id);
            assert_eq!(
                objective,
                "Keep improving the bare goal command until it feels calm and useful. with clearer wording"
            );
            assert_eq!(status, AppThreadGoalStatus::Paused);
            assert_eq!(token_budget, Some(80_000));
        }
        other => panic!("expected SetThreadGoalObjective event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn goal_edit_prompt_preserves_resumable_stopped_statuses() {
    for stopped_status in [
        AppThreadGoalStatus::Blocked,
        AppThreadGoalStatus::UsageLimited,
    ] {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        let thread_id = ThreadId::new();

        chat.show_goal_edit_prompt(
            thread_id,
            test_goal(
                thread_id,
                stopped_status,
                /*token_budget*/ Some(80_000),
            ),
        );
        chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

        match rx.try_recv() {
            Ok(AppEvent::SetThreadGoalObjective {
                mode:
                    crate::app_event::ThreadGoalSetMode::UpdateExisting {
                        status,
                        token_budget,
                    },
                ..
            }) => {
                assert_eq!(status, stopped_status);
                assert_eq!(token_budget, Some(80_000));
            }
            other => panic!("expected SetThreadGoalObjective event, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn goal_edit_prompt_resets_terminal_status_to_active() {
    let cases = [
        AppThreadGoalStatus::BudgetLimited,
        AppThreadGoalStatus::Complete,
        AppThreadGoalStatus::Cancelled,
    ];

    for terminal_status in cases {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        let thread_id = ThreadId::new();

        chat.show_goal_edit_prompt(
            thread_id,
            test_goal(
                thread_id,
                terminal_status,
                /*token_budget*/ Some(80_000),
            ),
        );
        chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

        match rx.try_recv() {
            Ok(AppEvent::SetThreadGoalObjective {
                mode:
                    crate::app_event::ThreadGoalSetMode::UpdateExisting {
                        status,
                        token_budget,
                    },
                ..
            }) => {
                assert_eq!(status, AppThreadGoalStatus::Active);
                assert_eq!(token_budget, Some(80_000));
            }
            other => panic!("expected SetThreadGoalObjective event, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn resume_paused_goal_prompt_default_resumes_goal() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_resume_paused_goal_prompt(thread_id, "Finish the paused goal.".to_string());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::SetThreadGoalStatus {
            thread_id: event_thread_id,
            status,
        }) => {
            assert_eq!(event_thread_id, thread_id);
            assert_eq!(status, AppThreadGoalStatus::Active);
        }
        other => panic!("expected SetThreadGoalStatus event, got {other:?}"),
    }
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn resume_paused_goal_prompt_can_leave_goal_paused() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_resume_paused_goal_prompt(thread_id, "Finish the paused goal.".to_string());
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert!(chat.no_modal_or_popup_active());
}

fn test_goal(
    thread_id: ThreadId,
    status: AppThreadGoalStatus,
    token_budget: Option<i64>,
) -> AppThreadGoal {
    AppThreadGoal {
        thread_id: thread_id.to_string(),
        goal_id: "goal-1".to_string(),
        objective: "Keep improving the bare goal command until it feels calm and useful."
            .to_string(),
        title: None,
        status,
        token_budget,
        tokens_used: 12_500,
        time_used_seconds: 90,
        created_at: 1_776_272_400,
        updated_at: 1_776_272_460,
    }
}

fn test_plan(
    thread_id: &str,
    status: ThreadGoalPlanStatus,
    auto_execute: ThreadGoalPlanAutoExecute,
    nodes: Vec<ThreadGoalPlanNode>,
) -> ThreadGoalPlan {
    let completed_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::Complete)
        .count() as i64;
    let active_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::Active)
        .count() as i64;
    let pending_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::Pending)
        .count() as i64;
    let paused_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::Paused)
        .count() as i64;
    let blocked_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::Blocked)
        .count() as i64;
    let usage_limited_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::UsageLimited)
        .count() as i64;
    let budget_limited_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::BudgetLimited)
        .count() as i64;
    let cancelled_node_count = nodes
        .iter()
        .filter(|node| node.status == ThreadGoalPlanNodeStatus::Cancelled)
        .count() as i64;
    let ready_node_count = nodes.iter().filter(|node| node.ready).count() as i64;
    let total_tokens_used = nodes.iter().map(|node| node.tokens_used).sum();
    let total_time_used_seconds = nodes.iter().map(|node| node.time_used_seconds).sum();
    ThreadGoalPlan {
        plan_id: "plan_goal_rollout".to_string(),
        thread_id: thread_id.to_string(),
        status,
        auto_execute,
        max_tokens: None,
        total_tokens_used,
        total_time_used_seconds,
        remaining_tokens: None,
        node_count: nodes.len() as i64,
        completed_node_count,
        ready_node_count,
        active_node_count,
        pending_node_count,
        paused_node_count,
        blocked_node_count,
        usage_limited_node_count,
        budget_limited_node_count,
        cancelled_node_count,
        created_at: 1_776_272_300,
        updated_at: 1_776_272_460,
        nodes,
    }
}

fn test_plan_node(
    key: &str,
    objective: &str,
    status: ThreadGoalPlanNodeStatus,
    depends_on: Vec<String>,
    thread_id: &str,
) -> ThreadGoalPlanNode {
    ThreadGoalPlanNode {
        node_id: format!("node_{key}"),
        plan_id: "plan_goal_rollout".to_string(),
        thread_id: thread_id.to_string(),
        assigned_thread_id: thread_id.to_string(),
        key: key.to_string(),
        sequence: 0,
        priority: 0,
        objective: objective.to_string(),
        title: None,
        status,
        ready: status == ThreadGoalPlanNodeStatus::Pending && depends_on.is_empty(),
        token_budget: None,
        tokens_used: 12_500,
        time_used_seconds: 90,
        projected_goal_id: Some(format!("projected_{key}")),
        depends_on,
        created_at: 1_776_272_300,
        updated_at: 1_776_272_460,
    }
}

fn rendered_goal_summary(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::app_event::AppEvent>,
) -> String {
    drain_insert_history(rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n")
}
