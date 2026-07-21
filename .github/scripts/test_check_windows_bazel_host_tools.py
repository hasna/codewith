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


def action(
    platform: str,
    tool: str,
    mnemonic: str = "CppCompile",
) -> dict[str, object]:
    return {
        "executionPlatform": platform,
        "mnemonic": mnemonic,
        "arguments": [tool, "-c", "input.cc"],
    }


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
        archive = action(
            "//:windows_x86_64_msvc",
            "external/llvm_toolchain_llvm/bin/windows-amd64/bin/llvm-ar.exe",
            "CppArchive",
        )
        wrong_archive = action(
            "//:windows_x86_64_msvc",
            "external/llvm_toolchain_llvm/bin/linux-amd64/bin/llvm-ar",
            "CppArchive",
        )

        MODULE.verify_actions({"actions": [good, archive]})
        with self.assertRaisesRegex(RuntimeError, "every action"):
            MODULE.verify_actions({"actions": [good, mixed, archive]})
        with self.assertRaisesRegex(RuntimeError, "llvm-ar"):
            MODULE.verify_actions({"actions": [good, wrong_archive]})
        with self.assertRaisesRegex(RuntimeError, "compiler arguments"):
            MODULE.verify_actions(
                {
                    "actions": [
                        {
                            "executionPlatform": "//:windows_x86_64_msvc",
                            "mnemonic": "CppCompile",
                        },
                        archive,
                    ]
                }
            )
        with self.assertRaisesRegex(RuntimeError, r"no CppArchive actions"):
            MODULE.verify_actions({"actions": [good]})
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
        archive = action(
            MODULE.LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/linux-amd64/bin/llvm-ar",
            "CppArchive",
        )
        wrong_archive = action(
            MODULE.LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/windows-amd64/bin/llvm-ar.exe",
            "CppArchive",
        )

        MODULE.verify_linux_control({"actions": [good, archive]})
        with self.assertRaisesRegex(RuntimeError, "every action"):
            MODULE.verify_linux_control({"actions": [good, mixed, archive]})
        with self.assertRaisesRegex(RuntimeError, "llvm-ar"):
            MODULE.verify_linux_control({"actions": [good, wrong_archive]})
        with self.assertRaisesRegex(RuntimeError, "compiler arguments"):
            MODULE.verify_linux_control(
                {
                    "actions": [
                        {
                            "executionPlatform": MODULE.LINUX_EXEC_PLATFORM,
                            "mnemonic": "CppCompile",
                        },
                        archive,
                    ]
                }
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
        arm_archive = action(
            MODULE.NATIVE_LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/linux-arm64/bin/llvm-ar",
            "CppArchive",
        )
        amd_archive = action(
            MODULE.NATIVE_LINUX_EXEC_PLATFORM,
            "external/llvm_toolchain_llvm/bin/linux-amd64/bin/llvm-ar",
            "CppArchive",
        )

        with patch.object(MODULE.platform, "machine", return_value="aarch64"):
            MODULE.verify_native_linux_control({"actions": [arm_action, arm_archive]})
            with self.assertRaisesRegex(RuntimeError, "every action"):
                MODULE.verify_native_linux_control(
                    {"actions": [arm_action, amd_action, arm_archive]}
                )
            with self.assertRaisesRegex(RuntimeError, "llvm-ar"):
                MODULE.verify_native_linux_control(
                    {"actions": [arm_action, amd_archive]}
                )


if __name__ == "__main__":
    unittest.main()
