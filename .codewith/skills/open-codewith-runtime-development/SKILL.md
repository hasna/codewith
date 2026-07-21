---
name: open-codewith-runtime-development
description: "Route and verify development work in the hasna/codewith fork across Rust runtime, provider/model/auth/profile behavior, TUI, app-server protocol, MCP lifecycle, goals/native loops, upstream merges, and release gates. Use when implementing, debugging, reviewing, or shipping work in any of these Codewith runtime domains, especially changes that cross crates or user-facing surfaces and need the correct project skills, focused tests, snapshots, schemas, or fork-preservation checks."
---

# Open Codewith Runtime Development

Use this as the concise entry point for cross-surface work in the Codewith fork. Delegate detailed procedures to the existing project skills instead of copying them here.

## Orient First

1. Read `.codewith/CODEWITH.md`; treat it as the authoritative repository policy.
2. Inspect `git status`, the current branch/worktree, and the relevant diff before choosing a route.
3. Map the change to the owning surfaces:
   - `codex-rs/core`, `config`, `protocol`, `login`: runtime, auth, profile persistence/switching, provider routing, and configuration.
   - `known-provider-models`, `models-manager`, `codex-api`: provider catalogs and model metadata.
   - `tui`: slash commands, pickers, status, snapshots, and other user-visible behavior.
   - `app-server-protocol`, `app-server`: v2 wire contracts, thread lifecycle, generated schemas, and SDK-facing behavior.
   - `codex-mcp`: MCP connection, tool refresh, and call lifecycle.
   - app-server, state, workflow, and TUI scheduling surfaces: Codewith-native `/loop`, goal, persistence, and scheduled runtime behavior.
   - `codex-cli` and release workflows: npm packaging and published binaries.
4. Resist adding new concepts to `codex-core` when a smaller owning crate is appropriate.

## Route To Existing Skills

Load the applicable skill before acting:

- Work limited to Codewith-native goals, `/loop`, schedules, claims, retries, or result delivery: `$codewith-native-loop-runtime`. Use this router with it only when the change crosses other runtime surfaces.
- Reproduce and diagnose an upstream issue: `$codewith-bug`.
- Build, format, lint, test, update schemas, or update Cargo/Bazel locks: `$codewith-rust-build`.
- Diagnose slow compile/link or test-binary shape: `$codewith-rust-test-speed`.
- Exercise the interactive terminal UI and capture logs: `$test-tui`.
- Isolate logical commits or push safely: `$codewith-git-ship`.
- Build, pack, publish, install, tag, and smoke a release: `$codewith-release-publish`.
- Prepare a pull-request description: `$codewith-pr-body`.
- Run final review: `$code-review` and its `code-review-*` reviewers.

Combine routes when needed. For example, a provider picker fix normally uses Rust build, TUI test, review, Git ship, and possibly release publish.

Specialist precedence is the default: use one focused skill for work inside its boundary. Compose skills only for a demonstrated cross-runtime contract, not merely because neighboring code exists.

## Preserve Cross-Surface Contracts

### Providers, Models, Auth, And Profiles

Keep provider selection, model metadata, context windows, credential resolution, auth-profile persistence, and request routing aligned. Prove that non-OpenAI providers do not inherit OpenAI or ChatGPT-account auth, that provider switching rebuilds the intended client state, and that model lists and fallbacks stay provider-scoped.

### TUI And App Server

Treat a behavior exposed through both surfaces as one contract. Update protocol v2, runtime handling, generated TypeScript/schema fixtures, TUI behavior, docs, and tests together when their shapes or semantics change. Add or update `insta` snapshots for every intentional user-visible TUI change and review pending snapshots before accepting them.

Keep Ratatui code immediate-mode: derive each frame from explicit application state, mutate that state in the event/update path, and keep rendering free of hidden side effects. Keep blocking I/O and long work out of the input/render loop; return typed events to the owner instead. Preserve terminal restoration on normal exit, error, panic, suspend, and resume, including raw mode, alternate-screen state, cursor visibility, and enabled input modes. Test state transitions and layout helpers directly, render deterministic buffers or snapshots for visual contracts, and use a PTY or interactive TUI smoke when terminal lifecycle behavior is involved.

### MCP Lifecycle

Prefer `codex-rs/codex-mcp/src/mcp_connection_manager.rs` for tool and connection mutation. Keep configuration, enable/disable, transport, startup diagnostics, package refresh, and interactive TUI state aligned. Preserve incremental refresh and client-session reuse; do not add unnecessary `reset_client_session` calls. If MCP configuration changes, include configuration schema generation and startup/refresh regression coverage.

### Goals, Loops, And Runtime State

Treat Codewith-native `/loop` as thread scheduling inside this repository, not as the separate OpenLoops package or daemon. Keep protocol, app-server runtime, state migrations, goal/workflow scheduling, launch context, TUI status, and persisted titles aligned. Bound context and persistent logs; do not turn state storage into a trace mirror.

### Async Runtime Discipline

Give every Tokio task an owner, cancellation path, and completion observation. Prefer structured task groups or retained handles over detached background work; cancel and join children during shutdown. Bound fan-out with queues, semaphores, or worker limits, and define overload behavior. Never hold a synchronous or async lock across `.await`; copy or move the required state out of the guard first. Use `spawn_blocking` for blocking work, apply timeouts at external boundaries, and test cancellation, shutdown, retry, and concurrency limits rather than only the happy path.

## Verify Proportionally

1. Run the narrowest changed-crate checks through `$codewith-rust-build`; use `$codewith-rust-test-speed` when feedback is dominated by compilation or linking.
2. Add focused regressions for the changed contract, including profile/provider separation and resume/fork behavior where relevant.
3. Apply required generators and gates:
   - Config type changes: `just write-config-schema`.
   - App-server protocol changes: `just write-app-server-schema` and protocol tests; include `--experimental` when affected.
   - Rust dependency changes: `just bazel-lock-update` and `just bazel-lock-check` from the repo root.
   - TUI output changes: focused `codex-tui` tests plus reviewed snapshots.
4. Use `$test-tui` for interactive command, picker, profile, MCP, or status behavior that static tests do not prove.
5. Run `$code-review` before finalizing substantial or integration-sensitive work.

## Preserve The Fork

When merging or comparing upstream, explicitly protect Codewith branding and packaging, `@hasna/codewith`, `.codewith` and `CODEWITH.md`, auth profiles, `/profile`, profile auto-switching, `/config`, provider-scoped behavior, app-server propagation, and the absence of automatic update prompts. Review both the upstream delta and the merge-resolution delta; passing compilation alone does not prove fork behavior survived.

## Choose The Release Gate

- For ordinary development, finish focused changed-crate tests and the final package gates required by `.codewith/CODEWITH.md`.
- For an urgent Codewith release, use the repository's fast release gate unless the user explicitly asks to wait for the full matrix: targeted regressions, package build, tarball smoke, published-install smoke, and tag-to-commit alignment through `$codewith-release-publish`.
- Use the full Rust/Bazel matrix as parallel or post-merge confidence for urgent broad/shared work, and as a blocking gate when explicitly requested. Do not claim it passed unless it actually ran and passed; investigate real failures even when the fast release path is otherwise sufficient.

## Check Global Fallback Drift

This repository-local skill is canonical for `hasna/codewith`. A global `$codewith-runtime-development` skill may exist as a cross-repo fallback. Compare shared invariants without requiring byte identity: specialist precedence, surface ownership, TUI lifecycle, Tokio discipline, protocol/schema gates, fork preservation, and live-artifact verification must not contradict. Repository-specific routes and the two skills' names, descriptions, and prompts are intentionally different. Port generally useful canonical changes to the fallback while keeping repository policy here authoritative.
