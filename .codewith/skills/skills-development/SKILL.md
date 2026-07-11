---
name: skills-development
description: "Create, update, validate, or debug repo-local Codewith skills in open-codewith. Use when authoring .codewith/skills, SKILL.md frontmatter, agents/openai.yaml, skill trigger descriptions, skill discovery/loading, /skills UI, explicit skill invocation, or skills/list behavior."
---

# Skills Development

## Start Here

1. Read `.codewith/CODEWITH.md` and `docs/skills.md`.
2. Inspect nearby examples in `.codewith/skills/` before adding a new pattern.
3. For new skills, create only the files the skill needs: `SKILL.md` is required, `agents/openai.yaml` is useful for UI metadata, and `scripts/`, `references/`, or `assets/` should exist only when they directly support the workflow.

## Key Surfaces

- Repo-local skills: `.codewith/skills/<skill-name>/SKILL.md`
- UI metadata: `.codewith/skills/<skill-name>/agents/openai.yaml`
- Skill loading and metadata: `codex-rs/core-skills/src/loader.rs`, `model.rs`, `render.rs`
- Skill loader coverage: `codex-rs/core-skills/src/loader_tests.rs`
- TUI skill surfaces: `codex-rs/tui/src/bottom_pane/skill_popup.rs`, `codex-rs/tui/src/chatwidget/skills.rs`
- App-server skill APIs: `codex-rs/app-server/src/request_processors/catalog_processor.rs`, `codex-rs/app-server/README.md`

## Workflow

1. Choose a concise kebab-case skill name. Avoid redundant product prefixes for repo-local skills unless matching an existing family.
2. Make the frontmatter `description` trigger-rich: include user phrases, concrete task names, and key repo terms.
3. Keep `SKILL.md` focused on operational guidance: what to inspect, what to change, how to validate, and what to avoid.
4. Put detailed references in `references/` only when the body would become large or variant-specific.
5. If adding `agents/openai.yaml`, include quoted `display_name`, 25-64 character `short_description`, and a short `default_prompt` that mentions `$<skill-name>`.
6. Keep generated user-facing docs out of skill folders.

## Validation

For skill-only content changes:

```bash
python3 - <<'PY'
from pathlib import Path
import yaml
for path in sorted(Path(".codewith/skills").glob("*/SKILL.md")):
    text = path.read_text()
    assert text.startswith("---\n"), path
    fm = text.split("---", 2)[1]
    data = yaml.safe_load(fm)
    assert data.get("name") and data.get("description"), path
print("skill frontmatter ok")
PY
```

If loader, rendering, or API code changes:

```bash
cd codex-rs
just test-fast -p codex-core-skills
just test-fast -p codex-app-server
just test-fast -p codex-tui skill_popup
```

## Pitfalls

- Do not bury essential trigger terms only in the body; only frontmatter is always visible for selection.
- Do not add README, CHANGELOG, or install guides inside a skill directory.
- Do not duplicate long reference content between `SKILL.md` and `references/`.
- Do not assume `.agents/skills` when this repo already uses `.codewith/skills` for local skills.
