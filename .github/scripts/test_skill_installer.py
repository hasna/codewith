from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALLER_SCRIPTS = (
    REPO_ROOT
    / "codex-rs"
    / "skills"
    / "src"
    / "assets"
    / "samples"
    / "skill-installer"
    / "scripts"
)
INSTALLER_PATH = INSTALLER_SCRIPTS / "install-skill-from-github.py"


def load_installer_module():
    sys.path.insert(0, str(INSTALLER_SCRIPTS))
    try:
        spec = importlib.util.spec_from_file_location(
            "testable_skill_installer", INSTALLER_PATH
        )
        if spec is None or spec.loader is None:
            raise RuntimeError(f"unable to load {INSTALLER_PATH}")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.remove(str(INSTALLER_SCRIPTS))


installer = load_installer_module()


def make_skill(repo_root: Path, relative_path: str, marker: str) -> Path:
    skill = repo_root / relative_path
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(
        f"---\nname: {skill.name}\ndescription: test skill\n---\n\n{marker}\n",
        encoding="utf-8",
    )
    (skill / "marker.txt").write_text(marker, encoding="utf-8")
    return skill


class SkillInstallerTransactionTests(unittest.TestCase):
    def test_existing_destination_is_preserved_byte_for_byte(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            source = root / "source"
            make_skill(source, "skills/existing", "replacement")
            destination = root / "home" / "skills" / "existing"
            destination.mkdir(parents=True)
            prior = destination / "prior.bin"
            prior.write_bytes(b"prior-bytes\x00\xff")
            stdout = io.StringIO()
            stderr = io.StringIO()

            with (
                mock.patch.object(installer, "_prepare_repo", return_value=str(source)),
                redirect_stdout(stdout),
                redirect_stderr(stderr),
            ):
                result = installer.main(
                    [
                        "--repo",
                        "example/private",
                        "--path",
                        "skills/existing",
                        "--dest",
                        str(root / "home" / "skills"),
                    ]
                )

            self.assertEqual(result, 1)
            self.assertEqual(prior.read_bytes(), b"prior-bytes\x00\xff")
            self.assertEqual(sorted(path.name for path in destination.iterdir()), ["prior.bin"])
            self.assertIn("Destination already exists", stderr.getvalue())

    def test_copy_failure_rolls_back_only_transaction_created_destinations(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            source = root / "source"
            make_skill(source, "skills/first", "first")
            make_skill(source, "skills/second", "second")
            destination_root = root / "home" / "skills"
            unrelated = destination_root / "unrelated"
            unrelated.mkdir(parents=True)
            sentinel = unrelated / "keep.bin"
            sentinel.write_bytes(b"keep-me")
            original_copytree = installer.shutil.copytree
            copy_count = 0

            def fail_second_copy(src: str, dest: str, *args, **kwargs):
                nonlocal copy_count
                copy_count += 1
                if copy_count == 2:
                    partial = Path(dest)
                    partial.mkdir(parents=True, exist_ok=True)
                    (partial / "partial.txt").write_text("partial", encoding="utf-8")
                    raise OSError("injected copy failure")
                return original_copytree(src, dest, *args, **kwargs)

            with (
                mock.patch.object(installer, "_prepare_repo", return_value=str(source)),
                mock.patch.object(installer.shutil, "copytree", side_effect=fail_second_copy),
                redirect_stdout(io.StringIO()),
                redirect_stderr(io.StringIO()),
            ):
                result = installer.main(
                    [
                        "--repo",
                        "example/private",
                        "--path",
                        "skills/first",
                        "skills/second",
                        "--dest",
                        str(destination_root),
                    ]
                )

            self.assertEqual(result, 1)
            self.assertFalse((destination_root / "first").exists())
            self.assertFalse((destination_root / "second").exists())
            self.assertEqual(sentinel.read_bytes(), b"keep-me")


class SkillInstallerGitFallbackTests(unittest.TestCase):
    def test_failed_branch_clone_is_removed_before_fallback_clone(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            unrelated = root / "unrelated.bin"
            unrelated.write_bytes(b"keep-me")
            calls: list[list[str]] = []
            fallback_saw_partial: bool | None = None

            def fake_run_git(args: list[str]) -> None:
                nonlocal fallback_saw_partial
                calls.append(args)
                repo_dir = root / "repo"
                if len(calls) == 1:
                    repo_dir.mkdir()
                    (repo_dir / "partial").write_text("partial", encoding="utf-8")
                    raise installer.InstallError("branch clone failed")
                if len(calls) == 2:
                    fallback_saw_partial = repo_dir.exists()
                    if repo_dir.exists():
                        for child in repo_dir.iterdir():
                            child.unlink()
                    else:
                        repo_dir.mkdir()

            with mock.patch.object(installer, "_run_git", side_effect=fake_run_git):
                installer._git_sparse_checkout(
                    "https://github.com/example/private.git",
                    "a" * 40,
                    ["skills/example"],
                    str(root),
                )

            self.assertFalse(fallback_saw_partial)
            self.assertEqual(unrelated.read_bytes(), b"keep-me")

    def test_exact_commit_fallback_fetches_the_requested_ref(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            requested_ref = "b" * 40
            calls: list[list[str]] = []

            def fake_run_git(args: list[str]) -> None:
                calls.append(args)
                if len(calls) == 1:
                    (root / "repo").mkdir()
                    raise installer.InstallError("--branch cannot name a commit")

            with mock.patch.object(installer, "_run_git", side_effect=fake_run_git):
                installer._git_sparse_checkout(
                    "https://github.com/example/private.git",
                    requested_ref,
                    ["skills/example"],
                    str(root),
                )

            self.assertIn(
                [
                    "git",
                    "-C",
                    str(root / "repo"),
                    "fetch",
                    "--depth",
                    "1",
                    "origin",
                    requested_ref,
                ],
                calls,
            )
            self.assertEqual(
                calls[-1],
                ["git", "-C", str(root / "repo"), "checkout", "--detach", "FETCH_HEAD"],
            )

    def test_failed_exact_ref_removes_only_the_transaction_repo(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            unrelated = root / "unrelated.bin"
            unrelated.write_bytes(b"keep-me")
            calls = 0

            def fake_run_git(_args: list[str]) -> None:
                nonlocal calls
                calls += 1
                repo_dir = root / "repo"
                if calls == 1:
                    repo_dir.mkdir()
                    raise installer.InstallError("branch clone failed")
                if calls == 2:
                    repo_dir.mkdir()
                if calls == 4:
                    (repo_dir / "partial-fetch").write_text("partial", encoding="utf-8")
                    raise installer.InstallError("requested ref missing")

            with (
                mock.patch.object(installer, "_run_git", side_effect=fake_run_git),
                self.assertRaisesRegex(installer.InstallError, "requested ref missing"),
            ):
                installer._git_sparse_checkout(
                    "https://github.com/example/private.git",
                    "c" * 40,
                    ["skills/example"],
                    str(root),
                )

            self.assertFalse((root / "repo").exists())
            self.assertEqual(unrelated.read_bytes(), b"keep-me")


class SkillInstallerAuthenticationTests(unittest.TestCase):
    def test_authenticated_archive_request_uses_header_without_printing_value(self) -> None:
        github_utils = sys.modules[installer.github_request.__module__]
        observed_authorization: list[str | None] = []

        class Response:
            def __enter__(self):
                return self

            def __exit__(self, *_args) -> None:
                return None

            def read(self) -> bytes:
                return b"archive-bytes"

        def fake_urlopen(request):
            observed_authorization.append(request.get_header("Authorization"))
            return Response()

        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch.dict(
                os.environ,
                {"GITHUB_TOKEN": "unit-test-auth-value", "GH_TOKEN": ""},
            ),
            mock.patch.object(github_utils.urllib.request, "urlopen", side_effect=fake_urlopen),
            redirect_stdout(stdout),
            redirect_stderr(stderr),
        ):
            payload = github_utils.github_request(
                "https://codeload.github.com/example/private/zip/ref",
                "test-agent",
            )

        self.assertEqual(payload, b"archive-bytes")
        self.assertEqual(observed_authorization, ["token unit-test-auth-value"])
        self.assertNotIn("unit-test-auth-value", stdout.getvalue())
        self.assertNotIn("unit-test-auth-value", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
