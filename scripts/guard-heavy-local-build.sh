#!/usr/bin/env bash

set -euo pipefail

intent="${1:?usage: guard-heavy-local-build.sh <intent> [command...]}"
shift

if [[ "${GITHUB_ACTIONS:-}" == "true" || "${CI:-}" == "true" || "${CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS:-}" == "1" ]]; then
  if [[ $# -eq 0 ]]; then
    exit 0
  fi
  exec "$@"
fi

cat >&2 <<EOF
Refusing local ${intent}.

Heavy Rust/Bazel validation for local Codewith agents defaults to GitHub Actions
with BuildBuddy cache/RBE. Start with:

  just remote-bazel-validation --mode auth-smoke

Then run a focused remote check, for example:

  just remote-bazel-validation --mode test-focused --targets '//codex-rs/tui:tui-unit-tests'

CI may still run these recipes on remote runners. For an intentional one-off
local run, set CODEWITH_ALLOW_LOCAL_HEAVY_BUILDS=1.
EOF
exit 2
