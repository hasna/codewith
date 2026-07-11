# Workflow Strategy

The workflows in this directory are split so that pull requests get fast, review-friendly signal while `main` still gets the full cross-platform verification pass.

## Pull Requests

- `bazel.yml` is the main pre-merge verification path for Rust code.
  It runs Bazel `test` and Bazel `clippy` on the supported Bazel targets,
  including the generated Rust test binaries needed to lint inline `#[cfg(test)]`
  code.
- `rust-ci.yml` keeps the Cargo-native PR checks intentionally small:
  - `cargo fmt --check`
  - `cargo shear`
  - `argument-comment-lint` on Linux and Windows
  - `tools/argument-comment-lint` package tests when the lint or its workflow wiring changes
- `codewith-cli.yml` builds the local-friendly `codewith` CLI artifact on
  Linux for pull requests, manual runs, and the `feat/loop-scheduled-tasks`
  feature branch so this fork can be installed without replacing an existing
  `codex` binary. The artifact uses a `codewith` wrapper that defaults to
  `~/.codewith` for state so Codewith sessions do not mutate legacy Codex
  profiles.

## Post-Merge On `main`

- `bazel.yml` also runs on pushes to `main`.
  This re-verifies the merged Bazel path and helps keep the BuildBuddy caches warm.
- `rust-ci-full.yml` is the full Cargo-native verification workflow.
  It keeps the heavier checks off the PR path while still validating them after merge:
  - the full Cargo `clippy` matrix
  - the full Cargo `nextest` matrix via per-platform archive-backed shards
  - Windows ARM64 nextest archives cross-compiled on Windows x64, then replayed on native Windows ARM64 shards
  - release-profile Cargo builds
  - `argument-comment-lint` on Linux and Windows
  - Linux remote-env tests

## Manual Remote Bazel

- `remote-rust-bazel.yml` is the agent-friendly manual offload path for
  targeted Bazel validation. It requires the repository secret
  `BUILDBUDDY_API_KEY`, selects the generic BuildBuddy RBE config through
  `.github/scripts/run_bazel_with_buildbuddy.py`, and fails closed when the
  secret is missing instead of falling back to local compilation.
- Local non-CI `just` recipes for heavy Rust/Bazel validation refuse by default
  and point agents at `just remote-bazel-validation`. GitHub Actions and other
  CI runners may still use those recipes remotely. Set
  `CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1` only for an intentional local run.
- Start with `auth-smoke` before broader checks. It builds only
  `//tools/remote-build-smoke:remote_execution_smoke`, which is intended to
  validate the GitHub Actions, BuildBuddy auth, and remote-execution path with
  minimal cache transfer.
- BuildBuddy's 100 GB free cache-transfer limit is not a per-build artifact
  size. Targeted remote Bazel checks should be far below it, while repeated full
  matrix and V8 artifact runs can consume much more over time.

### Self-Hosted AWS Options

Self-hosting is viable, but it should be a second step for Codewith. Start with
GitHub Actions plus BuildBuddy Cloud/free tier so the repo can validate auth,
target sizing, cache-transfer volume, and RBE compatibility before owning
Kubernetes, TLS, auth, executor images, worker pools, and cache lifecycle.

- AWS-native job offload: CodeBuild-hosted GitHub Actions runners are the
  closest AWS-managed option for running whole workflow jobs remotely in AWS.
  They are a good fit when the goal is "do not build on station02" while keeping
  GitHub Actions as the control plane. CodeBuild can use EC2 or Lambda compute,
  reserved capacity fleets for warm dedicated instances, and S3 or local caching
  for build-environment reuse. This is job-level remote execution, not Bazel
  action-level RBE: it does not provide BuildBuddy invocation UI, Bazel CAS/AC
  semantics, or remote executor scheduling for individual Bazel actions.
  Sources: https://docs.aws.amazon.com/codebuild/latest/userguide/action-runner.html,
  https://docs.aws.amazon.com/codebuild/latest/userguide/action-runner-questions.html,
  https://docs.aws.amazon.com/codebuild/latest/userguide/fleets.html,
  https://docs.aws.amazon.com/codebuild/latest/userguide/build-caching.html
- BuildBuddy OSS on EKS via Helm is the lowest-friction AWS self-hosted
  BuildBuddy option. It gives the Build Event Service/results UI and remote
  cache; the OSS Helm chart documents Kubernetes deployment and exposes HTTP and
  gRPC endpoints. Treat managed/on-prem RBE, Remote Bazel, remote runners, and
  Workflows as BuildBuddy Cloud/Enterprise capabilities unless a specific
  licensed on-prem deployment is approved.
  Sources: https://github.com/buildbuddy-io/buildbuddy,
  https://www.buildbuddy.io/docs/on-prem/,
  https://buildbuddy-io.github.io/buildbuddy-helm/charts/buildbuddy/,
  https://www.buildbuddy.io/docs/remote-runner-introduction/,
  https://www.buildbuddy.io/docs/workflows-setup/
- `bazel-remote` on ECS or EKS with an S3 backend is the simplest AWS
  cache-only path. It supports HTTP and gRPC remote cache APIs plus S3 storage,
  but it does not offload execution, so cache misses still build on the GitHub
  runner or local machine.
  Source: https://github.com/buchgr/bazel-remote
- Buildbarn or NativeLink are the OSS full-RBE candidates when Codewith needs
  owned execution on AWS. Both require materially more operations work:
  scheduler/storage/worker services, remote toolchain compatibility, container
  images, autoscaling, observability, and security boundaries. Bazel lists
  Buildbarn and NativeLink as self-service remote execution options.
  Sources: https://bazel.build/community/remote-execution-services,
  https://github.com/buildbarn/bb-deployments,
  https://github.com/TraceMachina/nativelink

Practical recommendation: use CodeBuild-hosted GitHub Actions only if Codewith
needs AWS-resident whole-job runners for compliance, networking, or larger
GitHub runner shapes. Use BuildBuddy Cloud now for Bazel-native cache/RBE and
results UI. Revisit BuildBuddy OSS, bazel-remote, Buildbarn, or NativeLink after
the `auth-smoke` and focused remote checks show actual cache-transfer volume and
which workloads need cache-only versus full RBE.

## Rule Of Thumb

- If a build/test/clippy check can be expressed in Bazel, prefer putting the PR-time version in `bazel.yml`.
- Keep `rust-ci.yml` fast enough that it usually does not dominate PR latency.
- Reserve `rust-ci-full.yml` for heavyweight Cargo-native coverage that Bazel does not replace yet.
