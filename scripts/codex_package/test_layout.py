#!/usr/bin/env python3

from pathlib import Path
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from codex_package.layout import build_package_dir
from codex_package.layout import THIRD_PARTY_LICENSE_FILES
from codex_package.layout import validate_package_dir
from codex_package.targets import PACKAGE_VARIANTS
from codex_package.targets import PackageInputs
from codex_package.targets import TARGET_SPECS


class PackageLayoutTest(unittest.TestCase):
    def test_primary_package_uses_codewith_layout(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            package_dir = root / "package"
            input_dir = root / "input"
            input_dir.mkdir()

            inputs = PackageInputs(
                entrypoint_bin=touch_executable(input_dir / "codewith"),
                rg_bin=touch_executable(input_dir / "rg"),
                zsh_bin=None,
                bwrap_bin=touch_executable(input_dir / "bwrap"),
                codex_command_runner_bin=None,
                codex_windows_sandbox_setup_bin=None,
            )

            package_dir.mkdir()
            build_package_dir(
                package_dir,
                "0.1.0",
                PACKAGE_VARIANTS["codex"],
                TARGET_SPECS["x86_64-unknown-linux-musl"],
                inputs,
            )
            validate_package_dir(
                package_dir,
                PACKAGE_VARIANTS["codex"],
                TARGET_SPECS["x86_64-unknown-linux-musl"],
                include_zsh=False,
            )

            self.assertTrue((package_dir / "bin" / "codewith").is_file())
            self.assertTrue((package_dir / "codewith-path" / "rg").is_file())
            self.assertTrue((package_dir / "codewith-resources" / "bwrap").is_file())
            self.assertEqual(
                (package_dir / "LICENSE").read_text(), repo_file("LICENSE")
            )
            self.assertEqual((package_dir / "NOTICE").read_text(), repo_file("NOTICE"))
            self.assertEqual(
                (package_dir / "MODIFICATIONS.md").read_text(),
                repo_file("MODIFICATIONS.md"),
            )
            self.assertEqual(
                (package_dir / "THIRD_PARTY_NOTICES.md").read_text(),
                repo_file("THIRD_PARTY_NOTICES.md"),
            )
            for filename, source in THIRD_PARTY_LICENSE_FILES.items():
                self.assertEqual(
                    (package_dir / "licenses" / filename).read_text(),
                    source.read_text(encoding="utf-8"),
                )
            self.assertFalse((package_dir / "bin" / "codex").exists())
            self.assertFalse((package_dir / "codex-path").exists())
            self.assertFalse((package_dir / "codex-resources").exists())


def touch_executable(path: Path) -> Path:
    path.write_text("", encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path.resolve()


def repo_file(name: str) -> str:
    return (Path(__file__).resolve().parents[2] / name).read_text(encoding="utf-8")


if __name__ == "__main__":
    unittest.main()
