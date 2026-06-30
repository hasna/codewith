ALTER TABLE thread_schedules
    ADD COLUMN parent_schedule_id TEXT REFERENCES thread_schedules(schedule_id) ON DELETE CASCADE;

ALTER TABLE thread_schedules
    ADD COLUMN nesting_depth INTEGER NOT NULL DEFAULT 1 CHECK(nesting_depth BETWEEN 1 AND 5);

CREATE INDEX idx_thread_schedules_parent
    ON thread_schedules(parent_schedule_id);

CREATE TRIGGER validate_thread_schedule_nesting_insert
BEFORE INSERT ON thread_schedules
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: root schedule depth must be 1')
    WHERE NEW.parent_schedule_id IS NULL AND NEW.nesting_depth != 1;

    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule not found')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1
          FROM thread_schedules parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
      );

    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule must belong to the same thread')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.thread_id != NEW.thread_id
      );

    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule must use dynamic or interval cadence')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.schedule_kind NOT IN ('dynamic', 'interval')
      );

    SELECT RAISE(ABORT, 'invalid nested loop: child schedule must use dynamic or interval cadence')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND NEW.schedule_kind NOT IN ('dynamic', 'interval');

    SELECT RAISE(ABORT, 'invalid nested loop: maximum nesting depth is 5')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.nesting_depth >= 5
      );

    SELECT RAISE(ABORT, 'invalid nested loop: nesting depth must be parent depth plus one')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND NEW.nesting_depth != parent.nesting_depth + 1
      );

    SELECT RAISE(ABORT, 'invalid nested loop: child cadence must be slower than parent cadence')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND (
                CASE NEW.schedule_kind
                    WHEN 'dynamic' THEN 60
                    WHEN 'interval' THEN NEW.interval_amount * CASE NEW.interval_unit
                        WHEN 'minutes' THEN 60
                        WHEN 'hours' THEN 3600
                        WHEN 'days' THEN 86400
                    END
                END
            ) <= (
                CASE parent.schedule_kind
                    WHEN 'dynamic' THEN 60
                    WHEN 'interval' THEN parent.interval_amount * CASE parent.interval_unit
                        WHEN 'minutes' THEN 60
                        WHEN 'hours' THEN 3600
                        WHEN 'days' THEN 86400
                    END
                END
            )
      );
END;

CREATE TRIGGER validate_thread_schedule_nesting_immutable_update
BEFORE UPDATE OF thread_id, parent_schedule_id, nesting_depth ON thread_schedules
FOR EACH ROW
WHEN OLD.thread_id != NEW.thread_id
  OR OLD.parent_schedule_id IS NOT NEW.parent_schedule_id
  OR OLD.nesting_depth != NEW.nesting_depth
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule and nesting depth are immutable');
END;

CREATE TRIGGER validate_thread_schedule_parent_cadence_update
BEFORE UPDATE OF schedule_kind, interval_amount, interval_unit, cron_expression ON thread_schedules
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM thread_schedules child
    WHERE child.parent_schedule_id = OLD.schedule_id
)
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule with child loops must use dynamic or interval cadence')
    WHERE NEW.schedule_kind NOT IN ('dynamic', 'interval');

    SELECT RAISE(ABORT, 'invalid nested loop: child cadence must be slower than parent cadence')
    WHERE EXISTS (
        SELECT 1
        FROM thread_schedules child
        WHERE child.parent_schedule_id = OLD.schedule_id
          AND (
              CASE child.schedule_kind
                  WHEN 'dynamic' THEN 60
                  WHEN 'interval' THEN child.interval_amount * CASE child.interval_unit
                      WHEN 'minutes' THEN 60
                      WHEN 'hours' THEN 3600
                      WHEN 'days' THEN 86400
                  END
              END
          ) <= (
              CASE NEW.schedule_kind
                  WHEN 'dynamic' THEN 60
                  WHEN 'interval' THEN NEW.interval_amount * CASE NEW.interval_unit
                      WHEN 'minutes' THEN 60
                      WHEN 'hours' THEN 3600
                      WHEN 'days' THEN 86400
                  END
              END
          )
    );
END;

CREATE TRIGGER validate_thread_schedule_child_cadence_update
BEFORE UPDATE OF schedule_kind, interval_amount, interval_unit, cron_expression ON thread_schedules
FOR EACH ROW
WHEN OLD.parent_schedule_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: child schedule must use dynamic or interval cadence')
    WHERE NEW.schedule_kind NOT IN ('dynamic', 'interval');

    SELECT RAISE(ABORT, 'invalid nested loop: child cadence must be slower than parent cadence')
    WHERE EXISTS (
        SELECT 1
        FROM thread_schedules parent
        WHERE parent.schedule_id = OLD.parent_schedule_id
          AND (
              CASE NEW.schedule_kind
                  WHEN 'dynamic' THEN 60
                  WHEN 'interval' THEN NEW.interval_amount * CASE NEW.interval_unit
                      WHEN 'minutes' THEN 60
                      WHEN 'hours' THEN 3600
                      WHEN 'days' THEN 86400
                  END
              END
          ) <= (
              CASE parent.schedule_kind
                  WHEN 'dynamic' THEN 60
                  WHEN 'interval' THEN parent.interval_amount * CASE parent.interval_unit
                      WHEN 'minutes' THEN 60
                      WHEN 'hours' THEN 3600
                      WHEN 'days' THEN 86400
                  END
              END
          )
    );
END;
