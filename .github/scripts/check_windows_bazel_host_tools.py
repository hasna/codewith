#!/usr/bin/env python3

"""Verify Windows-cross Bazel actions resolve Windows execution tools.

This is an analysis probe rather than part of the lightweight Python unit-test
job because the aquery loads the Windows Rust and C++ toolchains. CI runs it
once on the primary Linux leg; it does not execute any build actions.
"""

import json
import os
import platform
import stat
import subprocess
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
WINDOWS_EXEC_PLATFORM = "//:windows_x86_64_msvc"
LLVM_WINDOWS_TOOL_FRAGMENT = "windows-amd64/bin/clang.exe"
LLVM_LINUX_TOOL_FRAGMENT = "linux-amd64/bin/clang"
LLVM_WINDOWS_ARCHIVER_FRAGMENT = "windows-amd64/bin/llvm-ar.exe"
LLVM_LINUX_ARCHIVER_FRAGMENT = "linux-amd64/bin/llvm-ar"
LINUX_EXEC_PLATFORM = "//:linux_x86_64_ci_analysis"
NATIVE_LINUX_EXEC_PLATFORM = "//:local_linux"


def verify_actions(payload: dict[str, object]) -> None:
    actions = payload.get("actions")
    if not isinstance(actions, list) or not actions:
        raise RuntimeError("aquery returned no C++ compile actions")

    wrong_platforms = sorted(
        {
            str(action.get("executionPlatform"))
            for action in actions
            if isinstance(action, dict)
            and action.get("executionPlatform") != WINDOWS_EXEC_PLATFORM
        }
    )
    if wrong_platforms:
        raise RuntimeError(
            "Windows-cross actions resolved non-Windows execution platforms: "
            + ", ".join(wrong_platforms)
        )

    compiler_paths = _tool_paths(actions, "Windows-cross", "CppCompile", "compiler")
    wrong_compilers = sorted(
        path for path in compiler_paths if LLVM_WINDOWS_TOOL_FRAGMENT not in path
    )
    if wrong_compilers:
        raise RuntimeError(
            "Windows-cross C++ actions did not use Windows AMD64 LLVM clang.exe for "
            f"every action; unexpected compilers were: {wrong_compilers}"
        )

    archiver_paths = _tool_paths(actions, "Windows-cross", "CppArchive", "archiver")
    wrong_archivers = sorted(
        path
        for path in archiver_paths
        if LLVM_WINDOWS_ARCHIVER_FRAGMENT not in path
    )
    if wrong_archivers:
        raise RuntimeError(
            "Windows-cross C++ archive actions did not use Windows AMD64 LLVM "
            f"llvm-ar.exe for every action; unexpected archivers were: {wrong_archivers}"
        )


def _tool_paths(
    actions: list[object],
    description: str,
    mnemonic: str,
    tool_description: str,
) -> list[str]:
    matching_actions = [
        action
        for action in actions
        if isinstance(action, dict) and action.get("mnemonic") == mnemonic
    ]
    if not matching_actions:
        raise RuntimeError(f"{description} aquery returned no {mnemonic} actions")

    tool_paths: list[str] = []
    for action in matching_actions:
        arguments = action.get("arguments")
        if (
            not isinstance(arguments, list)
            or not arguments
            or not isinstance(arguments[0], str)
        ):
            raise RuntimeError(
                f"{description} aquery returned an action without "
                f"{tool_description} arguments"
            )
        tool_paths.append(arguments[0].replace("\\", "/"))
    return tool_paths


def run_aquery(env: dict[str, str], *options: str) -> dict[str, object]:
    command = [
        env.get("CODEX_BAZEL_BIN", "bazel"),
        "--noexperimental_remote_repo_contents_cache",
        "aquery",
        *options,
        "--repo_contents_cache=",
        "--remote_cache=",
        "--remote_executor=",
        "--experimental_remote_downloader=",
        "--output=jsonproto",
        'mnemonic("Cpp(Compile|Archive)", @openssl//:crypto)',
    ]
    result = subprocess.run(
        command,
        cwd=REPO_ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "Bazel host-tools aquery failed:\n"
            f"stdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
    return json.loads(result.stdout)


def verify_linux_control(payload: dict[str, object]) -> None:
    actions = payload.get("actions")
    if not isinstance(actions, list) or not actions:
        raise RuntimeError("Linux control aquery returned no C++ compile actions")
    if any(
        not isinstance(action, dict)
        or action.get("executionPlatform") != LINUX_EXEC_PLATFORM
        for action in actions
    ):
        raise RuntimeError("x86_64 Linux C++ actions changed execution platform")
    compiler_paths = _tool_paths(actions, "Linux control", "CppCompile", "compiler")
    wrong_compilers = sorted(
        path for path in compiler_paths if LLVM_LINUX_TOOL_FRAGMENT not in path
    )
    if wrong_compilers:
        raise RuntimeError(
            "x86_64 Linux C++ actions did not use Linux AMD64 LLVM clang for every "
            f"action; unexpected compilers were: {wrong_compilers}"
        )

    archiver_paths = _tool_paths(actions, "Linux control", "CppArchive", "archiver")
    wrong_archivers = sorted(
        path for path in archiver_paths if LLVM_LINUX_ARCHIVER_FRAGMENT not in path
    )
    if wrong_archivers:
        raise RuntimeError(
            "x86_64 Linux C++ archive actions did not use Linux AMD64 LLVM llvm-ar "
            f"for every action; unexpected archivers were: {wrong_archivers}"
        )


def verify_native_linux_control(payload: dict[str, object]) -> None:
    actions = payload.get("actions")
    if not isinstance(actions, list) or not actions:
        raise RuntimeError("Native Linux control aquery returned no C++ compile actions")
    if any(
        not isinstance(action, dict)
        or action.get("executionPlatform") != NATIVE_LINUX_EXEC_PLATFORM
        for action in actions
    ):
        raise RuntimeError("native Linux C++ actions changed execution platform")

    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        compiler_fragment = "linux-amd64/bin/clang"
        archiver_fragment = "linux-amd64/bin/llvm-ar"
    elif machine in {"aarch64", "arm64"}:
        compiler_fragment = "linux-arm64/bin/clang"
        archiver_fragment = "linux-arm64/bin/llvm-ar"
    else:
        raise RuntimeError(f"unsupported native Linux architecture: {machine}")

    compiler_paths = _tool_paths(
        actions,
        "Native Linux control",
        "CppCompile",
        "compiler",
    )
    wrong_compilers = sorted(
        path for path in compiler_paths if compiler_fragment not in path
    )
    if wrong_compilers:
        raise RuntimeError(
            "native Linux C++ actions did not use the host-architecture LLVM clang "
            f"for every action; unexpected compilers were: {wrong_compilers}"
        )

    archiver_paths = _tool_paths(
        actions,
        "Native Linux control",
        "CppArchive",
        "archiver",
    )
    wrong_archivers = sorted(
        path for path in archiver_paths if archiver_fragment not in path
    )
    if wrong_archivers:
        raise RuntimeError(
            "native Linux C++ archive actions did not use the host-architecture "
            f"LLVM llvm-ar for every action; unexpected archivers were: {wrong_archivers}"
        )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="codewith-windows-aquery-") as temp_dir:
        temp_path = Path(temp_dir)
        env = os.environ.copy()
        if os.name != "nt":
            # rules_rs locates lld-link.exe while materializing the registered
            # Windows exec Rust toolchain. Aquery never executes this stub.
            lld_link = temp_path / "lld-link.exe"
            lld_link.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
            lld_link.chmod(lld_link.stat().st_mode | stat.S_IXUSR)
            env["PATH"] = f"{temp_path}{os.pathsep}{env['PATH']}"

        verify_actions(run_aquery(env, "--config=ci-windows-cross"))
        if os.name != "nt":
            verify_linux_control(
                run_aquery(
                    env,
                    "--extra_execution_platforms=//:linux_x86_64_ci_analysis",
                )
            )
            verify_native_linux_control(run_aquery(env))

    print(
        "Windows-cross aquery resolved Windows execution platform and Windows AMD64 "
        "LLVM compiler and archiver tools; the x86_64 Linux control retained Linux "
        "AMD64 LLVM tools, and the native Linux control retained its host-architecture "
        "LLVM tools."
    )


if __name__ == "__main__":
    main()
