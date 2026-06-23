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
ssh -o ConnectTimeout=15 "$HOST" "CODEWITH_CLI_PATH=$(printf '%q' "$CLI_PATH") bash -s" <<REMOTE
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
if [ "\$CLI_SRC" -ef "\$BIN" ]; then
  echo "refusing to bundle the CodeWith GUI executable as the codewith CLI" >&2
  exit 1
fi
resolve_realpath() {
  python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "\$1"
}
native_cli_from_package() {
  local src="\$1"
  local real root arch target package candidate
  real="\$(resolve_realpath "\$src")"
  if [ "\$(basename "\$real")" != "codex.js" ] || [ "\$(basename "\$(dirname "\$real")")" != "bin" ]; then
    return 1
  fi
  root="\$(cd "\$(dirname "\$real")/.." && pwd -P)"
  arch="\$(uname -m)"
  case "\$arch" in
    arm64|aarch64)
      target="aarch64-apple-darwin"
      package="codewith-darwin-arm64"
      ;;
    x86_64)
      target="x86_64-apple-darwin"
      package="codewith-darwin-x64"
      ;;
    *)
      return 1
      ;;
  esac
  for candidate in \
    "\$root/../\$package/vendor/\$target/bin/codewith" \
    "\$root/vendor/\$target/bin/codewith"; do
    if [ -x "\$candidate" ]; then
      printf '%s\n' "\$candidate"
      return 0
    fi
  done
  return 1
}
stage_cli() {
  local src="\$1"
  local dest="\$2"
  local real native sibling
  real="\$(resolve_realpath "\$src")"
  if native="\$(native_cli_from_package "\$real")"; then
    cp "\$native" "\$dest/codewith"
  else
    sibling="\$(dirname "\$real")/codewith-bin"
    if [ -x "\$sibling" ]; then
      cp "\$sibling" "\$dest/codewith"
    else
      cp "\$real" "\$dest/codewith"
    fi
  fi
  chmod 755 "\$dest/codewith"
}
rm -rf "\$APP"
mkdir -p "\$APP/Contents/MacOS" "\$APP/Contents/Resources"
cp "\$BIN" "\$APP/Contents/MacOS/CodeWith"
stage_cli "\$CLI_SRC" "\$APP/Contents/Resources"
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
env -i PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="\$HOME" "\$APP/Contents/Resources/codewith" app-server --help >/tmp/codewith-bundled-cli-smoke.log 2>&1 || {
  tail -40 /tmp/codewith-bundled-cli-smoke.log >&2
  exit 1
}
echo "bundle ready: \$APP"
echo "bundled CLI: \$CLI_SRC"
REMOTE

if [[ "$LAUNCH" == "1" ]]; then
  echo "==> launch in GUI session"
  ssh -o ConnectTimeout=15 "$HOST" "open '$APP'"
  echo "==> CodeWith should now be visible on apple03's display."
else
  echo "==> launch skipped. Pass --launch to open the app on $HOST."
fi
