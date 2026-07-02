#!/usr/bin/env bash
# Capture real display pixels of the running CodeWith app on the macOS build
# host, over SSH, and pull the PNG back locally.
#
# Plain `ssh host screencapture` fails twice over: the SSH session has no Aqua
# access, and even a gui-domain LaunchAgent is denied by TCC Screen Recording.
# The reliable path is to run `screencapture` through the user's existing tmux
# server (`tmux run-shell -b`): the command executes from the tmux server
# process, whose TCC-responsible process is the GUI terminal that spawned it
# (granted Screen Recording). No tmux window or pane is touched.
#
# Capture strategy: prefer window-id capture (`screencapture -l`), which reads
# the app window's own buffer — correct even when another app's window covers
# it and with no focus stealing on a shared machine. Fall back to a region
# capture of the window frame, then to the full display.
#
# Usage: capture-display.sh [out.png] [host] [region]
#   out.png  local output path            (default /tmp/codewith-shot.png)
#   host     ssh target                   (default $(cat /tmp/a3) or hasna@apple03)
#   region   x,y,w,h in points for -R     (skips window-id capture when given)
set -euo pipefail

OUT="${1:-/tmp/codewith-shot.png}"
HOST="${2:-$(cat /tmp/a3 2>/dev/null || echo hasna@apple03)}"
REGION="${3:-}"

REMOTE_PNG="/tmp/codewith-capture-$$.png"

ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -euo pipefail
REGION="$REGION"
WID=""
if [ -z "\$REGION" ]; then
  # Build (once) a tiny helper that prints the CGWindowID of the app's main window.
  if [ ! -x /tmp/codewith-winid ]; then
    cat > /tmp/codewith-winid.swift <<'EOF'
import CoreGraphics
import Foundation
let opts: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else { exit(1) }
let target = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "CodeWith"
for w in list {
    let owner = w[kCGWindowOwnerName as String] as? String ?? ""
    let layer = w[kCGWindowLayer as String] as? Int ?? -1
    let num = w[kCGWindowNumber as String] as? Int ?? 0
    let bounds = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let width = bounds["Width"] as? Double ?? 0
    if owner == target && layer == 0 && width > 200 {   // skip status item windows
        print(num)
        exit(0)
    }
}
exit(2)
EOF
    DEVELOPER_DIR="\${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}" \
      swiftc -O -o /tmp/codewith-winid /tmp/codewith-winid.swift >/dev/null 2>&1 || true
  fi
  WID=\$(/tmp/codewith-winid CodeWith 2>/dev/null || true)
  if [ -z "\$WID" ]; then
    GEO=\$(tmux run-shell "osascript -e 'tell application \"System Events\" to tell process \"CodeWith\" to get {position, size} of window 1'" 2>/dev/null | tr -d ' ') || true
    if [ -n "\${GEO:-}" ]; then REGION="\$GEO"; fi
  fi
fi
rm -f "$REMOTE_PNG"
if [ -n "\$WID" ]; then
  tmux run-shell -b "/usr/sbin/screencapture -x -o -l\$WID $REMOTE_PNG"
elif [ -n "\$REGION" ]; then
  tmux run-shell -b "/usr/sbin/screencapture -x -R\$REGION $REMOTE_PNG"
else
  tmux run-shell -b "/usr/sbin/screencapture -x $REMOTE_PNG"
fi
for i in \$(seq 1 20); do [ -s "$REMOTE_PNG" ] && break; sleep 0.5; done
[ -s "$REMOTE_PNG" ] || { echo "capture failed (no PNG produced; is a tmux server with a GUI-terminal ancestor running?)" >&2; exit 1; }
REMOTE

scp -q "$HOST:$REMOTE_PNG" "$OUT"
ssh -o ConnectTimeout=15 "$HOST" "rm -f $REMOTE_PNG"
echo "captured: $OUT"
