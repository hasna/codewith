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
```

Use `/config` in the TUI to edit common settings interactively.
