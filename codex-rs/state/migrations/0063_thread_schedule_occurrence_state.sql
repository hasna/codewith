ALTER TABLE thread_schedule_runs
ADD COLUMN deferral_kind TEXT CHECK(deferral_kind IS NULL OR deferral_kind IN ('idle', 'capacity'));

UPDATE thread_schedule_runs
SET deferral_kind = 'idle'
WHERE status = 'deferred'
  AND error IN (
      'scheduled thread is busy',
      'scheduled thread has pending mailbox trigger-turn work'
  );

UPDATE thread_schedule_runs
SET deferral_kind = 'capacity'
WHERE status = 'deferred'
  AND deferral_kind IS NULL;

UPDATE thread_schedule_runs
SET status = 'failed',
    error = 'superseded by later active schedule run during occurrence migration',
    completed_at_ms = COALESCE(completed_at_ms, started_at_ms)
WHERE status IN ('leased', 'running')
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_runs AS later
      WHERE later.schedule_id = thread_schedule_runs.schedule_id
        AND later.status IN ('leased', 'running')
        AND (
            later.started_at_ms > thread_schedule_runs.started_at_ms
            OR (
                later.started_at_ms = thread_schedule_runs.started_at_ms
                AND later.rowid > thread_schedule_runs.rowid
            )
        )
  );

CREATE TABLE thread_schedule_occurrences (
    occurrence_id TEXT PRIMARY KEY NOT NULL,
    schedule_id TEXT NOT NULL UNIQUE,
    thread_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('waiting_idle', 'enqueued', 'started', 'terminal')),
    turn_id TEXT NOT NULL,
    goal_id TEXT,
    auth_profile_recorded INTEGER NOT NULL DEFAULT 0 CHECK(auth_profile_recorded IN (0, 1)),
    auth_profile TEXT,
    scheduled_for_ms INTEGER,
    retry_at_ms INTEGER,
    turn_input TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY(schedule_id) REFERENCES thread_schedules(schedule_id) ON DELETE CASCADE
);

INSERT INTO thread_schedule_occurrences (
    occurrence_id,
    schedule_id,
    thread_id,
    state,
    turn_id,
    goal_id,
    scheduled_for_ms,
    retry_at_ms,
    turn_input,
    created_at_ms,
    updated_at_ms
)
SELECT
    run_id,
    schedule_id,
    thread_id,
    CASE status WHEN 'running' THEN 'started' ELSE 'waiting_idle' END,
    COALESCE(turn_id, run_id),
    goal_id,
    scheduled_for_ms,
    NULL,
    NULL,
    started_at_ms,
    started_at_ms
FROM thread_schedule_runs
WHERE status IN ('leased', 'running');

DELETE FROM thread_schedule_runs
WHERE status = 'leased'
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_occurrences
      WHERE thread_schedule_occurrences.occurrence_id = thread_schedule_runs.run_id
        AND thread_schedule_occurrences.state = 'waiting_idle'
  );

CREATE INDEX idx_thread_schedule_occurrences_ready
    ON thread_schedule_occurrences(state, retry_at_ms, updated_at_ms);

CREATE TRIGGER thread_schedule_runs_reject_legacy_duplicate_occurrence_insert
BEFORE INSERT ON thread_schedule_runs
WHEN (
    NEW.status IN ('leased', 'running')
    AND NOT EXISTS (
        SELECT 1
        FROM thread_schedule_occurrences
        WHERE thread_schedule_occurrences.schedule_id = NEW.schedule_id
          AND thread_schedule_occurrences.occurrence_id = NEW.run_id
          AND thread_schedule_occurrences.state != 'terminal'
    )
) OR EXISTS (
        SELECT 1
        FROM thread_schedule_occurrences
        WHERE thread_schedule_occurrences.schedule_id = NEW.schedule_id
          AND (
              thread_schedule_occurrences.occurrence_id != NEW.run_id
              OR thread_schedule_occurrences.state = 'terminal'
          )
    )
BEGIN
    SELECT RAISE(ABORT, 'active schedule occurrence must be reused');
END;

CREATE TRIGGER thread_schedule_occurrences_follow_legacy_terminal_update
AFTER UPDATE OF status ON thread_schedule_runs
WHEN NEW.status IN ('deferred', 'completed', 'failed')
 AND EXISTS (
     SELECT 1
     FROM thread_schedules
     WHERE thread_schedules.schedule_id = NEW.schedule_id
       AND thread_schedules.lease_id IS NULL
 )
BEGIN
    UPDATE thread_schedule_occurrences
    SET state = 'terminal', updated_at_ms = COALESCE(NEW.completed_at_ms, updated_at_ms)
    WHERE occurrence_id = NEW.run_id;

    DELETE FROM thread_schedule_occurrences
    WHERE occurrence_id = NEW.run_id
      AND EXISTS (
          SELECT 1
          FROM thread_schedules
          WHERE thread_schedules.schedule_id = thread_schedule_occurrences.schedule_id
            AND thread_schedules.lease_id IS NULL
      );
END;

CREATE TRIGGER thread_schedule_occurrences_follow_legacy_schedule_hold
AFTER UPDATE OF status, lease_id ON thread_schedules
WHEN NEW.status IN ('paused', 'expired')
 AND NEW.lease_id IS NULL
 AND EXISTS (
     SELECT 1
     FROM thread_schedule_occurrences
     WHERE thread_schedule_occurrences.schedule_id = NEW.schedule_id
 )
BEGIN
    UPDATE thread_schedule_runs
    SET status = 'failed',
        error = CASE NEW.status
            WHEN 'paused' THEN 'scheduled run cancelled because schedule was paused'
            ELSE 'scheduled run cancelled because schedule expired'
        END,
        completed_at_ms = COALESCE(completed_at_ms, NEW.updated_at_ms)
    WHERE run_id IN (
        SELECT occurrence_id
        FROM thread_schedule_occurrences
        WHERE schedule_id = NEW.schedule_id AND state = 'started'
    )
      AND status = 'running';

    INSERT INTO thread_schedule_runs (
        run_id,
        schedule_id,
        thread_id,
        status,
        lease_id,
        turn_id,
        goal_id,
        error,
        scheduled_for_ms,
        started_at_ms,
        completed_at_ms
    )
    SELECT
        occurrence_id,
        schedule_id,
        thread_id,
        'failed',
        COALESCE(OLD.lease_id, 'legacy-schedule-hold'),
        turn_id,
        goal_id,
        CASE NEW.status
            WHEN 'paused' THEN 'scheduled run cancelled because schedule was paused'
            ELSE 'scheduled run cancelled because schedule expired'
        END,
        scheduled_for_ms,
        created_at_ms,
        NEW.updated_at_ms
    FROM thread_schedule_occurrences
    WHERE schedule_id = NEW.schedule_id AND state = 'enqueued';

    DELETE FROM thread_schedule_occurrences
    WHERE schedule_id = NEW.schedule_id;
END;
