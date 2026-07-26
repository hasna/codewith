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

Each profile is locked to the subscription provider it was created for. A profile whose provider is not ChatGPT (Claude.ai, Cursor, Grok) can be listed and switched to like any other, but it never lends OpenAI credentials to Codewith model auth: it carries no `auth.json` of its own, refreshed root tokens are not mirrored into it, and any stray `auth.json` left inside it is ignored rather than used. Selecting such a profile leaves Codewith without ChatGPT model auth — the agent is still free to select any provider it has configured.

### Switching vs. pinning

These are two different mechanisms, and the difference matters:

- `codewith profile switch <name>` changes the **root login** for the whole machine and records the choice in `auth_profiles/.active`. For a ChatGPT profile it copies that profile's credentials into the root login. For a provider-locked profile there are no OpenAI credentials to copy, so the root login keeps whatever it held and Codewith simply stops using it for model auth until you switch back to a ChatGPT profile. Switching does **not** scope later processes to that profile: it does not change their sandbox or approval settings, does not disable `CODEX_API_KEY` / `CODEX_ACCESS_TOKEN`, and does not give them a separate app-server socket.
- `--auth-profile <name>`, `CODEWITH_AUTH_PROFILE`, and `CODEX_AUTH_PROFILE` **pin one process** to a profile. A pinned process reads that profile's own credentials, inherits the permission settings saved with it, ignores global env credentials, and gets its own app-server socket namespace. Pins always win over the persisted active profile.

If `auth_profiles/.active` or a profile's `profile.json` becomes unreadable, Codewith cannot prove which profile owns the root login, so it fails closed and declines to use root credentials for model auth. Commands do not fail: `codewith profile list`, `codewith profile switch <name>`, and `codewith login` all keep working so you can repair the marker.

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
