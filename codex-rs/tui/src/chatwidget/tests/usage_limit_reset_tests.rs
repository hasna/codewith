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
mod action_boundary;
mod automatic;
mod coordination;
mod manual;
mod profile_workflows;
mod safety_invariants;

fn set_canonical_reset_provider(chat: &mut ChatWidget) {
    chat.config.model_provider_id = codex_model_provider_info::OPENAI_PROVIDER_ID.to_string();
    chat.config.model_provider =
        codex_model_provider_info::ModelProviderInfo::create_openai_provider(
            /*base_url*/ None,
        );
    chat.runtime_model_provider_base_url =
        Some(codex_model_provider_info::CHATGPT_CODEX_BASE_URL.to_string());
    chat.has_chatgpt_account = true;
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
        credit_id: "credit-earliest".to_string(),
        auth_profile: None,
        account_identity_fingerprint: "sha256:test-account".to_string(),
        generation: 0,
        automatic: true,
        trigger_key: Some("profile:codex:weekly:123".to_string()),
        retry_count: 0,
        verification: RateLimitResetVerification::LimitsOnly,
    }
}

fn start_manual_reset_consumption(chat: &mut ChatWidget) -> RateLimitResetAttempt {
    set_canonical_reset_provider(chat);
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    chat.open_rate_limit_reset_confirm();
    let mut attempt = reset_attempt();
    attempt.auth_profile = chat.config.selected_auth_profile.clone();
    attempt.generation = chat.rate_limit_reset_generation;
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

fn queue_automatic_reset_attempt(chat: &mut ChatWidget, attempt: &mut RateLimitResetAttempt) {
    chat.on_rate_limit_snapshot(Some(exhausted_weekly_snapshot()));
    chat.on_rate_limit_reset_credits(Some(exact_reset_summary()));
    attempt.trigger_key = Some(format!(
        "{:?}:codex:weekly:{:?}",
        chat.config.selected_auth_profile,
        Some(123)
    ));
    chat.pending_rate_limit_reset_consumption = Some(attempt.clone());
}

fn start_auto_reset_failed_turn(
    chat: &mut ChatWidget,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>,
    reached_type: Option<RateLimitReachedType>,
) {
    super::status_and_layout::configure_test_session(chat);
    set_canonical_reset_provider(chat);
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
                account_identity_fingerprint: "sha256:test-account".to_string(),
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
