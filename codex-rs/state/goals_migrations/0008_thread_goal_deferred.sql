-- no-transaction
PRAGMA foreign_keys=OFF;

CREATE TABLE thread_goals_new (
    thread_id TEXT PRIMARY KEY NOT NULL,
    goal_id TEXT NOT NULL,
    objective TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'active',
        'paused',
        'blocked',
        'usage_limited',
        'budget_limited',
        'deferred',
        'complete',
        'cancelled'
    )),
    token_budget INTEGER,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    time_used_seconds INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    title TEXT
);

INSERT INTO thread_goals_new (
    thread_id,
    goal_id,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms,
    title
)
SELECT
    thread_id,
    goal_id,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms,
    title
FROM thread_goals;

DROP TABLE thread_goals;
ALTER TABLE thread_goals_new RENAME TO thread_goals;

CREATE TABLE thread_goal_plan_nodes_new (
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
        'deferred',
        'complete',
        'cancelled'
    )),
    token_budget INTEGER,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    time_used_seconds INTEGER NOT NULL DEFAULT 0,
    projected_goal_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    assigned_thread_id TEXT,
    title TEXT,
    FOREIGN KEY(plan_id) REFERENCES thread_goal_plans(plan_id) ON DELETE CASCADE,
    UNIQUE(plan_id, key)
);

INSERT INTO thread_goal_plan_nodes_new (
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms,
    assigned_thread_id,
    title
)
SELECT
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms,
    assigned_thread_id,
    title
FROM thread_goal_plan_nodes;

DROP TABLE thread_goal_plan_nodes;
ALTER TABLE thread_goal_plan_nodes_new RENAME TO thread_goal_plan_nodes;

CREATE INDEX idx_thread_goal_plan_nodes_plan_status
    ON thread_goal_plan_nodes(plan_id, status, sequence);

CREATE INDEX idx_thread_goal_plan_nodes_thread_status
    ON thread_goal_plan_nodes(thread_id, status, sequence);

CREATE INDEX idx_thread_goal_plan_nodes_projected_goal
    ON thread_goal_plan_nodes(projected_goal_id);

CREATE INDEX idx_thread_goal_plan_nodes_assigned_status
    ON thread_goal_plan_nodes(assigned_thread_id, status, sequence);

PRAGMA foreign_keys=ON;
