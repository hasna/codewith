ALTER TABLE thread_schedule_runs
ADD COLUMN goal_id TEXT;

CREATE INDEX idx_thread_schedule_runs_running_turn
    ON thread_schedule_runs(thread_id, turn_id)
    WHERE status = 'running' AND turn_id IS NOT NULL;
