#!/usr/bin/env bash
# Build CodeWith on apple03 and wrap the SwiftPM executable into a proper .app
# bundle. It does not launch the GUI app unless --launch is passed.
#
# Usage: run-on-apple03.sh [--launch] [--cli-path /path/to/codewith] [build-host-ssh-target]
set -euo pipefail

LAUNCH=0
CLI_PATH="${CODEWITH_CLI_PATH:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --launch)
      LAUNCH=1
      shift
      ;;
    --cli-path)
      CLI_PATH="${2:-}"
      if [[ -z "$CLI_PATH" ]]; then
        echo "--cli-path requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "unknown option: $1" >&2
      exit 2
      ;;
    *)
      break
      ;;
  esac
done

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
# CLI_PATH (from --cli-path or the invoking host's CODEWITH_CLI_PATH) names a
# path on the build host; forward it shell-quoted so spaces and
# metacharacters cannot break or inject into the remote script.
ssh -o ConnectTimeout=15 "$HOST" "CODEWITH_CLI_PATH=$(printf '%q' "$CLI_PATH") bash -s" <<REMOTE
set -euo pipefail
APP="$APP"
BIN="$REMOTE_DIR/.build/release/CodeWith"
CLI_SRC="\${CODEWITH_CLI_PATH:-}"
if [ -z "\$CLI_SRC" ]; then
  CLI_SRC="\$(command -v codewith || true)"
fi
if [ -z "\$CLI_SRC" ] || [ ! -x "\$CLI_SRC" ]; then
  echo "codewith CLI not found on build host; install it or set CODEWITH_CLI_PATH / --cli-path" >&2
  exit 1
fi
# Resolve symlinks. A bun/npm shim (bin/codex.js) cannot run standalone once
# copied out of its node_modules tree, so bundle the platform vendor Mach-O
# binary that the shim dispatches to instead. Prefer the package matching the
# build host's architecture so an arm64 host never silently bundles an x86_64
# CLI that would run under Rosetta.
CLI_SRC="\$(readlink -f "\$CLI_SRC")"
case "\$CLI_SRC" in
  *.js|*.mjs|*.cjs)
    PKG_DIR="\$(cd "\$(dirname "\$CLI_SRC")/.." && pwd)"   # …/@hasna/codewith
    case "\$(uname -m)" in
      arm64|aarch64) ARCH_PKG="codewith-darwin-arm64" ;;
      x86_64)        ARCH_PKG="codewith-darwin-x64" ;;
      *)             ARCH_PKG="codewith-darwin-*" ;;
    esac
    VENDOR=""
    for cand in "\$PKG_DIR"/node_modules/@hasna/\$ARCH_PKG/vendor/*/bin/codewith \
                "\$PKG_DIR"/../\$ARCH_PKG/vendor/*/bin/codewith \
                "\$PKG_DIR"/node_modules/@hasna/codewith-darwin-*/vendor/*/bin/codewith \
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
# Hermetic check: the bundled CLI must run without the ambient ssh env
# (PATH/node), or the GUI app will fail at runtime.
if ! env -i PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="\$HOME" \
    "\$APP/Contents/Resources/codewith" --version >/tmp/codewith-bundled-cli-version.log 2>&1; then
  echo "bundled codewith CLI does not execute standalone from the app bundle" >&2
  tail -20 /tmp/codewith-bundled-cli-version.log >&2
  exit 1
fi
# Stamp the bundle with the bundled CLI's version so version-based
# upgrade/cache checks see deploys change.
CLI_VERSION="\$(awk '{print \$2; exit}' /tmp/codewith-bundled-cli-version.log)"
if [ -z "\$CLI_VERSION" ]; then
  CLI_VERSION="1.0"
fi
cat > "\$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>CodeWith</string>
  <key>CFBundleDisplayName</key><string>CodeWith</string>
  <key>CFBundleIdentifier</key><string>com.hasna.codewith</string>
  <key>CFBundleVersion</key><string>\$CLI_VERSION</string>
  <key>CFBundleShortVersionString</key><string>\$CLI_VERSION</string>
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
env -i PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="\$HOME" "\$APP/Contents/Resources/codewith" app-server --help >/tmp/codewith-bundled-cli-smoke.log 2>&1 || {
  tail -40 /tmp/codewith-bundled-cli-smoke.log >&2
  exit 1
}
echo "bundle ready: \$APP"
echo "bundled CLI: \$CLI_SRC"
echo "bundled CLI version: \$CLI_VERSION"
REMOTE

if [[ "$LAUNCH" == "1" ]]; then
  echo "==> launch in GUI session"
  ssh -o ConnectTimeout=15 "$HOST" "open '$APP' && echo launched || echo 'open failed'"
  echo "==> CodeWith should now be visible on apple03's display."
else
  echo "==> launch skipped. Pass --launch to open the app on $HOST."
fi
