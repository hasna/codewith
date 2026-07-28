ALTER TABLE thread_goals
ADD COLUMN lines_added INTEGER NOT NULL DEFAULT 0;

ALTER TABLE thread_goals
ADD COLUMN lines_deleted INTEGER NOT NULL DEFAULT 0;

ALTER TABLE thread_goal_plan_nodes
ADD COLUMN lines_added INTEGER NOT NULL DEFAULT 0;

ALTER TABLE thread_goal_plan_nodes
ADD COLUMN lines_deleted INTEGER NOT NULL DEFAULT 0;
