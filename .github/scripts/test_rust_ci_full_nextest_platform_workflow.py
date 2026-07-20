import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "rust-ci-full-nextest-platform.yml"
HOSTED_CLEANUP_IF = (
    "${{ inputs.hosted_linux_preinstalled_tool_cleanup && runner.os == 'Linux' && "
    "runner.environment == 'github-hosted' && runner.arch == 'X64' }}"
)


class RustCiFullNextestPlatformWorkflowTest(unittest.TestCase):
    def test_hosted_cleanup_is_limited_to_x64_linux_runners(self) -> None:
        workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
        cleanup_steps = [
            (job_name, step)
            for job_name, job in workflow["jobs"].items()
            for step in job.get("steps", [])
            if step.get("name") == "Free hosted runner disk space (Linux)"
        ]

        self.assertEqual(
            ["archive", "shard"],
            [job_name for job_name, _ in cleanup_steps],
        )
        self.assertEqual(
            [HOSTED_CLEANUP_IF, HOSTED_CLEANUP_IF],
            [step.get("if") for _, step in cleanup_steps],
        )

        for _, step in cleanup_steps:
            with self.subTest(job=step):
                run = step.get("run", "")
                self.assertNotIn("/opt/hostedtoolcache", run)
                self.assertNotIn("docker system prune", run)
                self.assertNotIn("apt-get clean", run)


if __name__ == "__main__":
    unittest.main()
