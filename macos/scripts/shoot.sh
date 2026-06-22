#!/usr/bin/env bash
# Sync the CodeWith Swift app to the macOS build host, build it, render every
# screen via the in-app ImageRenderer snapshot harness, and pull the PNGs back.
#
# Usage: shoot.sh [build-host-ssh-target]
#   default host target read from /tmp/a3 (e.g. hasna@100.100.226.69)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"      # macos/
REPO="$(cd "$HERE/.." && pwd)"
HOST="${1:-$(cat /tmp/a3 2>/dev/null || echo hasna@apple03)}"
REMOTE_DIR="/Users/hasna/codewith-build"
REMOTE_SHOTS="/tmp/codewith-shots"
LOCAL_RENDERS="$REPO/design-refs/renders"
DEV_DIR="/Applications/Xcode.app/Contents/Developer"

echo "==> host=$HOST"
mkdir -p "$LOCAL_RENDERS"

echo "==> sync sources"
rsync -az --delete \
  --exclude '.build' --exclude '.git' \
  -e "ssh -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new" \
  "$HERE/CodeWith/" "$HOST:$REMOTE_DIR/"

echo "==> build"
ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -euo pipefail
export DEVELOPER_DIR="$DEV_DIR"
cd "$REMOTE_DIR"
rm -f .build/release/CodeWith
if ! swift build -c release > /tmp/codewith-swift-build.log 2>&1; then
  tail -80 /tmp/codewith-swift-build.log
  exit 1
fi
tail -40 /tmp/codewith-swift-build.log
test -x .build/release/CodeWith
REMOTE

echo "==> render snapshots"
ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -euo pipefail
export DEVELOPER_DIR="$DEV_DIR"
export CODEWITH_SNAPSHOT=1
export CODEWITH_SNAPSHOT_DIR="$REMOTE_SHOTS"
rm -rf "$REMOTE_SHOTS"
mkdir -p "$REMOTE_SHOTS"
cd "$REMOTE_DIR"
if ! ./.build/release/CodeWith > /tmp/codewith-snapshot.log 2>&1; then
  tail -80 /tmp/codewith-snapshot.log
  exit 1
fi
tail -20 /tmp/codewith-snapshot.log
count=\$(find "$REMOTE_SHOTS" -type f -name '*.png' -size +0c | wc -l | tr -d ' ')
if [ "\$count" -eq 0 ]; then
  echo "no non-empty PNG snapshots rendered" >&2
  exit 1
fi
ls -1 "$REMOTE_SHOTS"
REMOTE

echo "==> pull renders"
rsync -az -e "ssh -o ConnectTimeout=15" "$HOST:$REMOTE_SHOTS/" "$LOCAL_RENDERS/"
echo "==> done. renders in $LOCAL_RENDERS"
ls -la "$LOCAL_RENDERS"
