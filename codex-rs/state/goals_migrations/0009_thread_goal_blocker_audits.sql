CREATE TABLE thread_goal_blocker_audits (
    thread_id TEXT PRIMARY KEY NOT NULL,
    goal_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK(
        length(fingerprint) > 0
        AND length(fingerprint) <= 256
    ),
    -- Host-owned goal turn identifiers for the current fingerprint streak.
    -- These are not upstream transport request identifiers. The child table
    -- below durably holds every distinct id counted in this bounded streak.
    first_turn_id TEXT NOT NULL CHECK(
        length(first_turn_id) > 0
        AND length(first_turn_id) <= 256
    ),
    last_turn_id TEXT NOT NULL CHECK(
        length(last_turn_id) > 0
        AND length(last_turn_id) <= 256
    ),
    consecutive_turns INTEGER NOT NULL CHECK(
        consecutive_turns >= 1
        AND consecutive_turns <= 3
    ),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY(thread_id) REFERENCES thread_goals(thread_id) ON DELETE CASCADE
);

CREATE TABLE thread_goal_blocker_audit_turns (
    thread_id TEXT NOT NULL,
    goal_id TEXT NOT NULL,
    turn_id TEXT NOT NULL CHECK(
        length(turn_id) > 0
        AND length(turn_id) <= 256
    ),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY(thread_id, goal_id, turn_id),
    FOREIGN KEY(thread_id) REFERENCES thread_goal_blocker_audits(thread_id) ON DELETE CASCADE
);

CREATE TRIGGER clear_thread_goal_blocker_audit_after_lifecycle_change
AFTER UPDATE OF goal_id, objective, title, status, token_budget ON thread_goals
WHEN OLD.goal_id IS NOT NEW.goal_id
    OR OLD.objective IS NOT NEW.objective
    OR OLD.title IS NOT NEW.title
    OR OLD.token_budget IS NOT NEW.token_budget
    OR (
        OLD.status IS NOT NEW.status
        AND NOT (
            OLD.status = 'paused' AND NEW.status = 'active'
        )
    )
BEGIN
    DELETE FROM thread_goal_blocker_audits
    WHERE thread_id = NEW.thread_id;
END;
