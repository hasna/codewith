import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("check_windows_bazel_host_tools.py")
SPEC = importlib.util.spec_from_file_location("check_windows_bazel_host_tools", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def action(platform: str, compiler: str) -> dict[str, object]:
    return {"executionPlatform": platform, "arguments": [compiler, "-c", "input.cc"]}


class CheckWindowsBazelHostToolsTest(unittest.TestCase):
    def test_windows_requires_every_action_to_use_windows_amd64_clang(self) -> None:
        good = action(
            "//:windows_x86_64_msvc",
            "external/llvm_toolchain_llvm/bin/windows-amd64/bin/clang.exe",
        )
        mixed = action(
            "//:windows_x86_64_msvc",
            "external/llvm_toolchain_llvm/bin/linux-amd64/bin/clang",
        )

        MODULE.verify_actions({"actions": [good]})
        with self.assertRaisesRegex(RuntimeError, "every action"):
            MODULE.verify_actions({"actions": [good, mixed]})
        with self.assertRaisesRegex(RuntimeError, "compiler arguments"):
            MODULE.verify_actions(
                {"actions": [{"executionPlatform": "//:windows_x86_64_msvc"}]}
            )
        with self.assertRaisesRegex(RuntimeError, r"no C\+\+ compile actions"):
            MODULE.verify_actions({"actions": []})

    def test_linux_requires_every_action_to_use_linux_amd64_clang(self) -> None:
        good = action(
            MODULE.LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/linux-amd64/bin/clang",
        )
        mixed = action(
            MODULE.LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/windows-amd64/bin/clang.exe",
        )

        MODULE.verify_linux_control({"actions": [good]})
        with self.assertRaisesRegex(RuntimeError, "every action"):
            MODULE.verify_linux_control({"actions": [good, mixed]})
        with self.assertRaisesRegex(RuntimeError, "compiler arguments"):
            MODULE.verify_linux_control(
                {"actions": [{"executionPlatform": MODULE.LINUX_EXEC_PLATFORM}]}
            )
        with self.assertRaisesRegex(RuntimeError, r"no C\+\+ compile actions"):
            MODULE.verify_linux_control({"actions": []})

    def test_native_linux_control_matches_the_host_architecture(self) -> None:
        arm_action = action(
            MODULE.NATIVE_LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/linux-arm64/bin/clang",
        )
        amd_action = action(
            MODULE.NATIVE_LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/linux-amd64/bin/clang",
        )

        with patch.object(MODULE.platform, "machine", return_value="aarch64"):
            MODULE.verify_native_linux_control({"actions": [arm_action]})
            with self.assertRaisesRegex(RuntimeError, "every action"):
                MODULE.verify_native_linux_control(
                    {"actions": [arm_action, amd_action]}
                )


if __name__ == "__main__":
    unittest.main()
