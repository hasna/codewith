ALTER TABLE managed_worktrees ADD COLUMN worktree_path_key TEXT;

CREATE INDEX idx_managed_worktrees_live_isolated_path_key
    ON managed_worktrees(worktree_path_key)
    WHERE mode = 'isolated_worktree'
      AND deleted_at_ms IS NULL
      AND worktree_path_key IS NOT NULL;

CREATE TRIGGER reject_live_isolated_worktree_path_key_collision
BEFORE INSERT ON managed_worktrees
WHEN NEW.mode = 'isolated_worktree'
  AND NEW.deleted_at_ms IS NULL
  AND NEW.worktree_path_key IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM managed_worktrees
      WHERE mode = 'isolated_worktree'
        AND deleted_at_ms IS NULL
        AND worktree_path_key = NEW.worktree_path_key
  )
BEGIN
    SELECT RAISE(ABORT, 'managed worktree admission rejected: normalized isolated worktree path is already live');
END;
