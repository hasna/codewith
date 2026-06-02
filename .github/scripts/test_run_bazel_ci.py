import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


class RunBazelCiTest(unittest.TestCase):
    def test_local_fallback_clears_remote_services(self) -> None:
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
            env.pop("BUILDBUDDY_API_KEY", None)
            env["FAKE_BAZEL_ARGS"] = str(args_path)
            env["PATH"] = f"{tmp_path}{os.pathsep}{env['PATH']}"
            env["RUNNER_OS"] = "Linux"

            result = subprocess.run(
                [
                    "bash",
                    ".github/scripts/run-bazel-ci.sh",
                    "--",
                    "build",
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

            self.assertEqual(
                result.returncode,
                0,
                f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
            )
            self.assertIn("using local Bazel configuration", result.stdout)

            bazel_args = args_path.read_text(encoding="utf-8").splitlines()
            self.assertIn("--remote_cache=", bazel_args)
            self.assertIn("--remote_executor=", bazel_args)
            self.assertIn("--experimental_remote_downloader=", bazel_args)


if __name__ == "__main__":
    unittest.main()
