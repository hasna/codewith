use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(start_paused = true)]
async fn submission_during_automatic_reset_check_runs_after_failed_turn_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);

    chat.submit_user_message(UserMessage::from("queued during reset check"));
    assert!(
        op_rx.try_recv().is_err(),
        "a new turn must stay queued while the failed turn owns the reset"
    );

    accept_automatic_reset(&mut chat, &mut rx);
    finish_automatic_reset_and_assert_turn_order(
        &mut chat,
        &mut op_rx,
        "recover this failed turn",
        "queued during reset check",
    );
}

#[tokio::test(start_paused = true)]
async fn manual_reset_cannot_replace_a_pending_automatic_recovery() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.submit_user_message(UserMessage::from("queued behind automatic recovery"));
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));
    let automatic_attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("pending automatic reset consumption");
    let mut manual_attempt = reset_attempt();
    manual_attempt.auth_profile = chat.config.selected_auth_profile.clone();
    manual_attempt.automatic = false;
    manual_attempt.trigger_key = None;

    assert!(!chat.start_rate_limit_reset_consumption(&manual_attempt));
    assert!(chat.start_rate_limit_reset_consumption(&automatic_attempt));
    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            automatic_attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
                account_identity_fingerprint: "sha256:test-account".to_string(),
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));
    finish_automatic_reset_and_assert_turn_order(
        &mut chat,
        &mut op_rx,
        "recover this failed turn",
        "queued behind automatic recovery",
    );
}

#[tokio::test]
async fn manual_reset_picker_cannot_overlap_automatic_recovery() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));

    chat.start_rate_limit_reset_picker();
    chat.open_rate_limit_reset_confirm();

    assert_eq!(chat.pending_rate_limit_reset_picker, None);
    assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
    assert!(!render_bottom_popup(&chat, /*width*/ 90).contains("Usage limit resets"));
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(
                event,
                AppEvent::RefreshRateLimits {
                    origin: RateLimitRefreshOrigin::ResetPicker { .. },
                    ..
                }
            ),
            "manual reset picker must not refresh during automatic recovery"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn submission_during_automatic_reset_verification_runs_after_failed_turn_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);

    chat.submit_user_message(UserMessage::from("queued during reset verification"));
    assert!(
        op_rx.try_recv().is_err(),
        "a new turn must stay queued while reset verification owns the failed turn"
    );

    finish_automatic_reset_and_assert_turn_order(
        &mut chat,
        &mut op_rx,
        "recover this failed turn",
        "queued during reset verification",
    );
}

#[tokio::test(start_paused = true)]
async fn reset_queue_preserves_disallowed_shell_escape_policy() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);

    assert_eq!(
        chat.submit_user_message_as_plain_user_turn(UserMessage::from("!echo must stay text")),
        None
    );
    assert!(op_rx.try_recv().is_err());

    accept_automatic_reset(&mut chat, &mut rx);
    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
    assert_user_turn_text(next_submit_op(&mut op_rx), "recover this failed turn");
    chat.on_task_started();
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert_user_turn_text(
        next_user_turn_or_shell_op(&mut op_rx),
        "!echo must stay text",
    );
}

#[tokio::test(start_paused = true)]
async fn opted_out_automatic_reset_drains_the_queued_follow_up() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);
    chat.submit_user_message(UserMessage::from("continue after opt out"));
    chat.set_usage_limit_auto_reset_enabled(/*enabled*/ false);

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
    assert_user_turn_text(next_submit_op(&mut op_rx), "continue after opt out");
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn no_credit_without_a_fallback_drains_the_queued_follow_up() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    let attempt = start_automatic_reset_consumption(&mut chat, &mut rx);
    chat.submit_user_message(UserMessage::from("continue without a reset"));
    chat.config.usage_self_heal.enabled = false;

    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::NoCredit,
                account_identity_fingerprint: "sha256:test-account".to_string(),
            }),
        ),
        RateLimitResetCompletion::Ignore
    ));
    assert_user_turn_text(next_submit_op(&mut op_rx), "continue without a reset");
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn verification_failure_keeps_follow_up_behind_active_self_heal() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);
    chat.submit_user_message(UserMessage::from("continue after self heal"));

    chat.finish_post_reset_refresh(
        /*generation*/ 1,
        Err("verification failed".to_string()),
    );
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("failed reset verification should keep A ahead of B");
    assert!(op_rx.try_recv().is_err());
    assert!(chat.on_usage_self_heal_retry(retry_id));
    assert_user_turn_text(next_submit_op(&mut op_rx), "recover this failed turn");
    chat.on_task_started();
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert_user_turn_text(next_submit_op(&mut op_rx), "continue after self heal");
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn profile_slash_command_is_blocked_while_automatic_reset_owns_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.submit_user_message(UserMessage::from("queued behind automatic recovery"));
    let queued_before = chat.queued_user_message_texts();
    while rx.try_recv().is_ok() {}

    assert!(!render_bottom_popup(&chat, /*width*/ 90).contains("Select Profile"));
    chat.dispatch_command(SlashCommand::Profile);

    assert!(
        !render_bottom_popup(&chat, /*width*/ 90).contains("Select Profile"),
        "/profile must not expose an auth mutation path during automatic recovery"
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert_chatwidget_snapshot!(
        "usage_limit_reset_command_blocked",
        lines_to_single_string(&cells[0])
    );
    assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
    assert_eq!(chat.queued_user_message_texts(), queued_before);
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn preopened_profile_popup_is_invalidated_when_automatic_reset_takes_ownership() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    super::status_and_layout::save_test_auth_profile(&chat, "personal");
    super::status_and_layout::configure_test_session(&mut chat);
    set_canonical_reset_provider(&mut chat);
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.submit_user_message(UserMessage::from("recover this failed turn"));
    assert_user_turn_text(next_submit_op(&mut op_rx), "recover this failed turn");
    chat.on_task_started();
    chat.open_profile_popup();
    assert!(render_bottom_popup(&chat, /*width*/ 90).contains("Select Profile"));
    while rx.try_recv().is_ok() {}
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );

    assert!(!render_bottom_popup(&chat, /*width*/ 90).contains("Select Profile"));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, AppEvent::SwitchAuthProfile { .. }),
            "pre-opened /profile action escaped automatic reset ownership: {event:?}"
        );
    }
    assert!(chat.automatic_usage_limit_reset_owns_failed_turn());
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn preopened_profile_action_classes_are_generation_tagged_across_auto_completion() {
    let actions = [
        ("login", 0, KeyCode::Enter),
        ("relogin", 1, KeyCode::Char('l')),
        ("rename", 1, KeyCode::Char('r')),
        ("delete", 1, KeyCode::Char('d')),
        ("settings", 1, KeyCode::Char('s')),
        ("move-up", 1, KeyCode::Char('[')),
        ("move-down", 1, KeyCode::Char(']')),
    ];

    for (label, down_count, key_code) in actions {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        super::status_and_layout::save_test_auth_profile(&chat, "personal");
        super::status_and_layout::save_test_auth_profile(&chat, "work");
        super::status_and_layout::configure_test_session(&mut chat);
        set_canonical_reset_provider(&mut chat);
        chat.open_profile_popup();
        while rx.try_recv().is_ok() {}
        if label == "login" {
            chat.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
            assert_eq!(
                chat.bottom_pane
                    .selected_index_for_active_view("auth-profile-selection"),
                Some(3)
            );
        } else {
            for _ in 0..down_count {
                chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            }
        }
        chat.handle_key_event(KeyEvent::new(key_code, KeyModifiers::NONE));
        let event = rx
            .try_recv()
            .unwrap_or_else(|_| panic!("missing {label} action event"));
        let action_generation = match event {
            AppEvent::OpenAuthProfileLoginPrompt { reset_generation }
            | AppEvent::ReloginAuthProfile {
                reset_generation, ..
            }
            | AppEvent::OpenAuthProfileRenamePrompt {
                reset_generation, ..
            }
            | AppEvent::OpenAuthProfileDeleteConfirm {
                reset_generation, ..
            }
            | AppEvent::OpenAuthProfileSettings {
                reset_generation, ..
            }
            | AppEvent::MoveAuthProfile {
                reset_generation, ..
            } => reset_generation,
            other => panic!("unexpected {label} action event: {other:?}"),
        };

        chat.config.usage_limit.auto_reset_enabled = true;
        assert_eq!(
            chat.request_usage_limit_auto_reset_check(),
            UsageLimitAutoResetCheckOutcome::Started
        );
        let automatic_generation = std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|event| match event {
                AppEvent::RefreshRateLimits {
                    origin: RateLimitRefreshOrigin::AutoResetCheck { generation },
                    target: RateLimitRefreshTarget::Selected,
                } => Some(generation),
                _ => None,
            })
            .expect("automatic reset check");
        chat.finish_usage_limit_auto_reset_check(
            automatic_generation,
            Err("recovery completed without a reset".to_string()),
        );

        assert!(
            !chat.is_rate_limit_reset_generation_current(action_generation),
            "pre-opened {label} action must be stale after automatic recovery completes"
        );
        assert!(!chat.automatic_usage_limit_reset_owns_failed_turn());
    }
}

#[tokio::test]
async fn stale_manual_reset_confirmation_is_invalid_after_automatic_recovery() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.usage_self_heal.enabled = false;
    super::status_and_layout::configure_test_session(&mut chat);
    set_canonical_reset_provider(&mut chat);
    chat.submit_user_message(UserMessage::from("recover this failed turn"));
    assert_user_turn_text(next_submit_op(&mut op_rx), "recover this failed turn");
    chat.on_task_started();
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_rate_limit_reset_confirm();
    assert!(render_bottom_popup(&chat, /*width*/ 90).contains("Usage limit resets"));
    chat.handle_key_event(KeyEvent::from(KeyCode::Up));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let stale_attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } if !attempt.automatic => {
                Some(attempt)
            }
            _ => None,
        })
        .expect("manual reset confirmation action");
    assert!(render_bottom_popup(&chat, /*width*/ 90).contains("Usage limit resets"));
    assert!(
        chat.bottom_pane
            .dismiss_active_view_if_id("usage-limit-reset-confirmation")
    );

    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );
    assert!(!render_bottom_popup(&chat, /*width*/ 90).contains("Usage limit resets"));
    let generation = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::AutoResetCheck { generation },
                target: RateLimitRefreshTarget::Selected,
            } => Some(generation),
            _ => None,
        })
        .expect("automatic reset refresh");
    chat.finish_usage_limit_auto_reset_check(generation, Err("refresh failed".to_string()));

    assert!(!chat.automatic_usage_limit_reset_owns_failed_turn());
    assert!(!chat.start_rate_limit_reset_consumption(&stale_attempt));
}

#[tokio::test]
async fn manual_reset_selection_remains_reserved_until_consumption_starts() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_rate_limit_reset_confirm();
    chat.handle_key_event(KeyEvent::from(KeyCode::Up));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } if !attempt.automatic => {
                Some(attempt)
            }
            _ => None,
        })
        .expect("manual reset selection");

    assert!(
        chat.manual_usage_limit_reset_is_active(),
        "dismissing the picker before the queued consume is reserved leaves a profile-switch gap"
    );
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
    assert!(
        !render_bottom_popup(&chat, /*width*/ 90).contains("Usage limit resets"),
        "the picker should close only after the manual spend is reserved"
    );
}
