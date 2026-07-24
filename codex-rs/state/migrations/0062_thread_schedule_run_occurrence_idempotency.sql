ALTER TABLE thread_schedule_runs
ADD COLUMN occurrence_id TEXT;

ALTER TABLE thread_schedule_runs
ADD COLUMN materialized_at_ms INTEGER;

ALTER TABLE thread_schedule_runs
ADD COLUMN turn_input TEXT;

UPDATE thread_schedule_runs
SET occurrence_id = run_id
WHERE occurrence_id IS NULL;

UPDATE thread_schedule_runs
SET materialized_at_ms = started_at_ms
WHERE status = 'running'
  AND turn_id IS NOT NULL
  AND materialized_at_ms IS NULL;

UPDATE thread_schedule_runs
SET status = 'failed',
    error = 'superseded by later schedule run during occurrence migration',
    completed_at_ms = COALESCE(
        completed_at_ms,
        (
            SELECT later.started_at_ms
            FROM thread_schedule_runs AS later
            WHERE later.schedule_id = thread_schedule_runs.schedule_id
              AND (
                  later.started_at_ms > thread_schedule_runs.started_at_ms
                  OR (
                      later.started_at_ms = thread_schedule_runs.started_at_ms
                      AND later.rowid > thread_schedule_runs.rowid
                  )
              )
            ORDER BY later.started_at_ms, later.rowid
            LIMIT 1
        )
    )
WHERE status IN ('leased', 'running', 'deferred')
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_runs AS later
      WHERE later.schedule_id = thread_schedule_runs.schedule_id
        AND (
            later.started_at_ms > thread_schedule_runs.started_at_ms
            OR (
                later.started_at_ms = thread_schedule_runs.started_at_ms
                AND later.rowid > thread_schedule_runs.rowid
            )
        )
  );

CREATE INDEX idx_thread_schedule_runs_occurrence
    ON thread_schedule_runs(occurrence_id, started_at_ms DESC);

CREATE UNIQUE INDEX idx_thread_schedule_runs_occurrence_dispatch
    ON thread_schedule_runs(occurrence_id)
    WHERE materialized_at_ms IS NOT NULL;

CREATE TRIGGER thread_schedule_runs_reject_legacy_duplicate_dispatch_before_insert
BEFORE INSERT ON thread_schedule_runs
WHEN NEW.occurrence_id IS NULL
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_runs AS prior
      WHERE prior.schedule_id = NEW.schedule_id
        AND (
            prior.status = 'deferred'
            OR (
                prior.status = 'failed'
                AND prior.scheduled_for_ms IS NEW.scheduled_for_ms
            )
        )
  )
BEGIN
    SELECT RAISE(
        ABORT,
        'legacy schedule runtime cannot replace a dispatched occurrence'
    );
END;

CREATE TRIGGER thread_schedule_runs_fill_occurrence_after_insert
AFTER INSERT ON thread_schedule_runs
WHEN NEW.occurrence_id IS NULL
BEGIN
    UPDATE thread_schedule_runs
    SET occurrence_id = NEW.run_id
    WHERE run_id = NEW.run_id;
END;

CREATE TRIGGER thread_schedule_runs_mark_legacy_dispatch_after_update
AFTER UPDATE OF status, turn_id ON thread_schedule_runs
WHEN NEW.status = 'running'
  AND NEW.turn_id IS NOT NULL
  AND NEW.materialized_at_ms IS NULL
BEGIN
    UPDATE thread_schedule_runs
    SET materialized_at_ms = NEW.started_at_ms
    WHERE run_id = NEW.run_id;
END;

CREATE TRIGGER thread_schedule_runs_settle_legacy_siblings_after_materialization
AFTER UPDATE OF materialized_at_ms ON thread_schedule_runs
WHEN OLD.materialized_at_ms IS NULL
  AND NEW.materialized_at_ms IS NOT NULL
BEGIN
    UPDATE thread_schedule_runs
    SET status = 'failed',
        error = 'superseded by materialized logical occurrence',
        completed_at_ms = NEW.materialized_at_ms
    WHERE run_id != NEW.run_id
      AND occurrence_id = NEW.occurrence_id
      AND materialized_at_ms IS NULL
      AND status IN ('leased', 'running', 'deferred');
END;

CREATE TRIGGER thread_schedule_runs_reject_null_occurrence_before_update
BEFORE UPDATE OF occurrence_id ON thread_schedule_runs
WHEN NEW.occurrence_id IS NULL
BEGIN
    SELECT RAISE(ABORT, 'thread schedule occurrence_id cannot be null');
END;
