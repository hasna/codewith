use super::*;
use codex_app_server_protocol::ThreadWorkflow;
use codex_app_server_protocol::ThreadWorkflowListResponse;
use codex_app_server_protocol::ThreadWorkflowRun;
use codex_app_server_protocol::ThreadWorkflowRunListResponse;
use codex_app_server_protocol::ThreadWorkflowRunStatus;
use codex_app_server_protocol::ThreadWorkflowStatus;

fn test_workflow(id: &str, status: ThreadWorkflowStatus) -> ThreadWorkflow {
    ThreadWorkflow {
        thread_id: "thread-1".to_string(),
        workflow_record_id: id.to_string(),
        spec_workflow_id: format!("spec_{id}"),
        schema_version: "workflow.codex.codewith/v0".to_string(),
        display_name: format!("Workflow {id}"),
        status,
        source_yaml_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
        agent_count: 3,
        step_count: 7,
        parallel_group_count: 2,
        verifier_count: 4,
        run_command_verifier_count: 1,
        model_routed_step_count: 6,
        created_at: 1_800_000_000,
        updated_at: 1_800_000_123,
    }
}

fn test_run(id: &str, status: ThreadWorkflowRunStatus) -> ThreadWorkflowRun {
    ThreadWorkflowRun {
        thread_id: Some("thread-1".to_string()),
        run_id: id.to_string(),
        workflow_record_id: "workflow-alpha".to_string(),
        spec_workflow_id: "spec_workflow-alpha".to_string(),
        schema_version: "workflow.codex.codewith/v0".to_string(),
        source_yaml_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
        status,
        status_reason: None,
        reason_code: None,
        generation: 1,
        pending_step_count: 1,
        ready_step_count: 1,
        active_step_count: if matches!(
            status,
            ThreadWorkflowRunStatus::Running | ThreadWorkflowRunStatus::Waiting
        ) {
            2
        } else {
            0
        },
        waiting_verifier_step_count: if status == ThreadWorkflowRunStatus::Waiting {
            1
        } else {
            0
        },
        blocked_step_count: 0,
        failed_step_count: if status == ThreadWorkflowRunStatus::Failed {
            1
        } else {
            0
        },
        succeeded_step_count: if status == ThreadWorkflowRunStatus::Completed {
            7
        } else {
            3
        },
        skipped_step_count: 0,
        verifier_count: 4,
        event_count: 9,
        created_at: 1_800_000_000,
        updated_at: 1_800_000_456,
        started_at: Some(1_800_000_010),
        completed_at: (status == ThreadWorkflowRunStatus::Completed).then_some(1_800_000_456),
    }
}

#[tokio::test]
async fn workflow_manager_empty_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_thread_workflow_manager(
        thread_id,
        ThreadWorkflowListResponse {
            data: Vec::new(),
            next_cursor: None,
        },
        ThreadWorkflowRunListResponse {
            data: Vec::new(),
            next_cursor: None,
        },
    );

    assert_chatwidget_snapshot!(
        "workflow_manager_empty",
        render_bottom_popup(&chat, /*width*/ 110)
    );
}

#[tokio::test]
async fn workflow_manager_loading_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_thread_workflow_manager_loading(ThreadId::new());

    assert_chatwidget_snapshot!(
        "workflow_manager_loading",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn workflow_manager_error_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let err = color_eyre::eyre::eyre!("ephemeral thread does not support workflows: thread-1");

    chat.show_thread_workflow_manager_error(ThreadId::new(), "read workflow specs", &err);

    assert_chatwidget_snapshot!(
        "workflow_manager_error",
        render_bottom_popup(&chat, /*width*/ 105)
    );
}

#[tokio::test]
async fn workflow_manager_active_runs_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_thread_workflow_manager(
        thread_id,
        ThreadWorkflowListResponse {
            data: vec![test_workflow("workflow-alpha", ThreadWorkflowStatus::Draft)],
            next_cursor: None,
        },
        ThreadWorkflowRunListResponse {
            data: vec![test_run(
                "run-active-123456789",
                ThreadWorkflowRunStatus::Waiting,
            )],
            next_cursor: None,
        },
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    assert_chatwidget_snapshot!(
        "workflow_manager_active_runs",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn workflow_manager_completed_run_actions_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_thread_workflow_run_actions(
        ThreadId::new(),
        test_run(
            "run-completed-123456789",
            ThreadWorkflowRunStatus::Completed,
        ),
    );

    assert_chatwidget_snapshot!(
        "workflow_manager_completed_run_actions",
        render_bottom_popup(&chat, /*width*/ 110)
    );
}

#[tokio::test]
async fn workflow_manager_row_opens_actions() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.show_thread_workflow_manager(
        thread_id,
        ThreadWorkflowListResponse {
            data: vec![test_workflow("workflow-alpha", ThreadWorkflowStatus::Draft)],
            next_cursor: None,
        },
        ThreadWorkflowRunListResponse {
            data: Vec::new(),
            next_cursor: None,
        },
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let event = rx.try_recv().expect("expected workflow action event");
    let AppEvent::OpenThreadWorkflowActions {
        thread_id: actual_thread_id,
        workflow,
    } = event
    else {
        panic!("expected OpenThreadWorkflowActions, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(workflow.workflow_record_id, "workflow-alpha");
}

#[tokio::test]
async fn workflow_manager_draft_prompt_is_blocked_while_task_runs() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.show_thread_workflow_draft_prompt();

    assert!(!chat.bottom_pane.has_active_view());
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, AppEvent::PrefillComposer { .. }),
            "blocked draft prompt should not prefill the composer"
        );
    }
}
