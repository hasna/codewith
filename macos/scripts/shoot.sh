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
ssh -o ConnectTimeout=15 "$HOST" \
  "export DEVELOPER_DIR=$DEV_DIR; cd $REMOTE_DIR && swift build -c release 2>&1 | tail -40"

echo "==> render snapshots"
ssh -o ConnectTimeout=15 "$HOST" \
  "export DEVELOPER_DIR=$DEV_DIR CODEWITH_SNAPSHOT=1 CODEWITH_SNAPSHOT_DIR=$REMOTE_SHOTS; \
   rm -rf $REMOTE_SHOTS; mkdir -p $REMOTE_SHOTS; \
   cd $REMOTE_DIR && ./.build/release/CodeWith 2>&1 | tail -20; ls -1 $REMOTE_SHOTS"

echo "==> pull renders"
rsync -az -e "ssh -o ConnectTimeout=15" "$HOST:$REMOTE_SHOTS/" "$LOCAL_RENDERS/"
echo "==> done. renders in $LOCAL_RENDERS"
ls -la "$LOCAL_RENDERS"
