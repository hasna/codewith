# Cursor Composer external-agent runtime

Status: **design + first slice**. This document specifies how Cursor (Composer
2.5, `@cursor/sdk`, and the Cursor CLI) maps onto Codewith's provider/runtime
model. The ACP path described here is largely implemented; the Composer SDK path
is designed and scaffolded but intentionally gated off. See the
[Designed / implemented / remaining](#designed--implemented--remaining) matrix
for exactly what ships today.

Scaffold: `codex-rs/external-agent/src/cursor.rs`
Contract: `codex-rs/external-agent/src/contract.rs`
Shared ACP harness: `codex-rs/external-agent/src/acp.rs`
Runtime registry: `codex-rs/external-agent/src/runtimes.rs`
App-server bridge: `codex-rs/app-server/src/request_processors/thread_external_agent_processor.rs`

## 1. Decision: Cursor is an external agent runtime, not an HTTP model

Cursor is **not** modelled as an OpenAI-compatible HTTP model endpoint. It is an
agent harness with its own sessions, tools, prompts, permissions, and process
lifecycle. Provider execution in Codewith resolves to one of two shapes:

- `OpenAiCompatible { wire_api }` — a real HTTP model provider. `WireApi` stays
  restricted to `responses` and `chat` (`model-provider-info/src/lib.rs`).
- `ExternalAgent { runtime }` — an agent harness selected by
  `ExternalAgentRuntimeId` (`contract.rs`). Cursor is
  `ExternalAgent { runtime: "cursor" }`.

Because Cursor is an `ExternalAgent`, it deliberately does **not** flow through
`ResponseEvent` (the internal HTTP-model contract). It uses the agent-native
`ExternalAgentEvent` stream instead, and only the genuinely shared UI/history
surfaces are adapted (see §5).

### Adversarial decision: ACP first, SDK deferred

Two Cursor surfaces exist. We ship the one that lets Codewith stay the enforcing
client:

| Surface | Transport | Codewith-as-client fit | Status |
| --- | --- | --- | --- |
| **Cursor ACP** (`cursor-agent … acp`) | JSON-RPC over stdio | Strong: session lifecycle, modes, streaming, cancellation, explicit `session/request_permission`, host-served `fs/*` and `terminal/*` | **Shipping** |
| **Composer SDK** (`@cursor/sdk`) | In-process Node / Cursor cloud | Weak today: tools execute in-process with no per-call approval callback (only file hooks + optional local sandbox) | **Deferred** |

ACP is the better fit because every side-effecting action is a server→client
request Codewith can approve, deny, or execute itself. The SDK executes tools
in-process and cannot yet match Codewith native enforcement, so local SDK
execution is deferred until parity is proven (§6). `@cursor/sdk` still matters
later for model discovery, cloud agents, and CI automation.

## 2. The common runtime contract

All runtimes implement the same narrow trait pair from `contract.rs`:

```rust
pub trait ExternalAgentRuntime {
    fn id(&self) -> ExternalAgentRuntimeId;
    fn readiness(&self) -> impl Future<Output = ExternalAgentReadiness> + Send;
    fn run(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
    ) -> impl Future<Output = Result<ExternalAgentResult, ExternalAgentError>> + Send;
}

pub trait ExternalAgentHarness: ExternalAgentRuntime {
    fn harness_kind(&self) -> ExternalAgentHarnessKind; // AcpStdio | Sdk | Cloud
}
```

The runtime never touches the workspace directly. It reports progress and
requests side effects through the host:

```rust
pub trait ExternalAgentHost {
    fn emit(&self, event: ExternalAgentEvent) -> …;
    fn request_permission(&self, request: ExternalAgentPermissionRequest) -> …<ExternalAgentPermissionDecision>;
    fn perform_action(&self, action: ExternalAgentActionRequest) -> …<ExternalAgentActionResult>;
    fn is_cancelled(&self) -> …<bool>;
}
```

This keeps transcript, approval, and audit ownership inside Codewith regardless
of which harness family a runtime belongs to.

### Modes and capabilities

`ExternalAgentMode` (`Consult`, `Plan`, `Propose`, `Managed`) derives a
capability policy via `ExternalAgentCapabilities::for_mode`. Cursor advertises
only `Plan` and `Propose` today (`runtimes.rs`); `Managed` stays gated until
process-sandbox enforcement is complete (enforced by
`visible_runtimes_do_not_advertise_managed_mode`). Capabilities are strictly
increasing and never grant direct mutation without host mediation.

## 3. How Cursor sits on the common ACP harness

Cursor reuses `AcpStdioHarness` verbatim; there is no bespoke Cursor protocol
code. `cursor_acp_harness()` resolves the descriptor and wraps it.

Descriptor (`runtimes.rs`):

```
id: "cursor", display_name: "Cursor"
command: program "agent", args ["acp"]   (also resolves "cursor-agent")
supported_modes: [Plan, Propose], default Plan, visible: true
```

Run sequence (`AcpStdioHarness::run_protocol`):

1. `initialize` — advertises client capabilities derived from the mode
   (`fs.readTextFile`, `fs.writeTextFile`, `terminal`).
2. `authenticate` — only if the server offers `authMethods`; Cursor prefers
   `cursor_login`, then `cached_token` (`acp_auth_method`).
3. `session/new` or `session/load` (resume) → `ExternalAgentSessionState` with
   `external_session_id`. Missing `sessionId` is a protocol error.
4. `session/set_mode` — `Plan` maps to ACP mode `plan`.
5. `session/prompt` — sends the task text.
6. Streaming `session/update` notifications and server requests are handled
   until the response arrives.

Process hardening the harness applies before any bytes flow:

- **Env sanitization** — only `LANG/LC_*/PATH/TERM` plus explicit extras survive;
  `CURSOR_API_KEY` / `CURSOR_AUTH_TOKEN` are injected deliberately
  (`acp_runtime_auth_env_vars`).
- **Per-run isolation** — a private `HOME`/XDG/temp tree
  (`AcpProcessIsolation`) so the agent cannot read or mutate the real home.
- **Platform sandbox** — the launch is refused unless wrapped by Codewith's
  platform sandbox (`ExternalAgentLaunchIsolation`); the unwrapped spec cannot be
  spawned.
- **Path confinement** — `fs/*` paths are lexically confined to `cwd`
  (`confine_path`).
- **Cancellation** — `is_cancelled` is polled each second between reads and the
  child process group is killed on shutdown.

## 4. Auth

One Cursor auth profile drives both surfaces:

- **Interactive**: `cursor-agent login` (browser). ACP then authenticates with
  `cursor_login`.
- **Headless / CI**: `CURSOR_API_KEY` (or `CURSOR_AUTH_TOKEN`). Passed through to
  the child; ACP authenticates with `cached_token`.

App-server gating (`thread_external_agent_processor.rs`):

- `validate_external_agent_subscription_profile` requires the selected auth
  profile to be a **Cursor** subscription (a **ChatGPT** profile is also
  accepted); a mismatched profile is rejected with a clear message.
- Readiness must be `Ready` before a run starts; otherwise the run is *gated*
  with the readiness detail (missing runtime / missing auth / disabled).
- Cursor config dirs (`~/.cursor`, `~/.cursor-agent`, and the XDG equivalents)
  are added as **read-only** roots so the agent can read its own config without
  gaining write access.

Secrets are never logged; only env-var *names* are referenced, never values.

## 5. Tool / approval semantics and streaming

The harness translates ACP traffic into the agent-native event/action contract.
Codewith owns the decision and (eventually) the execution of every side effect.

Streaming (`handle_session_update`):

| ACP `session/update` | `ExternalAgentEvent` | UI/history surface |
| --- | --- | --- |
| `agent_message_chunk` / `assistant_message_chunk` | `OutputTextDelta` | assistant text |
| `reasoning_chunk` / `thinking_chunk` | `ReasoningDelta` | reasoning stream |
| `tool_call` | `ProposedAction { Other }` | proposed-action card |

Server requests (`handle_server_request`) — Cursor asks, Codewith answers:

| ACP server request | Host path | Capability gate |
| --- | --- | --- |
| `session/request_permission` | `request_permission` | always (approval bridge) |
| `fs/read_text_file` | `perform_action(ReadFile)` | `filesystem != None` |
| `fs/write_text_file` | permission + `perform_action(WriteFile)` | `filesystem == ManagedReadWrite` |
| `terminal/create` | permission + `perform_action(RunCommand)` | `terminal == Managed` |
| `terminal/output` / `wait_for_exit` / `kill` / `release` | host-recorded terminal buffer | — |
| anything else | JSON-RPC error `-32601` | rejected |

The app-server bridge parks each permission request for a bounded window
(`EXTERNAL_AGENT_PERMISSION_TIMEOUT`, 5 min), **default-denies** on
timeout/cancellation, and emits `PermissionRequested` / `PermissionResolved`
audit events. Session id, cancellation, and permission bridging are wired
end-to-end.

## 6. Composer SDK path (designed, scaffolded, deferred)

`@cursor/sdk` (public beta, Composer 2.5 default) exposes: `Agent.create` /
`Agent.resume` / `Agent.prompt`, `agent.send → Run`, `run.stream` / `run.wait` /
`run.cancel`, stream messages (`system`, `user`, `assistant`, `thinking`,
`tool_call`, `status`, `usage`), MCP servers, custom tools, and
`Cursor.models.list()` for model discovery. Runs execute either **local**
(in-process, disk access) or **cloud** (Cursor VM, cloned repo, downloadable
artifacts); the runtime is inferred from the agent id prefix (`bc-` = cloud).

### Why it is deferred

The SDK has **no per-call approval callback**. Local agents execute built-in
tools (shell, file write, edit) directly; the only controls are file-based hooks
(`.cursor/hooks.json`), an `autoReview` classifier, and an optional local
sandbox. That is weaker than the ACP model, where every action is a
server→client request Codewith mediates. Cloud execution moves code and tool
execution entirely onto Cursor infrastructure — a different trust boundary (data
egress) that is only appropriate for explicit cloud-agent/automation use, never
the default in-editor runtime.

### What SDK parity requires (remaining)

- **Local**: run `@cursor/sdk` inside a Codewith-owned Node sidecar; disable the
  built-in mutating tools and expose **only** Codewith custom tools whose
  `execute()` routes through `ExternalAgentHost::perform_action` (turning
  in-process calls into approvable actions); enable `local.sandboxOptions`; map
  SDK stream messages to `ExternalAgentEvent`. Harness kind: `Sdk`.
- **Cloud**: gate behind explicit data-egress consent; map `status` → `Status`,
  artifacts → `ExternalAgentArtifact`, `run.cancel` → cancellation; persist the
  cloud agent id as `external_session_id`. Harness kind: `Cloud`.
- **Model discovery**: replace `cursor_composer_seed_models()` with a live
  `Cursor.models.list()` query surfaced as `CursorComposerModel`.

### Scaffold

`cursor.rs` makes the design concrete and type-checked without faking a runtime:

- `CursorComposerExecution { LocalSdk, Cloud }` → `harness_kind()`.
- `CursorComposerModel` + `cursor_composer_seed_models()` (default
  `composer-2.5`) — the shape model discovery will fill in.
- `CursorComposerSdkHarness` implements `ExternalAgentRuntime` /
  `ExternalAgentHarness` but is inert: `readiness()` reports
  `ExternalAgentReadinessStatus::Disabled` and `run()` returns
  `ExternalAgentError::NotReady` with `CURSOR_COMPOSER_SDK_DEFERRED_REASON`. It
  is **not** registered in `BUILTIN_EXTERNAL_AGENT_RUNTIMES`, so it is not
  selectable; it only documents intent and lets the rest of Codewith compile
  against the eventual shape.

## 7. Session persistence

`ExternalAgentSessionState.external_session_id` is the durable handle. ACP
returns it from `session/new`; the app-server bridge emits `SessionResolved`.
Resume uses `session/load` with the stored id. Historical Cursor sessions on
disk (project JSONL) are imported by `external-agent-sessions` (`detect.rs`,
capped and ledgered) so prior Cursor work shows up in Codewith history.

## Designed / implemented / remaining

| Area | Designed | Implemented | Remaining |
| --- | --- | --- | --- |
| Provider model (`ExternalAgent` vs `OpenAiCompatible`) | yes | done (`contract.rs`, registry) | — |
| Agent-native event stream (not `ResponseEvent`) | yes | done (`ExternalAgentEvent`) | — |
| Cursor ACP run loop (init/auth/session/prompt) | yes | done (`acp.rs`) | — |
| Streaming (text / reasoning / tool_call) | yes | done | — |
| Permission bridge (request/timeout/cancel/audit) | yes | done (`thread_external_agent_processor.rs`) | — |
| Session id persistence + resume | yes | done | — |
| Auth (interactive + `CURSOR_API_KEY`) + profile gating | yes | done | — |
| Sandbox / env isolation / path confinement | yes | done | — |
| Host-executed `fs/terminal` actions (Propose) | yes | partial — app-server `perform_action` currently returns `Rejected` ("managed action routing is not enabled yet") | wire `perform_action` to native Codewith fs/exec |
| `Managed` mode (writes + terminal) | yes | gated | enable after process-sandbox enforcement |
| Composer SDK local runtime | yes | scaffold only (`cursor.rs`, inert) | custom-tool to host bridge, sandbox, event mapping |
| Composer SDK cloud runtime | yes | scaffold only | egress consent, artifacts, status mapping |
| Model discovery (`Cursor.models.list()`) | yes | seed list only | live discovery to `CursorComposerModel` |

Status words: **done** shipped · **partial** wired but incomplete · **scaffold**
inert stub · **gated** intentionally disabled.
