# Codewith Project Instructions

Codewith reads project instructions from `.codewith/CODEWITH.md` by default. Root `CODEWITH.md` and legacy `AGENTS.md` files remain compatibility fallbacks.

## Includes

Project instruction files can include Markdown snippets from `.codewith/instructions` using a line-only directive:

```markdown
{{ include "rust.md" }}
{{ include "reviews/security.md" }}
```

Includes are expanded before instructions are shown to the model. Directives in root fallback `CODEWITH.md` or `AGENTS.md` files also resolve under the sibling `.codewith/instructions` directory.

Only local `.md` files under that instructions directory can be included. Nested include directives inside included files are left literal.

## Hierarchical agents message

When the `child_agents_md` feature flag is enabled (via `[features]` in `config.toml`), Codewith appends additional guidance about project-instruction scope and precedence to the user instructions message and emits that message even when no project instruction file is present.
