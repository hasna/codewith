CREATE TABLE workflow_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    workflow_record_id TEXT NOT NULL REFERENCES workflow_specs(workflow_record_id) ON DELETE CASCADE,
    source_thread_id TEXT REFERENCES threads(id) ON DELETE CASCADE,
    idempotency_key TEXT,
    spec_workflow_id TEXT NOT NULL CHECK(LENGTH(TRIM(spec_workflow_id)) > 0),
    schema_version TEXT NOT NULL CHECK(LENGTH(TRIM(schema_version)) > 0),
    source_yaml_sha256 TEXT NOT NULL CHECK(
        LENGTH(source_yaml_sha256) = 64
        AND source_yaml_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    status TEXT NOT NULL CHECK(LENGTH(TRIM(status)) > 0),
    status_reason TEXT,
    reason_code TEXT,
    generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
    owner_id TEXT,
    lease_expires_at_ms INTEGER,
    heartbeat_at_ms INTEGER,
    last_event_seq INTEGER NOT NULL DEFAULT 0 CHECK(last_event_seq >= 0),
    agents_json TEXT NOT NULL CHECK(json_valid(agents_json)),
    execution_defaults_json TEXT NOT NULL CHECK(json_valid(execution_defaults_json)),
    limits_json TEXT NOT NULL CHECK(json_valid(limits_json)),
    approvals_json TEXT NOT NULL CHECK(json_valid(approvals_json)),
    artifacts_json TEXT NOT NULL CHECK(json_valid(artifacts_json)),
    cleanup_json TEXT NOT NULL CHECK(json_valid(cleanup_json)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER,
    CHECK(started_at_ms IS NULL OR started_at_ms >= created_at_ms),
    CHECK(completed_at_ms IS NULL OR completed_at_ms >= created_at_ms)
);

CREATE UNIQUE INDEX idx_workflow_runs_workflow_idempotency
    ON workflow_runs(workflow_record_id, idempotency_key);

CREATE INDEX idx_workflow_runs_workflow_updated
    ON workflow_runs(workflow_record_id, updated_at_ms DESC, run_id);

CREATE INDEX idx_workflow_runs_source_thread_updated
    ON workflow_runs(source_thread_id, updated_at_ms DESC, run_id);

CREATE INDEX idx_workflow_runs_source_thread_status_updated
    ON workflow_runs(source_thread_id, status, updated_at_ms DESC, run_id);

CREATE INDEX idx_workflow_runs_status_updated
    ON workflow_runs(status, updated_at_ms DESC, run_id);

CREATE INDEX idx_workflow_runs_owner_lease
    ON workflow_runs(owner_id, lease_expires_at_ms, run_id);

CREATE TABLE workflow_run_steps (
    step_run_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
    step_id TEXT NOT NULL CHECK(LENGTH(TRIM(step_id)) > 0),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    title TEXT NOT NULL CHECK(LENGTH(TRIM(title)) > 0),
    agent_id TEXT NOT NULL CHECK(LENGTH(TRIM(agent_id)) > 0),
    status TEXT NOT NULL CHECK(LENGTH(TRIM(status)) > 0),
    status_reason TEXT,
    reason_code TEXT,
    parallel_group TEXT,
    approval_gate TEXT,
    model_route_json TEXT CHECK(model_route_json IS NULL OR json_valid(model_route_json)),
    workspace_json TEXT CHECK(workspace_json IS NULL OR json_valid(workspace_json)),
    completion_model_marked_state TEXT,
    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER,
    UNIQUE(run_id, step_id),
    CHECK(started_at_ms IS NULL OR started_at_ms >= created_at_ms),
    CHECK(completed_at_ms IS NULL OR completed_at_ms >= created_at_ms)
);

CREATE INDEX idx_workflow_run_steps_run_status_sequence
    ON workflow_run_steps(run_id, status, sequence);

CREATE TABLE workflow_run_step_dependencies (
    run_id TEXT NOT NULL,
    step_id TEXT NOT NULL CHECK(LENGTH(TRIM(step_id)) > 0),
    depends_on_step_id TEXT NOT NULL CHECK(LENGTH(TRIM(depends_on_step_id)) > 0),
    PRIMARY KEY(run_id, step_id, depends_on_step_id),
    FOREIGN KEY(run_id, step_id)
        REFERENCES workflow_run_steps(run_id, step_id)
        ON DELETE CASCADE,
    FOREIGN KEY(run_id, depends_on_step_id)
        REFERENCES workflow_run_steps(run_id, step_id)
        ON DELETE CASCADE,
    CHECK(step_id != depends_on_step_id)
);

CREATE INDEX idx_workflow_run_step_dependencies_parent
    ON workflow_run_step_dependencies(run_id, depends_on_step_id);

CREATE TABLE workflow_run_step_verifiers (
    verifier_run_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    step_id TEXT NOT NULL CHECK(LENGTH(TRIM(step_id)) > 0),
    verifier_id TEXT NOT NULL CHECK(LENGTH(TRIM(verifier_id)) > 0),
    verifier_type TEXT NOT NULL CHECK(LENGTH(TRIM(verifier_type)) > 0),
    status TEXT NOT NULL CHECK(LENGTH(TRIM(status)) > 0),
    status_reason TEXT,
    reason_code TEXT,
    definition_json TEXT NOT NULL CHECK(json_valid(definition_json)),
    last_result_json TEXT CHECK(last_result_json IS NULL OR json_valid(last_result_json)),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
    max_attempts INTEGER CHECK(max_attempts IS NULL OR max_attempts >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    UNIQUE(run_id, step_id, verifier_id),
    FOREIGN KEY(run_id, step_id)
        REFERENCES workflow_run_steps(run_id, step_id)
        ON DELETE CASCADE,
    CHECK(completed_at_ms IS NULL OR completed_at_ms >= created_at_ms)
);

CREATE INDEX idx_workflow_run_step_verifiers_run_status
    ON workflow_run_step_verifiers(run_id, status, verifier_id);

CREATE INDEX idx_workflow_run_step_verifiers_step_status
    ON workflow_run_step_verifiers(run_id, step_id, status, verifier_id);

CREATE TABLE workflow_run_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
    seq INTEGER NOT NULL CHECK(seq >= 1),
    event_type TEXT NOT NULL CHECK(LENGTH(TRIM(event_type)) > 0),
    actor_kind TEXT NOT NULL CHECK(LENGTH(TRIM(actor_kind)) > 0),
    actor_id TEXT,
    step_run_id TEXT REFERENCES workflow_run_steps(step_run_id) ON DELETE SET NULL,
    verifier_run_id TEXT REFERENCES workflow_run_step_verifiers(verifier_run_id) ON DELETE SET NULL,
    visibility TEXT NOT NULL DEFAULT 'internal' CHECK(LENGTH(TRIM(visibility)) > 0),
    event_payload_json TEXT NOT NULL CHECK(json_valid(event_payload_json)),
    created_at_ms INTEGER NOT NULL,
    UNIQUE(run_id, seq)
);

CREATE INDEX idx_workflow_run_events_run_seq
    ON workflow_run_events(run_id, seq);

CREATE INDEX idx_workflow_run_events_created
    ON workflow_run_events(created_at_ms, event_id);
