---
name: test-tui
description: Run the Codewith TUI interactively for manual verification. Use when changes need terminal UI smoke testing, log capture with RUST_LOG, scripted keystroke checks, or a live codewith session launched from the repo justfile.
---

You can start and use Codewith TUI to verify changes.

Important notes:

Start interactively.
Always set RUST_LOG="trace" when starting the process.
Pass `-c log_dir=<some_temp_dir>` argument to have logs written to a specific directory to help with debugging.
When sending a test message programmatically, send text first, then send Enter in a separate write (do not send text + Enter in one burst).
From the repo root, use the `just codewith` target to run from `codex-rs`, for example `just codewith -c log_dir=<some_temp_dir>`.
