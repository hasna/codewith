#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
verifier="${repo_root}/.github/scripts/verify-macos-release-artifact.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
touch "$tmp/codewith"

cat >"$tmp/bin/codesign" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ " $* " == *" -dv "* ]]; then
  printf 'Identifier=%s\n' "${FAKE_IDENTIFIER}" >&2
  printf 'Authority=%s\n' "${FAKE_AUTHORITY}" >&2
  printf 'TeamIdentifier=%s\n' "${FAKE_TEAM_ID}" >&2
fi
exit "${FAKE_CODESIGN_STATUS:-0}"
EOF
cat >"$tmp/bin/spctl" <<'EOF'
#!/usr/bin/env bash
echo "source=${FAKE_SPCTL_SOURCE:-Notarized Developer ID}" >&2
exit "${FAKE_SPCTL_STATUS:-0}"
EOF
chmod +x "$tmp/bin/codesign" "$tmp/bin/spctl"

export PATH="$tmp/bin:$PATH"
export FAKE_IDENTIFIER=codewith
export FAKE_TEAM_ID=ABCDEFGHIJ
export FAKE_AUTHORITY='Developer ID Application: Hasna (ABCDEFGHIJ)'

"$verifier" \
  "$tmp/codewith" \
  codewith \
  ABCDEFGHIJ \
  'Developer ID Application: Hasna (ABCDEFGHIJ)'

FAKE_TEAM_ID=ZZZZZZZZZZ
export FAKE_TEAM_ID
if "$verifier" "$tmp/codewith" codewith ABCDEFGHIJ 'Developer ID Application: Hasna (ABCDEFGHIJ)'; then
  echo "wrong TeamIdentifier unexpectedly passed" >&2
  exit 1
fi

FAKE_TEAM_ID=ABCDEFGHIJ
FAKE_AUTHORITY='Developer ID Application: Attacker (ABCDEFGHIJ)'
export FAKE_TEAM_ID FAKE_AUTHORITY
if "$verifier" "$tmp/codewith" codewith ABCDEFGHIJ 'Developer ID Application: Hasna (ABCDEFGHIJ)'; then
  echo "wrong authority unexpectedly passed" >&2
  exit 1
fi

FAKE_AUTHORITY='Developer ID Application: Hasna (ABCDEFGHIJ)'
FAKE_SPCTL_SOURCE='Developer ID'
export FAKE_AUTHORITY FAKE_SPCTL_SOURCE
if "$verifier" "$tmp/codewith" codewith ABCDEFGHIJ 'Developer ID Application: Hasna (ABCDEFGHIJ)'; then
  echo "non-notarized Gatekeeper source unexpectedly passed" >&2
  exit 1
fi

echo "macOS release artifact verifier fixtures passed"
