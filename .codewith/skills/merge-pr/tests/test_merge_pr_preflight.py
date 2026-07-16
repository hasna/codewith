#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "merge_pr_preflight.py"
FIXTURES = Path(__file__).resolve().parent / "fixtures"


def run_preflight(fixture: str, mode: str = "preflight") -> dict[str, object]:
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--fixture", str(FIXTURES / fixture), "--mode", mode],
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


if __name__ == "__main__":
    unittest.main()
