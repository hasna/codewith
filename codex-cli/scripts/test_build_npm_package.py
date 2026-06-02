#!/usr/bin/env python3

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import build_npm_package


PACKAGE_MANAGER = (
    "pnpm@10.33.0+sha512.10568bb4a6afb58c9eb3630da90cc9516417abebd3fabbe6739"
    "f0ae795728da1491e9db5a544c76ad8eb7570f5c4bb3d6c637b2cb41bfdcdb47fa823c8649319"
)


class BuildNpmPackageTest(unittest.TestCase):
    def test_codex_package_stages_public_codewith_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            build_npm_package.stage_sources(staging_dir, "1.2.3", "codex")

            package_json = read_package_json(staging_dir)

        self.assertEqual(
            package_json,
            {
                "name": "@hasna/codewith",
                "version": "1.2.3",
                "description": "Codewith command-line coding agent from Hasna.",
                "license": "Apache-2.0",
                "bin": {"codewith": "bin/codex.js"},
                "type": "module",
                "engines": {"node": ">=16"},
                "publishConfig": {
                    "registry": "https://registry.npmjs.org",
                    "access": "public",
                },
                "files": ["bin/codex.js"],
                "repository": {
                    "type": "git",
                    "url": "git+https://github.com/hasna/codewith.git",
                    "directory": "codex-cli",
                },
                "packageManager": PACKAGE_MANAGER,
                "optionalDependencies": {
                    "@hasna/codewith-linux-x64": "1.2.3",
                    "@hasna/codewith-linux-arm64": "1.2.3",
                    "@hasna/codewith-darwin-x64": "1.2.3",
                    "@hasna/codewith-darwin-arm64": "1.2.3",
                    "@hasna/codewith-win32-x64": "1.2.3",
                    "@hasna/codewith-win32-arm64": "1.2.3",
                },
            },
        )

    def test_linux_arm64_package_stages_public_native_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            build_npm_package.stage_sources(staging_dir, "1.2.3", "codex-linux-arm64")

            package_json = read_package_json(staging_dir)

        self.assertEqual(
            package_json,
            {
                "name": "@hasna/codewith-linux-arm64",
                "version": "1.2.3",
                "license": "Apache-2.0",
                "os": ["linux"],
                "cpu": ["arm64"],
                "files": ["vendor"],
                "publishConfig": {
                    "registry": "https://registry.npmjs.org",
                    "access": "public",
                },
                "repository": {
                    "type": "git",
                    "url": "git+https://github.com/hasna/codewith.git",
                    "directory": "codex-cli",
                },
                "engines": {"node": ">=16"},
                "packageManager": PACKAGE_MANAGER,
            },
        )

    def test_linux_alias_uses_musl_target_for_upstream_release_artifacts(self) -> None:
        self.assertEqual(
            build_npm_package.PACKAGE_EXPANSIONS["codex"],
            [
                "codex",
                "codex-linux-x64",
                "codex-linux-arm64",
                "codex-darwin-x64",
                "codex-darwin-arm64",
                "codex-win32-x64",
                "codex-win32-arm64",
            ],
        )
        self.assertEqual(
            build_npm_package.CODEX_PLATFORM_PACKAGES["codex-linux-arm64"],
            {
                "npm_name": "@hasna/codewith-linux-arm64",
                "npm_tag": "linux-arm64",
                "target_triple": "aarch64-unknown-linux-musl",
                "os": "linux",
                "cpu": "arm64",
            },
        )


def read_package_json(staging_dir: Path) -> dict:
    with open(staging_dir / "package.json", "r", encoding="utf-8") as fh:
        return json.load(fh)


if __name__ == "__main__":
    unittest.main()
