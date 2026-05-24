# Authentication

For information about Codex CLI authentication, see [this documentation](https://developers.openai.com/codex/auth).

## Auth Profiles

Codex can save multiple local authentication profiles after normal login:

```shell
codex login --profile work
codex login --profile personal
codex profile list
codex profile switch work
```

Profiles are named local credential snapshots stored under `CODEX_HOME` using the configured credential storage mode. Switching a profile replaces the active local Codex credentials with that saved profile. It does not bypass login, logout, or account authorization; each profile must be created from a normal successful login.

Available commands:

```shell
codex profile list
codex profile save <name>
codex profile switch <name>
codex profile remove <name>
```

`codex --profile <name>` is still the runtime config-profile flag. Use `codex profile ...` for authentication profiles.
