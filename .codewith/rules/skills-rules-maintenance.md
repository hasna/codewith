# Skills And Rules Maintenance

Project instructions live in `.codewith/CODEWITH.md`. Repo-local skills live in `.codewith/skills/<skill-name>/SKILL.md`; `docs/skills.md` documents the local skill concept.

Skill source surfaces:

- `.codewith/skills/` for repo-local skill guidance.
- `codex-rs/core-skills/src/` for skill loading, metadata, rendering, invocation helpers, config rules, and system skills.
- `codex-rs/ext/skills/src/` for the skills extension/provider API.
- `codex-rs/tui/src/bottom_pane/skill_popup.rs` and `codex-rs/tui/src/chatwidget/skills.rs` for TUI surfaces.
- `codex-rs/app-server/README.md` and `codex-rs/app-server/src/request_processors/catalog_processor.rs` for `skills/list` and related APIs.

Maintenance rules:

- Add a new skill only when a reusable workflow needs more than a short prompt. Prefer updating an existing focused skill when one already owns the work.
- Keep `SKILL.md` frontmatter limited to supported fields, with a trigger-rich `description`.
- `agents/openai.yaml` is optional UI metadata; keep it short and consistent with nearby examples.
- Do not add generated docs, changelogs, or broad product docs inside skill directories.
- Keep `.codewith/rules` Markdown files short and source-backed. Rules are a routing layer, not a replacement for skills or `.codewith/CODEWITH.md`.
- If a rule references a path, verify the path exists before landing the change.

Validation:

```bash
for skill in .codewith/skills/*; do
  python3 codex-rs/skills/src/assets/samples/skill-creator/scripts/quick_validate.py "$skill"
done

python3 - <<'PY'
from pathlib import Path
extra = []
for path in Path(".codewith/skills").rglob("*"):
    if path.is_file() and path.name.lower() in {"readme.md", "changelog.md"}:
        extra.append(str(path))
if extra:
    raise SystemExit("unexpected skill docs:\n" + "\n".join(extra))
print("skill clutter scan ok")
PY
```
