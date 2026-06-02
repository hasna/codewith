import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
BAZEL_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "bazel.yml"


class BazelWorkflowTest(unittest.TestCase):
    def test_keyless_bazel_matrix_does_not_schedule_macos(self) -> None:
        workflow = yaml.safe_load(BAZEL_WORKFLOW.read_text(encoding="utf-8"))

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


if __name__ == "__main__":
    unittest.main()
