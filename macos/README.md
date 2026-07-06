# CodeWith — macOS app

A native macOS (macOS 26 "Liquid Glass") SwiftUI app for CodeWith, the open
fork of Codex. The UI targets **1:1 visual parity** with the OpenAI Codex macOS
app, adapted for the fork:

- **Apps** (the Codex "Plugins" entry, renamed)
- **Machines** — a new sidebar entry for multi-machine fleet sessions
- **Loops / Goals / Workflows**, **Profiles** switching — fork capabilities
- CodeWith branding throughout

## Layout

```
macos/
  CodeWith/                     SwiftPM executable (SwiftUI), tools 6.2, .macOS(.v26)
    Sources/CodeWith/
      App/                      @main entry + ImageRenderer snapshot harness + catalog
      DesignSystem/             Theme tokens, snapshot env / ScrollColumn
      Shell/                    WindowFrame, Sidebar, RootView, Composer
      Screens/                  Home, Chat, AddMenu, TaskResult, Login
        Settings/               General, Profile, Appearance, Configuration, Personalization
  scripts/
    shoot.sh                    sync → build → ImageRenderer snapshots → pull PNGs
    run-on-apple03.sh           build → .app bundle + bundled CLI (pass --launch for GUI)
    capture-display.sh          real screencapture of the running app over SSH
```

## Building & screenshotting

Authored on spark01, built on **apple03** (the only fleet Mac with full Xcode 26).
macOS apps can't be built on Linux, and plain `screencapture` over SSH is denied
(no Aqua session + TCC Screen Recording), so screens are rendered **in-process**
via `ImageRenderer` — pixel-exact, permission-free, deterministic. For real
display pixels of the *running* app, `capture-display.sh` routes `screencapture`
through the user's tmux server (`tmux run-shell -b`), whose TCC-responsible
process is the GUI terminal that spawned it — that grant carries Screen Recording.

```bash
bash macos/scripts/shoot.sh                   # renders every screen → design-refs/renders/
bash macos/scripts/run-on-apple03.sh          # builds the .app bundle on apple03
bash macos/scripts/run-on-apple03.sh --cli-path /path/to/codewith
bash macos/scripts/run-on-apple03.sh --launch # opens the windowed app on apple03
```

`run-on-apple03.sh` requires a `codewith` CLI on the build host, or an explicit
`CODEWITH_CLI_PATH` / `--cli-path`. It copies that CLI into `CodeWith.app/Contents/Resources/codewith`
so the Swift app talks to the existing `codewith app-server` harness instead of
depending on whichever external install happens to be first on `PATH`.

### Runtime compatibility

The Machines screen uses the app-server machine registry. Older bundled or
external `codewith app-server` builds that predate `machineRegistry/list` cannot
return fleet data, so the macOS app degrades without treating that as a startup
failure. Rebuild the app with a current CLI, or set `CODEWITH_CLI_PATH` to a
newer CLI before running `macos/scripts/run-on-apple03.sh`, to restore the full
Machines view.

Reference captures live in `design-refs/screenshots/`; our renders land in
`design-refs/renders/` for side-by-side parity comparison. See the project memory
note `macos-app-build-pipeline` for the full mechanics (Tailscale IPs, `DEVELOPER_DIR`,
the `ScrollColumn`/`snapshotMode` fallback for `ImageRenderer`).

## Screens (parity targets)

| # | Screen | Reference |
|---|--------|-----------|
| 01 | Home — "What should we work on?" | 01 |
| 02 | Chat / session + Outputs/Sources panel | 02 |
| 03 | Composer "+" Add menu (Files / Goal / Plan + Agents) | 03 |
| 04 | Cloud task result (Notes / Summary / Testing / diff / Apply) | 04 |
| 05–09 | Settings: General, Profile, Appearance, Configuration, Personalization | 05–09 |
| 10 | Task result + Cloud-changes diff (3-pane) | 10 |
| 11 | Login — "Get started with CodeWith" | 11 |
| 12 | Machines | fork |
| 13 | Profiles | fork |
| 14 | Apps | fork |
| 15 | Loops | fork |
| 16 | Goals | fork |
| 17 | Search | fork |
| 18 | Workflows | fork |
