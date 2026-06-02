import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
SDK_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "sdk.yml"


class SdkWorkflowTest(unittest.TestCase):
    def test_sdk_job_has_local_build_timeout_budget(self) -> None:
        workflow = yaml.safe_load(SDK_WORKFLOW.read_text(encoding="utf-8"))

        timeout_minutes = workflow["jobs"]["sdks"]["timeout-minutes"]
        self.assertGreaterEqual(timeout_minutes, 45)

    def test_sdk_job_uses_cargo_built_codewith_cli(self) -> None:
        workflow = yaml.safe_load(SDK_WORKFLOW.read_text(encoding="utf-8"))
        steps = workflow["jobs"]["sdks"]["steps"]
        serialized_steps = "\n".join(str(step) for step in steps)

        self.assertIn("cargo build --locked -p codex-cli --bin codewith", serialized_steps)
        self.assertIn("CODEWITH_EXEC_PATH=${install_dir}/codewith", serialized_steps)
        self.assertIn("CODEX_EXEC_PATH=${install_dir}/codewith", serialized_steps)
        self.assertNotIn("run-bazel-ci.sh", serialized_steps)
        self.assertNotIn("setup-bazel-ci", serialized_steps)


if __name__ == "__main__":
    unittest.main()
