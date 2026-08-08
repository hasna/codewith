from __future__ import annotations

import re
import unittest
from pathlib import Path


def find_repo_root() -> Path:
    candidates = (Path.cwd(), *Path(__file__).resolve().parents)
    for candidate in candidates:
        if (
            candidate.joinpath(".codewith/CODEWITH.md").is_file()
            and candidate.joinpath(".github/workflows/blacksmith-testbox.yml").is_file()
        ):
            return candidate
    raise RuntimeError("run this contract test from a hasna/codewith checkout")


REPO_ROOT = find_repo_root()
SKILL_PATH = REPO_ROOT / ".codewith/skills/blacksmith-testbox/SKILL.md"
POLICY_PATH = REPO_ROOT / ".codewith/CODEWITH.md"
WORKFLOW_PATH = REPO_ROOT / ".github/workflows/blacksmith-testbox.yml"


class BlacksmithTestboxSkillContractTest(unittest.TestCase):
    def test_canonical_skill_exists_with_portable_frontmatter(self) -> None:
        self.assertTrue(SKILL_PATH.is_file(), f"missing canonical skill: {SKILL_PATH}")
        raw = SKILL_PATH.read_text(encoding="utf-8")
        match = re.match(r"^---\n(.*?)\n---\n", raw, re.DOTALL)
        self.assertIsNotNone(match, "SKILL.md must start with YAML frontmatter")
        assert match is not None
        keys = [
            line.split(":", 1)[0].strip()
            for line in match.group(1).splitlines()
            if line.strip()
        ]
        self.assertEqual(keys, ["name", "description"])
        self.assertIn("name: blacksmith-testbox", match.group(1))

    def test_repo_policy_routes_rust_work_to_testbox_skill(self) -> None:
        policy = POLICY_PATH.read_text(encoding="utf-8")
        remote_build_policy = policy.split(
            "### Builds and tests run on Blacksmith Testbox", 1
        )[1].split("In the codex-rs folder", 1)[0]
        self.assertIn("via the `blacksmith-testbox` skill", policy)
        self.assertNotIn("via the `remote-sandbox-build` skill", policy)
        self.assertIn("Testbox is not a generic sandbox backend", policy)
        self.assertNotIn("remote sandbox instead", remote_build_policy)
        self.assertNotIn("warm sandbox", remote_build_policy)

    def test_skill_uses_current_codewith_workflow_and_two_supported_lanes(self) -> None:
        skill = SKILL_PATH.read_text(encoding="utf-8")
        required_markers = (
            ".github/workflows/blacksmith-testbox.yml",
            "gh workflow run blacksmith-testbox.yml",
            "--repo hasna/codewith",
            "--ref <branch>",
            "blacksmith testbox warmup",
            "--job light-checks-testbox",
            "blacksmith testbox run --id <testbox-id>",
            "blacksmith testbox stop --id <testbox-id>",
        )
        for marker in required_markers:
            with self.subTest(marker=marker):
                self.assertIn(marker, skill)

    def test_skill_rejects_generic_sandbox_and_fargate_substitution(self) -> None:
        skill = SKILL_PATH.read_text(encoding="utf-8")
        normalized = re.sub(r"\s+", " ", skill)
        for marker in (
            "remote-sandbox-build.mjs",
            "Blacksmith Sandbox",
            "AWS Fargate",
            "E2B",
            "Daytona",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, normalized)
        self.assertIn("must not be substituted for Blacksmith Testbox", normalized)

    def test_workflow_contract_still_exposes_testbox_actions_and_build_gate(self) -> None:
        workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        required_markers = (
            "workflow_dispatch:",
            "build_command:",
            "useblacksmith/begin-testbox@",
            "BUILD_COMMAND: ${{ inputs.build_command }}",
            'bash -lc "$BUILD_COMMAND"',
            "useblacksmith/run-testbox@",
        )
        for marker in required_markers:
            with self.subTest(marker=marker):
                self.assertIn(marker, workflow)
        self.assertLess(
            workflow.index("useblacksmith/begin-testbox@"),
            workflow.index("useblacksmith/run-testbox@"),
        )


if __name__ == "__main__":
    unittest.main()
