use codex_app_server_protocol::ThreadMailboxClaim;
use codex_app_server_protocol::ThreadMailboxDeliveryAttempt;
use codex_app_server_protocol::ThreadMailboxMessageDetail;
use codex_app_server_protocol::ThreadMailboxMessageKind;
use codex_app_server_protocol::ThreadMailboxMessageStatus;
use codex_app_server_protocol::ThreadMailboxMessageSummary;
use codex_app_server_protocol::ThreadMailboxReceipt;
use codex_app_server_protocol::ThreadMailboxReceiptKind;
use codex_app_server_protocol::ThreadMailboxRedaction;

pub(crate) fn api_mailbox_kind_to_state(
    kind: ThreadMailboxMessageKind,
) -> codex_state::MailboxMessageKind {
    match kind {
        ThreadMailboxMessageKind::UserInstruction => {
            codex_state::MailboxMessageKind::UserInstruction
        }
        ThreadMailboxMessageKind::UserReply => codex_state::MailboxMessageKind::UserReply,
        ThreadMailboxMessageKind::Control => codex_state::MailboxMessageKind::Control,
    }
}

pub(crate) fn api_mailbox_status_to_state(
    status: ThreadMailboxMessageStatus,
) -> codex_state::MailboxMessageStatus {
    match status {
        ThreadMailboxMessageStatus::Queued => codex_state::MailboxMessageStatus::Queued,
        ThreadMailboxMessageStatus::Claimed => codex_state::MailboxMessageStatus::Claimed,
        ThreadMailboxMessageStatus::Acknowledged => codex_state::MailboxMessageStatus::Acknowledged,
        ThreadMailboxMessageStatus::Failed => codex_state::MailboxMessageStatus::Failed,
        ThreadMailboxMessageStatus::Poisoned => codex_state::MailboxMessageStatus::Poisoned,
        ThreadMailboxMessageStatus::Expired => codex_state::MailboxMessageStatus::Expired,
        ThreadMailboxMessageStatus::Canceled => codex_state::MailboxMessageStatus::Canceled,
    }
}

fn state_mailbox_kind_to_api(kind: codex_state::MailboxMessageKind) -> ThreadMailboxMessageKind {
    match kind {
        codex_state::MailboxMessageKind::UserInstruction => {
            ThreadMailboxMessageKind::UserInstruction
        }
        codex_state::MailboxMessageKind::UserReply => ThreadMailboxMessageKind::UserReply,
        codex_state::MailboxMessageKind::Control => ThreadMailboxMessageKind::Control,
    }
}

fn state_mailbox_status_to_api(
    status: codex_state::MailboxMessageStatus,
) -> ThreadMailboxMessageStatus {
    match status {
        codex_state::MailboxMessageStatus::Queued => ThreadMailboxMessageStatus::Queued,
        codex_state::MailboxMessageStatus::Claimed => ThreadMailboxMessageStatus::Claimed,
        codex_state::MailboxMessageStatus::Acknowledged => ThreadMailboxMessageStatus::Acknowledged,
        codex_state::MailboxMessageStatus::Failed => ThreadMailboxMessageStatus::Failed,
        codex_state::MailboxMessageStatus::Poisoned => ThreadMailboxMessageStatus::Poisoned,
        codex_state::MailboxMessageStatus::Expired => ThreadMailboxMessageStatus::Expired,
        codex_state::MailboxMessageStatus::Canceled => ThreadMailboxMessageStatus::Canceled,
    }
}

fn state_receipt_kind_to_api(kind: codex_state::MailboxReceiptKind) -> ThreadMailboxReceiptKind {
    match kind {
        codex_state::MailboxReceiptKind::Enqueued => ThreadMailboxReceiptKind::Enqueued,
        codex_state::MailboxReceiptKind::Claimed => ThreadMailboxReceiptKind::Claimed,
        codex_state::MailboxReceiptKind::Acknowledged => ThreadMailboxReceiptKind::Acknowledged,
        codex_state::MailboxReceiptKind::Failed => ThreadMailboxReceiptKind::Failed,
        codex_state::MailboxReceiptKind::Poisoned => ThreadMailboxReceiptKind::Poisoned,
        codex_state::MailboxReceiptKind::Canceled => ThreadMailboxReceiptKind::Canceled,
        codex_state::MailboxReceiptKind::Expired => ThreadMailboxReceiptKind::Expired,
        codex_state::MailboxReceiptKind::LeaseExpired => ThreadMailboxReceiptKind::LeaseExpired,
    }
}

pub(crate) fn api_mailbox_summary(
    message: codex_state::MailboxMessage,
) -> ThreadMailboxMessageSummary {
    ThreadMailboxMessageSummary {
        message_id: message.message_id,
        target_thread_id: message.target_thread_id.to_string(),
        sender_thread_id: message
            .sender_thread_id
            .map(|thread_id| thread_id.to_string()),
        sender_label: message.sender_label,
        kind: state_mailbox_kind_to_api(message.kind),
        status: state_mailbox_status_to_api(message.status),
        payload_sha256: message.payload_sha256,
        payload_preview: message.payload_preview,
        redactions: vec![
            ThreadMailboxRedaction::MessageBody,
            ThreadMailboxRedaction::IdempotencyKey,
        ],
        priority: message.priority,
        attempt_count: message.attempt_count,
        max_attempts: message.max_attempts,
        next_attempt_at: message.next_attempt_at.timestamp(),
        lease_expires_at: message
            .lease_expires_at
            .map(|timestamp| timestamp.timestamp()),
        last_error: None,
        expires_at: message.expires_at.map(|timestamp| timestamp.timestamp()),
        acknowledged_at: message
            .acknowledged_at
            .map(|timestamp| timestamp.timestamp()),
        terminal_at: message.terminal_at.map(|timestamp| timestamp.timestamp()),
        created_at: message.created_at.timestamp(),
        updated_at: message.updated_at.timestamp(),
    }
}

pub(crate) fn api_mailbox_detail(
    message: codex_state::MailboxMessage,
) -> ThreadMailboxMessageDetail {
    let payload = message.payload_json.clone();
    ThreadMailboxMessageDetail {
        summary: api_mailbox_summary(message),
        message: payload,
    }
}

pub(crate) fn api_mailbox_claim(claim: codex_state::MailboxClaim) -> ThreadMailboxClaim {
    ThreadMailboxClaim {
        message: api_mailbox_detail(claim.message),
        attempt: ThreadMailboxDeliveryAttempt {
            attempt_id: claim.attempt.attempt_id,
            lease_id: claim.attempt.lease_id,
            lease_owner: claim.attempt.lease_owner,
            attempt_number: claim.attempt.attempt_number,
            claimed_at: claim.attempt.claimed_at.timestamp(),
            lease_expires_at: claim.attempt.lease_expires_at.timestamp(),
        },
    }
}

pub(crate) fn api_mailbox_receipt(receipt: codex_state::MailboxReceipt) -> ThreadMailboxReceipt {
    ThreadMailboxReceipt {
        receipt_id: receipt.receipt_id,
        message_id: receipt.message_id,
        attempt_id: receipt.attempt_id,
        thread_id: receipt.thread_id.to_string(),
        kind: state_receipt_kind_to_api(receipt.kind),
        status_after: state_mailbox_status_to_api(receipt.status_after),
        payload: receipt.payload_json,
        created_at: receipt.created_at.timestamp(),
    }
}
