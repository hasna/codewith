---
name: remote-build
description: Use remote-first GitHub Actions and BuildBuddy/RBE validation for Codewith Rust/Bazel checks instead of running heavy builds locally on station02.
---

# Remote Build

## Policy

Codewith agents no longer run heavy Rust/Bazel builds locally by default. Heavy Rust/Bazel validation for this app must be remote-first through GitHub Actions and BuildBuddy/RBE.

Local heavy builds require explicit human approval or `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1` for a deliberate one-off. Cache-only setups such as `bazel-remote` or `sccache` do not satisfy this policy because cache misses still compile locally.

## Classify First

- Lightweight static inspection: reading files, `rg`, `git diff --check`, Markdown/frontmatter sanity checks, and small parsers that do not compile Rust or invoke Bazel.
- Heavy validation: any Rust compile/test/lint/build, Bazel query/build/test/clippy, `just test*`, `just check*`, `just fix`, `cargo build`, release binaries, and Bazel lock checks.

Use lightweight checks locally. For heavy validation, route through the remote workflow unless the human explicitly approved a local one-off.

## Remote Flow

1. Ensure the branch/ref is pushed or otherwise dispatchable by GitHub Actions.
2. Confirm `.github/workflows/remote-rust-bazel.yml` exists on the default branch before relying on `gh workflow run` for non-default refs.
3. Start with:

```bash
just remote-bazel-validation --mode auth-smoke --ref <ref>
```

4. Follow with the narrowest remote check that proves the change:

```bash
just remote-bazel-validation --mode argument-comment-lint --ref <ref>
just remote-bazel-validation --mode clippy --ref <ref>
just remote-bazel-validation --mode test-focused --targets '<labels>' --ref <ref>
```

5. Watch GitHub Actions until the run reaches a terminal status. Collect the run URL, status, commit/ref, selected mode, and targets as evidence in the task or handoff.
6. If the BuildBuddy key, workflow, or dispatch path is missing, fix or route the remote setup. Do not fall back to local compilation just to get a result.

## Fallback Choices

- Whole-job remote execution: GitHub-hosted runners or CodeBuild-hosted GitHub Actions runners.
- Bazel remote execution: BuildBuddy, EngFlow, Aspect, or NativeLink.
- Cache-only tools: `bazel-remote` and `sccache` can reduce repeated work, but they are not enough for the no-local-build policy because they do not guarantee remote execution.
