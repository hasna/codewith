CREATE TABLE app_webhook_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    source_app_id TEXT NOT NULL,
    source_app_name TEXT,
    subscription_id TEXT,
    event_type TEXT NOT NULL,
    external_delivery_id TEXT,
    idempotency_key TEXT,
    target_thread_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('unread', 'processed', 'archived', 'injected', 'queued')),
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    payload_preview TEXT NOT NULL,
    redactions_json TEXT NOT NULL DEFAULT '[]',
    received_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_app_webhook_events_source_delivery
    ON app_webhook_events(source_app_id, external_delivery_id)
    WHERE external_delivery_id IS NOT NULL;

CREATE UNIQUE INDEX idx_app_webhook_events_idempotency
    ON app_webhook_events(source_app_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX idx_app_webhook_events_status_received
    ON app_webhook_events(status, received_at_ms DESC, event_id);

CREATE INDEX idx_app_webhook_events_thread_received
    ON app_webhook_events(target_thread_id, received_at_ms DESC, event_id)
    WHERE target_thread_id IS NOT NULL;

CREATE INDEX idx_app_webhook_events_source_received
    ON app_webhook_events(source_app_id, received_at_ms DESC, event_id);
