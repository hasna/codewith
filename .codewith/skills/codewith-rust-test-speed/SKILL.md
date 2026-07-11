---
name: codewith-rust-test-speed
description: Optimize slow Rust test workflows in Codewith codex-rs. Use when tests feel slow, compile/link dominates execution, app-server integration tests are filtered but still build huge binaries, or when deciding local/CI test strategy, target dirs, nextest filters, or Codewith skill guidance for test speed.
---

# Codewith Rust Test Speed

## Goal

Reduce feedback-loop time without weakening validation. Prefer changing the remote workflow and test/build shape over waiting longer.

Heavy Rust/Bazel validation is remote-first for Codewith agents. Use the repository-local `remote-build` skill for compile, test, clippy, Bazel, and release validation. Local heavy runs are only for explicit human-approved one-offs or when `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1` is deliberately set.

## Diagnose First

- Separate remote queue/setup time, compile/link time, and test execution time in GitHub Actions/BuildBuddy output.
- Use remote workflow logs and BuildBuddy invocation data to identify the critical path when compile time dominates.
- Check selected Bazel labels or test binaries to see whether a filter is still selecting a large integration target.
- Persistent target directories are local-only tuning and require the local heavy-build approval gate.
- Check the Rust toolchain with `rustc -Vv` before adding linker workarounds. Rust 1.90+ uses `rust-lld` by default on `x86_64-unknown-linux-gnu`, so custom linker config may be unnecessary on current Linux toolchains.

## Fast Path

```bash
just remote-bazel-validation --mode auth-smoke --ref <ref>
just remote-bazel-validation --mode test-focused --targets '<labels>' --ref <ref>
just remote-bazel-validation --mode clippy --ref <ref>
```

Start with `auth-smoke`, then run the narrowest focused remote check that proves the change. Use local `just test-fast`, `just test`, or `just check-fast` only after explicit approval or with `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1`.

## Integration Test Shape

Name filters can skip execution while still requiring Cargo to compile and link the selected test binary. If the selected binary is a large aggregate integration suite, split hot areas into their own top-level integration target:

```rust
// tests/my_area.rs
#[path = "suite/v2/my_area.rs"]
mod my_area;
```

Then remove that module from the aggregate `tests/suite/.../mod.rs` if duplicate execution would be wasteful. Run it with:

```bash
just remote-bazel-validation --mode test-focused --targets '<labels-for-my-area>' --ref <ref>
```

Keep shared helpers under subdirectories such as `tests/common/mod.rs`; top-level files under `tests/` become separate integration crates.

For new feature work, create the standalone integration binary before the suite gets slow. Keep the aggregate `tests/all.rs` as the broad compatibility run, but do not make it the only way to exercise a frequently edited API area.

## CI And Release Gates

- Use remote `test-focused`, `clippy`, and `argument-comment-lint` as the agent inner loop for heavy validation.
- Use the full workspace gate only when explicitly requested or when shared/common changes justify it, and prefer running it on remote GitHub Actions/BuildBuddy.
- If CI execution time, not compile time, becomes the bottleneck, consider nextest archive and partition support so tests can be built once and sharded across runners.

## What To Avoid

- Do not switch to `cargo test` directly.
- Do not use local target-directory tuning unless a local heavy run was explicitly approved.
- Do not add timeouts or larger runners before checking whether the test binary boundaries are wrong.
- Do not commit `rustc-wrapper`, linker, or machine-specific Cargo config unless the repo declares the tool for every developer and CI path.
- Do not hide slow local runs behind aliases that still build the same oversized test binary.

## Optional Machine-Level Caches

If the environment already provides `sccache`, using `RUSTC_WRAPPER=sccache` or a user-level Cargo config can help approved local one-offs. Treat it as local machine setup, not as a repo requirement, unless CI and developer bootstrap install it consistently. Cache-only acceleration does not satisfy the no-local-build policy.
