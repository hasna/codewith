# Codewith Project Instructions

Codewith reads project instructions from `.codewith/CODEWITH.md` by default. Root `CODEWITH.md` and legacy `AGENTS.md` files remain compatibility fallbacks.

## Hierarchical agents message

When the `child_agents_md` feature flag is enabled (via `[features]` in `config.toml`), Codewith appends additional guidance about project-instruction scope and precedence to the user instructions message and emits that message even when no project instruction file is present.
