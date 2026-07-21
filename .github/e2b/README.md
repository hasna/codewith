# Codewith PR Drain E2B Template

This directory contains the draft E2B template for fast remote validation of
Codewith PR-drain work. Keep it here instead of the repository root so E2B does
not auto-discover it during unrelated local commands.

## Resources

Primary target:

```bash
e2b template build-v2 codewith-pr-drain \
  --path .github/e2b \
  --dockerfile codewith-pr-drain.Dockerfile \
  --cpu-count 8 \
  --memory-mb 16384
```

Fallback if the team limit rejects 16 GB:

```bash
e2b template build-v2 codewith-pr-drain-8gb \
  --path .github/e2b \
  --dockerfile codewith-pr-drain.Dockerfile \
  --cpu-count 8 \
  --memory-mb 8192
```

E2B documents CPU and memory as build inputs. Disk size is not an input knob; it
comes from the E2B team tier that builds the template.

## Secret handling

Do not store API keys in this repository. The caller must inject:

```bash
E2B_API_KEY=<secret-ref:e2b/api-key>
```

If a private fork or rate-limited GitHub operation is required, pass a GitHub
token through the remote runner's secret system and keep it out of template
source and logs.

## API build draft

The SDK/API entrypoint is `codewith-pr-drain-template.mjs`. It expects
`E2B_API_KEY` from a secret ref and accepts:

```bash
E2B_TEMPLATE_NAME=codewith-pr-drain
E2B_CPU_COUNT=8
E2B_MEMORY_MB=16384
E2B_SKIP_CACHE=0
```

Run from this directory in an environment that already has the E2B SDK
installed:

```bash
node codewith-pr-drain-template.mjs
```

The script intentionally uses the current three-argument SDK build shape:

```js
Template.build(template, templateName, { cpuCount, memoryMB, apiKey })
```

## Template contents

The Dockerfile installs:

- Rust stable with `clippy` and `rustfmt`
- Bazelisk and the `bazel` shim
- Bun
- `just`
- `ripgrep`
- Python 3, `pip`, `venv`, and `pytest`
- `cargo-nextest`
- `cargo-insta`
- native build prerequisites used by the Rust and Bazel validation paths

Tool pins are held in Dockerfile build args: `BAZELISK_VERSION`, `BUN_VERSION`,
`JUST_VERSION`, `NEXT_VERSION`, and `INSTA_VERSION`. Rust intentionally tracks
`stable` unless a future validation incident requires pinning an exact toolchain.

It also warms dependency metadata and fetch caches:

```bash
cd /opt/codewith/codex-rs
cargo fetch --locked
bazelisk fetch //codex-rs/...
just --list
```

The warmed source checkout is only a dependency/cache seed, not a full compiled
Rust or Bazel build cache. Remote validation jobs should clone or checkout the
exact PR SHA under `/workspace` and run tests there, not mutate `/opt/codewith`.
If later E2B build-time budget allows compiled cache warming, add those as
remote-only bounded steps and keep local spark01 validation static.

## Suggested sandbox validation commands

Use these inside a sandbox created from the template after checking out the
target PR into `/workspace/codewith`:

```bash
cd /workspace/codewith/codex-rs
just fmt --check
just check-fast -p codex-core
just test-fast -p codex-core
```

For Bazel-specific regressions, prefer bounded targets first:

```bash
cd /workspace/codewith
bazelisk query //codex-rs/... >/tmp/codewith-bazel-query.txt
bazelisk build --nobuild //codex-rs/...
```

Run heavier package gates only when the PR scope warrants it.
