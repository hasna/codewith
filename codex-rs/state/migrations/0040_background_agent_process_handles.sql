ALTER TABLE background_agent_process_leases
    ADD COLUMN start_token TEXT;

ALTER TABLE background_agent_process_leases
    ADD COLUMN stderr_log_path TEXT;
