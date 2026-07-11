---
name: app-server-agents
description: "Develop or debug Codewith app-server agent and orchestration features. Use for app-server protocol v2, background agents, subagents, collab agent events, activeSession, localSession, missionControl, durable mailbox, pending interactions, thread goals, agent navigation, and TUI thread routing."
---

# App-Server Agents

## Start Here

1. Read `.codewith/CODEWITH.md` and `codex-rs/app-server/README.md`.
2. Decide whether the task is protocol shape, app-server state, core event mapping, TUI presentation, or CLI command plumbing.
3. Keep app-server v2 as the active API surface.

## Key Surfaces

- Protocol types: `codex-rs/app-server-protocol/src/protocol/v2/`
- Agent protocol: `codex-rs/app-server-protocol/src/protocol/v2/agent.rs`
- Thread history/event mapping: `codex-rs/app-server-protocol/src/protocol/thread_history.rs`, `event_mapping.rs`
- App-server request processing: `codex-rs/app-server/src/request_processors/`
- Thread lifecycle/state: `codex-rs/app-server/src/thread_state.rs`, `thread_status.rs`
- Active/local session and mission control APIs: `codex-rs/app-server/README.md`
- TUI agent routing/navigation: `codex-rs/tui/src/app/thread_routing.rs`, `session_lifecycle.rs`, `agent_navigation.rs`, `background_agent_actions.rs`

## Workflow

1. For protocol additions, follow v2 naming and serialization rules from `.codewith/CODEWITH.md`: `*Params`, `*Response`, `*Notification`, camelCase wire fields, `#[ts(export_to = "v2/")]`.
2. Keep live active-session routing separate from durable mailbox or mission-control storage. Active sessions do not resume unloaded threads.
3. Preserve redactions for process details, message bodies, idempotency keys, and sensitive routing data.
4. Model pending interactions with terminal statuses rather than ad hoc strings.
5. When changing TUI presentation, update collab/background agent snapshots and ensure replayed history and live notifications render consistently.

## Validation

```bash
cd codex-rs
just write-app-server-schema
just test-fast -p codex-app-server-protocol
just test-fast -p codex-app-server agent
just test-fast -p codex-tui agent
```

For CLI/background agent command paths:

```bash
cd codex-rs
just test-fast -p codex-cli agent
just test-fast -p codex-core subagent
```

## Pitfalls

- Do not add new API surface to app-server v1.
- Do not imply mission control can execute shell commands, mutate files, or dispatch remote machines unless that capability is explicitly implemented and authorized.
- Do not collapse unloaded sessions, active peers, runtime session ids, and durable thread ids into one concept.
- Do not skip schema regeneration after protocol changes.
