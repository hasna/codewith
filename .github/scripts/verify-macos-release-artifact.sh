#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <artifact> <expected-identifier> <expected-team-id> <expected-authority>" >&2
  exit 2
}

[[ $# -eq 4 ]] || usage

artifact="$1"
expected_identifier="$2"
expected_team_id="$3"
expected_authority="$4"

[[ -f "$artifact" && ! -L "$artifact" ]] || {
  echo "macOS release artifact must be a regular non-symlink file: $artifact" >&2
  exit 1
}
[[ "$expected_identifier" =~ ^[A-Za-z0-9._-]+$ ]] || {
  echo "invalid expected macOS signing identifier" >&2
  exit 1
}
[[ "$expected_team_id" =~ ^[A-Z0-9]{10}$ ]] || {
  echo "invalid expected Apple TeamIdentifier" >&2
  exit 1
}
[[ -n "$expected_authority" && "$expected_authority" != *$'\n'* && "$expected_authority" == "Developer ID Application: "* ]] || {
  echo "invalid expected Developer ID Application authority" >&2
  exit 1
}

codesign --verify --strict --verbose=4 "$artifact"
details="$(codesign -dv --verbose=4 "$artifact" 2>&1)"
actual_identifier="$(sed -n 's/^Identifier=//p' <<<"$details" | head -n 1)"
actual_authority="$(sed -n 's/^Authority=//p' <<<"$details" | head -n 1)"
actual_team_id="$(sed -n 's/^TeamIdentifier=//p' <<<"$details" | head -n 1)"

[[ "$actual_identifier" == "$expected_identifier" ]] || {
  echo "macOS signing identifier mismatch for $artifact" >&2
  exit 1
}
[[ "$actual_authority" == "$expected_authority" ]] || {
  echo "macOS signing authority mismatch for $artifact" >&2
  exit 1
}
[[ "$actual_team_id" == "$expected_team_id" ]] || {
  echo "macOS TeamIdentifier mismatch for $artifact" >&2
  exit 1
}

designated_requirement="identifier \"${expected_identifier}\" and anchor apple generic and certificate leaf[subject.OU] = \"${expected_team_id}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
codesign --verify --strict --verbose=4 -R="$designated_requirement" "$artifact"

# For standalone Mach-O binaries, an accepted notarization ticket is normally
# looked up online rather than stapled. Gatekeeper assessment is therefore the
# release gate; DMGs retain their separate stapler validation.
assessment="$(spctl --assess --type execute --verbose=4 "$artifact" 2>&1)" || {
  printf '%s\n' "$assessment" >&2
  exit 1
}
printf '%s\n' "$assessment"
grep -Fq 'source=Notarized Developer ID' <<<"$assessment" || {
  echo "Gatekeeper assessment did not prove Notarized Developer ID provenance" >&2
  exit 1
}
