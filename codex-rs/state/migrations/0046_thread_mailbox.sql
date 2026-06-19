CREATE TABLE thread_mailbox_messages (
    message_id TEXT PRIMARY KEY NOT NULL,
    target_thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    sender_thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
    sender_label TEXT,
    idempotency_key TEXT CHECK(idempotency_key IS NULL OR LENGTH(TRIM(idempotency_key)) > 0),
    kind TEXT NOT NULL CHECK(kind IN ('user_instruction', 'user_reply', 'control')),
    status TEXT NOT NULL CHECK(status IN ('queued', 'claimed', 'acknowledged', 'failed', 'poisoned', 'expired', 'canceled')),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    payload_sha256 TEXT NOT NULL CHECK(
        LENGTH(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    payload_preview TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
    max_attempts INTEGER NOT NULL DEFAULT 10 CHECK(max_attempts >= 1),
    next_attempt_at_ms INTEGER NOT NULL,
    lease_id TEXT,
    lease_owner TEXT,
    lease_expires_at_ms INTEGER,
    last_attempt_id TEXT,
    last_error TEXT,
    expires_at_ms INTEGER,
    acknowledged_at_ms INTEGER,
    terminal_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(attempt_count <= max_attempts),
    CHECK(
        (lease_id IS NULL AND lease_owner IS NULL AND lease_expires_at_ms IS NULL)
        OR (lease_id IS NOT NULL AND lease_owner IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
    ),
    CHECK(acknowledged_at_ms IS NULL OR acknowledged_at_ms >= created_at_ms),
    CHECK(terminal_at_ms IS NULL OR terminal_at_ms >= created_at_ms),
    CHECK(expires_at_ms IS NULL OR expires_at_ms >= created_at_ms)
);

CREATE UNIQUE INDEX idx_thread_mailbox_messages_idempotency
    ON thread_mailbox_messages(target_thread_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX idx_thread_mailbox_messages_due
    ON thread_mailbox_messages(status, next_attempt_at_ms, lease_expires_at_ms, priority DESC, created_at_ms, message_id)
    WHERE status IN ('queued', 'claimed');

CREATE INDEX idx_thread_mailbox_messages_target_status_created
    ON thread_mailbox_messages(target_thread_id, status, created_at_ms DESC, message_id);

CREATE INDEX idx_thread_mailbox_messages_sender_created
    ON thread_mailbox_messages(sender_thread_id, created_at_ms DESC, message_id)
    WHERE sender_thread_id IS NOT NULL;

CREATE TABLE thread_mailbox_delivery_attempts (
    attempt_id TEXT PRIMARY KEY NOT NULL,
    message_id TEXT NOT NULL REFERENCES thread_mailbox_messages(message_id) ON DELETE CASCADE,
    lease_id TEXT NOT NULL CHECK(LENGTH(TRIM(lease_id)) > 0),
    lease_owner TEXT NOT NULL CHECK(LENGTH(TRIM(lease_owner)) > 0),
    attempt_number INTEGER NOT NULL CHECK(attempt_number >= 1),
    status TEXT NOT NULL CHECK(status IN ('claimed', 'acknowledged', 'failed', 'expired')),
    claimed_at_ms INTEGER NOT NULL,
    lease_expires_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    error TEXT,
    UNIQUE(message_id, attempt_number),
    UNIQUE(lease_id),
    CHECK(lease_expires_at_ms > claimed_at_ms),
    CHECK(completed_at_ms IS NULL OR completed_at_ms >= claimed_at_ms)
);

CREATE INDEX idx_thread_mailbox_delivery_attempts_message
    ON thread_mailbox_delivery_attempts(message_id, attempt_number DESC);

CREATE INDEX idx_thread_mailbox_delivery_attempts_active
    ON thread_mailbox_delivery_attempts(status, lease_expires_at_ms, attempt_id)
    WHERE status = 'claimed';

CREATE TABLE thread_mailbox_receipts (
    receipt_id TEXT PRIMARY KEY NOT NULL,
    message_id TEXT NOT NULL REFERENCES thread_mailbox_messages(message_id) ON DELETE CASCADE,
    attempt_id TEXT REFERENCES thread_mailbox_delivery_attempts(attempt_id) ON DELETE SET NULL,
    thread_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('enqueued', 'claimed', 'acknowledged', 'failed', 'poisoned', 'canceled', 'expired', 'lease_expired')),
    status_after TEXT NOT NULL CHECK(status_after IN ('queued', 'claimed', 'acknowledged', 'failed', 'poisoned', 'expired', 'canceled')),
    payload_json TEXT CHECK(payload_json IS NULL OR json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_thread_mailbox_receipts_message_created
    ON thread_mailbox_receipts(message_id, created_at_ms, receipt_id);

CREATE INDEX idx_thread_mailbox_receipts_thread_created
    ON thread_mailbox_receipts(thread_id, created_at_ms DESC, receipt_id);

CREATE TABLE thread_mailbox_dead_letters (
    message_id TEXT PRIMARY KEY NOT NULL REFERENCES thread_mailbox_messages(message_id) ON DELETE CASCADE,
    failed_attempt_id TEXT REFERENCES thread_mailbox_delivery_attempts(attempt_id) ON DELETE SET NULL,
    reason TEXT NOT NULL CHECK(LENGTH(TRIM(reason)) > 0),
    created_at_ms INTEGER NOT NULL
);
