ALTER TABLE managed_worktrees
    ADD COLUMN base_repo_path_key BLOB;

ALTER TABLE managed_worktrees
    ADD COLUMN worktree_path_key BLOB;
