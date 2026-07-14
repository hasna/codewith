use super::*;
use crate::app_event::RateLimitRefreshOrigin;
use crate::app_event::RateLimitRefreshTarget;
use crate::app_event::RateLimitResetAttempt;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::chatwidget::RateLimitResetCompletion;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditResponse;
use codex_app_server_protocol::RateLimitResetCredit;
use codex_app_server_protocol::RateLimitResetCreditStatus;
use codex_app_server_protocol::RateLimitResetCreditsSummary;
use codex_app_server_protocol::RateLimitResetType;
use codex_app_server_protocol::RateLimitWindow;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn usage_panel_shows_banked_reset_action() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.update_account_state(
        Some(StatusAccountDisplay::ChatGpt {
            email: Some("dev@example.com".to_string()),
            plan: Some("Pro".to_string()),
        }),
        /*plan_type*/ None,
        /*has_chatgpt_account*/ true,
    );
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_usage_panel();
    let request_id = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::UsagePanel { request_id },
                target: RateLimitRefreshTarget::Selected,
            } => Some(request_id),
            _ => None,
        })
        .expect("usage refresh");
    chat.finish_usage_panel_rate_limit_refresh(request_id, Ok(()));

    assert_chatwidget_snapshot!(
        "usage_panel_banked_reset",
        normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 100)),
    );
}

#[tokio::test]
async fn reset_picker_defaults_to_cancel() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_rate_limit_reset_confirm();

    assert_chatwidget_snapshot!(
        "usage_limit_reset_picker_default_cancel",
        normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 90)),
    );
}

#[tokio::test]
async fn idle_snapshots_never_consume_a_reset() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));

    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(
            event,
            AppEvent::ConsumeRateLimitResetCredit { .. }
        ));
    }
}

#[tokio::test]
async fn failed_turn_requests_one_correlated_fresh_reset_check() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));

    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Duplicate weekly usage limit signal.".to_string(),
    );

    let refreshes = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::RefreshRateLimits { origin, target } => Some((origin, target)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        refreshes,
        vec![(
            RateLimitRefreshOrigin::AutoResetCheck { generation: 1 },
            RateLimitRefreshTarget::Selected,
        )]
    );
}

#[tokio::test]
async fn external_provider_usage_limit_never_requests_or_consumes_a_codex_reset() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.model_provider_id = "openrouter".to_string();
    chat.config.model_provider =
        codex_model_provider_info::ModelProviderInfo::create_openrouter_provider();
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));

    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "External provider usage limit reached.".to_string(),
    );

    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(
                event,
                AppEvent::RefreshRateLimits {
                    origin: RateLimitRefreshOrigin::AutoResetCheck { .. },
                    ..
                } | AppEvent::ConsumeRateLimitResetCredit { .. }
            ),
            "an external provider must never enter Codex bank-reset recovery: {event:?}"
        );
    }
    assert!(!chat.automatic_usage_limit_reset_owns_failed_turn());
}

#[tokio::test(start_paused = true)]
async fn duplicate_limit_signal_waits_for_reset_and_resumes_failed_turn_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));
    let attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("queued exact reset consumption");
    chat.config.usage_self_heal.initial_backoff_secs = 1;
    chat.config.usage_self_heal.max_backoff_secs = 1;

    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Duplicate weekly usage limit signal.".to_string(),
    );
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;

    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, AppEvent::UsageSelfHealRetry { .. }),
            "ordinary self-heal must not race an active reset"
        );
    }
    assert!(
        op_rx.try_recv().is_err(),
        "the failed turn must remain parked while reset is active"
    );

    assert!(chat.start_rate_limit_reset_consumption(&attempt));
    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));
    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(1, Ok(()));

    assert!(matches!(next_submit_op(&mut op_rx), Op::UserTurn { .. }));
    assert!(
        op_rx.try_recv().is_err(),
        "reset verification must resume the failed turn exactly once"
    );
}

#[tokio::test(start_paused = true)]
async fn manual_reset_picker_never_owns_a_failed_automatic_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    configure_usage_limit_self_heal(&mut chat);
    chat.start_rate_limit_reset_picker();
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
            event,
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::ResetPicker { generation: 0 },
                target: RateLimitRefreshTarget::Selected,
            }
        ))
    );

    submit_failed_weekly_turn(&mut chat, &mut op_rx, "recover after manual picker");

    assert_no_automatic_reset_action(&mut rx);
    assert_one_self_heal_retry(&mut chat, &mut rx, &mut op_rx).await;
}

#[tokio::test(start_paused = true)]
async fn manual_reset_consumption_never_owns_a_failed_automatic_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    configure_usage_limit_self_heal(&mut chat);
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    attempt.automatic = false;
    attempt.trigger_key = None;
    assert!(chat.start_rate_limit_reset_consumption(&attempt));

    submit_failed_weekly_turn(&mut chat, &mut op_rx, "recover during manual reset");

    assert_no_automatic_reset_action(&mut rx);
    assert_one_self_heal_retry(&mut chat, &mut rx, &mut op_rx).await;
}

#[tokio::test(start_paused = true)]
async fn manual_reset_acceptance_preserves_the_failed_turn_fallback() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    configure_usage_limit_self_heal(&mut chat);
    let attempt = start_manual_reset_consumption(&mut chat);
    submit_failed_weekly_turn(&mut chat, &mut op_rx, "recover after manual acceptance");

    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));
    assert_one_self_heal_retry(&mut chat, &mut rx, &mut op_rx).await;
}

#[tokio::test(start_paused = true)]
async fn manual_reset_retry_preserves_the_failed_turn_fallback() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    configure_usage_limit_self_heal(&mut chat);
    let attempt = start_manual_reset_consumption(&mut chat);
    submit_failed_weekly_turn(&mut chat, &mut op_rx, "recover after manual retry");
    let RateLimitResetCompletion::Retry(retry) =
        chat.finish_rate_limit_reset_consumption(attempt, Err("connection closed".to_string()))
    else {
        panic!("manual reset should retry an ambiguous result once");
    };

    assert!(chat.start_rate_limit_reset_consumption(&retry));
    assert_one_self_heal_retry(&mut chat, &mut rx, &mut op_rx).await;
}

#[tokio::test(start_paused = true)]
async fn manual_reset_verification_preserves_the_failed_turn_fallback() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    configure_usage_limit_self_heal(&mut chat);
    let attempt = start_manual_reset_consumption(&mut chat);
    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));
    submit_failed_weekly_turn(&mut chat, &mut op_rx, "recover after manual verification");

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(0, Ok(()));
    assert_one_self_heal_retry(&mut chat, &mut rx, &mut op_rx).await;
}

#[tokio::test]
async fn manual_reset_states_do_not_suppress_profile_fallback_for_failed_turn() {
    #[derive(Clone, Copy)]
    enum ManualResetState {
        Picker,
        Consumption,
        Retry,
        Verification,
        Confirmation,
    }

    for state in [
        ManualResetState::Picker,
        ManualResetState::Consumption,
        ManualResetState::Retry,
        ManualResetState::Verification,
        ManualResetState::Confirmation,
    ] {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        super::status_and_layout::save_test_auth_profile(&chat, "work");
        super::status_and_layout::save_test_auth_profile(&chat, "personal");
        chat.config.selected_auth_profile = Some("work".to_string());
        chat.config.auth_profile_auto_switch.enabled = true;
        chat.config.auth_profile_auto_switch.profiles =
            vec!["work".to_string(), "personal".to_string()];
        chat.config.usage_self_heal.enabled = false;
        super::status_and_layout::configure_test_session(&mut chat);
        while rx.try_recv().is_ok() {}

        chat.submit_user_message(UserMessage::from("recover on another profile"));
        assert_user_turn_text(next_submit_op(&mut op_rx), "recover on another profile");
        chat.on_task_started();

        let mut attempt = reset_attempt();
        attempt.auth_profile = chat.config.selected_auth_profile.clone();
        attempt.automatic = false;
        attempt.trigger_key = None;
        match state {
            ManualResetState::Picker => {
                chat.pending_rate_limit_reset_picker = Some(chat.rate_limit_reset_generation);
            }
            ManualResetState::Consumption => {
                assert!(chat.start_rate_limit_reset_consumption(&attempt));
            }
            ManualResetState::Retry => {
                chat.rate_limit_reset_retry = Some(attempt);
            }
            ManualResetState::Verification => {
                chat.pending_post_reset_refresh = Some(attempt);
            }
            ManualResetState::Confirmation => {
                chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
                chat.open_rate_limit_reset_confirm();
            }
        }

        chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .all(|event| !matches!(event, AppEvent::SwitchAuthProfile { .. })),
            "manual reset refresh must retain precedence before a turn actually fails"
        );
        chat.on_rate_limit_error(
            RateLimitErrorKind::UsageLimit,
            "Weekly usage limit reached.".to_string(),
        );

        let switches = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                AppEvent::SwitchAuthProfile {
                    profile,
                    reason,
                    resume_queued_input,
                    ..
                } => Some((profile, reason, resume_queued_input)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            switches,
            vec![(
                Some("personal".to_string()),
                crate::app_event::AuthProfileSwitchReason::AutoRateLimit {
                    window: "weekly".to_string(),
                },
                true,
            )]
        );
        assert_eq!(chat.pending_usage_self_heal_retry_id(), None);
        assert_eq!(
            chat.queued_user_message_texts(),
            vec!["recover on another profile"]
        );
        assert!(op_rx.try_recv().is_err());
    }
}

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
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));
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
    chat.finish_post_reset_refresh(1, Ok(()));
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
    chat.set_usage_limit_auto_reset_enabled(false);

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(1, Ok(()));
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

    chat.finish_post_reset_refresh(1, Err("verification failed".to_string()));
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

#[tokio::test]
async fn disabled_auto_reset_never_requests_a_reset_check() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = false;
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );

    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(
            event,
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::AutoResetCheck { .. },
                ..
            }
        ));
    }
}

#[tokio::test]
async fn auto_reset_refresh_error_hands_failed_turn_to_self_heal_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);

    chat.finish_usage_limit_auto_reset_check(1, Err("refresh failed".to_string()));
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("failed auto-reset refresh should hand off to self-heal");
    chat.finish_usage_limit_auto_reset_check(1, Err("duplicate refresh failure".to_string()));

    assert_eq!(chat.pending_usage_self_heal_retry_id(), Some(retry_id));
}

#[tokio::test]
async fn repeated_same_window_auto_reset_hands_failed_turn_to_self_heal_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.usage_limit_auto_reset_key = Some(format!(
        "{:?}:codex:weekly:{:?}",
        chat.config.selected_auth_profile,
        Some(123)
    ));

    chat.finish_usage_limit_auto_reset_check(1, Ok(()));
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("same-window attempt should hand off to self-heal");
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));

    assert_eq!(chat.pending_usage_self_heal_retry_id(), Some(retry_id));
    assert_no_reset_consumption(&mut rx);
}

#[tokio::test]
async fn post_reset_refresh_error_hands_failed_turn_to_self_heal_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);

    chat.finish_post_reset_refresh(1, Err("verification failed".to_string()));
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("failed verification should hand off to self-heal");
    chat.finish_post_reset_refresh(1, Err("duplicate verification failure".to_string()));

    assert_eq!(chat.pending_usage_self_heal_retry_id(), Some(retry_id));
}

#[tokio::test]
async fn still_exhausted_post_reset_hands_failed_turn_to_self_heal_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);

    chat.finish_post_reset_refresh(1, Ok(()));
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("still-exhausted verification should hand off to self-heal");
    chat.finish_post_reset_refresh(1, Ok(()));

    assert_eq!(chat.pending_usage_self_heal_retry_id(), Some(retry_id));
}

#[tokio::test]
async fn disabling_auto_reset_during_refresh_prevents_consumption() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));

    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));

    assert!(!chat.config.usage_limit.auto_reset_enabled);
    assert_no_reset_consumption(&mut rx);
}

#[tokio::test]
async fn disabling_auto_reset_during_in_flight_consume_reconciles_without_resuming() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    let attempt = start_automatic_reset_consumption(&mut chat, &mut rx);

    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );
    let RateLimitResetCompletion::Verify(reconcile) = chat.finish_rate_limit_reset_consumption(
        attempt.clone(),
        Ok(ConsumeAccountRateLimitResetCreditResponse {
            outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
        }),
    ) else {
        panic!("an already-sent reset must still be reconciled after opt-out");
    };
    assert_eq!(reconcile.idempotency_key, attempt.idempotency_key);

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(reconcile.generation, Ok(()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Duplicate signal after opted-out reconciliation.".to_string(),
    );

    assert!(op_rx.try_recv().is_err());
    assert_eq!(chat.pending_usage_self_heal_retry_id(), None);
    assert_no_automatic_reset_action(&mut rx);
}

#[tokio::test]
async fn disabling_auto_reset_during_ambiguous_in_flight_consume_verifies_without_retry() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    let attempt = start_automatic_reset_consumption(&mut chat, &mut rx);

    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );
    let RateLimitResetCompletion::Verify(reconcile) = chat
        .finish_rate_limit_reset_consumption(attempt.clone(), Err("connection closed".to_string()))
    else {
        panic!("an ambiguous already-sent reset must reconcile without a retry after opt-out");
    };

    assert_eq!(reconcile, attempt);
    assert_eq!(chat.rate_limit_reset_retry, None);
    assert_eq!(chat.pending_post_reset_refresh, Some(reconcile));
    assert!(op_rx.try_recv().is_err());
    assert_no_reset_consumption(&mut rx);
}

#[tokio::test]
async fn disabling_auto_reset_with_ambiguous_retry_queued_verifies_without_another_post() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    let attempt = start_automatic_reset_consumption(&mut chat, &mut rx);
    let RateLimitResetCompletion::Retry(retry) =
        chat.finish_rate_limit_reset_consumption(attempt, Err("connection closed".to_string()))
    else {
        panic!("ambiguous result should queue one same-key retry");
    };
    chat.app_event_tx
        .send(AppEvent::ConsumeRateLimitResetCredit {
            attempt: retry.clone(),
        });

    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );

    let mut queued_attempts = Vec::new();
    let mut verification_refreshes = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => queued_attempts.push(attempt),
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::PostReset { generation },
                target,
            } => verification_refreshes.push((generation, target)),
            _ => {}
        }
    }
    assert_eq!(queued_attempts, vec![retry.clone()]);
    assert!(
        !chat.start_rate_limit_reset_consumption(&queued_attempts[0]),
        "the queued retry must not dispatch another POST after opt-out"
    );
    assert_eq!(
        verification_refreshes,
        vec![(retry.generation, RateLimitRefreshTarget::Selected)]
    );
    assert_eq!(queued_attempts[0].idempotency_key, retry.idempotency_key);

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(retry.generation, Ok(()));
    assert!(op_rx.try_recv().is_err());
    assert_eq!(chat.pending_usage_self_heal_retry_id(), None);
}

#[tokio::test]
async fn disabling_auto_reset_during_post_reset_verification_never_resumes_after_reenable() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);

    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );
    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(true),
    );
    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(1, Ok(()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Duplicate signal after opted-out verification.".to_string(),
    );

    assert!(op_rx.try_recv().is_err());
    assert_eq!(chat.pending_usage_self_heal_retry_id(), None);
    assert_no_automatic_reset_action(&mut rx);
}

#[tokio::test]
async fn opted_out_post_reset_verification_error_never_falls_back_or_self_heals() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);
    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );

    chat.finish_post_reset_refresh(1, Err("verification failed".to_string()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Duplicate signal after opted-out verification error.".to_string(),
    );

    assert!(op_rx.try_recv().is_err());
    assert_eq!(chat.pending_usage_self_heal_retry_id(), None);
    assert_eq!(chat.pending_auth_profile_auto_switch_trigger, None);
    assert_no_automatic_reset_action(&mut rx);
}

#[tokio::test]
async fn opted_out_still_exhausted_verification_never_falls_back_or_self_heals() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);
    chat.apply_config_popup_value(
        "usage_limit.auto_reset_enabled",
        &serde_json::Value::Bool(false),
    );

    chat.finish_post_reset_refresh(1, Ok(()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Duplicate signal after opted-out still-exhausted verification.".to_string(),
    );

    assert!(op_rx.try_recv().is_err());
    assert_eq!(chat.pending_usage_self_heal_retry_id(), None);
    assert_eq!(chat.pending_auth_profile_auto_switch_trigger, None);
    assert_no_automatic_reset_action(&mut rx);
}

#[tokio::test]
async fn workspace_owner_limit_uses_fresh_exact_automatic_reset_credit() {
    assert_workspace_limit_uses_exact_reset(RateLimitReachedType::WorkspaceOwnerCreditsDepleted)
        .await;
}

#[tokio::test]
async fn workspace_member_limit_uses_fresh_exact_automatic_reset_credit() {
    assert_workspace_limit_uses_exact_reset(RateLimitReachedType::WorkspaceMemberCreditsDepleted)
        .await;
}

#[tokio::test]
async fn zero_available_count_with_available_detail_never_consumes() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    let mut summary = exact_reset_summary();
    summary.available_count = 0;
    chat.on_rate_limit_reset_credits(Some(summary));

    chat.finish_usage_limit_auto_reset_check(1, Ok(()));

    assert_no_reset_consumption(&mut rx);
    assert!(
        chat.pending_usage_self_heal_retry_id().is_some(),
        "inconsistent zero-count details must fall back"
    );
}

#[tokio::test]
async fn reset_attempts_are_profile_and_generation_correlated() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.selected_auth_profile = Some("work".to_string());
    let mut attempt = reset_attempt();
    attempt.auth_profile = Some("personal".to_string());
    queue_automatic_reset_attempt(&mut chat, &attempt);
    assert!(!chat.start_rate_limit_reset_consumption(&attempt));

    attempt.auth_profile = Some("work".to_string());
    attempt.generation = 1;
    queue_automatic_reset_attempt(&mut chat, &attempt);
    assert!(!chat.start_rate_limit_reset_consumption(&attempt));

    attempt.generation = 0;
    queue_automatic_reset_attempt(&mut chat, &attempt);
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
}

#[tokio::test]
async fn automatic_reset_rechecks_toggle_immediately_before_consumption() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    queue_automatic_reset_attempt(&mut chat, &attempt);

    assert!(!chat.start_rate_limit_reset_consumption(&attempt));
    assert_eq!(chat.rate_limit_reset_generation, 1);
}

#[tokio::test]
async fn manual_reset_remains_available_when_automatic_reset_is_disabled() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    attempt.automatic = false;
    attempt.trigger_key = None;

    assert!(chat.start_rate_limit_reset_consumption(&attempt));
}

#[tokio::test]
async fn ambiguous_retry_reuses_exact_request_key_and_credit() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    queue_automatic_reset_attempt(&mut chat, &attempt);
    assert!(chat.start_rate_limit_reset_consumption(&attempt));

    let RateLimitResetCompletion::Retry(retry) = chat
        .finish_rate_limit_reset_consumption(attempt.clone(), Err("connection closed".to_string()))
    else {
        panic!("ambiguous result should retry once");
    };
    assert_eq!(retry.idempotency_key, attempt.idempotency_key);
    assert_eq!(retry.credit_id, attempt.credit_id);
    assert_eq!(retry.generation, attempt.generation);
    assert_eq!(retry.retry_count, 1);
}

#[tokio::test]
async fn second_ambiguous_result_reconciles_without_another_request_key() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    queue_automatic_reset_attempt(&mut chat, &attempt);
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
    let RateLimitResetCompletion::Retry(retry) =
        chat.finish_rate_limit_reset_consumption(attempt, Err("connection closed".to_string()))
    else {
        panic!("first ambiguous result should retry once");
    };
    assert!(chat.start_rate_limit_reset_consumption(&retry));

    let RateLimitResetCompletion::Verify(reconcile) =
        chat.finish_rate_limit_reset_consumption(retry.clone(), Err("timed out again".to_string()))
    else {
        panic!("second ambiguous result should reconcile with a fresh limits read");
    };

    assert_eq!(reconcile, retry);
    assert_eq!(
        chat.usage_limit_auto_reset_key, retry.trigger_key,
        "an ambiguous automatic spend must block a new request key for the same window"
    );
}

#[tokio::test]
async fn verified_reset_resumes_failed_turn_exactly_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    super::status_and_layout::configure_test_session(&mut chat);
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.auth_profile_auto_switch.enabled = false;

    chat.submit_user_message(UserMessage::from("resume me once"));
    assert!(matches!(next_submit_op(&mut op_rx), Op::UserTurn { .. }));
    chat.on_task_started();
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
            event,
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::AutoResetCheck { generation: 1 },
                target: RateLimitRefreshTarget::Selected,
            }
        ))
    );

    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));
    let attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("exact reset consumption");
    assert_eq!(attempt.credit_id, "credit-earliest");
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(1, Ok(()));
    assert!(matches!(next_submit_op(&mut op_rx), Op::UserTurn { .. }));
    chat.finish_post_reset_refresh(1, Ok(()));
    assert!(
        op_rx.try_recv().is_err(),
        "failed turn must resume only once"
    );
}

fn exact_reset_summary() -> RateLimitResetCreditsSummary {
    RateLimitResetCreditsSummary {
        available_count: 1,
        credits: Some(vec![RateLimitResetCredit {
            id: "credit-earliest".to_string(),
            reset_type: RateLimitResetType::CodexRateLimits,
            status: RateLimitResetCreditStatus::Available,
            granted_at: 1,
            expires_at: Some(i64::MAX),
            title: Some("Banked reset".to_string()),
            description: Some("Earned reset credit.".to_string()),
        }]),
    }
}

fn exhausted_weekly_snapshot() -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: Some("codex".to_string()),
        limit_name: Some("Codex".to_string()),
        primary: Some(RateLimitWindow {
            used_percent: 100,
            window_duration_mins: Some(7 * 24 * 60),
            resets_at: Some(123),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        plan_type: None,
        rate_limit_reached_type: None,
    }
}

fn non_exhausted_weekly_snapshot() -> RateLimitSnapshot {
    let mut snapshot = exhausted_weekly_snapshot();
    snapshot.primary.as_mut().expect("primary").used_percent = 0;
    snapshot
}

fn reset_attempt() -> RateLimitResetAttempt {
    RateLimitResetAttempt {
        idempotency_key: "stable-request-key".to_string(),
        credit_id: "exact-credit".to_string(),
        auth_profile: None,
        generation: 0,
        automatic: true,
        trigger_key: Some("profile:codex:weekly:123".to_string()),
        retry_count: 0,
    }
}

fn start_manual_reset_consumption(chat: &mut ChatWidget) -> RateLimitResetAttempt {
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    attempt.automatic = false;
    attempt.trigger_key = None;
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
    attempt
}

fn configure_usage_limit_self_heal(chat: &mut ChatWidget) {
    super::status_and_layout::configure_test_session(chat);
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.auth_profile_auto_switch.enabled = false;
    chat.config.usage_self_heal.enabled = true;
    chat.config.usage_self_heal.max_retries = 2;
    chat.config.usage_self_heal.initial_backoff_secs = 1;
    chat.config.usage_self_heal.max_backoff_secs = 1;
}

fn submit_failed_weekly_turn(
    chat: &mut ChatWidget,
    op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>,
    text: &str,
) {
    chat.submit_user_message(UserMessage::from(text));
    assert_user_turn_text(next_submit_op(op_rx), text);
    chat.on_task_started();
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );
}

async fn assert_one_self_heal_retry(
    chat: &mut ChatWidget,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>,
) {
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("manual reset state must hand the failed turn to self-heal");
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;

    let retries = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::UsageSelfHealRetry { retry_id } => Some(retry_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retries, vec![retry_id]);
    assert!(chat.on_usage_self_heal_retry(retry_id));
    assert!(matches!(next_submit_op(op_rx), Op::UserTurn { .. }));
    assert!(
        op_rx.try_recv().is_err(),
        "failed turn must retry only once"
    );
}

fn finish_automatic_reset_and_assert_turn_order(
    chat: &mut ChatWidget,
    op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>,
    failed_turn: &str,
    queued_turn: &str,
) {
    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(1, Ok(()));
    assert_user_turn_text(next_submit_op(op_rx), failed_turn);
    assert!(
        op_rx.try_recv().is_err(),
        "only the failed turn resumes first"
    );

    chat.on_task_started();
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert_user_turn_text(next_submit_op(op_rx), queued_turn);
    assert!(
        op_rx.try_recv().is_err(),
        "queued turn submits exactly once"
    );

    chat.on_task_started();
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert!(op_rx.try_recv().is_err(), "no turn may be duplicated");
}

fn assert_user_turn_text(op: Op, expected: &str) {
    let Op::UserTurn { items, .. } = op else {
        panic!("expected user turn");
    };
    assert_eq!(
        items,
        vec![UserInput::Text {
            text: expected.to_string(),
            text_elements: Vec::new(),
        }]
    );
}

fn next_user_turn_or_shell_op(op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>) -> Op {
    loop {
        match op_rx.try_recv() {
            Ok(op @ (Op::UserTurn { .. } | Op::RunUserShellCommand { .. })) => return op,
            Ok(_) => continue,
            Err(error) => panic!("expected queued user turn, got {error:?}"),
        }
    }
}

fn queue_automatic_reset_attempt(chat: &mut ChatWidget, attempt: &RateLimitResetAttempt) {
    chat.pending_rate_limit_reset_consumption = Some(attempt.clone());
}

fn start_auto_reset_failed_turn(
    chat: &mut ChatWidget,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>,
    reached_type: Option<RateLimitReachedType>,
) {
    super::status_and_layout::configure_test_session(chat);
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.auth_profile_auto_switch.enabled = false;
    chat.config.usage_self_heal.enabled = true;
    chat.config.usage_self_heal.max_retries = 2;

    chat.submit_user_message(UserMessage::from("recover this failed turn"));
    assert!(matches!(next_submit_op(op_rx), Op::UserTurn { .. }));
    chat.on_task_started();
    let mut snapshot = exhausted_weekly_snapshot();
    snapshot.rate_limit_reached_type = reached_type;
    chat.on_rate_limit_snapshot(Some(snapshot));
    chat.on_rate_limit_error(
        RateLimitErrorKind::UsageLimit,
        "Weekly usage limit reached.".to_string(),
    );
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
            event,
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::AutoResetCheck { generation: 1 },
                target: RateLimitRefreshTarget::Selected,
            }
        ))
    );
}

fn accept_automatic_reset(
    chat: &mut ChatWidget,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) {
    let attempt = start_automatic_reset_consumption(chat, rx);
    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));
}

fn start_automatic_reset_consumption(
    chat: &mut ChatWidget,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> RateLimitResetAttempt {
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));
    let attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("exact reset consumption");
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
    attempt
}

async fn assert_workspace_limit_uses_exact_reset(reached_type: RateLimitReachedType) {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, Some(reached_type));
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.finish_usage_limit_auto_reset_check(1, Ok(()));

    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
            event,
            AppEvent::ConsumeRateLimitResetCredit { attempt }
                if attempt.automatic && attempt.credit_id == "credit-earliest"
        ))
    );
}

fn assert_no_reset_consumption(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(
            event,
            AppEvent::ConsumeRateLimitResetCredit { .. }
        ));
    }
}

fn assert_no_automatic_reset_action(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(
                event,
                AppEvent::ConsumeRateLimitResetCredit { .. }
                    | AppEvent::RefreshRateLimits {
                        origin: RateLimitRefreshOrigin::AutoResetCheck { .. },
                        ..
                    }
            ),
            "opt-out must remain latched for duplicate signals from the failed turn"
        );
    }
}
