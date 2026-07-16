#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "merge_pr_preflight.py"
FIXTURES = Path(__file__).resolve().parent / "fixtures"


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
            "timestamp": "2099-01-01T00:00:00Z",
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


if __name__ == "__main__":
    unittest.main()
