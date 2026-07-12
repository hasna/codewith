import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
BAZEL_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "bazel.yml"
V8_WORKFLOWS = [
    REPO_ROOT / ".github" / "workflows" / "v8-canary.yml",
    REPO_ROOT / ".github" / "workflows" / "rusty-v8-release.yml",
]

FORK_MACOS_V8_SKIP_IF = (
    "${{ github.repository != 'openai/codex' && startsWith(matrix.runner, 'macos-') }}"
)
UPSTREAM_OR_NON_MACOS_IF = "${{ github.repository == 'openai/codex' || startsWith(matrix.runner, 'macos-') == false }}"


class BazelWorkflowTest(unittest.TestCase):
    def bazel_workflow(self) -> dict:
        return yaml.safe_load(BAZEL_WORKFLOW.read_text(encoding="utf-8"))

    def test_keyless_bazel_matrix_does_not_schedule_macos(self) -> None:
        workflow = self.bazel_workflow()

        for job_name in ("test", "clippy", "verify-release-build"):
            with self.subTest(job=job_name):
                include = workflow["jobs"][job_name]["strategy"]["matrix"]["include"]
                macos_entries = [
                    entry
                    for entry in include
                    if str(entry.get("os", "")).startswith("macos")
                    or str(entry.get("runs_on", "")).startswith("macos")
                ]
                self.assertEqual([], macos_entries)

    def test_windows_clippy_uses_effective_buildbuddy_rbe_predicate(self) -> None:
        workflow = self.bazel_workflow()
        clippy_steps = workflow["jobs"]["clippy"]["steps"]
        clippy_build_step = next(
            step
            for step in clippy_steps
            if step.get("name") == "bazel build --config=clippy lint targets"
        )
        run_script = clippy_build_step["run"]

        self.assertEqual(
            "${{ vars.CODEWITH_BAZEL_ENABLE_BUILDBUDDY_RBE }}",
            workflow["env"]["CODEWITH_BAZEL_ENABLE_BUILDBUDDY_RBE"],
        )
        self.assertIn('use_buildbuddy_rbe=0', run_script)
        self.assertIn('if [[ -n "${BUILDBUDDY_API_KEY:-}" ]]; then', run_script)
        self.assertIn(
            '"${GITHUB_REPOSITORY:-}" == "hasna/codewith"',
            run_script,
        )
        self.assertIn(
            '"${CODEWITH_BAZEL_ENABLE_BUILDBUDDY_RBE:-}" != "1"',
            run_script,
        )
        self.assertIn('if [[ $use_buildbuddy_rbe -eq 0 ]]; then', run_script)
        self.assertNotIn(
            'if [[ -z "${BUILDBUDDY_API_KEY:-}" ]]; then',
            run_script,
        )

    def test_keyless_v8_artifact_jobs_skip_macos_outside_upstream(self) -> None:
        for workflow_path in V8_WORKFLOWS:
            workflow = yaml.safe_load(workflow_path.read_text(encoding="utf-8"))
            steps = workflow["jobs"]["build"]["steps"]

            with self.subTest(workflow=workflow_path.name, step="skip"):
                self.assertEqual(
                    FORK_MACOS_V8_SKIP_IF,
                    steps[0].get("if"),
                )

            for step in steps[1:]:
                with self.subTest(workflow=workflow_path.name, step=step.get("name")):
                    self.assertEqual(UPSTREAM_OR_NON_MACOS_IF, step.get("if"))


if __name__ == "__main__":
    unittest.main()
