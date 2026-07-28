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
LINUX_REMOTE_RUNNER = "blacksmith-16vcpu-ubuntu-2404"
REMOTE_CAPABILITY_FILTER = "binary_id(codex-core::all) and test(/^suite::remote_env::/)"


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

    def test_linux_remote_archive_and_shards_use_capacity_runner(self) -> None:
        workflow = yaml.safe_load(FULL_WORKFLOW.read_text(encoding="utf-8"))

        self.assertEqual(
            {
                "runner": LINUX_REMOTE_RUNNER,
                "target": "x86_64-unknown-linux-gnu",
                "profile": "ci-test",
                "artifact_id": "linux-x64-remote",
                "remote_test_filter": REMOTE_CAPABILITY_FILTER,
                "use_sccache": True,
                "hosted_linux_preinstalled_tool_cleanup": True,
            },
            workflow["jobs"]["tests_linux_x64_remote"]["with"],
        )

    def test_remote_environment_is_scoped_to_capability_tests(self) -> None:
        workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
        workflow_call = workflow.get("on", workflow.get(True))["workflow_call"]
        inputs = workflow_call["inputs"]
        shard_steps = workflow["jobs"]["shard"]["steps"]
        setup_step = next(
            step
            for step in shard_steps
            if step.get("name") == "Set up remote test env (Docker)"
        )
        ordinary_test_step = next(
            step for step in shard_steps if step.get("id") == "test"
        )
        remote_test_step = next(
            step for step in shard_steps if step.get("id") == "remote_test"
        )
        junit_steps = [
            step
            for step in shard_steps
            if step.get("name") == "Upload nextest JUnit report"
        ]
        verify_step = next(
            step for step in shard_steps if step.get("name") == "verify tests passed"
        )

        self.assertNotIn("remote_env", inputs)
        self.assertEqual("string", inputs["remote_test_filter"]["type"])
        self.assertNotIn("$GITHUB_ENV", setup_step["run"])
        self.assertIn("$GITHUB_OUTPUT", setup_step["run"])
        self.assertNotIn("CODEX_TEST_REMOTE_ENV", ordinary_test_step["run"])
        self.assertEqual(
            "${{ inputs.remote_test_filter == '' }}",
            ordinary_test_step["if"],
        )
        self.assertLess(
            shard_steps.index(ordinary_test_step),
            shard_steps.index(setup_step),
        )
        self.assertLess(
            shard_steps.index(setup_step),
            shard_steps.index(remote_test_step),
        )
        expected_remote_env = {
            "CODEX_TEST_REMOTE_ENV": "${{ steps.remote_env.outputs.container_name }}",
            "CODEX_TEST_REMOTE_EXEC_SERVER_URL": (
                "${{ steps.remote_env.outputs.exec_server_url }}"
            ),
            "CODEX_TEST_REMOTE_CODEX_PATH": (
                "${{ steps.remote_env.outputs.codex_path }}"
            ),
            "REMOTE_TEST_FILTER": "${{ inputs.remote_test_filter }}",
        }
        self.assertEqual(
            expected_remote_env,
            {key: remote_test_step["env"][key] for key in expected_remote_env},
        )
        self.assertIn("--filterset", remote_test_step["run"])
        self.assertIn('"${REMOTE_TEST_FILTER}"', remote_test_step["run"])
        self.assertIn("--no-tests pass", remote_test_step["run"])
        self.assertNotIn("--skip", remote_test_step["run"])
        self.assertEqual(1, len(junit_steps))
        self.assertEqual("always()", junit_steps[0]["if"])
        self.assertLess(
            shard_steps.index(remote_test_step),
            shard_steps.index(junit_steps[0]),
        )
        self.assertIn("!cancelled()", setup_step["if"])
        self.assertEqual(
            "${{ !cancelled() && (steps.test.outcome == 'failure' || "
            "steps.remote_test.outcome == 'failure') }}",
            verify_step["if"],
        )

    def test_user_namespace_setup_is_shard_only(self) -> None:
        workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
        user_namespace_step = "Enable unprivileged user namespaces (Linux)"
        shard_steps = workflow["jobs"]["shard"]["steps"]
        shard_user_namespace_step = next(
            step for step in shard_steps if step.get("name") == user_namespace_step
        )

        self.assertNotIn(
            user_namespace_step,
            [step.get("name") for step in workflow["jobs"]["archive"]["steps"]],
        )
        self.assertIn(
            user_namespace_step,
            [step.get("name") for step in shard_steps],
        )
        self.assertIn(
            "/proc/sys/kernel/unprivileged_userns_clone",
            shard_user_namespace_step["run"],
        )
        self.assertIn(
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
            shard_user_namespace_step["run"],
        )


if __name__ == "__main__":
    unittest.main()
