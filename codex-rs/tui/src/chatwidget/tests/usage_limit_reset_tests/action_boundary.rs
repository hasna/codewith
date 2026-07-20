use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn dynamic_service_tier_commands_are_blocked_during_automatic_reset() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    let fast = ServiceTierCommand {
        id: ServiceTier::Fast.request_value().to_string(),
        name: "fast".to_string(),
        description: "Fastest inference with increased plan usage".to_string(),
    };
    let configured_before = chat.configured_service_tier();

    chat.handle_service_tier_command_dispatch(fast.clone());
    chat.handle_service_tier_command_with_args_dispatch(fast, "on".to_string());

    assert_eq!(chat.configured_service_tier(), configured_before);
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(
                event,
                AppEvent::CodexOp(Op::OverrideTurnContext { .. })
                    | AppEvent::PersistServiceTierSelection { .. }
            ),
            "dynamic service-tier mutation escaped automatic reset ownership: {event:?}"
        );
    }
    assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn thread_and_orchestration_commands_are_blocked_during_automatic_reset() {
    let blocked = [
        SlashCommand::Session,
        SlashCommand::Goal,
        SlashCommand::MissionControl,
        SlashCommand::Workflow,
    ];

    for command in blocked {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
        chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
        chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
        chat.submit_user_message(UserMessage::from("queued behind automatic recovery"));
        let queued_before = chat.queued_user_message_texts();

        chat.dispatch_command(command);

        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(
                    event,
                    AppEvent::OpenAgentPicker
                        | AppEvent::OpenThreadGoalMenu { .. }
                        | AppEvent::OpenMissionControlOverview
                        | AppEvent::ManageThreadWorkflow { .. }
                ),
                "/{} emitted a mutation event during automatic recovery: {event:?}",
                command.command()
            );
        }
        assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
        assert_eq!(chat.queued_user_message_texts(), queued_before);
        assert!(op_rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn inline_goal_and_workflow_mutations_are_blocked_during_automatic_reset() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    chat.set_feature_enabled(Feature::Workflows, /*enabled*/ true);
    chat.submit_user_message(UserMessage::from("queued behind automatic recovery"));
    let queued_before = chat.queued_user_message_texts();

    chat.dispatch_command_with_args(SlashCommand::Goal, "clear".to_string(), Vec::new());
    chat.dispatch_command_with_args(
        SlashCommand::Workflow,
        "run cancel run-1".to_string(),
        Vec::new(),
    );

    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(
                event,
                AppEvent::ClearThreadGoal { .. }
                    | AppEvent::SetThreadGoalStatus { .. }
                    | AppEvent::SetThreadGoalObjective { .. }
                    | AppEvent::ManageThreadWorkflow { .. }
            ),
            "inline goal/workflow command emitted a mutation event: {event:?}"
        );
    }
    assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
    assert_eq!(chat.queued_user_message_texts(), queued_before);
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn status_remains_available_during_automatic_reset() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.submit_user_message(UserMessage::from("queued behind automatic recovery"));
    let queued_before = chat.queued_user_message_texts();

    chat.dispatch_command(SlashCommand::Status);

    assert!(!chat.bottom_pane.no_modal_or_popup_active());
    assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
    assert_eq!(chat.queued_user_message_texts(), queued_before);
    assert!(op_rx.try_recv().is_err());
}
