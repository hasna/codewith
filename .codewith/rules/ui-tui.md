# UI And TUI

Start with `.codewith/CODEWITH.md` and `codex-rs/tui/styles.md`. The TUI is the `codex-tui` crate, built on ratatui/crossterm, with library code denying accidental stdout/stderr writes in `codex-rs/tui/src/lib.rs`.

Key surfaces to inspect before editing:

- `codex-rs/tui/src/app.rs` and `codex-rs/tui/src/app/` for top-level app orchestration, thread routing, app-server requests, goals, schedules, monitors, worktrees, and session lifecycle.
- `codex-rs/tui/src/chatwidget.rs` and `codex-rs/tui/src/chatwidget/` for transcript, streaming output, slash dispatch, status controls, tool lifecycle, and mode-specific UI.
- `codex-rs/tui/src/bottom_pane/` for composer, footer, popups, overlays, approval prompts, skill and MCP views.
- `codex-rs/tui/src/history_cell/`, `codex-rs/tui/src/render/`, `codex-rs/tui/src/markdown_render.rs`, `codex-rs/tui/src/live_wrap.rs`, `codex-rs/tui/src/wrapping.rs`, and `codex-rs/tui/src/text_formatting.rs` for terminal rendering and wrapping.
- `codex-rs/tui/src/**/snapshots/*.snap` and nearby `*_tests.rs` files for expected terminal output.

Patterns:

- Keep feature behavior near the owning module. Avoid growing central files such as `app.rs`, `chatwidget.rs`, `bottom_pane/mod.rs`, and `bottom_pane/footer.rs` unless the change is orchestration.
- Use ratatui `Stylize` helpers and shared color helpers; prefer the default foreground and the Codewith emerald accent where existing style guidance calls for it.
- Wrap plain strings with `textwrap::wrap`; wrap ratatui `Line` values with local wrapping helpers instead of ad hoc string slicing.
- User-visible TUI changes need snapshot coverage. Review generated `.snap.new` files before accepting snapshots.
- Test narrow widths, long labels, status/footer overlap, popup scrolling, and replayed history when changing layout or terminal rendering.

Validation:

```bash
just test-fast -p codex-tui <focused-filter>
just test -p codex-tui
cargo insta pending-snapshots -p codex-tui
cargo insta show -p codex-tui path/to/file.snap.new
cargo insta accept -p codex-tui
```
