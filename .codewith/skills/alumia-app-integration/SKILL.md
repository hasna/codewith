---
name: alumia-app-integration
description: "Guide agents integrating an OSS app into Alumia apps functionality. Use for integrate OSS app, add Hasna app to Alumia, Alumia apps functionality, app registry, app:// mentions, MCP app, codex_apps, @hasna/alumia adapters, open-alumia boundaries, and mapping Hasna open-source app repos into app surfaces."
---

# Alumia App Integration

## Start Here

1. Do not inspect or edit `platform-alumia` unless the user explicitly targets that repo.
2. Inspect the source OSS app repo read-only first, usually under `/home/hasna/workspace/hasna/opensource/open-<app>`.
3. Inspect `open-alumia` read-only for public kernel contracts: `/home/hasna/workspace/hasna/opensource/open-alumia`.
4. Make code changes only in the repo the user assigned.

## Source App Checklist

- `package.json`: package name, exports, bins, scripts, files, and dependency boundaries.
- `README.md` and docs: supported local/self-hosted workflow, CLI, SDK, and MCP claims.
- `src/index.ts`: public SDK exports.
- `src/mcp/` or equivalent: MCP server tools and schemas, if present.
- Tests: what behavior the app already promises.

## Alumia Boundary

`open-alumia` publishes `@hasna/alumia` and currently exposes SDK, CLI, MCP, local data dirs, and hosted adapter contracts. Read:

- `/home/hasna/workspace/hasna/opensource/open-alumia/README.md`
- `/home/hasna/workspace/hasna/opensource/open-alumia/docs/open-core-boundary.md`
- `/home/hasna/workspace/hasna/opensource/open-alumia/docs/platform-extension-contract.md`
- `/home/hasna/workspace/hasna/opensource/open-alumia/src/index.ts`
- `/home/hasna/workspace/hasna/opensource/open-alumia/src/mcp/index.ts`

Keep hosted tenant auth, billing, provider vaults, hosted connector token custody, production deploy, compliance retention, and platform admin authority behind adapters. Do not move those concerns into OSS app code.

## Codewith App Surfaces

In Codewith, an installed app is equivalent to MCP tools exposed through the `codex_apps` MCP server.

- App instructions: `codex-rs/core/src/context/apps_instructions.rs`
- Apps MCP server: `codex-rs/codex-mcp/src/codex_apps.rs`, `codex-rs/codex-mcp/src/mcp/mod.rs`
- App/tool tests: `codex-rs/core/src/connectors_tests.rs`, `codex-rs/core/tests/suite/search_tool.rs`
- App link UI: `codex-rs/tui/src/bottom_pane/app_link_view.rs`
- App invocation example: `codex-rs/app-server/README.md`

## Workflow

1. Identify the app's public surfaces and avoid private implementation imports.
2. Map app capabilities to Alumia as SDK functions, CLI commands, MCP tools, connector descriptors, or hosted adapter requirements.
3. If the task mentions app registry, confirm the actual registry owner before editing; do not invent platform assumptions.
4. For Codewith app mentions, use `app://<connector-id>` and respect `codex_apps`/`tool_search` behavior.
5. Validate both the source app behavior and any Codewith app exposure you changed.

## Validation

For `open-alumia` changes when explicitly assigned there:

```bash
bun test
bun run typecheck
bun run build
```

For Codewith app/MCP exposure changes:

```bash
cd codex-rs
just test-fast -p codex-core connectors
just test-fast -p codex-mcp
just test-fast -p codex-tui app_link
```

## Pitfalls

- Do not treat `platform-alumia` as the default integration target.
- Do not store or pass hosted secrets, connector tokens, tenant sessions, or billing authority through OSS contracts.
- Do not call an app an MCP app unless it actually exposes MCP tools or is routed through `codex_apps`.
- Do not assume every Hasna OSS app has the same package, CLI, SDK, or MCP shape.
