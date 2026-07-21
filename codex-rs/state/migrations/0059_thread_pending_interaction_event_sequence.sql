CREATE TABLE thread_pending_interaction_events_new (
    insertion_seq INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    interaction_id TEXT NOT NULL REFERENCES thread_pending_interactions(interaction_id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL CHECK(event_kind IN (
        'created',
        'delivered',
        'responded',
        'expired',
        'cancelled',
        'denied',
        'no_longer_waiting'
    )),
    status TEXT NOT NULL CHECK(status IN (
        'pending',
        'delivered',
        'responded',
        'expired',
        'cancelled',
        'denied',
        'no_longer_waiting'
    )),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    payload_sha256 TEXT NOT NULL CHECK(
        LENGTH(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    payload_preview TEXT NOT NULL,
    redactions_json TEXT NOT NULL CHECK(json_valid(redactions_json)),
    created_at_ms INTEGER NOT NULL
);

INSERT INTO thread_pending_interaction_events_new (
    insertion_seq,
    event_id,
    interaction_id,
    thread_id,
    event_kind,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    created_at_ms
)
SELECT
    rowid,
    event_id,
    interaction_id,
    thread_id,
    event_kind,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    created_at_ms
FROM thread_pending_interaction_events
ORDER BY rowid ASC;

DROP TABLE thread_pending_interaction_events;
ALTER TABLE thread_pending_interaction_events_new RENAME TO thread_pending_interaction_events;

CREATE INDEX idx_thread_pending_interaction_events_interaction_created
    ON thread_pending_interaction_events(interaction_id, created_at_ms, insertion_seq);

CREATE INDEX idx_thread_pending_interaction_events_thread_created
    ON thread_pending_interaction_events(thread_id, created_at_ms DESC, insertion_seq DESC);
