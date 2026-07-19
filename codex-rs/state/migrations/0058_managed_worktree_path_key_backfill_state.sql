CREATE TABLE managed_worktree_path_key_backfill_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_scanned_worktree_id TEXT,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1))
);
