import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


class RunBazelCiTest(unittest.TestCase):
    def run_with_fake_bazel(
        self,
        *,
        env_updates: dict[str, str | None],
        bazel_args: list[str] | None = None,
        bazel_targets: list[str] | None = None,
        wrapper_args: list[str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
        bazel_args = bazel_args or ["build"]
        bazel_targets = bazel_targets or ["//fake:target"]

        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = Path(tmp_dir)
            args_path = tmp_path / "bazel-args.txt"
            fake_bazel = tmp_path / "bazel"
            fake_bazel.write_text(
                "#!/usr/bin/env bash\n"
                "set -euo pipefail\n"
                'printf "%s\\n" "$@" > "$FAKE_BAZEL_ARGS"\n',
                encoding="utf-8",
            )
            fake_bazel.chmod(
                fake_bazel.stat().st_mode
                | stat.S_IXUSR
                | stat.S_IXGRP
                | stat.S_IXOTH
            )

            env = os.environ.copy()
            for key, value in env_updates.items():
                if value is None:
                    env.pop(key, None)
                else:
                    env[key] = value
            env["FAKE_BAZEL_ARGS"] = str(args_path)
            env["PATH"] = f"{tmp_path}{os.pathsep}{env['PATH']}"

            result = subprocess.run(
                [
                    "bash",
                    ".github/scripts/run-bazel-ci.sh",
                    *(wrapper_args or []),
                    "--",
                    *bazel_args,
                    "--",
                    *bazel_targets,
                ],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

            bazel_args = []
            if args_path.exists():
                bazel_args = args_path.read_text(encoding="utf-8").splitlines()
            return result, bazel_args

    def test_local_fallback_clears_remote_services(self) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BUILDBUDDY_API_KEY": None,
                "RUNNER_OS": "Linux",
            }
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        self.assertIn("using local Bazel configuration", result.stdout)

        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)

    def test_keyed_generic_linux_uses_buildbuddy_cache_configuration(self) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BUILDBUDDY_API_KEY": "fake-token",
                "GITHUB_ACTIONS": "true",
                "GITHUB_REPOSITORY": "hasna/codewith",
                "RUNNER_OS": "Linux",
            }
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        self.assertIn(
            "using buildbuddy-generic Bazel configuration",
            result.stdout,
        )

        self.assertIn("--config=buildbuddy-generic", bazel_args)
        self.assertIn("--remote_header=x-buildbuddy-api-key=fake-token", bazel_args)
        self.assertIn("--config=ci-bazel", bazel_args)
        self.assertIn("--build_metadata=TAG_os=linux", bazel_args)
        self.assertNotIn("--config=ci-linux", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)
        self.assertNotIn("--platforms=//:rbe", bazel_args)

        remote_idx = bazel_args.index("--config=buildbuddy-generic")
        ci_idx = bazel_args.index("--config=ci-bazel")
        os_metadata_idx = bazel_args.index("--build_metadata=TAG_os=linux")
        self.assertLess(remote_idx, ci_idx)
        self.assertLess(ci_idx, os_metadata_idx)

    def test_keyed_generic_windows_cross_uses_keyless_local_msvc_fallback(
        self,
    ) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BAZEL_REPO_CONTENTS_CACHE": None,
                "BAZEL_REPOSITORY_CACHE": None,
                "BUILDBUDDY_API_KEY": "fake-token",
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\tools\bin",
                "GITHUB_ACTIONS": "true",
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_REPOSITORY": "hasna/codewith",
                "RUNNER_OS": "Windows",
            },
            wrapper_args=["--windows-cross-compile"],
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        self.assertIn("using local Bazel configuration", result.stdout)

        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)
        self.assertIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertIn("--jobs=8", bazel_args)
        self.assertIn(r"--action_env=PATH=C:\tools\bin", bazel_args)
        self.assertIn(r"--host_action_env=PATH=C:\tools\bin", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic", bazel_args)
        self.assertNotIn("--remote_header=x-buildbuddy-api-key=fake-token", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)
        self.assertNotIn("--config=ci-windows-cross", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)
        self.assertNotIn("--shell_executable=/bin/bash", bazel_args)
        self.assertNotIn("--action_env=PATH=/usr/bin:/bin", bazel_args)
        self.assertNotIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)

    def test_keyed_generic_windows_cross_clippy_uses_keyless_local_skip_fallback(
        self,
    ) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BAZEL_REPO_CONTENTS_CACHE": None,
                "BAZEL_REPOSITORY_CACHE": None,
                "BUILDBUDDY_API_KEY": "fake-token",
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\tools\bin",
                "GITHUB_ACTIONS": "true",
                "GITHUB_REPOSITORY": "hasna/codewith",
                "RUNNER_OS": "Windows",
            },
            bazel_args=["build", "--config=clippy"],
            wrapper_args=["--windows-cross-compile"],
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )

        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)
        self.assertIn("--config=clippy", bazel_args)
        self.assertIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertIn("--jobs=8", bazel_args)
        self.assertIn("--skip_incompatible_explicit_targets", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic", bazel_args)
        self.assertNotIn("--remote_header=x-buildbuddy-api-key=fake-token", bazel_args)
        self.assertNotIn("--config=ci-windows-cross", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)

    def test_trusted_openai_linux_keeps_rbe_configuration(self) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BUILDBUDDY_API_KEY": "fake-token",
                "GITHUB_ACTIONS": "true",
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_REPOSITORY": "openai/codex",
                "RUNNER_OS": "Linux",
            }
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        self.assertIn(
            "using buildbuddy-openai-rbe Bazel configuration",
            result.stdout,
        )

        self.assertIn("--config=buildbuddy-openai-rbe", bazel_args)
        self.assertIn("--remote_header=x-buildbuddy-api-key=fake-token", bazel_args)
        self.assertIn("--config=ci-linux", bazel_args)

        remote_idx = bazel_args.index("--config=buildbuddy-openai-rbe")
        ci_idx = bazel_args.index("--config=ci-linux")
        self.assertLess(remote_idx, ci_idx)

    def test_trusted_openai_windows_cross_keeps_rbe_configuration(self) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BAZEL_REPO_CONTENTS_CACHE": None,
                "BAZEL_REPOSITORY_CACHE": None,
                "BUILDBUDDY_API_KEY": "fake-token",
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\tools\bin",
                "GITHUB_ACTIONS": "true",
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_REPOSITORY": "openai/codex",
                "RUNNER_OS": "Windows",
            },
            wrapper_args=["--windows-cross-compile"],
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        self.assertIn(
            "using buildbuddy-openai-rbe Bazel configuration",
            result.stdout,
        )

        self.assertIn("--config=buildbuddy-openai-rbe", bazel_args)
        self.assertIn("--remote_header=x-buildbuddy-api-key=fake-token", bazel_args)
        self.assertIn("--config=ci-windows-cross", bazel_args)
        self.assertIn("--host_platform=//:rbe", bazel_args)
        self.assertIn("--shell_executable=/bin/bash", bazel_args)
        self.assertIn("--action_env=PATH=/usr/bin:/bin", bazel_args)
        self.assertIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)
        self.assertNotIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertNotIn("--jobs=8", bazel_args)

        remote_idx = bazel_args.index("--config=buildbuddy-openai-rbe")
        ci_idx = bazel_args.index("--config=ci-windows-cross")
        host_idx = bazel_args.index("--host_platform=//:rbe")
        self.assertLess(remote_idx, ci_idx)
        self.assertLess(ci_idx, host_idx)

    def test_keyless_windows_cross_uses_local_msvc_fallback(self) -> None:
        result, bazel_args = self.run_with_fake_bazel(
            env_updates={
                "BAZEL_REPO_CONTENTS_CACHE": None,
                "BAZEL_REPOSITORY_CACHE": None,
                "BUILDBUDDY_API_KEY": None,
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\tools\bin",
                "GITHUB_ACTIONS": "true",
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_REPOSITORY": "hasna/codewith",
                "RUNNER_OS": "Windows",
            },
            wrapper_args=["--windows-cross-compile"],
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        self.assertIn("using local Bazel configuration", result.stdout)

        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)
        self.assertIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertIn("--jobs=8", bazel_args)
        self.assertNotIn("--config=ci-windows-cross", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)
        self.assertNotIn("--shell_executable=/bin/bash", bazel_args)


if __name__ == "__main__":
    unittest.main()
