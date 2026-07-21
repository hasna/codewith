#!/usr/bin/env python3
"""Tiered Codewith validation harness (T0-T3).

Runs the *cheapest sufficient* validation for a PR-drain lane on a persistent
per-lane target directory, so warm rebuilds only recompile changed crates
instead of the whole `codex-rs` workspace.

Tiers (see `.codewith/CODEWITH.md` -> "Tiered validation"):

  fmt   (T0)  rustfmt --check                       seconds, format-only lanes
  check (T1)  cargo check --tests, changed crates   compile boundary, no exec
  test  (T2)  cargo nextest run, changed crates     scoped behavior tests
  full  (T3)  cargo nextest run, whole workspace    shared/common/core/protocol

The `check`/`test` tiers auto-scope to the crates that own the files changed
versus a base ref (default `origin/main`). Scoping is what avoids compiling and
linking the oversized aggregate test binaries of unrelated crates. When a change
touches a workspace-root manifest/config (Cargo.toml, Cargo.lock, nextest.toml,
.cargo/config.toml, rust-toolchain.toml, deny/clippy/rustfmt.toml) or a
codex-rs file that cannot be attributed to a crate, the tier automatically
escalates to the whole workspace so nothing is silently under-validated.

`--rdeps` additionally pulls in every workspace crate that (transitively)
depends on a changed crate, computed offline from `cargo metadata`. Prefer it
for shared-crate `test` lanes; `full` already covers the whole graph.

This script is dependency-free (stdlib only) and cwd-independent: it locates the
repo root via git and runs cargo from `codex-rs`.
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path

# Files whose change invalidates crate-level scoping: a build-graph-wide edit
# means every crate may be affected, so we fall back to the whole workspace.
WORKSPACE_ROOT_FILES = {
    "Cargo.toml",
    "Cargo.lock",
    ".config/nextest.toml",
    ".cargo/config.toml",
    "rust-toolchain.toml",
    "deny.toml",
    "clippy.toml",
    "rustfmt.toml",
}

RUST_MIN_STACK = "8388608"  # 8 MiB, matches the justfile default.


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, text=True, capture_output=True, **kwargs)


def repo_root() -> Path:
    out = run(["git", "rev-parse", "--show-toplevel"])
    if out.returncode != 0:
        sys.exit(f"validate: not inside a git repo: {out.stderr.strip()}")
    return Path(out.stdout.strip())


def resolve_base(base: str | None) -> str:
    """Pick a usable base ref, preferring the caller's choice."""
    candidates = [base] if base else ["origin/main", "main", "HEAD~1"]
    for ref in candidates:
        if (
            ref
            and run(["git", "rev-parse", "--verify", "--quiet", ref]).returncode == 0
        ):
            return ref
    # Last resort: empty tree so "everything" is considered changed -> full.
    return ""


def changed_files(base: str, root: Path) -> tuple[list[str], bool]:
    """Return (repo-relative changed paths, base_missing).

    Unions the PR diff (base...HEAD), the working tree diff, and untracked
    files so both fleet lanes (committed) and local iteration are covered.
    """
    files: set[str] = set()
    base_missing = base == ""

    if base:
        mb = run(["git", "merge-base", base, "HEAD"])
        diff_base = mb.stdout.strip() if mb.returncode == 0 else base
        committed = run(
            ["git", "diff", "--name-only", "--no-renames", diff_base, "HEAD"]
        )
        if committed.returncode == 0:
            files.update(f for f in committed.stdout.splitlines() if f)

    working = run(["git", "diff", "--name-only", "--no-renames", "HEAD"])
    if working.returncode == 0:
        files.update(f for f in working.stdout.splitlines() if f)

    untracked = run(["git", "ls-files", "--others", "--exclude-standard"])
    if untracked.returncode == 0:
        files.update(f for f in untracked.stdout.splitlines() if f)

    return sorted(files), base_missing


def package_name(cargo_toml: Path) -> str | None:
    try:
        text = cargo_toml.read_text(encoding="utf-8")
    except OSError:
        return None
    in_package = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_package = stripped == "[package]"
            continue
        if in_package:
            m = re.match(r'name\s*=\s*"([^"]+)"', stripped)
            if m:
                return m.group(1)
    return None


def owning_crate(rel_path: str, root: Path) -> str | None:
    """Nearest ancestor Cargo.toml with a [package] name, bounded to codex-rs."""
    codex_rs = root / "codex-rs"
    p = (root / rel_path).parent
    while True:
        try:
            p.relative_to(codex_rs)
        except ValueError:
            return None
        cargo = p / "Cargo.toml"
        if cargo.is_file():
            name = package_name(cargo)
            if name:
                return name
        if p == codex_rs:
            return None
        p = p.parent


def classify(files: list[str], root: Path) -> tuple[set[str], bool, list[str]]:
    """Return (changed_crates, needs_full_workspace, reasons)."""
    crates: set[str] = set()
    needs_full = False
    reasons: list[str] = []

    for f in files:
        if not f.startswith("codex-rs/"):
            # Non-crate change (docs, justfile, .github, scripts). Does not by
            # itself force a workspace compile; the tier command still runs.
            continue
        rel = f[len("codex-rs/") :]
        if rel in WORKSPACE_ROOT_FILES:
            needs_full = True
            reasons.append(f"workspace-root file changed: {f}")
            continue
        crate = owning_crate(f, root)
        if crate:
            crates.add(crate)
        else:
            needs_full = True
            reasons.append(f"codex-rs file not attributable to a crate: {f}")

    return crates, needs_full, reasons


def workspace_rdeps(changed: set[str], root: Path) -> set[str]:
    """Every workspace crate that transitively depends on a changed crate."""
    meta = run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root / "codex-rs",
    )
    if meta.returncode != 0:
        print(
            "validate: cargo metadata failed; skipping --rdeps expansion",
            file=sys.stderr,
        )
        return set(changed)
    import json

    data = json.loads(meta.stdout)
    members = {pkg["name"] for pkg in data["packages"]}
    # dependents[x] = set of workspace crates that directly depend on x
    dependents: dict[str, set[str]] = {name: set() for name in members}
    for pkg in data["packages"]:
        for dep in pkg.get("dependencies", []):
            dep_name = dep.get("name")
            if dep_name in members:
                dependents.setdefault(dep_name, set()).add(pkg["name"])

    result: set[str] = set(changed)
    stack = list(changed)
    while stack:
        cur = stack.pop()
        for parent in dependents.get(cur, ()):  # crates depending on cur
            if parent not in result:
                result.add(parent)
                stack.append(parent)
    return result


def lane_target_dir(root: Path) -> str:
    """Stable per-lane target dir from the branch, unless one is provided."""
    branch = (
        run(["git", "rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip() or "detached"
    )
    safe = re.sub(r"[^A-Za-z0-9._-]", "-", branch)
    base = os.environ.get(
        "CODEWITH_VALIDATE_TARGET_ROOT", "/tmp/codewith-validate-targets"
    )
    return str(Path(base) / safe)


def build_command(
    tier: str, selection: list[str], extra: list[str], root: Path
) -> list[str]:
    if tier == "fmt":
        # Repo-canonical format gate (rust + python + justfile), warning-free on
        # the stable toolchain. Mirrors `just fmt-check`.
        return [sys.executable, str(root / "scripts" / "format.py"), "--check"]
    if tier == "check":
        return ["cargo", "check", "--tests", *selection, *extra]
    # test / full
    return ["cargo", "nextest", "run", "--no-fail-fast", *selection, *extra]


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="validate", description="Tiered Codewith validation (T0-T3)."
    )
    parser.add_argument("tier", choices=["fmt", "check", "test", "full"])
    parser.add_argument("--base", default=None, help="base ref (default: origin/main)")
    parser.add_argument(
        "--rdeps",
        action="store_true",
        help="also validate workspace crates that depend on changed crates",
    )
    parser.add_argument(
        "--target-dir",
        default=None,
        help="persistent CARGO_TARGET_DIR (default: per-branch under /tmp)",
    )
    parser.add_argument(
        "--print-crates",
        action="store_true",
        help="print the resolved crate selection and exit",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the command that would run and exit",
    )

    # Everything after a literal `--` is forwarded verbatim to the tier command
    # (e.g. nextest filters). Split it out before argparse so our own flags,
    # which may appear after the positional tier, are still recognized.
    argv = sys.argv[1:]
    if "--" in argv:
        cut = argv.index("--")
        head, extra = argv[:cut], argv[cut + 1 :]
    else:
        head, extra = argv, []
    args = parser.parse_args(head)

    root = repo_root()
    base = resolve_base(args.base)
    files, base_missing = changed_files(base, root)
    crates, needs_full, reasons = classify(files, root)

    force_full = args.tier == "full" or needs_full or base_missing
    if base_missing:
        reasons.append("no usable base ref; validating whole workspace")

    selection: list[str] = []
    scope_desc: str
    if args.tier == "fmt":
        scope_desc = "workspace (rustfmt --check)"
    elif force_full or not crates:
        scope_desc = "whole workspace"
        if not crates and not force_full:
            scope_desc = "whole workspace (no crate changes detected)"
    else:
        target_crates = set(crates)
        if args.rdeps:
            target_crates = workspace_rdeps(crates, root)
        for name in sorted(target_crates):
            selection += ["-p", name]
        scope_desc = (
            f"{len(target_crates)} crate(s): {', '.join(sorted(target_crates))}"
        )

    if args.print_crates:
        print(" ".join(selection))
        return 0

    cmd = build_command(args.tier, selection, extra, root)

    target_dir = args.target_dir or os.environ.get("CARGO_TARGET_DIR")
    if not target_dir:
        target_dir = lane_target_dir(root)

    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = target_dir
    env.setdefault("RUST_MIN_STACK", RUST_MIN_STACK)

    print(f"validate: tier={args.tier} base={base or '(none)'}")
    print(f"validate: changed files={len(files)} scope={scope_desc}")
    if reasons:
        for r in reasons:
            print(f"validate: escalation: {r}")
    print(f"validate: CARGO_TARGET_DIR={target_dir}")
    print(f"validate: $ {shlex.join(cmd)}")

    if args.dry_run:
        return 0

    start = time.monotonic()
    proc = subprocess.run(cmd, cwd=root / "codex-rs", env=env)
    elapsed = time.monotonic() - start
    print(f"validate: {args.tier} finished in {elapsed:.1f}s (exit {proc.returncode})")
    return proc.returncode


if __name__ == "__main__":
    raise SystemExit(main())
