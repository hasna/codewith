ALTER TABLE workflow_run_steps
    ADD COLUMN background_agent_run_id TEXT;

ALTER TABLE workflow_run_steps
    ADD COLUMN branch_admission_json TEXT CHECK(
        branch_admission_json IS NULL OR json_valid(branch_admission_json)
    );

CREATE UNIQUE INDEX idx_workflow_run_steps_background_agent_run_id
    ON workflow_run_steps(background_agent_run_id)
    WHERE background_agent_run_id IS NOT NULL;

CREATE INDEX idx_workflow_run_steps_run_background_agent
    ON workflow_run_steps(run_id, background_agent_run_id)
    WHERE background_agent_run_id IS NOT NULL;

INSERT INTO managed_worktree_assignments (
    assignment_id,
    worktree_id,
    thread_id,
    agent_run_id,
    attached_at_ms,
    detached_at_ms
)
SELECT
    LOWER(HEX(RANDOMBLOB(16))),
    worktree_id,
    NULL,
    owner_agent_run_id,
    created_at_ms,
    NULL
FROM managed_worktrees
WHERE owner_agent_run_id IS NOT NULL
  AND lifecycle_status = 'active'
  AND deleted_at_ms IS NULL
  AND NOT EXISTS (
      SELECT 1
      FROM managed_worktree_assignments AS assignment
      WHERE assignment.worktree_id = managed_worktrees.worktree_id
        AND assignment.detached_at_ms IS NULL
  );
