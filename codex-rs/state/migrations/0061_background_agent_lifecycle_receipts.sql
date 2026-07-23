ALTER TABLE background_agent_events
    ADD COLUMN receipt_key TEXT;

CREATE UNIQUE INDEX idx_background_agent_events_receipt_key
    ON background_agent_events(run_id, receipt_key)
    WHERE receipt_key IS NOT NULL;
