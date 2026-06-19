CREATE TABLE workflow_specs (
    workflow_record_id TEXT PRIMARY KEY NOT NULL,
    spec_workflow_id TEXT NOT NULL,
    source_thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
    schema_version TEXT NOT NULL CHECK(LENGTH(TRIM(schema_version)) > 0),
    display_name TEXT NOT NULL CHECK(LENGTH(TRIM(display_name)) > 0),
    status TEXT NOT NULL CHECK(status IN ('draft', 'needs_clarification', 'blocked')),
    source_yaml TEXT NOT NULL CHECK(LENGTH(TRIM(source_yaml)) > 0),
    source_yaml_sha256 TEXT NOT NULL CHECK(
        LENGTH(source_yaml_sha256) = 64
        AND source_yaml_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    agent_count INTEGER NOT NULL CHECK(agent_count >= 0),
    step_count INTEGER NOT NULL CHECK(step_count >= 0),
    parallel_group_count INTEGER NOT NULL CHECK(parallel_group_count >= 0),
    verifier_count INTEGER NOT NULL CHECK(verifier_count >= 0),
    run_command_verifier_count INTEGER NOT NULL CHECK(run_command_verifier_count >= 0),
    model_routed_step_count INTEGER NOT NULL CHECK(model_routed_step_count >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_workflow_specs_source_thread_spec_workflow_id
    ON workflow_specs(source_thread_id, spec_workflow_id);

CREATE INDEX idx_workflow_specs_source_thread_updated
    ON workflow_specs(source_thread_id, updated_at_ms DESC, workflow_record_id);
