---
name: codewith-release-publish
description: Publish Codewith npm releases for the hasna/codewith fork. Use when Codewith needs to bump or verify release versions, package @hasna/codewith, publish to npm, update a local install, smoke-test the installed codewith command, or align rust-v* tags with the published commit.
---

# Codewith Release Publish

## Overview

Use this skill for GitHub Actions-backed releases of Codewith. Codewith is the app and `@hasna/codewith` is the npm package; do not describe the product as `codex-cli` in user-facing release work.

The canonical native release build and package assembly path is
`.github/workflows/rust-release.yml`. Do not compile `codex-rs` locally unless
the user explicitly approves that exception. Treat `origin` as
`https://github.com/hasna/codewith.git`, but verify it before release work.

## Release Flow

1. Read `CODEWITH.md`, `CHANGELOG.md`, `codex-rs/Cargo.toml`,
   `codex-cli/package.json`, and the current Git state. Confirm that the Cargo,
   npm package, changelog, and intended `rust-v<version>` tag all name the same
   version.
2. Confirm the intended version is not already published. Only an npm `E404`
   proves absence; a transport, auth, or registry error is unverified and must
   stop the release:

```bash
registry_stdout="$(mktemp)"
registry_stderr="$(mktemp)"

set +e
npm view @hasna/codewith@<version> version --json \
  >"${registry_stdout}" \
  2>"${registry_stderr}"
registry_status=$?
set -e

case "${registry_status}" in
  0)
    echo "@hasna/codewith@<version> is already published"
    exit 1
    ;;
  *)
    if ! grep -q "E404" "${registry_stderr}"; then
      echo "Registry absence is unverified"
      exit "${registry_status}"
    fi
    ;;
esac

npm view @hasna/codewith version dist-tags --json
```

3. Preserve the release gates in `CODEWITH.md`. An urgent release still needs
   its targeted regressions, remote package build, tarball smoke, published
   install smoke, and tag-to-commit proof. The full Rust/Bazel matrix remains
   required when explicitly requested or when an applicable gate reports a real
   failure; the fast gate does not waive it.
4. Do not publish uncommitted app changes. Run the mandated staged and release
   range secret scans, then commit and push the intended source changes first,
   using `$codewith-git-ship` when needed. Record:
   - the exact release commit;
   - the current active `codewith` path and version; and
   - the previous known-good package version and reinstall command for
     rollback.
5. Build the candidate remotely from the pushed release commit. Dispatch the
   unsigned mode of the canonical workflow from the release branch, then select
   and watch only the run whose `headSha` equals the recorded release commit:

```bash
branch="$(git branch --show-current)"
release_commit="$(git rev-parse HEAD)"
git ls-remote --heads origin "refs/heads/${branch}"

gh workflow run rust-release.yml \
  --repo hasna/codewith \
  --ref "${branch}" \
  -f release_mode=build_unsigned

gh run list \
  --repo hasna/codewith \
  --workflow rust-release.yml \
  --branch "${branch}" \
  --event workflow_dispatch \
  --json databaseId,headSha,status,conclusion,createdAt \
  --limit 20

gh run watch <exact-run-id> --repo hasna/codewith --exit-status
gh run view <exact-run-id> \
  --repo hasna/codewith \
  --json workflowName,event,headBranch,headSha,status,conclusion
```

Do not substitute a local Cargo or Bazel build for this run.

6. Stage the npm package from that exact successful workflow run. Use the
   repository packaging script so the tarball is assembled from the remote
   release artifacts rather than a stale globally installed binary:

```bash
./scripts/stage_npm_packages.py \
  --release-version "<version>" \
  --workflow-url "https://github.com/hasna/codewith/actions/runs/<exact-run-id>" \
  --package codex \
  --output-dir dist/npm
```

7. Install the staged root tarball into a temporary prefix and smoke-test it
   before publishing:

```bash
smoke_prefix="$(mktemp -d)"
smoke_home="$(mktemp -d)"
npm install -g \
  --prefix "${smoke_prefix}" \
  "dist/npm/codex-npm-<version>.tgz"
CODEWITH_HOME="${smoke_home}" "${smoke_prefix}/bin/codewith" --version
CODEWITH_HOME="${smoke_home}" "${smoke_prefix}/bin/codewith" --help
```

8. Post publish intent to `git-publishing` before the tag push. Include
   `@hasna/codewith@<version>` and a one-line changelog. The tag-triggered
   workflow owns npm publication and its protected `NODE_AUTH_TOKEN` plus
   setup-node npmrc; never print, capture, or copy that credential locally.
9. Create a new immutable release tag. Both the local and remote checks must
   prove the tag absent. A pre-existing tag, an ambiguous remote result, or a
   push race stops the release; never move, delete, force-push, or reuse it:

```bash
version="<version>"
tag="rust-v${version}"
release_commit="$(git rev-parse HEAD)"

if git show-ref --verify --quiet "refs/tags/${tag}"; then
  echo "Local tag ${tag} already exists"
  exit 1
fi

set +e
git ls-remote --exit-code --tags \
  origin \
  "refs/tags/${tag}" \
  "refs/tags/${tag}^{}"
remote_tag_status=$?
set -e

case "${remote_tag_status}" in
  2) ;;
  0)
    echo "Remote tag ${tag} already exists"
    exit 1
    ;;
  *)
    echo "Remote tag check failed with ${remote_tag_status}"
    exit "${remote_tag_status}"
    ;;
esac

git tag -a "${tag}" "${release_commit}" -m "Codewith ${version}"
test "$(git rev-parse "${tag}^{commit}")" = "${release_commit}"
git push origin "refs/tags/${tag}"
```

The tag push triggers `.github/workflows/rust-release.yml`, which builds the
native artifacts, creates the GitHub release, stages the npm tarballs, and
publishes them. Do not run a separate local `npm publish`.

10. Select the tag-triggered `rust-release` run whose `headSha` equals the
    release commit and require it to complete successfully. Verify the GitHub
    release assets and npm registry state before installing:

```bash
gh run list \
  --repo hasna/codewith \
  --workflow rust-release.yml \
  --branch "rust-v<version>" \
  --event push \
  --json databaseId,headBranch,headSha,status,conclusion,createdAt \
  --limit 20

gh run watch <exact-tag-run-id> --repo hasna/codewith --exit-status
gh run view <exact-tag-run-id> \
  --repo hasna/codewith \
  --json workflowName,event,headBranch,headSha,status,conclusion
npm view @hasna/codewith@<version> version dist.tarball --json
npm view @hasna/codewith version dist-tags --json
```

11. Update the installation that owns the first active `codewith` command.
    Do not install through npm first and then repair PATH ambiguity afterward.
    Classify the active command before mutation, stop when its owner cannot be
    proven, and use exactly one matching installer:

```bash
set -eu

version="<version>"
active_codewith="$(command -v codewith || true)"
if [ -z "${active_codewith}" ]; then
  echo "No active codewith command was found"
  exit 1
fi

resolve_path() {
  python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$1"
}

codewith_home="${CODEWITH_HOME:-$HOME/.codewith}"
standalone_current="${codewith_home}/packages/standalone/current"
active_resolved="$(resolve_path "${active_codewith}")"
standalone_resolved=""
if [ -e "${standalone_current}" ] || [ -L "${standalone_current}" ]; then
  standalone_resolved="$(resolve_path "${standalone_current}")"
fi

install_owner=""
if [ -n "${standalone_resolved}" ]; then
  case "${active_resolved}" in
    "${standalone_resolved}"/*) install_owner="standalone" ;;
  esac
fi

npm_bin_dir=""
if [ -z "${install_owner}" ] && command -v npm >/dev/null 2>&1; then
  npm_bin_dir="$(npm prefix -g)/bin"
  case "${active_codewith}" in
    "${npm_bin_dir}"/*) install_owner="npm" ;;
  esac
fi

bun_bin_dir=""
if [ -z "${install_owner}" ] && command -v bun >/dev/null 2>&1; then
  bun_bin_dir="$(bun pm bin -g)"
  case "${active_codewith}" in
    "${bun_bin_dir}"/*) install_owner="bun" ;;
  esac
fi

case "${install_owner}" in
  standalone)
    (
      installer="$(mktemp)"
      trap 'rm -f "${installer}"' EXIT
      curl -fsSL \
        "https://github.com/hasna/codewith/releases/download/rust-v${version}/install.sh" \
        -o "${installer}"
      CODEWITH_NON_INTERACTIVE=1 sh "${installer}" --release "${version}"
    )
    ;;
  npm)
    npm install -g "@hasna/codewith@${version}"
    ;;
  bun)
    # Preserve Bun's minimumReleaseAge quarantine. The exact
    # @hasna/codewith package exclusion must already be present.
    bun install -g "@hasna/codewith@${version}"
    ;;
  *)
    echo "Active codewith owner is unverified: ${active_codewith}"
    exit 1
    ;;
esac

updated_codewith="$(command -v codewith)"
updated_resolved="$(resolve_path "${updated_codewith}")"

case "${install_owner}" in
  standalone)
    if [ ! -L "${standalone_current}" ]; then
      echo "Standalone current path is not a symlink: ${standalone_current}"
      exit 1
    fi
    updated_standalone_resolved="$(resolve_path "${standalone_current}")"
    case "${updated_resolved}" in
      "${updated_standalone_resolved}"/*) ;;
      *)
        echo "Active codewith no longer resolves through ${standalone_current}"
        exit 1
        ;;
    esac
    ;;
  npm)
    case "${updated_codewith}" in
      "${npm_bin_dir}"/*) ;;
      *) echo "npm no longer owns the active codewith command"; exit 1 ;;
    esac
    ;;
  bun)
    case "${updated_codewith}" in
      "${bun_bin_dir}"/*) ;;
      *) echo "Bun no longer owns the active codewith command"; exit 1 ;;
    esac
    ;;
esac

reported_version="$("${updated_codewith}" --version)"
test "${reported_version}" = "codewith ${version}"
"${updated_codewith}" --help >/dev/null
```

On Windows, apply the same owner-selection rule. A standalone install uses the
published `install.ps1` release asset with `-Release <version>`, then verifies
that the active executable resolves through
`$env:CODEWITH_HOME\packages\standalone\current` (or the default
`$HOME\.codewith` path) and reports the intended version. npm and Bun remain
valid only when their global bin directory owns the first active command.

12. Verify the annotated remote tag and its peeled commit, then confirm the
    publish in the original `git-publishing` thread:

```bash
git ls-remote --tags \
  origin \
  "refs/tags/rust-v<version>" \
  "refs/tags/rust-v<version>^{}"
```

## Rollback

- Before release, record the active executable path, install owner, installed
  version, and exact same-owner command that restores the previous known-good
  version. A standalone rollback re-runs the published installer for that exact
  prior release; npm and Bun roll back only through the manager that owned the
  active command.
- Before the tag push, stop without publishing when any gate fails.
- After the tag push or npm publication, rollback is forward-only: restore the
  prior installed package if needed, keep the published tag and npm version
  intact, and publish a new patch version from a new commit and new tag. Never
  rewrite release provenance to make a failed release appear successful.

## Verification

Before reporting success, verify:

- The changelog, Cargo version, npm package version, release commit, and tag
  agree.
- The pre-publish tarball came from the exact successful remote workflow run
  and passed temporary-prefix `--version` and `--help` smoke tests.
- The tag-triggered workflow ran at the release commit and completed
  successfully.
- npm `latest` is the intended version.
- `codewith --version` from the active PATH reports the intended version.
- The post-publication installer matched the pre-mutation active install owner.
  For standalone installs, `packages/standalone/current` is a symlink whose
  resolved release contains the resolved active executable.
- The release tag dereferences to the published commit.
- `git status --short` is clean in the release worktree.
- Staged/range secret scans, `git-publishing` intent/confirmation, and rollback
  evidence are recorded.
- Any full-suite failures are explained with exact host/tooling causes and focused changed-path tests are listed.
