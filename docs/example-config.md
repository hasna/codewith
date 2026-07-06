# Sample configuration

Codewith reads `config.toml` from `CODEWITH_HOME`, which defaults to
`~/.codewith`.

```toml
model = "gpt-5"
approval_policy = "on-request"
sandbox_mode = "workspace-write"

[history]
persistence = "save-all"

[analytics]
enabled = true

[feedback]
enabled = true

# Cap concurrent subagent threads per agent run (default: 6, minimum: 1).
[agents]
max_threads = 4
```

Use `/config` in the TUI to edit common settings interactively.
