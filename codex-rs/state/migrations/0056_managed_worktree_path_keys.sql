ALTER TABLE managed_worktrees ADD COLUMN worktree_path_key TEXT;

CREATE INDEX idx_managed_worktrees_live_isolated_path_key
    ON managed_worktrees(worktree_path_key)
    WHERE mode = 'isolated_worktree'
      AND deleted_at_ms IS NULL
      AND worktree_path_key IS NOT NULL;
