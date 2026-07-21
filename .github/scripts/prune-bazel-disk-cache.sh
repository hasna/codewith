#!/usr/bin/env bash

# Bound a Bazel disk cache directory to a byte budget by evicting the
# least-recently-accessed files first.
#
# Why: keyless CI (no BuildBuddy) relies on a persisted local Bazel disk cache
# (see the `ci-keyless` config in `.bazelrc`) that the workflow uploads with
# `actions/cache`. A GitHub Actions cache entry may not exceed 10 GB, and
# Bazel's own disk-cache garbage collection is idle-triggered
# (`--experimental_disk_cache_gc_idle_delay`, default 5m) so it usually does not
# run in a one-shot CI invocation whose server exits at job end. This script is
# the authoritative, version-independent size bound: it runs right before the
# cache save step.
#
# Safety: Bazel treats a missing disk-cache entry as a cache miss and simply
# rebuilds it, so evicting entries is always safe for correctness.
#
# Usage: prune-bazel-disk-cache.sh <cache_dir> <max_bytes>

set -euo pipefail

cache_dir="${1:?usage: prune-bazel-disk-cache.sh <cache_dir> <max_bytes>}"
max_bytes="${2:?usage: prune-bazel-disk-cache.sh <cache_dir> <max_bytes>}"

if [[ ! -d "${cache_dir}" ]]; then
  echo "prune: no cache directory at ${cache_dir}; nothing to do."
  exit 0
fi

dir_size_bytes() {
  du -sb "${cache_dir}" 2>/dev/null | awk '{ print $1 }'
}

before="$(dir_size_bytes)"
before="${before:-0}"

if (( before <= max_bytes )); then
  echo "prune: ${cache_dir} is ${before} B <= budget ${max_bytes} B; no eviction needed."
  exit 0
fi

echo "prune: ${cache_dir} is ${before} B > budget ${max_bytes} B; evicting least-recently-accessed files."

current="${before}"
# find prints "<access-time-epoch>\t<size-bytes>\t<path>"; sort oldest-access
# first and delete until the running total drops under the budget.
while IFS=$'\t' read -r _atime size path; do
  (( current <= max_bytes )) && break
  if rm -f -- "${path}" 2>/dev/null; then
    current=$(( current - size ))
  fi
done < <(find "${cache_dir}" -type f -printf '%A@\t%s\t%p\n' 2>/dev/null | sort -n -k1,1)

after="$(dir_size_bytes)"
after="${after:-0}"
echo "prune: ${cache_dir} now ${after} B (budget ${max_bytes} B)."
