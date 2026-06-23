use super::*;
use crate::app_event::McpInventoryTarget;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::tmux_handoff::TmuxHandoffDestination;
use chrono::Local;
use chrono::LocalResult;
use chrono::NaiveDate;
use chrono::TimeZone;
use codex_app_server_protocol::AgentDesiredState;
use codex_app_server_protocol::AgentRetentionState;
use codex_app_server_protocol::AgentRun;
use codex_app_server_protocol::AgentRunStatus;
use codex_app_server_protocol::ThreadExternalAgentMode;
use codex_app_server_protocol::ThreadScheduleIntervalUnit;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleSpec;
use pretty_assertions::assert_eq;
use serial_test::serial;

fn force_pet_image_support(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Supported(
        crate::pets::ImageProtocol::Kitty,
    ));
}

fn force_tmux_pet_image_unsupported(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Unsupported(
        crate::pets::PetImageUnsupportedReason::Tmux,
    ));
}

fn force_terminal_pet_image_unsupported(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Unsupported(
        crate::pets::PetImageUnsupportedReason::Terminal,
    ));
}

fn force_old_iterm2_pet_image_unsupported(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Unsupported(
        crate::pets::PetImageUnsupportedReason::Iterm2TooOld,
    ));
}

fn fast_tier_command() -> ServiceTierCommand {
    ServiceTierCommand {
        id: ServiceTier::Fast.request_value().to_string(),
        name: "fast".to_string(),
        description: "Fastest inference with increased plan usage".to_string(),
    }
}

fn complete_turn_with_message(chat: &mut ChatWidget, turn_id: &str, message: Option<&str>) {
    if let Some(message) = message {
        complete_assistant_message(
            chat,
            &format!("{turn_id}-message"),
            message,
            Some(MessagePhase::FinalAnswer),
        );
    }
    handle_turn_completed(chat, turn_id, /*duration_ms*/ None);
}

fn submit_composer_text(chat: &mut ChatWidget, text: &str) {
    chat.bottom_pane
        .set_composer_text(text.to_string(), Vec::new(), Vec::new());
    submit_current_composer(chat);
}

fn test_background_agent(agent_id: &str, status: AgentRunStatus, updated_at: i64) -> AgentRun {
    AgentRun {
        agent_id: agent_id.to_string(),
        idempotency_key: None,
        request_id: None,
        source: "test".to_string(),
        prompt_snapshot_ref: "inline:test:prompt".to_string(),
        input_snapshot_ref: None,
        thread_id: None,
        thread_store_kind: "background-agent".to_string(),
        thread_store_id: None,
        rollout_path: None,
        parent_thread_id: None,
        parent_agent_run_id: None,
        spawn_linkage: None,
        worktree_lease_id: None,
        auth_profile_ref: None,
        desired_state: AgentDesiredState::Running,
        status,
        status_reason: None,
        config_fingerprint: None,
        version_fingerprint: None,
        retention_state: AgentRetentionState::Active,
        archive_after: None,
        delete_after: None,
        archived_at: None,
        deleted_at: None,
        supervisor_id: None,
        generation: 0,
        pid: None,
        pgid: None,
        job_id: None,
        heartbeat_at: None,
        crash_reason: None,
        exit_code: None,
        exit_signal: None,
        last_event_seq: 0,
        last_snapshot_seq: 0,
        created_at: 1,
        updated_at,
        started_at: None,
        completed_at: None,
    }
}

fn submit_current_composer(chat: &mut ChatWidget) {
    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn queue_composer_text_with_tab(chat: &mut ChatWidget, text: &str) {
    chat.bottom_pane
        .set_composer_text(text.to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
}

fn recall_latest_after_clearing(chat: &mut ChatWidget) -> String {
    chat.bottom_pane
        .set_composer_text(String::new(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    chat.bottom_pane.composer_text()
}

#[tokio::test]
async fn external_agent_slash_with_task_requests_child_thread() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::ExternalAgent,
        "grok-build inspect the diff".to_string(),
        Vec::new(),
    );

    match rx.try_recv() {
        Ok(AppEvent::StartExternalAgentChildThread {
            runtime_id,
            runtime_display_name,
            task,
            mode,
        }) => {
            assert_eq!(runtime_id, "grok-build");
            assert_eq!(runtime_display_name, "Grok Build");
            assert_eq!(task, "inspect the diff");
            assert_eq!(mode, ThreadExternalAgentMode::Plan);
        }
        other => panic!("expected external-agent child-thread event, got {other:?}"),
    }
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn external_agent_slash_with_claude_task_requests_child_thread() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::ExternalAgent,
        "claude inspect the diff".to_string(),
        Vec::new(),
    );

    match rx.try_recv() {
        Ok(AppEvent::StartExternalAgentChildThread {
            runtime_id,
            runtime_display_name,
            task,
            mode,
        }) => {
            assert_eq!(runtime_id, "claude");
            assert_eq!(runtime_display_name, "Claude Code");
            assert_eq!(task, "inspect the diff");
            assert_eq!(mode, ThreadExternalAgentMode::Plan);
        }
        other => panic!("expected external-agent child-thread event, got {other:?}"),
    }
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn external_agent_slash_inline_task_submits_current_thread_op() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::ExternalAgent,
        "inline propose claude inspect the diff".to_string(),
        Vec::new(),
    );

    match op_rx.try_recv() {
        Ok(Op::StartExternalAgent {
            runtime_id,
            task,
            mode,
        }) => {
            assert_eq!(runtime_id, "claude");
            assert_eq!(task, "inspect the diff");
            assert_eq!(mode, ThreadExternalAgentMode::Propose);
        }
        other => panic!("expected inline external-agent op, got {other:?}"),
    }
    assert_matches!(rx.try_recv(), Ok(AppEvent::InsertHistoryCell(_)));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn external_agent_slash_rejects_grok_alias_without_submitting_op() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::ExternalAgent,
        "grok inspect the diff".to_string(),
        Vec::new(),
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn external_agent_slash_rejects_adversarial_alias_without_submitting_op() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::ExternalAgent,
        "adversarial claude cursor inspect the diff".to_string(),
        Vec::new(),
    );

    let event = rx.try_recv().expect("expected external-agent usage error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains(
                    "Usage: /external-agent [inline|--inline] [plan|propose] [cursor|grok-build|claude] [task]"
                ),
                "expected external-agent usage error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

fn next_add_to_history_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> String {
    loop {
        match rx.try_recv() {
            Ok(AppEvent::AppendMessageHistoryEntry { text, .. }) => return text,
            Ok(_) => continue,
            Err(TryRecvError::Empty) => {
                panic!("expected AppendMessageHistoryEntry event but queue was empty")
            }
            Err(TryRecvError::Disconnected) => {
                panic!("expected AppendMessageHistoryEntry event but channel closed")
            }
        }
    }
}

fn next_session_recap_request_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> (ThreadId, Option<String>, bool) {
    loop {
        match rx.try_recv() {
            Ok(AppEvent::RequestSessionRecap {
                thread_id,
                prompt,
                automatic,
            }) => return (thread_id, prompt, automatic),
            Ok(_) => continue,
            Err(TryRecvError::Empty) => {
                panic!("expected RequestSessionRecap event but queue was empty")
            }
            Err(TryRecvError::Disconnected) => {
                panic!("expected RequestSessionRecap event but channel closed")
            }
        }
    }
}

#[tokio::test]
async fn service_tier_commands_lowercase_catalog_names() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    let mut preset = get_available_model(&chat, "gpt-5.4");
    let expected_description = preset
        .service_tiers
        .iter()
        .find(|tier| tier.id == ServiceTier::Fast.request_value())
        .expect("fast tier")
        .description
        .clone();
    preset
        .service_tiers
        .iter_mut()
        .find(|tier| tier.id == ServiceTier::Fast.request_value())
        .expect("fast tier")
        .name = "Fast".to_string();
    chat.model_catalog = std::sync::Arc::new(ModelCatalog::new(vec![preset]));

    assert_eq!(
        chat.current_model_service_tier_commands(),
        vec![ServiceTierCommand {
            id: ServiceTier::Fast.request_value().to_string(),
            name: "fast".to_string(),
            description: expected_description,
        }]
    );
}

#[tokio::test]
async fn slash_compact_eagerly_queues_follow_up_before_turn_start() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Compact);

    assert!(chat.bottom_pane.is_task_running());
    match rx.try_recv() {
        Ok(AppEvent::CodexOp(Op::Compact)) => {}
        other => panic!("expected compact op to be submitted, got {other:?}"),
    }

    chat.bottom_pane.set_composer_text(
        "queued before compact turn start".to_string(),
        Vec::new(),
        Vec::new(),
    );
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(chat.input_queue.pending_steers.is_empty());
    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue.queued_user_messages.front().unwrap().text,
        "queued before compact turn start"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn queued_slash_compact_dispatches_after_active_turn() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/compact");

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue
            .queued_user_messages
            .front()
            .unwrap()
            .action,
        QueuedInputAction::ParseSlash
    );
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::CodexOp(Op::Compact))),
        "expected queued /compact to submit compact op; events: {events:?}"
    );
}

#[tokio::test]
async fn queued_slash_review_with_args_dispatches_after_active_turn() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/review check regressions");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    match op_rx.try_recv() {
        Ok(Op::Review { target }) => assert_eq!(
            target,
            ReviewTarget::Custom {
                instructions: "check regressions".to_string(),
            }
        ),
        other => panic!("expected queued /review to submit review op, got {other:?}"),
    }
}

#[tokio::test]
async fn queued_slash_review_with_args_restores_for_edit() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/review check regressions");
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));

    assert_eq!(
        chat.bottom_pane.composer_text(),
        "/review check regressions"
    );
}

#[tokio::test]
async fn queued_bang_shell_dispatches_after_active_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "!echo hi");

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue
            .queued_user_messages
            .front()
            .unwrap()
            .action,
        QueuedInputAction::RunShell
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    match op_rx.try_recv() {
        Ok(Op::RunUserShellCommand { command }) => assert_eq!(command, "echo hi"),
        other => panic!("expected queued shell command op, got {other:?}"),
    }
    assert_eq!(next_add_to_history_event(&mut rx), "!echo hi");
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_empty_bang_shell_reports_help_when_dequeued_and_drains_next_input() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "!");
    queue_composer_text_with_tab(&mut chat, "hello after help");

    assert!(drain_insert_history(&mut rx).is_empty());

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(USER_SHELL_COMMAND_HELP_TITLE),
        "expected delayed shell help, got {rendered:?}"
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after help".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after empty shell command, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_bang_shell_waits_for_user_shell_completion_before_next_input() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "!echo hi");
    queue_composer_text_with_tab(&mut chat, "hello after shell");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    match op_rx.try_recv() {
        Ok(Op::RunUserShellCommand { command }) => assert_eq!(command, "echo hi"),
        other => panic!("expected queued shell command op, got {other:?}"),
    }
    assert_eq!(next_add_to_history_event(&mut rx), "!echo hi");
    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);

    let begin = begin_exec_with_source(
        &mut chat,
        "user-shell-echo",
        "echo hi",
        ExecCommandSource::UserShell,
    );
    end_exec(&mut chat, begin, "hi\n", "", /*exit_code*/ 0);

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after shell".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after shell completion, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

async fn assert_cancelled_queued_menu_drains_next_input(command: &str, expected_popup_text: &str) {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, command);
    queue_composer_text_with_tab(&mut chat, "hello after menu");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains(expected_popup_text),
        "expected {command} menu to open; popup:\n{popup}"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after menu".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after cancelling {command}, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_slash_menu_cancel_drains_next_input() {
    assert_cancelled_queued_menu_drains_next_input("/model", "Select Model").await;
    assert_cancelled_queued_menu_drains_next_input("/permissions", "Update Model Permissions")
        .await;
}

#[tokio::test]
async fn queued_slash_menu_selection_drains_next_input() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/permissions");
    queue_composer_text_with_tab(&mut chat, "hello after selection");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Update Model Permissions"),
        "expected permissions menu to open; popup:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after selection".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after permissions selection, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_bare_rename_drains_next_input_after_name_update() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/rename");
    queue_composer_text_with_tab(&mut chat, "hello after rename");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Name thread"));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    chat.handle_paste("Queued rename".to_string());
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::SetThreadName { name }) if name == "Queued rename"
        )),
        "expected rename prompt to submit thread name; events: {events:?}"
    );

    chat.handle_server_notification(
        ServerNotification::ThreadNameUpdated(
            codex_app_server_protocol::ThreadNameUpdatedNotification {
                thread_id: thread_id.to_string(),
                thread_name: Some("Queued rename".to_string()),
            },
        ),
        /*replay_kind*/ None,
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after rename".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after /rename, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_inline_rename_does_not_drain_again_before_turn_started() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/rename Queued rename");
    queue_composer_text_with_tab(&mut chat, "first after rename");
    queue_composer_text_with_tab(&mut chat, "second after rename");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::SetThreadName { name }) if name == "Queued rename"
        )),
        "expected queued /rename to submit thread name; events: {events:?}"
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "first after rename".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected first queued message after /rename, got {other:?}"),
    }
    assert!(events.iter().any(|event| matches!(
        event,
        AppEvent::AppendMessageHistoryEntry { text, .. } if text == "first after rename"
    )));
    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["second after rename"]
    );
    let input_state = chat.capture_thread_input_state().unwrap();
    assert!(input_state.user_turn_pending_start);
    chat.restore_thread_input_state(/*input_state*/ None);
    assert!(!chat.input_queue.user_turn_pending_start);
    chat.restore_thread_input_state(Some(input_state));
    assert!(chat.input_queue.user_turn_pending_start);
    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["second after rename"]
    );

    chat.handle_server_notification(
        ServerNotification::ThreadNameUpdated(
            codex_app_server_protocol::ThreadNameUpdatedNotification {
                thread_id: thread_id.to_string(),
                thread_name: Some("Queued rename".to_string()),
            },
        ),
        /*replay_kind*/ None,
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["second after rename"]
    );

    handle_turn_started(&mut chat, "turn-2");
    complete_turn_with_message(&mut chat, "turn-2", Some("done"));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "second after rename".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected second queued message after turn complete, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_unknown_slash_reports_error_when_dequeued() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/does-not-exist");

    assert!(drain_insert_history(&mut rx).is_empty());

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Unrecognized command '/does-not-exist'"),
        "expected delayed slash error, got {rendered:?}"
    );
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn ctrl_d_quits_without_prompt() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_matches!(rx.try_recv(), Ok(AppEvent::Exit(ExitMode::ShutdownFirst)));
}

#[tokio::test]
async fn ctrl_d_with_modal_open_does_not_quit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_approvals_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));

    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn slash_init_skips_when_project_doc_exists() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let tempdir = tempdir().unwrap();
    let existing_path = tempdir.path().join(DEFAULT_PROJECT_AGENTS_MD_PATH);
    std::fs::create_dir_all(existing_path.parent().expect("project doc parent")).unwrap();
    std::fs::write(&existing_path, "existing instructions").unwrap();
    chat.config.cwd = tempdir.path().to_path_buf().abs();

    submit_composer_text(&mut chat, "/init");

    match op_rx.try_recv() {
        Err(TryRecvError::Empty) => {}
        other => panic!("expected no Codewith op to be sent, got {other:?}"),
    }

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(DEFAULT_PROJECT_AGENTS_MD_PATH),
        "info message should mention the existing file: {rendered:?}"
    );
    assert!(
        rendered.contains("Skipping /init"),
        "info message should explain why /init was skipped: {rendered:?}"
    );
    assert_eq!(
        std::fs::read_to_string(existing_path).unwrap(),
        "existing instructions"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/init");
}

#[tokio::test]
async fn bare_slash_command_is_available_from_local_recall_after_dispatch() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/diff");

    let _ = drain_insert_history(&mut rx);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "/diff");
}

#[tokio::test]
async fn inline_slash_command_is_available_from_local_recall_after_dispatch() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/rename Better title");

    let _ = drain_insert_history(&mut rx);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "/rename Better title");
}

#[tokio::test]
async fn goal_slash_command_emits_set_goal_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/goal --tokens 98.5K improve benchmark coverage";

    submit_composer_text(&mut chat, command);

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        mode,
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "--tokens 98.5K improve benchmark coverage");
    assert_eq!(mode, crate::app_event::ThreadGoalSetMode::ConfirmIfExists);
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn goal_slash_command_uses_plain_text_for_mentions() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        "/goal use $figma for the mockup".to_string(),
        Vec::new(),
        Vec::new(),
        vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        ..
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "use $figma for the mockup");
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn goal_slash_command_drops_attached_images() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/goal.png".to_string();
    let local_image = PathBuf::from("/tmp/goal-local.png");
    let placeholder = "[Image #2]";
    let command = format!("/goal describe {placeholder}");
    let placeholder_start = command.find(placeholder).expect("placeholder in command");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane.set_composer_text(
        command,
        vec![TextElement::new(
            (placeholder_start..placeholder_start + placeholder.len()).into(),
            Some(placeholder.to_string()),
        )],
        vec![local_image],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        ..
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "describe [Image #2]");
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn recap_slash_command_emits_recap_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/recap";

    submit_composer_text(&mut chat, command);

    let (actual_thread_id, prompt, automatic) = next_session_recap_request_event(&mut rx);
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(prompt, None);
    assert!(!automatic);
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn recap_slash_command_with_prompt_emits_recap_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/recap list unresolved blockers";

    submit_composer_text(&mut chat, command);

    let (actual_thread_id, prompt, automatic) = next_session_recap_request_event(&mut rx);
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(prompt, Some("list unresolved blockers".to_string()));
    assert!(!automatic);
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn loop_slash_command_emits_create_schedule_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/loop 5m check whether CI is green";

    submit_composer_text(&mut chat, command);

    let event = rx.try_recv().expect("expected loop create event");
    let AppEvent::CreateThreadLoopSchedule {
        thread_id: actual_thread_id,
        prompt,
        prompt_source,
        schedule,
    } = event
    else {
        panic!("expected CreateThreadLoopSchedule, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(prompt, "check whether CI is green");
    assert_eq!(prompt_source, ThreadSchedulePromptSource::Inline);
    assert_eq!(
        schedule,
        ThreadScheduleSpec::Interval {
            amount: 5,
            unit: ThreadScheduleIntervalUnit::Minutes,
        }
    );
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn loop_slash_command_without_prompt_uses_default_loop_prompt() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/loop 5m";

    submit_composer_text(&mut chat, command);

    let event = rx.try_recv().expect("expected loop create event");
    let AppEvent::CreateThreadLoopSchedule {
        thread_id: actual_thread_id,
        prompt,
        prompt_source,
        schedule,
    } = event
    else {
        panic!("expected CreateThreadLoopSchedule, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(prompt, "Default loop prompt");
    assert_eq!(prompt_source, ThreadSchedulePromptSource::Default);
    assert_eq!(
        schedule,
        ThreadScheduleSpec::Interval {
            amount: 5,
            unit: ThreadScheduleIntervalUnit::Minutes,
        }
    );
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn schedule_slash_command_emits_create_schedule_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/schedule 5m check whether CI is green";
    let before = chrono::Utc::now().timestamp();

    submit_composer_text(&mut chat, command);
    let after = chrono::Utc::now().timestamp();

    let event = rx.try_recv().expect("expected schedule create event");
    let AppEvent::CreateThreadSchedule {
        thread_id: actual_thread_id,
        prompt,
        prompt_source,
        schedule,
        next_run_at,
    } = event
    else {
        panic!("expected CreateThreadSchedule, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(prompt, "check whether CI is green");
    assert_eq!(prompt_source, ThreadSchedulePromptSource::Inline);
    assert_eq!(schedule, ThreadScheduleSpec::Once);
    let next_run_at = next_run_at.expect("one-time schedule should include next_run_at");
    assert!(next_run_at >= before + 300);
    assert!(next_run_at <= after + 300);
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn schedule_slash_command_accepts_exact_timestamp() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/schedule 2099-06-05T09:30:00Z ask me something";

    submit_composer_text(&mut chat, command);

    let event = rx.try_recv().expect("expected schedule create event");
    let AppEvent::CreateThreadSchedule {
        thread_id: actual_thread_id,
        prompt,
        prompt_source,
        schedule,
        next_run_at,
    } = event
    else {
        panic!("expected CreateThreadSchedule, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(prompt, "ask me something");
    assert_eq!(prompt_source, ThreadSchedulePromptSource::Inline);
    assert_eq!(schedule, ThreadScheduleSpec::Once);
    assert_eq!(
        next_run_at,
        Some(
            chrono::DateTime::parse_from_rfc3339("2099-06-05T09:30:00Z")
                .expect("test timestamp should parse")
                .timestamp()
        )
    );
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn loop_slash_command_emits_manage_events() {
    let cases = [
        ("/loop list", "list", None),
        ("/loop pause sched-1", "pause", Some("sched-1")),
        ("/loop resume sched-1", "resume", Some("sched-1")),
        ("/loop delete sched-1", "delete", Some("sched-1")),
        ("/loop run-now sched-1", "run-now", Some("sched-1")),
        ("/loop edit sched-1", "edit", Some("sched-1")),
        ("/loop stats sched-1", "stats", Some("sched-1")),
    ];

    for (command, expected_kind, expected_schedule_id) in cases {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
        let thread_id = ThreadId::new();
        chat.thread_id = Some(thread_id);

        submit_composer_text(&mut chat, command);

        let event = rx.try_recv().expect("expected loop management event");
        match (expected_kind, event) {
            (
                "list",
                AppEvent::OpenThreadLoopManager {
                    thread_id: actual_thread_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
            }
            (
                "pause",
                AppEvent::PauseThreadLoopSchedule {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "resume",
                AppEvent::ResumeThreadLoopSchedule {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "delete",
                AppEvent::DeleteThreadLoopSchedule {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "run-now",
                AppEvent::RunThreadLoopScheduleNow {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "edit",
                AppEvent::OpenThreadLoopEditor {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "stats",
                AppEvent::OpenThreadLoopScheduleStats {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (kind, event) => panic!("expected {kind} loop event, got {event:?}"),
        }
        assert_no_submit_op(&mut op_rx);
    }
}

#[tokio::test]
async fn schedule_slash_command_emits_manage_events() {
    let cases = [
        ("/schedule list", "list", None),
        ("/schedule pause sched-1", "pause", Some("sched-1")),
        ("/schedule resume sched-1", "resume", Some("sched-1")),
        ("/schedule delete sched-1", "delete", Some("sched-1")),
        ("/schedule run-now sched-1", "run-now", Some("sched-1")),
        ("/schedule edit sched-1", "edit", Some("sched-1")),
        ("/schedule stats sched-1", "stats", Some("sched-1")),
    ];

    for (command, expected_kind, expected_schedule_id) in cases {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
        let thread_id = ThreadId::new();
        chat.thread_id = Some(thread_id);

        submit_composer_text(&mut chat, command);

        let event = rx.try_recv().expect("expected schedule management event");
        match (expected_kind, event) {
            (
                "list",
                AppEvent::OpenThreadScheduleManager {
                    thread_id: actual_thread_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
            }
            (
                "pause",
                AppEvent::PauseThreadSchedule {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "resume",
                AppEvent::ResumeThreadSchedule {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "delete",
                AppEvent::DeleteThreadSchedule {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "run-now",
                AppEvent::RunThreadScheduleNow {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "edit",
                AppEvent::OpenThreadScheduleEditor {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (
                "stats",
                AppEvent::OpenThreadScheduleStats {
                    thread_id: actual_thread_id,
                    schedule_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(schedule_id.as_deref(), expected_schedule_id);
            }
            (kind, event) => panic!("expected {kind} schedule event, got {event:?}"),
        }
        assert_no_submit_op(&mut op_rx);
    }
}

#[tokio::test]
async fn monitor_slash_command_emits_manage_events() {
    let cases = [
        ("/monitor list", "list", None),
        ("/monitor read", "read", None),
        ("/monitor read mon-1", "read", Some("mon-1")),
        ("/monitor stop mon-1", "stop", Some("mon-1")),
        ("/monitor restart mon-1", "restart", Some("mon-1")),
        ("/monitor delete mon-1", "delete", Some("mon-1")),
    ];

    for (command, expected_kind, expected_monitor_id) in cases {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
        let thread_id = ThreadId::new();
        chat.thread_id = Some(thread_id);

        submit_composer_text(&mut chat, command);

        let event = rx.try_recv().expect("expected monitor management event");
        match (expected_kind, event) {
            (
                "list",
                AppEvent::OpenThreadMonitorManager {
                    thread_id: actual_thread_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
            }
            (
                "read",
                AppEvent::ReadThreadMonitor {
                    thread_id: actual_thread_id,
                    monitor_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(monitor_id.as_deref(), expected_monitor_id);
            }
            (
                "stop",
                AppEvent::StopThreadMonitor {
                    thread_id: actual_thread_id,
                    monitor_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(monitor_id.as_deref(), expected_monitor_id);
            }
            (
                "restart",
                AppEvent::RestartThreadMonitor {
                    thread_id: actual_thread_id,
                    monitor_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(monitor_id.as_deref(), expected_monitor_id);
            }
            (
                "delete",
                AppEvent::DeleteThreadMonitor {
                    thread_id: actual_thread_id,
                    monitor_id,
                },
            ) => {
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(monitor_id.as_deref(), expected_monitor_id);
            }
            (kind, event) => panic!("expected {kind} monitor event, got {event:?}"),
        }
        assert_no_submit_op(&mut op_rx);
        assert_eq!(recall_latest_after_clearing(&mut chat), command);
    }
}

#[tokio::test]
async fn monitor_manage_slash_command_queues_before_thread_starts() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let command = "/monitor stop mon-1";

    submit_composer_text(&mut chat, command);

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue.queued_user_messages.front().unwrap().text,
        command
    );
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.maybe_send_next_queued_input();

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::StopThreadMonitor {
            thread_id: actual_thread_id,
            monitor_id,
        }) if actual_thread_id == thread_id && monitor_id.as_deref() == Some("mon-1")
    );
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn monitor_manage_slash_command_rejects_extra_args() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    chat.thread_id = Some(ThreadId::new());

    submit_composer_text(&mut chat, "/monitor read mon-1 extra");

    let event = rx.try_recv().expect("expected monitor usage error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains("Usage: /monitor read [id]"),
                "expected monitor read usage error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn agent_slash_command_emits_active_session_events() {
    let cases = [
        ("/agent peers", "list", "", "", false),
        (
            "/agent send 019eca00-0000-7000-8000-000000000001 hello active peer",
            "send",
            "019eca00-0000-7000-8000-000000000001",
            "hello active peer",
            false,
        ),
        (
            "/agent send --wake 019eca00-0000-7000-8000-000000000001 \"hello active peer\"",
            "send",
            "019eca00-0000-7000-8000-000000000001",
            "hello active peer",
            true,
        ),
    ];

    for (command, expected_kind, expected_target, expected_message, expected_wake) in cases {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

        submit_composer_text(&mut chat, command);

        let event = rx.try_recv().expect("expected active-session event");
        match (expected_kind, event) {
            ("list", AppEvent::ListActiveSessions) => {}
            (
                "send",
                AppEvent::SendActiveSessionMessage {
                    target_peer_id,
                    message,
                    wake,
                },
            ) => {
                assert_eq!(target_peer_id, expected_target);
                assert_eq!(message, expected_message);
                assert_eq!(wake, expected_wake);
            }
            (kind, event) => panic!("expected {kind} active-session event, got {event:?}"),
        }
        assert_no_submit_op(&mut op_rx);
        assert_eq!(recall_latest_after_clearing(&mut chat), command);
    }
}

#[tokio::test]
async fn background_agent_slash_command_emits_manage_events() {
    let cases = [
        ("/agent list", "list", None, None, None),
        (
            "/agent start fix the flaky test",
            "start",
            None,
            Some("fix the flaky test"),
            None,
        ),
        (
            "/agent start --worktree wt-123 fix the flaky test",
            "start",
            None,
            Some("fix the flaky test"),
            Some("wt-123"),
        ),
        ("/background-agent list", "list", None, None, None),
        (
            "/background-agent diagnostics",
            "diagnostics",
            None,
            None,
            None,
        ),
        (
            "/background-agent start fix the flaky test",
            "start",
            None,
            Some("fix the flaky test"),
            None,
        ),
        (
            "/background-agent start --worktree=wt-456 fix the flaky test",
            "start",
            None,
            Some("fix the flaky test"),
            Some("wt-456"),
        ),
        ("/background-agent read", "read", None, None, None),
        (
            "/background-agent read agent-1",
            "read",
            Some("agent-1"),
            None,
            None,
        ),
        (
            "/background-agent logs agent-1",
            "logs",
            Some("agent-1"),
            None,
            None,
        ),
        (
            "/background-agent attach agent-1",
            "attach",
            Some("agent-1"),
            None,
            None,
        ),
        (
            "/background-agent detach agent-1",
            "detach",
            Some("agent-1"),
            None,
            None,
        ),
        (
            "/background-agent stop agent-1",
            "stop",
            Some("agent-1"),
            None,
            None,
        ),
        (
            "/background-agent delete agent-1",
            "delete",
            Some("agent-1"),
            None,
            None,
        ),
    ];

    for (command, expected_kind, expected_agent_id, expected_prompt, expected_worktree_id) in cases
    {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

        submit_composer_text(&mut chat, command);

        let event = rx
            .try_recv()
            .expect("expected background-agent management event");
        match (expected_kind, event) {
            ("list", AppEvent::OpenBackgroundAgentManager) => {}
            ("diagnostics", AppEvent::ShowBackgroundAgentDiagnostics) => {}
            (
                "start",
                AppEvent::StartBackgroundAgent {
                    prompt,
                    worktree_id,
                },
            ) => {
                assert_eq!(Some(prompt.as_str()), expected_prompt);
                assert_eq!(worktree_id.as_deref(), expected_worktree_id);
            }
            ("read", AppEvent::ReadBackgroundAgent { agent_id }) => {
                assert_eq!(agent_id.as_deref(), expected_agent_id);
            }
            ("logs", AppEvent::ShowBackgroundAgentLogs { agent_id }) => {
                assert_eq!(agent_id.as_deref(), expected_agent_id);
            }
            ("attach", AppEvent::AttachBackgroundAgent { agent_id }) => {
                assert_eq!(agent_id.as_deref(), expected_agent_id);
            }
            ("detach", AppEvent::DetachBackgroundAgent { agent_id }) => {
                assert_eq!(agent_id.as_deref(), expected_agent_id);
            }
            ("stop", AppEvent::StopBackgroundAgent { agent_id }) => {
                assert_eq!(agent_id.as_deref(), expected_agent_id);
            }
            ("delete", AppEvent::DeleteBackgroundAgent { agent_id }) => {
                assert_eq!(agent_id.as_deref(), expected_agent_id);
            }
            (kind, event) => panic!("expected {kind} background-agent event, got {event:?}"),
        }
        assert_no_submit_op(&mut op_rx);
        assert_eq!(recall_latest_after_clearing(&mut chat), command);
    }
}

#[tokio::test]
async fn pr_slash_command_opens_read_only_overview() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let _ = drain_insert_history(&mut rx);

    submit_composer_text(&mut chat, "/pr");

    let event = rx.try_recv().expect("expected pr overview event");
    match event {
        AppEvent::OpenPullRequestOverview => {}
        other => panic!("expected OpenPullRequestOverview event, got {other:?}"),
    }
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::AppendMessageHistoryEntry {
            thread_id: event_thread_id,
            text,
        }) if event_thread_id == thread_id && text == "/pr"
    );
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), "/pr");
}

#[tokio::test]
async fn worktree_slash_command_emits_manage_events() {
    let cases = [
        ("/worktree", "list", None),
        ("/worktree list", "list", None),
        ("/worktree reconcile", "reconcile", None),
        ("/worktree create feature", "create", Some("feature")),
        ("/worktree read wt-123", "read", Some("wt-123")),
        ("/worktree actions wt-456", "actions", Some("wt-456")),
        ("/worktree use wt-789", "use", Some("wt-789")),
        ("/worktree release wt-999", "release", Some("wt-999")),
        (
            "/worktree cleanup --force wt-999",
            "cleanup",
            Some("wt-999"),
        ),
        ("/worktree merge wt-999 main", "merge", Some("wt-999")),
    ];

    for (command, expected_kind, expected_worktree_id) in cases {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

        submit_composer_text(&mut chat, command);

        let event = rx.try_recv().expect("expected worktree event");
        match (expected_kind, event) {
            ("list", AppEvent::OpenWorktreeManager) => {}
            ("reconcile", AppEvent::ReconcileWorktrees) => {}
            (
                "create",
                AppEvent::CreateWorktree {
                    name,
                    branch,
                    start_point,
                },
            ) => {
                assert_eq!(name.as_deref(), expected_worktree_id);
                assert_eq!(branch, None);
                assert_eq!(start_point, None);
            }
            (
                "read",
                AppEvent::ReadWorktree {
                    worktree_id,
                    base_repo_path,
                },
            ) => {
                assert_eq!(worktree_id.as_deref(), expected_worktree_id);
                assert_eq!(base_repo_path, None);
            }
            (
                "actions",
                AppEvent::OpenWorktreeActions {
                    worktree_id,
                    base_repo_path,
                },
            ) => {
                assert_eq!(Some(worktree_id.as_str()), expected_worktree_id);
                assert_eq!(base_repo_path, None);
            }
            (
                "use",
                AppEvent::UseWorktree {
                    worktree_id,
                    base_repo_path,
                },
            ) => {
                assert_eq!(Some(worktree_id.as_str()), expected_worktree_id);
                assert_eq!(base_repo_path, None);
            }
            (
                "release",
                AppEvent::ReleaseWorktree {
                    worktree_id,
                    base_repo_path,
                },
            ) => {
                assert_eq!(Some(worktree_id.as_str()), expected_worktree_id);
                assert_eq!(base_repo_path, None);
            }
            (
                "cleanup",
                AppEvent::CleanupWorktree {
                    worktree_id,
                    base_repo_path,
                    force_delete,
                },
            ) => {
                assert_eq!(Some(worktree_id.as_str()), expected_worktree_id);
                assert_eq!(base_repo_path, None);
                assert!(force_delete);
            }
            (
                "merge",
                AppEvent::RefreshWorktreeMergeCandidate {
                    worktree_id,
                    base_repo_path,
                    target_ref,
                },
            ) => {
                assert_eq!(Some(worktree_id.as_str()), expected_worktree_id);
                assert_eq!(base_repo_path, None);
                assert_eq!(target_ref.as_deref(), Some("main"));
            }
            (kind, event) => panic!("expected {kind} worktree event, got {event:?}"),
        }
        assert_no_submit_op(&mut op_rx);
        assert_eq!(recall_latest_after_clearing(&mut chat), command);
    }
}

#[tokio::test]
async fn background_agent_manager_grouped_roster_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.show_background_agent_manager(vec![
        test_background_agent(
            "done-agent",
            AgentRunStatus::Completed,
            local_timestamp_for_snapshot(/*hour*/ 2, /*minute*/ 0, /*second*/ 3),
        ),
        test_background_agent(
            "run-agent",
            AgentRunStatus::Running,
            local_timestamp_for_snapshot(/*hour*/ 2, /*minute*/ 0, /*second*/ 2),
        ),
        test_background_agent(
            "wait-agent",
            AgentRunStatus::WaitingOnUser,
            local_timestamp_for_snapshot(/*hour*/ 2, /*minute*/ 0, /*second*/ 1),
        ),
        test_background_agent(
            "stop-agent",
            AgentRunStatus::Cancelled,
            local_timestamp_for_snapshot(/*hour*/ 2, /*minute*/ 0, /*second*/ 4),
        ),
    ]);

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert_chatwidget_snapshot!("background_agent_manager_grouped_roster", popup);
}

fn local_timestamp_for_snapshot(hour: u32, minute: u32, second: u32) -> i64 {
    let naive = NaiveDate::from_ymd_opt(1970, 1, 1)
        .expect("valid snapshot date")
        .and_hms_opt(hour, minute, second)
        .expect("valid snapshot time");
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(datetime) => datetime.timestamp(),
        LocalResult::Ambiguous(datetime, _) => datetime.timestamp(),
        LocalResult::None => naive.and_utc().timestamp(),
    }
}

#[tokio::test]
async fn bare_loop_slash_command_opens_manager_and_drains_attachments() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/loop.png".to_string();
    let local_image = PathBuf::from("/tmp/loop-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/loop".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenThreadLoopManager { thread_id: opened }) if opened == thread_id
    );
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn bare_schedule_slash_command_opens_manager_and_drains_attachments() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/schedule.png".to_string();
    let local_image = PathBuf::from("/tmp/schedule-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/schedule".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenThreadScheduleManager { thread_id: opened }) if opened == thread_id
    );
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn bare_monitor_slash_command_opens_manager_and_drains_attachments() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/monitor.png".to_string();
    let local_image = PathBuf::from("/tmp/monitor-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/monitor".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenThreadMonitorManager { thread_id: opened }) if opened == thread_id
    );
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn bare_background_agent_slash_command_opens_manager_and_drains_attachments() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let remote_url = "https://example.com/background-agent.png".to_string();
    let local_image = PathBuf::from("/tmp/background-agent-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane.set_composer_text(
        "/background-agent".to_string(),
        Vec::new(),
        vec![local_image],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenBackgroundAgentManager));
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn bare_agent_slash_command_opens_background_agent_manager_and_drains_attachments() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let remote_url = "https://example.com/agent.png".to_string();
    let local_image = PathBuf::from("/tmp/agent-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/agent".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenBackgroundAgentManager));
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn bare_session_slash_command_opens_agent_picker_and_drains_attachments() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let remote_url = "https://example.com/session.png".to_string();
    let local_image = PathBuf::from("/tmp/session-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/session".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenAgentPicker));
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn monitor_slash_command_submits_setup_prompt() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    chat.thread_id = Some(ThreadId::new());
    let command = "/monitor watch CI until it fails";

    submit_composer_text(&mut chat, command);

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            let [
                UserInput::Text {
                    text: submitted, ..
                },
            ] = items.as_slice()
            else {
                panic!("expected monitor setup prompt text item, got {items:?}");
            };
            assert!(submitted.contains("Set up a Codewith monitor for this request."));
            assert!(submitted.contains("Use the `manage_monitor` tool"));
            assert!(submitted.contains("create exactly one monitor"));
            assert!(submitted.contains("watch CI until it fails"));
        }
        other => panic!("expected monitor setup user turn, got {other:?}"),
    }
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn monitor_slash_command_queues_setup_prompt_before_thread_starts() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::ScheduledTasks, /*enabled*/ true);
    let command = "/monitor alert when the release log changes";

    submit_composer_text(&mut chat, command);

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    let queued = &chat.input_queue.queued_user_messages.front().unwrap().text;
    assert!(queued.contains("Use the `manage_monitor` tool"));
    assert!(queued.contains("alert when the release log changes"));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    chat.thread_id = Some(ThreadId::new());
    chat.maybe_send_next_queued_input();

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            let [
                UserInput::Text {
                    text: submitted, ..
                },
            ] = items.as_slice()
            else {
                panic!("expected queued monitor setup prompt text item, got {items:?}");
            };
            assert!(submitted.contains("Use the `manage_monitor` tool"));
            assert!(submitted.contains("alert when the release log changes"));
        }
        other => panic!("expected queued monitor setup user turn, got {other:?}"),
    }
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn bare_goal_slash_command_drains_pending_submission_state() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/goal-menu.png".to_string();
    let local_image = PathBuf::from("/tmp/goal-menu-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/goal".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenThreadGoalMenu { thread_id: opened }) if opened == thread_id
    );
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn goal_control_slash_commands_emit_goal_events() {
    let cases = [
        ("/goal clear", None),
        ("/goal pause", Some(AppThreadGoalStatus::Paused)),
        ("/goal resume", Some(AppThreadGoalStatus::Active)),
        ("/goal cancel", Some(AppThreadGoalStatus::Cancelled)),
    ];

    for (command, status) in cases {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
        let thread_id = ThreadId::new();
        chat.thread_id = Some(thread_id);

        submit_composer_text(&mut chat, command);

        match status {
            Some(status) => {
                let event = rx.try_recv().expect("expected goal status event");
                let AppEvent::SetThreadGoalStatus {
                    thread_id: actual_thread_id,
                    status: actual_status,
                } = event
                else {
                    panic!("expected SetThreadGoalStatus, got {event:?}");
                };
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(actual_status, status);
            }
            None => {
                let event = rx.try_recv().expect("expected clear goal event");
                let AppEvent::ClearThreadGoal {
                    thread_id: actual_thread_id,
                } = event
                else {
                    panic!("expected ClearThreadGoal, got {event:?}");
                };
                assert_eq!(actual_thread_id, thread_id);
            }
        }
    }
}

#[tokio::test]
async fn goal_control_slash_command_without_thread_shows_full_usage() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);

    submit_composer_text(&mut chat, "/goal pause");

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected goal usage message");
    insta::assert_snapshot!(
        lines_to_single_string(&cells[0]),
        @"• Usage: /goal [<objective>|cancel|clear|edit|pause|resume] The session must start before you can change a goal."
    );
}

#[tokio::test]
async fn goal_edit_slash_command_opens_goal_editor() {
    for thread_id in [Some(ThreadId::new()), None] {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
        chat.thread_id = thread_id;

        submit_composer_text(&mut chat, "/goal edit");

        let event = rx.try_recv().expect("expected goal editor event");
        let AppEvent::OpenThreadGoalEditor {
            thread_id: actual_thread_id,
        } = event
        else {
            panic!("expected OpenThreadGoalEditor, got {event:?}");
        };
        assert_eq!(actual_thread_id, thread_id);
        assert_no_submit_op(&mut op_rx);
    }
}

#[tokio::test]
async fn workflow_list_slash_command_emits_metadata_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/workflow list";

    submit_composer_text(&mut chat, command);

    let event = rx.try_recv().expect("expected workflow manager event");
    let AppEvent::ManageThreadWorkflow {
        thread_id: actual_thread_id,
        action,
    } = event
    else {
        panic!("expected ManageThreadWorkflow, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(action, crate::app_event::ThreadWorkflowAction::List);
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn workflow_run_slash_commands_emit_management_events() {
    let cases = [
        (
            "/workflow show workflow-1",
            crate::app_event::ThreadWorkflowAction::Show {
                workflow_record_id: "workflow-1".to_string(),
            },
        ),
        (
            "/workflow run",
            crate::app_event::ThreadWorkflowAction::RunList,
        ),
        (
            "/workflow run show run-1",
            crate::app_event::ThreadWorkflowAction::RunShow {
                run_id: "run-1".to_string(),
            },
        ),
        (
            "/workflow run start workflow-1",
            crate::app_event::ThreadWorkflowAction::RunStart {
                workflow_record_id: "workflow-1".to_string(),
            },
        ),
        (
            "/workflow run pause run-1",
            crate::app_event::ThreadWorkflowAction::RunPause {
                run_id: "run-1".to_string(),
            },
        ),
        (
            "/workflow run resume run-1",
            crate::app_event::ThreadWorkflowAction::RunResume {
                run_id: "run-1".to_string(),
            },
        ),
        (
            "/workflow run cancel run-1",
            crate::app_event::ThreadWorkflowAction::RunCancel {
                run_id: "run-1".to_string(),
            },
        ),
    ];

    for (command, expected_action) in cases {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
        let thread_id = ThreadId::new();
        chat.thread_id = Some(thread_id);

        submit_composer_text(&mut chat, command);

        let event = rx.try_recv().expect("expected workflow event");
        let AppEvent::ManageThreadWorkflow {
            thread_id: actual_thread_id,
            action,
        } = event
        else {
            panic!("expected ManageThreadWorkflow, got {event:?}");
        };
        assert_eq!(actual_thread_id, thread_id);
        assert_eq!(action, expected_action);
        assert_no_submit_op(&mut op_rx);
        assert_eq!(recall_latest_after_clearing(&mut chat), command);
    }
}

#[tokio::test]
async fn workflow_run_controls_work_while_task_running_but_draft_is_blocked() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::Workflow,
        "run cancel run-1".to_string(),
        Vec::new(),
    );

    let event = rx.try_recv().expect("expected workflow run cancel event");
    let AppEvent::ManageThreadWorkflow {
        thread_id: actual_thread_id,
        action,
    } = event
    else {
        panic!("expected ManageThreadWorkflow, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(
        action,
        crate::app_event::ThreadWorkflowAction::RunCancel {
            run_id: "run-1".to_string(),
        }
    );
    assert_no_submit_op(&mut op_rx);

    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
    chat.thread_id = Some(ThreadId::new());
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.handle_slash_command_with_args_dispatch(
        SlashCommand::Workflow,
        "draft build a workflow".to_string(),
        Vec::new(),
    );

    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("'/workflow draft' is disabled while a task is in progress."),
        "expected /workflow draft task-running error, got {rendered:?}"
    );
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn workflow_draft_slash_command_prefills_yaml_generation_prompt_without_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
    chat.thread_id = Some(ThreadId::new());

    submit_composer_text(
        &mut chat,
        "/workflow draft build a SaaS that collects leads for dentists",
    );

    let event = rx
        .try_recv()
        .expect("expected workflow draft prefill event");
    let AppEvent::PrefillComposer { text: submitted } = event else {
        panic!("expected PrefillComposer, got {event:?}");
    };
    assert!(submitted.contains("Return only raw YAML"));
    assert!(submitted.contains("workflow.codex.codewith/v0"));
    assert!(submitted.contains("model_gateway, provider, model, reasoning"));
    assert!(submitted.contains("ancient Greek or Roman"));
    assert!(submitted.contains("at least two adversarial"));
    assert!(submitted.contains("bounded test-loop"));
    assert!(submitted.contains("Do not start goals, schedules, monitors, agents"));
    assert!(submitted.contains("collects leads for dentists"));
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn workflow_commands_are_inert_when_feature_is_disabled() {
    for command in [
        "/workflow",
        "/workflow list",
        "/workflow draft build a SaaS that collects leads for dentists",
        "/workflow run",
        "/workflow run start workflow-1",
    ] {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());

        submit_composer_text(&mut chat, command);

        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_no_submit_op(&mut op_rx);
    }
}

#[tokio::test]
async fn queued_goal_slash_command_emits_set_goal_event_after_thread_starts() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let command = "/goal improve benchmark coverage";

    submit_composer_text(&mut chat, command);
    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.maybe_send_next_queued_input();

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        ..
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "improve benchmark coverage");
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn queued_goal_slash_command_preserves_current_draft_metadata() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let command = "/goal improve benchmark coverage";

    submit_composer_text(&mut chat, command);
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    let remote_url = "https://example.com/current-draft.png".to_string();
    let local_image = PathBuf::from("/tmp/current-draft-local.png");
    let placeholder = "[Image #3]";
    let draft = format!("draft with {placeholder}");
    let placeholder_start = draft.find(placeholder).expect("placeholder in draft");
    chat.set_remote_image_urls(vec![remote_url.clone()]);
    chat.bottom_pane.set_composer_text(
        draft.clone(),
        vec![TextElement::new(
            (placeholder_start..placeholder_start + placeholder.len()).into(),
            Some(placeholder.to_string()),
        )],
        vec![local_image.clone()],
    );

    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.maybe_send_next_queued_input();

    let event = rx.try_recv().expect("expected goal objective event");
    assert_matches!(
        event,
        AppEvent::SetThreadGoalObjective {
            thread_id: actual_thread_id,
            ..
        } if actual_thread_id == thread_id
    );
    assert_no_submit_op(&mut op_rx);
    assert_eq!(chat.bottom_pane.composer_text(), draft);
    assert_eq!(chat.remote_image_urls(), vec![remote_url]);
    assert_eq!(
        chat.bottom_pane.composer_local_image_paths(),
        vec![local_image]
    );
}

#[tokio::test]
async fn restored_queued_goal_slash_command_emits_set_goal_event() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let command = "/goal improve benchmark coverage";

    submit_composer_text(&mut chat, command);
    let input_state = chat
        .capture_thread_input_state()
        .expect("expected queued input state");

    let (mut restored_chat, mut restored_rx, mut restored_op_rx) =
        make_chatwidget_manual(/*model_override*/ None).await;
    restored_chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    restored_chat.restore_thread_input_state(Some(input_state));
    let thread_id = ThreadId::new();
    restored_chat.thread_id = Some(thread_id);
    restored_chat.maybe_send_next_queued_input();

    let event = restored_rx
        .try_recv()
        .expect("expected goal objective event");
    assert_matches!(
        event,
        AppEvent::SetThreadGoalObjective {
            thread_id: actual_thread_id,
            ..
        } if actual_thread_id == thread_id
    );
    assert_no_submit_op(&mut restored_op_rx);
}

#[test]
fn merged_history_record_preserves_raw_text_and_rebased_elements() {
    let first = UserMessage {
        text: "Ask $figma".to_string(),
        local_images: Vec::new(),
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new((4..10).into(), Some("$figma".to_string()))],
        mention_bindings: vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }],
    };
    let second = UserMessage::from("internal prompt");

    let (_message, history_record) = merge_user_messages_with_history_record(vec![
        (first, UserMessageHistoryRecord::UserMessageText),
        (
            second,
            UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
                text: "/goal inspect [Image #1]".to_string(),
                text_elements: vec![TextElement::new(
                    (14..24).into(),
                    Some("[Image #1]".to_string()),
                )],
            }),
        ),
    ]);

    assert_eq!(
        history_record,
        UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
            text: "Ask $figma\n/goal inspect [Image #1]".to_string(),
            text_elements: vec![
                TextElement::new((4..10).into(), Some("$figma".to_string())),
                TextElement::new((25..35).into(), Some("[Image #1]".to_string())),
            ],
        })
    );
}

#[test]
fn merged_history_record_remaps_override_image_placeholders() {
    let first_placeholder = "[Image #1]";
    let second_placeholder = "[Image #1]";
    let first = UserMessage {
        text: format!("first {first_placeholder}"),
        local_images: vec![LocalImageAttachment {
            placeholder: first_placeholder.to_string(),
            path: PathBuf::from("/tmp/first.png"),
        }],
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new(
            (6..16).into(),
            Some(first_placeholder.to_string()),
        )],
        mention_bindings: Vec::new(),
    };
    let second = UserMessage {
        text: format!("internal {second_placeholder}"),
        local_images: vec![LocalImageAttachment {
            placeholder: second_placeholder.to_string(),
            path: PathBuf::from("/tmp/second.png"),
        }],
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new(
            (9..19).into(),
            Some(second_placeholder.to_string()),
        )],
        mention_bindings: Vec::new(),
    };

    let (message, history_record) = merge_user_messages_with_history_record(vec![
        (first, UserMessageHistoryRecord::UserMessageText),
        (
            second,
            UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
                text: format!("goal {second_placeholder}"),
                text_elements: vec![TextElement::new(
                    (5..15).into(),
                    Some(second_placeholder.to_string()),
                )],
            }),
        ),
    ]);

    assert_eq!(message.text, "first [Image #1]\ninternal [Image #2]");
    assert_eq!(
        message.text_elements,
        vec![
            TextElement::new((6..16).into(), Some("[Image #1]".to_string())),
            TextElement::new((26..36).into(), Some("[Image #2]".to_string())),
        ]
    );
    assert_eq!(
        message
            .local_images
            .iter()
            .map(|image| image.placeholder.as_str())
            .collect::<Vec<_>>(),
        vec!["[Image #1]", "[Image #2]"]
    );
    assert_eq!(
        history_record,
        UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
            text: "first [Image #1]\ngoal [Image #2]".to_string(),
            text_elements: vec![
                TextElement::new((6..16).into(), Some("[Image #1]".to_string())),
                TextElement::new((22..32).into(), Some("[Image #2]".to_string())),
            ],
        })
    );
}

#[tokio::test]
async fn interrupted_merged_message_history_encodes_mentions_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.on_task_started();
    chat.on_agent_message_delta("Final answer line\n".to_string());
    let text = "use $figma now";
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        text.to_string(),
        Vec::new(),
        Vec::new(),
        vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            let [
                UserInput::Text {
                    text: submitted, ..
                },
            ] = items.as_slice()
            else {
                panic!("expected text item, got {items:?}");
            };
            assert_eq!(submitted, text);
        }
        other => panic!("expected user turn, got {other:?}"),
    }
    let encoded = "use [$figma](app://figma) now";
    assert_eq!(next_add_to_history_event(&mut rx), encoded);

    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    next_interrupt_op(&mut op_rx);
    chat.on_interrupted_turn(TurnAbortReason::Interrupted);

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            let [
                UserInput::Text {
                    text: submitted, ..
                },
            ] = items.as_slice()
            else {
                panic!("expected resubmitted text item, got {items:?}");
            };
            assert_eq!(submitted, text);
        }
        other => panic!("expected resubmitted user turn, got {other:?}"),
    }
    assert_eq!(next_add_to_history_event(&mut rx), encoded);
}

#[tokio::test]
async fn slash_rename_prefills_existing_thread_name() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_name = Some("Current project title".to_string());

    chat.dispatch_command(SlashCommand::Rename);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_rename_prefilled_prompt", popup);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::CodexOp(Op::SetThreadName { name })) if name == "Current project title"
    );
}

#[tokio::test]
async fn slash_rename_without_existing_thread_name_starts_empty() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Rename);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("Name thread"));
    assert!(popup.contains("Type a name and press Enter"));

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn agent_rename_prompt_submits_thread_scoped_name_update() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let agent_thread_id = ThreadId::new();

    chat.show_agent_rename_prompt(
        agent_thread_id,
        Some("Existing agent title".to_string()),
        "Agent".to_string(),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("agent_rename_prefilled_prompt", popup);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::SubmitThreadOp {
            thread_id,
            op: Op::SetThreadName { name },
        }) if thread_id == agent_thread_id && name == "Existing agent title"
    );
}

#[tokio::test]
async fn usage_error_slash_command_is_available_from_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;

    submit_composer_text(&mut chat, "/raw maybe");

    assert_eq!(chat.bottom_pane.composer_text(), "");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /raw [on|off]"),
        "expected usage message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/raw maybe");
}

#[tokio::test]
async fn unrecognized_slash_command_is_not_added_to_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/does-not-exist");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Unrecognized command '/does-not-exist'"),
        "expected unrecognized-command message, got: {rendered:?}"
    );
    assert_eq!(chat.bottom_pane.composer_text(), "/does-not-exist");
    assert_eq!(recall_latest_after_clearing(&mut chat), "");
}

#[tokio::test]
async fn unavailable_slash_command_is_available_from_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    submit_composer_text(&mut chat, "/model");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("'/model' is disabled while a task is in progress."),
        "expected disabled-command message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/model");
}

#[tokio::test]
async fn slash_quit_requests_exit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Quit);

    assert_matches!(rx.try_recv(), Ok(AppEvent::Exit(ExitMode::ShutdownFirst)));
}

#[tokio::test]
async fn slash_logout_requests_app_server_logout() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Logout);

    assert!(
        rx.try_recv().is_err(),
        "logout should wait for confirmation"
    );
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_logout_confirmation_popup", popup.clone());
    assert!(
        popup.contains("Log out of Codewith?"),
        "expected logout confirmation popup, got:\n{popup}"
    );
    assert!(
        popup.contains("No, keep working"),
        "expected non-destructive default option, got:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::Logout));
}

#[tokio::test]
async fn slash_copy_state_tracks_turn_complete_final_reply() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    complete_turn_with_message(&mut chat, "turn-1", Some("Final reply **markdown**"));

    assert_eq!(
        chat.last_agent_markdown_text(),
        Some("Final reply **markdown**")
    );
}

#[tokio::test]
async fn slash_copy_state_tracks_plan_item_completion() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let plan_text = "## Plan\n\n1. Build it\n2. Test it".to_string();

    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: AppServerThreadItem::Plan {
                id: "plan-1".to_string(),
                text: plan_text.clone(),
            },
        }),
        /*replay_kind*/ None,
    );
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);

    assert_eq!(chat.last_agent_markdown_text(), Some(plan_text.as_str()));
    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response == &plan_text
    );
}

#[tokio::test]
async fn slash_copy_reports_when_no_agent_response_exists() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Copy);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert_chatwidget_snapshot!("slash_copy_no_output_info_message", rendered);
    assert!(
        rendered.contains("No agent response to copy"),
        "expected no-output message, got {rendered:?}"
    );
}

#[tokio::test]
async fn ctrl_o_copy_reports_when_no_agent_response_exists() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("No agent response to copy"),
        "expected no-output message, got {rendered:?}"
    );
}

#[tokio::test]
async fn keymap_capture_can_capture_current_copy_shortcut() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let runtime_keymap = crate::keymap::RuntimeKeymap::defaults();
    chat.open_keymap_capture(
        "composer".to_string(),
        "submit".to_string(),
        crate::app_event::KeymapEditIntent::ReplaceAll,
        &runtime_keymap,
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

    let AppEvent::KeymapCaptured {
        context,
        action,
        key,
        intent,
    } = rx.try_recv().expect("captured key event")
    else {
        panic!("expected keymap capture event");
    };
    assert_eq!(context, "composer");
    assert_eq!(action, "submit");
    assert_eq!(key, "ctrl-o");
    assert_eq!(intent, crate::app_event::KeymapEditIntent::ReplaceAll);
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "copy shortcut should not run while key capture is active"
    );
}

#[tokio::test]
async fn slash_keymap_capture_can_capture_app_shortcuts() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let runtime_keymap = crate::keymap::RuntimeKeymap::defaults();

    for (key, expected) in [('t', "ctrl-t"), ('l', "ctrl-l"), ('g', "ctrl-g")] {
        chat.open_keymap_capture(
            "global".to_string(),
            "open_transcript".to_string(),
            crate::app_event::KeymapEditIntent::ReplaceAll,
            &runtime_keymap,
        );

        chat.handle_key_event(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL));

        let AppEvent::KeymapCaptured {
            context,
            action,
            key,
            intent,
        } = rx.try_recv().expect("captured key event")
        else {
            panic!("expected keymap capture event");
        };
        assert_eq!(context, "global");
        assert_eq!(action, "open_transcript");
        assert_eq!(key, expected);
        assert_eq!(intent, crate::app_event::KeymapEditIntent::ReplaceAll);
    }
}

#[tokio::test]
async fn slash_keymap_debug_opens_keypress_inspector() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Keymap, "debug".to_string(), Vec::new());

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("Keypress Inspector"));
    assert!(popup.contains("Waiting for a keypress"));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("global.copy (Copy)"));
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "debug inspector should open without transcript messages"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_keymap_debug_can_inspect_app_shortcuts() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Keymap, "debug".to_string(), Vec::new());

    for (key, expected_action) in [
        ('t', "global.open_transcript (Open Transcript)"),
        ('l', "global.clear_terminal (Clear Terminal)"),
        ('g', "global.open_external_editor (Open External Editor)"),
    ] {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL));

        let popup = render_bottom_popup(&chat, /*width*/ 100);
        assert!(
            popup.contains(expected_action),
            "expected {expected_action:?} in debug popup for ctrl-{key}, got {popup:?}"
        );
    }

    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "debug inspector should not run app shortcut side effects"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_keymap_debug_can_inspect_permission_cycle_shortcut() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Keymap, "debug".to_string(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::BackTab));

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("global.cycle_permissions (Cycle Permissions)"),
        "expected permission cycle shortcut in debug popup, got {popup:?}"
    );
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "debug inspector should not run app shortcut side effects"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_keymap_invalid_args_show_usage() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/keymap nope");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /keymap [debug]"),
        "expected usage message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/keymap nope");
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn copy_shortcut_can_be_remapped() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut keymap_config = chat.config_ref().tui_keymap.clone();
    keymap_config.global.copy = Some(codex_config::types::KeybindingsSpec::One(
        codex_config::types::KeybindingSpec("ctrl-x".to_string()),
    ));
    let runtime_keymap =
        crate::keymap::RuntimeKeymap::from_config(&keymap_config).expect("valid copy remap");
    chat.apply_keymap_update(keymap_config, &runtime_keymap);

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "old copy shortcut should no longer copy"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("No agent response to copy"),
        "expected remapped copy shortcut to run, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_copy_stores_clipboard_lease_and_preserves_it_on_failure() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.transcript.last_agent_markdown = Some("copy me".to_string());

    chat.copy_last_agent_markdown_with(|markdown| {
        assert_eq!(markdown, "copy me");
        Ok(Some(crate::clipboard_copy::ClipboardLease::test()))
    });

    assert!(chat.clipboard_lease.is_some());
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one success message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Copied last message to clipboard"),
        "expected success message, got {rendered:?}"
    );

    chat.copy_last_agent_markdown_with(|markdown| {
        assert_eq!(markdown, "copy me");
        Err("blocked".into())
    });

    assert!(chat.clipboard_lease.is_some());
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one failure message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Copy failed: blocked"),
        "expected failure message, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_copy_state_is_preserved_during_running_task() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    complete_turn_with_message(&mut chat, "turn-1", Some("Previous completed reply"));
    chat.on_task_started();

    assert_eq!(
        chat.last_agent_markdown_text(),
        Some("Previous completed reply")
    );
}

#[tokio::test]
async fn slash_copy_uses_agent_message_item_when_turn_complete_omits_final_text() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    handle_turn_started(&mut chat, "turn-1");
    complete_assistant_message(
        &mut chat,
        "msg-1",
        "Legacy item final message",
        /*phase*/ None,
    );
    let _ = drain_insert_history(&mut rx);
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    let _ = drain_insert_history(&mut rx);

    assert_eq!(
        chat.last_agent_markdown_text(),
        Some("Legacy item final message")
    );
    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response == "Legacy item final message"
    );
}

#[tokio::test]
async fn agent_turn_complete_notification_does_not_reuse_stale_copy_source() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    complete_turn_with_message(&mut chat, "turn-1", Some("Previous reply"));
    chat.pending_notification = None;

    handle_turn_completed(&mut chat, "turn-2", /*duration_ms*/ None);

    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response.is_empty()
    );
}

#[tokio::test]
async fn active_goal_without_follow_up_suppresses_agent_turn_complete_notification() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    chat.handle_server_notification(
        ServerNotification::ThreadGoalUpdated(
            codex_app_server_protocol::ThreadGoalUpdatedNotification {
                thread_id: "thread-1".to_string(),
                turn_id: None,
                goal: codex_app_server_protocol::ThreadGoal {
                    thread_id: "thread-1".to_string(),
                    goal_id: "goal-1".to_string(),
                    objective: "finish the benchmark".to_string(),
                    status: codex_app_server_protocol::ThreadGoalStatus::Active,
                    token_budget: None,
                    tokens_used: 0,
                    time_used_seconds: 0,
                    created_at: 1,
                    updated_at: 1,
                },
            },
        ),
        /*replay_kind*/ None,
    );

    complete_turn_with_message(&mut chat, "turn-1", Some("Still working"));

    assert_matches!(chat.pending_notification, None);
}

#[tokio::test]
async fn queued_follow_up_suppresses_agent_turn_complete_notification() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");
    chat.queue_user_message("Continue".into());

    complete_turn_with_message(&mut chat, "turn-1", Some("Still working"));

    assert_matches!(chat.pending_notification, None);
    assert!(chat.input_queue.queued_user_messages.is_empty());
    assert_matches!(next_submit_op(&mut op_rx), Op::UserTurn { .. });
}

#[tokio::test]
async fn queued_menu_slash_keeps_agent_turn_complete_notification() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");
    queue_composer_text_with_tab(&mut chat, "/model");

    complete_turn_with_message(&mut chat, "turn-1", Some("Done"));

    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response == "Done"
    );
    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Select Model"));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn slash_copy_uses_latest_surviving_response_after_rollback() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    replay_user_message_text(&mut chat, "user-1", "foo", ReplayKind::ThreadSnapshot);
    replay_agent_message(
        &mut chat,
        "agent-1",
        "foo response",
        ReplayKind::ThreadSnapshot,
    );
    replay_user_message_text(&mut chat, "user-2", "bar", ReplayKind::ThreadSnapshot);
    replay_agent_message(
        &mut chat,
        "agent-2",
        "bar response",
        ReplayKind::ThreadSnapshot,
    );
    let _ = drain_insert_history(&mut rx);
    assert_eq!(chat.last_agent_markdown_text(), Some("bar response"));

    chat.truncate_agent_copy_history_to_user_turn_count(/*user_turn_count*/ 1);

    assert_eq!(chat.last_agent_markdown_text(), Some("foo response"));
    chat.copy_last_agent_markdown_with(|markdown| {
        assert_eq!(markdown, "foo response");
        Ok(None)
    });
}

#[tokio::test]
async fn slash_copy_reports_when_rewind_exceeds_retained_copy_history() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    replay_user_message_text(&mut chat, "user-1", "foo", ReplayKind::ThreadSnapshot);
    replay_agent_message(
        &mut chat,
        "agent-1",
        "foo response",
        ReplayKind::ThreadSnapshot,
    );
    let _ = drain_insert_history(&mut rx);

    chat.truncate_agent_copy_history_to_user_turn_count(/*user_turn_count*/ 0);
    chat.dispatch_command(SlashCommand::Copy);

    let cells = drain_insert_history(&mut rx);
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(
            "Cannot copy that response after rewinding. Only the most recent 32 responses are available to /copy."
        ),
        "expected evicted-history message, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_exit_requests_exit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Exit);

    assert_matches!(rx.try_recv(), Ok(AppEvent::Exit(ExitMode::ShutdownFirst)));
}

#[tokio::test]
async fn slash_changelog_prints_release_notes() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Changelog);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one changelog history cell");
    let rendered = lines_to_single_string(&cells[0]);
    let preview = lines_to_single_string(&cells[0][..cells[0].len().min(12)])
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    assert_chatwidget_snapshot!("slash_changelog_release_notes_preview", preview);
    assert!(
        rendered.contains("Codewith Changelog"),
        "expected changelog title, got {rendered:?}"
    );
    assert!(
        rendered.contains("Unreleased"),
        "expected unreleased section, got {rendered:?}"
    );
    assert!(
        !rendered.contains("Known evidence gaps"),
        "expected repository notes to be omitted, got {rendered:?}"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_clear_requests_ui_clear_when_idle() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Clear);

    assert_matches!(rx.try_recv(), Ok(AppEvent::ClearUi));
}

#[tokio::test]
async fn slash_clear_after_ctrl_c_keeps_stashed_draft_recallable() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.bottom_pane
        .set_history_metadata(thread_id, /*log_id*/ 1, /*entry_count*/ 0);

    submit_composer_text(&mut chat, "ok");
    assert_eq!(next_add_to_history_event(&mut rx), "ok");

    let stashed_draft = "explain why history recall lost this draft";

    chat.bottom_pane
        .set_composer_text(stashed_draft.to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "");
    assert_eq!(next_add_to_history_event(&mut rx), stashed_draft);

    chat.bottom_pane
        .set_composer_text("/clear".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Ok(AppEvent::ClearUi));
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), stashed_draft);

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "ok");
}

#[tokio::test]
async fn slash_clear_is_disabled_while_task_running() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.dispatch_command(SlashCommand::Clear);

    let event = rx.try_recv().expect("expected disabled command error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains("'/clear' is disabled while a task is in progress."),
                "expected /clear task-running error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected no follow-up events");
}

#[tokio::test]
async fn slash_tmux_is_disabled_while_task_running() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.dispatch_command(SlashCommand::Tmux);

    let event = rx.try_recv().expect("expected disabled command error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains("'/tmux' is disabled while a task is in progress."),
                "expected /tmux task-running error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected no follow-up events");
}

#[tokio::test]
async fn slash_archive_is_disabled_while_task_running() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.dispatch_command(SlashCommand::Archive);

    let event = rx.try_recv().expect("expected disabled command error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains("'/archive' is disabled while a task is in progress."),
                "expected /archive task-running error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected no follow-up events");
}

#[tokio::test]
async fn slash_mcp_opens_control_center() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Mcp);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("View all MCPs"));
    assert!(popup.contains("Add new MCP"));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_ps_opens_background_terminal_manager() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.unified_exec_processes.push(UnifiedExecProcessSummary {
        key: "proc-1".to_string(),
        call_id: "call-1".to_string(),
        command_display: "sleep 60".to_string(),
        recent_chunks: Vec::new(),
    });

    chat.dispatch_command(SlashCommand::Ps);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Background Terminals"),
        "got popup:\n{popup}"
    );
    assert!(popup.contains("Stop all"), "got popup:\n{popup}");
    assert!(popup.contains("Print snapshot"), "got popup:\n{popup}");
    assert!(
        rx.try_recv().is_err(),
        "expected no app event before selecting an action"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcps_alias_opens_control_center() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.bottom_pane
        .set_composer_text("/mcps".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("View all MCPs"), "got popup:\n{popup}");
    assert!(popup.contains("Add new MCP"), "got popup:\n{popup}");
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_list_requests_history_inventory_via_app_server() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    submit_composer_text(&mut chat, "/mcp list");

    assert!(active_blob(&chat).contains("Loading MCP inventory"));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::FetchMcpInventory {
            detail: McpServerStatusDetail::ToolsAndAuthOnly,
            thread_id: Some(actual_thread_id),
            target: McpInventoryTarget::History,
        }) if actual_thread_id == thread_id
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_reload_requests_mcp_reload() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/mcp reload");

    assert_matches!(rx.try_recv(), Ok(AppEvent::ReloadMcpServers));
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_add_opens_add_flow() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/mcp add");

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenMcpAddServer));
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_add_spec_requests_config_write() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(
        &mut chat,
        "/mcp add docs npx -y @scope/docs --env-var DOCS_KEY",
    );

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::AddMcpServer { spec }) if spec == "docs npx -y @scope/docs --env-var DOCS_KEY"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_invalid_args_show_usage() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/mcp full");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /mcp [manager|list|list verbose|reload|add [name spec...]]"),
        "expected usage message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/mcp full");
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_memories_opens_memory_menu() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ true);

    chat.dispatch_command(SlashCommand::Memories);

    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Use memories"));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_resume_opens_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Resume);

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenResumePicker));
}

#[tokio::test]
async fn slash_tmux_requests_default_handoff() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Tmux);

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenInTmux { destination, replace_existing })
            if destination == TmuxHandoffDestination::default() && replace_existing
    );
}

#[tokio::test]
async fn slash_archive_confirmation_requests_current_thread_archive() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Archive);

    assert!(chat.bottom_pane.has_active_view());
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_archive_confirmation_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::ArchiveCurrentThread));
}

#[tokio::test]
async fn slash_resume_with_arg_requests_named_session() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.bottom_pane.set_composer_text(
        "/resume my-saved-thread".to_string(),
        Vec::new(),
        Vec::new(),
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::ResumeSessionByIdOrName(id_or_name)) if id_or_name == "my-saved-thread"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn slash_tmux_with_args_requests_named_handoff() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.bottom_pane.set_composer_text(
        "/tmux --no-replace \"named session\"".to_string(),
        Vec::new(),
        Vec::new(),
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenInTmux { destination, replace_existing })
            if destination == (TmuxHandoffDestination::NewSession {
                name: Some("named session".to_string()),
            })
                && !replace_existing
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn slash_tmux_with_target_session_requests_window_handoff() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.bottom_pane.set_composer_text(
        "/tmux --session dev --window codewith".to_string(),
        Vec::new(),
        Vec::new(),
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenInTmux { destination, replace_existing })
            if destination == (TmuxHandoffDestination::ExistingSession {
                session_name: "dev".to_string(),
                window_name: Some("codewith".to_string()),
            }) && replace_existing
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_opens_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_pet_image_support(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(chat.bottom_pane.has_active_view());
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_pets_picker", popup);
}

#[tokio::test]
#[serial]
async fn slash_pets_with_arg_selects_named_pet() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_pet_image_support(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pets chefito".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::PetSelected { pet_id }) if pet_id == "chefito"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_disable_disables_pets_even_on_unsupported_terminal() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pets disable".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::PetDisabled));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pet_hide_disables_pets_even_on_unsupported_terminal() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pet hide".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::PetDisabled));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_on_unsupported_terminal_warns_without_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(!chat.bottom_pane.has_active_view());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets are disabled in tmux."));
    assert!(rendered.contains("outside tmux"));
}

#[tokio::test]
#[serial]
async fn slash_pets_with_arg_on_unsupported_terminal_warns_without_selection() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pets chefito".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets are disabled in tmux."));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_on_unsupported_terminal_shows_terminal_warning() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_terminal_pet_image_unsupported(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(!chat.bottom_pane.has_active_view());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets aren’t available in this terminal."));
    assert!(rendered.contains("Kitty graphics or Sixel support"));
}

#[tokio::test]
#[serial]
async fn slash_pets_on_old_iterm2_shows_upgrade_warning() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_old_iterm2_pet_image_unsupported(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(!chat.bottom_pane.has_active_view());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets require iTerm2 3.6 or newer."));
    assert!(rendered.contains("Upgrade iTerm2 to use terminal pets."));
}

#[tokio::test]
async fn slash_fork_requests_current_fork() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Fork);

    assert_matches!(rx.try_recv(), Ok(AppEvent::ForkCurrentSession));
}

#[tokio::test]
async fn slash_fork_with_thread_but_no_rollout_shows_starting_error() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.dispatch_command(SlashCommand::Fork);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected fork startup error");
    assert_chatwidget_snapshot!(
        "slash_fork_with_thread_but_no_rollout_shows_starting_error",
        lines_to_single_string(&cells[0])
    );
    assert!(rx.try_recv().is_err(), "fork should not call app-server");
}

#[tokio::test]
async fn slash_app_requests_desktop_handoff() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    chat.dispatch_command(SlashCommand::App);

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenDesktopThread {
            thread_id: actual_thread_id,
        }) if actual_thread_id == thread_id
    );
}

#[tokio::test]
async fn slash_app_without_thread_id_shows_starting_error() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::App);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected app startup error");
    assert_chatwidget_snapshot!(
        "slash_app_without_thread_id_shows_starting_error",
        lines_to_single_string(&cells[0])
    );
}

#[tokio::test]
async fn slash_rollout_displays_current_path() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let rollout_path = PathBuf::from("/tmp/codex-test-rollout.jsonl");
    chat.current_rollout_path = Some(rollout_path.clone());

    chat.dispatch_command(SlashCommand::Rollout);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected info message for rollout path");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(&rollout_path.display().to_string()),
        "expected rollout path to be shown: {rendered}"
    );
}

#[tokio::test]
async fn slash_rollout_handles_missing_path() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Rollout);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "expected info message explaining missing path"
    );
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("not available"),
        "expected missing rollout path message: {rendered}"
    );
}

#[tokio::test]
async fn fast_slash_command_updates_and_persists_local_service_tier() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode override app event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier),
            }
            if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode persistence app event; events: {events:?}"
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn fast_keybinding_toggle_uses_same_events_as_fast_slash_command() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.toggle_fast_mode_from_ui();

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode override app event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier),
            }
            if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode persistence app event; events: {events:?}"
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn fast_slash_on_arg_enables_even_when_fast_is_already_selected() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());
    let _events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

    chat.bottom_pane
        .set_composer_text("/fast on".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected /fast on to keep fast-mode override; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier),
            }
            if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected /fast on to persist fast-mode selection; events: {events:?}"
    );
    assert_eq!(
        chat.current_service_tier(),
        Some(ServiceTier::Fast.request_value())
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn fast_slash_off_arg_selects_default_service_tier() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());
    let _events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

    chat.bottom_pane
        .set_composer_text("/fast off".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE
        )),
        "expected /fast off to send default service tier override; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier)
            } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE
        )),
        "expected /fast off to persist default service tier; events: {events:?}"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn fast_keybinding_toggle_requires_feature_and_idle_surface() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ false);

    assert!(!chat.can_toggle_fast_mode_from_keybinding());

    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);
    assert!(chat.can_toggle_fast_mode_from_keybinding());

    chat.bottom_pane.set_task_running(/*running*/ true);
    assert!(!chat.can_toggle_fast_mode_from_keybinding());
}

#[tokio::test]
async fn user_turn_carries_service_tier_after_fast_toggle() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());

    let _events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

    chat.bottom_pane
        .set_composer_text("hello".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == ServiceTier::Fast.request_value() => {}
        other => panic!("expected Op::UserTurn with fast service tier, got {other:?}"),
    }
}

#[tokio::test]
async fn model_switch_recomputes_catalog_default_service_tier() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    let mut models = chat.model_catalog.try_list_models().expect("test catalog");
    let default_model = models
        .iter_mut()
        .find(|model| model.model == "gpt-5.4")
        .expect("gpt-5.4 test model");
    default_model.default_service_tier = Some(ServiceTier::Fast.request_value().to_string());
    chat.model_catalog = std::sync::Arc::new(ModelCatalog::new(models));
    chat.refresh_effective_service_tier();

    assert_eq!(chat.current_service_tier(), None);

    chat.set_model("gpt-5.4");
    assert_eq!(
        chat.current_service_tier(),
        Some(ServiceTier::Fast.request_value())
    );

    chat.set_model("gpt-5.3-codex");
    assert_eq!(chat.current_service_tier(), None);

    chat.bottom_pane
        .set_composer_text("hello".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE => {}
        other => panic!("expected Op::UserTurn with default service tier override, got {other:?}"),
    }
}

#[tokio::test]
async fn queued_fast_slash_applies_before_next_queued_message() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/fast");
    queue_composer_text_with_tab(&mut chat, "hello after fast");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected queued /fast to update service tier before next turn; events: {events:?}"
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            items,
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == ServiceTier::Fast.request_value() => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after fast".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message to submit with fast tier, got {other:?}"),
    }
}

#[tokio::test]
async fn queued_slash_edits_and_moves_local_user_queue() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.queue_user_message(UserMessage::from("first queued"));
    chat.queue_user_message(UserMessage::from("second queued"));

    chat.dispatch_command_with_args(
        SlashCommand::Queued,
        "edit 2 changed queued".to_string(),
        Vec::new(),
    );

    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["first queued", "changed queued"]
    );

    chat.dispatch_command_with_args(SlashCommand::Queued, "up 2".to_string(), Vec::new());

    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["changed queued", "first queued"]
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn queued_slash_edits_and_moves_retry_first_queue() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.input_queue
        .rejected_steers_queue
        .push_back(UserMessage::from("first retry"));
    chat.input_queue
        .rejected_steers_queue
        .push_back(UserMessage::from("second retry"));

    chat.dispatch_command_with_args(
        SlashCommand::Queued,
        "edit retry:2 changed retry".to_string(),
        Vec::new(),
    );

    assert_eq!(
        chat.input_queue
            .rejected_steers_queue
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["first retry", "changed retry"]
    );

    chat.dispatch_command_with_args(SlashCommand::Queued, "up retry:2".to_string(), Vec::new());

    assert_eq!(
        chat.input_queue
            .rejected_steers_queue
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["changed retry", "first retry"]
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    chat.show_queued_messages(/*agent_messages*/ None);
    let rendered = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::InsertHistoryCell(cell) => {
                Some(lines_to_single_string(&cell.display_lines(/*width*/ 120)))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Retry-first queue"),
        "expected retry-first queue in /queued output, got {rendered:?}"
    );
    assert!(
        rendered.contains("retry:1."),
        "expected retry target in /queued output, got {rendered:?}"
    );
}

#[tokio::test]
async fn queued_slash_dispatches_agent_queue_updates() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    chat.dispatch_command_with_args(
        SlashCommand::Queued,
        "edit agent:msg-1 revised agent message".to_string(),
        Vec::new(),
    );
    chat.dispatch_command_with_args(
        SlashCommand::Queued,
        "down agent:msg-1".to_string(),
        Vec::new(),
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::UpdateQueuedThreadMessage {
                thread_id: actual_thread_id,
                message_id,
                text,
            } if *actual_thread_id == thread_id
                && message_id == "msg-1"
                && text == "revised agent message"
        )),
        "expected queued agent update event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::MoveQueuedThreadMessage {
                thread_id: actual_thread_id,
                message_id,
                direction,
            } if *actual_thread_id == thread_id
                && message_id == "msg-1"
                && *direction == ThreadQueuedMessageMoveDirection::Down
        )),
        "expected queued agent move event; events: {events:?}"
    );
}

#[tokio::test]
async fn user_turn_sends_standard_override_after_fast_is_turned_off() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());
    let _events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

    chat.handle_service_tier_command_dispatch(fast_tier_command());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE
        )),
        "expected fast-mode off default service tier app event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier)
            } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE
        )),
        "expected default service tier persistence app event; events: {events:?}"
    );

    chat.bottom_pane
        .set_composer_text("hello".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE => {}
        other => panic!("expected Op::UserTurn with default service tier override, got {other:?}"),
    }
}

#[tokio::test]
async fn raw_slash_command_toggles_and_accepts_on_off_args() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Raw);
    assert!(chat.raw_output_mode());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::RawOutputModeChanged { enabled: true }))
    );

    chat.dispatch_command_with_args(SlashCommand::Raw, "off".to_string(), Vec::new());
    assert!(!chat.raw_output_mode());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::RawOutputModeChanged { enabled: false }))
    );

    chat.dispatch_command_with_args(SlashCommand::Raw, "on".to_string(), Vec::new());
    assert!(chat.raw_output_mode());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::RawOutputModeChanged { enabled: true }))
    );
}

#[tokio::test]
async fn raw_slash_command_reports_usage_for_invalid_arg() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Raw, "status".to_string(), Vec::new());

    assert!(!chat.raw_output_mode());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /raw [on|off]"),
        "expected raw usage error, got {rendered:?}"
    );
}

#[tokio::test]
async fn compact_queues_user_messages_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    chat.submit_user_message(UserMessage::from(
        "Steer submitted while /compact was running.".to_string(),
    ));
    handle_error(
        &mut chat,
        "cannot steer a compact turn",
        Some(CodexErrorInfo::ActiveTurnNotSteerable {
            turn_kind: NonSteerableTurnKind::Compact,
        }),
    );

    let width: u16 = 80;
    let height: u16 = 18;
    let backend = VT100Backend::new(width, height);
    let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
    let desired_height = chat.desired_height(width).min(height);
    term.set_viewport_area(Rect::new(0, height - desired_height, width, desired_height));
    term.draw(|f| {
        chat.render(f.area(), f.buffer_mut());
    })
    .unwrap();
    assert_chatwidget_snapshot!(
        "compact_queues_user_messages_snapshot",
        normalize_snapshot_paths(term.backend().vt100().screen().contents())
    );
}
