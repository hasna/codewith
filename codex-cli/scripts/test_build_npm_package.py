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
    def test_codex_package_stages_private_iappcodex_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            build_npm_package.stage_sources(staging_dir, "1.2.3", "codex")

            package_json = read_package_json(staging_dir)

        self.assertEqual(
            package_json,
            {
                "name": "@hasnaxyz/iappcodex",
                "version": "1.2.3",
                "description": "Hasna XYZ internal Codex CLI package.",
                "license": "Apache-2.0",
                "bin": {"iappcodex": "bin/codex.js"},
                "type": "module",
                "engines": {"node": ">=16"},
                "publishConfig": {
                    "registry": "https://registry.npmjs.org",
                    "access": "restricted",
                },
                "files": ["bin/codex.js"],
                "repository": {
                    "type": "git",
                    "url": "git+https://github.com/hasnaxyz/iapp-codex.git",
                    "directory": "codex-cli",
                },
                "packageManager": PACKAGE_MANAGER,
                "optionalDependencies": {
                    "@hasnaxyz/iappcodex-linux-arm64": "1.2.3"
                },
            },
        )

    def test_linux_arm64_package_stages_private_native_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            build_npm_package.stage_sources(staging_dir, "1.2.3", "codex-linux-arm64")

            package_json = read_package_json(staging_dir)

        self.assertEqual(
            package_json,
            {
                "name": "@hasnaxyz/iappcodex-linux-arm64",
                "version": "1.2.3",
                "license": "Apache-2.0",
                "os": ["linux"],
                "cpu": ["arm64"],
                "files": ["vendor"],
                "publishConfig": {
                    "registry": "https://registry.npmjs.org",
                    "access": "restricted",
                },
                "repository": {
                    "type": "git",
                    "url": "git+https://github.com/hasnaxyz/iapp-codex.git",
                    "directory": "codex-cli",
                },
                "engines": {"node": ">=16"},
                "packageManager": PACKAGE_MANAGER,
            },
        )

    def test_linux_alias_uses_gnu_target_for_local_release(self) -> None:
        self.assertEqual(
            build_npm_package.PACKAGE_EXPANSIONS["codex"],
            ["codex", "codex-linux-arm64"],
        )
        self.assertEqual(
            build_npm_package.CODEX_PLATFORM_PACKAGES["codex-linux-arm64"],
            {
                "npm_name": "@hasnaxyz/iappcodex-linux-arm64",
                "npm_tag": "linux-arm64",
                "target_triple": "aarch64-unknown-linux-gnu",
                "os": "linux",
                "cpu": "arm64",
            },
        )


def read_package_json(staging_dir: Path) -> dict:
    with open(staging_dir / "package.json", "r", encoding="utf-8") as fh:
        return json.load(fh)


if __name__ == "__main__":
    unittest.main()
