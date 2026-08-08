#!/usr/bin/env python3

from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import stage_npm_packages


class SelectTargetArtifactsTest(unittest.TestCase):
    target = "aarch64-apple-darwin"

    @patch.object(stage_npm_packages, "BINARY_TARGETS", (target,))
    @patch.object(stage_npm_packages, "list_workflow_artifacts")
    def test_codex_package_rejects_unsigned_only_artifact(
        self, list_workflow_artifacts
    ) -> None:
        list_workflow_artifacts.return_value = [
            stage_npm_packages.WorkflowArtifact(
                name=f"{self.target}-unsigned",
                size_in_bytes=1,
            )
        ]

        with self.assertRaisesRegex(
            FileNotFoundError,
            "codex-package requires signed workflow artifact",
        ):
            stage_npm_packages.select_target_artifacts(
                "123",
                [stage_npm_packages.CODEX_PACKAGE_COMPONENT],
            )

    @patch.object(stage_npm_packages, "BINARY_TARGETS", (target,))
    @patch.object(stage_npm_packages, "list_workflow_artifacts")
    def test_codex_package_accepts_signed_artifact(
        self, list_workflow_artifacts
    ) -> None:
        signed = stage_npm_packages.WorkflowArtifact(
            name=self.target,
            size_in_bytes=1,
        )
        list_workflow_artifacts.return_value = [signed]

        self.assertEqual(
            stage_npm_packages.select_target_artifacts(
                "123",
                [stage_npm_packages.CODEX_PACKAGE_COMPONENT],
            ),
            [signed],
        )

    @patch.object(stage_npm_packages, "BINARY_TARGETS", (target,))
    @patch.object(stage_npm_packages, "list_workflow_artifacts")
    def test_binary_component_accepts_unsigned_artifact(
        self, list_workflow_artifacts
    ) -> None:
        unsigned = stage_npm_packages.WorkflowArtifact(
            name=f"{self.target}-unsigned",
            size_in_bytes=1,
        )
        list_workflow_artifacts.return_value = [unsigned]

        self.assertEqual(
            stage_npm_packages.select_target_artifacts(
                "123",
                ["codex-responses-api-proxy"],
            ),
            [unsigned],
        )


if __name__ == "__main__":
    unittest.main()
