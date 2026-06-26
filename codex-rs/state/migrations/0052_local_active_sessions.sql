CREATE TABLE local_active_sessions (
    thread_id TEXT PRIMARY KEY NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    owner_id TEXT NOT NULL CHECK(LENGTH(TRIM(owner_id)) > 0),
    session_id TEXT NOT NULL CHECK(LENGTH(TRIM(session_id)) > 0),
    pid INTEGER CHECK(pid IS NULL OR pid >= 0),
    last_seen_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(updated_at_ms >= created_at_ms),
    CHECK(last_seen_at_ms >= created_at_ms)
);

CREATE INDEX idx_local_active_sessions_owner_seen
    ON local_active_sessions(owner_id, last_seen_at_ms DESC, thread_id);

CREATE INDEX idx_local_active_sessions_seen
    ON local_active_sessions(last_seen_at_ms DESC, thread_id);

CREATE TABLE thread_mailbox_target_leases (
    target_thread_id TEXT PRIMARY KEY NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    owner_id TEXT NOT NULL CHECK(LENGTH(TRIM(owner_id)) > 0),
    lease_id TEXT NOT NULL CHECK(LENGTH(TRIM(lease_id)) > 0),
    lease_expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(lease_expires_at_ms > created_at_ms),
    CHECK(updated_at_ms >= created_at_ms)
);

CREATE INDEX idx_thread_mailbox_target_leases_owner
    ON thread_mailbox_target_leases(owner_id, lease_expires_at_ms, target_thread_id);

CREATE INDEX idx_thread_mailbox_target_leases_expiry
    ON thread_mailbox_target_leases(lease_expires_at_ms, target_thread_id);
