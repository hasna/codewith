---
name: worktree-pr-landing
description: Land already-validated local or worktree Codewith changes through a GitHub PR. Use when turning validated repo-local worktree changes into scoped commits, a pushed branch, PR, review/CI handling, merge after verifier approval, default-branch confirmation, post-merge verification, and safe cleanup without mixing unrelated edits.
---

# Worktree PR Landing

## Scope

Use this skill after implementation and local validation are already complete and the task is to land the change through a PR. This skill coordinates the landing path; reuse narrower skills instead of duplicating them:

- `$codewith-git-ship` for explicit-path staging, commit grouping, and safe push mechanics.
- `$codewith-pr-body` for PR title/body creation or refresh.
- `$babysit-pr` for CI, review comments, mergeability, and post-push monitoring.
- `$code-review` for adversarial review before merge when a reviewer was not already recorded.

Do not use this skill for initial implementation, one-shot PR monitoring, release publishing, or direct pushes to the default branch.

## Workflow

1. Confirm the landing target:
   - Verify the repo, worktree path, current branch, remote, and default branch.
   - Fetch the default branch before comparing: `git fetch origin <default-branch>`.
   - Confirm the diff base and review `git status --short`, `git diff --stat origin/<default-branch>...HEAD`, and `git diff --name-status origin/<default-branch>...HEAD`.

2. Prove scope:
   - Separate intended product changes from unrelated local edits.
   - Stage only explicit paths. Do not use broad `git add .` in a dirty checkout.
   - Stop before commit if generated files, task metadata, local databases, or user edits are mixed with the intended change and cannot be separated safely.

3. Validate before commit:
   - Run the task-specific verification that proves the change works.
   - Run `git diff --check` and `git diff --cached --check` after staging.
   - Run a staged secret scan without printing matched values. If the scanner reports a secret-like match, unstage/remove it before continuing.
   - Record the exact validation commands and results for the PR body and handoff.

4. Commit safely:
   - Commit only the intended staged paths.
   - Use a normal concise commit message with no `Co-Authored-By` trailers.
   - Inspect `git show --stat --oneline --decorate --no-renames HEAD` and `git status --short` after commit.

5. Push and open or refresh the PR:
   - Push the feature branch to `origin`.
   - Create the PR against the confirmed default branch unless the user specified a stacked or alternate base.
   - Include why the change exists, what changed, and verification evidence. Avoid absolute local paths and confidential details.
   - If a PR already exists, refresh its title/body instead of opening a duplicate.

6. Review, CI, and merge gate:
   - Run or consume an independent adversarial review before merge.
   - Babysit the PR until CI is green, mergeability is clean, and review feedback is handled.
   - Merge only after verifier approval is explicit in the task context or PR thread. Do not treat green CI alone as approval.
   - Use the repository's normal merge strategy and avoid force-merging around branch protection.

7. Confirm landing:
   - After merge, fetch the default branch and verify the merge commit or squash commit is reachable from `origin/<default-branch>`.
   - Run any requested post-merge smoke check against the default branch state.
   - Report the PR number, merge SHA, default branch confirmation, verification evidence, and any cleanup that remains.

8. Cleanup recommendations:
   - Delete the remote feature branch only when the PR is merged and the repository policy allows it.
   - Remove temporary worktrees only after confirming no uncommitted work remains.
   - Leave unrelated dirty checkout state untouched and report it separately if relevant.

## Stop Conditions

Stop and ask for direction instead of landing when:

- Scope is ambiguous or unrelated edits cannot be separated.
- Verification fails or is missing for a user-impacting change.
- A secret-like value appears in staged content.
- CI or review failure is not clearly branch-caused.
- Merge requires bypassing protection, force push, or undocumented approval.
