# Changelog

This is the canonical product changelog for Codewith, the `@hasna/codewith`
CLI, and the stable Codewith `rust-v0.1.*` product release tags in this fork.
GitHub Releases remain the place for binaries, checksums, installers, and
DotSlash artifacts, but release notes should be copied from this file instead
of generated from the release bump commit.

The fork-specific history starts after upstream commit `20fedafff8` on
2026-05-21. The first fork-only commit is `809fe3130c`, `feat: add scheduled
loop tasks`.

Sources used to rebuild this changelog: local git history and tags,
`origin/main`, release branches for `0.1.27`, `0.1.28`, and `0.1.46`,
GitHub Releases, and npm metadata for `@hasna/codewith`.

Known evidence gaps:

- GitHub Releases currently exist for `0.1.26`, `0.1.27`, `0.1.28`,
  `0.1.29`, `0.1.33`, `0.1.39`, `0.1.40`, `0.1.41`, `0.1.42`,
  `0.1.43`, `0.1.45`, and `0.1.46`.
- GitHub Releases were not found for `0.1.30`, `0.1.31`, `0.1.32`,
  `0.1.34`, `0.1.35`, `0.1.36`, `0.1.37`, `0.1.38`, or `0.1.44`,
  though matching local tags and/or npm publications exist for some of these
  versions.
- The GitHub Release bodies for `0.1.29` and `0.1.33` were generated from tag
  commit messages, not this changelog.
- npm has published `0.1.4`, `0.1.6`, and `0.1.15`, but no matching local tag
  or committed release bump was found in the current refs.
- npm metadata still has old timestamps for `0.1.0` and `0.1.1`, but
  `npm view @hasna/codewith versions` currently starts at `0.1.2`.
- `0.1.27`, `0.1.28`, and `0.1.46` live on release branches, not
  `origin/main`; `0.1.46` was cut from `fix/loops-runtime-0.1.46`.
- `origin/main` remains at Codewith `0.1.45` until the `0.1.46` hotfix branch
  is merged, cherry-picked, or superseded by a later release.
- The `rust-v0.1.46` GitHub Release is metadata-only and is not marked latest;
  use `rust-v0.1.45` for the latest asset-bearing platform binary release until
  a later full asset release supersedes it.
- This file intentionally excludes pre-fork alpha tags
  `rust-v0.1.0-alpha.*`, upstream high-version `rust-v*` tags, `python-v*`
  SDK tags, and `rusty-v8-v*` dependency artifact tags.

## [Unreleased]

## [0.1.66] - 2026-07-16

Tag: `rust-v0.1.66`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.66>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.65...rust-v0.1.66>
Release status: prepared; tag and npm publication pending.

Hotfix release for banked usage-limit resets and remote-compaction history
preservation.

### Added

- Usage limits: `/usage` can now redeem an available banked reset, and a new
  default-off `/config` toggle can automatically redeem an exact weekly reset
  after Codewith revalidates the account, profile, credit, exhaustion window,
  and reset generation immediately before consumption.

### Fixed

- Compaction: V1 and V2 remote compaction now stop when the provider rejects
  the request for context overflow, preserving the complete unsummarized
  semantic history and leaving live history unchanged instead of deleting an
  older prefix before retrying.

## [0.1.65] - 2026-07-14

Tag: `rust-v0.1.65`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.65>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.64...rust-v0.1.65>

Hotfix release for GPT-5.6 Sol backend version gating.

### Fixed

- Model selection: Codewith now advertises upstream Codex API compatibility
  `0.144.4` instead of the fork's low `0.1.x` package version, so
  `gpt-5.6-sol` requests do not fail with the backend "requires a newer version
  of Codex" gate.
- Model discovery: ChatGPT model catalog refreshes now use the same advertised
  Codex API compatibility version for model-list cache/version checks, keeping
  model availability aligned with request headers.

## [0.1.64] - 2026-07-13

Tag: `rust-v0.1.64`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.64>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.63...rust-v0.1.64>

Hotfix release for ChatGPT subscription context-window compaction.

### Fixed

- Compaction: ChatGPT-authenticated OpenAI GPT-5.4, GPT-5.5, and GPT-5.6
  model metadata now clamps API-sized fallback or stale remote context windows
  to the subscription-sized 272K raw window by default, so auto-compaction uses
  the 244.8K default compact threshold and the 258.4K effective full-window cap
  instead of waiting for a 1M-class API budget.
- Status: ChatGPT profile switches and resumed/forked token-count replay now
  refresh or clamp stale API-sized context-window denominators while preserving
  smaller backend-reported ChatGPT replay windows.

## [0.1.61] - 2026-07-09

Tag: `rust-v0.1.61`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.61>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.60...rust-v0.1.61>

Hotfix release for auth-profile switch status context windows.

### Fixed

- Status: sessions opened under a non-API/ChatGPT profile now refresh
  token-count context windows immediately after switching to an OpenAI API-key
  profile, so `/status` stops showing the prior 258K denominator and reflects
  the API model's 1M-class window without waiting for a new session.

### Security

- Dependencies: updated `crossbeam-epoch` to `0.9.20` to resolve
  `RUSTSEC-2026-0204` from the release gate.

## [0.1.60] - 2026-07-08

Tag: `rust-v0.1.60`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.60>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.59...rust-v0.1.60>

Hotfix release for restored OpenAI API-key profile status context windows.

### Fixed

- Status: resumed and forked sessions now refresh restored token-usage context
  windows from the active model metadata for API-key profiles, so `/status`
  does not continue displaying an old 258K denominator after a GPT model is
  configured with a 1M-class context window. ChatGPT login profiles preserve
  their replayed ChatGPT-catalog context behavior.

## [0.1.59] - 2026-07-08

Tag: `rust-v0.1.59`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.59>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.58...rust-v0.1.59>

Hotfix release for OpenAI API-key profile context-window metadata.

### Fixed

- Provider model metadata: OpenAI API-key and provider-auth profiles now keep
  the bundled GPT-5.5 and GPT-5.4 1.05M context-window values when stale
  remote or cached OpenAI model metadata reports a smaller window. This fixes
  `/status` showing an effective 258K context denominator for API profiles
  instead of the effective 998K window after the configured 95% cap. ChatGPT
  login profiles continue to use the remote ChatGPT model catalog as the
  authoritative source.

## [0.1.57] - 2026-07-05

Tag: `rust-v0.1.57`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.57>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.56...rust-v0.1.57>

Hotfix release for the loop-worker fork-conflict failure class, plus a
user-facing agent max-threads control and the CI npm publish repair
(#121, #123).

### Fixed

- Core: a full-history subagent fork (`fork_context=true` on stable v1,
  `fork_turns=all` on multi_agent_v2) combined with `agent_type`, `model`, or
  `reasoning_effort` no longer hard-rejects the spawn. Those overrides were
  already ignored on the full-fork path (the child config is built from the
  parent), so the reject was converted into a notice of the ignored fields
  (logged via `tracing::warn`) and the fork proceeds with inherited values.
  This removes the conflict-class errors that were failing routed loop
  workers in production; tool-schema descriptions now state the fields are
  inherited on a full-history fork. (#121)
- Release: the `publish-npm` job now authenticates with the
  `NODE_AUTH_TOKEN` repository secret. npm trusted publishing (OIDC) was
  never configured for the `@hasna/codewith*` packages, so no fork release
  had ever published from CI; every prior npm release was published by hand.
  (#123)

### Added

- TUI: `/config` gains an agent max-threads control for the existing
  `[agents] max_threads` cap (preset choices, persisted to `config.toml`,
  live-applied to the session), replacing hand-editing of
  `~/.codewith/config.toml`. Hidden while `multi_agent_v2` governs threads.
  (#121)

## [0.1.56] - 2026-07-04

Tag: `rust-v0.1.56`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.56>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.55...rust-v0.1.56>

Stability and provider-compatibility release. Follows 0.1.55 with 17 merged
pull requests (#90-#107) focused on sqlite state-layer contention, chat and
responses API compatibility across providers, TUI robustness, and the release
toolchain pin that unblocks reproducible platform builds.

### Fixed

- State layer: writes are serialized on a single-connection pool to eliminate
  intra-process `SQLITE_BUSY (517)` under concurrent sessions; hot write paths
  (mailbox, logs, rollout) retry `BUSY_SNAPSHOT` and use `BEGIN IMMEDIATE`.
- Chat-Completions tool-calling hardened for compatibility providers; dropped
  SSE items are now observable and `function_call_arguments.delta` is handled;
  reasoning is read and replayed on the chat-completions path.
- `reasoning_effort: none` maps to `low` for Cerebras/NVIDIA chat/completions;
  namespace tools are disabled for the xAI responses API; Gemini
  `thought_signature` round-trips and the stray `client_version` query
  parameter is dropped.
- Provider catalog: ollama and lmstudio built-in providers restored; known
  provider-model metadata and fallbacks refreshed.
- TUI: `/fork` degrades gracefully on a thread with no rollout yet; `/apps`
  surfaces the underlying app/list failure reason; Enter dispatches the exact
  slash alias (`/exit` no longer opens `/experimental`).
- Core: router `FunctionCallError` auto-log downgraded from error to debug;
  arg0 helper-symlink spawn race closed under concurrent sessions.
- Release: Rust toolchain pinned to 1.96.1 in the release workflows for
  reproducible cross-platform builds.

### Performance

- State: default INFO-level log capture, bounded HTTP traces, off-transaction
  prune, and WAL checkpoints across all pools.

## [0.1.55] - 2026-07-02

Tag: `rust-v0.1.55`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.55>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.54...rust-v0.1.55>

Consolidation release: merges all outstanding work since 0.1.51 (40 pull requests,
17 branches, and the 0.1.52-0.1.54 release lineage whose tags never published).
Versions 0.1.52-0.1.54 were tagged but never reached npm due to release-pipeline
failures fixed here.

### Fixed

- Release pipeline: libcap-2.75 musl download now falls back across verified
  mirrors (kernel.org -> OSUOSL -> Debian) with mandatory checksum; macOS
  builds use CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 and the aarch64 primary
  runs on macos-15-xlarge, ending the 3-hour timeouts that killed 0.1.52-0.1.54.
- Goals state: databases stamped by published 0.1.48 are repaired before
  migration, preventing a permanent VersionMismatch(5) that disabled the
  sqlite state layer (threads, schedules, mailbox, goals) after upgrade.
- OpenRouter GLM-5.2 metadata restored (parallel tool calls, High/XHigh
  reasoning presets, gpt-oss-120b as default fallback).
- Usage-profile broker: scheduled dispatches now defer while all sibling
  profiles are cooling down instead of feeding the failure circuit breaker.
- Remote and sandboxed filesystems support symlink-safe reads, so project
  instructions (CODEWITH.md/AGENTS.md) load in remote sessions.
- Mailbox: queued-input API restored; delivery defer rolls back on enqueue
  rejection; delivery-policy parsing matches the SQL claim filter; queued
  context delivery is bounded.
- Loops/schedules: deferred runs during usage waits; stale-scheduler guards;
  live-session owner routing; scheduled turns no longer truncate interactive
  session history.
- macOS app: dock re-open restores the hidden window; deploy script bundles a
  real CLI binary with CLI-derived bundle version.

### Added

- /teach, /variant, /usage, /pair TUI commands; slash-command alias dedupe.
- Webhook event inbox; session PR worktree mode; nested loops to depth five;
  goal plan appends; approval-gated agent MCP management; compact CLI output;
  experimental smart suggest; CODEWITH.md native instruction imports.
- Auth profile usage health checks and hardened usage-profile autoswitch.
- Codewith bridge adapter surfaces (profile list --json, authProfile in thread
  inventory); workflow model routing contract; sourced additional context.
- Security hardening: canonical PR-mode cwd, thread-monitor path handling,
  remote-control-origin RPC gating, mission-control model tool hardening,
  update-RPC import blocking, symlinked project-instruction rejection,
  remote-control credential handling.
- macOS app: navigation data and session controls, profile settings flows,
  visual system aligned with the Hasna dashboard.
- Release CI: sccache (fail-open) across release workflows for warm-cache
  builds from 0.1.56 onward.

### Security

- Node supply-chain advisories patched; anyhow 1.0.103 (RUSTSEC-2026-0190);
  quick-xml advisories documented as compile-time-only exposure; gitleaks
  config with tightly-scoped test-canary allowlist.

### Known issues

- Windows argument-comment-lint CI job fails on aws-lc-sys feature probes
  under clang-cl (pre-existing; release builds unaffected).
- sccache resolves its latest release at install time (fail-open; pin planned).
- Remote no-follow reads have a documented TOCTOU window between metadata and
  read RPCs.

### Fixed

- Recovered loop-driven and long-running turns from context-window overflow
  errors by compacting mid-turn and retrying once before surfacing the failure.

## [0.1.54] - 2026-07-02

Tag: `rust-v0.1.54`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.54>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.53...rust-v0.1.54>

### Fixed

- Tuned macOS primary release builds to override the workspace release
  `codegen-units = 1` setting, preventing the full CLI release build from
  timing out before npm publishing can run.

## [0.1.53] - 2026-07-01

Tag: `rust-v0.1.53`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.53>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.52...rust-v0.1.53>

### Fixed

- Preserved durable resumed thread cwd, workspace roots, permission profile,
  auth profile, model settings, approval policy, and reasoning effort unless a
  `codewith exec resume` caller explicitly overrides them.
- Restored permission profiles from persisted rollout history when resuming
  headless threads, preventing ambient caller sandbox defaults from replacing
  the original agentic work sandbox.
- Fixed the human startup summary for resumed `codewith exec` sessions so it
  reports the effective resumed session configuration instead of the local
  caller process defaults.

## [0.1.52] - 2026-07-01

Tag: `rust-v0.1.52`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.52>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.51...rust-v0.1.52>

### Fixed

- Added explicit durable headless execution flags for `codewith exec`
  (`--durable` and `--persist`) while keeping persistent execution as the
  default and rejecting conflicting `--ephemeral` usage.
- Preserved GPT-5.5 model metadata at the full 272k context window and added
  focused regression coverage for GPT-5.5 context-window handling and
  auto-compact behavior.

## [0.1.51] - 2026-06-26

Tag: `rust-v0.1.51`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.51>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.50...rust-v0.1.51>

### Fixed

- Added a SQLite-level guard so already-running stale Codewith scheduler
  processes cannot claim a `/loop` that belongs to a fresh live interactive
  session, preventing background execution from stealing turns that should be
  visible in the TUI.
- Hardened manual `/loop` run-now claims with the same live-owner protection,
  including regression coverage for legacy claim suppression, foreign live-owner
  rejection, and owner-scoped successful claims.

## [0.1.50] - 2026-06-26

Tag: `rust-v0.1.50`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.50>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.49...rust-v0.1.50>

### Fixed

- Fixed fresh interactive `/loop` sessions so creating a loop materializes the
  thread rollout before the loop can fire, allowing cold resume and cross-process
  recovery instead of failing with `thread not found`.
- Routed due scheduled loop claims away from other local Codewith processes
  while a fresh live owner is active, so background pollers cannot consume a
  loop that should inject into the visible interactive session.
- Added regression coverage for fresh no-turn loop materialization, foreign
  active-owner claim suppression, stale-owner recovery, and repeated visible
  scheduled-turn injection.

## [0.1.49] - 2026-06-26

Tag: `rust-v0.1.49`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.49>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.47...rust-v0.1.49>

### Notes

- Corrective release that intentionally supersedes `0.1.48`. The `0.1.48`
  publish came from a broad spark01 preservation branch and could trigger local
  SQLite migration checksum failures; this release returns to the migration-safe
  `0.1.47` base and reapplies the loop and mailbox fixes.

### Fixed

- Hardened scheduled `/loop` execution so usage waits and busy threads defer
  runs without incrementing failure counts, then re-arm correctly after retry.
- Restored repeated interval loop injection as visible interactive turns, with
  regression coverage proving each firing creates a fresh turn, persists user
  and assistant messages, and submits a distinct model request.
- Reapplied mailbox dispatcher fixes on the safe migration base, including
  queued mailbox input handling and local active-session migration stability.

## [0.1.46] - 2026-06-24

Tag: `rust-v0.1.46`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.46>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.45...rust-v0.1.46>

### Fixed

- Hardened scheduled Codewith runs so usage-profile handoff, auth-profile
  fallback, and usage-exhaustion retry timing are resolved before a scheduled
  turn starts.
- Hardened scheduled-run turn bookkeeping so immediate approval, permission, or
  startup failures do not lose the run mapping before the model-side wait can
  resume.
- Improved scheduled-run failure messages with contextual error reporting and
  structured handling for turn errors.
- Clarified the scheduled-run prompt so cadence text is treated as schedule
  context and durable follow-up work uses native goal tools rather than
  accidental nested schedules.

### Release

- Published `@hasna/codewith` `0.1.46` as a scheduled-run hotfix from the
  `fix/loops-runtime-0.1.46` release branch.
- Left `@hasna/codewith-sdk` and `@hasna/codewith-responses-api-proxy` at
  `0.1.45`; those packages are staged by the release packaging script from
  source manifests that intentionally keep template versions.
- Reconciled the missing GitHub Release metadata for `rust-v0.1.46` as a
  metadata-only, non-latest release during the 2026-06-24 release-compliance
  follow-up; no platform binaries, installers, checksums, or DotSlash artifacts
  were attached to this hotfix release.

## [0.1.45] - 2026-06-24

Tag: `rust-v0.1.45`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.45>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.43...rust-v0.1.45>

### Changed

- Prepared the branch and worktree drain release after merging the completed
  non-macOS backlog PRs into `main`.
- Bumped the Codewith CLI, Rust workspace, and npm package metadata to
  `0.1.45`.

### Fixed

- Included the merged drain hardening for queued rules, session machine context,
  schedule auth recovery, pending interaction responses, provider usage
  accounting, managed worktree cleanup, SQLite feedback log churn, TUI PR
  surfaces, statusline summaries, and goal title display.
- Moved the x86_64 macOS release build to the `macos-15-xlarge` runner so the
  signed platform package matrix can complete.

### Verification

- Rebuilt and smoke-tested the Linux ARM64 release binary and npm package before
  publish; the `rust-release` workflow builds and publishes the remaining
  signed platform packages from this tag.

## [0.1.43] - 2026-06-21

Tag: `rust-v0.1.43`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.43>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.42...rust-v0.1.43>

### Added

- Added `/tmux --session <name> --window <name>` handoff support so an active
  Codewith TUI can move into a specific existing tmux session and window, while
  preserving the existing automatic session creation behavior when no target is
  supplied.
- Added workflow run management surfaces for creating, listing, starting,
  pausing, resuming, and cancelling workflow runs from the TUI and model-visible
  workflow management tool.
- Added `/changelog` in the TUI so release notes are available inside the
  product.
- Added Mission Control schedule visibility and schedule-aware empty states.
- Added Z.ai GLM-5.2 model metadata with the documented `glm-5.2` model ID,
  1M context window, text-only input modality, native tool/web-search support,
  and GLM-5.2 reasoning presets.

### Changed

- Promoted workflows into the experimental feature menu while keeping them
  disabled by default.
- Updated model instruction templates and catalog metadata to ask models for
  effort estimates when planning work.
- Improved loop and schedule manager rows so prompts are the primary label and
  status/spec details stay in row descriptions.
- Aligned Claude external-agent auth with Claude Agent SDK guidance by using
  API-key/provider environment auth for launches and hiding Claude.ai from the
  generic new subscription-profile picker.

### Fixed

- Hardened workflow lifecycle handling around run control, boxed display
  responses, and model-facing unavailable states for unsaved threads.
- Hardened app-server daemon startup so embedded/local daemon starts can use the
  active Codewith binary while managed installs still use the managed binary.
- Hardened background-agent managed worktree release so active background-agent
  leases use the lease release path and cannot be bypassed by generic worktree
  detach or release.
- Hardened background-agent stop/delete ordering, diagnostics pending counts,
  and rollout/thread validation.
- Hardened durable background-agent supervisor attach/stop behavior for
  delivered interactions, delete-requested runs, and unclaimed queued runs.
- Preserved usage accounting when resuming blocked or usage-limited goals.
- Kept usage self-heal retry tests active even when retry behavior is gated by
  config defaults.

## [0.1.42] - 2026-06-20

Tag: `rust-v0.1.42`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.42>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.41...rust-v0.1.42>

### Fixed

- Fixed strict Responses API schemas for Mission Control tools so optional
  arguments are represented as nullable required fields and invalid strict tool
  schemas are rejected locally before reaching the provider.

## [0.1.41] - 2026-06-19

Tag: `rust-v0.1.41`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.41>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.40...rust-v0.1.41>

### Added

- Added shared collaboration surfaces for Mission Control, managed worktrees,
  active sessions, workflows, mailboxes, pending interactions, and local session
  discovery through the app-server API and TUI.
- Added durable workflow and worktree state, including managed worktree
  assignment, cleanup policies, workflow plan projections, and machine
  registry support.
- Added interactive TUI flows for `/worktree`, Mission Control, workflow
  displays, usage profile routing, and goal-plan detail navigation.

### Changed

- Promoted the Mission Control and worktree app-server entrypoints from
  experimental-only APIs to stable protocol surfaces.
- Bounded goal-plan event payloads so large plans stay useful without flooding
  model-visible event context.

### Fixed

- Hardened worktree assignment rollback, stale owner cleanup, agent attachment
  validation, workflow prompt bounds, and cancelled goal-plan status handling
  after adversarial review.

## [0.1.40] - 2026-06-17

Tag: `rust-v0.1.40`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.40>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.39...rust-v0.1.40>

### Added

- Added model gateway metadata and gateway-aware model/provider browsing,
  including OpenRouter routing details in the app-server API and TUI pickers.
- Added durable goal-plan storage, app-server plan listing, AI goal-plan tools,
  and a TUI goal manager view for plan progress.
- Added external-agent readiness checks, cancellation, safer readable roots, and
  hardened Cursor, Claude, and Grok Build flows.
- Added `Shift+Tab` permission preset cycling with `/keymap` remapping support.

### Changed

- Preserved auth-profile permission choices across profile switches and added
  profile-targeted rate-limit reads for fresher profile picker usage state.
- Reorganized config popups into clearer sections and expanded common config
  toggle coverage.
- Updated built-in provider/model metadata and model availability display.

## [0.1.39] - 2026-06-16

Tag: `rust-v0.1.39`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.39>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.38...rust-v0.1.39>

### Fixed

- Superseded the partial `0.1.38` npm wrapper so the final launch package can
  publish a fresh root version with the complete platform optional dependency
  matrix.

## [0.1.38] - 2026-06-16

Tag: `rust-v0.1.38`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.38>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.37...rust-v0.1.38>

### Fixed

- Superseded `0.1.37` with a smaller Apple Silicon native npm package after the
  initial Darwin tarball metadata published but the tarball blob was not
  fetchable from the registry.

## [0.1.37] - 2026-06-16

Tag: `rust-v0.1.37`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.37>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.36...rust-v0.1.37>

### Added

- Added `/loop stats` and `/schedule stats` slash command handling with
  automatic schedule selection when only one matching item exists.
- Included schedule run statistics in loop management tool responses.

### Changed

- Improved auth-profile picker descriptions with plan and email details, compact
  separators, and a shared action hint.
- Recognized the newer scheduled-prompt guardrail text while still hiding legacy
  scheduled-prompt scaffolding from user-visible message history.

### Fixed

- Published matching native Bun packages for the machines updated during this
  release, including Apple Silicon installs.

## [0.1.36] - 2026-06-16

Tag: `rust-v0.1.36`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.36>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.35...rust-v0.1.36>

### Changed

- Kept auth-profile usage data fresh in the profile picker while the TUI
  remains open.
- Simplified selected auth-profile helper text and hid unknown usage placeholders
  when no usage data has been loaded yet.

### Fixed

- Preserved the selected profile row when usage data refreshes while the profile
  picker is open.

## [0.1.35] - 2026-06-16

Tag: `rust-v0.1.35`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.35>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.34...rust-v0.1.35>

### Added

- Added external subscription auth profiles for Claude.ai, Cursor, and Grok.
- Added Claude Code as an external-agent runtime alongside Cursor and Grok
  Build.
- Added provider metadata to auth profile listings and AI-callable profile
  management responses.

### Changed

- Claude Code external-agent runs now require a matching active Claude.ai
  subscription profile before launching the Claude CLI.
- Claude Code external-agent launches preserve stable local Claude config paths
  without copying provider API keys.
- Slash command results remain flat and top-level while showing useful aliases,
  `/apps` when available, and `/debug-config`.

### Fixed

- Fixed release workflow notes so stable `rust-v0.1.*` tags read the matching
  section from `CHANGELOG.md`.
- Fixed external-agent sandbox selection for permission profiles that require
  direct runtime enforcement.
- Preserved legacy Cursor and Grok Build external-agent CLI auth paths when no
  matching Codewith subscription profile is selected.
- Hid internal memory debug slash commands from the regular command picker.
- Refined Codewith TUI accent/shimmer styling for better contrast.

## [0.1.34] - 2026-06-15

Tag: `rust-v0.1.34`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.34>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.33...rust-v0.1.34>

### Fixed

- Restored the flat, one-level slash command picker.
- Removed nested slash command category rows and trailing category paths such
  as `/session/`.
- Kept slash command aliases hidden by default while showing real top-level
  commands for an empty `/` prompt.

## [0.1.33] - 2026-06-15

Tag: `rust-v0.1.33`
Release: <https://github.com/hasna/codewith/releases/tag/rust-v0.1.33>
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.33>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.32...rust-v0.1.33>

### Added

- Added an AI-callable goal resume tool so the model can resume paused,
  blocked, or usage-limited goals when appropriate.

### Fixed

- Rejected attempts to resume budget-limited goals without changing the token
  budget.
- Avoided token-accounting and event side effects when a budget-limited goal
  resume is rejected.

## [0.1.32] - 2026-06-15

Tag: `rust-v0.1.32`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.32>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.31...rust-v0.1.32>

### Added

- Added app-server active-session peer registry and messaging APIs.
- Added lean TUI active-session and agent commands.
- Added Codewith authentication profile login, persisted profile ordering,
  usage hints, and active profile switching.

### Fixed

- Fixed scheduled loop run visibility.
- Hardened active-session delivery when a target session is no longer loaded.
- Repaired main merge fallout in app-server and TUI test/config paths.

### Notes

- This was a large main-line repair release after merging the Codewith release
  line and upstream sync work back together.

## [0.1.31] - 2026-06-15

Tag: `rust-v0.1.31`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.31>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.30...rust-v0.1.31>

### Fixed

- Fixed Xiaomi MiMo provider routing across core turns and app-server thread
  settings, starts, and turn starts.

## [0.1.30] - 2026-06-15

Tag: `rust-v0.1.30`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.30>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.29...rust-v0.1.30>

### Added

- Added the durable background-agent runtime foundation and app-server/CLI/TUI
  management surfaces.
- Added the external-agent app-server runtime bridge.
- Added a config MCP self-heal workflow.
- Added xAI as a built-in model provider.
- Added monitor management APIs and slash-command controls.
- Added slash command fuzzy ranking.

### Changed

- Restored the canonical Codewith changelog after `0.1.29` reduced it to a
  release-page pointer.
- Refreshed rate-limit status completions by auth profile.
- Added fast Rust test workflow guidance.

## [0.1.29] - 2026-06-14

Tag: `rust-v0.1.29`
Release: <https://github.com/hasna/codewith/releases/tag/rust-v0.1.29>
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.29>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.28...rust-v0.1.29>

### Added

- Expanded TUI session-management surfaces, including terminal/tmux handoff,
  UI dynamic tools, status controls, and profile/session actions.
- Added an external-agent runtime bridge for app-server-managed agent runs.
- Added provider hosted-search metadata and hosted tool specs.
- Added monitor manager creation support.

### Fixed

- Hardened monitor and schedule runtimes in app-server.
- Completed hosted provider tool metadata.

### Notes

- The `CHANGELOG.md` file in this tag only pointed to the GitHub Releases page;
  this entry was reconstructed from the tagged git history and npm metadata.

## [0.1.28] - 2026-06-12

Tag: `rust-v0.1.28`
Release: <https://github.com/hasna/codewith/releases/tag/rust-v0.1.28>
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.28>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.27...rust-v0.1.28>

### Fixed

- Continued auth profile auto-switching across profiles instead of stopping
  after the first profile transition.

## [0.1.27] - 2026-06-12

Tag: `rust-v0.1.27`
Release: <https://github.com/hasna/codewith/releases/tag/rust-v0.1.27>
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.27>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.26...rust-v0.1.27>

### Added

- Added the durable background-agent runtime foundation, including the
  `codex-rs/background-agent` crate, daemon, supervisor, and process lifecycle
  handling.
- Added persisted background-agent state for runs, events, snapshots,
  interactions, worktrees, and process handles.
- Added app-server background-agent APIs for starting, listing, reading,
  attaching, detaching, stopping, deleting, reading events, responding to
  pending interactions, and retrieving daemon diagnostics.
- Added CLI controls for starting, listing, reading, attaching to, viewing logs
  for, stopping, deleting, and diagnosing background agents.
- Added TUI background-agent slash surfaces through `/agent`,
  `/background-agent`, and `/bg-agent`.

### Release

- Gated optional release follow-up work so it does not block the package path
  unexpectedly.

## [0.1.26] - 2026-06-11

Tag: `rust-v0.1.26`
Release: <https://github.com/hasna/codewith/releases/tag/rust-v0.1.26>
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.26>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.25...rust-v0.1.26>

### Fixed

- Reduced Linux release build pressure and made artifact compression faster.
- Gave cold release builds more headroom.
- Allowed unsigned macOS publishing for the Codewith release path.
- Satisfied provider model checks required by the release gate.

## [0.1.25] - 2026-06-09

Tag: `rust-v0.1.25`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.25>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.24...rust-v0.1.25>

### Added

- Added Anthropic as a built-in model provider.

## [0.1.24] - 2026-06-09

Tag: `rust-v0.1.24`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.24>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.23...rust-v0.1.24>

### Added

- Added Xiaomi MiMo as a built-in model provider.

### Documentation

- Documented the fast Codewith release gate policy.

## [0.1.23] - 2026-06-09

Tag: `rust-v0.1.23`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.23>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.22...rust-v0.1.23>

### Changed

- Merged upstream Codex through the V8 update while preserving Codewith
  provider, auth-profile, and release behavior.
- Increased Bazel and argument-comment-lint timeouts for slower CI jobs.
- Adjusted Linux ARM test and archive jobs to reduce release pressure.

### Fixed

- Preserved provider auth and active provider state through compaction.
- Reported auth profile settings through app-server thread settings.
- Fixed provider model cache metadata, identity, source, and TTL behavior.
- Hardened Windows PTY, Guardian, remote-control, websocket, schedule, hook,
  and remote exec tests.
- Stabilized fork-specific CI snapshots, codespell checks, argument comments,
  advisory baselines, and platform-specific imports.

## [0.1.22] - 2026-06-06

Tag: `rust-v0.1.22`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.22>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.21...rust-v0.1.22>

### Fixed

- Refreshed NVIDIA fallback model metadata.

## [0.1.21] - 2026-06-06

Tag: `rust-v0.1.21`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.21>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.20...rust-v0.1.21>

### Added

- Added thread monitor workflows.

## [0.1.20] - 2026-06-06

Tag: `rust-v0.1.20`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.20>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.19...rust-v0.1.20>

### Fixed

- Refreshed provider fallback models.

## [0.1.19] - 2026-06-06

Tag: `rust-v0.1.19`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.19>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.18...rust-v0.1.19>

### Fixed

- Repaired provider switching.

## [0.1.18] - 2026-06-05

Tag: `rust-v0.1.18`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.18>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.17...rust-v0.1.18>

### Added

- Added the provider-scoped metadata catalog.

### Fixed

- Clarified MCP startup failures.

## [0.1.17] - 2026-06-05

Tag: `rust-v0.1.17`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.17>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.16...rust-v0.1.17>

### Fixed

- Preserved the selected model provider for turns.

## [0.1.16] - 2026-06-05

Tag: `rust-v0.1.16`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.16>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.9...rust-v0.1.16>

### Notes

- This is an aggregate tagged release covering the npm-only `0.1.10` through
  `0.1.15` publications. The entries below preserve the per-publication npm
  history, while this tagged section records the complete tag delta from
  `rust-v0.1.9` to `rust-v0.1.16`.
- No additional user-facing delta beyond release/tag catch-up was found between
  the committed `0.1.14` release bump and the `0.1.16` tag in the available
  refs.

## [0.1.15] - 2026-06-04

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.15>

### Notes

- Historical npm publication. No matching local tag or committed release bump
  was found in the available refs; changes are covered by the surrounding
  `0.1.14` to `0.1.16` release range.

## [0.1.14] - 2026-06-04

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.14>

### Tests

- Isolated Codewith home in integration tests.
- Added coverage for shell and network permission hooks.

## [0.1.13] - 2026-06-04

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.13>

### Fixed

- Preserved live schedule leases through expiry.

## [0.1.12] - 2026-06-04

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.12>

### Fixed

- Recomputed recurring next runs when loops resume.

## [0.1.11] - 2026-06-04

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.11>

### Fixed

- Expired completed one-time schedules.
- Scoped schedule and loop actions to the current thread.

## [0.1.10] - 2026-06-04

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.10>

### Added

- Added Codewith workflow skills.
- Made schedules one-time events.

## [0.1.9] - 2026-06-03

Tag: `rust-v0.1.9`
npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.9>
Compare: <https://github.com/hasna/codewith/compare/rust-v0.1.0...rust-v0.1.9>

### Added

- Added thread schedule controls.
- Added an auth profile status item.
- Added spec-plan loop tools, TUI approval overlay updates, and skills
  rendering.

### Changed

- Switched provider selection from prefixed model IDs to explicit providers.
- Bundled third-party licenses in the Codewith package layout.

### Fixed

- Wrapped the passive footer status line.

## [0.1.8] - 2026-06-03

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.8>

### Fixed

- Advertised a Codex API-compatible version during login so `gpt-5.5` is not
  incorrectly gated.

## [0.1.7] - 2026-06-03

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.7>

### Changed

- Completed the broad Codex to Codewith rebrand across `codex-rs`, CI,
  scripts, docs, root config, and release metadata.
- Migrated project configuration and skill paths from `.codex` to `.codewith`.
- Rebranded the Python and TypeScript SDK surfaces to Codewith.

### Fixed

- Resolved Codewith package layout directories while keeping legacy Codex
  fallback paths.
- Preferred Codewith instruction paths.
- Stabilized cross-platform CI expectations and remote unified exec tests.

## [0.1.6] - 2026-06-03

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.6>

### Notes

- Historical npm publication. No matching local tag or committed release bump
  was found in the available refs. Available history around this publication is
  the early release-workflow hardening window after `rust-v0.1.0`.

## [0.1.4] - 2026-06-03

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.4>

### Notes

- Historical npm publication. No matching local tag or committed release bump
  was found in the available refs. Available history before this npm timestamp
  includes native binary exposure, Codewith package metadata and release
  workflow hardening, runtime/docs alignment, bundled prompt branding, model
  catalog fixes, and CI/test stabilization.

## [0.1.3] - 2026-06-02

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.3>

### Changed

- Aligned repository metadata with Codewith.
- Completed the Codewith runtime rename.
- Preferred `.codewith` project instructions.

### Fixed

- Refreshed stale workspace fixtures.

## [0.1.2] - 2026-06-02

npm: <https://www.npmjs.com/package/@hasna/codewith/v/0.1.2>

### Changed

- Rebranded the CLI as Codewith.

### Fixed

- Reconstructed raw command history.
- Preserved large code-mode text output.
- Stabilized scheduled loop acknowledgements.

## [0.1.1] - 2026-06-02

### Notes

- Historical Codewith release bump found in git history. npm no longer lists
  this version in the current `versions` array.

## [0.1.0] - 2026-06-03

Tag: `rust-v0.1.0`
Compare: <https://github.com/hasna/codewith/compare/20fedafff8...rust-v0.1.0>

### Notes

- This is the first stable Codewith `rust-v0.1.*` tag. It aggregates the early
  fork work and overlaps the first npm publications, whose registry history is
  less complete than the tag history.

### Added

- Added scheduled loop tasks.
- Added provider picker and OpenRouter model support.
- Added provider-scoped model selection.
- Added auth profile switching, concurrent auth profiles, and session auth
  profile switching.
- Added the `/config` popup.
- Added explicit goal replacement.
- Added OpenRouter cache routing and live pricing metadata.

### Changed

- Packaged the private `iappcodex` CLI and pointed early package metadata at
  the fork repository.
- Rebranded the CLI as Codewith.
- Shared config value persistence.
- Included local agent state.

### Fixed

- Disabled managed automatic update prompts for the fork.
- Hardened auth-home isolation for the private package.
- Refreshed status after auth profile switches.
- Used musl Linux platform artifacts for npm packages.
- Stabilized stack-heavy TUI tests.

## Maintenance Process

Use this process when preparing or auditing Codewith release notes:

1. Determine published npm versions:
   `npm view @hasna/codewith versions --json` and
   `npm view @hasna/codewith time --json`.
2. Determine relevant Codewith tags:
   `git tag --list 'rust-v0.1.*' --sort=v:refname`.
3. Check GitHub release coverage:
   `gh release list --repo hasna/codewith --limit 40`.
4. For each release, choose the previous Codewith boundary. Prefer the previous
   stable `rust-v0.1.*` tag; when npm publications exist without tags, add
   explicit npm-only entries or an aggregate tagged-release note.
5. Audit `git log <previous-boundary>..<new-boundary>` and filter out
   upstream-only noise with `--cherry-pick`, `--right-only`, and first-parent
   history as needed.
6. Classify user-facing changes under Added, Changed, Fixed, Security,
   Release, Documentation, Tests, or Notes. Do not paste raw commit dumps.
7. Record evidence gaps explicitly when a published npm version has no matching
   local tag or committed release bump.
8. Do not record uncommitted dirty-worktree state as release notes. Move active
   work into `Unreleased` only after it is committed or otherwise has a durable
   review boundary.
9. Before every public `rust-v*` release, verify that `codex-rs/Cargo.toml`,
   `codex-cli/package.json`, the `rust-v*` tag, GitHub Release name, and npm
   version agree.
10. The release workflow or release operator must stop if the matching
    `CHANGELOG.md` section is missing. GitHub Release bodies must come from the
    matching section, not from the tagged release bump commit.
11. If a release is cut from a release branch, merge or backport the changelog
    update to the durable branch.
