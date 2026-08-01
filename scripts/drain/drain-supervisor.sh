#!/usr/bin/env bash
# drain-supervisor.sh — keep the station02 PR-drain at MAX_LANES concurrent lanes.
#
# Runs from cron every minute. Single-instance via flock. Each invocation:
#   1. counts live lane.sh parents
#   2. reads /proc/loadavg AT THAT MOMENT and refuses to launch above the ceiling
#   3. re-classifies a BOUNDED number of previously-burned PRs (see RETRY POLICY)
#   4. launches lanes up to the cap, one per repo, from the queue
#
# Lanes are detached nohup processes and outlive both this script and any agent
# session. The supervisor is a cron job for the same reason: it must restart
# itself if it dies, which a long-lived loop cannot do.
set -uo pipefail

# cron gives a minimal PATH (/usr/bin:/bin). gh, codewith and bun all live under
# $HOME/.bun/bin and $HOME/.local/bin, so without this every gh call and every
# launched lane fails. Measured 2026-07-31: the missing PATH made `gh pr view`
# return empty, which the open-PR check below then read as "PR is not open" and
# burned 212 PRs into the attempted list in a single pass.
export PATH="$HOME/.bun/bin:$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

# The two knobs below take an env override so that the same code path can be run
# as a one-shot priming pass (a large recheck budget, no launches) and under a
# test harness, WITHOUT a second script or a second writer on the shared state.
# Defaults are the production values; an unset environment behaves identically to
# a hardcoded constant.
MAX_LANES="${DRAIN_MAX_LANES:-12}"   # owner's cap for station02 — not a number to improvise on
LAUNCH_SLEEP="${DRAIN_LAUNCH_SLEEP:-2}"
LOAD_CEIL="${DRAIN_LOAD_CEIL:-12.0}" # 1-min loadavg on 20 cores; refuse to launch above this
QUEUE="$HOME/drain-queue.txt"
ATTEMPTED="$HOME/drain-attempted.txt"
STATE="$HOME/drain-state.tsv"
SUPLOG="$HOME/drain-supervisor.log"
MARKER="$HOME/drain-supervisor.marker"
STARVE_MARKER="$HOME/drain-starved.marker"
STALL_COUNT="$HOME/drain-stall-count"
QUEUE_MAX_AGE=1800      # rebuild queue if older than 30 min
LANE="$HOME/lane.sh"
NOATTEMPT_DIR="${DRAIN_NOATTEMPT_DIR:-$HOME/drain-noattempt}"  # lane.sh drops a flag here
                        # when a lane never got as far as looking at the PR

# ---------------------------------------------------------------------------
# RETRY POLICY — why a burned key is no longer burned forever.
#
# The previous picker read `grep -qxF "$key" "$ATTEMPTED" && continue`: a bare
# key, written once at LAUNCH time, never revisited. Three measured consequences,
# all on 2026-07-31:
#   1. a lane that produced NOTHING still burned its key permanently — 10 of 83
#      burned-but-open PRs carried no [REVIEW] comment at all;
#   2. a PROBE FAILURE burned keys wholesale — 212 live PRs in a single pass when
#      gh was absent from cron's PATH;
#   3. a verdict that AGES could not be revisited — 17 cleared keys named a sha
#      that was no longer head, 10 carried "[REBASED] — GO INVALIDATED".
# The queue itself refills from GitHub every 30 min, so the burn set was the only
# thing that grew monotonically: eligible = queue minus attempted went to zero and
# the drain stopped dead at 21:47Z.
#
# THIS IS NOT A BLANKET RETRY, and a blanket retry would be wrong. drusus read
# four current-head NO_GO bodies: two were gate-only, two were substantive and
# would recur identically (contracts#62 breaks verifyApiKey across ~30 consumers;
# contacts#13 hides a now()-to-monotonic-counter change inside a CI-gate PR).
# Roughly half, not all. So the discriminator here is MECHANICAL and narrow:
#
#   a NO_GO whose sha IS STILL HEAD stays burned    — the verdict still describes
#                                                     the code that is there
#   a NO_GO whose sha IS NO LONGER HEAD is eligible — the code moved under it
#
# That returns exactly the class drusus cleared by hand and leaves the 21
# substantive ones burned, without anyone judging a PR body.
#
# ATTEMPTED remains the authoritative burn set so that every existing tool keeps
# working (drain-requeue-authfail.sh removes keys from it; an operator can too).
# STATE is a sidecar carrying class + timestamp + sha + attempt count. A key
# present in STATE but absent from ATTEMPTED has been un-burned by something
# else and is simply eligible — external un-burns are honoured, not fought.
# ---------------------------------------------------------------------------
RECHECK_MAX="${DRAIN_RECHECK_MAX:-10}"  # gh probes spent on re-classification per pass (bounds
                        # pass latency: the supervisor holds the flock and cron
                        # fires every 60s)
COOL_LEGACY=0           # pre-existing bare keys: classify at the first chance
COOL_LAUNCHED=2700      # 45 min — a lane runs ~10-20 min; do not judge it early
COOL_NOGO=1200          # 20 min — re-probe only to see whether head has moved
COOL_GO_OPEN=5400       # 90 min — GO that never merged; retry, do not thrash
COOL_NOARTEFACT=1500    # 25 min — lane produced nothing; the PR was never judged
COOL_UNPARSED=5400      # artefact exists but its verdict line is unreadable
COOL_NOARTEFACT_COLD=21600  # 6h — see the "cap is a RATE LIMIT" note in classify_burned
COOL_GO_OPEN_COLD=21600 # 6h — same rate-limit treatment for a GO that keeps not merging
COOL_NEEDS_REBASE=21600 # 6h — GO at head but the branch conflicts; costs one gh
                        # probe, never a lane, and self-clears once rebased
COOL_DEFAULT=3600
MAX_ATTEMPTS=3          # after this many launches a key is RATE-LIMITED, and LOUDLY.
                        # It is a retirement ONLY for the classes that carry a real
                        # verdict against head (stale / unattributed / unparsed).

# --- INSTRUMENT STALENESS: "the JUDGE moved under it" -------------------------
# The mirror image of the sha rule at the top of this file. That rule says a
# verdict stops being authoritative when THE CODE moves under it. The same is
# true when THE REVIEWER moves under it: a NO_GO produced by a reviewer whose
# gate was subsequently fixed is not evidence about the PR, for exactly the same
# reason a NO_GO at an old sha is not.
#
# MEASURED 2026-08-01, and this is the payload: lane.sh line 131 hardcodes the
# gate as "bun install ; bun run typecheck ; bun test" while telling the reviewer
# to "run the repo's own gates" — a self-contradicting instruction in one
# sentence. Of the 10 repos in the current queue, 7 declare exactly `bun test`
# and 3 declare something else, and all 3 differences are the load-sensitivity
# flags: shield `--parallel=1`, contracts `--timeout 120000`, mementos
# `--isolate --timeout=10000`. hasna/shield#5's NO_GO is that defect caught in
# the act — the reviewer recorded `bun test` -> exit 1 (427 pass, 5 fail) and, in
# the SAME comment, `bun run test` -> exit 0 (432 pass, 0 fail).
#
# DEFAULT 0 = DISABLED, so installing this file changes nothing. Arm it ONLY by
# setting it to the epoch-seconds at which the reviewer gate was actually fixed.
# Arming it without fixing the gate spends one box and one auth profile per key
# to reproduce the identical NO_GO — that is not a bar-lowering, it is a pure
# waste, and it is the reason this is off by default rather than on.
#
# BOUND: fires at most ONCE per key per epoch bump. The re-review posts a new
# verdict whose createdAt is > the epoch, so the very next classification takes
# the ordinary nogo_at_head path and the key stays burned. It cannot loop.
REVIEWER_EPOCH="${DRAIN_REVIEWER_EPOCH:-0}"

# A SECOND STALL SHAPE, and the first version of this file was blind to it.
# Measured 22:40Z: eligible=13, lanes=7 of 12, launched=0 — for three passes in a
# row. NOT starvation (there were keys) and NOT a stopped drain (lanes were
# running), so neither alarm fired and nothing said a word while five lane slots
# sat idle. The cause was one-lane-per-repo: all 13 eligible keys lived in the 7
# repos that already had a lane. That rule is correct and is not being weakened —
# but "we have work, we have capacity, and we launched nothing" has to be
# audible, because it is exactly what a partial stop looks like from outside.
# One such pass is ordinary; a run of them is a stall.
STALL_PASSES=10         # consecutive no-launch passes before this is called a stall
ALARM_INTERVAL=900      # 15 min between starvation posts
ALARM_AGENT="station02-drain-watch"
ALARM_CHANNEL="board"

# HEALTHY profiles only. Measured 2026-07-31 across 40 lane logs: account012,
# 013, 014, 018 and 019 fail 100% of the time with
#   "Your access token could not be refreshed because your refresh token was
#    already used. Please log out and sign in again."
# The lane still starts, runs ~2s, exits rc=1 and produces NO artefact — while
# the PR it was given is already marked attempted. Those five must therefore be
# out of rotation, not merely deprioritised.
#
# CONSEQUENCE FOR THE CAP: only 11 profiles work, so 12 concurrent lanes is not
# reachable. The owner's cap of 12 is a CEILING and stays as written; the pool
# is what actually binds, and the supervisor holds at 11 rather than handing a
# lane a credential known to fail. Re-authing those five restores the 12th lane.
PROFILES="account001 account002 account003 account004 account005 account006 account007 account008 account009 account010 account011"

log() { echo "[$(date -u +%FT%TZ)] $*" >> "$SUPLOG"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
# shellcheck source=scripts/drain/drain-review-supersession.sh
. "$SCRIPT_DIR/drain-review-supersession.sh"

# A drain that stops must SAY it stopped. On 2026-07-31 it logged
# "no eligible queue entry" every minute for twenty minutes and nothing surfaced
# it; the outage was found by a human reading a log. Rate-limited so the channel
# is not a keystroke log, and self-clearing so the recovery is visible too.
alarm() {
  local msg="$1" now last
  now=$(date +%s)
  last=$(stat -c %Y "$STARVE_MARKER" 2>/dev/null || echo 0)
  log "ALARM $msg"
  if [ $(( now - last )) -ge "$ALARM_INTERVAL" ]; then
    date -u +%FT%TZ > "$STARVE_MARKER"
    conversations send --channel "$ALARM_CHANNEL" --from "$ALARM_AGENT" \
      "[DRAIN-ALARM] station02: $msg" >/dev/null 2>&1 \
      || log "ALARM post failed — logged only (channel unreachable)"
  fi
}
alarm_clear() {
  local msg="$1"
  [ -f "$STARVE_MARKER" ] || return 0
  rm -f "$STARVE_MARKER"
  log "ALARM CLEARED $msg"
  conversations send --channel "$ALARM_CHANNEL" --from "$ALARM_AGENT" \
    "[DRAIN-OK] station02: $msg" >/dev/null 2>&1 || true
}

# Lock fd is 9. Every launched lane MUST close it (9>&- on the nohup below):
# a child inherits open fds, and an inherited fd keeps the flock held for the
# child's entire lifetime — which for a lane is 10+ minutes. That silently turns
# every later cron pass into a no-op that exits 0 with no output. Measured
# 2026-07-31: the first version of this script launched 6 lanes and then never
# ran again, while cron fired correctly every minute.
# The lock file name is versioned so a fix cannot be blocked by lanes still
# holding the previous generation's fd.
exec 9>"$HOME/.drain-supervisor-v2.lock"
flock -n 9 || exit 0

touch "$ATTEMPTED" "$STATE"
date -u +%FT%TZ > "$MARKER"

# Return any PR burned by a dead auth profile to the queue. Cost is bounded:
# processed logs are renamed .authfail, so this is a no-op once quiet.
"$HOME/drain-requeue-authfail.sh" 2>/dev/null || true

# --- state sidecar helpers ---------------------------------------------------
# Row: key <TAB> class <TAB> epoch <TAB> sha <TAB> attempts
state_get() {  # $1=key -> "class<TAB>epoch<TAB>sha<TAB>attempts" or empty
  awk -F'\t' -v k="$1" 'BEGIN{OFS="\t"} $1==k {c=$2;t=$3;s=$4;a=$5}
       END{ if (c!="") print c,t,s,a }' "$STATE"
}
state_set() {  # $1=key $2=class $3=sha $4=attempts
  local now; now=$(date +%s)
  awk -F'\t' -v k="$1" '$1!=k' "$STATE" > "$STATE.tmp" 2>/dev/null || : > "$STATE.tmp"
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$now" "$3" "$4" >> "$STATE.tmp"
  mv "$STATE.tmp" "$STATE"
}
# Remove a key from the burn set. Note the deliberate `|| true`: `grep -v` exits 1
# when it emits NO lines, and `grep ... > tmp && mv` therefore silently skips the
# move exactly when the file would become empty — leaving the burn set unchanged.
unburn() {  # $1=key $2=reason
  grep -vxF "$1" "$ATTEMPTED" > "$ATTEMPTED.tmp" || true
  mv "$ATTEMPTED.tmp" "$ATTEMPTED"
  log "UNBURN $1 — $2"
}

# Rewrite ONLY the class and attempts of an existing row, PRESERVING its epoch.
# state_set stamps `now`, which would push the key out by a fresh cooldown — a
# refund must not delay the re-probe it exists to enable.
state_set_class_att() {  # $1=key $2=class $3=attempts
  awk -F'\t' -v k="$1" -v c="$2" -v a="$3" 'BEGIN{OFS="\t"} $1==k{$2=c; $5=a} {print}' \
    "$STATE" > "$STATE.tmp" && mv "$STATE.tmp" "$STATE"
}

# --- a LAUNCH that never became an ATTEMPT -----------------------------------
# THE INVARIANT: only a lane that actually got to LOOK at a PR may count as an
# attempt. `att` is incremented optimistically at LAUNCH because the lane is
# nohup'd, disowned, and has its output sent to /dev/null — the supervisor can
# NEVER read its exit code, so there is nothing to wait for and exit 30 has no
# possible receiver.
#
# lane.sh exits 30 with FETCH_BLOCKED_NOT_AN_ATTEMPT when the fetch itself was
# blocked. That is ENVIRONMENTAL and says nothing about the PR, yet three of them
# reach MAX_ATTEMPTS and permanently retire a PR that was NEVER REVIEWED AT ALL.
# Same shape as the 212-key incident — a probe that could not run mutating state
# on failure — one level up, and named as such by the parent task.
#
# WHY NOT GREP THE LANE LOG, which is the obvious fix and is WRONG: lane.sh
# closes its block with `} >> "$LOG"` — APPEND, not truncate. A log accumulates
# every run of that PR, so the marker's presence means "a blocked fetch happened
# at some point", NOT "the last run was blocked". Grepping it would refund an
# attempt forever on any PR that was ever blocked once.
#
# So lane.sh drops one UNIQUELY NAMED flag per blocked fetch and this consumes
# exactly the flags it can see, BY NAME. A flag written by a lane after the glob
# survives to the next pass instead of being deleted uncredited, so the lane and
# the supervisor need no lock between them.
keysafe() { printf '%s' "$1" | tr '/#' '__'; }
refund_noattempt() {  # $1=key $2=class $3=attempts -> echoes "class<TAB>attempts"
  local key="$1" cls="$2" att="$3" n=0 f new
  for f in "$NOATTEMPT_DIR/$(keysafe "$key")."*; do
    [ -e "$f" ] || continue
    n=$(( n + 1 )); rm -f "$f"
  done
  if [ "$n" -gt 0 ]; then
    new=$(( att - n )); [ "$new" -lt 0 ] && new=0
    # A key already retired by blocked fetches is RECOVERED, not merely slowed:
    # exhausted is terminal at the scan_round gate, so refunding below the cap
    # without re-classing would leave it retired with a corrected counter.
    if [ "$cls" = "exhausted" ] && [ "$new" -lt "$MAX_ATTEMPTS" ]; then cls="noartefact"; fi
    state_set_class_att "$key" "$cls" "$new"
    log "REFUND $key — $n blocked fetch(es) never reached the PR; attempts $att -> $new class=$cls"
    att="$new"
  fi
  printf '%s\t%s\n' "$cls" "$att"
}

cooldown_for() {
  case "$1" in
    legacy)     echo "$COOL_LEGACY" ;;
    launched)   echo "$COOL_LAUNCHED" ;;
    nogo)       echo "$COOL_NOGO" ;;
    go_open)    echo "$COOL_GO_OPEN" ;;
    noartefact) echo "$COOL_NOARTEFACT" ;;
    noartefact_cold) echo "$COOL_NOARTEFACT_COLD" ;;
    go_open_cold)    echo "$COOL_GO_OPEN_COLD" ;;
    needs_rebase)    echo "$COOL_NEEDS_REBASE" ;;
    unreviewable)    echo "$COOL_DEFAULT" ;;   # terminal at the gate below; value unused
    unparsed)   echo "$COOL_UNPARSED" ;;
    *)          echo "$COOL_DEFAULT" ;;
  esac
}

# --- classification ----------------------------------------------------------
# One gh call. Decides what a burned key actually IS, from the PR's own artefacts.
# Echoes: eligible:<why> | blocked:<why> | probefail
# NEVER burns anything on a probe failure — a failed probe is not evidence about
# its subject. That rule is what the 212-key incident already taught one level
# down; this applies it to re-classification.
# NOTE ON THE SUBSHELL, because it already bit me once: this function is invoked
# as d="$(classify_burned ...)", so it runs in a SUBSHELL. Anything it writes to
# a FILE (state_set) survives; anything it writes to a VARIABLE or an array does
# NOT. The head sha therefore leaves through the echoed decision string rather
# than through an array assignment, which silently vanished.
# --- is this repo capable of receiving a verdict AT ALL? ---------------------
# THE DISCRIMINATOR THE "no artefact" BRANCH WAS MISSING. It could tell "a lane
# ran and left no review" but not WHY, so it treated every such key as transient
# and retried it. An ARCHIVED repo is read-only and its issues are locked, so
# `gh pr comment` can never succeed there — no lane will EVER produce an artefact,
# no matter how many times it is run.
#
# Measured 2026-08-01 on hasnaxyz/community-openclaw (the only archived repo of
# the 18 in the queue): the reviewer completed a full review and then got
#   GraphQL: Repository was archived so is read-only and unable to create comment
#   because issue is locked (addComment)
# openclaw#29 alone accumulated 6 LANE STARTs and 0 [REVIEW] comments, ~10-20 min
# of a codewith agent and an auth profile each, and #29/#30/#31 then hit the
# attempt cap and were retired as exhausted_noartefact — permanently discarded
# WITHOUT EVER HAVING BEEN REVIEWED. Both halves are wrong and this fixes both:
# they are not transient (so stop retrying) and they were never judged (so the
# retirement was not a verdict).
#
# GitHub IS THE SOURCE OF TRUTH here, deliberately: the alternative is adding the
# repo name to the hardcoded EXCLUDE_REPOS list, which goes stale silently the
# moment a repo is archived or unarchived and which nothing re-validates.
#
# FAIL-OPEN, and that is the same rule as everywhere else in this file: a probe
# that could not run is not evidence about its subject. An unreachable gh returns
# neither true nor false, and the key keeps its ordinary transient handling
# rather than being blocked on a network blip.
#
# NO IN-MEMORY CACHE, ON PURPOSE. classify_burned is invoked as $( ), so it runs
# in a SUBSHELL and an associative-array cache written here would silently vanish
# between calls — the exact trap already documented for the head sha below. The
# call is made only on the nrev==0 branch, which is rare, and RECHECK_MAX bounds
# it to at most that many probes per pass. A cache that does not work is worse
# than no cache, because it reads as though it does.
repo_is_archived() {  # $1=owner/repo -> rc 0 ONLY when GitHub says true
  local v
  v="$(gh repo view "$1" --json isArchived --jq '.isArchived' 2>/dev/null)"
  [ "$v" = "true" ]
}

declare -A PROBED_HEAD
classify_burned() {  # $1=repo $2=prn $3=key $4=attempts
  local repo="$1" prn="$2" key="$3" att="$4"
  local j rc st dr head nrev line verdict reviewer _l _s line_at _rec _at _vt mrg
  local -a _rl

  j="$(gh pr view "$prn" --repo "$repo" \
        --json state,isDraft,headRefOid,comments,mergeable 2>/dev/null)"
  rc=$?
  if [ "$rc" -ne 0 ] || [ -z "$j" ]; then echo "probefail"; return 0; fi

  st="$(jq -r '.state // ""' <<<"$j" 2>/dev/null)"
  dr="$(jq -r '.isDraft // false' <<<"$j" 2>/dev/null)"
  mrg="$(jq -r '.mergeable // "UNKNOWN"' <<<"$j" 2>/dev/null)"
  head="$(jq -r '.headRefOid // ""' <<<"$j" 2>/dev/null)"
  nrev="$(jq -r '[.comments[]?.body | select(startswith("[REVIEW]"))] | length' <<<"$j" 2>/dev/null)"
  [ -z "$st" ] && { echo "probefail"; return 0; }

  # Settled: closed, merged or converted to draft. Terminal, and this is the
  # guarantee that must not regress — a correctly closed or merged PR is never
  # re-attempted.
  if [ "$st" != "OPEN" ] || [ "$dr" = "true" ]; then
    state_set "$key" settled "$head" "$att"; echo "blocked:settled $head"; return 0
  fi

  # No artefact at all: the lane ran (or died) and left no review on the PR. The
  # PR was never actually judged, so the burn recorded a lane failure as a verdict.
  if [ "${nrev:-0}" = "0" ]; then
    # TERMINAL, and only GitHub can say so. An archived repo cannot accept a
    # comment, so "no artefact" here is a permanent property of the repository
    # rather than a lane failure. Blocked, and never retried — the opposite
    # treatment from every other no-artefact key below.
    if repo_is_archived "$repo"; then
      state_set "$key" unreviewable "$head" "$att"; echo "blocked:unreviewable_archived $head"; return 0
    fi
    # NOT TERMINAL, and this is the clause the parent task names: a lane that
    # produced no artefact never got a fair verdict, so retiring the key discards
    # work that was never tried. THE ATTEMPT CAP THEREFORE BECOMES A RATE LIMIT
    # RATHER THAN A RETIREMENT — past the cap the key still comes back, but on a
    # 6h cooldown instead of 25 min, so a genuinely broken PR costs ~4 lanes a day
    # instead of spinning, and a transient environmental failure still recovers.
    #
    # WHY THIS DOES NOT REINTRODUCE THE THRASH IT REPLACES: the one case that
    # could never recover — an archived repo — is caught terminally above, and it
    # was the whole of the observed thrash. Logged at WARN volume because an
    # unbounded attempt counter on a PR nothing can review is a real cost and must
    # be visible rather than merely bounded.
    if [ "$att" -ge "$MAX_ATTEMPTS" ]; then
      # Cold retry is a new rate-limit window, not an increment past the ceiling.
      # Reset deliberately so the next launch is logged as 1/MAX rather than
      # producing the misleading and previously reachable attempt=4/3 shape.
      state_set "$key" noartefact_cold "$head" "0"
      echo "eligible:noartefact_cold_reset $head"; return 0
    fi
    state_set "$key" noartefact "$head" "$att"; echo "eligible:noartefact $head"; return 0
  fi

  # The guard and supervisor share this head-verdict supersession decision. If
  # the guard would block the existing head verdict set because a NO_GO remains
  # live, a lane cannot merge this SHA and must not be launched.
  if ! drain_review_supersession_decision "$j"; then echo "probefail"; return 0; fi
  if [ "${DRAIN_REVIEW_BLOCKING_NOGO:-0}" = "1" ]; then
    log "BLOCK $key — shared supersession decision: ${DRAIN_REVIEW_BLOCKING_CLASS} at head ${head:0:12}"
    state_set "$key" nogo "$head" "$att"; echo "blocked:nogo_at_head $head"; return 0
  fi

  # SELECT THE LAST VERDICT THAT DESCRIBES HEAD — the SAME selection the merge
  # guard makes. This used to read `| last` unconditionally while
  # drain-merge-guard.sh selected head-describing verdicts, and the two checks
  # drifted apart the moment P2-2 was fixed in the guard alone. The harmful shape
  # is a STALE GO POSTED LAST while a NO_GO describes head: the picker classed it
  # go_open, un-burned it, and spent a lane and an auth profile; the guard then
  # correctly refused the merge; att incremented; three rounds and the PR is
  # exhausted — permanently retired for being reviewed correctly.
  #
  # Aligning them is right rather than merely symmetrical, and the picker's own
  # comments say why: nogo_at_head STAYS BURNED because "the drain reviews and
  # merges; it only fixes when a lane judges the remedy small", and the ~22 live
  # NO_GOs need a FIXER, not another reviewer. A picker that spends a lane on a
  # PR whose head carries a live NO_GO contradicts that intent AND cannot succeed,
  # because the guard will refuse the merge it was sent to perform.
  #
  # A verdict carrying NO sha is treated as describing head — deliberately
  # conservative, identical to the guard: it cannot be shown stale, so a NO_GO of
  # that shape still blocks rather than being silently dropped.
  # Each record is "createdAt<TAB>first line of body". The timestamp rides along
  # so the instrument-staleness test below can ask WHEN the selected verdict was
  # produced; `line` still holds only the first line, so every downstream grep is
  # unchanged.
  mapfile -t _rl < <(jq -r '.comments[]? | select(.body|startswith("[REVIEW]")) | "\(.createdAt)\t\(.body|split("\n")[0])"' <<<"$j" 2>/dev/null)
  line=""; line_at=""
  for _rec in "${_rl[@]}"; do
    _at="${_rec%%$'\t'*}"; _l="${_rec#*$'\t'}"
    _s="$(grep -oE '@[[:space:]]*[0-9a-f]{7,40}' <<<"$_l" | head -1 | grep -oE '[0-9a-f]{7,40}')"
    if [ -z "$_s" ] || [ "${head:0:${#_s}}" = "$_s" ]; then line="$_l"; line_at="$_at"; fi
  done
  # Every verdict on the PR names a sha that is no longer head: the code moved
  # out from under all of them, so none is authoritative and the PR is reviewable
  # again. This subsumes the old per-verdict staleness branch, which is now
  # unreachable by construction and has been removed.
  if [ -z "$line" ]; then
    if [ "$att" -ge "$MAX_ATTEMPTS" ]; then
      state_set "$key" exhausted "$head" "$att"; echo "blocked:exhausted_stale $head"; return 0
    fi
    state_set "$key" nogo "$head" "$att"; echo "eligible:verdict_stale $head"; return 0
  fi
  reviewer="$(grep -oiE 'reviewer[[:space:]]+[A-Za-z][A-Za-z0-9_-]*' <<<"$line" | head -1 | awk '{print $2}')"
  case "$line" in
    *NO_GO*) verdict="NO_GO" ;;
    *GO*)    verdict="GO" ;;
    *)       verdict="UNPARSED" ;;
  esac

  if [ "$verdict" = "NO_GO" ]; then
    # Clause 2 of the merge policy, applied to the picker as well: a verdict that
    # names NO AUTHOR is not a valid block. Measured on hasna/codewith#462 — an
    # unattributed NO_GO nobody can withdraw, because withdrawal requires the
    # author. Left as a block it is a permanent, unliftable veto.
    if [ -z "$reviewer" ]; then
      if [ "$att" -ge "$MAX_ATTEMPTS" ]; then
        state_set "$key" exhausted "$head" "$att"; echo "blocked:exhausted_unattributed $head"; return 0
      fi
      state_set "$key" nogo "$head" "$att"; echo "eligible:nogo_unattributed $head"; return 0
    fi
    # NOTE: the per-verdict staleness branch that stood here is GONE, not lost.
    # Selection above never yields a verdict that fails the staleness test, so
    # this branch was unreachable by construction — the same removal the merge
    # guard made for the same reason. The stale path is still exercised, by the
    # empty-selection case above (eligible:verdict_stale) and by test T4.
    # THE JUDGE MOVED UNDER IT. Same test as the sha rule, on the other axis: a
    # verdict produced BEFORE the reviewer gate was fixed was produced by an
    # instrument we have since replaced, so it is not evidence about this PR.
    # Disabled unless REVIEWER_EPOCH is armed; see the note at the top of the file
    # for why arming it without fixing the gate is pure waste.
    #
    # FAIL-CLOSED on an unparsable timestamp, which is the opposite of the
    # fail-OPEN rule used for probes elsewhere, and deliberately so: a probe that
    # cannot run is not evidence either way, but a timestamp that cannot be read
    # must not be allowed to LIFT a live block. Unknown age keeps the burn.
    if [ "$REVIEWER_EPOCH" -gt 0 ] && [ -n "$line_at" ]; then
      _vt="$(date -u -d "$line_at" +%s 2>/dev/null || echo 0)"
      [[ "$_vt" =~ ^[0-9]+$ ]] || _vt=0
      if [ "$_vt" -gt 0 ] && [ "$_vt" -lt "$REVIEWER_EPOCH" ]; then
        if [ "$att" -ge "$MAX_ATTEMPTS" ]; then
          state_set "$key" exhausted "$head" "$att"; echo "blocked:exhausted_pre_epoch $head"; return 0
        fi
        state_set "$key" nogo "$head" "$att"; echo "eligible:nogo_pre_epoch $head"; return 0
      fi
    fi
    # Attributed NO_GO at the current head, from the current reviewer generation.
    # STAYS BURNED. This is the clause that keeps the drain off the ~half of
    # NO_GOs that are substantive.
    state_set "$key" nogo "$head" "$att"; echo "blocked:nogo_at_head $head"; return 0
  fi

  if [ "$verdict" = "GO" ]; then
    # A GO on a PR that is still open means the disposition never completed —
    # the merge did not happen. Retrying finishes the job rather than reproducing
    # a refutation.
    #
    # THE CAP IS A RATE LIMIT HERE, NOT A RETIREMENT — the same correction the
    # noartefact branch above already carries, applied to the branch that needed
    # it just as badly. MEASURED 2026-08-01: hasna/domains#18 carries FOUR GO
    # verdicts at head c8fe9346 (22:12, 22:38, 23:24, 00:10Z) and was retired at
    # 00:54:03Z as exhausted_go_open. A PR the drain reviewed four times and
    # passed four times is now permanently discarded, and the thing that
    # discarded it was an attempt counter rather than any verdict.
    #
    # Retiring on a GO is the WRONG DIRECTION on the only axis that matters: the
    # review passed, and what failed was the DISPOSITION (the merge). Those are
    # different failures with different remedies, and the attempt counter cannot
    # tell them apart. The spin the old cap guarded against is real, so the guard
    # stays — as a 6h cooldown, which bounds a permanently-unmergeable PR to ~4
    # lanes a day instead of retiring a mergeable one forever.
    #
    # This does NOT lower the review bar: it never turns a NO_GO into a GO, and
    # every launch it permits still faces the merge guard unchanged.
    #
    # BUT FIRST — A GO THE MERGE GUARD CAN NEVER SATISFY. GitHub says the branch
    # conflicts with its base, so `gh pr merge` will refuse no matter how many
    # times a lane reviews it. A REVIEWER cannot fix a conflict; a REBASE can.
    # Sending another reviewer is not a retry, it is the wrong worker.
    #
    # MEASURED 2026-08-01 on hasna/domains#18 — the single key that motivated the
    # rate-limit above, which turned out to need this instead. Its lane log:
    #   bun install -> 0; bun run typecheck -> 0; bun test -> 0, 401 pass / 0 fail
    #   gh pr merge ... --match-head-commit c8fe9346 -> exit 1
    #   Reason: PR is CONFLICTING / DIRTY (main already has .editorconfig and
    #   src/test/setup-env.ts; both sides added bunfig.toml)
    # Three lanes did exactly that, each burning a box and an auth profile to
    # re-derive the same refusal, and the third retired the PR as
    # exhausted_go_open — which reads as a verdict and was an accounting artifact.
    #
    # NOT TERMINAL, and that is the point of using a class rather than the
    # exhausted bucket: it re-probes on the 6h cooldown at a cost of ONE gh call,
    # and the moment somebody rebases the branch the next probe sees MERGEABLE and
    # it flows to go_open and launches by itself. Blocked, visible, self-healing,
    # and zero boxes while it waits.
    #
    # FAIL-OPEN on UNKNOWN: GitHub computes mergeability asynchronously and
    # answers UNKNOWN while it is still thinking. Same rule as every other probe
    # in this file — an answer that is not yet an answer is not evidence, so the
    # key keeps its ordinary handling rather than being blocked on a race.
    if [ "$mrg" = "CONFLICTING" ]; then
      state_set "$key" needs_rebase "$head" "$att"; echo "blocked:needs_rebase $head"; return 0
    fi
    if [ "$att" -ge "$MAX_ATTEMPTS" ]; then
      state_set "$key" go_open_cold "$head" "0"
      echo "eligible:go_open_cold_reset $head"; return 0
    fi
    state_set "$key" go_open "$head" "$att"; echo "eligible:go_open $head"; return 0
  fi

  if [ "$att" -ge "$MAX_ATTEMPTS" ]; then
    state_set "$key" exhausted "$head" "$att"; echo "blocked:exhausted_unparsed $head"; return 0
  fi
  state_set "$key" unparsed "$head" "$att"; echo "eligible:unparsed $head"; return 0
}

# --- current lanes -----------------------------------------------------------
# Count lane.sh PARENTS, never codewith processes: one lane spawns ~1.6 codewith
# processes, so a process count is not a lane count.
#
# Counting method matters. `pgrep -f "bash $LANE"` matches ANY process whose
# command line merely CONTAINS that string — including a shell running a
# diagnostic that greps for it. Measured 2026-07-31: a probe reported 11 lanes
# where 9 were running, because the probe's own command line contained the
# pattern. Here that inflation would make the supervisor believe it is at cap and
# silently UNDER-launch. So: walk /proc and require the cmdline to START with the
# lane binary, which no observer of the lanes can accidentally satisfy.
LANE_PIDS=()
for _p in /proc/[0-9]*; do
  _pid="${_p#/proc/}"
  # Braces + redirect: the "No such file" comes from the SHELL's redirection when
  # a pid exits mid-walk, not from tr, so 2>/dev/null on tr alone never silenced
  # it. It spammed the cron log on every pass.
  _cl="$( { tr '\0' ' ' < "$_p/cmdline"; } 2>/dev/null )" || continue
  case "$_cl" in
    "bash $LANE "*) LANE_PIDS+=("$_pid") ;;
  esac
done
NLANES=${#LANE_PIDS[@]}

BUSY_REPOS=""; BUSY_PROFILES=""
for p in "${LANE_PIDS[@]}"; do
  cl="$( { tr '\0' ' ' < "/proc/$p/cmdline"; } 2>/dev/null )" || continue
  # cmdline: bash /home/hasna/lane.sh <owner/repo> <pr> <dir> <profile>
  BUSY_REPOS="$BUSY_REPOS $(awk '{print $3}' <<<"$cl")"
  BUSY_PROFILES="$BUSY_PROFILES $(awk '{print $6}' <<<"$cl")"
done

LOAD1="$(cut -d' ' -f1 /proc/loadavg)"
NEED=$(( MAX_LANES - NLANES ))

# --- queue -------------------------------------------------------------------
NOW=$(date +%s)
QAGE=$(( NOW - $(stat -c %Y "$QUEUE" 2>/dev/null || echo 0) ))
if [ ! -s "$QUEUE" ] || [ "$QAGE" -gt "$QUEUE_MAX_AGE" ]; then
  log "queue empty or stale (age=${QAGE}s) — rebuilding"
  "$HOME/drain-queue-build.sh" >> "$SUPLOG" 2>&1
fi
[ -s "$QUEUE" ] || { log "queue still empty after rebuild — drain may be complete"; exit 0; }

# --- PHASE A: eligibility, including bounded re-classification ----------------
# Two priority rounds over the queue so the recheck budget is not consumed by
# routine nogo re-probes while pre-existing bare keys go unexamined:
#   round 1 — keys with NO state row (the legacy burn set)
#   round 2 — everything else whose cooldown has elapsed
ELIGIBLE=()
declare -A SEEN
RECHECKS=0
BLOCKED_BY=""

scan_round() {  # $1 = "legacy" | "aged"
  local want="$1" repo prn dir key row cls ts sha att cool d out now rfd
  while IFS=$'\t' read -r repo prn dir; do
    [ -z "$repo" ] && continue
    key="$repo#$prn"
    [ -n "${SEEN[$key]:-}" ] && continue

    if ! grep -qxF "$key" "$ATTEMPTED"; then
      SEEN["$key"]=1; ELIGIBLE+=("$repo	$prn	$dir"); continue
    fi

    row="$(state_get "$key")"
    if [ -z "$row" ]; then cls="legacy"; ts=0; sha=""; att=0
    else IFS=$'\t' read -r cls ts sha att <<<"$row"; fi
    att="${att:-0}"; ts="${ts:-0}"

    # Refund BEFORE the terminal gate below, so a key already retired by blocked
    # fetches is recovered rather than read as settled business.
    rfd="$(refund_noattempt "$key" "$cls" "$att")"
    IFS=$'\t' read -r cls att <<<"$rfd"

    # TERMINAL CLASSES — skipped before any recheck budget is spent on them.
    # 'unreviewable' joins settled/exhausted because an archived repo does not
    # become un-archived by being probed again, and a key that can never yield an
    # artefact must not consume a gh probe that a live key could have used.
    case "$cls" in
      settled|exhausted|unreviewable) SEEN["$key"]=1; BLOCKED_BY="$BLOCKED_BY $cls"; continue ;;
    esac

    if [ "$want" = "legacy" ] && [ "$cls" != "legacy" ]; then continue; fi
    if [ "$want" = "aged" ]   && [ "$cls"  = "legacy" ]; then continue; fi

    cool="$(cooldown_for "$cls")"
    now=$(date +%s)
    if [ $(( now - ts )) -lt "$cool" ]; then
      SEEN["$key"]=1; BLOCKED_BY="$BLOCKED_BY $cls"; continue
    fi
    if [ "$RECHECKS" -ge "$RECHECK_MAX" ]; then continue; fi   # retry next pass
    RECHECKS=$(( RECHECKS + 1 ))

    out="$(classify_burned "$repo" "$prn" "$key" "$att")"
    d="${out%% *}"
    [ "$out" != "$d" ] && PROBED_HEAD["$key"]="${out#* }"
    case "$d" in
      eligible:*)
        row="$(state_get "$key")"
        if [ -n "$row" ]; then
          IFS=$'\t' read -r cls ts sha att <<<"$row"
          att="${att:-0}"
        fi
        SEEN["$key"]=1
        unburn "$key" "${d#eligible:} (attempt $(( att + 1 ))/$MAX_ATTEMPTS)"
        ELIGIBLE+=("$repo	$prn	$dir")
        ;;
      probefail)
        # No state change. The key stays burned exactly as it was and is retried
        # on a later pass — a probe that could not run must not mutate state.
        log "WATCHDOG reclassify probe failed for $key — state unchanged"
        ;;
      *)
        SEEN["$key"]=1; BLOCKED_BY="$BLOCKED_BY ${d#blocked:}"
        [ "${d#blocked:}" != "settled" ] && [ "${d#blocked:}" != "nogo_at_head" ] \
          && log "BLOCKED $key — ${d#blocked:}"
        ;;
    esac
  done < "$QUEUE"
}

scan_round legacy
scan_round aged

NELIG=${#ELIGIBLE[@]}
NQUEUE=$(wc -l < "$QUEUE")
NBURN=$(wc -l < "$ATTEMPTED")

# --- starvation is LOUD ------------------------------------------------------
# STARVATION is knowable before the launch loop: there is simply nothing to pick.
# "STOPPED" is NOT — it needs the launch loop's result, because "0 lanes running"
# is the normal state of a drain that is about to start one. Alarming on it here
# would fire on every healthy cold start, and an alarm that cries wolf on the
# healthy case is how the real one gets ignored. It is asserted after the loop.
log "eligible=$NELIG queued=$NQUEUE burned=$NBURN lanes=$NLANES/$MAX_LANES rechecks=$RECHECKS load=$LOAD1"

if [ "$NELIG" -eq 0 ] && [ "$NEED" -gt 0 ]; then
  alarm "DRAIN STARVED — 0 eligible of $NQUEUE queued, $NBURN burned, $NLANES/$MAX_LANES lanes, rechecks=$RECHECKS/$RECHECK_MAX this pass. Blocked by:$(tr ' ' '\n' <<<"$BLOCKED_BY" | sed '/^$/d' | sort | uniq -c | sort -rn | tr '\n' ' ')"
  exit 0
fi

if [ "$NEED" -le 0 ]; then
  alarm_clear "at cap: $NLANES/$MAX_LANES lanes, eligible=$NELIG"
  log "lanes=$NLANES/$MAX_LANES load=$LOAD1 — at cap, nothing to do"
  exit 0
fi

if awk -v l="$LOAD1" -v c="$LOAD_CEIL" 'BEGIN{exit !(l>c)}'; then
  # Not an alarm: the ceiling doing its job is the system working, and lanes are
  # running. Do not clear either — load shedding says nothing about starvation.
  log "lanes=$NLANES/$MAX_LANES load=$LOAD1 > ceiling $LOAD_CEIL — refusing to launch"
  exit 0
fi

# --- PHASE B: launch ---------------------------------------------------------
launched=0
GHFAIL=0
SKIP_BUSY_REPO=0
NOPROFILE=0
for entry in "${ELIGIBLE[@]}"; do
  [ "$launched" -ge "$NEED" ] && break
  IFS=$'\t' read -r repo prn dir <<<"$entry"
  key="$repo#$prn"

  # one lane per repo: a merge moves the base under any sibling lane, and two
  # lanes fetching into one checkout contend on the git index
  grep -qw -- "$repo" <<<"$BUSY_REPOS" && { SKIP_BUSY_REPO=$(( SKIP_BUSY_REPO + 1 )); continue; }

  # Verify the PR is STILL open — the queue can be up to 30 min stale and other
  # lanes merge things out from under it.
  #
  # This check MUST distinguish "the check ran and the PR is closed" from "the
  # check could not run". Conflating them is what destroyed the queue on the
  # first deploy: a failing gh returned an empty string, which compared unequal
  # to OPEN, so 212 live PRs were permanently marked attempted. A failed probe
  # is never evidence about its subject.
  head_sha="${PROBED_HEAD[$key]:-}"
  if [ -n "$head_sha" ]; then
    st="OPEN"; dr="false"; ghrc=0          # just probed during re-classification
  else
    state="$(gh pr view "$prn" --repo "$repo" --json state,isDraft,headRefOid \
              --jq '"\(.state)\t\(.isDraft)\t\(.headRefOid)"' 2>/dev/null)"
    ghrc=$?
    st="$(cut -f1 <<<"$state")"; dr="$(cut -f2 <<<"$state")"; head_sha="$(cut -f3 <<<"$state")"
  fi

  if [ "$ghrc" -ne 0 ] || [ -z "$st" ]; then
    # Probe failure: do NOT mark attempted, do NOT launch. Emit a watchdog line,
    # because a supervisor that goes quiet is indistinguishable from a drained
    # queue.
    GHFAIL=$(( GHFAIL + 1 ))
    log "WATCHDOG gh probe failed for $key (rc=$ghrc) — not marking attempted"
    if [ "$GHFAIL" -ge 3 ]; then
      alarm "gh probe failed ${GHFAIL}x in one pass — aborting pass, queue left intact"
      break
    fi
    continue
  fi

  if [ "$st" != "OPEN" ] || [ "$dr" = "true" ]; then
    log "skip $key — state=$st draft=$dr (probe ok)"
    grep -qxF "$key" "$ATTEMPTED" || echo "$key" >> "$ATTEMPTED"
    state_set "$key" settled "$head_sha" "0"
    continue
  fi

  # Pick a profile not currently held by a running lane.
  profile=""
  for p in $PROFILES; do
    grep -qw -- "$p" <<<"$BUSY_PROFILES" || { profile="$p"; break; }
  done
  [ -z "$profile" ] && { NOPROFILE=1; log "no free auth profile — holding at $NLANES lanes"; break; }

  row="$(state_get "$key")"; att=0
  [ -n "$row" ] && att="$(cut -f4 <<<"$row")"
  att=$(( ${att:-0} + 1 ))

  grep -qxF "$key" "$ATTEMPTED" || echo "$key" >> "$ATTEMPTED"
  state_set "$key" launched "$head_sha" "$att"
  # 9>&- is load-bearing: without it the lane inherits the flock fd and holds the
  # supervisor's lock until it exits, no-oping every subsequent cron pass.
  nohup "$LANE" "$repo" "$prn" "$dir" "$profile" >/dev/null 2>&1 9>&- &
  disown
  log "LAUNCH $key dir=$dir profile=$profile attempt=$att/$MAX_ATTEMPTS sha=${head_sha:0:8} (lane $((NLANES+launched+1))/$MAX_LANES, load=$LOAD1)"

  BUSY_REPOS="$BUSY_REPOS $repo"
  BUSY_PROFILES="$BUSY_PROFILES $profile"
  launched=$(( launched + 1 ))
  sleep "$LAUNCH_SLEEP"
done

# Now the launch result is known, so the two stall shapes can be told apart and
# both asserted honestly.
STALLS=$(cat "$STALL_COUNT" 2>/dev/null || echo 0)
[[ "$STALLS" =~ ^[0-9]+$ ]] || STALLS=0

if [ "$launched" -eq 0 ] && [ "$NEED" -gt 0 ] && [ "$NELIG" -gt 0 ]; then
  STALLS=$(( STALLS + 1 ))
  if   [ "$NOPROFILE" = "1" ];        then why="auth pool exhausted (${#LANE_PIDS[@]} lanes hold every healthy profile)"
  elif [ "$SKIP_BUSY_REPO" -gt 0 ];   then why="all $NELIG eligible keys are in repos that already have a lane (one-lane-per-repo), $SKIP_BUSY_REPO skipped"
  elif [ "$GHFAIL" -gt 0 ];           then why="gh probe failed ${GHFAIL}x"
  else                                     why="unknown — investigate"; fi
  log "NO-LAUNCH pass ${STALLS}/${STALL_PASSES}: $why"
  if [ "$STALLS" -ge "$STALL_PASSES" ]; then
    alarm "DRAIN STALLED — $STALLS consecutive passes launched nothing while $NELIG keys were eligible and $NEED lane slots were free. Reason: $why. lanes=$NLANES/$MAX_LANES load=$LOAD1"
    STALLS=0
  fi
elif [ "$launched" -eq 0 ] && [ "$NLANES" -eq 0 ] && [ "$NELIG" -gt 0 ]; then
  alarm "DRAIN STOPPED — $NELIG eligible of $NQUEUE queued but 0 lanes running and 0 launched this pass (load=$LOAD1, ceiling=$LOAD_CEIL, gh failures=$GHFAIL)"
else
  STALLS=0
  [ "$launched" -gt 0 ] && alarm_clear "eligible=$NELIG queued=$NQUEUE lanes=$((NLANES+launched))/$MAX_LANES — drain feeding again"
fi
echo "$STALLS" > "$STALL_COUNT"

log "pass done: lanes_before=$NLANES launched=$launched eligible=$NELIG load=$LOAD1"
