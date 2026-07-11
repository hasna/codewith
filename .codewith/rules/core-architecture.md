# Core Architecture

Start with `codex-rs/README.md`, `codex-rs/core/README.md`, and `.codewith/CODEWITH.md`.

The Cargo workspace lives under `codex-rs/`. Crate directories keep `codex-*` crate names even though the product is Codewith.

Core boundaries:

- `codex-rs/core/` owns business logic shared by UIs: thread/session orchestration, config resolution, prompts/context, tool execution, MCP integration, sandboxing, rollout handling, and model event mapping.
- Resist adding new concepts to `codex-rs/core/` by default. Check existing sibling crates first, especially `codex-rs/tools/`, `codex-rs/state/`, `codex-rs/protocol/`, `codex-rs/thread-store/`, `codex-rs/core-skills/`, `codex-rs/core-plugins/`, and focused extension crates under `codex-rs/ext/`.
- `codex-rs/tools/` is the shared host-side tool model/adaptation crate; do not move core session orchestration there prematurely.
- `codex-rs/state/` owns SQLite-backed state, models, runtime accessors, and migrations. Do not treat loaded thread maps or rollout files as durable state when a state table exists.
- `codex-rs/protocol/` owns shared protocol/config types used across crates.

Inspect these source areas before changing behavior:

- `codex-rs/core/src/lib.rs` for public exports and module ownership.
- `codex-rs/core/src/codex_thread.rs`, `codex-rs/core/src/session/`, and `codex-rs/core/src/thread_manager.rs` for thread and turn lifecycle.
- `codex-rs/core/src/config/` for config schema and resolution.
- `codex-rs/core/src/context/` and `codex-rs/core/src/context_manager/` for model-visible instruction/context assembly.
- `codex-rs/core/src/tools/`, `codex-rs/core/src/mcp_tool_call.rs`, and `codex-rs/core/src/unified_exec/` for tool dispatch and execution.
- `codex-rs/core/src/state_db_bridge.rs` and `codex-rs/state/` before changing persisted state behavior.

Validation:

```bash
just check-fast -p codex-core
just test-fast -p codex-core <focused-filter>
just test -p codex-core
```

When changing config types, run `just write-config-schema`. When changing Rust dependencies, run `just bazel-lock-update` and `just bazel-lock-check`.
