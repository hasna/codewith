#!/usr/bin/env bash
# drain-merge-guard.sh — may this PR be merged right now?
#
#   usage: drain-merge-guard.sh <owner/repo> <pr-number>
#   exit 0 = allowed        exit 1 = BLOCKED
#
# THE PREDICATE INVERTED, 2026-08-01 (task 562328b1, wired by vibius).
# This guard used to ask "is an attributed NO_GO live at head?" and allow whenever
# the answer was no. That predicate has three known holes, and the sentence that
# closes all three is:
#
#        ABSENCE OF A REJECTION IS NOT THE PRESENCE OF AN APPROVAL.
#
# So the guard now REQUIRES a GO that demonstrably describes what will land, and
# blocks otherwise. The three faces it had to cover, all measured, none hypothetical:
#
#   FACE 1 — a GO SUPERSEDES a NO_GO at the SAME sha without addressing it.
#     Measured on hasna/recordings#65 (cincinnatus, 2026-08-01). The old selection
#     loop took the LAST verdict describing head, so a newer GO at the same sha
#     displaced a reproduced P1 NO_GO and the guard was satisfied. hasna/mementos#18
#     MERGED this way, on a GO that ran contract:check, got exit 1, called it "the
#     documented open issue", and passed.
#     CLOSED BY: clause 1 below no longer looks at only the newest verdict. ANY
#     attributed NO_GO describing head blocks, and a later GO at the same sha does
#     not displace it. Moving head or withdrawing the verdict is the way past it.
#
#   FACE 2 — the BASE is retargeted, so head does not move; the verdict's sha still
#     matches head and the staleness test stays TRUE while becoming INSUFFICIENT in
#     the same instant. What gets squash-merged is a combination of the branch and a
#     base that has since moved — an artefact no reviewer has ever read.
#     Already ratified as `global-pr-base-change-invalidates-review`, which SHIPS the
#     check. This guard does not reimplement it; clause 4 runs it as written.
#     Measured here on 9 real PRs carrying a GO at head: 3 diverge. One of them,
#     hasna/attachments#28, is mergeStateStatus=CLEAN — GitHub's own mergeability
#     signal does not reveal this, which is precisely why a separate check is needed.
#
#   FACE 3 — a REBASE moves head, so NO verdict describes head at all: not a GO, not
#     a NO_GO. With nothing to block on, the old guard returned allowed, and the PR
#     was simultaneously mergeable. A REBASE THEREFORE CONVERTED A BLOCKED PR INTO A
#     GUARD-PASSING PR WITH ZERO REVIEW.
#     Already realised in this guard's own log before the fix:
#       [2026-07-31T23:34:33Z] ALLOW hasna/todos#130 — 1 verdict(s) present but none
#       describes head 050156185bdb
#     CLOSED BY: clause 3 below. No verdict describing head is now a BLOCK, and the
#     zero-verdict case (20 of 51 surveyed open PRs) is the same block.
#
# TRIGGER WIDTH. The ratified rule fires on a BASE CHANGE. Faces 1 and 3 need it to
# fire on any head-or-verdict change, and a base change is not reliably detectable
# from the API anyway. Clauses 1-3 therefore run on every gated PR. CLAUSE 4 DOES
# NOT: it fires only when the base actually moved AFTER the verdict, which is the
# ratified rule's own scope. Running it unconditionally was measured to take the
# fleet's mergeable population from 28 to 1. Condition edits throughout — the
# command underneath is the ratified one, unmodified.
#
# ── PRESERVED FROM THE ORIGINAL, DELIBERATELY ──────────────────────────────────
#
# CLAUSE 2 — a verdict that names NO AUTHOR is not a valid block. UNCHANGED.
#   Measured on hasna/codewith#462: an UNATTRIBUTED NO_GO that nobody can withdraw,
#   because withdrawal requires the author. Blocking on it is a permanent, unliftable
#   veto cast by nobody. It still does not block. Note what the inversion buys here:
#   under the old predicate an unattributed NO_GO was unliftable BY CONSTRUCTION;
#   under this one, ANY reviewer can clear it by posting a GO at head. The clause is
#   preserved and the veto it guarded against is now actually escapable.
#
# FAIL-OPEN ON PROBE FAILURE, DELIBERATELY. UNCHANGED, and extended to the new git
#   probe. If gh cannot answer, or the objects cannot be fetched, this guard does not
#   know whether the merge result was reviewed — and an inability to check is not
#   evidence of a defect. Failing closed would let one GitHub blip or one network
#   hiccup stop every lane on the box. Every fail-open is logged loudly so the silence
#   is not mistaken for a pass.
#   ONE MEASURED SUBTLETY, or this would fail closed by accident: on git 2.43.0
#   `git merge-tree --write-tree` returns rc=1 for a GENUINE CONFLICT and rc=1 for a
#   BOGUS REV alike — the exit code cannot tell a finding from an error. So both
#   commits are rev-parse --verify'd FIRST; only after that is a non-zero merge-tree
#   unambiguously a conflict rather than a broken probe.
set -uo pipefail

REPO="${1:-}"; PRN="${2:-}"
GUARDLOG="${DRAIN_GUARD_LOG:-$HOME/drain-merge-guard.log}"
GUARDCACHE="${DRAIN_GUARD_CACHE:-$HOME/.drain-guard-cache}"
# Raised from 60s: a first fetch of a large repo could not finish inside it, so that
# repo's merge-result check was permanently fail-open (8/8 failures on
# community-openclaw). A slow gate is recoverable; a gate that is always off is not.
FETCH_TIMEOUT="${DRAIN_GUARD_FETCH_TIMEOUT:-240}"

glog() { echo "[$(date -u +%FT%TZ)] $*" >> "$GUARDLOG"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
# shellcheck source=scripts/drain/drain-review-supersession.sh
. "$SCRIPT_DIR/drain-review-supersession.sh"

# Resolve the REAL gh, never the shim that calls us (which would recurse).
# Canonicalise both sides, for the same reason as drain-bin/gh: SHIMDIR is a raw
# string from the environment and the PATH entries are raw strings, so any
# difference in spelling (trailing slash, symlink, //) makes this skip fail and
# resolves REAL_GH back to the shim. Milder here than in the shim — the call
# below is `pr view`, not a merge, so the shim passes it through and it
# terminates one hop later rather than looping — but it is the same defect and
# the same one-line remedy.
SHIMDIR_C="$(cd "${DRAIN_BIN:-$HOME/drain-bin}" 2>/dev/null && pwd -P)" || SHIMDIR_C=""
REAL_GH=""
IFS=: read -ra _pd <<< "$PATH"
for d in "${_pd[@]}"; do
  [ -n "$d" ] || continue
  dc="$(cd "$d" 2>/dev/null && pwd -P)" || dc=""
  [ -n "$dc" ] && [ -n "$SHIMDIR_C" ] && [ "$dc" = "$SHIMDIR_C" ] && continue
  [ -x "$d/gh" ] || continue
  REAL_GH="$d/gh"; break
done
[ -z "$REAL_GH" ] && [ -x "$HOME/.bun/bin/gh" ] && REAL_GH="$HOME/.bun/bin/gh"

if [ -z "$REPO" ] || [ -z "$PRN" ] || [ -z "$REAL_GH" ]; then
  glog "ALLOW (fail-open): cannot identify target repo=$REPO pr=$PRN gh=$REAL_GH"
  exit 0
fi

j="$("$REAL_GH" pr view "$PRN" --repo "$REPO" --json headRefOid,baseRefName,baseRefOid,comments 2>/dev/null)"
rc=$?
if [ "$rc" -ne 0 ] || [ -z "$j" ]; then
  glog "ALLOW (fail-open): gh probe failed rc=$rc for $REPO#$PRN — cannot read verdicts"
  exit 0
fi

head="$(jq -r '.headRefOid // ""' <<<"$j" 2>/dev/null)"
baseref="$(jq -r '.baseRefName // ""' <<<"$j" 2>/dev/null)"
if [ -z "$head" ] || [ -z "$baseref" ]; then
  glog "ALLOW (fail-open): $REPO#$PRN — gh returned no head/baseRef"
  exit 0
fi

if ! drain_review_supersession_decision "$j"; then
  glog "ALLOW (fail-open): $REPO#$PRN — review supersession probe failed"
  exit 0
fi

# ── CLAUSE 3 (face 3, and the zero-verdict case): a verdict must describe head ──
if [ "$DRAIN_REVIEW_HEAD_VERDICT_COUNT" = "0" ]; then
  if [ "$DRAIN_REVIEW_VERDICT_COUNT" = "0" ]; then
    glog "BLOCK $REPO#$PRN — NO [REVIEW] verdict on the PR at all; head ${head:0:12} is unreviewed"
    echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN carries no [REVIEW] verdict at all." >&2
  else
    glog "BLOCK $REPO#$PRN — $DRAIN_REVIEW_VERDICT_COUNT verdict(s) present but NONE describes head ${head:0:12} (rebase/force-push voided them)"
    echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN has $DRAIN_REVIEW_VERDICT_COUNT verdict(s), none describing head ${head:0:12}." >&2
  fi
  echo "Absence of a rejection is not the presence of an approval. Get a [REVIEW] GO at the current head." >&2
  exit 1
fi

case "${DRAIN_REVIEW_BLOCKING_CLASS:-}" in
  live_nogo)
    glog "BLOCK $REPO#$PRN — live NO_GO by ${DRAIN_REVIEW_NOGO_BY} at head ${head:0:12}"
    echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN carries a live NO_GO from reviewer '${DRAIN_REVIEW_NOGO_BY}' at the current head ${head:0:12}." >&2
    echo "Verdict line: $DRAIN_REVIEW_BLOCKING_LINE" >&2
    echo "A later GO at the same sha does NOT clear it. Fix the finding and push, or have ${DRAIN_REVIEW_NOGO_BY} withdraw the verdict." >&2
    exit 1
    ;;
  unwithdrawable_nogo)
    glog "BLOCK $REPO#$PRN — $DRAIN_REVIEW_UNATTR_COUNT unwithdrawable NO_GO(s) at head ${head:0:12} (byline '${DRAIN_REVIEW_UNATTRIB_BYLINE}' pre-${DRAIN_REVIEW_UNATTRIB_BEFORE}), newest $DRAIN_REVIEW_UNATTR_AT; no fresh GO from a registered reviewer supersedes them${DRAIN_REVIEW_SUPERSEDING_AT:+ (newest qualifying GO $DRAIN_REVIEW_SUPERSEDING_AT by $DRAIN_REVIEW_SUPERSEDING_BY is not later)}"
    echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN carries $DRAIN_REVIEW_UNATTR_COUNT unwithdrawable NO_GO(s) at head." >&2
    echo "Their byline was stamped by lane.sh and names an agent that did not write them, so nobody can withdraw them." >&2
    echo "To clear: post a fresh [REVIEW] GO at head ${head:0:12} under your OWN registered name, dated after $DRAIN_REVIEW_UNATTR_AT." >&2
    exit 1
    ;;
esac

if [ "$DRAIN_REVIEW_UNATTR_COUNT" -gt 0 ]; then
  glog "note $REPO#$PRN — $DRAIN_REVIEW_UNATTR_COUNT unwithdrawable NO_GO(s) (newest $DRAIN_REVIEW_UNATTR_AT) SUPERSEDED by GO from registered reviewer $DRAIN_REVIEW_SUPERSEDING_BY at $DRAIN_REVIEW_SUPERSEDING_AT"
fi

if [ -z "$DRAIN_REVIEW_GO_LINE" ]; then
  glog "BLOCK $REPO#$PRN — $DRAIN_REVIEW_HEAD_VERDICT_COUNT verdict(s) describe head ${head:0:12} but NONE is a sha-bearing GO"
  echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN has no [REVIEW] GO describing head ${head:0:12}." >&2
  echo "Absence of a rejection is not the presence of an approval." >&2
  exit 1
fi

_go_line="$DRAIN_REVIEW_GO_LINE"
_go_sha="$DRAIN_REVIEW_GO_SHA"
_go_at="$DRAIN_REVIEW_GO_AT"

# ── CLAUSE 4 (face 2): the ratified merge-result check, run as written ─────────
# `global-pr-base-change-invalidates-review`. Not reimplemented — the two lines
# below are the rule's own command. Everything around them exists only to put real
# objects in front of it and to keep a probe failure fail-open.
d="$GUARDCACHE/${REPO//\//__}.git"
mkdir -p "$GUARDCACHE" 2>/dev/null
if [ ! -d "$d" ]; then git init --bare -q "$d" 2>/dev/null; fi
if [ ! -d "$d" ]; then
  glog "ALLOW (fail-open): $REPO#$PRN — cannot create object cache at $d; merge-result unchecked"
  exit 0
fi
# DRAIN_GUARD_REMOTE exists so the fixture suite drives THIS code path rather than a
# test-only branch. A test that exercises different code proves nothing about this
# script; production leaves it unset and gets the GitHub URL.
REMOTE="${DRAIN_GUARD_REMOTE:-https://github.com/$REPO.git}"
# REFS ARE SCOPED PER PR. They were fixed names (refs/drainguard/base|head), and the
# object cache is shared per REPO across concurrent lanes — so two lanes gating two
# PRs of the same repo raced on the same two refs, last writer won, and the loser
# compared the WRONG head. Found by the operational-safety review (reviewer
# sulpicius, 2026-08-01) and demonstrated on hasna/todos PR110 vs PR119. The loser
# fails open, so the check was silently OFF in exactly the 12-lane operation it was
# built for — a race that disables a gate without ever reporting a failure.
RB="refs/drainguard/$PRN/base"; RH="refs/drainguard/$PRN/head"
timeout "$FETCH_TIMEOUT" git -C "$d" fetch --no-tags -q "$REMOTE" \
  "+refs/heads/$baseref:$RB" "+refs/pull/$PRN/head:$RH" 2>/dev/null
frc=$?
if [ "$frc" -ne 0 ]; then
  # A KILLED OR FAILED FETCH LEAVES ITS PARTIAL PACK BEHIND, AND NOTHING RESUMES OR
  # PRUNES IT. Measured on community-openclaw: 8 of 8 attempts timed out and left
  # 6.5GB of orphaned tmp_pack_* — an unbounded disk leak on a box running lanes.
  # Remove our own debris rather than letting a slow repo fill the disk.
  find "$d/objects/pack" -maxdepth 1 -name 'tmp_pack_*' -delete 2>/dev/null
  glog "ALLOW (fail-open): $REPO#$PRN — git fetch failed rc=$frc (base=$baseref, timeout=${FETCH_TIMEOUT}s); merge-result unchecked; partial packs cleaned"
  exit 0
fi

# Verify BOTH commits resolve before merge-tree runs. On git 2.43.0 merge-tree
# returns rc=1 for a bogus rev and rc=1 for a conflict, so without this the two
# are indistinguishable and a broken probe would fail closed.
base_c="$(git -C "$d" rev-parse --verify -q "$RB^{commit}" 2>/dev/null)"
head_c="$(git -C "$d" rev-parse --verify -q "$RH^{commit}" 2>/dev/null)"
if [ -z "$base_c" ] || [ -z "$head_c" ]; then
  glog "ALLOW (fail-open): $REPO#$PRN — base/head object missing after fetch; merge-result unchecked"
  exit 0
fi
if [ "$head_c" != "$head" ]; then
  # CHANGED FROM FAIL-OPEN TO BLOCK, per the adversarial bypass review (reviewer
  # castor, 2026-08-01, P2). Fail-open is for when the guard CANNOT SEE — a dead
  # gh, an unreachable remote. This is the opposite: the guard saw clearly and
  # found a DETECTED INCONSISTENCY. The head moved between the API read and the
  # fetch, which means the verdicts just matched against the API head no longer
  # describe what would land — the exact condition clause 3 blocks on. Allowing it
  # contradicted this file's own doctrine.
  glog "BLOCK $REPO#$PRN — head moved mid-probe: fetched ${head_c:0:12} != API ${head:0:12}; verdicts describe neither reliably"
  echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN moved while being checked (${head:0:12} -> ${head_c:0:12}). Re-run once it settles." >&2
  exit 1
fi

# ── TRIGGER WIDTH, NARROWED 2026-08-01 after the operational-safety review ────
# It ran clause 4 on EVERY gated PR. Measured consequence over all 49 open PRs:
# allowed went 28 -> 1, twenty-seven flipped, because the fleet's PRs are routinely
# a commit or two behind their base and the lane posts its own sha-bearing GO at
# head, so clauses 1-3 self-satisfy and clause 4 decided every merge. Worse, each
# merge moves that repo's base and invalidates every other open PR in it, which
# serialises the drain to about one PR per repo per review cycle.
# The ratified rule scopes ITSELF: "It fires on a BASE CHANGE, not on every merge."
# So fire only when the base actually moved AFTER the verdict was written — the
# case where the reviewer demonstrably could not have seen the merge result.
# If the base was already ahead when the GO was posted, the reviewer reviewed that
# state and the existing checks cover it.
_mb="$(git -C "$d" merge-base "$base_c" "$head_c" 2>/dev/null)"
_base_moved_at=""
if [ -n "$_mb" ]; then
  # UTC-normalised deliberately: %cI carries a local offset, and comparing an
  # offset timestamp against a Z-suffixed one lexicographically is a units error.
  _base_moved_at="$(TZ=UTC git -C "$d" log -1 --date=format-local:%Y-%m-%dT%H:%M:%SZ \
                      --format=%cd "$_mb..$base_c" 2>/dev/null)"
fi
if [ -z "$_base_moved_at" ]; then
  glog "ALLOW $REPO#$PRN — GO by-verdict at head ${head:0:12} (GO @ ${_go_sha}); base $baseref is an ancestor of head, nothing to re-review"
  exit 0
fi
if [ -n "$_go_at" ] && [[ ! "$_base_moved_at" > "$_go_at" ]]; then
  glog "ALLOW $REPO#$PRN — GO by-verdict at head ${head:0:12} (GO @ ${_go_sha}); base last moved $_base_moved_at, BEFORE the verdict $_go_at — reviewer saw this base"
  exit 0
fi
TREE=$(git -C "$d" merge-tree --write-tree "$base_c" "$head_c" 2>/dev/null)
mrc=$?
if [ "$mrc" -ne 0 ]; then
  # Both revs verified above, so this is a genuine conflict, not a probe error.
  glog "BLOCK $REPO#$PRN — merge of base $baseref (${base_c:0:12}) into head ${head:0:12} CONFLICTS; no reviewed artefact exists"
  echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN does not merge cleanly into $baseref." >&2
  exit 1
fi
git -C "$d" diff --quiet "$head_c" "$TREE" 2>/dev/null
drc=$?
if [ "$drc" -ne 0 ]; then
  glog "BLOCK $REPO#$PRN — UNREVIEWED AT HEAD: merge of $baseref (${base_c:0:12}) with head ${head:0:12} differs from the reviewed tree (GO @ ${_go_sha})"
  echo "MERGE BLOCKED by drain-merge-guard: $REPO#$PRN is UNREVIEWED AT HEAD." >&2
  echo "The GO describes ${_go_sha}, but merging base $baseref (${base_c:0:12}) produces a tree nobody has read." >&2
  echo "Update the branch and get a fresh [REVIEW] GO at the new head." >&2
  exit 1
fi

glog "ALLOW $REPO#$PRN — GO by-verdict at head ${head:0:12} (GO @ ${_go_sha}), merge result matches reviewed tree, no NO_GO at head"
exit 0
