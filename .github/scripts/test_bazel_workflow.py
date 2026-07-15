import os
import subprocess
import tempfile
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
    def test_linux_bazel_test_setup_enables_unprivileged_user_namespaces(self) -> None:
        workflow = yaml.safe_load(BAZEL_WORKFLOW.read_text(encoding="utf-8"))
        namespace_steps = [
            (job_name, step)
            for job_name, job in workflow["jobs"].items()
            for step in job.get("steps", [])
            if "kernel.unprivileged_userns_clone" in str(step.get("run", ""))
            or "kernel.apparmor_restrict_unprivileged_userns"
            in str(step.get("run", ""))
        ]

        self.assertEqual(["test"], [job_name for job_name, _ in namespace_steps])
        step = namespace_steps[0][1]
        self.assertEqual("Enable unprivileged user namespaces (Linux)", step.get("name"))

        self.assertEqual("runner.os == 'Linux'", step.get("if"))
        self.assertEqual("bash", step.get("shell"))
        run = step.get("run", "")
        self.assertIn("sudo sysctl -w kernel.unprivileged_userns_clone=1", run)
        self.assertIn(
            "[[ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]]",
            run,
        )
        self.assertNotIn("sysctl -a", run)
        self.assertIn(
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0",
            run,
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            apparmor_setting = temp_path / "apparmor_restrict_unprivileged_userns"
            apparmor_setting.touch()
            sudo_log = temp_path / "sudo.log"
            mock_sudo = temp_path / "sudo"
            mock_sudo.write_text(
                '#!/usr/bin/env bash\nprintf \'%s\\n\' "$*" >> "$MOCK_SUDO_LOG"\n',
                encoding="utf-8",
            )
            mock_sudo.chmod(0o755)
            runnable = run.replace(
                "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
                str(apparmor_setting),
            )
            env = os.environ.copy()
            env["PATH"] = f"{temp_path}:{env['PATH']}"
            env["MOCK_SUDO_LOG"] = str(sudo_log)
            subprocess.run(
                ["bash", "-eo", "pipefail", "-c", runnable],
                check=True,
                env=env,
            )

            self.assertEqual(
                [
                    "sysctl -w kernel.unprivileged_userns_clone=1",
                    "sysctl -w kernel.apparmor_restrict_unprivileged_userns=0",
                ],
                sudo_log.read_text(encoding="utf-8").splitlines(),
            )

        disk_cleanup_steps = [
            step
            for step in workflow["jobs"]["test"]["steps"]
            if step.get("name") == "Free Linux runner disk for Bazel tests"
        ]
        self.assertEqual(1, len(disk_cleanup_steps))
        self.assertEqual("runner.os == 'Linux'", disk_cleanup_steps[0].get("if"))

    def test_host_tool_aquery_runs_once_on_the_primary_linux_leg(self) -> None:
        workflow = yaml.safe_load(BAZEL_WORKFLOW.read_text(encoding="utf-8"))
        steps = workflow["jobs"]["test"]["steps"]
        matching_steps = [
            step
            for step in steps
            if any(
                probe in str(step.get("run", ""))
                for probe in (
                    "check_windows_bazel_host_tools.py",
                    "check-windows-bazel-host-tools",
                )
            )
        ]

        self.assertEqual(1, len(matching_steps))
        self.assertEqual(
            "python3 .github/scripts/check_windows_bazel_host_tools.py",
            matching_steps[0].get("run"),
        )
        self.assertNotIn("just", str(matching_steps[0].get("run", "")))
        self.assertEqual(
            "matrix.os == 'ubuntu-24.04' && matrix.target == 'x86_64-unknown-linux-gnu'",
            matching_steps[0].get("if"),
        )

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
