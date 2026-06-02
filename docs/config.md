# Configuration

For basic Codewith configuration instructions, see the [upstream compatibility reference](https://developers.openai.com/codex/config-basic).

For advanced Codewith configuration instructions, see the [upstream compatibility reference](https://developers.openai.com/codex/config-advanced).

For the full configuration reference, see the [upstream compatibility reference](https://developers.openai.com/codex/config-reference).

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
