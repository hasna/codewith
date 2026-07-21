use super::*;
use pretty_assertions::assert_eq;

fn drain_history_text(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> String {
    drain_insert_history(rx)
        .into_iter()
        .flatten()
        .map(|line| lines_to_single_string(&[line]))
        .collect::<Vec<_>>()
        .join("\n")
}

fn redeemed_summary(credit_id: &str) -> RateLimitResetCreditsSummary {
    RateLimitResetCreditsSummary {
        available_count: 0,
        credits: Some(vec![RateLimitResetCredit {
            id: credit_id.to_string(),
            reset_type: RateLimitResetType::CodexRateLimits,
            status: RateLimitResetCreditStatus::Redeemed,
            granted_at: 1,
            expires_at: Some(i64::MAX),
            title: None,
            description: None,
        }]),
    }
}

fn start_terminally_ambiguous_manual_attempt(chat: &mut ChatWidget) -> RateLimitResetAttempt {
    let attempt = start_manual_reset_consumption(chat);
    let RateLimitResetCompletion::Retry(retry) =
        chat.finish_rate_limit_reset_consumption(attempt, Err("first timeout".to_string()))
    else {
        panic!("first ambiguous response must retry");
    };
    assert!(chat.start_rate_limit_reset_consumption(&retry));
    let RateLimitResetCompletion::Verify(reconcile) =
        chat.finish_rate_limit_reset_consumption(retry, Err("second timeout".to_string()))
    else {
        panic!("second ambiguous response must reconcile");
    };
    reconcile
}

#[tokio::test]
async fn manual_reset_ownership_advances_and_retains_the_profile_action_generation() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let pre_reset_generation = chat.rate_limit_reset_generation;

    chat.start_rate_limit_reset_picker();
    let owned_generation = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::ResetPicker { generation },
                target: RateLimitRefreshTarget::Selected,
            } => Some(generation),
            _ => None,
        })
        .expect("manual reset refresh");

    assert_ne!(owned_generation, pre_reset_generation);
    assert!(!chat.is_rate_limit_reset_generation_current(pre_reset_generation));
    chat.finish_rate_limit_reset_picker(owned_generation, Err("offline".to_string()));
    assert!(chat.is_rate_limit_reset_generation_current(owned_generation));

    chat.start_rate_limit_reset_picker();
    let no_credit_generation = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::ResetPicker { generation },
                target: RateLimitRefreshTarget::Selected,
            } => Some(generation),
            _ => None,
        })
        .expect("manual reset refresh without credits");
    chat.on_rate_limit_reset_credits(Some(RateLimitResetCreditsSummary {
        available_count: 0,
        credits: Some(Vec::new()),
    }));
    chat.finish_rate_limit_reset_picker(no_credit_generation, Ok(()));
    assert!(!chat.manual_usage_limit_reset_is_active());
    assert!(chat.is_rate_limit_reset_generation_current(no_credit_generation));

    chat.start_rate_limit_reset_picker();
    let cancelled_generation = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::ResetPicker { generation },
                target: RateLimitRefreshTarget::Selected,
            } => Some(generation),
            _ => None,
        })
        .expect("manual reset refresh before cancel");
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.finish_rate_limit_reset_picker(cancelled_generation, Ok(()));
    assert!(chat.manual_usage_limit_reset_is_active());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert!(!chat.manual_usage_limit_reset_is_active());
    assert!(chat.is_rate_limit_reset_generation_current(cancelled_generation));
    assert!(!chat.is_rate_limit_reset_generation_current(no_credit_generation));

    let post_generation_attempt = start_manual_reset_consumption(&mut chat);
    assert_eq!(post_generation_attempt.generation, cancelled_generation);
    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            post_generation_attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
                account_identity_fingerprint: "sha256:test-account".to_string(),
            }),
        ),
        RateLimitResetCompletion::Verify(_)
    ));
    chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    chat.finish_post_reset_refresh(cancelled_generation, Ok(()));
    assert!(!chat.manual_usage_limit_reset_is_active());
    assert!(chat.is_rate_limit_reset_generation_current(cancelled_generation));
}

#[tokio::test]
async fn auto_reset_requires_canonical_provider_id_and_effective_endpoint() {
    let (mut spoofed, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    spoofed.config.usage_limit.auto_reset_enabled = true;
    spoofed.config.model_provider_id = "custom-openai".to_string();
    spoofed.config.model_provider =
        codex_model_provider_info::ModelProviderInfo::create_openai_provider(
            /*base_url*/ None,
        );
    spoofed.runtime_model_provider_base_url =
        Some(codex_model_provider_info::CHATGPT_CODEX_BASE_URL.to_string());
    assert_eq!(
        spoofed.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Unavailable
    );
    spoofed.start_rate_limit_reset_picker();
    assert!(spoofed.pending_rate_limit_reset_picker.is_none());

    let (mut overridden, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    overridden.config.usage_limit.auto_reset_enabled = true;
    set_canonical_reset_provider(&mut overridden);
    overridden.runtime_model_provider_base_url = Some("https://example.test/v1".to_string());
    assert_eq!(
        overridden.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Unavailable
    );
    overridden.start_rate_limit_reset_picker();
    assert!(overridden.pending_rate_limit_reset_picker.is_none());

    let (mut canonical, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    canonical.config.usage_limit.auto_reset_enabled = true;
    set_canonical_reset_provider(&mut canonical);
    assert_eq!(
        canonical.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Started
    );
}

#[tokio::test]
async fn built_in_openai_default_without_a_resolved_model_url_can_reset() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.usage_limit.auto_reset_enabled = true;
    chat.config.model_provider_id = codex_model_provider_info::OPENAI_PROVIDER_ID.to_string();
    chat.config.model_provider =
        codex_model_provider_info::ModelProviderInfo::create_openai_provider(
            /*base_url*/ None,
        );
    chat.runtime_model_provider_base_url = None;

    assert_eq!(
        chat.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Started
    );
}

#[tokio::test]
async fn explicit_model_or_chatgpt_backend_override_blocks_reset() {
    let (mut model_override, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    model_override.config.usage_limit.auto_reset_enabled = true;
    model_override.config.model_provider_id =
        codex_model_provider_info::OPENAI_PROVIDER_ID.to_string();
    model_override.config.model_provider =
        codex_model_provider_info::ModelProviderInfo::create_openai_provider(Some(
            "https://example.test/v1".to_string(),
        ));
    model_override.runtime_model_provider_base_url = None;
    assert_eq!(
        model_override.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Unavailable
    );

    let (mut backend_override, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    backend_override.config.usage_limit.auto_reset_enabled = true;
    set_canonical_reset_provider(&mut backend_override);
    backend_override.config.chatgpt_base_url = "https://example.test/backend-api".to_string();
    assert_eq!(
        backend_override.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Unavailable
    );
}

#[tokio::test]
async fn dismissed_manual_selection_cannot_spend_but_a_new_selection_can() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_rate_limit_reset_confirm();
    chat.handle_key_event(KeyEvent::from(KeyCode::Up));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let stale_attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("queued manual reset selection");

    chat.handle_key_event(KeyEvent::from(KeyCode::Esc));
    assert!(!chat.start_rate_limit_reset_consumption(&stale_attempt));

    chat.start_rate_limit_reset_picker();
    let generation = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::ResetPicker { generation },
                ..
            } => Some(generation),
            _ => None,
        })
        .expect("new manual reset refresh");
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.finish_rate_limit_reset_picker(generation, Ok(()));
    chat.handle_key_event(KeyEvent::from(KeyCode::Up));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let current_attempt = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("new manual reset selection");
    assert!(chat.start_rate_limit_reset_consumption(&current_attempt));
}

#[tokio::test]
async fn provider_or_endpoint_change_before_spend_fails_closed() {
    let (mut manual, mut manual_rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    manual.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    manual.open_rate_limit_reset_confirm();
    manual.handle_key_event(KeyEvent::from(KeyCode::Up));
    manual.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let manual_attempt = std::iter::from_fn(|| manual_rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("manual reset selection");
    manual.config.model_provider_id = "custom-openai".to_string();
    assert!(!manual.start_rate_limit_reset_consumption(&manual_attempt));

    let (mut automatic, mut automatic_rx, _op_rx) =
        make_chatwidget_manual(/*model_override*/ None).await;
    automatic.config.usage_limit.auto_reset_enabled = true;
    assert_eq!(
        automatic.request_usage_limit_auto_reset_check(),
        UsageLimitAutoResetCheckOutcome::Started
    );
    let generation = std::iter::from_fn(|| automatic_rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::AutoResetCheck { generation },
                ..
            } => Some(generation),
            _ => None,
        })
        .expect("automatic reset refresh");
    automatic.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    automatic.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    automatic.finish_usage_limit_auto_reset_check(generation, Ok(()));
    let automatic_attempt = std::iter::from_fn(|| automatic_rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::ConsumeRateLimitResetCredit { attempt } => Some(attempt),
            _ => None,
        })
        .expect("automatic reset selection");
    automatic.runtime_model_provider_base_url = Some("https://example.test/v1".to_string());
    assert!(!automatic.start_rate_limit_reset_consumption(&automatic_attempt));
}

#[tokio::test]
async fn selecting_a_profile_with_relogin_in_flight_blocks_reset() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.selected_auth_profile = Some("personal".to_string());
    chat.begin_selected_auth_profile_credential_mutation("work");
    chat.config.selected_auth_profile = Some("work".to_string());

    assert!(chat.selected_auth_profile_credential_mutation_in_flight());
    chat.start_rate_limit_reset_picker();
    assert!(chat.pending_rate_limit_reset_picker.is_none());
}

#[tokio::test]
async fn overlapping_relogins_remain_blocking_until_every_attempt_finishes() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.selected_auth_profile = Some("work".to_string());

    chat.begin_selected_auth_profile_credential_mutation("work");
    chat.begin_selected_auth_profile_credential_mutation("work");
    chat.finish_selected_auth_profile_credential_mutation(
        "work", /*credentials_changed*/ false,
    );
    assert!(chat.selected_auth_profile_credential_mutation_in_flight());

    chat.finish_selected_auth_profile_credential_mutation(
        "work", /*credentials_changed*/ false,
    );
    assert!(!chat.selected_auth_profile_credential_mutation_in_flight());
}

#[tokio::test]
async fn stale_cancel_cannot_dismiss_a_newer_manual_reset_selection() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.rate_limit_reset_generation = 2;
    chat.on_rate_limit_account_identity(Some("sha256:account-b".to_string()));
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_rate_limit_reset_confirm();
    assert!(chat.manual_usage_limit_reset_is_active());

    chat.cancel_manual_rate_limit_reset_selection(/*generation*/ 1);
    assert!(chat.manual_usage_limit_reset_is_active());
    assert_eq!(
        chat.manual_rate_limit_reset_authority
            .as_ref()
            .map(|authority| authority.generation),
        Some(2)
    );

    chat.cancel_manual_rate_limit_reset_selection(/*generation*/ 2);
    assert!(!chat.manual_usage_limit_reset_is_active());
}

#[tokio::test]
async fn ambiguous_manual_reset_requires_exact_credit_redemption_proof() {
    for summary in [
        RateLimitResetCreditsSummary {
            available_count: 1,
            credits: None,
        },
        exact_reset_summary(),
        RateLimitResetCreditsSummary {
            available_count: 2,
            credits: Some(vec![RateLimitResetCredit {
                id: "different-credit".to_string(),
                reset_type: RateLimitResetType::CodexRateLimits,
                status: RateLimitResetCreditStatus::Redeemed,
                granted_at: 1,
                expires_at: None,
                title: None,
                description: None,
            }]),
        },
    ] {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        let reconcile = start_terminally_ambiguous_manual_attempt(&mut chat);
        while rx.try_recv().is_ok() {}
        chat.on_rate_limit_reset_credits(Some(summary));
        chat.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
        chat.finish_post_reset_refresh(reconcile.generation, Ok(()));

        let history = drain_history_text(&mut rx);
        assert!(
            history.contains("Couldn't confirm exact usage limit reset redemption"),
            "count-only, still-available, and capped details must remain unconfirmed: {history}"
        );
        assert!(!history.contains("Usage limit reset verified."));
    }
}

#[tokio::test]
async fn exact_redeemed_credit_or_explicit_outcome_verifies_manual_reset() {
    let (mut ambiguous, mut ambiguous_rx, _op_rx) =
        make_chatwidget_manual(/*model_override*/ None).await;
    let reconcile = start_terminally_ambiguous_manual_attempt(&mut ambiguous);
    while ambiguous_rx.try_recv().is_ok() {}
    ambiguous.on_rate_limit_reset_credits(Some(redeemed_summary(&reconcile.credit_id)));
    ambiguous.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    ambiguous.finish_post_reset_refresh(reconcile.generation, Ok(()));
    assert!(drain_history_text(&mut ambiguous_rx).contains("Usage limit reset verified."));

    let (mut explicit, mut explicit_rx, _op_rx) =
        make_chatwidget_manual(/*model_override*/ None).await;
    let attempt = start_manual_reset_consumption(&mut explicit);
    let RateLimitResetCompletion::Verify(verify) = explicit.finish_rate_limit_reset_consumption(
        attempt,
        Ok(ConsumeAccountRateLimitResetCreditResponse {
            outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
            account_identity_fingerprint: "sha256:test-account".to_string(),
        }),
    ) else {
        panic!("explicit reset must verify");
    };
    while explicit_rx.try_recv().is_ok() {}
    explicit.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    explicit.on_rate_limit_snapshot(Some(non_exhausted_weekly_snapshot()));
    explicit.finish_post_reset_refresh(verify.generation, Ok(()));
    assert!(drain_history_text(&mut explicit_rx).contains("Usage limit reset verified."));
}

#[tokio::test]
async fn reset_response_for_a_different_account_fails_closed() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let attempt = start_manual_reset_consumption(&mut chat);

    assert!(matches!(
        chat.finish_rate_limit_reset_consumption(
            attempt,
            Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::Reset,
                account_identity_fingerprint: "sha256:different-account".to_string(),
            }),
        ),
        RateLimitResetCompletion::Ignore
    ));
    assert!(chat.pending_post_reset_refresh.is_none());
    assert!(
        drain_history_text(&mut rx)
            .contains("Usage limit reset stopped because the authenticated account changed.")
    );
}

/// Regression: a background rate-limit refresh whose authoritative reset-credit
/// ledger fetch failed (so the app-server fell back to the coarse inline usage
/// count of 0) must not overwrite a banked reset count the panel already
/// surfaced. Otherwise `/usage` flips back to "no usage limit resets available"
/// while banked resets are still redeemable and the "You have N usage limit
/// resets available" banner still stands.
#[tokio::test]
async fn coarse_zero_refresh_does_not_clobber_banked_reset_count() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    // Authoritative, detail-backed ledger reports one banked reset.
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    assert_eq!(
        chat.rate_limit_reset_credits
            .as_ref()
            .map(|summary| summary.available_count),
        Some(1)
    );

    // A coarse fallback (detail rows absent) reporting zero must not downgrade
    // the count the user already saw.
    chat.on_rate_limit_reset_credits(Some(RateLimitResetCreditsSummary {
        available_count: 0,
        credits: None,
    }));

    assert_eq!(
        chat.rate_limit_reset_credits
            .as_ref()
            .map(|summary| summary.available_count),
        Some(1),
        "a coarse fallback 0 must not clobber a known banked reset count"
    );
}

/// The guard distrusts only *coarse* zeros: an authoritative, detail-backed
/// summary reporting zero (e.g. right after the last reset is consumed) must
/// still clear the count so `/usage` stops advertising a spent reset.
#[tokio::test]
async fn authoritative_zero_clears_banked_reset_count() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.on_rate_limit_reset_credits(Some(RateLimitResetCreditsSummary {
        available_count: 0,
        credits: Some(Vec::new()),
    }));

    assert_eq!(
        chat.rate_limit_reset_credits
            .as_ref()
            .map(|summary| summary.available_count),
        Some(0),
        "an authoritative detail-backed 0 must clear the reset count"
    );
}

/// A coarse fallback is only distrusted when it would *downgrade* to zero; a
/// coarse count that raises the available total is still applied so newly
/// banked resets surface promptly even if the ledger detail fetch lagged.
#[tokio::test]
async fn coarse_refresh_may_raise_banked_reset_count() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.on_rate_limit_reset_credits(Some(RateLimitResetCreditsSummary {
        available_count: 3,
        credits: None,
    }));

    assert_eq!(
        chat.rate_limit_reset_credits
            .as_ref()
            .map(|summary| summary.available_count),
        Some(3),
        "a coarse fallback that raises the count is still trusted"
    );
}
