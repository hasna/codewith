use super::*;
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
    // Expire 3d 4h 30m out so the countdown renders a stable "Expires in 3d 4h."
    // (the trailing 30m keeps the day+hour bucket clear of test-run drift).
    chat.on_rate_limit_reset_credits(Some(reset_summary_expiring_in(
        3 * 86_400 + 4 * 3_600 + 30 * 60,
    )));
    chat.open_rate_limit_reset_confirm();

    assert_chatwidget_snapshot!(
        "usage_limit_reset_picker_default_cancel",
        normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 90)),
    );
}

#[tokio::test]
async fn config_usage_limit_reset_left_and_escape_return_one_level() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_canonical_reset_provider(&mut chat);
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    while rx.try_recv().is_ok() {}

    chat.open_config_popup();
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenConfigSection {
            section: crate::common_config_options::CommonConfigSection::AccountAutomation,
        })
    );
    chat.open_config_section_popup(
        crate::common_config_options::CommonConfigSection::AccountAutomation,
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenRateLimitResetConfirm));
    let generation = chat.rate_limit_reset_generation;
    chat.open_rate_limit_reset_confirm();

    chat.handle_key_event(KeyEvent::from(KeyCode::Left));
    let account_section = render_bottom_popup(&chat, /*width*/ 90);
    assert!(
        account_section.contains("Config: Account & automation"),
        "{account_section}"
    );
    assert!(!account_section.contains("Usage limit resets"));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::CancelRateLimitResetCreditSelection {
            generation: cancelled_generation,
        }) if cancelled_generation == generation
    );
    assert_matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenRateLimitResetConfirm));
    chat.open_rate_limit_reset_confirm();

    chat.handle_key_event(KeyEvent::from(KeyCode::Esc));
    let account_section = render_bottom_popup(&chat, /*width*/ 90);
    assert!(
        account_section.contains("Config: Account & automation"),
        "{account_section}"
    );
    assert!(!account_section.contains("Usage limit resets"));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::CancelRateLimitResetCreditSelection {
            generation: cancelled_generation,
        }) if cancelled_generation == generation
    );
    assert_matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Left));
    let root = render_bottom_popup(&chat, /*width*/ 90);
    assert!(
        root.contains("Choose a focused config.toml settings section."),
        "{root}"
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
                account_identity_fingerprint: "sha256:test-account".to_string(),
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
    set_canonical_reset_provider(&mut chat);
    chat.start_rate_limit_reset_picker();
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
            event,
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::ResetPicker { generation: 1 },
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
    let _attempt = start_manual_reset_consumption(&mut chat);

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
                account_identity_fingerprint: "sha256:test-account".to_string(),
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
                account_identity_fingerprint: "sha256:test-account".to_string(),
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
        set_canonical_reset_provider(&mut chat);
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
                let _attempt = start_manual_reset_consumption(&mut chat);
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
