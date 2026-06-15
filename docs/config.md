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

Use CLI overrides for one run:

```shell
codewith --model gpt-5
codewith --profile work
codewith exec --model gpt-5 "summarize this repo"
```

`codewith --profile <name>` selects a runtime configuration profile. Auth
profiles are separate; use `--auth-profile <name>` or `codewith profile ...`
for credential profiles.

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
