# Authentication

For information about Codewith authentication, see the bundled `codewith login --help` output.

## Auth Profiles

Codewith can save multiple local authentication profiles after normal login:

```shell
codewith login --profile work
codewith login --profile personal
codewith profile list
codewith profile switch work
```

Profiles are named local credential snapshots stored under `CODEWITH_HOME` using the configured credential storage mode. Switching a profile replaces the active local Codewith credentials with that saved profile. It does not bypass login, logout, or account authorization; each profile must be created from a normal successful login.

For concurrent sessions, prefer per-session auth profile pinning:

```shell
codewith login --with-api-key --auth-profile work
codewith login --device-auth --auth-profile personal
codewith login --auth-profile personal --use-device-code

codewith --auth-profile work
codewith --auth-profile personal exec "check status"
```

`--auth-profile <name>` reads and writes credentials directly in `CODEWITH_HOME/auth_profiles/<name>` for that process. It does not copy credentials into root `auth.json`, and it does not update `auth_profiles/.active`. This lets two TUI, exec, or app-server sessions share one `CODEWITH_HOME` while using different logged-in accounts.

The same selector is available through environment variables. `CODEWITH_AUTH_PROFILE` takes precedence over `CODEX_AUTH_PROFILE`:

```shell
CODEWITH_AUTH_PROFILE=work codewith
CODEX_AUTH_PROFILE=personal codewith exec "who am i logged in as?"
```

For Codewith, the npm command is `codewith` and the default home is isolated from the legacy Codex home:

```shell
codewith login --auth-profile work --use-device-code
codewith --auth-profile work
CODEWITH_AUTH_PROFILE=personal codewith app-server --listen unix://
```

Codewith stores state under `~/.codewith` unless `CODEWITH_HOME` is set. Direct native binaries also retain `CODEX_HOME` as a compatibility override. Codewith does not read from or seed `~/.codex`.

Available commands:

```shell
codewith profile list
codewith profile save <name>
codewith profile switch <name>
codewith profile remove <name>
```

`codewith --profile <name>` is still the runtime config-profile flag. Use `codewith profile ...` for saved authentication profile management, and use `--auth-profile <name>` when a session must stay pinned to one auth profile without changing the root active login.
