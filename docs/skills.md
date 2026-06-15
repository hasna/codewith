# Skills

Skills are reusable instructions that teach Codewith how to perform a specific
kind of work. A skill lives in a directory with a `SKILL.md` file and optional
supporting assets, scripts, or references.

Use skills when a workflow needs more than a short prompt, such as release
publishing, PR monitoring, UI audits, specialized test workflows, or
project-specific implementation rules.

## Using Skills

In the TUI, run:

```text
/skills
```

Codewith discovers available skills from configured skill locations and loads a
skill when the task or user request matches its description.

## Project Instructions

For repository-specific guidance, create:

```text
.codewith/CODEWITH.md
```

Codewith also supports root `CODEWITH.md` and legacy `AGENTS.md` files as
compatibility fallbacks. More specific project instruction files take
precedence for files inside their directory tree.
