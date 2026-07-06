# Codewith CLI (Rust Implementation)

We provide Codewith CLI as a standalone executable to ensure a zero-dependency install.

## Installing Codewith

Today, the easiest way to install Codewith is via `bun`:

```shell
bun install -g @hasna/codewith
codewith
```

You can also download a platform-specific release directly from our [GitHub Releases](https://github.com/hasna/codewith/releases).

## Documentation quickstart

- First run with Codewith? Start with [`docs/getting-started.md`](../docs/getting-started.md) (links to the walkthrough for prompts, keyboard shortcuts, and session management).
- Want deeper control? See [`docs/config.md`](../docs/config.md) and [`docs/install.md`](../docs/install.md).

## What's new in the Rust CLI

The Rust implementation is now the maintained Codewith CLI and serves as the default experience. It includes a number of features that the legacy TypeScript CLI never supported.

### Config

Codewith supports a rich set of configuration options. Note that the Rust CLI uses `config.toml` instead of `config.json`. See [`docs/config.md`](../docs/config.md) for details.

### Model Context Protocol Support

#### MCP client

Codewith CLI functions as an MCP client that allows the Codewith CLI and IDE extension to connect to MCP servers on startup. See the [`configuration documentation`](../docs/config.md#connecting-to-mcp-servers) for details.

#### MCP server (experimental)

Codewith can be launched as an MCP _server_ by running `codewith mcp-server`. This allows _other_ MCP clients to use Codewith as a tool for another agent.

Use the [`@modelcontextprotocol/inspector`](https://github.com/modelcontextprotocol/inspector) to try it out:

```shell
npx @modelcontextprotocol/inspector codewith mcp-server
```

Use `codewith mcp` to add/list/get/remove MCP server launchers defined in `config.toml`, and `codewith mcp-server` to run the MCP server directly.

### Notifications

You can enable notifications by configuring a script that is run whenever the agent finishes a turn. The [notify documentation](../docs/config.md#notify) includes a detailed example that explains how to get desktop notifications via [terminal-notifier](https://github.com/julienXX/terminal-notifier) on macOS. When Codewith detects that it is running under WSL 2 inside Windows Terminal (`WT_SESSION` is set), the TUI automatically falls back to native Windows toast notifications so approval prompts and completed turns surface even though Windows Terminal does not implement OSC 9.

### `codewith exec` to run Codewith programmatically/non-interactively

To run Codewith non-interactively, run `codewith exec PROMPT` (you can also pass the prompt via `stdin`) and Codewith will work on your task until it decides that it is done and exits. If you provide both a prompt argument and piped stdin, Codewith appends stdin as a `<stdin>` block after the prompt so patterns like `echo "my output" | codewith exec "Summarize this concisely"` work naturally. Output is printed to the terminal directly. You can set the `RUST_LOG` environment variable to see more about what's going on.
Headless `codewith exec` runs persist session rollout files by default so long-running automation can resume or inspect the thread later. Use `--durable` or `--persist` when a workflow needs to state that contract explicitly, and use `codewith exec --ephemeral ...` only for intentional one-off runs that should not be materialized on disk.

### Experimenting with the Codewith Sandbox

To test to see what happens when a command is run under the sandbox provided by Codewith, use the `sandbox` subcommand in Codewith CLI:

```
# Uses the sandbox implementation for the current host OS:
# Seatbelt on macOS, the Linux sandbox on Linux, and Windows restricted token on Windows.
codewith sandbox [COMMAND]...

# macOS-only diagnostic option
codewith sandbox --log-denials [COMMAND]...
```

`codewith sandbox` also accepts `--profile NAME` (`-p NAME`) to layer
`$CODEWITH_HOME/NAME.config.toml` onto the base user config for the sandboxed
command.

### Selecting a sandbox policy via `--sandbox`

The Rust CLI exposes a dedicated `--sandbox` (`-s`) flag that lets you pick the sandbox policy **without** having to reach for the generic `-c/--config` option:

```shell
# Run Codewith with the default, read-only sandbox
codewith --sandbox read-only

# Allow the agent to write within the current workspace while still blocking network access
codewith --sandbox workspace-write

# Danger! Disable sandboxing entirely (only do this if you are already running in a container or other isolated env)
codewith --sandbox danger-full-access
```

In `workspace-write`, Codewith also includes `~/.codewith/memories` in its writable roots so memory maintenance does not require an extra approval.

## Code Organization

This folder is the root of a Cargo workspace. It contains quite a bit of experimental code, but here are the key crates:

- [`core/`](./core) contains the business logic for Codewith. Ultimately, we hope this becomes a library crate that is generally useful for building other Rust/native applications that use Codewith.
- [`exec/`](./exec) "headless" CLI for use in automation.
- [`tui/`](./tui) CLI that launches a fullscreen TUI built with [Ratatui](https://ratatui.rs/).
- [`cli/`](./cli) CLI multitool that provides the aforementioned CLIs via subcommands.

If you want to contribute or inspect behavior in detail, start by reading the module-level `README.md` files under each crate and run the project workspace from the top-level `codex-rs` directory so shared config, features, and build scripts stay aligned.
