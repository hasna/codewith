ALTER TABLE thread_schedules
    ADD COLUMN parent_schedule_id TEXT REFERENCES thread_schedules(schedule_id) ON DELETE CASCADE;

ALTER TABLE thread_schedules
    ADD COLUMN nesting_depth INTEGER NOT NULL DEFAULT 1 CHECK(nesting_depth BETWEEN 1 AND 3);

CREATE INDEX idx_thread_schedules_parent
    ON thread_schedules(parent_schedule_id);

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

    SELECT RAISE(ABORT, 'invalid nested loop: maximum nesting depth is 3')
    WHERE NEW.parent_schedule_id IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM thread_schedules AS parent
          WHERE parent.schedule_id = NEW.parent_schedule_id
            AND parent.nesting_depth >= 3
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

CREATE TRIGGER thread_schedules_reject_parent_cadence_update_with_children
BEFORE UPDATE OF schedule_kind, interval_amount, interval_unit, cron_expression ON thread_schedules
WHEN EXISTS (
        SELECT 1
        FROM thread_schedules AS child
        WHERE child.parent_schedule_id = OLD.schedule_id
    )
    AND (
        NEW.schedule_kind IS NOT OLD.schedule_kind
        OR NEW.interval_amount IS NOT OLD.interval_amount
        OR NEW.interval_unit IS NOT OLD.interval_unit
        OR NEW.cron_expression IS NOT OLD.cron_expression
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: cannot update loop cadence while it has nested child loops; update or clear child loops first');
END;

CREATE TRIGGER thread_schedules_validate_nested_cadence_update
BEFORE UPDATE OF schedule_kind, interval_amount, interval_unit, cron_expression ON thread_schedules
WHEN NEW.parent_schedule_id IS NOT NULL
    AND (
        NEW.schedule_kind IS NOT OLD.schedule_kind
        OR NEW.interval_amount IS NOT OLD.interval_amount
        OR NEW.interval_unit IS NOT OLD.interval_unit
        OR NEW.cron_expression IS NOT OLD.cron_expression
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid nested loop: nested loop schedules must use dynamic or interval cadence')
    WHERE NEW.schedule_kind IN ('once', 'cron');

    SELECT RAISE(ABORT, 'invalid nested loop: child cadence must be slower than parent cadence')
    WHERE EXISTS (
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
