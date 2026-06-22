#!/usr/bin/env bash
# Build CodeWith on apple03, wrap the SwiftPM executable into a proper .app
# bundle, and launch it in the GUI session (so it appears on screen).
#
# Usage: run-on-apple03.sh
set -euo pipefail

HOST="${1:-$(cat /tmp/a3 2>/dev/null || echo hasna@apple03)}"
REMOTE_DIR="/Users/hasna/codewith-build"
APP="/Users/hasna/Applications/CodeWith.app"
DEV_DIR="/Applications/Xcode.app/Contents/Developer"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "==> sync + build (release)"
rsync -az --delete --exclude '.build' --exclude '.git' \
  -e "ssh -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new" \
  "$HERE/CodeWith/" "$HOST:$REMOTE_DIR/"
ssh -o ConnectTimeout=15 "$HOST" "export DEVELOPER_DIR=$DEV_DIR; cd $REMOTE_DIR && swift build -c release 2>&1 | tail -5"

echo "==> assemble app bundle"
ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -e
APP="$APP"
BIN="$REMOTE_DIR/.build/release/CodeWith"
rm -rf "\$APP"
mkdir -p "\$APP/Contents/MacOS" "\$APP/Contents/Resources"
cp "\$BIN" "\$APP/Contents/MacOS/CodeWith"
cat > "\$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>CodeWith</string>
  <key>CFBundleDisplayName</key><string>CodeWith</string>
  <key>CFBundleIdentifier</key><string>com.hasna.codewith</string>
  <key>CFBundleVersion</key><string>1.0</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleExecutable</key><string>CodeWith</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST
echo "bundle ready: \$APP"
REMOTE

echo "==> launch in GUI session"
ssh -o ConnectTimeout=15 "$HOST" "open '$APP' && echo launched || echo 'open failed'"
echo "==> CodeWith should now be visible on apple03's display."
