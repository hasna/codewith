ALTER TABLE thread_goal_plan_nodes
    ADD COLUMN parent_node_id TEXT;

ALTER TABLE thread_goal_plan_nodes
    ADD COLUMN nesting_depth INTEGER NOT NULL DEFAULT 1;

CREATE INDEX idx_thread_goal_plan_nodes_parent
    ON thread_goal_plan_nodes(parent_node_id);
