---
name: codewith-release-publish
description: Publish Codewith npm releases for the hasna/codewith fork. Use when Codewith needs to bump or verify release versions, package @hasna/codewith, publish to npm, update a local install, smoke-test the installed codewith command, or align rust-v* tags with the published commit.
---

# Codewith Release Publish

## Overview

Use this skill for direct npm releases of Codewith. Codewith is the app and `@hasna/codewith` is the npm package; do not describe the product as `codex-cli` in user-facing release work.

Always inspect current state first. Treat `origin` as `https://github.com/hasna/codewith.git`, but verify with `git remote -v`.

## Release Flow

1. Read `CODEWITH.md`, `codex-cli/package.json`, and the current Git state.
2. Confirm the intended version is not already published:

```bash
npm view @hasna/codewith version dist-tags --json
npm view @hasna/codewith@<version> version --json || true
```

3. Do not publish uncommitted app changes. Commit and push the intended source changes first, using `$codewith-git-ship` when needed.
4. Build a fresh release binary from the commit being published:

```bash
cd codex-rs
cargo build --release --bin codewith
./target/release/codewith --version
```

5. Pack using the current repo packaging scripts when they match the release target. If the registry still uses the root package with a local Linux arm64 vendor payload, mirror the latest published tarball layout exactly: `bin/codex.js`, `vendor/aarch64-unknown-linux-musl/bin/codewith`, `vendor/aarch64-unknown-linux-musl/codewith-path/rg`, and any required compliance files. Use the newly built binary, never a stale globally installed one.
6. Install the tarball into a temporary prefix and smoke-test before publishing:

```bash
npm install -g --prefix /tmp/codewith-release-test <tarball>
CODEWITH_HOME=$HOME/.codewith-smoke-<version> /tmp/codewith-release-test/bin/codewith --version
CODEWITH_HOME=$HOME/.codewith-smoke-<version> /tmp/codewith-release-test/bin/codewith --help
```

7. Publish only after the tarball smoke test passes:

```bash
npm publish <tarball> --access public
npm view @hasna/codewith version dist-tags --json
```

8. Install the published package locally and verify the active command:

```bash
npm install -g @hasna/codewith@<version>
which -a codewith
CODEWITH_HOME=$HOME/.codewith-smoke-<version> codewith --version
```

If `which -a codewith` resolves to Bun before npm, update Bun's global package too:

```bash
bun install -g @hasna/codewith@<version>
CODEWITH_HOME=$HOME/.codewith-smoke-<version> codewith --version
```

9. Align the release tag with the commit that produced the published package:

```bash
git tag -f -a rust-v<version> -m "Codewith <version>" HEAD
git push origin refs/tags/rust-v<version> --force
git ls-remote --tags origin rust-v<version> "rust-v<version>^{}"
```

## Verification

Before reporting success, verify:

- npm `latest` is the intended version.
- `codewith --version` from the active PATH reports the intended version.
- The release tag dereferences to the published commit.
- `git status --short` is clean in the release worktree.
- Any full-suite failures are explained with exact host/tooling causes and focused changed-path tests are listed.
