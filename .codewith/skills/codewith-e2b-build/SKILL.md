---
name: codewith-e2b-build
description: Build and test Codewith Rust crates on remote compute (E2B, AWS Fargate, or Daytona) instead of this machine. Use when a worktree worker must offload heavy codex-rs builds/tests, run the FULL suite (codex-core) that needs big RAM/disk, run many builds in parallel, or avoid loading the local box. Not for local inner-loop edits.
user_invocable: true
---

# Codewith Remote Build (E2B / AWS / Daytona)

Offload heavy Codewith Rust builds/tests from this machine to **remote compute**.
One CLI, three interchangeable backends (`--backend`):

| Backend | Per-box vCPU / RAM / usable disk | Cap? | Best for |
|---|---|---|---|
| **`aws`** (Fargate) | up to **16 / 120 GB / 200 GB** (default 8 / 32 GB / 150 GB) | **none** — we own it | **the FULL suite** (codex-core), anything that OOM/disk-fills on E2B |
| `e2b` (default) | 8 / 8 GB / ~9 GB free | tier: 8 GB RAM, no volumes | fast scoped `test-fast`/`check`, warm-reuse builds |
| `daytona` | 4 / 8 GB / 10 GB | tier: per-sandbox 4/8/10 | small scoped checks only (too small for full suites) |

**Why AWS exists here:** E2B (Pro) hard-caps templates at **8 GB RAM** ("Memory can't be
higher than 8192 MiB — contact support"), disk is **not a build knob** (rootfs ~21 GB, only
~9 GB free), and **Volumes are disabled** ("use of volumes is not enabled"). Daytona Tier 2
raised the org pool but per-sandbox is still **4 vCPU / 8 GB / 10 GB** (contact support to
raise). Both need a support ticket to go bigger. **AWS Fargate has none of these caps** and
is our canonical self-hosted stack — so full-suite builds (e.g. `codex-core`, which needs
15-40 GB of target/artifacts and links large binaries) run there without OOM or
"No space left on device". The linker "Bus error" seen on E2B was **SIGBUS-on-full-disk**
(lld mmaps its output), i.e. a disk problem, not RAM — AWS's 150 GB disk fixes it.

Entry point: **`scripts/codewith-remote-build.mjs`** (multi-backend). The legacy
`scripts/codewith-e2b-build.mjs` remains as an E2B-only alias for existing callers.

Read the root `CODEWITH.md` and the `codewith-rust-build` skill for the Rust
workflow itself; this skill is only the remote-execution wrapper. Template/env
details discovered during setup live in [references/e2b-template-notes.md](references/e2b-template-notes.md)
and [references/backends.md](references/backends.md).

## Source of truth = git (nothing is lost when a box expires)

The sandbox is **ephemeral compute only**. Your code lives in the pushed git
branch. The box always does `git fetch origin <branch>` + `git reset --hard`
(or fetches an exact `--sha`) before building. If a box expires, dies, or is
killed, **nothing is lost** — re-run against the same branch on a new box.
Push your branch before invoking. Uncommitted local changes can be layered on
with `--worktree <path>` (a `git diff HEAD` patch), but pushing is preferred.

## Quick start

```bash
# Installed skill location (invoke by absolute path; the script self-locates, so cwd doesn't matter):
cd ~/.claude/skills/codewith-e2b-build/scripts   # (or .codewith/skills/codewith-e2b-build/scripts in the repo)
bun install            # once: installs the e2b SDK locally (auto-runs if missing)

# FULL suite (codex-core) on AWS Fargate — big RAM + 150 GB disk, no cap:
bun codewith-remote-build.mjs --backend aws --branch <branch> --crate codex-core --full

# Fast scoped test on E2B (default backend), fresh box, auto-killed:
bun codewith-remote-build.mjs --branch <branch> --crate codex-core

# Compile-only signal (any backend):
bun codewith-remote-build.mjs --backend aws --branch <branch> --crate codex-core --check
```

Auth per backend (never print secrets — reference by vault name only):
- **e2b:** `E2B_API_KEY` else `secrets get hasnaxyz/e2b/live/api_key --raw`.
- **daytona:** `secrets get hasnaxyz/daytona/live/{api_key,api_url} --raw`.
- **aws:** the named profile's creds (`--aws-profile`, default `hasna-tools`) — no key printed.

## Invocation

```
codewith-e2b-build --branch <git-branch> --crate <crate> [--crate <crate> ...] [opts]
```

Required:
- `--branch <b>` — branch pushed to origin (source of truth).
- `--crate <c>` / `-p <c>` — crate(s) to build/test, e.g. `codex-core`. Repeatable.
  Runs are **scoped**: never a full-workspace build unless you pass `--all`.

Build mode (default `test` = `just test-fast-target`):
- `--check` — compile-only (`just check-fast`), fastest signal.
- `--full` — official gate (`just test`, includes bench-smoke). Use only when asked.
- `-- <args>` — pass extra args to the just recipe, e.g. `-- --test <binary>`.

Sandbox lifecycle:
- `--sandbox <id>` — **reuse a running box** (keeps its target-dir cache warm → fast rebuilds).
- `--keep` — leave a fresh box running after the build (prints the reuse command).
- `--pause` — snapshot + pause the box after the build (resume later with a warm cache).
- `--kill` — force-kill even a reused box.
- `--timeout-min <n>` — box lifetime / keepalive window (default 90).
- `--template <t>` — E2B template (default `codewith-pr-drain`).

Other:
- `--sha <sha>` — build an exact commit instead of the branch tip.
- `--worktree <path>` — apply local `git diff HEAD` from this checkout over the branch.
- `--json` — print a final machine-readable `JSON {...}` result line.

## What it returns

Streams the live build log to stdout and to a log file, then prints a RESULT
block: `verdict` (PASS/FAIL), crates, branch, the nextest summary
(`N tests run: X passed, Y failed`), `exit` code, wall-clock, the `sandbox` id +
disposition, and the `log:` path. Exit code is 0 on PASS, 1 on FAIL. With
`--json` it also emits one JSON line for machines to parse.

## Default template + toolchain env (the exit-127 fix)

Default template `codewith-pr-drain` (8 vCPU / 8 GB). It carries the toolchain
under `/opt/rust`, a codewith checkout at `/opt/codewith` (with a warm cargo
registry cache — no crate re-downloads), and a persistent target at
`/opt/codewith-target`.

The E2B command runner executes as the unprivileged `user` with a minimal PATH
and never inherits the Docker image ENV or rustup's default toolchain — that is
why a bare `rustc --version` returns exit 127. The wrapper fixes this by running
as `user: 'root'` with the toolchain env passed explicitly:

```
CARGO_HOME=/opt/rust/cargo  RUSTUP_HOME=/opt/rust/rustup
CARGO_TARGET_DIR=/opt/codewith-target  RUST_MIN_STACK=8388608
PATH=/opt/rust/cargo/bin:/root/.bun/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
```

Running as root also fixes git "dubious ownership" (root owns `/opt/codewith`);
the wrapper adds `safe.directory` and does the fetch/checkout in place.

## Longevity, reuse & speed

- **Keepalive**: the box is created with a long `--timeout-min` (default 90) and
  the wrapper refreshes the timeout every 30s (`Sandbox.setTimeout`) so long
  builds never expire mid-run. Extend at any time by re-running with a larger
  `--timeout-min` against `--sandbox <id>`.
- **Reuse for speed**: the first build on a fresh box is a cold dev-profile
  compile (~9 min for a `codex-core`-adjacent crate). Keep the box (`--keep`)
  and pass `--sandbox <id>` to later builds — the warm `CARGO_TARGET_DIR` means
  only changed crates recompile, so reruns are far faster.
- **Pause/resume**: `--pause` snapshots the box; resume later with `--sandbox <id>`
  to get the warm cache back without paying to keep it running.

## Parallel workers

Each worker runs its own invocation (its own box, or its own reused box id) →
N concurrent remote builds, zero local Rust load. Mind E2B concurrent-sandbox
limits: **Hobby ≈ 20, Pro ≈ 100** concurrent sandboxes (add-on up to ~1,100).
Keep parallel worker count under the account cap; check current usage with
`Sandbox.list()` or `bunx @e2b/cli@latest sandbox list`.

## Cleanup policy

- Fresh box, no flag → **killed** after the build (safe default, no orphan cost).
- `--keep` / `--pause` → box survives for reuse.
- `--sandbox <id>` (reuse) → **left running** unless you pass `--kill`.
Kill a stray box any time: `bunx @e2b/cli@latest sandbox kill <id>`.
