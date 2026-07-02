CREATE TRIGGER thread_schedules_ignore_legacy_live_owner_claim
BEFORE UPDATE OF lease_id ON thread_schedules
WHEN NEW.lease_id IS NOT NULL
    AND NEW.lease_id NOT LIKE 'owner:%'
    AND EXISTS (
        SELECT 1
        FROM local_active_sessions AS active
        WHERE active.thread_id = NEW.thread_id
            AND active.last_seen_at_ms >= (CAST(strftime('%s', 'now') AS INTEGER) * 1000 - 15000)
    )
BEGIN
    SELECT RAISE(IGNORE);
END;
