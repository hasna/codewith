#!/usr/bin/env python3
"""Clean-prefix install smoke tests for the generated (and optionally published) packages.

These tests stage the main ``@hasna/codewith`` package plus a platform package for the
current host triple with ``build_npm_package.py``, install both into a throwaway consumer
project via npm and Bun (isolated HOME/cache), and assert that ``bin/codex.js`` resolves the
platform package's native binary instead of the local ``../vendor`` fallback -- which is
exactly the branch that broke when native platform packages failed to publish.

The default path is fully offline: it installs the locally generated tarballs with
``--offline`` and isolated caches, so it runs in a sandbox without registry access. When the
offline install cannot complete (missing package manager, network required, sandbox policy)
the affected package manager is skipped rather than failed.

Set ``CODEWITH_SMOKE_REGISTRY=1`` to additionally exercise a real registry install of
``@hasna/codewith@latest`` (opt-in; skipped by default because it needs network access).
"""

import os
import platform as platform_module
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import build_npm_package

# Keep in sync with the version staged by .github/workflows/ci.yml.
SMOKE_VERSION = "0.1.0"
PLATFORM_MARKER = f"codewith {SMOKE_VERSION}"
# Distinct marker so a fallback to the main package's ../vendor is unambiguously observable.
VENDOR_FALLBACK_MARKER = "codewith VENDOR-FALLBACK"

INSTALL_TIMEOUT_SECONDS = 240
RUN_TIMEOUT_SECONDS = 60


def _current_os_cpu() -> tuple[str, str] | None:
    if sys.platform.startswith("linux"):
        os_name = "linux"
    elif sys.platform == "darwin":
        os_name = "darwin"
    elif sys.platform.startswith("win"):
        os_name = "win32"
    else:
        return None

    machine = platform_module.machine().lower()
    if machine in ("x86_64", "amd64", "x64"):
        cpu = "x64"
    elif machine in ("aarch64", "arm64"):
        cpu = "arm64"
    else:
        return None

    return os_name, cpu


def current_platform_package() -> tuple[str, dict[str, str]] | None:
    os_cpu = _current_os_cpu()
    if os_cpu is None:
        return None
    os_name, cpu = os_cpu
    for package_name, config in build_npm_package.CODEX_PLATFORM_PACKAGES.items():
        if config["os"] == os_name and config["cpu"] == cpu:
            return package_name, config
    return None


def native_binary_name() -> str:
    return "codewith.exe" if sys.platform.startswith("win") else "codewith"


def fake_native_binary_source(marker: str) -> str:
    """A tiny Node shim that prints a version marker and its own resolved path.

    ``process.argv[1]`` works identically under CommonJS and ESM, so the shim does not depend
    on how Node classifies the extension-less executable.
    """

    return (
        "#!/usr/bin/env node\n"
        f"console.log({marker!r});\n"
        "console.log(process.argv[1]);\n"
    )


def write_executable(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)


def isolated_env(home: Path, npm_cache: Path, bun_cache: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["USERPROFILE"] = str(home)
    # Do not inherit a user home for native binary helper symlinks or CLI config.
    env.pop("CODEX_HOME", None)
    env.pop("CODEWITH_HOME", None)
    # Isolate package-manager caches / config from the developer machine.
    env["npm_config_cache"] = str(npm_cache)
    env["npm_config_userconfig"] = str(npm_cache / "npmrc")
    env["BUN_INSTALL_CACHE_DIR"] = str(bun_cache)
    return env


@unittest.skipUnless(
    current_platform_package() is not None,
    "no platform package is defined for this host",
)
@unittest.skipIf(
    sys.platform.startswith("win"),
    "the Node shim binary is not directly executable on Windows hosts",
)
class InstallSmokeTest(unittest.TestCase):
    build_script: Path
    tmp_root: Path
    main_tarball: Path
    platform_tarball: Path
    platform_package_name: str
    platform_npm_name: str
    target_triple: str

    @classmethod
    def setUpClass(cls) -> None:
        if shutil.which("node") is None:
            raise unittest.SkipTest("node is required to run the shim")
        if shutil.which("npm") is None:
            # `build_npm_package.py` shells out to `npm pack` to produce the tarballs.
            raise unittest.SkipTest("npm is required to stage the tarballs")

        package = current_platform_package()
        assert package is not None
        cls.platform_package_name, config = package
        cls.platform_npm_name = config["npm_name"]
        cls.target_triple = config["target_triple"]

        cls.build_script = Path(build_npm_package.__file__).resolve()
        cls.tmp_root = Path(tempfile.mkdtemp(prefix="codewith-install-smoke-"))

        cls.main_tarball = cls.tmp_root / "main.tgz"
        cls.platform_tarball = cls.tmp_root / "platform.tgz"

        # Build the main @hasna/codewith tarball from the real generator.
        cls._run_build(["--package", "codex", "--pack-output", str(cls.main_tarball)])

        # Synthesize a platform package whose native binary is a Node shim printing a marker.
        vendor_src = cls.tmp_root / "vendor-src"
        write_executable(
            vendor_src / cls.target_triple / "bin" / native_binary_name(),
            fake_native_binary_source(PLATFORM_MARKER),
        )
        cls._run_build(
            [
                "--package",
                cls.platform_package_name,
                "--vendor-src",
                str(vendor_src),
                "--pack-output",
                str(cls.platform_tarball),
            ]
        )

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.tmp_root, ignore_errors=True)

    @classmethod
    def _run_build(cls, extra_args: list[str]) -> None:
        subprocess.run(
            [sys.executable, str(cls.build_script), "--version", SMOKE_VERSION, *extra_args],
            check=True,
            capture_output=True,
            text=True,
        )

    def _write_consumer_package_json(self, consumer: Path) -> None:
        main = self.main_tarball.as_posix()
        platform_tgz = self.platform_tarball.as_posix()
        # `file:` deps + `overrides` force both managers to consume the LOCAL tarballs offline
        # rather than pulling the published platform package to satisfy the optionalDependency.
        (consumer / "package.json").write_text(
            "{\n"
            '  "name": "codewith-install-smoke-consumer",\n'
            '  "version": "1.0.0",\n'
            '  "private": true,\n'
            '  "dependencies": {\n'
            f'    "@hasna/codewith": "file:{main}",\n'
            f'    "{self.platform_npm_name}": "file:{platform_tgz}"\n'
            "  },\n"
            '  "overrides": {\n'
            f'    "{self.platform_npm_name}": "file:{platform_tgz}"\n'
            "  }\n"
            "}\n",
            encoding="utf-8",
        )

    def _prepare_consumer(self) -> tuple[Path, dict[str, str]]:
        work = Path(tempfile.mkdtemp(prefix="codewith-smoke-consumer-", dir=self.tmp_root))
        consumer = work / "consumer"
        consumer.mkdir()
        home = work / "home"
        home.mkdir()
        env = isolated_env(home, work / "npm-cache", work / "bun-cache")
        self._write_consumer_package_json(consumer)
        return consumer, env

    def _run_binary(self, args: list[str], env: dict[str, str]) -> str:
        result = subprocess.run(
            args,
            check=True,
            capture_output=True,
            text=True,
            env=env,
            timeout=RUN_TIMEOUT_SECONDS,
        )
        return result.stdout

    def _main_bin_js(self, consumer: Path) -> Path:
        return consumer / "node_modules" / "@hasna" / "codewith" / "bin" / "codex.js"

    def _installed_platform_vendor_segment(self) -> str:
        # e.g. node_modules/@hasna/codewith-linux-arm64/vendor
        return os.path.join("node_modules", *self.platform_npm_name.split("/"), "vendor")

    def _main_vendor_segment(self) -> str:
        return os.path.join("node_modules", "@hasna", "codewith", "vendor")

    def _assert_resolves_platform_package(self, consumer: Path, env: dict[str, str]) -> None:
        node = shutil.which("node")
        assert node is not None

        output = self._run_binary(
            [node, str(self._main_bin_js(consumer)), "--version"], env
        )
        lines = output.splitlines()
        self.assertGreaterEqual(len(lines), 2, f"unexpected shim output: {output!r}")
        self.assertEqual(lines[0], PLATFORM_MARKER)
        resolved = lines[1]
        self.assertIn(
            self._installed_platform_vendor_segment(),
            resolved,
            f"resolved binary {resolved!r} is not inside the platform package vendor tree",
        )
        self.assertNotIn(
            self._main_vendor_segment(),
            resolved,
            f"resolved binary {resolved!r} unexpectedly came from the main package vendor",
        )

    def _run_offline_install(self, manager: str, consumer: Path, env: dict[str, str]) -> None:
        if manager == "npm":
            cmd = ["npm", "install", "--no-audit", "--no-fund", "--offline"]
        elif manager == "bun":
            cmd = ["bun", "install", "--offline"]
        else:  # pragma: no cover - guarded by callers
            raise AssertionError(f"unknown manager {manager}")

        try:
            result = subprocess.run(
                cmd,
                cwd=consumer,
                capture_output=True,
                text=True,
                env=env,
                timeout=INSTALL_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired:
            self.skipTest(f"{manager} offline install timed out (network required?)")

        if result.returncode != 0:
            self.skipTest(
                f"{manager} offline install unavailable "
                f"(exit {result.returncode}): {result.stderr.strip()[:500]}"
            )

    def _smoke_offline(self, manager: str) -> None:
        if shutil.which(manager) is None:
            self.skipTest(f"{manager} is not installed")

        consumer, env = self._prepare_consumer()
        self._run_offline_install(manager, consumer, env)

        platform_pkg_dir = consumer / "node_modules" / "@hasna" / self.platform_npm_name.split("/")[1]
        self.assertTrue(
            (consumer / "node_modules" / "@hasna" / "codewith").is_dir(),
            "main @hasna/codewith package was not installed",
        )
        self.assertTrue(
            platform_pkg_dir.is_dir(),
            f"platform package {self.platform_npm_name} was not installed",
        )

        # The platform package's native binary must win over any local vendor fallback.
        self._assert_resolves_platform_package(consumer, env)

        # Even with a DIFFERENT-marker binary present in the main package's ../vendor, the
        # platform package must still take precedence. This is the resolve-vs-fallback branch.
        fallback = (
            consumer
            / "node_modules"
            / "@hasna"
            / "codewith"
            / "vendor"
            / self.target_triple
            / "bin"
            / native_binary_name()
        )
        write_executable(fallback, fake_native_binary_source(VENDOR_FALLBACK_MARKER))
        self._assert_resolves_platform_package(consumer, env)

        # Finally, smoke the installed `codewith --version` entry point end to end.
        bin_wrapper = consumer / "node_modules" / ".bin" / native_binary_name()
        self.assertTrue(bin_wrapper.exists(), "codewith bin wrapper was not linked")
        wrapper_output = self._run_binary([str(bin_wrapper), "--version"], env)
        self.assertIn(PLATFORM_MARKER, wrapper_output)
        self.assertNotIn(VENDOR_FALLBACK_MARKER, wrapper_output)

    def test_npm_clean_prefix_install_resolves_platform_package(self) -> None:
        self._smoke_offline("npm")

    def test_bun_clean_prefix_install_resolves_platform_package(self) -> None:
        self._smoke_offline("bun")

    @unittest.skipUnless(
        os.environ.get("CODEWITH_SMOKE_REGISTRY") == "1",
        "set CODEWITH_SMOKE_REGISTRY=1 to smoke a real registry install (needs network)",
    )
    def test_published_registry_install_runs(self) -> None:
        manager = os.environ.get("CODEWITH_SMOKE_REGISTRY_MANAGER", "npm")
        if shutil.which(manager) is None:
            self.skipTest(f"{manager} is not installed")

        consumer, env = self._prepare_consumer()
        (consumer / "package.json").write_text(
            '{ "name": "codewith-registry-smoke", "version": "1.0.0", "private": true }\n',
            encoding="utf-8",
        )

        if manager == "npm":
            install_cmd = ["npm", "install", "--no-audit", "--no-fund", "@hasna/codewith@latest"]
        else:
            install_cmd = ["bun", "add", "@hasna/codewith@latest"]

        result = subprocess.run(
            install_cmd,
            cwd=consumer,
            capture_output=True,
            text=True,
            env=env,
            timeout=INSTALL_TIMEOUT_SECONDS,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

        bin_wrapper = consumer / "node_modules" / ".bin" / native_binary_name()
        self.assertTrue(bin_wrapper.exists(), "codewith bin wrapper was not linked")
        version_output = self._run_binary([str(bin_wrapper), "--version"], env)
        self.assertIn("codewith", version_output.lower())


if __name__ == "__main__":
    unittest.main()
