CREATE TABLE workflow_goal_plan_projections (
    projection_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL UNIQUE,
    thread_id TEXT NOT NULL,
    plan_id TEXT NOT NULL UNIQUE,
    idempotency_key TEXT,
    status TEXT NOT NULL CHECK(status IN ('projected')),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY(plan_id) REFERENCES thread_goal_plans(plan_id) ON DELETE CASCADE
);

CREATE INDEX idx_workflow_goal_plan_projections_thread
    ON workflow_goal_plan_projections(thread_id, updated_at_ms);

CREATE INDEX idx_workflow_goal_plan_projections_idempotency
    ON workflow_goal_plan_projections(run_id, idempotency_key);

CREATE TABLE workflow_goal_plan_node_projections (
    projection_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    step_run_id TEXT NOT NULL,
    node_id TEXT NOT NULL UNIQUE,
    agent_id TEXT NOT NULL,
    model_route_json TEXT CHECK(model_route_json IS NULL OR json_valid(model_route_json)),
    workspace_json TEXT CHECK(workspace_json IS NULL OR json_valid(workspace_json)),
    verifier_summary_json TEXT NOT NULL CHECK(json_valid(verifier_summary_json)),
    time_budget_seconds INTEGER,
    token_budget INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(projection_id, step_id),
    FOREIGN KEY(projection_id) REFERENCES workflow_goal_plan_projections(projection_id) ON DELETE CASCADE,
    FOREIGN KEY(node_id) REFERENCES thread_goal_plan_nodes(node_id) ON DELETE CASCADE
);

CREATE INDEX idx_workflow_goal_plan_node_projections_run_step
    ON workflow_goal_plan_node_projections(run_id, step_id);
