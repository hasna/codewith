# App Server, Background Agents, MCP, And Tools

Start with `.codewith/CODEWITH.md`, `codex-rs/app-server/README.md`, and `codex-rs/background-agent/ARCHITECTURE.md`.

App-server boundaries:

- Active API work belongs in app-server v2. Protocol types live in `codex-rs/app-server-protocol/src/protocol/v2/`.
- Cross-version/common protocol helpers live in `codex-rs/app-server-protocol/src/protocol/common.rs`, `codex-rs/app-server-protocol/src/protocol/thread_history.rs`, `codex-rs/app-server-protocol/src/protocol/event_mapping.rs`, and `codex-rs/app-server-protocol/src/protocol/mappers.rs`.
- Runtime request handling lives under `codex-rs/app-server/src/request_processors/`; connection, transport, config, message processing, and tracing live in sibling app-server modules.
- Generated schema fixtures live under `codex-rs/app-server-protocol/schema/` and must be regenerated when API shapes change.

Background-agent boundaries:

- `codex-rs/background-agent/` defines durable background-agent contracts and supervisor behavior.
- The durable roster is the `background_agent_runs` state table described in `codex-rs/background-agent/ARCHITECTURE.md`; loaded thread maps, app-server connection lifetime, rollout files, and legacy agent job rows are not liveness sources.
- Pending approvals, user input, MCP elicitation, and execution snapshots must preserve conservative unattended behavior and redaction.
- App-server background-agent API surfaces live in `codex-rs/app-server/src/request_processors/background_agent_processor.rs`, `codex-rs/app-server/src/request_processors/background_agent_live.rs`, and related tests.

MCP and tool boundaries:

- MCP client/server integration lives in `codex-rs/codex-mcp/`, `codex-rs/rmcp-client/`, and `codex-rs/mcp-server/`.
- Shared tool models and adaptation live in `codex-rs/tools/`.
- Model-visible tool execution and search still intersect `codex-rs/core/src/tools/` and `codex-rs/core/src/mcp_tool_call.rs`.
- App-server MCP API types live in `codex-rs/app-server-protocol/src/protocol/v2/mcp.rs`; request handling lives in `codex-rs/app-server/src/request_processors/mcp_processor.rs`.

Rules:

- Preserve redaction for tokens, headers, provider auth, process details, idempotency keys, and sensitive routing data.
- Keep live active-session delivery separate from durable mailbox, mission-control, and background-agent storage.
- Do not add new v1 app-server API surface.
- Regenerate schema fixtures after protocol shape changes.

Validation:

```bash
just write-app-server-schema
just test-fast -p codex-app-server-protocol
just test-fast -p codex-app-server <focused-filter>
just test-fast -p codex-background-agent
just test-fast -p codex-mcp
just test-fast -p codex-rmcp-client
just test-fast -p codex-mcp-server
just test-fast -p codex-tools
```
