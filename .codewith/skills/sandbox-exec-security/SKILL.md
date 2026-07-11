---
name: sandbox-exec-security
description: "Work on Codewith command execution, sandboxing, approvals, and exec security. Use for sandbox modes, permission profiles, exec policy, unified exec, command/exec app-server RPCs, linux sandbox, seatbelt, Windows sandbox, network/file permissions, dangerous command gating, and approval prompts."
---

# Sandbox Exec Security

## Start Here

1. Read `.codewith/CODEWITH.md`, especially the sandbox environment variable warning.
2. Identify the surface: shell tool execution, app-server `command/exec`, exec mode, sandbox implementation, or exec policy.
3. Treat bypasses, approval suppression, and widened filesystem/network access as high-risk changes.

## Key Surfaces

- Sandbox request and execution: `codex-rs/core/src/sandboxing/mod.rs`
- Permission and approval policy: `codex-rs/core/src/exec_policy.rs`, `codex-rs/execpolicy/src/`
- Unified exec: `codex-rs/core/src/unified_exec/`
- App-server command execution: `codex-rs/app-server/src/command_exec.rs`
- Exec CLI mode: `codex-rs/exec/src/`
- Platform sandboxes: `codex-rs/sandboxing/src/`, `codex-rs/linux-sandbox/src/`, `codex-rs/windows-sandbox-rs/src/`
- Shell command classification: `codex-rs/shell-command/src/`
- Doctor checks: `codex-rs/cli/src/doctor.rs`

## Workflow

1. Preserve the distinction between approval policy, sandbox policy, permission profiles, and exec-policy rules.
2. Do not edit behavior around `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `CODEX_SANDBOX_ENV_VAR` unless explicitly assigned.
3. Prefer structured command parsing and existing shell-command helpers over ad hoc string matching.
4. Keep Windows, macOS Seatbelt, Linux bwrap/Landlock, and external sandbox behavior distinct.
5. If a command can mutate or exfiltrate, prove how the sandbox, approval policy, or exec policy blocks or prompts for it.
6. Keep app-server streaming constraints intact; Windows restricted-token execution does not support streaming `command/exec`.

## Validation

```bash
cd codex-rs
just test-fast -p codex-core exec_policy
just test-fast -p codex-sandboxing
just test-fast -p codex-linux-sandbox
just test-fast -p codex-exec
just test-fast -p codex-app-server command_exec
just test-fast -p codex-exec-server
```

For config/API shape changes:

```bash
cd codex-rs
just write-config-schema
just write-app-server-schema
```

## Pitfalls

- Do not treat read-only sandbox on every platform as equally strong; platform behavior differs.
- Do not add broad allow rules for shells, interpreters, `env`, `sudo`, or package managers without a narrow threat review.
- Do not silently downgrade from sandbox denial to unsandboxed execution.
- Avoid tests that depend on the host being able to run every platform sandbox locally.
