---
name: mcp-tools
description: "Develop, test, or debug Codewith MCP tools and app tools. Use for MCP server/client work, codex_apps, app:// mentions, Apps as MCP tools, tool_search, MCP OAuth, MCP elicitation, mcpServer status/reload, tools/list, tool approval metadata, and connector-backed app tools."
---

# MCP Tools

## Start Here

1. Decide which side owns the behavior: MCP server process, MCP client manager, model tool exposure, app-server RPC, or TUI inventory/approval UI.
2. Read `.codewith/CODEWITH.md`; it specifically points MCP tool-call mutations toward `codex-rs/codex-mcp/src/connection_manager.rs`.
3. Treat apps/connectors as MCP-backed tools when they flow through `codex_apps`.

## Key Surfaces

- MCP connection manager: `codex-rs/codex-mcp/src/connection_manager.rs`
- Codewith apps MCP server: `codex-rs/codex-mcp/src/codex_apps.rs`, `mcp/mod.rs`
- MCP client/OAuth/elicitation: `codex-rs/rmcp-client/src/`
- Codewith MCP server binary: `codex-rs/mcp-server/src/`
- Tool exposure/search: `codex-rs/core/src/tools/spec_plan.rs`, `codex-rs/core/src/tools/handlers/tool_search*.rs`
- MCP tool execution and approval: `codex-rs/core/src/mcp_tool_call.rs`
- TUI inventory and elicitation UI: `codex-rs/tui/src/app/background_requests.rs`, `codex-rs/tui/src/bottom_pane/mcp_server_elicitation.rs`
- App-server MCP APIs: `codex-rs/app-server-protocol/src/protocol/v2/mcp.rs`, `codex-rs/app-server/tests/suite/v2/mcp_server_status.rs`

## Workflow

1. Keep tool names, server names, and app connector ids stable. Apps use the `codex_apps` MCP server.
2. Preserve lazy loading semantics: when `tool_search` is available, deferred tools should be searchable without being injected into every request.
3. Preserve approval metadata and redaction. Never expose raw bearer tokens, OAuth tokens, headers, or secret query parameters.
4. For app tools, follow `core/src/context/apps_instructions.rs`: agents should use `tool_search` for app tool discovery rather than resource listing.
5. Keep TUI state updates routed through `AppEvent` and background request helpers so rendering stays single-threaded.

## Validation

```bash
cd codex-rs
just test-fast -p codex-mcp
just test-fast -p codex-rmcp-client
just test-fast -p codex-mcp-server
just test-fast -p codex-core mcp
```

For TUI/app-server surfaces:

```bash
cd codex-rs
just write-app-server-schema
just test-fast -p codex-app-server mcp
just test-fast -p codex-tui mcp
```

## Pitfalls

- Do not call `reset_client_session` as a broad fix; rely on incremental check logic unless evidence says otherwise.
- Do not mix app tool discovery with `list_mcp_resources` or `list_mcp_resource_templates`.
- HTTP MCP and package installs may require network access; keep approval and sandbox behavior explicit.
- Large tool sets should remain compatible with `tool_search` and prompt-size controls.
