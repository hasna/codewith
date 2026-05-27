CREATE TABLE thread_schedules (
    schedule_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    prompt_source TEXT NOT NULL CHECK(prompt_source IN ('inline', 'default')),
    prompt TEXT NOT NULL CHECK(LENGTH(TRIM(prompt)) > 0),
    schedule_kind TEXT NOT NULL CHECK(schedule_kind IN ('dynamic', 'interval', 'cron')),
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
        (schedule_kind = 'dynamic' AND interval_amount IS NULL AND interval_unit IS NULL AND cron_expression IS NULL)
        OR (schedule_kind = 'interval' AND interval_amount IS NOT NULL AND interval_unit IS NOT NULL AND cron_expression IS NULL)
        OR (schedule_kind = 'cron' AND interval_amount IS NULL AND interval_unit IS NULL AND cron_expression IS NOT NULL)
    ),
    CHECK(
        (lease_id IS NULL AND lease_expires_at_ms IS NULL)
        OR (lease_id IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
    )
);

CREATE INDEX idx_thread_schedules_thread_status
    ON thread_schedules(thread_id, status, next_run_at_ms);

CREATE INDEX idx_thread_schedules_due
    ON thread_schedules(status, next_run_at_ms, lease_expires_at_ms)
    WHERE status = 'active' AND next_run_at_ms IS NOT NULL;

CREATE TABLE thread_schedule_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    schedule_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('leased', 'running', 'completed', 'failed')),
    lease_id TEXT NOT NULL,
    turn_id TEXT,
    error TEXT,
    scheduled_for_ms INTEGER,
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    FOREIGN KEY(schedule_id) REFERENCES thread_schedules(schedule_id) ON DELETE CASCADE
);

CREATE INDEX idx_thread_schedule_runs_schedule_started
    ON thread_schedule_runs(schedule_id, started_at_ms DESC);
