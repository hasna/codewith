ALTER TABLE thread_goal_plan_nodes
    ADD COLUMN assigned_thread_id TEXT;

UPDATE thread_goal_plan_nodes
SET assigned_thread_id = thread_id
WHERE assigned_thread_id IS NULL;

CREATE INDEX idx_thread_goal_plan_nodes_assigned_status
    ON thread_goal_plan_nodes(assigned_thread_id, status, sequence);
