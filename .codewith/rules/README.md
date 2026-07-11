# Codewith Local Rules

No formal `.codewith/rules` loader, schema, or frontmatter convention was found in this repo. Treat these Markdown files as repo-local routing guidance for humans and agents.

These rules do not override `.codewith/CODEWITH.md`; read that file first. Keep each rule concise, app-specific, and tied to paths that exist in this worktree.

When updating this directory:

- Prefer one focused Markdown file per durable topic.
- Verify referenced source paths still exist.
- Link to existing skills in `.codewith/skills/` instead of duplicating long workflow instructions.
- Keep generated or user-facing product docs out of `.codewith/rules`.
