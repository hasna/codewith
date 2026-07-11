---
name: config-auth
description: "Change, review, or debug Codewith config and authentication. Use for config.toml layering, requirements, /config, /profile, auth profiles, CODEWITH_AUTH_PROFILE, login/logout, credential storage, keyring/file auth, model provider auth, auth-profile auto-switch, and config schema updates."
---

# Config Auth

## Start Here

1. Read `.codewith/CODEWITH.md`.
2. Identify whether the task concerns config parsing, layer precedence, CLI/TUI UX, app-server config RPCs, or credential storage.
3. Do not print, log, diff, or summarize credential values.

## Key Surfaces

- Config types and schema: `codex-rs/config/src/config_toml.rs`, `types.rs`
- Config loading/layers: `codex-rs/config/src/loader/`, `codex-rs/config/src/state.rs`
- Resolved runtime config: `codex-rs/core/src/config/mod.rs`
- Auth storage and profiles: `codex-rs/login/src/auth/profile.rs`, `storage.rs`, `manager.rs`
- CLI login/profile commands: `codex-rs/cli/src/login.rs`, `profile_cmd.rs`, `main.rs`
- TUI settings/profile UX: `codex-rs/tui/src/chatwidget/tests/popups_and_settings.rs`, `status_and_layout.rs`
- App-server config/account API: `codex-rs/app-server/src/config_manager_service.rs`, `codex-rs/app-server-protocol/src/protocol/v2/config.rs`, `account.rs`

## Workflow

1. Map the user-facing path first: CLI flag, environment variable, config file, TUI popup, or app-server RPC.
2. Preserve layer precedence: system and managed requirements constrain user/profile/project/session values.
3. For auth profiles, keep root auth and named auth profile behavior distinct. Validate profile names through existing helpers.
4. For credential persistence, honor `AuthCredentialsStoreMode` and never create a new plaintext path without an explicit product decision.
5. If `ConfigToml` or nested config types change, regenerate `codex-rs/core/config.schema.json`.
6. If app-server protocol fields change, regenerate schema fixtures.

## Validation

For config schema changes:

```bash
cd codex-rs
just write-config-schema
just test-fast -p codex-config
```

For auth/profile changes:

```bash
cd codex-rs
just test-fast -p codex-login
just test-fast -p codex-cli profile
just test-fast -p codex-core auth_profile
```

For TUI or app-server config changes:

```bash
cd codex-rs
just write-app-server-schema
just test-fast -p codex-app-server config
just test-fast -p codex-tui auth_profile
```

## Pitfalls

- `CODEWITH_AUTH_PROFILE` takes precedence before legacy compatibility variables; preserve tests around both.
- `config/read` intentionally mirrors config data and may expose only safe projections.
- Project config and project exec policy depend on trust; do not silently load untrusted rules.
- Login diagnostics may write logs, but credential values must stay redacted.
