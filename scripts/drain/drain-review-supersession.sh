#!/usr/bin/env bash
# Shared review supersession decision for the station PR-drain supervisor and guard.
#
# The supervisor decides whether a PR should spend a lane. The merge guard decides
# whether an attempted merge is allowed. Both must agree on the one subtle part:
# whether a head-matching NO_GO is still live after later review comments.

drain_review_log() {
  if declare -F glog >/dev/null 2>&1; then
    glog "$*"
  elif declare -F log >/dev/null 2>&1; then
    log "$*"
  fi
}

drain_review_roster_load() {
  DRAIN_REVIEW_CACHE="${DRAIN_REVIEW_CACHE:-${DRAIN_GUARD_CACHE:-$HOME/.drain-review-cache}}"
  DRAIN_REVIEW_ROSTER="${DRAIN_REVIEW_ROSTER:-$DRAIN_REVIEW_CACHE/roster.txt}"
  DRAIN_REVIEW_ROSTER_TTL="${DRAIN_REVIEW_ROSTER_TTL:-${DRAIN_GUARD_ROSTER_TTL:-900}}"

  mkdir -p "$DRAIN_REVIEW_CACHE" 2>/dev/null || return 0
  local age=999999 tmp
  [ -s "$DRAIN_REVIEW_ROSTER" ] && age=$(( $(date +%s) - $(stat -c %Y "$DRAIN_REVIEW_ROSTER" 2>/dev/null || echo 0) ))
  if [ "$age" -gt "$DRAIN_REVIEW_ROSTER_TTL" ]; then
    tmp="$DRAIN_REVIEW_ROSTER.$$"
    if timeout 30 conversations agents list --json --limit 5000 2>/dev/null \
       | jq -r '(if type=="array" then . else .agents end)[]?|.agent//empty' 2>/dev/null \
       | tr 'A-Z' 'a-z' | sort -u > "$tmp" 2>/dev/null && [ -s "$tmp" ]; then
      mv -f "$tmp" "$DRAIN_REVIEW_ROSTER"
    else
      rm -f "$tmp" 2>/dev/null
      [ -s "$DRAIN_REVIEW_ROSTER" ] || drain_review_log "WARN review roster unavailable; no verdict can be treated as superseding"
    fi
  fi
}

drain_review_is_registered() {
  [ -n "${1:-}" ] || return 1
  [ -s "${DRAIN_REVIEW_ROSTER:-}" ] || return 1
  grep -qxF "$(tr 'A-Z' 'a-z' <<<"$1")" "$DRAIN_REVIEW_ROSTER"
}

drain_review_reset_decision() {
  DRAIN_REVIEW_HEAD=""
  DRAIN_REVIEW_VERDICT_COUNT=0
  DRAIN_REVIEW_HEAD_VERDICT_COUNT=0
  DRAIN_REVIEW_BLOCKING_NOGO=0
  DRAIN_REVIEW_BLOCKING_CLASS=""
  DRAIN_REVIEW_BLOCKING_LINE=""
  DRAIN_REVIEW_NOGO_BY=""
  DRAIN_REVIEW_UNATTR_COUNT=0
  DRAIN_REVIEW_UNATTR_AT=""
  DRAIN_REVIEW_SUPERSEDING_BY=""
  DRAIN_REVIEW_SUPERSEDING_AT=""
  DRAIN_REVIEW_GO_LINE=""
  DRAIN_REVIEW_GO_SHA=""
  DRAIN_REVIEW_GO_AT=""
}

drain_review_supersession_decision() {
  local j="$1"
  local _rec _at _line _sha _matches _reviewer _verdict
  local _nogo_attr="" _nogo_by="" _unattr_count=0 _unattr_at="" _sup_by="" _sup_at=""

  drain_review_reset_decision
  DRAIN_REVIEW_HEAD="$(jq -r '.headRefOid // ""' <<<"$j" 2>/dev/null)"
  [ -n "$DRAIN_REVIEW_HEAD" ] || return 1

  DRAIN_REVIEW_UNATTRIB_BYLINE="$(tr 'A-Z' 'a-z' <<<"${DRAIN_GUARD_UNATTRIB_BYLINE:-Augustus}")"
  DRAIN_REVIEW_UNATTRIB_BEFORE="${DRAIN_GUARD_ATTRIB_CUTOVER:-2026-08-01T10:40:51Z}"

  mapfile -t DRAIN_REVIEW_LINES < <(jq -r '.comments[]? | select(.body|startswith("[REVIEW]")) | "\(.createdAt)\t\(.body|split("\n")[0])"' <<<"$j" 2>/dev/null)
  DRAIN_REVIEW_VERDICT_COUNT="${#DRAIN_REVIEW_LINES[@]}"
  drain_review_roster_load

  for _rec in "${DRAIN_REVIEW_LINES[@]}"; do
    _at="${_rec%%$'\t'*}"
    _line="${_rec#*$'\t'}"
    _sha="$(grep -oE '@[[:space:]]*[0-9a-f]{7,40}' <<<"$_line" | head -1 | grep -oE '[0-9a-f]{7,40}')"
    _matches=0
    if [ -z "$_sha" ] || [ "${DRAIN_REVIEW_HEAD:0:${#_sha}}" = "$_sha" ]; then
      _matches=1
    fi
    [ "$_matches" = "1" ] || continue

    DRAIN_REVIEW_HEAD_VERDICT_COUNT=$(( DRAIN_REVIEW_HEAD_VERDICT_COUNT + 1 ))
    _reviewer="$(grep -oiE 'reviewer[[:space:]]+[A-Za-z][A-Za-z0-9_-]*' <<<"$_line" | head -1 | awk '{print $2}')"
    _verdict="UNPARSED"
    if [[ "$_line" =~ ^\[REVIEW\][[:space:]]*\*?_*[[:space:]]*NO[[:space:]_-]*GO([^A-Za-z0-9_]|$) ]]; then
      _verdict="NO_GO"
    elif [[ "$_line" =~ ^\[REVIEW\][[:space:]]*\*?_*[[:space:]]*GO([^A-Za-z0-9_]|$) ]]; then
      _verdict="GO"
    fi

    case "$_verdict" in
      NO_GO|NOGO)
        if [ "$(tr 'A-Z' 'a-z' <<<"${_reviewer:-}")" = "$DRAIN_REVIEW_UNATTRIB_BYLINE" ] && [[ "$_at" < "$DRAIN_REVIEW_UNATTRIB_BEFORE" ]]; then
          _unattr_count=$(( _unattr_count + 1 ))
          if [ -z "$_unattr_at" ] || [[ "$_at" > "$_unattr_at" ]]; then
            _unattr_at="$_at"
          fi
        elif [ -n "$_reviewer" ]; then
          [ -z "$_nogo_attr" ] && { _nogo_attr="$_line"; _nogo_by="$_reviewer"; }
        fi
        ;;
      GO)
        if [ -n "$_sha" ]; then
          DRAIN_REVIEW_GO_LINE="$_line"
          DRAIN_REVIEW_GO_SHA="$_sha"
          DRAIN_REVIEW_GO_AT="$_at"
        fi
        if [ -n "$_sha" ] && drain_review_is_registered "$_reviewer" \
           && [ "$(tr 'A-Z' 'a-z' <<<"${_reviewer:-}")" != "$DRAIN_REVIEW_UNATTRIB_BYLINE" ]; then
          if [ -z "$_sup_at" ] || [[ "$_at" > "$_sup_at" ]]; then
            _sup_at="$_at"
            _sup_by="$_reviewer"
          fi
        fi
        ;;
    esac
  done

  DRAIN_REVIEW_UNATTR_COUNT="$_unattr_count"
  DRAIN_REVIEW_UNATTR_AT="$_unattr_at"
  DRAIN_REVIEW_SUPERSEDING_BY="$_sup_by"
  DRAIN_REVIEW_SUPERSEDING_AT="$_sup_at"

  if [ -n "$_nogo_attr" ]; then
    DRAIN_REVIEW_BLOCKING_NOGO=1
    DRAIN_REVIEW_BLOCKING_CLASS="live_nogo"
    DRAIN_REVIEW_BLOCKING_LINE="$_nogo_attr"
    DRAIN_REVIEW_NOGO_BY="$_nogo_by"
    return 0
  fi

  if [ "$_unattr_count" -gt 0 ]; then
    if [ -z "$_sup_at" ] || [[ ! "$_sup_at" > "$_unattr_at" ]]; then
      DRAIN_REVIEW_BLOCKING_NOGO=1
      DRAIN_REVIEW_BLOCKING_CLASS="unwithdrawable_nogo"
      return 0
    fi
  fi

  return 0
}
