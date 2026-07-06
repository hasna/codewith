#!/usr/bin/env python3

"""Verify codex-tui does not depend on or import codex-core directly.

The source scan matches against *code only*: comments and string/char literals
are blanked before matching so a mention of `codex_core` inside a comment or a
string literal (e.g. documentation that references the upstream crate) does not
trip the boundary check. Line numbers are preserved for accurate diagnostics.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TUI_ROOT = ROOT / "codex-rs" / "tui"
TUI_MANIFEST = TUI_ROOT / "Cargo.toml"
FORBIDDEN_PACKAGE = "codex-core"
FORBIDDEN_SOURCE_PATTERNS = (
    re.compile(r"\bcodex_core\s*::"),
    re.compile(r"\buse\s+codex_core\b"),
    re.compile(r"\bextern\s+crate\s+codex_core\b"),
)

# Optional `b`/`br` or `c`/`cr` prefix, `r`, any number of `#`, then the
# opening quote.
_RAW_STRING_START = re.compile(r'(?:b|c)?r(?P<hashes>#*)"')


def main() -> int:
    failures = []
    failures.extend(manifest_failures())
    failures.extend(source_failures())

    if not failures:
        return 0

    print("codex-tui must not depend on or import codex-core directly.")
    print(
        "Use the app-server protocol/client boundary instead; temporary embedded "
        "startup gaps belong behind codex_app_server_client::legacy_core."
    )
    print()
    for failure in failures:
        print(f"- {failure}")

    return 1


def manifest_failures() -> list[str]:
    manifest = tomllib.loads(TUI_MANIFEST.read_text())
    failures = []
    for section_name, dependencies in dependency_sections(manifest):
        if FORBIDDEN_PACKAGE in dependencies:
            failures.append(
                f"{relative_path(TUI_MANIFEST)} declares `{FORBIDDEN_PACKAGE}` "
                f"in `[{section_name}]`"
            )
    return failures


def dependency_sections(manifest: dict) -> list[tuple[str, dict]]:
    sections: list[tuple[str, dict]] = []
    for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        dependencies = manifest.get(section_name)
        if isinstance(dependencies, dict):
            sections.append((section_name, dependencies))

    for target_name, target in manifest.get("target", {}).items():
        if not isinstance(target, dict):
            continue
        for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
            dependencies = target.get(section_name)
            if isinstance(dependencies, dict):
                sections.append((f"target.{target_name}.{section_name}", dependencies))

    return sections


def strip_comments_and_strings(text: str) -> str:
    """Replace comment and string/char-literal spans with spaces.

    Newlines are preserved so downstream line numbers remain accurate. This lets
    the forbidden-import patterns match real code only, never a `codex_core`
    reference that appears inside a `//`/`/* */` comment or a `"..."` literal.
    """
    out: list[str] = []
    i = 0
    n = len(text)

    def blank(chunk: str) -> None:
        out.append("".join("\n" if ch == "\n" else " " for ch in chunk))

    while i < n:
        ch = text[i]
        nxt = text[i + 1] if i + 1 < n else ""

        # Line comment: consume to end of line (newline kept as-is).
        if ch == "/" and nxt == "/":
            j = i
            while j < n and text[j] != "\n":
                j += 1
            blank(text[i:j])
            i = j
            continue

        # Block comment (Rust allows nesting).
        if ch == "/" and nxt == "*":
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if text[j] == "/" and j + 1 < n and text[j + 1] == "*":
                    depth += 1
                    j += 2
                elif text[j] == "*" and j + 1 < n and text[j + 1] == "/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(text[i:j])
            i = j
            continue

        # Raw string: (b?)r#*"..."#*  (no escape processing inside).
        raw = _RAW_STRING_START.match(text, i)
        if raw:
            terminator = '"' + raw.group("hashes")
            end = text.find(terminator, raw.end())
            j = n if end == -1 else end + len(terminator)
            blank(text[i:j])
            i = j
            continue

        # Normal / byte / C string literal: "...", b"...", or c"...".
        if ch == '"' or (ch in ("b", "c") and nxt == '"'):
            j = i + 1 if ch == '"' else i + 2  # skip opening quote and prefix.
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                j += 1
            blank(text[i:j])
            i = j
            continue

        # Char literal vs lifetime. Char: '\..' or 'x'. Lifetime: 'ident.
        if ch == "'":
            if nxt == "\\":
                j = i + 1
                while j < n:
                    if text[j] == "\\":
                        j += 2
                        continue
                    if text[j] == "'":
                        j += 1
                        break
                    j += 1
                blank(text[i:j])
                i = j
                continue
            if i + 2 < n and text[i + 2] == "'":
                blank(text[i : i + 3])
                i += 3
                continue
            # Lifetime (e.g. 'a, 'static): treat the quote as ordinary code.
            out.append(ch)
            i += 1
            continue

        out.append(ch)
        i += 1

    return "".join(out)


def source_failures() -> list[str]:
    failures = []
    for path in sorted(TUI_ROOT.glob("**/*.rs")):
        code = strip_comments_and_strings(path.read_text())
        for line_number, line in enumerate(code.splitlines(), start=1):
            if any(pattern.search(line) for pattern in FORBIDDEN_SOURCE_PATTERNS):
                failures.append(
                    f"{relative_path(path)}:{line_number} imports `codex_core`"
                )
    return failures


def relative_path(path: Path) -> str:
    return str(path.relative_to(ROOT))


if __name__ == "__main__":
    sys.exit(main())
