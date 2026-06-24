#!/usr/bin/env python3

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

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
            compliance_files = read_compliance_files(staging_dir)
            third_party_license_files = read_third_party_license_files(staging_dir)

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
                "files": [
                    "bin/codex.js",
                    "LICENSE",
                    "NOTICE",
                    "MODIFICATIONS.md",
                    "THIRD_PARTY_NOTICES.md",
                    "licenses",
                ],
                "repository": {
                    "type": "git",
                    "url": "git+https://github.com/hasna/codewith.git",
                    "directory": "codex-cli",
                },
                "bugs": {"url": "https://github.com/hasna/codewith/issues"},
                "homepage": "https://github.com/hasna/codewith#readme",
                "packageManager": PACKAGE_MANAGER,
                "optionalDependencies": {
                    platform_config["npm_name"]: "1.2.3"
                    for platform_config in build_npm_package.CODEX_PLATFORM_PACKAGES.values()
                },
            },
        )
        self.assertEqual(compliance_files, repo_compliance_files())
        self.assertEqual(third_party_license_files, repo_third_party_license_files())

    def test_linux_arm64_package_stages_public_native_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            build_npm_package.stage_sources(staging_dir, "1.2.3", "codex-linux-arm64")

            package_json = read_package_json(staging_dir)
            compliance_files = read_compliance_files(staging_dir)
            third_party_license_files = read_third_party_license_files(staging_dir)

        self.assertEqual(
            package_json,
            {
                "name": "@hasna/codewith-linux-arm64",
                "version": "1.2.3",
                "license": "Apache-2.0",
                "os": ["linux"],
                "cpu": ["arm64"],
                "files": [
                    "vendor",
                    "LICENSE",
                    "NOTICE",
                    "MODIFICATIONS.md",
                    "THIRD_PARTY_NOTICES.md",
                    "licenses",
                ],
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
                "bugs": {"url": "https://github.com/hasna/codewith/issues"},
                "homepage": "https://github.com/hasna/codewith#readme",
            },
        )
        self.assertEqual(compliance_files, repo_compliance_files())
        self.assertEqual(third_party_license_files, repo_third_party_license_files())

    def test_codex_expansion_matches_configured_native_release_artifacts(self) -> None:
        self.assertEqual(
            build_npm_package.PACKAGE_EXPANSIONS["codex"],
            ["codex", *build_npm_package.CODEX_PLATFORM_PACKAGES.keys()],
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

    def test_responses_api_proxy_package_stages_public_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            build_npm_package.stage_sources(
                staging_dir,
                "1.2.3",
                "codex-responses-api-proxy",
            )

            package_json = read_package_json(staging_dir)
            compliance_files = read_compliance_files(staging_dir)
            third_party_license_files = read_third_party_license_files(staging_dir)

        self.assertEqual(package_json["name"], "@hasna/codewith-responses-api-proxy")
        self.assertEqual(package_json["version"], "1.2.3")
        self.assertEqual(
            package_json["bugs"],
            {"url": "https://github.com/hasna/codewith/issues"},
        )
        self.assertEqual(package_json["homepage"], "https://github.com/hasna/codewith#readme")
        self.assertEqual(
            package_json["files"],
            [
                "bin",
                "vendor",
                "LICENSE",
                "NOTICE",
                "MODIFICATIONS.md",
                "THIRD_PARTY_NOTICES.md",
                "licenses",
            ],
        )
        self.assertEqual(
            package_json["publishConfig"],
            {
                "registry": "https://registry.npmjs.org",
                "access": "public",
            },
        )
        self.assertEqual(compliance_files, repo_compliance_files())
        self.assertEqual(third_party_license_files, repo_third_party_license_files())

    def test_sdk_package_stages_public_metadata(self) -> None:
        def fake_stage_sdk_sources(staging_dir: Path) -> None:
            (staging_dir / "dist").mkdir()

        with tempfile.TemporaryDirectory() as temp_dir:
            staging_dir = Path(temp_dir)

            with mock.patch.object(
                build_npm_package,
                "stage_codex_sdk_sources",
                fake_stage_sdk_sources,
            ):
                build_npm_package.stage_sources(staging_dir, "1.2.3", "codex-sdk")

            package_json = read_package_json(staging_dir)
            compliance_files = read_compliance_files(staging_dir)
            third_party_license_files = read_third_party_license_files(staging_dir)

        self.assertEqual(package_json["name"], "@hasna/codewith-sdk")
        self.assertEqual(package_json["version"], "1.2.3")
        self.assertEqual(
            package_json["bugs"],
            {"url": "https://github.com/hasna/codewith/issues"},
        )
        self.assertEqual(package_json["homepage"], "https://github.com/hasna/codewith#readme")
        self.assertEqual(
            package_json["files"],
            [
                "dist",
                "LICENSE",
                "NOTICE",
                "MODIFICATIONS.md",
                "THIRD_PARTY_NOTICES.md",
                "licenses",
            ],
        )
        self.assertEqual(
            package_json["publishConfig"],
            {
                "registry": "https://registry.npmjs.org",
                "access": "public",
            },
        )
        self.assertEqual(package_json["dependencies"]["@hasna/codewith"], "1.2.3")
        self.assertNotIn("prepare", package_json["scripts"])
        self.assertEqual(compliance_files, repo_compliance_files())
        self.assertEqual(third_party_license_files, repo_third_party_license_files())


def read_package_json(staging_dir: Path) -> dict:
    with open(staging_dir / "package.json", "r", encoding="utf-8") as fh:
        return json.load(fh)


def read_compliance_files(staging_dir: Path) -> dict[str, str]:
    return {
        name: (staging_dir / name).read_text(encoding="utf-8")
        for name in build_npm_package.COMPLIANCE_FILES
    }


def repo_compliance_files() -> dict[str, str]:
    return {
        name: (build_npm_package.REPO_ROOT / name).read_text(encoding="utf-8")
        for name in build_npm_package.COMPLIANCE_FILES
    }


def read_third_party_license_files(staging_dir: Path) -> dict[str, str]:
    return {
        name: (staging_dir / "licenses" / name).read_text(encoding="utf-8")
        for name in build_npm_package.THIRD_PARTY_LICENSE_FILES
    }


def repo_third_party_license_files() -> dict[str, str]:
    return {
        name: source.read_text(encoding="utf-8")
        for name, source in build_npm_package.THIRD_PARTY_LICENSE_FILES.items()
    }


if __name__ == "__main__":
    unittest.main()
