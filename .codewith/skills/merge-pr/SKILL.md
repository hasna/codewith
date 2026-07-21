---
name: merge-pr
description: Use when the user asks to merge a GitHub PR, merge when green, enable auto-merge, add to a merge queue, or decide whether a PR is mergeable. Do not use for ordinary PR review unless there is explicit merge intent.
---

# Merge PR

Use this skill for Codewith-native GitHub PR merge decisions and execution. Do not use it for ordinary review without merge intent, and do not modify or replace `scancommitpr`; that workflow must not auto-merge.

For the durable safety rationale behind these gates, see [merge-safety.md](references/merge-safety.md).

## Modes

- `preflight`: read-only advisory check. It may run before creating a Codewith goal plan. It must not mutate GitHub or local git state.
- `immediate-merge`: merge now after all gates pass.
- `auto-merge`: enable merge when ready only when the user explicitly asks for delayed intent such as "merge when green" or "enable auto-merge".
- `merge-queue`: enqueue or enable queue-when-ready. Merge queue is an execution mode, not a merge strategy.

## Non-Negotiable Gates

Actual merge means `immediate-merge`, `auto-merge`, or `merge-queue`.

1. For actual merge, create a native Codewith goal plan when available. Do not clear or replace an active goal unless the user explicitly authorizes it. The plan must track: `preflight`, `reviewer-artifact-1`, `reviewer-artifact-2`, `executor-recheck-merge`, and `postverify`.
2. Require two independent reviewer artifacts from separate reviewer runs tied to the exact PR head SHA. Self-review is not an acceptable fallback for actual merge. If two independent artifacts cannot be obtained, stop before merge.
3. Each reviewer artifact must include: repository, PR number, exact head SHA, reviewer identity or run id, timestamp, verdict, checked risks summary, and blocking findings. Treat missing, invalid, future, or stale artifact timestamps as blockers; the helper default staleness window is 24 hours unless `--max-artifact-age-hours` is explicitly set.
4. The executor must re-fetch and re-check immediately before merge. A preflight JSON snapshot is advisory only and is not authority to merge.
5. The merge command must include `gh pr merge <pr> --match-head-commit <head_sha>` and must never include `--admin`.

## Workflow

1. Resolve the repository and PR number or URL. Confirm merge intent and choose one mode.
2. Run read-only preflight. You may use `scripts/merge_pr_preflight.py` or equivalent read-only `gh pr view` / `gh pr checks` commands. Do not fetch, checkout, push, comment, label, approve, close, or merge during preflight.
3. For read-only mergeability questions, report the preflight verdict, blocking reasons, warnings, and recommended next step. Stop there unless the user asks to merge.
4. For actual merge, create or append to the native Codewith goal plan without replacing an active goal unless authorized. Record the PR head SHA in the plan.
5. Obtain two independent reviewer artifacts for that exact head SHA. Treat missing, stale, duplicate-run, self-review, or blocking-verdict artifacts as blockers.
6. Executor recheck immediately before merge:
   - `git fetch` or equivalent remote refresh.
   - Re-read PR state, head SHA, base, mergeability, checks, reviews, draft/conflict state, and queue/protection behavior.
   - Verify the current head SHA still equals the reviewed SHA. If it changed, stop and get new reviewer artifacts.
7. Build the `gh pr merge` command:
   - Always include `--match-head-commit <head_sha>`.
   - Never include `--admin`.
   - Use `--auto` only for explicit delayed intent.
   - For queue branches, follow local `gh pr merge --help`: queue-required branches need no strategy; passed checks enqueue; pending checks enable merge-when-ready only under GitHub queue semantics and only when the user explicitly asked for delayed merge intent.
8. Execute once all gates pass, then postverify.

## Preflight JSON

Use this shape for the read-only preflight snapshot. It is not the reviewer artifact schema, although it may include reviewer artifact summaries. `verdict` must be exactly one of `mergeable`, `not_mergeable`, `needs_review`, `pending`, or `unknown`. Include `observed_at` so staleness is explicit.

```json
{
  "mode": "preflight",
  "verdict": "mergeable|not_mergeable|needs_review|pending|unknown",
  "repo": "OWNER/REPO",
  "pr_number": 123,
  "pr_url": "https://github.com/OWNER/REPO/pull/123",
  "base": "main",
  "head": "user:branch",
  "head_sha": "40-char-sha",
  "merge_state": {
    "state": "OPEN",
    "is_draft": false,
    "mergeable": "MERGEABLE",
    "merge_state_status": "CLEAN"
  },
  "checks": [],
  "reviews": [],
  "reviewer_artifacts": [],
  "active_goal": null,
  "allowed_actions": ["preflight"],
  "blocking_reasons": [],
  "warnings": [],
  "recommended_next_step": "Obtain two independent reviewer artifacts for the exact head SHA before merge.",
  "observed_at": "2026-07-11T00:00:00Z"
}
```

## Postverify

Record the PR state, merged commit or queue state, target branch state, CI/check state when available, command used excluding secrets, and final outcome. If merge did not happen, record the exact gate that stopped execution.

## Validation Guidance

Validate the contract with static checks and fixtures:

- Static checks: trigger text present, `scancommitpr` non-goal present, four modes present, two independent reviewer artifacts required, no self-review fallback for actual merge, executor recheck required, `--match-head-commit` required, `--admin` forbidden, postverify fields present.
- Fixtures: green PR with two artifacts, no reviewer artifacts, one artifact, stale artifact head SHA, pending checks, failed checks, requested changes, draft PR, conflict PR, merge queue passed checks, merge queue pending checks, explicit merge-when-green, head SHA changes between preflight and executor, branch protection queue, and no generated command containing `--admin`.
