CREATE TABLE managed_worktree_path_key_backfill_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_scanned_worktree_id TEXT
);

CREATE TABLE managed_worktree_path_key_backfill_terminal (
    worktree_id TEXT PRIMARY KEY,
    base_repo_path_terminal INTEGER NOT NULL DEFAULT 0 CHECK (base_repo_path_terminal IN (0, 1)),
    worktree_path_terminal INTEGER NOT NULL DEFAULT 0 CHECK (worktree_path_terminal IN (0, 1))
);
