CREATE TABLE review_publisher_runs (
    review_run_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL,
    turn_id TEXT,
    envelope_json TEXT NOT NULL,
    envelope_sha256 TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK(status IN ('started', 'completed')),
    verdict TEXT CHECK(verdict IS NULL OR verdict IN ('GO', 'NO_GO')),
    terminal_reason TEXT,
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE review_publisher_outbox_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    review_run_id TEXT NOT NULL REFERENCES review_publisher_runs(review_run_id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL CHECK(event_kind IN ('started', 'completed')),
    sequence INTEGER NOT NULL CHECK(sequence IN (0, 1)),
    status TEXT NOT NULL CHECK(status IN ('pending', 'in_flight', 'delivered', 'dead_letter')),
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
    next_attempt_at_ms INTEGER NOT NULL,
    lease_owner TEXT,
    lease_expires_at_ms INTEGER,
    receipt_id TEXT,
    last_error_code TEXT,
    created_at_ms INTEGER NOT NULL,
    delivered_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(review_run_id, event_kind),
    UNIQUE(review_run_id, sequence),
    CHECK(
        (event_kind = 'started' AND sequence = 0)
        OR (event_kind = 'completed' AND sequence = 1)
    ),
    CHECK(
        (status = 'in_flight' AND lease_owner IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
        OR (status != 'in_flight' AND lease_owner IS NULL AND lease_expires_at_ms IS NULL)
    )
);

CREATE INDEX idx_review_publisher_outbox_due
    ON review_publisher_outbox_events(status, next_attempt_at_ms, lease_expires_at_ms, sequence);

CREATE INDEX idx_review_publisher_outbox_run
    ON review_publisher_outbox_events(review_run_id, sequence);
