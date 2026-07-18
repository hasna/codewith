CREATE INDEX idx_managed_worktrees_active_shared_repo_path_key
    ON managed_worktrees(worktree_path_key)
    WHERE mode = 'shared_repository'
      AND deleted_at_ms IS NULL
      AND released_at_ms IS NULL
      AND lifecycle_status = 'active'
      AND worktree_path_key IS NOT NULL;

CREATE TRIGGER reject_active_shared_repo_path_key_collision
BEFORE INSERT ON managed_worktrees
WHEN NEW.mode = 'shared_repository'
  AND NEW.deleted_at_ms IS NULL
  AND NEW.released_at_ms IS NULL
  AND NEW.lifecycle_status = 'active'
  AND NEW.worktree_path_key IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM managed_worktrees
      WHERE mode = 'shared_repository'
        AND deleted_at_ms IS NULL
        AND released_at_ms IS NULL
        AND lifecycle_status = 'active'
        AND worktree_path_key = NEW.worktree_path_key
  )
BEGIN
    SELECT RAISE(ABORT, 'managed worktree admission rejected: normalized shared repository path is already active');
END;
