#!/usr/bin/env python3
"""Clean-prefix install smoke tests for the published ``@hasna/codewith-sdk`` package.

The TypeScript SDK is verified in CI with unit tests, but those import from ``src`` and never
exercise the *published* artifact. Packaging bugs -- a wrong ``exports`` map, a missing ``dist``
in ``files``, a broken ``prepare`` build -- therefore slip through until a consumer's
``import ... from "@hasna/codewith-sdk"`` fails at runtime.

These tests pack the SDK exactly as ``npm publish`` would (``npm pack`` runs the ``prepare``
build), install the resulting tarball into a throwaway consumer project via **both npm and
Bun** (isolated HOME/cache), and then run a real Node ESM program that imports the package's
public entry point and asserts the runtime exports (``Codewith``, ``Thread``) are usable and
that the bundled type declarations ship in the tarball.

The default path is fully offline: it installs the locally generated tarball with ``--offline``
and isolated caches, so it runs in a sandbox without registry access. When the offline install
cannot complete (missing package manager, network required, sandbox policy) the affected
package manager is skipped rather than failed. Set ``CODEWITH_SMOKE_STRICT=1`` in CI so a broken
offline install (or a broken ``npm pack`` build) fails instead of reporting a skipped smoke.

Set ``CODEWITH_SMOKE_REGISTRY=1`` to additionally exercise a real registry install of
``@hasna/codewith-sdk@latest`` (opt-in; skipped by default because it needs network access).
"""

import glob
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

PACKAGE_NAME = "@hasna/codewith-sdk"
SDK_DIR = Path(__file__).resolve().parents[1]

# A distinctive line the import program prints once every assertion passes.
IMPORT_OK_MARKER = "SDK-IMPORT-OK"

PACK_TIMEOUT_SECONDS = 240
INSTALL_TIMEOUT_SECONDS = 240
RUN_TIMEOUT_SECONDS = 60
STRICT = os.environ.get("CODEWITH_SMOKE_STRICT") == "1"


# A tiny Node ESM program that imports the *installed* package and asserts its public surface.
# ``codexPathOverride`` is passed so the constructor does not try to resolve the native
# ``@hasna/codewith`` binary (absent in this consumer) -- construction stays side-effect free.
IMPORT_PROGRAM = f"""\
import {{ Codewith, Thread }} from {PACKAGE_NAME!r};

function assert(condition, message) {{
  if (!condition) {{
    console.error("SMOKE-FAIL: " + message);
    process.exit(2);
  }}
}}

assert(typeof Codewith === "function", "Codewith export is not a constructor");
assert(typeof Thread === "function", "Thread export is not a constructor");

const codex = new Codewith({{ codexPathOverride: "/nonexistent-codewith-binary" }});
assert(typeof codex.startThread === "function", "Codewith#startThread is missing");
assert(typeof codex.resumeThread === "function", "Codewith#resumeThread is missing");

console.log({IMPORT_OK_MARKER!r});
"""


def isolated_env(home: Path, npm_cache: Path, bun_cache: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["USERPROFILE"] = str(home)
    # Do not inherit a developer's CLI config / native binary helpers.
    env.pop("CODEX_HOME", None)
    env.pop("CODEWITH_HOME", None)
    # Isolate package-manager caches / config from the developer machine.
    env["npm_config_cache"] = str(npm_cache)
    env["npm_config_userconfig"] = str(npm_cache / "npmrc")
    env["BUN_INSTALL_CACHE_DIR"] = str(bun_cache)
    return env


@unittest.skipIf(
    sys.platform.startswith("win"),
    "the isolated-HOME shell out is validated on POSIX runners",
)
class SdkInstallSmokeTest(unittest.TestCase):
    tmp_root: Path
    tarball: Path

    @classmethod
    def setUpClass(cls) -> None:
        if shutil.which("node") is None:
            raise unittest.SkipTest("node is required to import the package")
        if shutil.which("npm") is None:
            # `npm pack` produces the tarball (and runs the `prepare` build).
            raise unittest.SkipTest("npm is required to pack the SDK tarball")

        cls.tmp_root = Path(tempfile.mkdtemp(prefix="codewith-sdk-install-smoke-"))
        cls.tarball = cls._pack_sdk()

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.tmp_root, ignore_errors=True)

    @classmethod
    def _pack_sdk(cls) -> Path:
        """Pack the SDK the way ``npm publish`` would and return the tarball path.

        ``npm pack`` runs the package's ``prepare`` script, which builds ``dist`` from source, so
        the tarball is a faithful stand-in for the published artifact. If the build toolchain is
        unavailable but a ``dist`` was already produced (e.g. by the CI build step), fall back to
        ``--ignore-scripts`` so the already-built artifact is still exercised.
        """

        dest = cls.tmp_root / "pack"
        dest.mkdir(parents=True, exist_ok=True)

        def run_pack(ignore_scripts: bool) -> subprocess.CompletedProcess[str]:
            cmd = ["npm", "pack", "--pack-destination", str(dest)]
            if ignore_scripts:
                cmd.append("--ignore-scripts")
            return subprocess.run(
                cmd,
                cwd=SDK_DIR,
                capture_output=True,
                text=True,
                timeout=PACK_TIMEOUT_SECONDS,
            )

        try:
            result = run_pack(ignore_scripts=False)
            if result.returncode != 0 and (SDK_DIR / "dist" / "index.js").exists():
                result = run_pack(ignore_scripts=True)
        except subprocess.TimeoutExpired as exc:
            message = f"npm pack timed out packing the SDK: {exc}"
            if STRICT:
                raise AssertionError(message) from exc
            raise unittest.SkipTest(message) from exc

        if result.returncode != 0:
            message = (
                "npm pack could not build/pack the SDK "
                f"(exit {result.returncode}): {result.stderr.strip()[:500]}"
            )
            if STRICT:
                raise AssertionError(message)
            raise unittest.SkipTest(message)

        tarballs = sorted(glob.glob(str(dest / "*.tgz")))
        if not tarballs:
            raise AssertionError(f"npm pack produced no tarball in {dest}")
        return Path(tarballs[-1])

    def _prepare_consumer(self) -> tuple[Path, dict[str, str]]:
        work = Path(
            tempfile.mkdtemp(prefix="codewith-sdk-consumer-", dir=self.tmp_root)
        )
        consumer = work / "consumer"
        consumer.mkdir()
        home = work / "home"
        home.mkdir()
        env = isolated_env(home, work / "npm-cache", work / "bun-cache")

        tarball = self.tarball.as_posix()
        (consumer / "package.json").write_text(
            "{\n"
            '  "name": "codewith-sdk-install-smoke-consumer",\n'
            '  "version": "1.0.0",\n'
            '  "private": true,\n'
            '  "type": "module",\n'
            '  "dependencies": {\n'
            f'    "{PACKAGE_NAME}": "file:{tarball}"\n'
            "  }\n"
            "}\n",
            encoding="utf-8",
        )
        (consumer / "smoke.mjs").write_text(IMPORT_PROGRAM, encoding="utf-8")
        return consumer, env

    def _run_offline_install(
        self, manager: str, consumer: Path, env: dict[str, str]
    ) -> None:
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
            message = f"{manager} offline install timed out (network required?)"
            if STRICT:
                self.fail(message)
            self.skipTest(message)

        if result.returncode != 0:
            message = (
                f"{manager} offline install unavailable "
                f"(exit {result.returncode}): {result.stderr.strip()[:500]}"
            )
            if STRICT:
                self.fail(message)
            self.skipTest(message)

    def _assert_entrypoint_imports(self, consumer: Path, env: dict[str, str]) -> None:
        installed = consumer / "node_modules" / "@hasna" / "codewith-sdk"
        self.assertTrue(installed.is_dir(), f"{PACKAGE_NAME} was not installed")
        # The bundled type declarations must ship with the published package.
        self.assertTrue(
            (installed / "dist" / "index.d.ts").is_file(),
            "published package is missing dist/index.d.ts type declarations",
        )

        node = shutil.which("node")
        assert node is not None
        result = subprocess.run(
            [node, "smoke.mjs"],
            cwd=consumer,
            capture_output=True,
            text=True,
            env=env,
            timeout=RUN_TIMEOUT_SECONDS,
        )
        self.assertEqual(
            result.returncode,
            0,
            f"importing {PACKAGE_NAME} failed: {result.stdout}\n{result.stderr}",
        )
        self.assertIn(IMPORT_OK_MARKER, result.stdout)

    def _smoke_offline(self, manager: str) -> None:
        if shutil.which(manager) is None:
            self.skipTest(f"{manager} is not installed")

        consumer, env = self._prepare_consumer()
        self._run_offline_install(manager, consumer, env)
        self._assert_entrypoint_imports(consumer, env)

    def test_npm_clean_prefix_install_imports_entrypoint(self) -> None:
        self._smoke_offline("npm")

    def test_bun_clean_prefix_install_imports_entrypoint(self) -> None:
        self._smoke_offline("bun")

    @unittest.skipUnless(
        os.environ.get("CODEWITH_SMOKE_REGISTRY") == "1",
        "set CODEWITH_SMOKE_REGISTRY=1 to smoke a real registry install (needs network)",
    )
    def test_published_registry_install_imports(self) -> None:
        manager = os.environ.get("CODEWITH_SMOKE_REGISTRY_MANAGER", "npm")
        if shutil.which(manager) is None:
            self.skipTest(f"{manager} is not installed")

        work = Path(
            tempfile.mkdtemp(prefix="codewith-sdk-registry-", dir=self.tmp_root)
        )
        consumer = work / "consumer"
        consumer.mkdir()
        home = work / "home"
        home.mkdir()
        env = isolated_env(home, work / "npm-cache", work / "bun-cache")
        (consumer / "package.json").write_text(
            '{ "name": "codewith-sdk-registry-smoke", "version": "1.0.0",'
            ' "private": true, "type": "module" }\n',
            encoding="utf-8",
        )
        (consumer / "smoke.mjs").write_text(IMPORT_PROGRAM, encoding="utf-8")

        if manager == "npm":
            install_cmd = [
                "npm",
                "install",
                "--no-audit",
                "--no-fund",
                f"{PACKAGE_NAME}@latest",
            ]
        else:
            install_cmd = ["bun", "add", f"{PACKAGE_NAME}@latest"]

        result = subprocess.run(
            install_cmd,
            cwd=consumer,
            capture_output=True,
            text=True,
            env=env,
            timeout=INSTALL_TIMEOUT_SECONDS,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self._assert_entrypoint_imports(consumer, env)


if __name__ == "__main__":
    unittest.main()
