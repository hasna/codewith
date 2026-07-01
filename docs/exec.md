# Non-interactive mode

Use `codewith exec` for one-off tasks where you want a command result instead
of the interactive TUI.

```shell
codewith exec "summarize the current git diff"
codewith exec "run the focused tests for this crate and explain failures"
```

`exec` accepts the same common model, profile, sandbox, and config override
flags as interactive Codewith.

Headless `exec` runs persist session files by default so long-running automation
can resume or inspect the thread later. Use `--durable` or `--persist` when a
workflow needs to state that contract explicitly. Use `--ephemeral` only for
intentional one-off runs that should not be materialized on disk.

Use `codewith review` when the task is specifically a code review:

```shell
codewith review
```
