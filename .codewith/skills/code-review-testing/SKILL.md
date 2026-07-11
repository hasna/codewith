---
name: code-review-testing
description: Review Codewith changes for appropriate test coverage and test shape. Use when a PR changes agent logic, TUI behavior, core workflows, or Rust implementation details and needs integration-test, snapshot-test, or unit-test guidance before merge.
---

For agent changes prefer integration tests over unit tests. Core integration tests are under `codex-rs/core/tests/suite` and use `codex-rs/core/tests/common/test_codex.rs` helpers to set up a test instance of Codewith.

Features that change the agent logic MUST add an integration test:
- Provide a list of major logic changes and user-facing behaviors that need to be tested.

If unit tests are needed, put them in a dedicated test file (*_tests.rs).
Avoid test-only functions in the main implementation.

Check whether there are existing helpers to make tests more streamlined and readable.
