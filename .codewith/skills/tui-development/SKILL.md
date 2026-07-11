---
name: tui-development
description: "Design, implement, test, or debug Codewith TUI work in codex-rs/tui. Use for ratatui widgets, chatwidget, bottom pane, popups, keyboard handling, status lines, terminal layout, snapshots, /profile, /mcp, /skills, and interactive TUI verification."
---

# TUI Development

## Start Here

1. Read `.codewith/CODEWITH.md`, especially TUI style, wrapping, and snapshot rules.
2. Read `codex-rs/tui/styles.md`.
3. Inspect the smallest owning module and its tests before editing shared files.

## Key Surfaces

- Main app orchestration: `codex-rs/tui/src/app.rs`, `codex-rs/tui/src/app/`
- Chat transcript and streamed output: `codex-rs/tui/src/chatwidget.rs`, `codex-rs/tui/src/chatwidget/`
- Composer, popups, overlays, footer: `codex-rs/tui/src/bottom_pane/`
- Rendering helpers: `codex-rs/tui/src/render/`, `codex-rs/tui/src/live_wrap.rs`, `codex-rs/tui/src/text_formatting.rs`
- App-server bridge: `codex-rs/tui/src/app_server_session.rs`, `codex-rs/tui/src/app/app_server_requests.rs`
- Snapshots: `codex-rs/tui/src/**/snapshots/*.snap`

## Workflow

1. Keep changes close to the owning feature module. Avoid growing central files such as `app.rs`, `chatwidget.rs`, `bottom_pane/mod.rs`, and `footer.rs` unless the change is truly orchestration.
2. Use ratatui `Stylize` helpers such as `"text".dim()`, `"text".bold()`, `"text".green()`, and `"text".red()`.
3. Prefer the default terminal foreground. Avoid hardcoded white, black, blue, cyan, and yellow unless a local style helper requires it.
4. Wrap plain strings with `textwrap::wrap`; wrap ratatui lines with helpers in `tui/src/wrapping.rs` or related local utilities.
5. Add or update snapshot coverage for user-visible UI changes.
6. For interactive checks, run with trace logging and a temporary log directory.

## Validation

```bash
cd codex-rs
just fmt
just test-fast -p codex-tui <focused-filter>
```

For intentional UI/text changes:

```bash
cd codex-rs
just test -p codex-tui
cargo insta pending-snapshots -p codex-tui
cargo insta show -p codex-tui path/to/file.snap.new
cargo insta accept -p codex-tui
```

Interactive smoke test:

```bash
cd codex-rs
RUST_LOG=trace just codewith -c log_dir=/tmp/codewith-tui-logs
```

## Pitfalls

- Do not run `cargo test` directly; use the repo `just` recipes.
- Do not accept snapshots without reviewing the `.snap.new` output.
- Do not refactor equivalent ratatui style forms just for churn.
- When sending scripted input to an interactive TUI, write text first and Enter as a separate input.
