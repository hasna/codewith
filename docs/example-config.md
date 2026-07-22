# Sample configuration

Codewith reads `config.toml` from `CODEWITH_HOME`, which defaults to
`~/.codewith`.

```toml
model = "gpt-5"
approval_policy = "on-request"
sandbox_mode = "workspace-write"

[history]
persistence = "save-all"

[analytics]
enabled = true

[feedback]
enabled = true

# Cap concurrent subagent threads per agent run (default: 6, minimum: 1).
[agents]
max_threads = 4

# Max characters of a goal-plan node objective echoed to the model
# (default: 4000, ~600 words; clamped to a ceiling of 8000).
[goals]
max_goal_plan_node_objective_chars = 4000

# Consume one banked reset credit after the weekly usage limit is exhausted.
[usage_limit]
auto_reset_enabled = false

# Automatic retry for recoverable usage-limit and transient availability errors.
[usage_self_heal]
enabled = false
max_retries = 3
initial_backoff_secs = 30
max_backoff_secs = 300
reset_retry_buffer_secs = 60
max_reset_retry_delay_secs = 86400

# Opt-in keep-going / auto-resume: after a clean turn-end, inject a neutral
# continuation prompt and start the next turn (bounded, never bypasses approvals).
[keep_going]
enabled = false
max_continuations = 25

# Switch to another saved auth profile when rate-limit windows are exhausted.
[auth_profile_auto_switch]
enabled = false
on_5h_limit = true
on_weekly_limit = true
strategy = "highest_available"
heartbeat_interval_secs = 60
heartbeat_freshness_secs = 120
```

Use `/config` in the TUI to edit common settings interactively.
