-- Track explicit user approval decisions for workflow run steps that declare an
-- approval gate. Steps without a gate keep a NULL approval_state and are admitted
-- automatically. Gated steps start life as 'pending' and are only admitted once a
-- user records an 'approved' decision.
ALTER TABLE workflow_run_steps
    ADD COLUMN approval_state TEXT
        CHECK(approval_state IS NULL OR approval_state IN ('pending', 'approved', 'rejected'));

-- Backfill existing gated steps so historical runs surface a pending decision
-- instead of silently stalling.
UPDATE workflow_run_steps
SET approval_state = 'pending'
WHERE approval_gate IS NOT NULL
  AND approval_state IS NULL;

CREATE INDEX idx_workflow_run_steps_run_approval
    ON workflow_run_steps(run_id, approval_state);
