---
name: codewith-self-heal
description: Diagnose and safely repair Codewith config.toml and MCP startup failures. Use only when the user explicitly starts the self-heal workflow or asks to fix Codewith configuration/MCP breakage.
---

# Codewith Self-Heal

Use this skill only for Codewith recovery work involving `config.toml`, strict-config failures, MCP server configuration, or MCP startup/doctor failures. Treat the supplied doctor report as redacted diagnostic context; do not ask the user to paste secrets.

## Workflow

1. Diagnose first. Identify whether the breakage is TOML syntax/deserialization, an unknown config key, MCP command/cwd/env configuration, MCP HTTP reachability, or an unsupported inline secret such as `bearer_token`.
2. Present the proposed repair before changing files. Include the target file path, the exact keys or MCP servers affected, and the validation command you will run afterward.
3. Ask for user confirmation before editing `config.toml`, disabling MCP servers, moving files, or writing replacement config. If the user does not confirm, do not write.
4. Before every config write, create a timestamped backup next to the original file and report the backup path.
5. Keep repairs narrow and reversible. Prefer fixing one broken key, replacing unsupported inline secrets with env-var references, correcting obvious command/cwd values, or disabling an optional broken MCP server. Do not delete the user's config or silently disable required MCP servers.
6. Avoid exposing secrets. Redact token values, bearer tokens, cookies, API keys, and URL credentials in explanations and command output.
7. Use structured config helpers when available. Prefer Codewith's config edit or MCP edit APIs and TOML-aware edits over ad hoc string rewrites.
8. After applying a repair, rerun the relevant validation: parse/load config, `codewith doctor`, or the MCP-specific doctor check when available. Report remaining issues plainly.

## Safe Repair Guidance

- TOML syntax failure: if the syntax issue is obvious, propose the smallest TOML-aware edit. If it is not obvious and Codewith cannot start, propose backing up the malformed file and writing a minimal valid config so the user can start Codewith, then recover settings from the backup.
- Unknown strict-config key: propose removing or renaming only the unknown key. Mention that removing it may change behavior tied to older or newer Codewith versions.
- Inline MCP `bearer_token`: propose moving the secret out of `config.toml`, replacing it with `bearer_token_env_var`, and telling the user which env var to set. Do not print the old token value.
- Missing MCP env var: explain the env var that must be set. Disable the server only if it is optional and the user confirms that Codewith should start without it.
- Missing MCP command or cwd: propose the likely corrected command/path only when it is clear. Otherwise explain how the user can install the command or choose a valid path.
- MCP HTTP reachability: do not rewrite URLs unless the corrected URL is obvious from user input. Optional unreachable servers may be disabled after confirmation; required servers should stay enabled unless the user explicitly changes their requirement.

## Confirmation Template

Before writes, use a concise confirmation request:

```text
I can apply this repair:
- Back up <config-path> to a timestamped .bak file.
- Change <specific key/server/action>.
- Rerun <validation command>.

Confirm that I should apply this change.
```
