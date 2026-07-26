# CLI Runtime Packaging for Python SDK

The Python source preview currently depends on the upstream
`openai-codex-cli-bin` version pinned in `sdk/python/pyproject.toml`. That is an
upstream Codex runtime, not a Codewith runtime, and default startup identifies
itself as upstream Codex.

`hasna-codewith-sdk` is not published on PyPI, and the Python preview is not the
supported integration path. For supported integrations today, use the published
[TypeScript SDK](../typescript/README.md). To experiment with Codewith through
the Python source, pass an explicit local Codewith executable through
`CodexConfig.codex_bin`.

This package template is staged during runtime release work so a CLI version can
be placed in a platform-specific wheel without checking binaries into the repo.

`openai-codex-cli-bin` is intentionally wheel-only. Do not build or publish an
sdist for this package.
