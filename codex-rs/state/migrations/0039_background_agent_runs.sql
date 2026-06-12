CREATE TABLE background_agent_runs (
    id TEXT PRIMARY KEY,
    idempotency_key TEXT,
    request_id TEXT,
    source TEXT NOT NULL,
    prompt_snapshot_ref TEXT NOT NULL,
    input_snapshot_ref TEXT,
    thread_id TEXT,
    thread_store_kind TEXT NOT NULL,
    thread_store_id TEXT,
    rollout_path TEXT,
    parent_thread_id TEXT,
    parent_agent_run_id TEXT,
    spawn_linkage_json TEXT,
    worktree_lease_id TEXT,
    auth_profile_ref TEXT,
    desired_state TEXT NOT NULL,
    status TEXT NOT NULL,
    status_reason TEXT,
    config_fingerprint TEXT,
    version_fingerprint TEXT,
    retention_state TEXT NOT NULL DEFAULT 'active',
    archive_after INTEGER,
    delete_after INTEGER,
    archived_at INTEGER,
    deleted_at INTEGER,
    supervisor_id TEXT,
    generation INTEGER NOT NULL DEFAULT 0,
    pid INTEGER,
    pgid INTEGER,
    job_id TEXT,
    heartbeat_at INTEGER,
    crash_reason TEXT,
    exit_code INTEGER,
    exit_signal INTEGER,
    last_event_seq INTEGER NOT NULL DEFAULT 0,
    last_snapshot_seq INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    completed_at INTEGER
);

CREATE UNIQUE INDEX idx_background_agent_runs_idempotency_key
    ON background_agent_runs(idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE UNIQUE INDEX idx_background_agent_runs_request_id
    ON background_agent_runs(request_id)
    WHERE request_id IS NOT NULL;

CREATE INDEX idx_background_agent_runs_roster
    ON background_agent_runs(retention_state, status, updated_at DESC);

CREATE INDEX idx_background_agent_runs_thread_id
    ON background_agent_runs(thread_id);

CREATE INDEX idx_background_agent_runs_supervisor
    ON background_agent_runs(supervisor_id, heartbeat_at);

CREATE TABLE background_agent_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE,
    UNIQUE(run_id, seq)
);

CREATE INDEX idx_background_agent_events_run_seq
    ON background_agent_events(run_id, seq);

CREATE INDEX idx_background_agent_events_created_at
    ON background_agent_events(created_at, id);

CREATE TABLE background_agent_status_snapshots (
    run_id TEXT PRIMARY KEY,
    seq INTEGER NOT NULL,
    status TEXT NOT NULL,
    desired_state TEXT NOT NULL,
    summary TEXT,
    pending_interaction_count INTEGER NOT NULL DEFAULT 0,
    last_event_seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE
);

CREATE TABLE background_agent_pending_interactions (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    worker_request_id TEXT,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    request_payload_json TEXT NOT NULL,
    response_payload_json TEXT,
    no_client_policy TEXT NOT NULL,
    timeout_at INTEGER,
    created_at INTEGER NOT NULL,
    delivered_at INTEGER,
    responded_at INTEGER,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE
);

CREATE INDEX idx_background_agent_pending_interactions_run_status
    ON background_agent_pending_interactions(run_id, status, created_at);

CREATE UNIQUE INDEX idx_background_agent_pending_interactions_worker_request
    ON background_agent_pending_interactions(run_id, worker_request_id)
    WHERE worker_request_id IS NOT NULL;

CREATE TABLE background_agent_worktree_leases (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    identity TEXT NOT NULL,
    mode TEXT NOT NULL,
    base_repo_path TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    branch TEXT,
    head_sha TEXT,
    status_snapshot_json TEXT NOT NULL,
    dirty INTEGER NOT NULL DEFAULT 0,
    cleanup_after INTEGER,
    force_delete_requested INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    released_at INTEGER,
    deleted_at INTEGER,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE
);

CREATE INDEX idx_background_agent_worktree_leases_run
    ON background_agent_worktree_leases(run_id);

CREATE UNIQUE INDEX idx_background_agent_worktree_leases_identity
    ON background_agent_worktree_leases(identity);

CREATE UNIQUE INDEX idx_background_agent_worktree_leases_active_isolated_path
    ON background_agent_worktree_leases(worktree_path)
    WHERE mode = 'isolated_worktree' AND deleted_at IS NULL;

CREATE UNIQUE INDEX idx_background_agent_worktree_leases_active_shared_repo
    ON background_agent_worktree_leases(base_repo_path)
    WHERE mode = 'shared_repository' AND released_at IS NULL AND deleted_at IS NULL;

CREATE INDEX idx_background_agent_worktree_leases_cleanup
    ON background_agent_worktree_leases(cleanup_after)
    WHERE cleanup_after IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE background_agent_process_leases (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    supervisor_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    pid INTEGER,
    pgid INTEGER,
    job_id TEXT,
    status TEXT NOT NULL,
    heartbeat_at INTEGER,
    exit_code INTEGER,
    exit_signal INTEGER,
    exit_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    stopped_at INTEGER,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE,
    UNIQUE(run_id, generation)
);

CREATE INDEX idx_background_agent_process_leases_supervisor
    ON background_agent_process_leases(supervisor_id, heartbeat_at);

CREATE TABLE background_agent_execution_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    snapshot_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    recovery_policy TEXT NOT NULL,
    config_fingerprint TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE,
    UNIQUE(run_id, seq)
);

CREATE INDEX idx_background_agent_execution_snapshots_run_seq
    ON background_agent_execution_snapshots(run_id, seq);

CREATE TABLE background_agent_cleanup_tombstones (
    run_id TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    worktree_path TEXT,
    dirty_worktree INTEGER NOT NULL DEFAULT 0,
    retained_until INTEGER,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    deleted_at INTEGER,
    FOREIGN KEY(run_id) REFERENCES background_agent_runs(id) ON DELETE CASCADE
);
