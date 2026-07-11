---
name: codewith-git-ship
description: Create logical commits and push Codewith changes safely. Use when asked to review current changes, split work into commits, avoid unrelated local work, push to GitHub, reconcile a dirty hasna/codewith checkout, or prepare commits before publishing.
---

# Codewith Git Ship

## Overview

Use this skill to move Codewith repo changes from working tree to GitHub without mixing unrelated work. This repo is `hasna/codewith`, a Codewith fork of upstream Codex.

## Guardrails

- Never revert or overwrite user changes unless the user explicitly asks.
- Never use `git reset --hard` or `git checkout -- <path>` for cleanup without explicit approval.
- Use non-interactive Git commands.
- Stage only explicit paths for the intended change. Avoid broad `git add .` in a dirty worktree.
- Keep commits logical and reviewable; prefer multiple focused commits over one mixed commit.

## Workflow

1. Inspect current state:

```bash
git status --short
git branch --show-current
git log --oneline --decorate -8
git remote -v
```

2. If the active checkout has unrelated local work, create or reuse a clean temporary worktree from `origin/main` and work there:

```bash
git fetch origin main
git worktree add --detach /tmp/codewith-current.<suffix> origin/main
```

3. Identify intended change groups with `git diff --stat`, `git diff -- <path>`, and `git status --short`. Read enough surrounding code to understand ownership and tests.
4. For each logical commit:

```bash
git add <explicit paths>
git diff --cached --stat
git diff --cached --check
git diff --cached -- <representative paths>
git commit -m "<type(scope): summary>"
```

5. Verify the commit stack before pushing:

```bash
git status --short
git log --oneline --decorate origin/main..HEAD
```

6. Push the intended feature branch to GitHub and land through a PR by default. If `git branch --show-current` is empty, create or check out an explicitly named feature branch before pushing:

```bash
branch=$(git branch --show-current)
test -n "$branch"
git push -u origin "$branch"
```

Open or refresh a PR against the confirmed default branch. Direct default-branch pushes are not the default; use one only when the human explicitly authorizes direct default-branch landing and confirms no protected-branch or PR requirement applies. In that exceptional case, use the explicit reviewed default branch name:

```bash
git push origin HEAD:<default-branch>
```

7. Confirm remote branch and PR state:

```bash
git ls-remote --heads origin <feature-branch>
git log --oneline --decorate -5
```

## Dirty Worktree Triage

When the user says to include "only what's right now", treat that as a request to isolate the current intended change from other local work. Do not assume every modified file in the root checkout is part of the requested commit.

If the intended change cannot be separated without risking user work, stop and summarize the exact conflicting paths instead of committing.

## Reporting

Report commit hashes, push target, and any tests or validation that were run. Mention unresolved dirty files only when they are in the relevant checkout and were intentionally left alone.
