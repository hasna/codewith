#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
RUST_CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "rust-ci.yml"


class RustCiWorkflowTest(unittest.TestCase):
    def workflow(self) -> dict:
        return yaml.safe_load(RUST_CI_WORKFLOW.read_text(encoding="utf-8"))

    def test_workflow_and_bazel_ci_changes_do_not_force_argument_comment_lint_prebuilt(
        self,
    ) -> None:
        workflow = self.workflow()
        detect_script = workflow["jobs"]["changed"]["steps"][1]["run"]
        steps = workflow["jobs"]["argument_comment_lint_prebuilt"]["steps"]
        gate_step = next(
            step
            for step in steps
            if step.get("id") == "argument_comment_lint_gate"
        )
        gate_script = gate_step["run"]

        self.assertIn(".github/actions/run-argument-comment-lint/*", detect_script)
        self.assertIn(".github/actions/setup-bazel-ci/*", detect_script)
        self.assertIn(".github/scripts/run-argument-comment-lint-bazel.sh", detect_script)
        self.assertIn(".github/scripts/compute-bazel-windows-path.ps1", detect_script)
        self.assertNotIn(".github/scripts/run-bazel-ci.sh", detect_script)
        self.assertNotIn(".github/scripts/run-bazel-query-ci.sh", detect_script)
        self.assertNotIn(".github/scripts/run_bazel_with_buildbuddy.py", detect_script)
        self.assertEqual(
            "${{ needs.changed.outputs.argument_comment_lint }}",
            gate_step["env"]["ARGUMENT_COMMENT_LINT"],
        )
        self.assertNotIn("WORKFLOWS", gate_step.get("env", {}))
        self.assertIn('[[ "$ARGUMENT_COMMENT_LINT" == "true" ]]', gate_script)
        self.assertNotIn("WORKFLOWS", gate_script)

        results_script = workflow["jobs"]["results"]["steps"][0]["run"]
        prebuilt_guard = (
            'if [[ "${NEEDS_CHANGED_OUTPUTS_ARGUMENT_COMMENT_LINT}" == '
            "'true' ]]; then"
        )
        self.assertIn(prebuilt_guard, results_script)
        self.assertNotIn(
            "NEEDS_CHANGED_OUTPUTS_ARGUMENT_COMMENT_LINT}\" == 'true' ||",
            results_script,
        )


if __name__ == "__main__":
    unittest.main()
