CREATE TABLE thread_goal_context_lifecycle (
    thread_id TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK(target_kind IN ('goal', 'goal_plan')),
    target_id TEXT NOT NULL,
    post_goal_action TEXT NOT NULL CHECK(post_goal_action IN (
        'keep',
        'compact'
    )),
    post_goal_plan_action TEXT CHECK(
        post_goal_plan_action IS NULL
        OR post_goal_plan_action IN (
            'keep',
            'compact'
        )
    ),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(thread_id, target_kind, target_id)
);
