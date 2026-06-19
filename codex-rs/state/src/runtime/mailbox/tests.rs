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
