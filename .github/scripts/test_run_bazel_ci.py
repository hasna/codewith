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
        runner_os: str,
        wrapper_args: tuple[str, ...] = (),
        bazel_args: tuple[str, ...] = ("build",),
        buildbuddy_api_key: str | None = None,
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
            if extra_env is not None:
                env.update(extra_env)
            env["FAKE_BAZEL_ARGS"] = str(args_path)
            env["PATH"] = f"{tmp_path}{os.pathsep}{env['PATH']}"
            env["RUNNER_OS"] = runner_os

            result = subprocess.run(
                [
                    "bash",
                    ".github/scripts/run-bazel-ci.sh",
                    *wrapper_args,
                    "--",
                    *bazel_args,
                    "--",
                    "//fake:target",
                ],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

            bazel_args = (
                args_path.read_text(encoding="utf-8").splitlines()
                if args_path.exists()
                else []
            )
            return result, bazel_args

    def assert_success(self, result: subprocess.CompletedProcess[str]) -> None:
        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )

    def test_keyed_windows_cross_uses_windows_host_tools(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("test", "--platforms=//:windows_x86_64_gnullvm"),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        self.assertIn("using keyed Bazel configuration", result.stdout)
        self.assertNotIn("using remote Bazel configuration", result.stdout)
        self.assertIn("--config=ci-windows-cross", bazel_args)
        config_index = bazel_args.index("--config=ci-windows-cross")
        post_config_args = bazel_args[config_index + 1 :]
        self.assertIn("--config=buildbuddy-generic", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic-rbe", bazel_args)
        self.assertNotIn("--config=buildbuddy-openai-rbe", bazel_args)
        self.assertTrue(
            any(
                arg.startswith("--remote_header=x-buildbuddy-api-key=")
                for arg in bazel_args
            )
        )
        self.assertIn("--platforms=//:windows_x86_64_gnullvm", bazel_args)
        self.assertIn(
            "--host_platform=//:local_windows_msvc",
            post_config_args,
        )
        self.assertIn(
            "--extra_execution_platforms=//:windows_x86_64_msvc",
            post_config_args,
        )
        self.assertIn(
            r"--host_action_env=PATH=C:\bazel;C:\Windows\System32",
            bazel_args,
        )
        self.assertNotIn("--host_platform=//:rbe", bazel_args)
        self.assertNotIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)
        self.assertNotIn("--shell_executable=/bin/bash", bazel_args)

    def test_keyless_windows_cross_keeps_bounded_local_host(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        self.assertIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertIn(
            "--extra_execution_platforms=//:windows_x86_64_msvc",
            bazel_args,
        )
        self.assertIn("--jobs=8", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)
        self.assertNotIn("--config=buildbuddy-generic", bazel_args)
        self.assertNotIn("--config=buildbuddy-openai", bazel_args)

    def test_trusted_upstream_windows_cross_uses_openai_cache_only(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
                "GITHUB_ACTIONS": "true",
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_REPOSITORY": "openai/codex",
            },
        )

        self.assert_success(result)
        self.assertIn("--config=buildbuddy-openai", bazel_args)
        self.assertNotIn("--config=buildbuddy-openai-rbe", bazel_args)
        self.assertNotIn("--remote_executor=grpcs://openai.buildbuddy.io", bazel_args)

    def test_explicit_windows_rbe_host_requires_remote_executor(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("build", "--host_platform=//:rbe"),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(bazel_args, [])
        self.assertIn("requires an endpoint-bearing remote execution", result.stderr)

    def test_explicit_windows_rbe_host_with_endpoint_keeps_linux_tools(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                "build",
                "--config=buildbuddy-generic-rbe",
                "--host_platform=@//:rbe",
            ),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        config_index = bazel_args.index("--config=ci-windows-cross")
        post_config_args = bazel_args[config_index + 1 :]
        self.assertIn(
            "--host_platform=@//:rbe",
            post_config_args,
        )
        self.assertIn(
            "--extra_execution_platforms=//:rbe,//:windows_x86_64_msvc",
            post_config_args,
        )
        self.assertIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)
        self.assertIn("--shell_executable=/bin/bash", bazel_args)
        self.assertNotIn("--host_platform=//:local_windows_msvc", bazel_args)

    def test_keyless_explicit_rbe_does_not_clear_direct_executor(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                "build",
                "--host_platform=//:rbe",
                "--remote_executor=grpc://remote.example.test",
            ),
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        self.assertIn("--config=ci-windows-cross", bazel_args)
        self.assertIn("--platforms=//:windows_x86_64_gnullvm", bazel_args)
        self.assertIn("--remote_executor=grpc://remote.example.test", bazel_args)
        self.assertNotIn("--remote_executor=", bazel_args)

    def test_last_windows_host_platform_override_wins(self) -> None:
        for host_args, expected_host, expects_rbe in (
            (
                (
                    "--host_platform=//:rbe",
                    "--host_platform=//:local_windows_msvc",
                ),
                "--host_platform=//:local_windows_msvc",
                False,
            ),
            (
                (
                    "--host_platform=//:local_windows_msvc",
                    "--host_platform=@@//:rbe",
                    "--remote_executor=grpcs://remote.example.test",
                ),
                "--host_platform=@@//:rbe",
                True,
            ),
        ):
            with self.subTest(host_args=host_args):
                result, bazel_args = self.run_wrapper(
                    runner_os="Windows",
                    wrapper_args=("--windows-cross-compile",),
                    bazel_args=("build", *host_args),
                    buildbuddy_api_key="x",
                    extra_env={
                        "CODEX_BAZEL_WINDOWS_PATH": (
                            r"C:\bazel;C:\Windows\System32"
                        ),
                    },
                )

                self.assert_success(result)
                config_index = bazel_args.index("--config=ci-windows-cross")
                post_config_args = bazel_args[config_index + 1 :]
                self.assertEqual(
                    [
                        arg
                        for arg in post_config_args
                        if arg.startswith("--host_platform=")
                    ],
                    [expected_host],
                )
                expected_exec_platforms = (
                    "--extra_execution_platforms=//:rbe,//:windows_x86_64_msvc"
                    if expects_rbe
                    else "--extra_execution_platforms=//:windows_x86_64_msvc"
                )
                self.assertIn(expected_exec_platforms, post_config_args)

    def test_spaced_host_override_after_rbe_returns_to_local_windows(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                "build",
                "--host_platform=//:rbe",
                "--remote_executor=grpcs://remote.example.test",
                "--host_platform",
                "//:local_windows_msvc",
            ),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        self.assertIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertIn(
            "--extra_execution_platforms=//:windows_x86_64_msvc", bazel_args
        )
        self.assertNotIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)

    def test_spaced_empty_executor_after_rbe_fails_closed(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                "build",
                "--host_platform=//:rbe",
                "--remote_executor=grpcs://remote.example.test",
                "--remote_executor",
                "",
            ),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(bazel_args, [])
        self.assertIn("requires an endpoint-bearing remote execution", result.stderr)

    def test_spaced_direct_rbe_requires_and_accepts_spaced_endpoint(self) -> None:
        common_args = (
            "build",
            "--host_platform",
            "@codex//:rbe",
        )
        env = {"CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32"}

        rejected, rejected_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=common_args,
            buildbuddy_api_key="x",
            extra_env=env,
        )
        accepted, accepted_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                *common_args,
                "--remote_executor",
                "grpcs://remote.example.test",
            ),
            buildbuddy_api_key="x",
            extra_env=env,
        )

        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(rejected_args, [])
        self.assert_success(accepted)
        self.assertIn("--host_platform=@codex//:rbe", accepted_args)
        self.assertIn("--host_action_env=PATH=/usr/bin:/bin", accepted_args)

    def test_spaced_rbe_config_supplies_endpoint(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                "build",
                "--config",
                "buildbuddy-generic-rbe",
                "--host_platform",
                "@@//:rbe",
            ),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        self.assertIn("--host_platform=@@//:rbe", bazel_args)
        self.assertIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)

    def test_external_repository_rbe_label_is_not_main_repository_rbe(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("build", "--host_platform=@external//:rbe"),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assert_success(result)
        self.assertIn("--host_platform=@external//:rbe", bazel_args)
        self.assertNotIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)
        self.assertNotIn("--shell_executable=/bin/bash", bazel_args)

    def test_rbe_host_rejects_final_windows_only_execution_platform(self) -> None:
        for topology_args in (
            (
                "--host_platform=//:rbe",
                "--remote_executor=grpcs://remote.example.test",
                "--extra_execution_platforms=//:rbe",
                "--extra_execution_platforms",
                "//:windows_x86_64_msvc",
            ),
            (
                "--extra_execution_platforms=//:windows_x86_64_msvc",
                "--host_platform=//:rbe",
                "--remote_executor=grpcs://remote.example.test",
            ),
        ):
            with self.subTest(topology_args=topology_args):
                result, bazel_args = self.run_wrapper(
                    runner_os="Windows",
                    wrapper_args=("--windows-cross-compile",),
                    bazel_args=("build", *topology_args),
                    buildbuddy_api_key="x",
                    extra_env={
                        "CODEX_BAZEL_WINDOWS_PATH": (
                            r"C:\bazel;C:\Windows\System32"
                        ),
                    },
                )

                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(bazel_args, [])
                self.assertIn("RBE-compatible execution platform", result.stderr)

    def test_rbe_host_accepts_final_rbe_execution_platform_in_both_orders(self) -> None:
        for execution_args in (
            (
                "--extra_execution_platforms=//:windows_x86_64_msvc",
                "--extra_execution_platforms=//:rbe,//:windows_x86_64_msvc",
            ),
            (
                "--extra_execution_platforms=//:windows_x86_64_msvc",
                "--extra_execution_platforms",
                "@codex//:rbe,//:windows_x86_64_msvc",
            ),
        ):
            with self.subTest(execution_args=execution_args):
                result, bazel_args = self.run_wrapper(
                    runner_os="Windows",
                    wrapper_args=("--windows-cross-compile",),
                    bazel_args=(
                        "build",
                        *execution_args,
                        "--host_platform=//:rbe",
                        "--remote_executor=grpcs://remote.example.test",
                    ),
                    buildbuddy_api_key="x",
                    extra_env={
                        "CODEX_BAZEL_WINDOWS_PATH": (
                            r"C:\bazel;C:\Windows\System32"
                        ),
                    },
                )

                self.assert_success(result)
                self.assertIn("--shell_executable=/bin/bash", bazel_args)
                self.assertIn("--host_action_env=PATH=/usr/bin:/bin", bazel_args)

    def test_empty_attached_values_rejected_except_remote_executor(self) -> None:
        env = {"CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32"}
        for option in ("--host_platform=", "--config=", "--extra_execution_platforms="):
            with self.subTest(option=option):
                result, bazel_args = self.run_wrapper(
                    runner_os="Windows",
                    wrapper_args=("--windows-cross-compile",),
                    bazel_args=("build", option),
                    buildbuddy_api_key="x",
                    extra_env=env,
                )

                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(bazel_args, [])
                self.assertIn("requires a non-empty value", result.stderr)

        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("build", "--remote_executor="),
            buildbuddy_api_key="x",
            extra_env=env,
        )
        self.assert_success(result)
        self.assertIn("--remote_executor=", bazel_args)

    def test_empty_spaced_values_rejected_except_remote_executor(self) -> None:
        env = {"CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32"}
        for option in ("--host_platform", "--config", "--extra_execution_platforms"):
            with self.subTest(option=option):
                result, bazel_args = self.run_wrapper(
                    runner_os="Windows",
                    wrapper_args=("--windows-cross-compile",),
                    bazel_args=("build", option, ""),
                    buildbuddy_api_key="x",
                    extra_env=env,
                )

                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(bazel_args, [])
                self.assertIn("requires a non-empty value", result.stderr)

        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=("build", "--remote_executor", ""),
            buildbuddy_api_key="x",
            extra_env=env,
        )
        self.assert_success(result)
        executor_index = bazel_args.index("--remote_executor")
        self.assertEqual("", bazel_args[executor_index + 1])

    def test_missing_spaced_option_value_is_not_misread_as_an_option(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Windows",
            wrapper_args=("--windows-cross-compile",),
            bazel_args=(
                "build",
                "--host_platform",
                "--remote_executor=grpcs://remote.example.test",
            ),
            buildbuddy_api_key="x",
            extra_env={
                "CODEX_BAZEL_WINDOWS_PATH": r"C:\bazel;C:\Windows\System32",
            },
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(bazel_args, [])
        self.assertIn("requires a value", result.stderr)

    def test_keyed_linux_configuration_is_unchanged(self) -> None:
        result, bazel_args = self.run_wrapper(
            runner_os="Linux",
            buildbuddy_api_key="x",
        )

        self.assert_success(result)
        self.assertIn("--config=ci-linux", bazel_args)
        self.assertNotIn("--host_platform=//:local_windows_msvc", bazel_args)
        self.assertNotIn("--host_platform=//:rbe", bazel_args)

    def test_local_fallback_clears_remote_services(self) -> None:
        result, bazel_args = self.run_wrapper(runner_os="Linux")

        self.assert_success(result)
        self.assertIn("using local Bazel configuration", result.stdout)
        self.assertIn("--remote_cache=", bazel_args)
        self.assertIn("--remote_executor=", bazel_args)
        self.assertIn("--experimental_remote_downloader=", bazel_args)


if __name__ == "__main__":
    unittest.main()
