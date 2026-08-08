# GitHub CLI / API Notes For `babysit-pr`

## Primary commands used

### PR metadata

- `gh pr view --json number,url,state,mergedAt,closedAt,headRefName,headRefOid,headRepository,headRepositoryOwner`

Used to resolve PR number, URL, branch, head SHA, and closed/merged state.

### PR checks summary

- `gh pr checks --json name,state,bucket`

Used to compute pending/failed/passed counts and whether the current CI round is terminal.

### Workflow runs for head SHA

- `gh api repos/{owner}/{repo}/actions/runs -X GET -f head_sha=<sha> -f per_page=100 --jq '[.workflow_runs[] | {id, name, status, conclusion, head_sha}]'`

Used to discover failed workflow runs and rerunnable run IDs.

### Failed log inspection

- `gh run view <run-id> --json name,workflowName,conclusion,status,headSha`
- `gh api repos/{owner}/{repo}/actions/runs/{run_id}/jobs -X GET -f per_page=100 --jq '[.jobs[] | {id, name, status, conclusion}]'`
- `gh api repos/{owner}/{repo}/actions/jobs/{job_id}/logs > /tmp/codex-gh-job-{job_id}-logs.zip`
- `gh run view <run-id> --log-failed`

Used by Codewith to classify branch-related vs flaky/unrelated failures. Prefer the direct job log endpoint as soon as a job has failed because `gh run view --log-failed` may not produce failed-job logs until the overall workflow run completes.

### Retry failed jobs only

- `gh run rerun <run-id> --failed`

Reruns only failed jobs (and dependencies) for a workflow run.

## Review-related endpoints

- Issue comments on PR:
  - `gh api repos/{owner}/{repo}/issues/<pr_number>/comments?per_page=100`
- Inline PR review comments:
  - `gh api repos/{owner}/{repo}/pulls/<pr_number>/comments?per_page=100`
- Review submissions:
  - `gh api repos/{owner}/{repo}/pulls/<pr_number>/reviews?per_page=100`

## JSON fields consumed by the watcher

Check, workflow-run, and job metadata is projected at the helper boundary. Never request or emit `statusCheckRollup`, `detailsUrl`, `link`, `url`, or `html_url` for those surfaces; use only the allowlisted fields below. Run and job IDs plus the relative logs endpoint preserve diagnosis and rerun behavior without carrying capability-bearing URLs.

### `gh pr view`

- `number`
- `url`
- `state`
- `mergedAt`
- `closedAt`
- `headRefName`
- `headRefOid`

### `gh pr checks`

- `name`
- `state`
- `bucket` (`pass`, `fail`, `pending`, `skipping`)

### Actions runs API (`workflow_runs[]`)

- `id`
- `name`
- `status`
- `conclusion`
- `head_sha`

### Actions run jobs API (`jobs[]`)

- `id`
- `name`
- `status`
- `conclusion`
