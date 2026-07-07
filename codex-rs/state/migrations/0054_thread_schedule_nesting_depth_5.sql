-- Raise the maximum nested loop depth from 3 to 5 while preserving every
-- nesting guard introduced in 0050_thread_schedule_nesting.sql.
--
-- SQLite cannot alter a column CHECK constraint in place, so the
-- nesting_depth column is swapped for one that allows depths 1-5. The two
-- triggers that reference nesting_depth are dropped first and recreated
-- afterwards with the new depth cap; the cadence-guard triggers from 0050 do
-- not reference nesting_depth and stay untouched.

DROP TRIGGER thread_schedules_validate_nesting_insert;
DROP TRIGGER thread_schedules_reject_nesting_identity_update;

ALTER TABLE thread_schedules
    ADD COLUMN nesting_depth_v2 INTEGER NOT NULL DEFAULT 1 CHECK(nesting_depth_v2 BETWEEN 1 AND 5);

UPDATE thread_schedules SET nesting_depth_v2 = nesting_depth;

ALTER TABLE thread_schedules DROP COLUMN nesting_depth;

ALTER TABLE thread_schedules RENAME COLUMN nesting_depth_v2 TO nesting_depth;

CREATE TRIGGER thread_schedules_validate_nesting_insert
BEFORE INSERT ON thread_schedules
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: root nesting depth must be 1')
    WHERE NEW.parent_schedule_id IS NULL AND NEW.nesting_depth != 1;

    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule not found')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
      );

    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule must belong to the same thread')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.thread_id != NEW.thread_id
      );

    SELECT RAISE(ABORT, 'invalid nested loop: parent schedule must be recurring')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.schedule_kind = 'once'
      );

    SELECT RAISE(ABORT, 'invalid nested loop: maximum nesting depth is 5')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.nesting_depth >= 5
      );

    SELECT RAISE(ABORT, 'invalid nested loop: nesting depth must be parent depth plus one')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND NEW.nesting_depth != parent.nesting_depth + 1
      );

    SELECT RAISE(ABORT, 'invalid nested loop: nested loop schedules must use dynamic or interval cadence')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND NEW.schedule_kind IN ('once', 'cron');

    SELECT RAISE(ABORT, 'invalid nested loop: parent cron schedules cannot be nested; use dynamic or interval cadence')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.schedule_kind = 'cron'
      );

    SELECT RAISE(ABORT, 'invalid nested loop: child cadence must be slower than parent cadence')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
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

CREATE TRIGGER thread_schedules_reject_nesting_identity_update
BEFORE UPDATE OF thread_id, parent_schedule_id, nesting_depth ON thread_schedules
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: schedule nesting identity cannot be updated');
END;
