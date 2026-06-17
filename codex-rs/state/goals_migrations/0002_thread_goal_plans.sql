CREATE TABLE thread_goal_plans (
    plan_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'active',
        'paused',
        'blocked',
        'budget_limited',
        'complete'
    )),
    auto_execute TEXT NOT NULL CHECK(auto_execute IN (
        'off',
        'ready_only',
        'ai_directed'
    )),
    max_tokens INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_thread_goal_plans_thread
    ON thread_goal_plans(thread_id, updated_at_ms);

CREATE TABLE thread_goal_plan_nodes (
    node_id TEXT PRIMARY KEY NOT NULL,
    plan_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    key TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    objective TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'pending',
        'active',
        'paused',
        'blocked',
        'usage_limited',
        'budget_limited',
        'complete'
    )),
    token_budget INTEGER,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    time_used_seconds INTEGER NOT NULL DEFAULT 0,
    projected_goal_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY(plan_id) REFERENCES thread_goal_plans(plan_id) ON DELETE CASCADE,
    UNIQUE(plan_id, key)
);

CREATE INDEX idx_thread_goal_plan_nodes_plan_status
    ON thread_goal_plan_nodes(plan_id, status, sequence);

CREATE INDEX idx_thread_goal_plan_nodes_thread_status
    ON thread_goal_plan_nodes(thread_id, status, sequence);

CREATE INDEX idx_thread_goal_plan_nodes_projected_goal
    ON thread_goal_plan_nodes(projected_goal_id);

CREATE TABLE thread_goal_plan_dependencies (
    node_id TEXT NOT NULL,
    depends_on_node_id TEXT NOT NULL,
    PRIMARY KEY(node_id, depends_on_node_id),
    FOREIGN KEY(node_id) REFERENCES thread_goal_plan_nodes(node_id) ON DELETE CASCADE,
    FOREIGN KEY(depends_on_node_id) REFERENCES thread_goal_plan_nodes(node_id) ON DELETE CASCADE
);

CREATE INDEX idx_thread_goal_plan_dependencies_depends_on
    ON thread_goal_plan_dependencies(depends_on_node_id);
