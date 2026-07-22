# Configuration

Codewith reads configuration from `config.toml` under `CODEWITH_HOME`, which
defaults to `~/.codewith`.

```text
~/.codewith/config.toml
```

Use `/config` in the TUI for interactive configuration, or edit the file
directly.

## Common Settings

```toml
model = "gpt-5"
approval_policy = "on-request"
sandbox_mode = "workspace-write"

[history]
persistence = "save-all"

[analytics]
enabled = true
```

## Agent subagent threads

`[agents] max_threads` caps how many subagent threads a single agent run may keep
open concurrently. When unset it defaults to the built-in limit (`6`), so leaving it
out preserves the current behavior. It must be at least `1`.

```toml
[agents]
max_threads = 4
```

This setting applies to the stable multi-agent mode. It cannot be combined with the
experimental `multi_agent_v2` feature (which manages its own
`max_concurrent_threads_per_session`); setting both is rejected at startup.

You can also change it interactively from `/config` in the TUI ("Agent subagent
threads"), which shows the current value and writes `[agents] max_threads` for you;
restart the session to apply the new limit.

Use CLI overrides for one run:

```shell
codewith --model gpt-5
codewith --profile work
codewith exec --model gpt-5 "summarize this repo"
codewith -c agents.max_threads=4
```

Any config key can be overridden for a single run with `-c <dotted.key>=<value>`
(the value is parsed as TOML), so `-c agents.max_threads=4` is equivalent to the
`[agents]` block above without editing `config.toml`.

`codewith --profile <name>` selects a runtime configuration profile. Auth
profiles are separate; use `--auth-profile <name>` or `codewith profile ...`
for credential profiles.

## Goal plans

`[goals] max_goal_plan_node_objective_chars` controls how many characters of each
goal-plan node objective Codewith echoes back to the model (in `create_goal_plan`
tool responses, plan events, and completion reports). Larger values let a full,
detailed objective show without being clipped.

| Key                                 | Default | Behavior                                                                                                    |
| ----------------------------------- | ------- | ----------------------------------------------------------------------------------------------------------- |
| `max_goal_plan_node_objective_chars` | `4000`  | Max characters of a goal-plan node objective echoed to the model (~600 words). Clamped to a ceiling of `8000`. |

```toml
[goals]
max_goal_plan_node_objective_chars = 4000
```

You can also change it interactively from `/config` in the TUI ("Goal objective
limit"), which shows the current value and writes
`[goals] max_goal_plan_node_objective_chars` for you; restart the session to apply
the new limit. Override it for a single run with
`-c goals.max_goal_plan_node_objective_chars=6000`.

## Usage limits & automatic recovery

Codewith can keep a session moving when it hits Codewith usage limits or transient
availability errors. These recovery behaviors are opt-in and can also be toggled from
`/config` in the TUI. Every value below matches the built-in default; omit a key to keep
the default.

### `[usage_limit]`

Controls the "auto on/off" banked-reset behavior for the weekly usage limit.

| Key                  | Default | Behavior                                                                                                             |
| -------------------- | ------- | -------------------------------------------------------------------------------------------------------------------- |
| `auto_reset_enabled` | `false` | When enabled, Codewith may consume one available reset credit after it confirms the weekly usage limit is exhausted. |

```toml
[usage_limit]
auto_reset_enabled = false
```

### `[usage_self_heal]`

Automatic retry for recoverable usage-limit and transient availability errors.

| Key                          | Default | Behavior                                                                                             |
| ---------------------------- | ------- | ---------------------------------------------------------------------------------------------------- |
| `enabled`                    | `false` | Enables automatic retry for recoverable usage and transient availability errors.                     |
| `max_retries`                | `3`     | Maximum automatic retry attempts per failing turn.                                                   |
| `initial_backoff_secs`       | `30`    | Initial retry delay for transient errors, or usage errors without reset metadata (minimum 1 second). |
| `max_backoff_secs`           | `300`   | Ceiling for exponential backoff (5 minutes); never drops below the initial backoff.                  |
| `reset_retry_buffer_secs`    | `60`    | Extra seconds to wait after a usage-limit reset timestamp before retrying.                           |
| `max_reset_retry_delay_secs` | `86400` | Longest reset-based delay Codewith schedules automatically (24 hours).                               |

```toml
[usage_self_heal]
enabled = false
max_retries = 3
initial_backoff_secs = 30
max_backoff_secs = 300
reset_retry_buffer_secs = 60
max_reset_retry_delay_secs = 86400
```

### `[keep_going]`

Opt-in keep-going / auto-resume. When enabled, after a clean turn-end (the model
returned a final message and the session would otherwise stop) Codewith injects a
neutral continuation prompt and automatically starts the next turn. It is bounded
per user turn and never bypasses approvals, the sandbox, or any refusal: the
continued turn is a normal turn where every tool call still passes all enforcement.
The interactive `/keep-going` toggle overrides this default for the active session.

| Key                 | Default | Behavior                                                                                                             |
| ------------------- | ------- | -------------------------------------------------------------------------------------------------------------------- |
| `enabled`           | `false` | Enables keep-going / auto-resume by default (overridden per session by `/keep-going`).                               |
| `max_continuations` | `25`    | Hard cap on automatic continuations per user turn so keep-going can never loop forever. Resets on each user message. |
| `prompt`            | (unset) | Optional continuation prompt. When unset, a built-in neutral continuation template is used.                          |

```toml
[keep_going]
enabled = false
max_continuations = 25
```

### `[auth_profile_auto_switch]`

Switches to another saved auth profile when the selected Codewith rate-limit windows are
fully exhausted.

| Key                        | Default             | Behavior                                                                                                                              |
| -------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `enabled`                  | `false`             | Enables runtime switching to another auth profile when selected rate-limit windows are fully exhausted.                               |
| `profiles`                 | `[]`                | Preferred profile order; when empty, saved auth profiles are used in sorted name order.                                               |
| `on_5h_limit`              | `true`              | Switch when the 5h Codewith window reaches 100%.                                                                                      |
| `on_weekly_limit`          | `true`              | Switch when the weekly Codewith window reaches 100%.                                                                                  |
| `strategy`                 | `highest_available` | Next-profile strategy: `highest_available` prefers the profile with the most remaining limit; `ordered` follows the configured order. |
| `heartbeat_interval_secs`  | `60`                | Seconds between background usage heartbeat checks (minimum 60).                                                                       |
| `heartbeat_freshness_secs` | `120`               | Maximum age (seconds) of usage data used to guide selection; clamped to at least the heartbeat interval.                              |

```toml
[auth_profile_auto_switch]
enabled = false
on_5h_limit = true
on_weekly_limit = true
strategy = "highest_available"
heartbeat_interval_secs = 60
heartbeat_freshness_secs = 120
```

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
