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
# Usage: capture-display.sh [out.png] [host] [region]
#   out.png  local output path            (default /tmp/codewith-shot.png)
#   host     ssh target                   (default $(cat /tmp/a3) or hasna@apple03)
#   region   x,y,w,h in points for -R     (default: CodeWith window 1 via
#            System Events; falls back to full display)
set -euo pipefail

OUT="${1:-/tmp/codewith-shot.png}"
HOST="${2:-$(cat /tmp/a3 2>/dev/null || echo hasna@apple03)}"
REGION="${3:-}"

REMOTE_PNG="/tmp/codewith-capture-$$.png"

ssh -o ConnectTimeout=15 "$HOST" "bash -s" <<REMOTE
set -euo pipefail
REGION="$REGION"
if [ -z "\$REGION" ]; then
  GEO=\$(tmux run-shell "osascript -e 'tell application \"System Events\" to tell process \"CodeWith\" to get {position, size} of window 1'" 2>/dev/null | tr -d ' ') || true
  if [ -n "\${GEO:-}" ]; then REGION="\$GEO"; fi
fi
rm -f "$REMOTE_PNG"
if [ -n "\$REGION" ]; then
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
