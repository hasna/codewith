import argparse
import importlib.util
from pathlib import Path

import pytest


MODULE_PATH = Path(__file__).with_name("gh_pr_watch.py")
SKILL_PATH = MODULE_PATH.parent.parent / "SKILL.md"
GITHUB_API_NOTES_PATH = MODULE_PATH.parent.parent / "references" / "github-api-notes.md"
MODULE_SPEC = importlib.util.spec_from_file_location("gh_pr_watch", MODULE_PATH)
gh_pr_watch = importlib.util.module_from_spec(MODULE_SPEC)
assert MODULE_SPEC.loader is not None
MODULE_SPEC.loader.exec_module(gh_pr_watch)


def sample_pr():
    return {
        "number": 123,
        "url": "https://github.com/openai/codex/pull/123",
        "repo": "openai/codex",
        "head_sha": "abc123",
        "head_branch": "feature",
        "state": "OPEN",
        "merged": False,
        "closed": False,
        "mergeable": "MERGEABLE",
        "merge_state_status": "CLEAN",
        "review_decision": "",
    }


def sample_checks(**overrides):
    checks = {
        "pending_count": 0,
        "failed_count": 0,
        "passed_count": 12,
        "all_terminal": True,
    }
    checks.update(overrides)
    return checks


def test_collect_snapshot_fetches_review_items_before_ci(monkeypatch, tmp_path):
    call_order = []
    pr = sample_pr()

    monkeypatch.setattr(gh_pr_watch, "resolve_pr", lambda *args, **kwargs: pr)
    monkeypatch.setattr(gh_pr_watch, "load_state", lambda path: ({}, True))
    monkeypatch.setattr(
        gh_pr_watch,
        "get_authenticated_login",
        lambda: call_order.append("auth") or "octocat",
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "fetch_new_review_items",
        lambda *args, **kwargs: call_order.append("review") or [],
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "get_pr_checks",
        lambda *args, **kwargs: call_order.append("checks") or [],
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "summarize_checks",
        lambda checks: call_order.append("summarize") or sample_checks(),
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "get_workflow_runs_for_sha",
        lambda *args, **kwargs: call_order.append("workflow") or [],
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "failed_runs_from_workflow_runs",
        lambda *args, **kwargs: call_order.append("failed_runs") or [],
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "failed_jobs_from_workflow_runs",
        lambda *args, **kwargs: call_order.append("failed_jobs") or [],
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "recommend_actions",
        lambda *args, **kwargs: call_order.append("recommend") or ["idle"],
    )
    monkeypatch.setattr(gh_pr_watch, "save_state", lambda *args, **kwargs: None)

    args = argparse.Namespace(
        pr="123",
        repo=None,
        state_file=str(tmp_path / "watcher-state.json"),
        max_flaky_retries=3,
    )

    gh_pr_watch.collect_snapshot(args)

    assert call_order.index("review") < call_order.index("checks")
    assert call_order.index("review") < call_order.index("workflow")


def test_recommend_actions_prioritizes_review_comments():
    actions = gh_pr_watch.recommend_actions(
        sample_pr(),
        sample_checks(failed_count=1),
        [{"run_id": 99}],
        [],
        [{"kind": "review_comment", "id": "1"}],
        0,
        3,
    )

    assert actions == [
        "process_review_comment",
        "diagnose_ci_failure",
        "retry_failed_checks",
    ]


def test_run_watch_keeps_polling_open_ready_to_merge_pr(monkeypatch):
    sleeps = []
    events = []
    snapshot = {
        "pr": sample_pr(),
        "checks": sample_checks(),
        "failed_runs": [],
        "failed_jobs": [],
        "new_review_items": [],
        "actions": ["ready_to_merge"],
        "retry_state": {
            "current_sha_retries_used": 0,
            "max_flaky_retries": 3,
        },
    }

    monkeypatch.setattr(
        gh_pr_watch,
        "collect_snapshot",
        lambda args: (snapshot, Path("/tmp/codex-babysit-pr-state.json")),
    )
    monkeypatch.setattr(
        gh_pr_watch,
        "print_event",
        lambda event, payload: events.append((event, payload)),
    )

    class StopWatch(Exception):
        pass

    def fake_sleep(seconds):
        sleeps.append(seconds)
        if len(sleeps) >= 2:
            raise StopWatch

    monkeypatch.setattr(gh_pr_watch.time, "sleep", fake_sleep)

    with pytest.raises(StopWatch):
        gh_pr_watch.run_watch(argparse.Namespace(poll_seconds=30))

    assert sleeps == [30, 30]
    assert [event for event, _ in events] == ["snapshot", "snapshot"]


def test_failed_jobs_include_direct_logs_endpoint(monkeypatch):
    jobs_by_run = {
        99: [
            {
                "id": 555,
                "name": "unit tests",
                "status": "completed",
                "conclusion": "failure",
                "html_url": "https://github.com/openai/codex/actions/runs/99/job/555",
            },
            {
                "id": 556,
                "name": "lint",
                "status": "completed",
                "conclusion": "success",
            },
        ]
    }

    monkeypatch.setattr(
        gh_pr_watch,
        "get_jobs_for_run",
        lambda repo, run_id: jobs_by_run[run_id],
    )

    failed_jobs = gh_pr_watch.failed_jobs_from_workflow_runs(
        "openai/codex",
        [
            {
                "id": 99,
                "name": "CI",
                "status": "in_progress",
                "conclusion": "",
                "head_sha": "abc123",
            }
        ],
        "abc123",
    )

    assert failed_jobs == [
        {
            "run_id": 99,
            "workflow_name": "CI",
            "run_status": "in_progress",
            "run_conclusion": "",
            "job_id": 555,
            "job_name": "unit tests",
            "status": "completed",
            "conclusion": "failure",
            "logs_endpoint": "repos/openai/codex/actions/jobs/555/logs",
        }
    ]


def test_check_metadata_is_projected_without_urls(monkeypatch):
    synthetic_capability_url = (
        "https://ci.example.invalid/enable-autofix?signature=synthetic-only"
    )
    gh_calls = []

    def fake_gh_json(args, repo=None):
        gh_calls.append({"args": args, "repo": repo})
        if args[:2] == ["pr", "checks"]:
            return [
                {
                    "name": "unit tests",
                    "state": "FAILURE",
                    "bucket": "fail",
                    "link": synthetic_capability_url,
                    "detailsUrl": synthetic_capability_url,
                }
            ]
        if args[1].endswith("/actions/runs"):
            return [
                {
                    "id": 99,
                    "name": "CI",
                    "status": "completed",
                    "conclusion": "failure",
                    "head_sha": "abc123",
                    "html_url": synthetic_capability_url,
                }
            ]
        return [
            {
                "id": 555,
                "name": "unit tests",
                "status": "completed",
                "conclusion": "failure",
                "html_url": synthetic_capability_url,
            }
        ]

    monkeypatch.setattr(gh_pr_watch, "gh_json", fake_gh_json)

    checks = gh_pr_watch.get_pr_checks("123", "openai/codex")
    workflow_runs = gh_pr_watch.get_workflow_runs_for_sha("openai/codex", "abc123")

    assert {
        "gh_calls": gh_calls,
        "checks": checks,
        "failed_runs": gh_pr_watch.failed_runs_from_workflow_runs(
            workflow_runs, "abc123"
        ),
        "failed_jobs": gh_pr_watch.failed_jobs_from_workflow_runs(
            "openai/codex", workflow_runs, "abc123"
        ),
    } == {
        "gh_calls": [
            {
                "args": [
                    "pr",
                    "checks",
                    "123",
                    "--json",
                    "name,state,bucket",
                ],
                "repo": "openai/codex",
            },
            {
                "args": [
                    "api",
                    "repos/openai/codex/actions/runs",
                    "-X",
                    "GET",
                    "-f",
                    "head_sha=abc123",
                    "-f",
                    "per_page=100",
                    "--jq",
                    gh_pr_watch.WORKFLOW_RUNS_JQ,
                ],
                "repo": "openai/codex",
            },
            {
                "args": [
                    "api",
                    "repos/openai/codex/actions/runs/99/jobs",
                    "-X",
                    "GET",
                    "-f",
                    "per_page=100",
                    "--jq",
                    gh_pr_watch.RUN_JOBS_JQ,
                ],
                "repo": "openai/codex",
            },
        ],
        "checks": [
            {
                "name": "unit tests",
                "state": "FAILURE",
                "bucket": "fail",
            }
        ],
        "failed_runs": [
            {
                "run_id": 99,
                "workflow_name": "CI",
                "status": "completed",
                "conclusion": "failure",
            }
        ],
        "failed_jobs": [
            {
                "run_id": 99,
                "workflow_name": "CI",
                "run_status": "completed",
                "run_conclusion": "failure",
                "job_id": 555,
                "job_name": "unit tests",
                "status": "completed",
                "conclusion": "failure",
                "logs_endpoint": "repos/openai/codex/actions/jobs/555/logs",
            }
        ],
    }


def test_written_check_commands_use_url_free_field_allowlists():
    skill_text = SKILL_PATH.read_text()
    api_notes_text = GITHUB_API_NOTES_PATH.read_text()

    assert {
        "skill_safe_checks": "`gh pr checks <pr-number> --json name,state,bucket`"
        in skill_text,
        "skill_safe_run": (
            "`gh run view <run-id> --json "
            "name,workflowName,conclusion,status,headSha`"
        )
        in skill_text,
        "notes_safe_checks": "`gh pr checks --json name,state,bucket`"
        in api_notes_text,
        "notes_safe_run": (
            "`gh run view <run-id> --json "
            "name,workflowName,conclusion,status,headSha`"
        )
        in api_notes_text,
        "skill_unsafe_run_absent": (
            "--json jobs,name,workflowName" not in skill_text
            and "status,url,headSha" not in skill_text
        ),
        "notes_unsafe_checks_absent": (
            "--json name,state,bucket,link" not in api_notes_text
        ),
    } == {
        "skill_safe_checks": True,
        "skill_safe_run": True,
        "notes_safe_checks": True,
        "notes_safe_run": True,
        "skill_unsafe_run_absent": True,
        "notes_unsafe_checks_absent": True,
    }
