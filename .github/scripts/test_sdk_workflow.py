import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
SDK_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "sdk.yml"


class SdkWorkflowTest(unittest.TestCase):
    def test_bazel_backed_sdk_job_has_local_build_timeout_budget(self) -> None:
        workflow = yaml.safe_load(SDK_WORKFLOW.read_text(encoding="utf-8"))

        timeout_minutes = workflow["jobs"]["sdks"]["timeout-minutes"]
        self.assertGreaterEqual(timeout_minutes, 30)


if __name__ == "__main__":
    unittest.main()
