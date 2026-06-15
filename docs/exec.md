# Non-interactive mode

Use `codewith exec` for one-off tasks where you want a command result instead
of the interactive TUI.

```shell
codewith exec "summarize the current git diff"
codewith exec "run the focused tests for this crate and explain failures"
```

`exec` accepts the same common model, profile, sandbox, and config override
flags as interactive Codewith.

Use `codewith review` when the task is specifically a code review:

```shell
codewith review
```
