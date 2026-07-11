# Merge Safety

These rules keep merge execution tied to fresh evidence rather than stale snapshots or assumed GitHub state.

- Preflight is read-only and advisory. It may summarize PR state, checks, reviews, and reviewer artifacts, but it must not mutate GitHub or local git state.
- Executor recheck is authoritative. Immediately before any actual merge action, re-fetch and re-read PR state, head SHA, mergeability, checks, reviews, draft/conflict state, and queue/protection behavior.
- Actual merge requires two independent reviewer artifacts from separate reviewer runs for the exact PR head SHA. Missing, duplicate-run, self-review, stale-head, blocking-verdict, or blocking-finding artifacts stop the merge.
- Reviewer artifact freshness is part of merge safety. Missing, invalid, future, or stale timestamps are blockers; the helper default staleness window is 24 hours unless explicitly overridden for a run.
- Every merge command must include `gh pr merge <pr> --match-head-commit <head_sha>`. This protects against merging a changed PR head after review.
- `--admin` is forbidden. Do not bypass branch protection, queue policy, required checks, or required reviews.
- Use `--auto` only when the user explicitly asks for delayed intent such as "merge when green" or "enable auto-merge".
- Merge queue is an execution mode, not a merge strategy. Follow local `gh pr merge --help`: queue-required branches need no strategy; passed checks enqueue; pending checks enable queue-when-ready only under GitHub queue semantics and explicit delayed user intent.
- Postverify must record PR state, merged commit or queue state, target branch state, CI/check state when available, command used excluding secrets, and final outcome. If execution stops, record the gate that stopped it.
