<p align="center"><strong>Codewith</strong> is a command-line coding agent from Hasna that runs locally on your computer.</p>

<p align="center">Codewith is a modified derivative of OpenAI Codex. Codewith is not affiliated with, sponsored by, or endorsed by OpenAI.</p>

---

## Quickstart

### Installing and running Codewith

Run the following on Mac, Linux, or Windows to install Codewith:

```shell
bun install -g @hasna/codewith
```

Codewith can also be installed through npm:

```shell
npm install -g @hasna/codewith
```

Then run `codewith` to get started. Codewith stores its local state in `~/.codewith` by default.

For building from source and all install options, see [Installing & building](./docs/install.md).

<details>
<summary>Download a prebuilt binary instead</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `codewith-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `codewith-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `codewith-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `codewith-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `codewith-x86_64-unknown-linux-musl`), so you likely want to rename it to `codewith` after extracting it.

</details>

### Using Codewith with your ChatGPT plan

Run `codewith` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Codewith as part of your Plus, Pro, Business, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-codex-in-chatgpt).

You can also use Codewith with an API key.

## Features

### Interactive Terminal UI

Codewith provides a rich terminal-based interface with:

- **Slash commands**: Use `/` to access commands like `/model`, `/profile`, `/config`, `/review`, and `/diff`
- **Session management**: Resume, fork, and archive previous conversations
- **Markdown rendering**: Display code, diffs, tables, and model responses in the terminal
- **Vim mode**: Optional Vim-style editing for the composer
- **Custom keybindings**: Remap TUI shortcuts to your preference
- **Theme selection**: Choose from multiple syntax highlighting themes

### Authentication Profiles

Save and switch between multiple local authentication profiles:

```shell
codewith login --auth-profile work
codewith login --auth-profile personal
codewith profile list
codewith --auth-profile work
```

### Execution Modes

- **Interactive mode**: Run `codewith` for the full TUI experience
- **Exec mode**: Run `codewith exec "your prompt"` for non-interactive tasks
- **Review mode**: Run `codewith review` for code review assistance
- **Apply mode**: Run `codewith apply` to apply diffs from previous sessions

### Sandbox Security

Codewith can run commands under platform-specific sandboxing:

- **macOS**: Uses Seatbelt sandboxing
- **Linux**: Uses Landlock and bubblewrap (bwrap) sandboxing
- **Windows**: Uses Windows sandbox with private desktop

### MCP (Model Context Protocol) Integration

Extend Codewith with external tools via MCP servers:

```shell
codewith mcp list
codewith mcp add <server-name>
```

### Skills System

Enhance Codewith's capabilities with reusable skill definitions:

- Create `.codewith/CODEWITH.md` for project-specific instructions
- Install and manage skills through configured skill sources
- Use `/skills` to browse and manage available skills

### Session Tools

Use built-in session tools while working:

- `/plan` switches to planning mode
- `/goal` tracks long-running objectives
- `/loop`, `/schedule`, and `/monitor` manage recurring work and lightweight monitors
- `/agent` and `/side` organize parallel or side conversations

## Docs

- [**Changelog**](./CHANGELOG.md) - Release notes for Codewith product versions
- [**Authentication**](./docs/authentication.md) - Auth profiles, login methods, and credential management
- [**Configuration**](./docs/config.md) - Config files, settings, and customization
- [**Contributing**](./docs/contributing.md) - Development setup and contribution guidelines
- [**Getting started**](./docs/getting-started.md) - First-run workflow and common commands
- [**Slash Commands**](./docs/slash_commands.md) - TUI command reference
- [**Sandbox & Security**](./docs/sandbox.md) - Security model and sandbox configuration
- [**Installing & building**](./docs/install.md) - Installation methods and building from source
- [**Skills**](./docs/skills.md) - Creating and using Codewith skills
- [**Open source fund**](./docs/open-source-fund.md) - Open source licensing and attribution

## License

This repository is licensed under the [Apache-2.0 License](LICENSE). See [NOTICE](NOTICE), [MODIFICATIONS.md](MODIFICATIONS.md), and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for attribution, modification, and bundled third-party notices. The Apache-2.0 license covers this codebase; it does not grant rights to OpenAI trademarks, services, accounts, subscriptions, models, or APIs.
