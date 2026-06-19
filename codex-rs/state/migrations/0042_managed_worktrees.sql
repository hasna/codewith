CREATE TABLE managed_worktrees (
    worktree_id TEXT PRIMARY KEY NOT NULL,
    identity TEXT,
    mode TEXT NOT NULL CHECK(mode IN ('isolated_worktree', 'shared_repository')),
    base_repo_path TEXT NOT NULL CHECK(LENGTH(TRIM(base_repo_path)) > 0),
    worktree_path TEXT NOT NULL CHECK(LENGTH(TRIM(worktree_path)) > 0),
    branch TEXT,
    base_sha TEXT,
    head_sha TEXT,
    lifecycle_status TEXT NOT NULL CHECK(lifecycle_status IN ('active', 'released', 'cleanup_pending', 'deleted')),
    status_snapshot_json TEXT NOT NULL CHECK(json_valid(status_snapshot_json)),
    dirty INTEGER NOT NULL CHECK(dirty IN (0, 1)),
    cleanup_policy TEXT NOT NULL CHECK(cleanup_policy IN ('retain', 'delete_if_clean', 'force_delete')),
    force_delete_requested INTEGER NOT NULL CHECK(force_delete_requested IN (0, 1)),
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('manual', 'main_session', 'sub_session', 'background_agent')),
    owner_thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
    owner_agent_run_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    released_at_ms INTEGER,
    cleanup_after_ms INTEGER,
    deleted_at_ms INTEGER
);

CREATE UNIQUE INDEX idx_managed_worktrees_live_isolated_path
    ON managed_worktrees(worktree_path)
    WHERE mode = 'isolated_worktree' AND deleted_at_ms IS NULL;

CREATE UNIQUE INDEX idx_managed_worktrees_active_shared_repo
    ON managed_worktrees(base_repo_path)
    WHERE mode = 'shared_repository'
      AND deleted_at_ms IS NULL
      AND released_at_ms IS NULL
      AND lifecycle_status = 'active';

CREATE INDEX idx_managed_worktrees_base_repo_status_updated
    ON managed_worktrees(base_repo_path, lifecycle_status, updated_at_ms DESC, worktree_id);

CREATE INDEX idx_managed_worktrees_owner_thread
    ON managed_worktrees(owner_thread_id, updated_at_ms DESC, worktree_id)
    WHERE owner_thread_id IS NOT NULL;

CREATE INDEX idx_managed_worktrees_owner_agent
    ON managed_worktrees(owner_agent_run_id, updated_at_ms DESC, worktree_id)
    WHERE owner_agent_run_id IS NOT NULL;

INSERT INTO managed_worktrees (
    worktree_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    head_sha,
    lifecycle_status,
    status_snapshot_json,
    dirty,
    cleanup_policy,
    force_delete_requested,
    owner_kind,
    owner_agent_run_id,
    created_at_ms,
    updated_at_ms,
    released_at_ms,
    cleanup_after_ms,
    deleted_at_ms
)
SELECT
    id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    head_sha,
    CASE
        WHEN deleted_at IS NOT NULL THEN 'deleted'
        WHEN mode = 'isolated_worktree' AND released_at IS NOT NULL AND dirty = 1 AND cleanup_after IS NOT NULL THEN 'cleanup_pending'
        WHEN released_at IS NOT NULL THEN 'released'
        ELSE 'active'
    END,
    status_snapshot_json,
    dirty,
    CASE
        WHEN force_delete_requested = 1 THEN 'force_delete'
        WHEN cleanup_after IS NOT NULL THEN 'delete_if_clean'
        ELSE 'retain'
    END,
    force_delete_requested,
    'background_agent',
    run_id,
    created_at * 1000,
    updated_at * 1000,
    released_at * 1000,
    cleanup_after * 1000,
    deleted_at * 1000
FROM background_agent_worktree_leases;

CREATE TABLE managed_worktree_assignments (
    assignment_id TEXT PRIMARY KEY NOT NULL,
    worktree_id TEXT NOT NULL REFERENCES managed_worktrees(worktree_id) ON DELETE CASCADE,
    thread_id TEXT REFERENCES threads(id) ON DELETE CASCADE,
    agent_run_id TEXT,
    attached_at_ms INTEGER NOT NULL,
    detached_at_ms INTEGER,
    CHECK (
        (thread_id IS NOT NULL AND agent_run_id IS NULL)
        OR (thread_id IS NULL AND agent_run_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_managed_worktree_assignments_worktree_active
    ON managed_worktree_assignments(worktree_id)
    WHERE detached_at_ms IS NULL;

CREATE INDEX idx_managed_worktree_assignments_worktree_history
    ON managed_worktree_assignments(worktree_id, attached_at_ms DESC, assignment_id);

CREATE UNIQUE INDEX idx_managed_worktree_assignments_thread_active
    ON managed_worktree_assignments(thread_id)
    WHERE thread_id IS NOT NULL AND detached_at_ms IS NULL;

CREATE UNIQUE INDEX idx_managed_worktree_assignments_agent_active
    ON managed_worktree_assignments(agent_run_id)
    WHERE agent_run_id IS NOT NULL AND detached_at_ms IS NULL;

CREATE TABLE managed_worktree_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    worktree_id TEXT NOT NULL REFERENCES managed_worktrees(worktree_id) ON DELETE CASCADE,
    seq INTEGER NOT NULL CHECK(seq >= 0),
    event_type TEXT NOT NULL CHECK(LENGTH(TRIM(event_type)) > 0),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL,
    UNIQUE(worktree_id, seq)
);

CREATE INDEX idx_managed_worktree_events_worktree_seq
    ON managed_worktree_events(worktree_id, seq);

CREATE TABLE managed_worktree_merge_candidates (
    candidate_id TEXT PRIMARY KEY NOT NULL,
    worktree_id TEXT NOT NULL REFERENCES managed_worktrees(worktree_id) ON DELETE CASCADE,
    target_ref TEXT NOT NULL CHECK(LENGTH(TRIM(target_ref)) > 0),
    target_sha TEXT,
    base_sha TEXT NOT NULL CHECK(LENGTH(TRIM(base_sha)) > 0),
    head_sha TEXT NOT NULL CHECK(LENGTH(TRIM(head_sha)) > 0),
    status TEXT NOT NULL CHECK(status IN ('open', 'blocked', 'applied', 'dismissed')),
    conflict_summary TEXT,
    test_summary_json TEXT CHECK(test_summary_json IS NULL OR json_valid(test_summary_json)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    applied_at_ms INTEGER,
    dismissed_at_ms INTEGER
);

CREATE UNIQUE INDEX idx_managed_worktree_merge_candidates_active_target
    ON managed_worktree_merge_candidates(worktree_id, head_sha, target_ref)
    WHERE status IN ('open', 'blocked');

CREATE INDEX idx_managed_worktree_merge_candidates_worktree_status
    ON managed_worktree_merge_candidates(worktree_id, status, created_at_ms DESC, candidate_id);
