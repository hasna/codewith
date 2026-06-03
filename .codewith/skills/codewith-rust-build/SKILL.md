---
name: codewith-rust-build
description: Build, format, lint, and test Codewith Rust crates in codex-rs. Use when making or verifying Rust changes, resolving Codewith build errors, updating Cargo or Bazel locks, building release binaries, or validating Rust before npm publish.
---

# Codewith Rust Build

## Overview

Use this skill for Rust work in `codex-rs`. Always read the root `CODEWITH.md` first; it is the authoritative local workflow and style guide.

## Core Rules

- The product is Codewith, even though crate names remain `codex-*`.
- Do not run `cargo test` directly. Use `just test`.
- Run `just fmt` from `codex-rs` after Rust code changes.
- Run scoped `just fix -p <project>` before finalizing substantial Rust changes.
- Be patient with Rust commands and do not kill them by PID.
- If Rust dependencies change, run `just bazel-lock-update` and `just bazel-lock-check` from the repo root.
- If `ConfigToml` or nested config types change, run `just write-config-schema`.
- If app-server protocol shapes change, run `just write-app-server-schema` and the app-server protocol tests.

## Standard Verification

1. Map changed paths to crates:

```bash
git diff --name-only
```

2. Format:

```bash
cd codex-rs
just fmt
```

3. Run focused tests for changed crates:

```bash
cd codex-rs
just test -p codex-tui
just test -p codex-core
```

Use the actual changed crates instead of the examples above.

4. Run scoped fixes:

```bash
cd codex-rs
just fix -p <changed-crate>
```

Do not rerun tests after `fmt` or `fix` unless the user asks or the command changed behavior unexpectedly.

## Snapshot Tests

For user-visible TUI changes:

```bash
cd codex-rs
just test -p codex-tui
cargo insta pending-snapshots -p codex-tui
cargo insta accept -p codex-tui
```

Only accept snapshots after reading the generated `.snap.new` files or otherwise verifying the visual/text change is intentional.

## Release Binary

For npm release smoke tests, build the native release binary:

```bash
cd codex-rs
cargo build --release --bin codewith
./target/release/codewith --version
```

The canonical package builder may target `aarch64-unknown-linux-musl` or `x86_64-unknown-linux-musl`. If that fails because the host lacks musl tooling, record the exact linker/toolchain error and use the established package layout only when it matches previous published packages for the current platform.

## Full Suite

If the user explicitly asks to "test everything", run the full suite with `just test` from `codex-rs` after focused tests. If the full suite fails due host limitations, collect concrete evidence such as AppArmor, Bubblewrap, missing user namespace support, network gating, or toolchain errors, then still run focused changed-path tests so the code changes have direct coverage.

Do not describe a full-suite run as passing unless it actually passes.
