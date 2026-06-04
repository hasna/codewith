PRAGMA foreign_keys=OFF;

CREATE TABLE thread_schedules_new (
    schedule_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    prompt_source TEXT NOT NULL CHECK(prompt_source IN ('inline', 'default')),
    prompt TEXT NOT NULL CHECK(LENGTH(TRIM(prompt)) > 0),
    schedule_kind TEXT NOT NULL CHECK(schedule_kind IN ('once', 'dynamic', 'interval', 'cron')),
    interval_amount INTEGER CHECK(interval_amount IS NULL OR interval_amount > 0),
    interval_unit TEXT CHECK(interval_unit IS NULL OR interval_unit IN ('minutes', 'hours', 'days')),
    cron_expression TEXT CHECK(cron_expression IS NULL OR LENGTH(TRIM(cron_expression)) > 0),
    timezone TEXT NOT NULL CHECK(LENGTH(TRIM(timezone)) > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'paused', 'expired')),
    next_run_at_ms INTEGER,
    last_run_at_ms INTEGER,
    expires_at_ms INTEGER,
    failure_count INTEGER NOT NULL DEFAULT 0 CHECK(failure_count >= 0),
    lease_id TEXT,
    lease_expires_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(
        (schedule_kind = 'once' AND interval_amount IS NULL AND interval_unit IS NULL AND cron_expression IS NULL)
        OR (schedule_kind = 'dynamic' AND interval_amount IS NULL AND interval_unit IS NULL AND cron_expression IS NULL)
        OR (schedule_kind = 'interval' AND interval_amount IS NOT NULL AND interval_unit IS NOT NULL AND cron_expression IS NULL)
        OR (schedule_kind = 'cron' AND interval_amount IS NULL AND interval_unit IS NULL AND cron_expression IS NOT NULL)
    ),
    CHECK(
        (lease_id IS NULL AND lease_expires_at_ms IS NULL)
        OR (lease_id IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
    )
);

INSERT INTO thread_schedules_new (
    schedule_id,
    thread_id,
    prompt_source,
    prompt,
    schedule_kind,
    interval_amount,
    interval_unit,
    cron_expression,
    timezone,
    status,
    next_run_at_ms,
    last_run_at_ms,
    expires_at_ms,
    failure_count,
    lease_id,
    lease_expires_at_ms,
    created_at_ms,
    updated_at_ms
)
SELECT
    schedule_id,
    thread_id,
    prompt_source,
    prompt,
    CASE WHEN should_convert_to_once THEN 'once' ELSE schedule_kind END,
    CASE WHEN should_convert_to_once THEN NULL ELSE interval_amount END,
    CASE WHEN should_convert_to_once THEN NULL ELSE interval_unit END,
    cron_expression,
    timezone,
    status,
    next_run_at_ms,
    last_run_at_ms,
    CASE WHEN should_convert_to_once THEN NULL ELSE expires_at_ms END,
    failure_count,
    lease_id,
    lease_expires_at_ms,
    created_at_ms,
    updated_at_ms
FROM (
    SELECT
        *,
        schedule_kind = 'interval'
            AND next_run_at_ms IS NOT NULL
            AND expires_at_ms IS NOT NULL
            AND expires_at_ms <= next_run_at_ms + CASE interval_unit
                WHEN 'minutes' THEN interval_amount * 60000
                WHEN 'hours' THEN interval_amount * 3600000
                WHEN 'days' THEN interval_amount * 86400000
                ELSE NULL
            END AS should_convert_to_once
    FROM thread_schedules
);

DROP TABLE thread_schedules;
ALTER TABLE thread_schedules_new RENAME TO thread_schedules;

CREATE INDEX idx_thread_schedules_thread_status
    ON thread_schedules(thread_id, status, next_run_at_ms);

CREATE INDEX idx_thread_schedules_due
    ON thread_schedules(status, next_run_at_ms, lease_expires_at_ms)
    WHERE status = 'active' AND next_run_at_ms IS NOT NULL;

PRAGMA foreign_keys=ON;
