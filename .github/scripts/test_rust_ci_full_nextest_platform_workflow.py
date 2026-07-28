import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "rust-ci-full-nextest-platform.yml"
FULL_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "rust-ci-full.yml"
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

    def test_linux_remote_archive_uses_capacity_runner(self) -> None:
        workflow = yaml.safe_load(FULL_WORKFLOW.read_text(encoding="utf-8"))

        self.assertEqual(
            {
                "runner": "ubuntu-24.04",
                "archive_runner": "blacksmith-16vcpu-ubuntu-2404",
                "target": "x86_64-unknown-linux-gnu",
                "profile": "ci-test",
                "artifact_id": "linux-x64-remote",
                "remote_env": True,
                "use_sccache": True,
                "hosted_linux_preinstalled_tool_cleanup": True,
            },
            workflow["jobs"]["tests_linux_x64_remote"]["with"],
        )

    def test_user_namespace_setup_is_shard_only(self) -> None:
        workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
        user_namespace_step = "Enable unprivileged user namespaces (Linux)"

        self.assertNotIn(
            user_namespace_step,
            [step.get("name") for step in workflow["jobs"]["archive"]["steps"]],
        )
        self.assertIn(
            user_namespace_step,
            [step.get("name") for step in workflow["jobs"]["shard"]["steps"]],
        )


if __name__ == "__main__":
    unittest.main()
