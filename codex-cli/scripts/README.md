# npm releases

Use the staging helper in the repo root to generate npm tarballs for a release. For
example, to stage the CLI, responses proxy, and SDK packages for version `0.6.0`:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.6.0 \
  --package codex \
  --package codex-responses-api-proxy \
  --package codex-sdk
```

This downloads the required native package archive artifacts, hydrates `vendor/` for
each package, and writes tarballs to `dist/npm/`.

When `--package codex` is provided, the staging helper expands it via
`expand_packages()` using `PACKAGE_EXPANSIONS` in `build_npm_package.py`: it
builds the `@hasna/codewith` meta package plus every platform package listed in
`CODEX_PLATFORM_PACKAGES` (currently `@hasna/codewith-linux-x64`,
`@hasna/codewith-linux-arm64`, `@hasna/codewith-darwin-x64`,
`@hasna/codewith-darwin-arm64`, `@hasna/codewith-win32-x64`, and
`@hasna/codewith-win32-arm64`). Keep this list in sync with
`CODEX_PLATFORM_PACKAGES` in `codex-cli/scripts/build_npm_package.py` (the same
source of truth referenced by `.github/workflows/rust-release.yml`).

Direct `build_npm_package.py` invocations are still useful for package-specific
debugging, but native packages expect `--vendor-src` to point at a prehydrated
`vendor/` tree. Release packaging should use `scripts/stage_npm_packages.py`.
