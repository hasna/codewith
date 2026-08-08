ALTER TABLE workflow_runs
    ADD COLUMN source_cwd TEXT;

ALTER TABLE workflow_runs
    ADD COLUMN source_repo_path TEXT;

ALTER TABLE workflow_runs
    ADD COLUMN owner_instance_id TEXT;

CREATE INDEX idx_workflow_runs_owner_instance_lease
    ON workflow_runs(owner_id, owner_instance_id, lease_expires_at_ms, run_id);
