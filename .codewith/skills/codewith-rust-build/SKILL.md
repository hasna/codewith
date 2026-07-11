---
name: codewith-rust-build
description: Format, inspect, and route validation for Codewith Rust crates in codex-rs, using remote-first heavy Rust/Bazel checks through the repository-local remote-build skill.
---

# Codewith Rust Build

## Overview

Use this skill for Rust work in `codex-rs`. Always read the root `CODEWITH.md` first; it is the authoritative local workflow and style guide.

## Core Rules

- The product is Codewith, even though crate names remain `codex-*`.
- Codewith agents no longer run heavy Rust/Bazel builds locally by default. For compile, test, clippy, Bazel, release, or lock validation, use the repository-local `remote-build` skill and `just remote-bazel-validation` unless the human explicitly approves a one-off local run or sets `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1`.
- Do not run `cargo test` directly. If local heavy validation is approved, use the repo `just` recipes instead of raw Cargo test commands.
- Run `just fmt` from `codex-rs` after Rust code changes.
- Run scoped fixes remotely where possible. Local `just fix -p <project>` is a heavy clippy build and needs the same approval or `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1`.
- Be patient with Rust commands and do not kill them by PID.
- If Rust dependencies change, lockfile mutation may require an explicitly approved local one-off; route any heavy lock validation through the approved remote path when available.
- If `ConfigToml` or nested config types change, run `just write-config-schema`.
- If app-server protocol shapes change, run `just write-app-server-schema`, then route app-server protocol test validation through remote focused checks unless local heavy validation was explicitly approved.

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

3. For heavy validation, use the repository-local `remote-build` skill. Start with the remote auth smoke after the workflow exists on the default branch:

```bash
just remote-bazel-validation --mode auth-smoke --ref <ref>
```

4. Then dispatch the narrowest relevant remote mode: `argument-comment-lint`, `clippy`, or `test-focused --targets '<labels>'`.
5. Record the GitHub Actions run URL, status, ref, mode, and targets as verification evidence.

## Local Static Loop

- Use `rg`, file inspection, `git diff --check`, schema/frontmatter sanity checks, and targeted non-compiling scripts locally.
- Do not invoke `just test-fast`, `just test`, `just check-fast`, `just fix`, `cargo build`, `cargo clippy`, or Bazel locally unless the local heavy-build policy is satisfied.
- For integration tests, prefer package and test-binary selection when choosing remote Bazel labels.
- If a name-filtered integration run still compiles a huge binary, split the area into a top-level `tests/<area>.rs` target and remove it from the aggregate module so it can build and link independently. For new hot API areas, create that standalone binary from the start.
- Keep machine-level acceleration such as `RUSTC_WRAPPER=sccache` or custom linker config in local setup unless this repo installs and validates it consistently.

## Snapshot Tests

For user-visible TUI changes:

- Prefer remote focused TUI validation first.
- Generating or accepting `.snap.new` files locally may require a heavy local test run. Get explicit approval or use `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1` for that deliberate one-off.
- Only accept snapshots after reading the generated `.snap.new` files or otherwise verifying the visual/text change is intentional.

## Release Binary

Release binary builds are heavy. Use a remote runner or the approved release workflow. Only build locally with explicit approval or `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1`, and record the exact command and result.

## Full Suite

If the user explicitly asks to "test everything", route the full or broad validation to remote GitHub Actions/BuildBuddy first. If the remote setup is missing or unhealthy, fix or route that setup instead of falling back to local compilation.

Do not describe a full-suite run as passing unless it actually passes.
