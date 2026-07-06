import unittest

import verify_tui_core_boundary as boundary


def source_hits(text: str) -> list[tuple[int, str]]:
    code = boundary.strip_comments_and_strings(text)
    return [
        (line_number, line)
        for line_number, line in enumerate(code.splitlines(), start=1)
        if any(pattern.search(line) for pattern in boundary.FORBIDDEN_SOURCE_PATTERNS)
    ]


class TuiCoreBoundaryTest(unittest.TestCase):
    def test_ignores_comments_and_string_literals_but_preserves_line_numbers(
        self,
    ) -> None:
        text = """
// use codex_core::config::Config;
/* extern crate codex_core; /* nested codex_core::x */ */
let normal = "codex_core::inside_string";
let byte = b"codex_core::inside_byte_string";
let raw = r###"use codex_core::raw"###;
let byte_raw = br#"codex_core::byte_raw"#;
let c_string = c"codex_core::c_string";
let c_raw = cr#"codex_core::c_raw"#;
let ok_lifetime: &'static str = "x";
use codex_core::config::Config;
"""
        stripped = boundary.strip_comments_and_strings(text)

        self.assertEqual(len(text.splitlines()), len(stripped.splitlines()))
        self.assertEqual(
            [(11, "use codex_core::config::Config;")],
            source_hits(text),
        )

    def test_catches_direct_codex_core_path_with_whitespace(self) -> None:
        self.assertEqual(
            [(1, "let _ = codex_core ::config::Config::default();")],
            source_hits("let _ = codex_core ::config::Config::default();"),
        )

    def test_keeps_lifetimes_as_code(self) -> None:
        self.assertEqual(
            [], source_hits("fn f<'a>(value: &'a str) -> &'a str { value }")
        )


if __name__ == "__main__":
    unittest.main()
