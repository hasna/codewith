PRAGMA foreign_keys = OFF;

CREATE TABLE thread_schedule_runs_new (
    run_id TEXT PRIMARY KEY NOT NULL,
    schedule_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('leased', 'running', 'deferred', 'completed', 'failed')),
    lease_id TEXT NOT NULL,
    turn_id TEXT,
    error TEXT,
    scheduled_for_ms INTEGER,
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    FOREIGN KEY(schedule_id) REFERENCES thread_schedules(schedule_id) ON DELETE CASCADE
);

INSERT INTO thread_schedule_runs_new (
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    turn_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
)
SELECT
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    turn_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
FROM thread_schedule_runs;

DROP TABLE thread_schedule_runs;

ALTER TABLE thread_schedule_runs_new RENAME TO thread_schedule_runs;

CREATE INDEX idx_thread_schedule_runs_schedule_started
    ON thread_schedule_runs(schedule_id, started_at_ms DESC);

PRAGMA foreign_keys = ON;
