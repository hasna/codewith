#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: scripts/remote-bazel-validation.sh [--mode <mode>] [--targets '<labels>'] [--ref <ref>] [--repo <owner/name>] [--no-watch]

Dispatches the GitHub Actions "Remote Rust/Bazel Validation" workflow so agents
do not run heavy Bazel builds on the local workstation.

Modes:
  auth-smoke                  Build a tiny remote-execution smoke target.
  argument-comment-lint       Run Bazel-backed argument-comment-lint targets.
  clippy                      Run Bazel-backed clippy targets.
  test-focused                Run Bazel test for the supplied --targets labels.

Examples:
  just remote-bazel-validation
  just remote-bazel-validation --mode test-focused --targets '//codex-rs/tui:tui-unit-tests'
  just remote-bazel-validation --mode clippy
EOF
}

mode="auth-smoke"
targets=""
ref=""
repo="${GH_REPO:-hasna/codewith}"
watch=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      mode="${2:?--mode requires a value}"
      shift 2
      ;;
    --targets)
      targets="${2:?--targets requires a value}"
      shift 2
      ;;
    --ref)
      ref="${2:?--ref requires a value}"
      shift 2
      ;;
    --repo)
      repo="${2:?--repo requires a value}"
      shift 2
      ;;
    --no-watch)
      watch=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
done

case "$mode" in
  auth-smoke|argument-comment-lint|clippy|test-focused)
    ;;
  *)
    echo "Unsupported mode: $mode" >&2
    usage
    exit 2
    ;;
esac

if ! command -v gh >/dev/null 2>&1; then
  echo "GitHub CLI 'gh' is required to dispatch remote validation." >&2
  exit 127
fi

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if [[ -z "$ref" ]]; then
  ref="$(git branch --show-current)"
  if [[ -z "$ref" ]]; then
    ref="$(git rev-parse HEAD)"
  fi
fi

workflow="remote-rust-bazel.yml"
run_args=(
  workflow
  run
  "$workflow"
  --repo "$repo"
  --ref "$ref"
  -f "mode=$mode"
)

if [[ -n "$targets" ]]; then
  run_args+=(-f "targets=$targets")
fi

echo "Dispatching $workflow on $repo@$ref with mode=$mode."
echo "This runs on GitHub Actions with BuildBuddy/RBE; no local Bazel build is started."

gh "${run_args[@]}"

if [[ $watch -eq 1 ]]; then
  echo "Waiting for the queued run to appear before watching it..."
  sleep 5
  run_id="$(gh run list --repo "$repo" --workflow "$workflow" --branch "$ref" --limit 1 --json databaseId --jq '.[0].databaseId // empty')"
  if [[ -z "$run_id" ]]; then
    echo "Could not find the new workflow run yet. Use: gh run list --repo $repo --workflow $workflow --branch $ref" >&2
    exit 1
  fi
  gh run watch "$run_id" --repo "$repo" --exit-status
else
  echo "Watch with: gh run list --repo $repo --workflow $workflow --branch $ref"
fi
