CREATE INDEX idx_managed_worktrees_base_repo_path_key
    ON managed_worktrees(base_repo_path_key)
    WHERE base_repo_path_key IS NOT NULL;

CREATE INDEX idx_managed_worktrees_worktree_path_key
    ON managed_worktrees(worktree_path_key)
    WHERE worktree_path_key IS NOT NULL;
