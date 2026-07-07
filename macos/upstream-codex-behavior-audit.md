# Upstream Codex macOS Behavior Audit

Task: `54799fea-3e08-46f4-a68a-785c1738bbd0`

Compared on 2026-07-07 from CodeWith branch
`openloops/open-codewith/54799fea-3e08-46f4-a68a-785c1738bbd0-f355f592`.

## Upstream Evidence

- Refreshed `upstream` from `https://github.com/openai/codex.git`.
- Current `upstream/main`: `f6e251c3ac6573a4d6eccddea6ad12587868485f`.
- `upstream/main` does not contain a `macos/` Swift app tree. A recursive path
  scan for `^macos/` and `*.swift` on `upstream/main` found no app source.
- A scan of fetched upstream refs found only
  `upstream/dev/jm/devicecheck-app-server` with Swift under
  `codex-rs/devicecheck-probe/DeviceCheckProbe.swift`; that is not desktop app
  source.
- The upstream macOS desktop behavior available on `main` is in
  `codex-rs/cli/src/desktop_app/mac.rs`, plus release/signing support for
  app-server binaries in `.github/scripts/macos-signing/` and
  `.github/workflows/rust-release.yml`.

## Current Upstream Behavior to Consider

Upstream `codex-rs/cli/src/desktop_app/mac.rs`:

- Opens an existing app from `/Applications/Codex.app` or
  `~/Applications/Codex.app`.
- If missing, downloads a default installer unless an explicit URL override is
  supplied.
- Chooses the default DMG URL by architecture:
  `Codex.dmg` for Apple Silicon and `Codex-latest-x64.dmg` for Intel.
- Detects Apple Silicon with `std::env::consts::ARCH == "aarch64"` plus macOS
  `sysctlbyname` checks for Rosetta translation and arm64 hardware.
- Launches with a deep link shaped as `codex://threads/new?path=<workspace>`.
- Mounts the DMG read-only with `hdiutil`, finds `Codex.app`, copies it to
  `/Applications` or `~/Applications`, detaches the DMG, and reports detach
  failures as warnings after the install attempt.
- Keeps tests around mount-point parsing and workspace-path URL encoding.

## Current CodeWith Behavior

CodeWith already has a native app under `macos/CodeWith`, not an upstream Swift
tree to merge:

- `macos/CodeWith/Package.swift` builds a SwiftPM executable named `CodeWith`
  for macOS 26.
- `macos/README.md` states the UI target: Codex visual parity with CodeWith
  branding and fork-only Apps, Machines, Loops, Goals, Workflows, and Profiles.
- `macos/CodeWith/Sources/CodeWith/App/CodeWithApp.swift` creates a regular
  AppKit app, keeps a menu-bar status item, restores the window on reopen, and
  dispatches URL opens through `CodeWithOpenURL`.
- `AppModel` starts a bundled or installed `codewith app-server`, completes the
  initialize handshake before marking the UI connected, and falls through to the
  next CLI candidate if one candidate starts but cannot initialize.
- The app-server client drives threads, turns, search, Apps, Machines, Loops,
  Goals, Workflows, auth profiles, config, MCP, hooks, worktrees, account usage,
  active sessions, agent runs, and pending approvals through JSON-RPC.
- Machine selection scopes threads, projects, loops, goals, and workflows, and
  blocks switching while server approval/user-input/elicitation prompts are
  pending.
- The app accepts both `codewith://` and `codex://` schemes for desktop URLs,
  but the CodeWith CLI launch path emits `codewith://threads/new?path=...`.
- `macos/scripts/run-on-apple03.sh` builds on apple03, bundles a standalone
  `codewith` CLI into `CodeWith.app/Contents/Resources/codewith`, stamps the app
  version from the bundled CLI, registers both `codewith` and `codex` URL
  schemes, ad-hoc signs the bundle, and smoke-tests the bundled CLI.
- `macos/scripts/shoot.sh` renders deterministic ImageRenderer snapshots and
  verifies every reported PNG exists and is non-empty.

CodeWith `codex-rs/cli/src/desktop_app/mac.rs` is already adapted for
`CodeWith.app` and `codewith://`, but it differs from upstream in one important
way: if the app is missing and no `download_url_override` is supplied, it bails
with "Install CodeWith.app or pass an explicit desktop app download URL" instead
of selecting a default installer URL.

## Inherit Recommendations

1. Inherit upstream's no-argument installer behavior after CodeWith has stable
   default DMG locations. The implementation should keep `download_url_override`
   precedence, add arm64/x64 CodeWith DMG constants, port the Apple Silicon
   detection helper, and add unit coverage for architecture selection and URL
   override precedence. Do not point CodeWith at upstream OpenAI DMG URLs.

2. Keep upstream's install flow shape for macOS: read-only `hdiutil attach`,
   `.app` discovery in the volume, install to `/Applications` then
   `~/Applications`, best-effort detach warning, and URL encoding tests. The
   current CodeWith code already follows most of this.

3. Preserve the app-server-first architecture. Upstream ships app-server release
   artifacts and signing entitlements; CodeWith's Swift app should continue to
   treat `codewith app-server` as the functional backend rather than reimplement
   agent/runtime behavior in Swift.

4. Preserve dual deep-link compatibility. `codewith://` must remain the primary
   emitted scheme, while `codex://` should continue to work for compatibility
   with upstream-style links and existing bundle registrations.

5. Preserve CodeWith fork features while importing parity behavior. Do not
   rename Apps back to Plugins, remove Machines, Loops, Goals, Workflows, or
   Profiles, or regress auth-profile switching and config surfaces.

6. Keep update behavior fork-specific. The repo rules prohibit re-enabling
   upstream automatic update checks, update notifications, or prompts; installer
   defaults must be explicit CodeWith release plumbing, not an upstream updater.

## Existing Follow-Up Coverage

Existing open-codewith todos already cover most broad macOS implementation work:

- `4ccc14a9-3c1c-48fb-8914-0826b81f7fa3`: build and smoke-test CodeWith macOS
  app on apple03.
- `c60c3cc7-a3b9-47ee-9b5f-b9dca6cdf2db`: fix macOS app implementation bugs
  without UI redesign.
- `d388f9f1-f7af-41ab-8c1c-763db21e7497`: reuse CodeWith CLI/app-server
  harnesses in the macOS app.
- `068cfd91-37b5-4a81-b9cd-cb2ba2bab9da`: wire macOS Goals and Loops creation
  to app-server APIs.
- `768a69f0-ec95-481d-9179-b0c61b950edc`: surface macOS goal plans and
  activate-node actions.
- `8deb5727-7acc-4ad2-a050-bd0096271d7f`: harden remaining inert macOS settings
  and action controls.
- `b77ee917-5e1e-42f4-b73c-691d4f9b63fd`: adversarially review macOS app fixes.

New non-duplicate follow-up candidate:

- Add CodeWith macOS default installer URL selection to `codewith app`.
  Acceptance should require CodeWith-owned arm64/x64 DMG URLs, no fallback to
  OpenAI-hosted Codex installers, existing explicit URL override behavior, and
  Rust tests for architecture/default-url selection.

## Validation Notes

This audit intentionally did not build the Swift app or run Rust tests because
it changes documentation only. Suitable validation for this change is:

- `git diff --check`
- staged secrets scan before commit
- `git status --short --branch`

If a future change implements installer defaults in Rust, validate with
`cd codex-rs && just fmt` and targeted `just test-fast -p codex-cli`. If Swift
behavior changes, validate on an Xcode/macOS 26 host with `swift test` or the
existing apple03 scripts.
