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
REMOTE_DIR="${CODEWITH_REMOTE_DIR:-/Users/hasna/codewith-build}"
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
CLI_SRC="\${CODEWITH_CLI_PATH:-}"
if [ -z "\$CLI_SRC" ]; then
  CLI_SRC="\$(command -v codewith || true)"
fi
if [ -z "\$CLI_SRC" ] || [ ! -x "\$CLI_SRC" ]; then
  echo "codewith CLI not found on build host; install it or set CODEWITH_CLI_PATH" >&2
  exit 1
fi
# Resolve symlinks. A bun/npm shim (bin/codex.js) cannot run standalone once
# copied out of its node_modules tree, so bundle the platform vendor Mach-O
# binary that the shim dispatches to instead.
CLI_SRC="\$(readlink -f "\$CLI_SRC")"
case "\$CLI_SRC" in
  *.js|*.mjs|*.cjs)
    PKG_DIR="\$(cd "\$(dirname "\$CLI_SRC")/.." && pwd)"   # …/@hasna/codewith
    VENDOR=""
    for cand in "\$PKG_DIR"/node_modules/@hasna/codewith-darwin-*/vendor/*/bin/codewith \
                "\$PKG_DIR"/../codewith-darwin-*/vendor/*/bin/codewith; do
      if [ -x "\$cand" ] && "\$cand" --version >/dev/null 2>&1; then VENDOR="\$cand"; break; fi
    done
    if [ -z "\$VENDOR" ]; then
      echo "codewith CLI resolves to a JS shim (\$CLI_SRC) with no runnable vendor binary;" >&2
      echo "set CODEWITH_CLI_PATH to a standalone codewith binary" >&2
      exit 1
    fi
    CLI_SRC="\$VENDOR"
    ;;
esac
if [ "\$CLI_SRC" -ef "\$BIN" ]; then
  echo "refusing to bundle the CodeWith GUI executable as the codewith CLI" >&2
  exit 1
fi
# Quit only the CodeWith GUI app (exact bundle path) before replacing its bundle.
pkill -f "\$APP/Contents/MacOS/CodeWith" 2>/dev/null && sleep 1 || true
rm -rf "\$APP"
mkdir -p "\$APP/Contents/MacOS" "\$APP/Contents/Resources"
cp "\$BIN" "\$APP/Contents/MacOS/CodeWith"
cp "\$CLI_SRC" "\$APP/Contents/Resources/codewith"
chmod 755 "\$APP/Contents/Resources/codewith"
if ! "\$APP/Contents/Resources/codewith" --version >/dev/null 2>&1; then
  echo "bundled codewith CLI does not execute standalone from the app bundle" >&2
  exit 1
fi
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
echo "bundled CLI: \$CLI_SRC"
REMOTE

if [[ "$LAUNCH" == "1" ]]; then
  echo "==> launch in GUI session"
  ssh -o ConnectTimeout=15 "$HOST" "open '$APP' && echo launched || echo 'open failed'"
  echo "==> CodeWith should now be visible on apple03's display."
else
  echo "==> launch skipped. Pass --launch to open the app on $HOST."
fi
