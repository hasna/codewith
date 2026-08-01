#!/usr/bin/env python3
"""Regression tests for station02 PR-drain supervisor/guard agreement."""

from __future__ import annotations

import json
import os
import re
import shutil
import stat
import subprocess
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SCRIPT_DIR = ROOT / "scripts" / "drain"
SCRIPT_DIR = Path(os.environ.get("DRAIN_SCRIPT_DIR", DEFAULT_SCRIPT_DIR))
HEAD = "13685fca00000000000000000000000000000000"
KEY = "hasna/attachments#22"


def review_line(verdict: str, reviewer: str = "Augustus") -> str:
    return (
        f"[REVIEW] {verdict} - hasna/attachments#22 @ {HEAD[:8]} "
        f"- lens: correctness+security+gates, reviewer {reviewer} (1 of 1)"
    )


def pr_json(comments: list[dict[str, str]], *, mergeable: str = "MERGEABLE") -> str:
    return json.dumps(
        {
            "state": "OPEN",
            "isDraft": False,
            "headRefOid": HEAD,
            "baseRefName": "main",
            "baseRefOid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "mergeable": mergeable,
            "comments": comments,
        }
    )


class DrainFixture:
    def __init__(
        self,
        temp_dir: str,
        *,
        comments: list[dict[str, str]],
        state_class: str,
        attempts: int,
    ) -> None:
        self.root = Path(temp_dir)
        self.home = self.root / "home"
        self.bin = self.root / "bin"
        self.home_bun_bin = self.home / ".bun" / "bin"
        self.home_local_bin = self.home / ".local" / "bin"
        self.home.mkdir()
        self.bin.mkdir()
        self.home_bun_bin.mkdir(parents=True)
        self.home_local_bin.mkdir(parents=True)
        (self.home / "drain-noattempt").mkdir()
        self.pr_json_path = self.root / "pr.json"
        self.pr_json_path.write_text(pr_json(comments), encoding="utf-8")
        self._write_executable("gh", self._gh_script())
        self._write_executable("conversations", self._conversations_script())
        self._write_executable("lane.sh", self._lane_script(), directory=self.home)
        self._write_executable(
            "drain-requeue-authfail.sh",
            "#!/usr/bin/env bash\nexit 0\n",
            directory=self.home,
        )
        self._write_executable(
            "drain-queue-build.sh", "#!/usr/bin/env bash\nexit 0\n", directory=self.home
        )
        (self.home / "drain-queue.txt").write_text(
            "hasna/attachments\t22\t/tmp/attachments-22\n", encoding="utf-8"
        )
        (self.home / "drain-attempted.txt").write_text(f"{KEY}\n", encoding="utf-8")
        (self.home / "drain-state.tsv").write_text(
            f"{KEY}\t{state_class}\t0\t{HEAD}\t{attempts}\n", encoding="utf-8"
        )

    def _write_executable(
        self, name: str, content: str, *, directory: Path | None = None
    ) -> None:
        path = (directory or self.home_bun_bin) / name
        path.write_text(content, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        if directory is None:
            mirror = self.bin / name
            mirror.write_text(content, encoding="utf-8")
            mirror.chmod(mirror.stat().st_mode | stat.S_IXUSR)

    def _gh_script(self) -> str:
        return textwrap.dedent(
            f"""\
            #!/usr/bin/env python3
            import json
            import os
            import sys

            args = sys.argv[1:]
            with open(os.environ["DRAIN_FIXTURE_PR_JSON"], encoding="utf-8") as f:
                data = json.load(f)
            if args[:2] == ["pr", "view"]:
                if "--jq" in args:
                    print(f"{{data['state']}}\\t{{str(data['isDraft']).lower()}}\\t{{data['headRefOid']}}")
                else:
                    print(json.dumps(data))
                raise SystemExit(0)
            if args[:2] == ["repo", "view"]:
                print("false" if "--jq" in args else "{{\\"isArchived\\": false}}")
                raise SystemExit(0)
            print("unexpected gh invocation: " + " ".join(args), file=sys.stderr)
            raise SystemExit(64)
            """
        )

    def _conversations_script(self) -> str:
        return textwrap.dedent(
            """\
            #!/usr/bin/env python3
            import json
            import sys

            if sys.argv[1:3] == ["agents", "list"]:
                print(json.dumps({"agents": [{"agent": "Castor"}]}))
                raise SystemExit(0)
            print("unexpected conversations invocation: " + " ".join(sys.argv[1:]), file=sys.stderr)
            raise SystemExit(64)
            """
        )

    def _lane_script(self) -> str:
        return textwrap.dedent(
            """\
            #!/usr/bin/env bash
            printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >> "$HOME/lane-invocations.tsv"
            """
        )

    def env(self) -> dict[str, str]:
        return {
            **os.environ,
            "HOME": str(self.home),
            "PATH": f"{self.bin}:{self.home_bun_bin}:{self.home_local_bin}:{os.environ['PATH']}",
            "DRAIN_FIXTURE_PR_JSON": str(self.pr_json_path),
            "DRAIN_MAX_LANES": "1",
            "DRAIN_LAUNCH_SLEEP": "0",
            "DRAIN_LOAD_CEIL": "9999",
            "DRAIN_RECHECK_MAX": "10",
            "DRAIN_GUARD_LOG": str(self.home / "drain-merge-guard.log"),
            "DRAIN_GUARD_CACHE": str(self.home / ".drain-guard-cache"),
            "DRAIN_GUARD_ATTRIB_CUTOVER": "2026-08-01T10:40:51Z",
        }

    def run_supervisor(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(SCRIPT_DIR / "drain-supervisor.sh")],
            text=True,
            capture_output=True,
            env=self.env(),
            check=False,
        )

    def run_guard(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                "bash",
                str(SCRIPT_DIR / "drain-merge-guard.sh"),
                "hasna/attachments",
                "22",
            ],
            text=True,
            capture_output=True,
            env=self.env(),
            check=False,
        )

    def supervisor_log(self) -> str:
        path = self.home / "drain-supervisor.log"
        return path.read_text(encoding="utf-8") if path.exists() else ""

    def guard_log(self) -> str:
        path = self.home / "drain-merge-guard.log"
        return path.read_text(encoding="utf-8") if path.exists() else ""

    def lane_invocations(self) -> str:
        path = self.home / "lane-invocations.tsv"
        return path.read_text(encoding="utf-8") if path.exists() else ""


class DrainSupervisorGuardTest(unittest.TestCase):
    def assert_successful_launch_control(self, temp_dir: str) -> None:
        fixture = DrainFixture(
            temp_dir,
            comments=[
                {
                    "createdAt": "2026-08-01T12:00:00Z",
                    "body": review_line("GO", reviewer="Castor"),
                }
            ],
            state_class="go_open",
            attempts=0,
        )
        result = fixture.run_supervisor()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "hasna/attachments\t22\t/tmp/attachments-22\taccount001",
            fixture.lane_invocations(),
        )

    def test_supervisor_does_not_launch_when_guard_blocks_same_head(self) -> None:
        with TemporaryDirectory() as control_dir:
            self.assert_successful_launch_control(control_dir)

        comments = [
            {
                "createdAt": "2026-08-01T10:20:00Z",
                "body": review_line("NO_GO", reviewer="Augustus"),
            },
            {
                "createdAt": "2026-08-01T12:00:00Z",
                "body": review_line("GO", reviewer="Augustus"),
            },
        ]
        with TemporaryDirectory() as temp_dir:
            fixture = DrainFixture(
                temp_dir, comments=comments, state_class="go_open", attempts=2
            )
            supervisor = fixture.run_supervisor()
            guard = fixture.run_guard()

            self.assertEqual(supervisor.returncode, 0, supervisor.stderr)
            self.assertEqual(guard.returncode, 1, guard.stderr)
            self.assertIn(
                "shared supersession decision: unwithdrawable_nogo",
                fixture.supervisor_log(),
            )
            self.assertIn("BLOCK hasna/attachments#22", fixture.guard_log())
            self.assertEqual(
                "",
                fixture.lane_invocations(),
                "attachments#22 guard-blocked fixture must not consume a lane",
            )

    def test_go_open_cold_never_logs_attempt_over_ceiling(self) -> None:
        comments = [
            {
                "createdAt": "2026-08-01T12:00:00Z",
                "body": review_line("GO", reviewer="Castor"),
            }
        ]
        with TemporaryDirectory() as control_dir:
            fixture = DrainFixture(
                control_dir, comments=comments, state_class="go_open", attempts=2
            )
            result = fixture.run_supervisor()
            self.assertEqual(result.returncode, 0, result.stderr)
            control_log = fixture.supervisor_log()
            self.assertIn("attempt=3/3", control_log)

        with TemporaryDirectory() as temp_dir:
            fixture = DrainFixture(
                temp_dir, comments=comments, state_class="go_open", attempts=3
            )
            result = fixture.run_supervisor()

            self.assertEqual(result.returncode, 0, result.stderr)
            log = fixture.supervisor_log()
            self.assertIsNone(
                re.search(r"attempt[ =]([4-9]|[1-9][0-9]+)/3", log),
                log,
            )

    def test_external_unburned_cold_key_resets_attempt_window(self) -> None:
        comments = [
            {
                "createdAt": "2026-08-01T12:00:00Z",
                "body": review_line("GO", reviewer="Castor"),
            }
        ]
        with TemporaryDirectory() as temp_dir:
            fixture = DrainFixture(
                temp_dir, comments=comments, state_class="go_open_cold", attempts=3
            )
            (fixture.home / "drain-attempted.txt").write_text("", encoding="utf-8")

            result = fixture.run_supervisor()

            self.assertEqual(result.returncode, 0, result.stderr)
            log = fixture.supervisor_log()
            self.assertIn("attempt=1/3", log)
            self.assertIsNone(
                re.search(r"attempt[ =]([4-9]|[1-9][0-9]+)/3", log),
                log,
            )


if __name__ == "__main__":
    if not SCRIPT_DIR.is_dir():
        raise SystemExit(f"drain script dir does not exist: {SCRIPT_DIR}")
    for script in ("drain-supervisor.sh", "drain-merge-guard.sh"):
        if not shutil.which("jq"):
            raise SystemExit("jq is required for drain script regression tests")
        if not (SCRIPT_DIR / script).is_file():
            raise SystemExit(f"missing required script: {SCRIPT_DIR / script}")
    unittest.main()
