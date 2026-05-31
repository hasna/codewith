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

For concurrent sessions, prefer per-session auth profile pinning:

```shell
codex login --with-api-key --auth-profile work
codex login --device-auth --auth-profile personal
codex login --auth-profile personal --use-device-code

codex --auth-profile work
codex --auth-profile personal exec "check status"
```

`--auth-profile <name>` reads and writes credentials directly in `CODEX_HOME/auth_profiles/<name>` for that process. It does not copy credentials into root `auth.json`, and it does not update `auth_profiles/.active`. This lets two TUI, exec, or app-server sessions share one `CODEX_HOME` while using different logged-in accounts.

The same selector is available through environment variables. `IAPPCODEX_AUTH_PROFILE` takes precedence over `CODEX_AUTH_PROFILE`:

```shell
IAPPCODEX_AUTH_PROFILE=work codex
CODEX_AUTH_PROFILE=personal codex exec "who am i logged in as?"
```

For iapp-codex, the npm command is `iappcodex` and the default home is isolated from the general Codex CLI:

```shell
iappcodex login --auth-profile work --use-device-code
iappcodex --auth-profile work
IAPPCODEX_AUTH_PROFILE=personal iappcodex app-server --listen unix://
```

iapp-codex stores state under `~/.hasna/internalapps/codex` unless `CODEX_HOME` or `IAPPCODEX_HOME` is set. It does not read from or seed `~/.codex`.

Available commands:

```shell
codex profile list
codex profile save <name>
codex profile switch <name>
codex profile remove <name>
```

`codex --profile <name>` is still the runtime config-profile flag. Use `codex profile ...` for legacy saved authentication profile management, and use `--auth-profile <name>` when a session must stay pinned to one auth profile without changing the root active login.
