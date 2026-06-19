ALTER TABLE workflow_runs
    ADD COLUMN loops_json TEXT CHECK(loops_json IS NULL OR json_valid(loops_json));

ALTER TABLE workflow_runs
    ADD COLUMN monitor_links_json TEXT CHECK(
        monitor_links_json IS NULL OR json_valid(monitor_links_json)
    );

CREATE TABLE workflow_run_timers (
    timer_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
    workflow_loop_id TEXT NOT NULL,
    title TEXT NOT NULL CHECK(LENGTH(TRIM(title)) > 0),
    schedule_json TEXT NOT NULL CHECK(json_valid(schedule_json)),
    timezone TEXT NOT NULL CHECK(LENGTH(TRIM(timezone)) > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'expired', 'cancelled')),
    stop_condition_json TEXT NOT NULL CHECK(json_valid(stop_condition_json)),
    trigger_step_id TEXT,
    max_iterations INTEGER NOT NULL CHECK(max_iterations > 0),
    iteration_count INTEGER NOT NULL DEFAULT 0 CHECK(iteration_count >= 0),
    next_fire_at_ms INTEGER,
    last_fire_at_ms INTEGER,
    expires_at_ms INTEGER,
    lease_id TEXT,
    lease_expires_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(run_id, workflow_loop_id),
    CHECK(iteration_count <= max_iterations),
    CHECK(
        (lease_id IS NULL AND lease_expires_at_ms IS NULL)
        OR (lease_id IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
    )
);

CREATE INDEX idx_workflow_run_timers_due
    ON workflow_run_timers(status, next_fire_at_ms, lease_expires_at_ms)
    WHERE status = 'active' AND next_fire_at_ms IS NOT NULL;

CREATE INDEX idx_workflow_run_timers_run_status
    ON workflow_run_timers(run_id, status, next_fire_at_ms);

CREATE TABLE workflow_run_timer_fires (
    timer_fire_id TEXT PRIMARY KEY NOT NULL,
    timer_id TEXT NOT NULL REFERENCES workflow_run_timers(timer_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
    workflow_loop_id TEXT NOT NULL,
    fire_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('claimed', 'completed', 'skipped', 'failed')),
    lease_id TEXT NOT NULL,
    scheduled_for_ms INTEGER NOT NULL,
    claimed_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
    UNIQUE(timer_id, fire_key)
);

CREATE INDEX idx_workflow_run_timer_fires_timer_claimed
    ON workflow_run_timer_fires(timer_id, status, claimed_at_ms);

CREATE TABLE workflow_run_monitor_links (
    link_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
    workflow_monitor_id TEXT NOT NULL,
    title TEXT NOT NULL CHECK(LENGTH(TRIM(title)) > 0),
    source TEXT NOT NULL CHECK(source IN ('existing_thread_monitor')),
    monitor_ref TEXT,
    trigger_step_id TEXT,
    stop_condition_json TEXT CHECK(stop_condition_json IS NULL OR json_valid(stop_condition_json)),
    max_events_per_tick INTEGER NOT NULL CHECK(max_events_per_tick > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'stopped', 'cancelled')),
    last_seen_event_id TEXT,
    last_seen_created_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(run_id, workflow_monitor_id)
);

CREATE INDEX idx_workflow_run_monitor_links_run_status
    ON workflow_run_monitor_links(run_id, status, workflow_monitor_id);
