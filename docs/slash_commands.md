# Slash Commands

Slash commands are available in the interactive TUI. Type `/` in the composer
to open the command picker.

## Common Commands

- `/model` chooses the model and reasoning effort.
- `/profile` chooses the auth profile for the current session.
- `/provider` chooses the default model provider.
- `/config` opens interactive configuration.
- `/permissions` changes what Codewith is allowed to do.
- `/keymap` remaps TUI shortcuts.
- `/vim` toggles Vim mode for the composer.
- `/review` reviews current changes.
- `/diff` shows the current git diff, including untracked files.
- `/mention` inserts a file mention.
- `/status` opens an interactive status panel (Overview, Usage, Tools, Session)
  with token usage, rate limits, and quick links to settings.
- `/resume`, `/fork`, `/new`, and `/archive` manage sessions.
- `/plan` switches to Plan mode.
- `/goal` sets or views the current long-running task goal.
- `/loop`, `/schedule`, and `/monitor` manage recurring prompts and monitors.
- `/agent`, `/session`, and `/side` manage parallel or side conversations.
- `/skills` opens skill management.
- `/mcp` lists configured MCP tools.
- `/plugins` browses plugins.
- `/quit` leaves Codewith.

Legacy duplicate aliases such as `/background-agent`, `/subagents`, `/btw`,
`/stats`, and `/exit` still dispatch for compatibility, but the command picker
only advertises their canonical forms.

Some commands accept inline arguments. Examples:

```text
/review check the staged changes only
/rename Release packaging cleanup
/sandbox-add-read-dir /absolute/path
/mcp verbose
```
