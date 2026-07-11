# Validation And Landing

Run repo Rust commands through the root `justfile`; it sets `codex-rs` as the working directory. Do not run `cargo test` directly for routine validation.

Area commands:

```bash
just fmt
just check-fast -p <crate>
just test-fast -p <crate> <focused-filter>
just test -p <crate>
just fix -p <crate>
```

Specific gates:

```bash
just test-fast -p codex-tui <focused-filter>
just test -p codex-tui
just write-config-schema
just write-app-server-schema
just test-fast -p codex-app-server-protocol
just test-fast -p codex-app-server <focused-filter>
just test-fast -p codex-core <focused-filter>
just test-fast -p codex-mcp
just test-fast -p codex-rmcp-client
just test-fast -p codex-mcp-server
```

Package and script notes:

- Root `package.json` and `codex-cli/package.json` declare `pnpm`; use the package manager declared by the file you are touching.
- Use `just bazel-lock-update` and `just bazel-lock-check` for Rust dependency changes.
- Use `cargo insta pending-snapshots -p codex-tui` and review `.snap.new` files before accepting TUI snapshots.

Landing safety:

- Work in a separate Git worktree for agent-authored changes and preserve unrelated edits.
- Before committing, inspect `git status --short` and `git diff --check`.
- Run a staged or changed-file secrets scan that reports only file, line, and finding type; never print secret values.
- Remove detected credentials from the diff before proceeding.
- Do not add `Co-Authored-By` trailers to commit messages.
- Do not commit, push, publish, or open a PR unless that step is explicitly assigned.
