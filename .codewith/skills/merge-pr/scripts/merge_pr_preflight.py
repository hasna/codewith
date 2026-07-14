#!/usr/bin/env python3
"""Read-only merge-pr preflight snapshot helper.

The helper can read a fixture JSON or query GitHub with read-only `gh` commands.
It does not fetch, checkout, push, comment, label, approve, close, enqueue, or
merge. The output is advisory and must be rechecked by the merge executor.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any


VERDICTS = {"mergeable", "not_mergeable", "needs_review", "pending", "unknown"}
ACTUAL_MERGE_MODES = {"immediate-merge", "auto-merge", "merge-queue"}
APPROVED_REVIEW_DECISIONS = {"APPROVED"}
BLOCKING_REVIEW_DECISIONS = {"CHANGES_REQUESTED", "REVIEW_REQUIRED"}
BLOCKING_MERGE_STATE_STATUSES = {"BLOCKED", "DIRTY", "UNKNOWN"}
PENDING_MERGE_STATE_STATUSES = {"BEHIND", "UNSTABLE"}
CHECK_STATE_FIELDS = ("bucket", "conclusion", "state", "status")
BLOCKING_CHECK_STATES = {
    "action_required",
    "cancel",
    "cancelled",
    "fail",
    "failed",
    "failure",
    "stale",
    "startup_failure",
    "timed_out",
}
PENDING_CHECK_STATES = {
    "expected",
    "in_progress",
    "pending",
    "queued",
    "requested",
    "waiting",
}


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def run_json(command: list[str]) -> Any:
    result = subprocess.run(command, check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return json.loads(result.stdout or "null")


def load_json(path: str | None) -> Any:
    if not path:
        return None
    return json.loads(Path(path).read_text())


def normalize_repo(repo: str | None, pr_view: dict[str, Any] | None) -> str | None:
    if repo:
        return repo
    if not pr_view:
        return None
    url = pr_view.get("url") or ""
    marker = "github.com/"
    if marker in url:
        owner_repo = url.split(marker, 1)[1].split("/pull/", 1)[0]
        if owner_repo.count("/") == 1:
            return owner_repo
    return None


def parse_timestamp(value: Any) -> datetime | None:
    if not isinstance(value, str) or not value.strip():
        return None
    text = value.strip()
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def review_decision(review_decision_value: str | None, reviews: list[dict[str, Any]]) -> tuple[str | None, list[str], list[str]]:
    blockers: list[str] = []
    warnings: list[str] = []
    normalized_decision = str(review_decision_value or "").upper()
    if normalized_decision in BLOCKING_REVIEW_DECISIONS:
        blockers.append(f"review_decision_{normalized_decision.lower()}")
        return "needs_review", blockers, warnings
    if normalized_decision and normalized_decision not in APPROVED_REVIEW_DECISIONS:
        warnings.append(f"review_decision_{normalized_decision.lower()}")

    latest_by_author: dict[str, str] = {}
    for review in reviews:
        author = (review.get("author") or {}).get("login") or "unknown"
        state = review.get("state") or "UNKNOWN"
        latest_by_author[author] = state
    if any(state == "CHANGES_REQUESTED" for state in latest_by_author.values()):
        blockers.append("review_changes_requested")
        return "needs_review", blockers, warnings
    if not any(state == "APPROVED" for state in latest_by_author.values()):
        warnings.append("no_approving_review_observed")
    return None, blockers, warnings


def normalize_check_state(value: Any) -> str | None:
    if value is None:
        return None
    normalized = str(value).strip().lower().replace("-", "_").replace(" ", "_")
    return normalized or None


def check_states(check: dict[str, Any]) -> set[str]:
    states: set[str] = set()
    for field in CHECK_STATE_FIELDS:
        normalized = normalize_check_state(check.get(field))
        if normalized:
            states.add(normalized)
    return states


def checks_decision(checks: list[dict[str, Any]]) -> tuple[str | None, list[str]]:
    if not checks:
        return None, ["no_checks_observed"]
    normalized = set().union(*(check_states(check) for check in checks))
    if normalized & BLOCKING_CHECK_STATES:
        return "not_mergeable", ["checks_not_successful"]
    if normalized & PENDING_CHECK_STATES:
        return "pending", ["checks_pending"]
    return None, []


def artifact_decision(
    artifacts: list[dict[str, Any]],
    repo: str | None,
    pr_number: int | None,
    head_sha: str | None,
    *,
    actual_merge_mode: bool,
    max_age_hours: float,
) -> tuple[list[str], list[str]]:
    blockers: list[str] = []
    warnings: list[str] = []
    now = datetime.now(timezone.utc)
    freshness_cutoff = now - timedelta(hours=max_age_hours)
    if not artifacts:
        target = blockers if actual_merge_mode else warnings
        target.append("no_reviewer_artifacts_provided")
        return blockers, warnings

    identities: set[str] = set()
    valid_count = 0
    for index, artifact in enumerate(artifacts, start=1):
        prefix = f"artifact_{index}"
        artifact_repo = artifact.get("repository") or artifact.get("repo")
        artifact_pr = artifact.get("pr_number")
        if artifact_repo is None:
            blockers.append(f"{prefix}_missing_repository")
        elif artifact_repo != repo:
            blockers.append(f"{prefix}_repo_mismatch")
        if artifact_pr is None:
            blockers.append(f"{prefix}_missing_pr_number")
        elif artifact_pr != pr_number:
            blockers.append(f"{prefix}_pr_mismatch")
        if not artifact.get("head_sha"):
            blockers.append(f"{prefix}_missing_head_sha")
        elif artifact.get("head_sha") != head_sha:
            blockers.append(f"{prefix}_head_sha_mismatch")
        identity = artifact.get("reviewer_identity") or artifact.get("reviewer_run_id")
        if not identity:
            blockers.append(f"{prefix}_missing_reviewer_identity")
        else:
            identities.add(str(identity))
        raw_timestamp = artifact.get("timestamp")
        timestamp = parse_timestamp(raw_timestamp)
        if timestamp is None:
            reason = "missing_timestamp" if not raw_timestamp else "invalid_timestamp"
            blockers.append(f"{prefix}_{reason}")
        elif timestamp > now:
            blockers.append(f"{prefix}_future_timestamp")
        elif timestamp < freshness_cutoff:
            blockers.append(f"{prefix}_stale_timestamp")
        if not artifact.get("checked_risks_summary"):
            blockers.append(f"{prefix}_missing_checked_risks_summary")
        verdict = str(artifact.get("verdict") or "").strip().lower()
        if verdict not in {"approve", "approved", "pass", "no_blockers"}:
            blockers.append(f"{prefix}_non_passing_verdict")
        if "blocking_findings" not in artifact:
            blockers.append(f"{prefix}_missing_blocking_findings")
        elif artifact.get("blocking_findings"):
            blockers.append(f"{prefix}_blocking_findings")
        valid_count += 1

    if valid_count < 2:
        target = blockers if actual_merge_mode else warnings
        target.append("fewer_than_two_reviewer_artifacts")
    if len(identities) < min(valid_count, 2):
        blockers.append("reviewer_artifacts_not_independent")
    return blockers, warnings


def merge_state_decision(pr_view: dict[str, Any]) -> tuple[str | None, list[str], list[str]]:
    blockers: list[str] = []
    warnings: list[str] = []
    mergeable = pr_view.get("mergeable")
    merge_state_status = str(pr_view.get("mergeStateStatus") or "").upper()
    if mergeable == "CONFLICTING":
        blockers.append("mergeable_conflicting")
    if merge_state_status in BLOCKING_MERGE_STATE_STATUSES:
        blockers.append(f"merge_state_status_{merge_state_status.lower()}")
    elif merge_state_status in PENDING_MERGE_STATE_STATUSES:
        warnings.append(f"merge_state_status_{merge_state_status.lower()}")
        return "pending", blockers, warnings
    return None, blockers, warnings


def build_snapshot(args: argparse.Namespace) -> dict[str, Any]:
    fixture = load_json(args.fixture)
    pr_view = fixture.get("pr_view") if isinstance(fixture, dict) and "pr_view" in fixture else fixture
    checks = fixture.get("checks", []) if isinstance(fixture, dict) else []
    artifacts = []
    for artifact_path in args.artifact:
        artifacts.append(load_json(artifact_path))

    if pr_view is None:
        fields = [
            "number",
            "url",
            "baseRefName",
            "headRefName",
            "headRefOid",
            "headRepositoryOwner",
            "isDraft",
            "mergeable",
            "mergeStateStatus",
            "reviewDecision",
            "reviews",
            "state",
        ]
        command = ["gh", "pr", "view", str(args.pr), "--json", ",".join(fields)]
        if args.repo:
            command.extend(["--repo", args.repo])
        pr_view = run_json(command)
        checks_command = [
            "gh",
            "pr",
            "checks",
            str(args.pr),
            "--json",
            "name,state,bucket,workflow,startedAt,completedAt,link",
        ]
        if args.repo:
            checks_command.extend(["--repo", args.repo])
        try:
            checks = run_json(checks_command)
        except subprocess.CalledProcessError as error:
            checks = []
            print(f"warning: unable to read checks: {error.stderr.strip()}", file=sys.stderr)

    repo = normalize_repo(args.repo, pr_view)
    pr_number = pr_view.get("number") or args.pr
    head_owner = ((pr_view.get("headRepositoryOwner") or {}).get("login")) if isinstance(pr_view.get("headRepositoryOwner"), dict) else None
    head_name = pr_view.get("headRefName")
    head = f"{head_owner}:{head_name}" if head_owner and head_name else head_name
    head_sha = pr_view.get("headRefOid")

    blocking_reasons: list[str] = []
    warnings: list[str] = []
    verdict = "unknown"

    state = pr_view.get("state")
    if state and state != "OPEN":
        blocking_reasons.append(f"pr_state_{state.lower()}")
    if pr_view.get("isDraft"):
        blocking_reasons.append("draft_pr")

    merge_state_verdict, merge_state_blockers, merge_state_warnings = merge_state_decision(pr_view)
    review_verdict, review_blockers, review_warnings = review_decision(
        pr_view.get("reviewDecision"), pr_view.get("reviews") or []
    )
    check_verdict, check_notes = checks_decision(checks or [])
    artifact_blockers, artifact_warnings = artifact_decision(
        artifacts,
        repo,
        pr_number,
        head_sha,
        actual_merge_mode=args.mode in ACTUAL_MERGE_MODES,
        max_age_hours=args.max_artifact_age_hours,
    )
    blocking_reasons.extend(merge_state_blockers)
    blocking_reasons.extend(review_blockers)
    warnings.extend(merge_state_warnings)
    warnings.extend(review_warnings)
    if check_verdict == "pending":
        warnings.extend(check_notes)
    else:
        blocking_reasons.extend(check_notes)
    blocking_reasons.extend(artifact_blockers)
    warnings.extend(artifact_warnings)

    if blocking_reasons:
        verdict = "not_mergeable"
    elif review_verdict:
        verdict = review_verdict
    elif merge_state_verdict:
        verdict = merge_state_verdict
    elif check_verdict:
        verdict = check_verdict
    elif pr_view.get("mergeable") == "MERGEABLE":
        verdict = "mergeable"

    if verdict not in VERDICTS:
        verdict = "unknown"

    allowed_actions = ["preflight"]
    if verdict in {"mergeable", "pending"}:
        allowed_actions.append("collect_reviewer_artifacts")

    return {
        "mode": args.mode,
        "verdict": verdict,
        "repo": repo,
        "pr_number": pr_number,
        "pr_url": pr_view.get("url"),
        "base": pr_view.get("baseRefName"),
        "head": head,
        "head_sha": head_sha,
        "merge_state": {
            "state": state,
            "is_draft": bool(pr_view.get("isDraft")),
            "mergeable": pr_view.get("mergeable"),
            "merge_state_status": pr_view.get("mergeStateStatus"),
            "review_decision": pr_view.get("reviewDecision"),
        },
        "checks": checks or [],
        "reviews": pr_view.get("reviews") or [],
        "reviewer_artifacts": artifacts,
        "active_goal": None,
        "allowed_actions": allowed_actions,
        "blocking_reasons": blocking_reasons,
        "warnings": warnings,
        "recommended_next_step": recommended_next_step(verdict, blocking_reasons, warnings),
        "observed_at": utc_now(),
    }


def recommended_next_step(verdict: str, blockers: list[str], warnings: list[str]) -> str:
    if blockers:
        return "Resolve blocking reasons before considering merge."
    if verdict == "pending":
        return "Wait for required checks or use auto-merge only if the user explicitly asked for merge-when-ready."
    if "no_reviewer_artifacts_provided" in warnings or "fewer_than_two_reviewer_artifacts" in warnings:
        return "Obtain two independent reviewer artifacts for the exact head SHA before actual merge."
    if verdict == "mergeable":
        return "Create the Codewith merge goal plan, verify reviewer artifacts, then executor recheck before merge."
    return "Inspect PR state manually before taking merge action."


def main() -> int:
    parser = argparse.ArgumentParser(description="Create a read-only merge-pr preflight JSON snapshot.")
    parser.add_argument("pr", nargs="?", type=int, help="GitHub PR number")
    parser.add_argument("--repo", help="GitHub repository as OWNER/REPO")
    parser.add_argument("--mode", default="preflight", choices=["preflight", "immediate-merge", "auto-merge", "merge-queue"])
    parser.add_argument("--fixture", help="Read PR/check data from fixture JSON instead of GitHub")
    parser.add_argument("--artifact", action="append", default=[], help="Reviewer artifact JSON file; may be repeated")
    parser.add_argument(
        "--max-artifact-age-hours",
        type=float,
        default=24.0,
        help="Maximum reviewer artifact age before it is stale (default: 24).",
    )
    args = parser.parse_args()

    if not args.fixture and not args.pr:
        parser.error("PR number is required unless --fixture is provided")

    snapshot = build_snapshot(args)
    print(json.dumps(snapshot, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
