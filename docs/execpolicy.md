# Execution policy

Execution policy controls how Codewith evaluates shell commands before running
them. It works with the active permission profile and sandbox mode to decide
whether a command can run directly, needs approval, or should be rejected.

Use execution policy for local safety boundaries, especially in shared
repositories or managed environments where command behavior needs to be
predictable.

The CLI includes hidden execpolicy tooling for diagnostics and compatibility:

```shell
codewith execpolicy --help
```

Prefer the TUI `/permissions` command for normal day-to-day permission changes.
