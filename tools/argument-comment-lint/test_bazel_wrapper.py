#!/usr/bin/env python3

from __future__ import annotations

import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]
WRAPPER = REPO_ROOT / ".github" / "scripts" / "run-argument-comment-lint-bazel.sh"
WORKSPACE_TARGET = "//codex-rs/..."
MANUAL_UNIT_TEST_TARGET = "//codex-rs/app-server:app-server-unit-tests-bin"


class BazelWrapperTest(unittest.TestCase):
    def run_linux_wrapper(
        self, discovered_targets: tuple[str, ...]
    ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            wrapper_path = (
                temp_path / ".github" / "scripts" / "run-argument-comment-lint-bazel.sh"
            )
            wrapper_path.parent.mkdir(parents=True)
            shutil.copy2(WRAPPER, wrapper_path)

            target_list = (
                temp_path / "tools" / "argument-comment-lint" / "list-bazel-targets.sh"
            )
            target_list.parent.mkdir(parents=True)
            target_list.write_text(
                "#!/usr/bin/env bash\n"
                "set -euo pipefail\n"
                + "".join(f"printf '%s\\n' '{target}'\n" for target in discovered_targets),
                encoding="utf-8",
            )
            self.make_executable(target_list)

            captured_args = temp_path / "bazel-args.txt"
            bazel_wrapper = temp_path / ".github" / "scripts" / "run-bazel-ci.sh"
            bazel_wrapper.write_text(
                "#!/usr/bin/env bash\n"
                "set -euo pipefail\n"
                'printf "%s\\n" "$@" > "$CAPTURED_BAZEL_ARGS"\n',
                encoding="utf-8",
            )
            self.make_executable(bazel_wrapper)

            env = os.environ.copy()
            env["CAPTURED_BAZEL_ARGS"] = str(captured_args)
            env["RUNNER_OS"] = "Linux"
            result = subprocess.run(
                [
                    "bash",
                    str(wrapper_path),
                    "--config=argument-comment-lint",
                    "--keep_going",
                ],
                cwd=temp_path,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            args = (
                captured_args.read_text(encoding="utf-8").splitlines()
                if captured_args.exists()
                else []
            )
            return result, args

    def make_executable(self, path: Path) -> None:
        path.chmod(
            path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
        )

    def test_linux_build_includes_manual_unit_test_targets(self) -> None:
        result, args = self.run_linux_wrapper(
            (WORKSPACE_TARGET, MANUAL_UNIT_TEST_TARGET)
        )

        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        target_separator = len(args) - 1 - args[::-1].index("--")
        self.assertEqual(
            [WORKSPACE_TARGET, MANUAL_UNIT_TEST_TARGET], args[target_separator + 1 :]
        )

    def test_linux_rejects_wildcard_only_discovery(self) -> None:
        result, args = self.run_linux_wrapper((WORKSPACE_TARGET,))

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual([], args)
        self.assertIn("manual Rust unit-test targets", result.stderr)


if __name__ == "__main__":
    unittest.main()
