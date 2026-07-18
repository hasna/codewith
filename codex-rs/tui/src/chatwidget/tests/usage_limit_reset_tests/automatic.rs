use super::*;
use pretty_assertions::assert_eq;

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

    chat.finish_usage_limit_auto_reset_check(
        /*generation*/ 1,
        Err("refresh failed".to_string()),
    );
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("failed auto-reset refresh should hand off to self-heal");
    chat.finish_usage_limit_auto_reset_check(
        /*generation*/ 1,
        Err("duplicate refresh failure".to_string()),
    );

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

    chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("same-window attempt should hand off to self-heal");
    chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));

    assert_eq!(chat.pending_usage_self_heal_retry_id(), Some(retry_id));
    assert_no_reset_consumption(&mut rx);
}

#[tokio::test]
async fn post_reset_refresh_error_hands_failed_turn_to_self_heal_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);

    chat.finish_post_reset_refresh(
        /*generation*/ 1,
        Err("verification failed".to_string()),
    );
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("failed verification should hand off to self-heal");
    chat.finish_post_reset_refresh(
        /*generation*/ 1,
        Err("duplicate verification failure".to_string()),
    );

    assert_eq!(chat.pending_usage_self_heal_retry_id(), Some(retry_id));
}

#[tokio::test]
async fn still_exhausted_post_reset_hands_failed_turn_to_self_heal_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
    accept_automatic_reset(&mut chat, &mut rx);

    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
    let retry_id = chat
        .pending_usage_self_heal_retry_id()
        .expect("still-exhausted verification should hand off to self-heal");
    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));

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
    chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));

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
            account_identity_fingerprint: "sha256:test-account".to_string(),
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
    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
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

    chat.finish_post_reset_refresh(
        /*generation*/ 1,
        Err("verification failed".to_string()),
    );
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

    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
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

    chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));

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
    queue_automatic_reset_attempt(&mut chat, &mut attempt);
    assert!(!chat.start_rate_limit_reset_consumption(&attempt));

    attempt.auth_profile = Some("work".to_string());
    attempt.generation = 1;
    queue_automatic_reset_attempt(&mut chat, &mut attempt);
    assert!(!chat.start_rate_limit_reset_consumption(&attempt));

    attempt.generation = 0;
    queue_automatic_reset_attempt(&mut chat, &mut attempt);
    assert!(chat.start_rate_limit_reset_consumption(&attempt));
}

#[tokio::test]
async fn automatic_reset_rechecks_toggle_immediately_before_consumption() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    queue_automatic_reset_attempt(&mut chat, &mut attempt);

    assert!(!chat.start_rate_limit_reset_consumption(&attempt));
    assert_eq!(chat.rate_limit_reset_generation, 1);
}

#[tokio::test]
async fn automatic_reset_rechecks_exact_weekly_exhaustion_immediately_before_consumption() {
    for used_percent in [99, 101] {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
        chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
        chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));
        let attempt = std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|event| match event {
                AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
                _ => None,
            })
            .expect("queued automatic reset");

        let mut snapshot = exhausted_weekly_snapshot();
        snapshot.primary.as_mut().expect("primary").used_percent = used_percent;
        chat.on_rate_limit_snapshot(Some(snapshot));

        assert!(
            !chat.start_rate_limit_reset_consumption(&attempt),
            "{used_percent}% must not satisfy the exact weekly exhaustion boundary"
        );
    }
}

#[tokio::test]
async fn automatic_reset_rechecks_selected_credit_immediately_before_consumption() {
    for replacement in [
        RateLimitResetCreditsSummary {
            available_count: 1,
            credits: Some(vec![RateLimitResetCredit {
                id: "different-credit".to_string(),
                ..exact_reset_summary()
                    .credits
                    .and_then(|credits| credits.into_iter().next())
                    .expect("exact credit")
            }]),
        },
        RateLimitResetCreditsSummary {
            available_count: 0,
            credits: Some(vec![RateLimitResetCredit {
                status: RateLimitResetCreditStatus::Redeemed,
                ..exact_reset_summary()
                    .credits
                    .and_then(|credits| credits.into_iter().next())
                    .expect("exact credit")
            }]),
        },
        RateLimitResetCreditsSummary {
            available_count: 1,
            credits: Some(vec![RateLimitResetCredit {
                expires_at: Some(0),
                ..exact_reset_summary()
                    .credits
                    .and_then(|credits| credits.into_iter().next())
                    .expect("exact credit")
            }]),
        },
    ] {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        start_auto_reset_failed_turn(&mut chat, &mut rx, &mut op_rx, /*reached_type*/ None);
        chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
        chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));
        let attempt = std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|event| match event {
                AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
                _ => None,
            })
            .expect("queued automatic reset");

        chat.on_rate_limit_reset_credits(Some(replacement));

        assert!(
            !chat.start_rate_limit_reset_consumption(&attempt),
            "a removed, redeemed, or expired queued credit must not be consumed"
        );
    }
}

#[tokio::test]
async fn manual_reset_remains_available_when_automatic_reset_is_disabled() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _attempt = start_manual_reset_consumption(&mut chat);
}

#[tokio::test]
async fn ambiguous_retry_reuses_exact_request_key_and_credit() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    queue_automatic_reset_attempt(&mut chat, &mut attempt);
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
    queue_automatic_reset_attempt(&mut chat, &mut attempt);
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
    set_canonical_reset_provider(&mut chat);
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
    chat.finish_usage_limit_auto_reset_check(/*generation*/ 1, Ok(()));
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
                account_identity_fingerprint: "sha256:test-account".to_string(),
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));

    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
    assert!(matches!(next_submit_op(&mut op_rx), Op::UserTurn { .. }));
    chat.finish_post_reset_refresh(/*generation*/ 1, Ok(()));
    assert!(
        op_rx.try_recv().is_err(),
        "failed turn must resume only once"
    );
}
