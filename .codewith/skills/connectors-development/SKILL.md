---
name: connectors-development
description: "Guide agents creating or updating API connectors in the Hasna connectors ecosystem. Use for create connector, new connector, connector development, API connector, operation schema, auth schema, connector registry, connectors MCP tools, CLI operations, @hasna/connectors, and safe credential handling."
---

# Connectors Development

## Start Here

1. Inspect `/home/hasna/workspace/hasna/opensource/open-connectors` read-only unless the user explicitly assigns that repo.
2. Read its `README.md`, `AGENTS.md`, and `docs/one-repo-one-product.md`.
3. Never read, print, commit, or place real API keys, OAuth tokens, refresh tokens, cookies, or passwords in tests, docs, comments, tasks, or examples.

## Key Surfaces

- Connector definition model: `src/core/connector.ts`
- Migrated internal connectors: `src/core/connectors/*.ts`, registered in `src/core/builtins.ts`
- Catalog metadata: `src/lib/registry.ts`, `src/lib/connectors/*.ts`
- Runtime execution: `src/lib/runner.ts`, `src/mcp/tools/operations.ts`
- Auth: `src/server/auth.ts`, `src/mcp/tools/auth.ts`
- CLI: `src/cli/`
- REST/dashboard: `src/server/`, `dashboard/`
- Legacy connector package shape: `connectors/<name>/src/`, `package.json`, `CLAUDE.md`, `.env.example`

## Workflow

1. Check the connector blacklist in `AGENTS.md` before adding anything. Browser-use scrapers do not belong in the open-source connector repo.
2. Prefer the one-product runtime: define/update an internal connector under `src/core/connectors/` and register it in `src/core/builtins.ts`.
3. Keep catalog metadata in sync when search, install, dashboard, or docs surfaces need it.
4. Model auth with `ConnectorAuthDefinition` and mark secret fields as secret. Store only through existing auth/profile helpers.
5. Define operations with stable names and Zod `inputSchema` values; operation names must match the existing operation-name pattern.
6. Keep CLI, MCP, REST, and SDK behavior aligned around the same connector definition.
7. Use placeholder values in `.env.example` and tests; use secret references in automation examples.

## Validation

From `/home/hasna/workspace/hasna/opensource/open-connectors` when that repo is explicitly assigned:

```bash
bun test
bun run typecheck
bun run build
```

For focused connector/runtime work:

```bash
bun test src/core/connector.test.ts
bun test src/core/builtins.test.ts
bun test src/mcp/mcp.test.ts
bun test src/server/server-connector-routes.test.ts
```

## Pitfalls

- Do not publish or bump versions unless explicitly requested.
- Do not add standalone per-project copied connector source as the primary model; `.connectors/manifest.json` is enablement, not source ownership.
- Do not leak credentials through operation inputs, action specs, run evidence, or MCP tool responses.
- Do not assume legacy connector CLI fallback means new connectors should skip internal runtime definitions.
