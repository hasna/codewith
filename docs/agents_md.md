# Codewith Project Instructions

Codewith reads project instructions from `.codewith/CODEWITH.md` by default. Root `CODEWITH.md` and legacy `AGENTS.md` files remain compatibility fallbacks.

## Discovery Order

For each directory from the detected project root down to the session cwd, Codewith loads the first matching instruction file in this order:

1. `.codewith/CODEWITH.override.md`
2. `.codewith/CODEWITH.md`
3. `CODEWITH.override.md`
4. `CODEWITH.md`
5. `AGENTS.override.md`
6. `AGENTS.md`
7. Any configured `project_doc_fallback_filenames`, in config order

Documents are concatenated from the project root toward the cwd. More deeply nested instruction files therefore appear later and take precedence when guidance conflicts.

For each directory in that same root-to-cwd walk, Codewith also loads repository-local rule files from `.codewith/rules/**/*.md` after that directory's selected instruction document. Rule files are returned in sorted path order, rule discovery is bounded by depth/file-count limits, and symlinked rule directories or files are skipped.

## Imports

Instruction files can import reusable fragments with a whole-line directive:

```md
@relative/path.md
@rules
```

Only relative import paths are supported. They resolve from the file containing the directive. Project imports must stay inside the detected project root. Global imports from `$CODEWITH_HOME/CODEWITH.md` must stay inside `$CODEWITH_HOME`, which lets profile-managed fragments live under locations such as `$CODEWITH_HOME/profiles/marcus.md` or `$CODEWITH_HOME/rules/`.

Direct file imports and directory imports are limited to instruction-like `.md`, `.mdc`, and `.txt` files. When an import target is a directory, Codewith loads immediate supported files in filename order. Directory imports are intended for rule folders and are not recursive.

Import expansion is bounded: cycles are skipped, symlink imports are not followed, nested imports have a maximum depth, import count is capped, and oversized imported files are truncated. Every imported file that contributes text is included in the loaded instruction source list shown by status/debug surfaces.

## Hierarchical agents message

When the `child_agents_md` feature flag is enabled (via `[features]` in `config.toml`), Codewith appends additional guidance about project-instruction scope and precedence to the user instructions message and emits that message even when no project instruction file is present.
