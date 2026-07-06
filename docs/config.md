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

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
