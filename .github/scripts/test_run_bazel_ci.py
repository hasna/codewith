import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


class RunBazelCiTest(unittest.TestCase):
    def run_wrapper(
        self,
        *,
        buildbuddy_api_key: str | None,
        runner_os: str = "Linux",
        wrapper_args: tuple[str, ...] = (),
        bazel_args: tuple[str, ...] = ("build",),
        bazel_targets: tuple[str, ...] = ("//fake:target",),
        extra_env: dict[str, str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
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
            if buildbuddy_api_key is None:
                env.pop("BUILDBUDDY_API_KEY", None)
            else:
                env["BUILDBUDDY_API_KEY"] = buildbuddy_api_key
            env.pop("GITHUB_ACTIONS", None)
            env.pop("GITHUB_REPOSITORY", None)
            env.pop("CODEWITH_BAZEL_ENABLE_BUILDBUDDY_RBE", None)
            env["FAKE_BAZEL_ARGS"] = str(args_path)
            env["PATH"] = f"{tmp_path}{os.pathsep}{env['PATH']}"
            env["RUNNER_OS"] = runner_os
            if extra_env is not None:
                env.update(extra_env)

            result = subprocess.run(
                [
                    "bash",
                    ".github/scripts/run-bazel-ci.sh",
                    *wrapper_args,
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

            bazel_invocation = []
            if args_path.exists():
                bazel_invocation = args_path.read_text(encoding="utf-8").splitlines()

            return result, bazel_invocation

    def assert_success(self, result: subprocess.CompletedProcess[str]) -> None:
        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )

    def assert_remote_config_before_ci_config(
        self, bazel_args: list[str], *, remote_config: str, ci_config: str
    ) -> None:
        self.assertIn(remote_config, bazel_args)
        self.assertIn(ci_config, bazel_args)
        self.assertLess(bazel_args.index(remote_config), bazel_args.index(ci_config))

    def test_local_fallback_clears_remote_services(self) -> None:
        result, bazel_args = self.run_wrapper(buildbuddy_api_key=None)

        self.assert_success(result)
        self.assertIn("using local Bazel configuration", result.stdout)
        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)

    def test_keyed_linux_uses_buildbuddy_rbe_config(self) -> None:
        result, bazel_args = self.run_wrapper(buildbuddy_api_key="test-token")

        self.assert_success(result)
        self.assertIn("using remote Bazel configuration", result.stdout)
        self.assert_remote_config_before_ci_config(
            bazel_args,
            remote_config="--config=buildbuddy-generic-rbe",
            ci_config="--config=ci-linux",
        )
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertNotIn("--remote_executor=", bazel_args)

    def test_keyed_upstream_actions_uses_openai_buildbuddy_rbe_config(self) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            extra_env={"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": "openai/codex"},
        )

        self.assert_success(result)
        self.assertIn("using remote Bazel configuration", result.stdout)
        self.assert_remote_config_before_ci_config(
            bazel_args,
            remote_config="--config=buildbuddy-openai-rbe",
            ci_config="--config=ci-linux",
        )
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)
        self.assertNotIn("--remote_executor=", bazel_args)

    def test_keyed_hasna_ci_linux_uses_buildbuddy_cache_only_by_default(self) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            extra_env={"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": "hasna/codewith"},
        )

        self.assert_success(result)
        self.assertIn(
            "using BuildBuddy cache with local Bazel execution", result.stdout
        )
        self.assertIn("--config=buildbuddy-openai", bazel_args)
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--config=ci-keyless", bazel_args)
        self.assertNotIn("--config=buildbuddy-openai-rbe", bazel_args)
        self.assertNotIn("--remote_cache=", bazel_args)
        self.assertNotIn("--experimental_remote_downloader=", bazel_args)
        self.assertLess(
            bazel_args.index("--config=buildbuddy-openai"),
            bazel_args.index("--config=ci-keyless"),
        )

    def test_keyed_hasna_ci_linux_can_opt_into_openai_buildbuddy_rbe_config(
        self,
    ) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            extra_env={
                "CODEWITH_BAZEL_ENABLE_BUILDBUDDY_RBE": "1",
                "GITHUB_ACTIONS": "true",
                "GITHUB_REPOSITORY": "hasna/codewith",
            },
        )

        self.assert_success(result)
        self.assertIn("using remote Bazel configuration", result.stdout)
        self.assert_remote_config_before_ci_config(
            bazel_args,
            remote_config="--config=buildbuddy-openai-rbe",
            ci_config="--config=ci-linux",
        )
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)

    def test_keyed_windows_cross_uses_buildbuddy_rbe_config(self) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("test",),
            extra_env={"CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32"},
        )

        self.assert_success(result)
        self.assertIn("using remote Bazel configuration", result.stdout)
        self.assert_remote_config_before_ci_config(
            bazel_args,
            remote_config="--config=buildbuddy-generic-rbe",
            ci_config="--config=ci-windows-cross",
        )
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertIn("--host_platform=//:rbe", bazel_args)
        self.assertIn("--shell_executable=/bin/bash", bazel_args)
        self.assertNotIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertNotIn("--remote_executor=", bazel_args)

    def test_keyed_hasna_ci_windows_cross_uses_keyless_fallback_by_default(
        self,
    ) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("test",),
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
                "GITHUB_ACTIONS": "true",
                "GITHUB_REPOSITORY": "hasna/codewith",
            },
        )

        self.assert_success(result)
        self.assertIn("using local Bazel configuration", result.stdout)
        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)
        self.assertIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertIn("--jobs=8", bazel_args)
        self.assertNotIn("--config=buildbuddy-openai-rbe", bazel_args)
        self.assertNotIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)
        self.assertNotIn("--shell_executable=/bin/bash", bazel_args)

    def test_keyed_hasna_ci_windows_cross_can_opt_into_openai_buildbuddy_rbe_config(
        self,
    ) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("test",),
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
                "CODEWITH_BAZEL_ENABLE_BUILDBUDDY_RBE": "1",
                "GITHUB_ACTIONS": "true",
                "GITHUB_REPOSITORY": "hasna/codewith",
            },
        )

        self.assert_success(result)
        self.assertIn("using remote Bazel configuration", result.stdout)
        self.assert_remote_config_before_ci_config(
            bazel_args,
            remote_config="--config=buildbuddy-openai-rbe",
            ci_config="--config=ci-windows-cross",
        )
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertIn("--host_platform=//:rbe", bazel_args)
        self.assertIn("--shell_executable=/bin/bash", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)

    def test_keyed_native_windows_preserves_non_rbe_ci_config(self) -> None:
        result, bazel_args = self.run_wrapper(
            buildbuddy_api_key="test-token",
            runner_os="Windows",
            extra_env={"CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32"},
        )

        self.assert_success(result)
        self.assertIn("--config=ci-windows", bazel_args)
        self.assertIn("--remote_header=x-buildbuddy-api-key=test-token", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)


if __name__ == "__main__":
    unittest.main()
