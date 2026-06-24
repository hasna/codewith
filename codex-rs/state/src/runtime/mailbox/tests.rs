use super::*;
use crate::runtime::test_support::test_thread_metadata;
use crate::runtime::test_support::unique_temp_dir;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn enqueue_is_idempotent_per_target_thread() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = ThreadId::new();
    runtime
        .upsert_thread(&test_thread_metadata(
            runtime.codex_home(),
            thread_id,
            runtime.codex_home().join("workspace-two"),
        ))
        .await?;

    let first = enqueue_test_message(runtime.as_ref(), thread_id, "same-key", 3).await?;
    let second = enqueue_test_message(runtime.as_ref(), thread_id, "same-key", 3).await?;

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.message, second.message);

    Ok(())
}

#[tokio::test]
async fn claim_ack_and_receipts_are_durable() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let enqueued = enqueue_test_message(runtime.as_ref(), thread_id, "claim-key", 3).await?;
    let claim = claim_next(runtime.as_ref(), thread_id)
        .await?
        .expect("claimed");

    assert_eq!(claim.message.message_id, enqueued.message.message_id);
    assert_eq!(claim.message.status, crate::MailboxMessageStatus::Claimed);
    assert_eq!(claim.message.attempt_count, 1);

    let acked = runtime
        .mailbox_messages()
        .ack_message(MailboxAckParams {
            message_id: claim.message.message_id.clone(),
            attempt_id: claim.attempt.attempt_id.clone(),
            lease_id: claim.attempt.lease_id.clone(),
            receipt_payload_json: Some(serde_json::json!({ "handled": true })),
            now: Utc::now(),
        })
        .await?
        .expect("acked");

    assert_eq!(acked.status, crate::MailboxMessageStatus::Acknowledged);
    assert_eq!(acked.lease_id, None);

    let receipts = runtime
        .mailbox_messages()
        .list_receipts(acked.message_id.as_str())
        .await?;
    assert_eq!(
        receipts
            .into_iter()
            .map(|receipt| receipt.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::MailboxReceiptKind::Enqueued,
            crate::MailboxReceiptKind::Claimed,
            crate::MailboxReceiptKind::Acknowledged,
        ]
    );

    Ok(())
}

#[tokio::test]
async fn retry_after_attempt_budget_poisoned_message() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    enqueue_test_message(runtime.as_ref(), thread_id, "retry-key", 1).await?;
    let claim = claim_next(runtime.as_ref(), thread_id)
        .await?
        .expect("claimed");
    let failed = runtime
        .mailbox_messages()
        .fail_message(MailboxFailParams {
            message_id: claim.message.message_id.clone(),
            attempt_id: claim.attempt.attempt_id.clone(),
            lease_id: claim.attempt.lease_id.clone(),
            error: "delivery failed".to_string(),
            disposition: MailboxFailDisposition::Retry {
                next_attempt_at: Utc::now(),
            },
            now: Utc::now(),
        })
        .await?
        .expect("failed");

    assert_eq!(failed.status, crate::MailboxMessageStatus::Poisoned);
    assert_eq!(failed.last_error, Some("delivery failed".to_string()));

    Ok(())
}

#[tokio::test]
async fn expired_claim_can_be_reclaimed() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    enqueue_test_message(runtime.as_ref(), thread_id, "expire-key", 3).await?;
    let now = Utc::now();
    let first = runtime
        .mailbox_messages()
        .claim_next_message(MailboxClaimParams {
            target_thread_id: thread_id,
            lease_owner: "dispatcher-a".to_string(),
            lease_duration: std::time::Duration::from_millis(1),
            now,
        })
        .await?
        .expect("first claim");
    let second = runtime
        .mailbox_messages()
        .claim_next_message(MailboxClaimParams {
            target_thread_id: thread_id,
            lease_owner: "dispatcher-b".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now: now + chrono::Duration::seconds(1),
        })
        .await?
        .expect("second claim");

    assert_eq!(first.message.message_id, second.message.message_id);
    assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
    assert_eq!(second.message.attempt_count, 2);

    Ok(())
}

#[tokio::test]
async fn dispatch_claim_claims_due_messages_across_targets_once() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let low_priority_thread_id = test_thread_id();
    let high_priority_thread_id = ThreadId::new();
    runtime
        .upsert_thread(&test_thread_metadata(
            runtime.codex_home(),
            high_priority_thread_id,
            runtime.codex_home().join("workspace-high-priority"),
        ))
        .await?;

    let now = Utc::now();
    let low_priority = enqueue_test_message_with_options(
        runtime.as_ref(),
        low_priority_thread_id,
        "global-low-priority",
        3,
        0,
        Some(now),
    )
    .await?;
    let high_priority = enqueue_test_message_with_options(
        runtime.as_ref(),
        high_priority_thread_id,
        "global-high-priority",
        3,
        10,
        Some(now),
    )
    .await?;

    let first_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "global-dispatcher-a".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "local-owner".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("high priority global claim");
    assert_eq!(
        first_claim.message.message_id,
        high_priority.message.message_id
    );
    assert_eq!(
        first_claim.message.target_thread_id,
        high_priority_thread_id
    );

    let duplicate_claim = runtime
        .mailbox_messages()
        .claim_next_message(MailboxClaimParams {
            target_thread_id: high_priority_thread_id,
            lease_owner: "targeted-dispatcher".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
        })
        .await?;
    assert!(duplicate_claim.is_none());

    let second_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "global-dispatcher-b".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "local-owner".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("remaining low priority global claim");
    assert_eq!(
        second_claim.message.message_id,
        low_priority.message.message_id
    );
    assert_eq!(
        second_claim.message.target_thread_id,
        low_priority_thread_id
    );

    Ok(())
}

#[tokio::test]
async fn dispatch_claim_skips_due_message_fresh_owned_by_another_local_session()
-> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let local_thread_id = test_thread_id();
    let foreign_thread_id = ThreadId::new();
    runtime
        .upsert_thread(&test_thread_metadata(
            runtime.codex_home(),
            foreign_thread_id,
            runtime.codex_home().join("workspace-foreign"),
        ))
        .await?;

    let now = Utc::now();
    let local_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        local_thread_id,
        "local-owner-message",
        /*max_attempts*/ 3,
        /*priority*/ 0,
        Some(now),
    )
    .await?;
    let foreign_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        foreign_thread_id,
        "foreign-owner-message",
        /*max_attempts*/ 3,
        /*priority*/ 10,
        Some(now),
    )
    .await?;
    runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id: foreign_thread_id,
            owner_id: "foreign-owner".to_string(),
            session_id: "foreign-session".to_string(),
            pid: Some(123),
            now,
        })
        .await?;

    let local_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-local-owner".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "local-owner".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("local owner should claim non-foreign message");
    assert_eq!(
        local_claim.message.message_id,
        local_message.message.message_id
    );
    assert_eq!(local_claim.message.target_thread_id, local_thread_id);

    let foreign_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-foreign-owner".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "foreign-owner".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("foreign owner should claim its own fresh target");
    assert_eq!(
        foreign_claim.message.message_id,
        foreign_message.message.message_id
    );
    assert_eq!(foreign_claim.message.target_thread_id, foreign_thread_id);

    Ok(())
}

#[tokio::test]
async fn dispatch_claim_ignores_stale_foreign_local_session_owner() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let now = Utc::now();
    let stale_seen_at = now - chrono::Duration::seconds(30);
    let message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "stale-foreign-owner-message",
        /*max_attempts*/ 3,
        /*priority*/ 10,
        Some(now),
    )
    .await?;
    runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id: "foreign-owner".to_string(),
            session_id: "foreign-session".to_string(),
            pid: Some(123),
            now: stale_seen_at,
        })
        .await?;

    let claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-local-owner".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "local-owner".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("stale foreign owner should not block due claim");
    assert_eq!(claim.message.message_id, message.message.message_id);
    assert_eq!(claim.message.target_thread_id, thread_id);

    Ok(())
}

#[tokio::test]
async fn local_active_session_heartbeat_keeps_newer_owner() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let newer_seen_at = Utc::now();
    let older_seen_at = newer_seen_at - chrono::Duration::seconds(30);

    let newer = runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id: "newer-owner".to_string(),
            session_id: "newer-session".to_string(),
            pid: Some(200),
            now: newer_seen_at,
        })
        .await?;
    let attempted_regression = runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id: "older-owner".to_string(),
            session_id: "older-session".to_string(),
            pid: Some(100),
            now: older_seen_at,
        })
        .await?;
    let current = runtime
        .local_active_sessions()
        .get_session(thread_id)
        .await?;

    assert_eq!(attempted_regression, newer);
    assert_eq!(current, Some(newer));

    Ok(())
}

#[tokio::test]
async fn local_active_session_heartbeat_keeps_newer_same_owner_seen_at() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let newer_seen_at = Utc::now();
    let older_seen_at = newer_seen_at - chrono::Duration::seconds(30);

    let newer = runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id: "same-owner".to_string(),
            session_id: "newer-session".to_string(),
            pid: Some(200),
            now: newer_seen_at,
        })
        .await?;
    let attempted_regression = runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id: "same-owner".to_string(),
            session_id: "older-session".to_string(),
            pid: Some(100),
            now: older_seen_at,
        })
        .await?;
    let current = runtime
        .local_active_sessions()
        .get_session(thread_id)
        .await?;

    assert_eq!(attempted_regression, newer);
    assert_eq!(current, Some(newer));

    Ok(())
}

#[tokio::test]
async fn prune_owner_sessions_keeps_rows_newer_than_refresh_observation() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let stale_thread_id = test_thread_id();
    let fresh_thread_id = ThreadId::new();
    runtime
        .upsert_thread(&test_thread_metadata(
            runtime.codex_home(),
            fresh_thread_id,
            runtime.codex_home().join("workspace-fresh"),
        ))
        .await?;
    let observed_at = Utc::now();

    runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id: stale_thread_id,
            owner_id: "local-owner".to_string(),
            session_id: "stale-session".to_string(),
            pid: Some(100),
            now: observed_at - chrono::Duration::seconds(1),
        })
        .await?;
    let concurrent = runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id: fresh_thread_id,
            owner_id: "local-owner".to_string(),
            session_id: "fresh-session".to_string(),
            pid: Some(100),
            now: observed_at + chrono::Duration::seconds(1),
        })
        .await?;

    let pruned = runtime
        .local_active_sessions()
        .prune_owner_sessions(LocalActiveSessionPruneOwnerParams {
            owner_id: "local-owner".to_string(),
            active_thread_ids: Vec::new(),
            observed_at,
        })
        .await?;
    let stale = runtime
        .local_active_sessions()
        .get_session(stale_thread_id)
        .await?;
    let fresh = runtime
        .local_active_sessions()
        .get_session(fresh_thread_id)
        .await?;

    assert_eq!(pruned, 1);
    assert_eq!(stale, None);
    assert_eq!(fresh, Some(concurrent));

    Ok(())
}

#[tokio::test]
async fn dispatch_claim_serializes_competing_dispatchers_per_target() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let now = Utc::now();
    let first_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "target-lease-first",
        /*max_attempts*/ 3,
        /*priority*/ 10,
        Some(now),
    )
    .await?;
    let second_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "target-lease-second",
        /*max_attempts*/ 3,
        /*priority*/ 5,
        Some(now),
    )
    .await?;

    let first_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-first".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "owner-first".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("first dispatcher should claim first target message");
    assert_eq!(
        first_claim.message.message_id,
        first_message.message.message_id
    );

    let blocked_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-second".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "owner-second".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?;
    assert!(blocked_claim.is_none());

    let released = runtime
        .mailbox_messages()
        .release_dispatch_target_lease(
            thread_id,
            "owner-first",
            first_claim.attempt.lease_id.as_str(),
        )
        .await?;
    assert!(released);

    let second_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-second".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "owner-second".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("second dispatcher can claim after target lease release");
    assert_eq!(
        second_claim.message.message_id,
        second_message.message.message_id
    );

    Ok(())
}

#[tokio::test]
async fn dispatch_claim_release_stays_blocked_by_fresh_local_owner() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let now = Utc::now();
    let first_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "local-active-target-lease-first",
        /*max_attempts*/ 3,
        /*priority*/ 10,
        Some(now),
    )
    .await?;
    let second_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "local-active-target-lease-second",
        /*max_attempts*/ 3,
        /*priority*/ 5,
        Some(now),
    )
    .await?;

    let first_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-first".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "owner-first".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("first dispatcher should claim first target message");
    assert_eq!(
        first_claim.message.message_id,
        first_message.message.message_id
    );

    runtime
        .local_active_sessions()
        .heartbeat_session(LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id: "owner-first".to_string(),
            session_id: "owner-first-session".to_string(),
            pid: Some(123),
            now,
        })
        .await?;
    let released = runtime
        .mailbox_messages()
        .release_dispatch_target_lease(
            thread_id,
            "owner-first",
            first_claim.attempt.lease_id.as_str(),
        )
        .await?;
    assert!(released);

    let blocked_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-second".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "owner-second".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?;
    assert!(blocked_claim.is_none());

    let owner_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-first".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now,
            local_active_owner_id: "owner-first".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("fresh local owner should claim next target message");
    assert_eq!(
        owner_claim.message.message_id,
        second_message.message.message_id
    );

    Ok(())
}

#[tokio::test]
async fn dispatch_claim_recovers_expired_target_dispatch_lease() -> anyhow::Result<()> {
    let runtime = test_runtime_with_thread().await?;
    let thread_id = test_thread_id();
    let now = Utc::now();
    let first_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "expired-target-lease-first",
        /*max_attempts*/ 3,
        /*priority*/ 10,
        Some(now),
    )
    .await?;
    let second_message = enqueue_test_message_with_options(
        runtime.as_ref(),
        thread_id,
        "expired-target-lease-second",
        /*max_attempts*/ 3,
        /*priority*/ 5,
        Some(now),
    )
    .await?;

    let first_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-first".to_string(),
            lease_duration: std::time::Duration::from_millis(1),
            now,
            local_active_owner_id: "owner-first".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("first dispatcher should claim first target message");
    assert_eq!(
        first_claim.message.message_id,
        first_message.message.message_id
    );

    let second_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-second".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now: now + chrono::Duration::seconds(1),
            local_active_owner_id: "owner-second".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("second dispatcher should recover expired target dispatch lease");
    assert_eq!(
        second_claim.message.message_id,
        first_message.message.message_id
    );
    assert_ne!(
        second_claim.attempt.attempt_id,
        first_claim.attempt.attempt_id
    );

    let third_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-third".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now: now + chrono::Duration::seconds(1),
            local_active_owner_id: "owner-third".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?;
    assert!(third_claim.is_none());

    let released = runtime
        .mailbox_messages()
        .release_dispatch_target_lease(
            thread_id,
            "owner-second",
            second_claim.attempt.lease_id.as_str(),
        )
        .await?;
    assert!(released);

    let next_claim = runtime
        .mailbox_messages()
        .claim_next_due_message(MailboxDispatchClaimParams {
            lease_owner: "dispatcher-third".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now: now + chrono::Duration::seconds(1),
            local_active_owner_id: "owner-third".to_string(),
            local_active_fresh_after: now - chrono::Duration::seconds(5),
        })
        .await?
        .expect("released recovered lease should allow next target message");
    assert_eq!(
        next_claim.message.message_id,
        second_message.message.message_id
    );

    Ok(())
}

#[tokio::test]
async fn claimed_message_survives_restart_and_reclaims_expired_lease() -> anyhow::Result<()> {
    let codex_home = unique_temp_dir();
    let thread_id = test_thread_id();
    let first_claimed_at;
    let message_id;
    let first_attempt_id;

    {
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        runtime
            .upsert_thread(&test_thread_metadata(
                runtime.codex_home(),
                thread_id,
                runtime.codex_home().join("workspace"),
            ))
            .await?;
        let enqueued = enqueue_test_message(runtime.as_ref(), thread_id, "restart-key", 3).await?;
        first_claimed_at = Utc::now();
        let first = runtime
            .mailbox_messages()
            .claim_next_message(MailboxClaimParams {
                target_thread_id: thread_id,
                lease_owner: "dispatcher-before-restart".to_string(),
                lease_duration: std::time::Duration::from_millis(1),
                now: first_claimed_at,
            })
            .await?
            .expect("first claim");

        assert_eq!(first.message.message_id, enqueued.message.message_id);
        message_id = first.message.message_id;
        first_attempt_id = first.attempt.attempt_id;
    }

    let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
    let persisted = runtime
        .mailbox_messages()
        .get_message(message_id.as_str())
        .await?
        .expect("persisted message");
    assert_eq!(persisted.status, crate::MailboxMessageStatus::Claimed);

    let second = runtime
        .mailbox_messages()
        .claim_next_message(MailboxClaimParams {
            target_thread_id: thread_id,
            lease_owner: "dispatcher-after-restart".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now: first_claimed_at + chrono::Duration::seconds(1),
        })
        .await?
        .expect("reclaimed after restart");

    assert_eq!(second.message.message_id, message_id);
    assert_ne!(second.attempt.attempt_id, first_attempt_id);
    assert_eq!(second.message.attempt_count, 2);

    let receipts = runtime
        .mailbox_messages()
        .list_receipts(message_id.as_str())
        .await?;
    let receipt_kinds = receipts
        .into_iter()
        .map(|receipt| receipt.kind)
        .collect::<Vec<_>>();
    assert_eq!(receipt_kinds.len(), 4);
    assert_eq!(
        receipt_kinds
            .iter()
            .filter(|kind| **kind == crate::MailboxReceiptKind::Enqueued)
            .count(),
        1
    );
    assert_eq!(
        receipt_kinds
            .iter()
            .filter(|kind| **kind == crate::MailboxReceiptKind::Claimed)
            .count(),
        2
    );
    assert_eq!(
        receipt_kinds
            .iter()
            .filter(|kind| **kind == crate::MailboxReceiptKind::LeaseExpired)
            .count(),
        1
    );

    Ok(())
}

async fn test_runtime_with_thread() -> anyhow::Result<Arc<StateRuntime>> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    runtime
        .upsert_thread(&test_thread_metadata(
            runtime.codex_home(),
            test_thread_id(),
            runtime.codex_home().join("workspace"),
        ))
        .await?;
    Ok(runtime)
}

fn test_thread_id() -> ThreadId {
    ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("thread id")
}

async fn enqueue_test_message(
    runtime: &StateRuntime,
    target_thread_id: ThreadId,
    idempotency_key: &str,
    max_attempts: i64,
) -> anyhow::Result<MailboxEnqueueOutcome> {
    enqueue_test_message_with_options(
        runtime,
        target_thread_id,
        idempotency_key,
        max_attempts,
        0,
        None,
    )
    .await
}

async fn enqueue_test_message_with_options(
    runtime: &StateRuntime,
    target_thread_id: ThreadId,
    idempotency_key: &str,
    max_attempts: i64,
    priority: i64,
    next_attempt_at: Option<DateTime<Utc>>,
) -> anyhow::Result<MailboxEnqueueOutcome> {
    runtime
        .mailbox_messages()
        .enqueue_message(MailboxEnqueueParams {
            target_thread_id,
            sender_thread_id: None,
            sender_label: Some("coordinator".to_string()),
            idempotency_key: Some(idempotency_key.to_string()),
            kind: crate::MailboxMessageKind::UserInstruction,
            payload_json: serde_json::json!({ "text": "hello" }),
            payload_preview: "hello".to_string(),
            priority,
            max_attempts,
            next_attempt_at,
            expires_at: None,
        })
        .await
}

async fn claim_next(
    runtime: &StateRuntime,
    target_thread_id: ThreadId,
) -> anyhow::Result<Option<MailboxClaim>> {
    runtime
        .mailbox_messages()
        .claim_next_message(MailboxClaimParams {
            target_thread_id,
            lease_owner: "dispatcher".to_string(),
            lease_duration: std::time::Duration::from_secs(30),
            now: Utc::now(),
        })
        .await
}
