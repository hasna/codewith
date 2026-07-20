---
name: codewith-rust-test-speed
description: Optimize slow Rust test workflows in Codewith codex-rs. Use when tests feel slow, compile/link dominates execution, app-server integration tests are filtered but still build huge binaries, or when deciding local/CI test strategy, target dirs, nextest filters, or Codewith skill guidance for test speed.
---

# Codewith Rust Test Speed

## Goal

Reduce feedback-loop time without weakening validation. Prefer changing the test/build shape over waiting longer.

## Diagnose First

- Separate compile/link time from test execution time in nextest output.
- Use `just build-timings -p <crate>` when compile time dominates and you need the critical path.
- Use `just test-binaries -p <crate>` to see which test binaries exist and whether a filter is still selecting a large integration target.
- Keep repeated work on a persistent target directory, for example `just test-fast-target /tmp/codewith-<scope>-target -p <crate>`.
- Check the Rust toolchain with `rustc -Vv` before adding linker workarounds. Rust 1.90+ uses `rust-lld` by default on `x86_64-unknown-linux-gnu`, so custom linker config may be unnecessary on current Linux toolchains.

## Fast Path

```bash
cd codex-rs
just test-fast-target /tmp/codewith-<scope>-target -p <crate>
just test-fast -p <crate> --test <integration-binary>
just check-fast -p <crate>
```

Use `just test-fast` during iteration. Use `just test` for the final package/workspace gate when the benchmark smoke check should run.

## Integration Test Shape

Name filters can skip execution while still requiring Cargo to compile and link the selected test binary. If the selected binary is a large aggregate integration suite, split hot areas into their own top-level integration target:

```rust
// tests/my_area.rs
#[path = "suite/v2/my_area.rs"]
mod my_area;
```

Then remove that module from the aggregate `tests/suite/.../mod.rs` if duplicate execution would be wasteful. Run it with:

```bash
just test-fast -p <crate> --test my_area
```

Keep shared helpers under subdirectories such as `tests/common/mod.rs`; top-level files under `tests/` become separate integration crates.

For new feature work, create the standalone integration binary before the suite gets slow. Keep the aggregate `tests/all.rs` as the broad compatibility run, but do not make it the only way to exercise a frequently edited API area.

## Tiered Validation (PR-drain lanes)

For PR-drain and multi-lane validation, use `just validate <tier>` instead of
hand-picking crates. It runs the cheapest sufficient tier on a persistent
per-lane target dir and auto-scopes to the crates that own the changed files:

```bash
just validate fmt                 # T0 format gate
just validate check               # T1 compile boundary, changed crates only
just validate test                # T2 scoped nextest, changed crates only
just validate test --rdeps        # T2 + workspace crates that depend on them
just validate full                # T3 whole-workspace gate
just changed-crates [--rdeps]     # preview the resolved -p selection
```

Scoping to changed crates is the main wall-time lever: a one-crate change stops
paying to compile and link every other crate's aggregate test binary. Scoping
auto-escalates to the whole workspace when a workspace-root manifest/config
changes, so nothing is silently under-validated. Each lane gets its own
`CARGO_TARGET_DIR` (override with `--target-dir` or `CARGO_TARGET_DIR`) so
parallel lanes stay warm and never contend on one target lock.

## CI And Release Gates

- Use `just test-fast` and `just check-fast` as the local inner loop.
- Use `just test -p <crate>` for the final package gate when benchmark smoke coverage matters.
- Use the full `just test` workspace gate only when explicitly requested or when shared/common changes justify it.
- If CI execution time, not compile time, becomes the bottleneck, consider nextest archive and partition support so tests can be built once and sharded across runners.

## What To Avoid

- Do not switch to `cargo test` directly.
- Do not use fresh target directories for every iteration unless you are intentionally measuring cold builds.
- Do not add timeouts or larger runners before checking whether the test binary boundaries are wrong.
- Do not commit `rustc-wrapper`, linker, or machine-specific Cargo config unless the repo declares the tool for every developer and CI path.
- Do not hide slow local runs behind aliases that still build the same oversized test binary.

## Optional Machine-Level Caches

If the environment already provides `sccache`, using `RUSTC_WRAPPER=sccache` or a user-level Cargo config can help cold and cross-branch rebuilds. Treat it as local machine setup, not as a repo requirement, unless CI and developer bootstrap install it consistently. Note that sccache has Rust-specific caveats, including reduced benefit for crates that invoke the system linker and cases involving proc macros or filesystem reads.
