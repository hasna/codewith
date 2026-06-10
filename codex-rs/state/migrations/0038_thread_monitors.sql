CREATE TABLE thread_monitors (
    monitor_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK(LENGTH(TRIM(name)) > 0),
    prompt TEXT NOT NULL CHECK(LENGTH(TRIM(prompt)) > 0),
    command TEXT NOT NULL CHECK(LENGTH(TRIM(command)) > 0),
    cwd TEXT,
    routing TEXT NOT NULL CHECK(routing IN ('stream', 'file', 'both')),
    output_file TEXT,
    status TEXT NOT NULL CHECK(status IN ('running', 'stopped', 'failed')),
    generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
    process_id INTEGER,
    last_event_at_ms INTEGER,
    last_error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(
        (routing = 'stream' AND output_file IS NULL)
        OR (routing IN ('file', 'both') AND output_file IS NOT NULL AND LENGTH(TRIM(output_file)) > 0)
    )
);

CREATE INDEX idx_thread_monitors_thread_status
    ON thread_monitors(thread_id, status, updated_at_ms DESC);

CREATE INDEX idx_thread_monitors_running
    ON thread_monitors(status, generation)
    WHERE status = 'running';

CREATE TABLE thread_monitor_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    monitor_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    stream TEXT NOT NULL CHECK(stream IN ('stdout', 'stderr', 'system')),
    text TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY(monitor_id) REFERENCES thread_monitors(monitor_id) ON DELETE CASCADE
);

CREATE INDEX idx_thread_monitor_events_monitor_created
    ON thread_monitor_events(monitor_id, created_at_ms, event_id);
