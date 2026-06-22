#!/usr/bin/env bash
# Build CodeWith on apple03 and wrap the SwiftPM executable into a proper .app
# bundle. It does not launch the GUI app unless --launch is passed.
#
# Usage: run-on-apple03.sh [--launch] [build-host-ssh-target]
set -euo pipefail

LAUNCH=0
if [[ "${1:-}" == "--launch" ]]; then
  LAUNCH=1
  shift
fi

HOST="${1:-$(cat /tmp/a3 2>/dev/null || echo hasna@apple03)}"
REMOTE_DIR="/Users/hasna/codewith-build"
APP="/Users/hasna/Applications/CodeWith.app"
DEV_DIR="/Applications/Xcode.app/Contents/Developer"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "==> sync + build (release)"
rsync -az --delete --exclude '.build' --exclude '.git' \
  -e "ssh -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new" \
  "$HERE/CodeWith/" "$HOST:$REMOTE_DIR/"
ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -euo pipefail
export DEVELOPER_DIR="$DEV_DIR"
cd "$REMOTE_DIR"
rm -f .build/release/CodeWith
if ! swift build -c release > /tmp/codewith-swift-build.log 2>&1; then
  tail -80 /tmp/codewith-swift-build.log
  exit 1
fi
tail -20 /tmp/codewith-swift-build.log
test -x .build/release/CodeWith
REMOTE

echo "==> assemble app bundle"
ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -euo pipefail
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
  <key>CFBundleURLTypes</key><array>
    <dict>
      <key>CFBundleURLName</key><string>CodeWith Links</string>
      <key>CFBundleURLSchemes</key><array>
        <string>codewith</string>
        <string>codex</string>
      </array>
    </dict>
  </array>
</dict></plist>
PLIST
plutil -lint "\$APP/Contents/Info.plist"
codesign --force --deep --sign - "\$APP"
codesign --verify --deep --strict "\$APP"
echo "bundle ready: \$APP"
REMOTE

if [[ "$LAUNCH" == "1" ]]; then
  echo "==> launch in GUI session"
  ssh -o ConnectTimeout=15 "$HOST" "open '$APP' && echo launched || echo 'open failed'"
  echo "==> CodeWith should now be visible on apple03's display."
else
  echo "==> launch skipped. Pass --launch to open the app on $HOST."
fi
