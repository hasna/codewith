ALTER TABLE background_agent_runs
    ADD COLUMN admission_identity_sha256 TEXT;

ALTER TABLE background_agent_runs
    ADD COLUMN admission_ready_at INTEGER;

ALTER TABLE background_agent_events
    ADD COLUMN receipt_key TEXT;

CREATE UNIQUE INDEX idx_background_agent_events_receipt_key
    ON background_agent_events(run_id, receipt_key)
    WHERE receipt_key IS NOT NULL;

CREATE TABLE background_agent_lifecycle_receipts (
    run_id TEXT NOT NULL,
    receipt_key TEXT NOT NULL,
    event_id INTEGER NOT NULL,
    event_seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    generation INTEGER NOT NULL,
    attempt INTEGER,
    operation_identity_sha256 TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, receipt_key),
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE
);

INSERT INTO background_agent_lifecycle_receipts (
    run_id,
    receipt_key,
    event_id,
    event_seq,
    event_type,
    generation,
    attempt,
    operation_identity_sha256,
    payload_json,
    created_at
)
SELECT
    run_id,
    receipt_key,
    id,
    seq,
    event_type,
    COALESCE(json_extract(payload_json, '$.generation'), 0),
    json_extract(payload_json, '$.attempt'),
    '',
    payload_json,
    created_at
FROM background_agent_events
WHERE receipt_key IS NOT NULL;
