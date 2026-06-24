# Contributing to Codewith

Thanks for your interest in contributing to Codewith.

Codewith is a Hasna-maintained fork of OpenAI Codex. Keep changes focused,
preserve Codewith-specific behavior, and avoid including private credentials,
local state, logs, databases, or environment files in commits or release
artifacts.

## Development Setup

```bash
git clone https://github.com/hasna/codewith.git
cd codewith
pnpm install --frozen-lockfile
```

Most Rust work lives under `codex-rs/`. Follow the repository instructions in
`CODEWITH.md` and `.codewith/CODEWITH.md` before editing.

## Validation

Use the smallest relevant validation for the change:

```bash
cd codex-rs
just fmt
just test-fast -p <changed-crate>
```

For package and release work, use the staging and package validation scripts in
`codex-cli/scripts/` and preserve the Apache-2.0 license, NOTICE,
MODIFICATIONS, third-party notices, and bundled license files.

## Making Changes

1. Create a focused branch for the change.
2. Preserve unrelated user or worker changes; do not reset or overwrite dirty
   paths you do not own.
3. Add or update focused tests when behavior changes.
4. Keep release notes accurate when user-visible behavior or package contents
   change.
5. Open a pull request with validation evidence.

## Reporting Issues

Use [GitHub Issues](https://github.com/hasna/codewith/issues). Include the
Codewith version (`codewith --version`), operating system, reproduction steps,
and sanitized logs or command output. Do not include API keys, auth tokens,
private rollout files, local databases, or other secrets.
