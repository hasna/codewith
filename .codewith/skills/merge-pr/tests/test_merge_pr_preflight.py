#!/usr/bin/env python3
from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "merge_pr_preflight.py"
FIXTURES = Path(__file__).resolve().parent / "fixtures"


def _load_module():
    spec = importlib.util.spec_from_file_location("merge_pr_preflight", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    spec.loader.exec_module(module)
    return module


preflight = _load_module()


def run_preflight(fixture: str, mode: str = "preflight", artifacts: list[Path] | None = None) -> dict[str, object]:
    command = [sys.executable, str(SCRIPT), "--fixture", str(FIXTURES / fixture), "--mode", mode]
    for artifact in artifacts or []:
        command.extend(["--artifact", str(artifact)])
    result = subprocess.run(
        command,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return json.loads(result.stdout)


class MergePrPreflightTests(unittest.TestCase):
    def test_current_success_shape_is_mergeable_with_artifact_warning(self) -> None:
        snapshot = run_preflight("checks_success_current.json")

        self.assertEqual(snapshot["verdict"], "mergeable")
        self.assertNotIn("checks_not_successful", snapshot["blocking_reasons"])
        self.assertIn("no_reviewer_artifacts_provided", snapshot["warnings"])

    def test_old_gh_schema_success_is_mergeable(self) -> None:
        # Old gh / statusCheckRollup shape uses conclusion/status, not state/bucket.
        snapshot = run_preflight("checks_success_oldschema.json")

        self.assertEqual(snapshot["verdict"], "mergeable")
        self.assertNotIn("checks_not_successful", snapshot["blocking_reasons"])
        self.assertNotIn("no_checks_observed", snapshot["blocking_reasons"])
        self.assertIsNone(snapshot["checks_error"])

    def test_no_check_pr_blocks_with_no_checks_observed(self) -> None:
        snapshot = run_preflight("checks_none_empty.json")

        self.assertEqual(snapshot["verdict"], "not_mergeable")
        self.assertIn("no_checks_observed", snapshot["blocking_reasons"])
        self.assertNotIn("checks_command_error", snapshot["blocking_reasons"])

    def test_backcompat_failure_and_cancelled_checks_block(self) -> None:
        snapshot = run_preflight("checks_failure_backcompat.json")

        self.assertEqual(snapshot["verdict"], "not_mergeable")
        self.assertIn("checks_not_successful", snapshot["blocking_reasons"])

    def test_pending_checks_warn_without_blocking(self) -> None:
        snapshot = run_preflight("checks_pending_mixed.json")

        self.assertEqual(snapshot["verdict"], "pending")
        self.assertIn("checks_pending", snapshot["warnings"])
        self.assertNotIn("checks_pending", snapshot["blocking_reasons"])

    def test_missing_artifacts_block_actual_merge_modes(self) -> None:
        snapshot = run_preflight("checks_success_current.json", mode="immediate-merge")

        self.assertEqual(snapshot["verdict"], "not_mergeable")
        self.assertIn("no_reviewer_artifacts_provided", snapshot["blocking_reasons"])
        self.assertNotIn("no_reviewer_artifacts_provided", snapshot["warnings"])

    def test_uppercase_pass_artifacts_allow_actual_merge(self) -> None:
        artifact_base = {
            "repository": "hasna/codewith",
            "pr_number": 215,
            "head_sha": "1111111111111111111111111111111111111111",
            "timestamp": datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
            "verdict": "PASS",
            "checked_risks_summary": "exact head, diff scope, checks, and secret posture reviewed",
            "blocking_findings": [],
        }
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = Path(tmp_dir)
            first = tmp_path / "artifact-1.json"
            second = tmp_path / "artifact-2.json"
            first.write_text(json.dumps({**artifact_base, "reviewer_identity": "reviewer-a"}))
            second.write_text(json.dumps({**artifact_base, "reviewer_identity": "reviewer-b"}))

            snapshot = run_preflight(
                "checks_success_current.json",
                mode="immediate-merge",
                artifacts=[first, second],
            )

        self.assertEqual(snapshot["verdict"], "mergeable")
        self.assertNotIn("artifact_1_non_passing_verdict", snapshot["blocking_reasons"])
        self.assertNotIn("artifact_2_non_passing_verdict", snapshot["blocking_reasons"])
        self.assertNotIn("fewer_than_two_reviewer_artifacts", snapshot["blocking_reasons"])


def _scripted_runner(responses: list[tuple[int, str, str]]):
    calls: list[list[str]] = []

    def runner(command: list[str]) -> tuple[int, str, str]:
        calls.append(command)
        return responses[min(len(calls) - 1, len(responses) - 1)]

    runner.calls = calls  # type: ignore[attr-defined]
    return runner


class CollectChecksTests(unittest.TestCase):
    def test_parses_json_even_when_exit_code_nonzero(self) -> None:
        # Some gh versions signal pending/failure via a non-zero exit while
        # still writing the JSON body to stdout. The body must win.
        body = json.dumps([{"name": "ci", "state": "PENDING", "bucket": "pending"}])
        runner = _scripted_runner([(8, body, "")])

        checks, error = preflight.collect_checks(215, "hasna/codewith", runner=runner)

        self.assertIsNone(error)
        self.assertEqual(len(checks), 1)

    def test_feature_detects_supported_fields_and_retries(self) -> None:
        err = 'Unknown JSON field: "workflow"\nAvailable fields:\n  bucket\n  name\n  state\n'
        ok = json.dumps([{"name": "ci", "state": "SUCCESS", "bucket": "pass"}])
        runner = _scripted_runner([(1, "", err), (0, ok, "")])

        checks, error = preflight.collect_checks(215, "hasna/codewith", runner=runner)

        self.assertIsNone(error)
        self.assertEqual(len(checks), 1)
        self.assertEqual(len(runner.calls), 2)
        retry_json = runner.calls[1][runner.calls[1].index("--json") + 1]
        self.assertNotIn("workflow", retry_json)
        self.assertIn("state", retry_json)

    def test_no_checks_reported_is_empty_not_error(self) -> None:
        runner = _scripted_runner([(1, "", "no checks reported on the 'feature' branch")])

        checks, error = preflight.collect_checks(215, "hasna/codewith", runner=runner)

        self.assertEqual(checks, [])
        self.assertIsNone(error)

    def test_real_command_error_is_surfaced(self) -> None:
        runner = _scripted_runner([(1, "", "HTTP 401: Bad credentials")])

        checks, error = preflight.collect_checks(215, "hasna/codewith", runner=runner)

        self.assertIsNone(checks)
        self.assertIsNotNone(error)
        self.assertIn("401", error)

    def test_unresolvable_field_error_is_surfaced(self) -> None:
        # No "Available fields" list to recover from; minimal set still rejected.
        err = "unknown json field: everything is broken"
        runner = _scripted_runner([(1, "", err)])

        checks, error = preflight.collect_checks(215, "hasna/codewith", runner=runner)

        self.assertIsNone(checks)
        self.assertIsNotNone(error)
        self.assertLessEqual(len(runner.calls), 2)

    def test_build_snapshot_surfaces_checks_command_error(self) -> None:
        args = argparse.Namespace(
            fixture=None,
            pr=15,
            repo="hasna/repos",
            mode="preflight",
            artifact=[],
            max_artifact_age_hours=24.0,
        )
        pr_view = {
            "number": 15,
            "url": "https://github.com/hasna/repos/pull/15",
            "baseRefName": "main",
            "headRefName": "feature",
            "headRefOid": "6666666666666666666666666666666666666666",
            "isDraft": False,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "reviewDecision": "APPROVED",
            "reviews": [{"author": {"login": "reviewer-a"}, "state": "APPROVED"}],
            "state": "OPEN",
        }
        original_run_json = preflight.run_json
        original_collect = preflight.collect_checks
        try:
            preflight.run_json = lambda command: pr_view
            preflight.collect_checks = lambda pr, repo, **kw: (None, "HTTP 401: Bad credentials")
            snapshot = preflight.build_snapshot(args)
        finally:
            preflight.run_json = original_run_json
            preflight.collect_checks = original_collect

        self.assertEqual(snapshot["verdict"], "not_mergeable")
        self.assertIn("checks_command_error", snapshot["blocking_reasons"])
        self.assertNotIn("no_checks_observed", snapshot["blocking_reasons"])
        self.assertEqual(snapshot["checks_error"], "HTTP 401: Bad credentials")


if __name__ == "__main__":
    unittest.main()
