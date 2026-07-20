//! Local `@cursor/sdk` (Cursor Composer) execution backend.
//!
//! Cursor Composer's local surface is the `@cursor/sdk` Node package, not an ACP
//! stdio process and not an HTTP model endpoint. Codewith runs it inside a
//! **Codewith-owned Node sidecar**: a small bridge script (embedded as
//! [`CURSOR_SDK_BRIDGE_JS`]) that imports `@cursor/sdk`, creates the agent with
//! the SDK's built-in mutating tools (`shell`/`write`/`edit`) **disabled**, and
//! exposes only Codewith custom tools. Every custom tool call is bridged over a
//! line-delimited JSON protocol back to this process, which forwards it to
//! [`ExternalAgentHost::request_permission`] and
//! [`ExternalAgentHost::perform_action`] so the Comp2 guard/executor stays the
//! *sole* enforcer — the sidecar never touches the filesystem, terminal, MCP, or
//! network itself.
//!
//! The same sidecar and stdio protocol are reused by the cloud backend
//! ([`crate::cursor_cloud`]) via [`run_cursor_sidecar`]; the only differences are
//! the config payload and whether local builtin-tool disabling is asserted.
//!
//! # Sidecar protocol
//!
//! Codewith → sidecar (stdin, one JSON object per line):
//! * first line: the config object built by [`CursorSdkBackend::local_config`]
//!   (or the cloud equivalent);
//! * `{"type":"tool-response","id":..,"result":<ExternalAgentActionResult>}`;
//! * `{"type":"cancel"}`.
//!
//! Sidecar → Codewith (stdout, one JSON object per line):
//! * `{"type":"ready","runtime":..,"builtinToolsDisabled":bool,"customTools":[..]}`
//! * `{"type":"session","agentId":".."}`
//! * `{"type":"output-delta","text":".."}`
//! * `{"type":"reasoning-delta","text":".."}`
//! * `{"type":"status","message":".."}`
//! * `{"type":"artifact","artifact":<ExternalAgentArtifact>}`
//! * `{"type":"tool-request","id":..,"action":<ExternalAgentActionRequest>}`
//! * `{"type":"completed","summary":..,"artifacts":[..]}`
//! * `{"type":"failed","message":".."}`

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::Lines;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::CURSOR_COMPOSER_SDK_AUTH_ENV_VARS;
use crate::EnvironmentCapability;
use crate::ExternalAgentActionRequest;
use crate::ExternalAgentActionResult;
use crate::ExternalAgentArtifact;
use crate::ExternalAgentCapabilities;
use crate::ExternalAgentError;
use crate::ExternalAgentEvent;
use crate::ExternalAgentHost;
use crate::ExternalAgentLaunchIsolation;
use crate::ExternalAgentLaunchSpec;
use crate::ExternalAgentPermissionDecision;
use crate::ExternalAgentPermissionOption;
use crate::ExternalAgentPermissionRequest;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentReadinessStatus;
use crate::ExternalAgentRequest;
use crate::ExternalAgentResult;
use crate::ExternalAgentRunStatus;
use crate::ExternalAgentRuntimeDescriptor;
use crate::ExternalAgentRuntimeId;
use crate::ExternalAgentSandboxConfig;
use crate::ExternalAgentSandboxedLaunchSpec;
use crate::ExternalAgentSessionRequest;
use crate::ExternalAgentSessionState;
use crate::FileSystemCapability;
use crate::McpCapability;
use crate::NetworkCapability;
use crate::TerminalCapability;
use crate::cursor_composer_runtime_descriptor;
use crate::platform_sandbox_external_agent_launch_with_writable_roots;
use crate::resolve_cursor_composer_model;

/// Node program used to host the `@cursor/sdk` sidecar.
pub const CURSOR_SDK_NODE_PROGRAM: &str = "node";
/// Filename of the embedded sidecar bridge inside its isolation directory.
pub const CURSOR_SDK_BRIDGE_FILE: &str = "cursor_sdk_bridge.mjs";
/// SDK built-in tools that must never be enabled for a Codewith-mediated run.
pub const CURSOR_SDK_DISABLED_BUILTIN_TOOLS: &[&str] = &["shell", "write", "edit"];

const CURSOR_SDK_SAFE_ENV_VARS: &[&str] = &[
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "PATH",
    "TERM",
    "TMPDIR",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "NODE_PATH",
    "NPM_CONFIG_PREFIX",
    "CURSOR_CONFIG_DIR",
];
const CURSOR_SDK_CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CURSOR_SDK_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const CURSOR_SDK_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const CURSOR_SDK_READINESS_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CURSOR_SDK_STDERR_MAX_BYTES: usize = 64 * 1024;

static CURSOR_SDK_ISOLATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Embedded Codewith-owned `@cursor/sdk` sidecar bridge (ES module).
///
/// This is the production sidecar. It targets the documented `@cursor/sdk`
/// (Composer 2.5) surface — `Agent.create`/`Agent.resume`, `agent.send`,
/// `run.stream`, `run.cancel`. The Rust-side protocol handling in this module is
/// what the crate's unit tests exercise (via fake sidecars); this script is
/// validated end-to-end against a live Cursor account by the integrator.
//
// Kept inline (rather than a separate asset) so the crate ships a single
// self-contained artifact and does not depend on a non-Rust file that would fall
// outside this component's ownership boundary.
pub const CURSOR_SDK_BRIDGE_JS: &str = r#"// Codewith-owned @cursor/sdk sidecar bridge.
//
// Every mutating action is bridged to the Codewith host over stdio; this
// process never touches the filesystem, terminal, MCP, or network itself. The
// SDK's built-in mutating tools are disabled and only Codewith custom tools are
// exposed.
import readline from 'node:readline';

function send(message) {
  process.stdout.write(JSON.stringify(message) + '\n');
}

function fail(message) {
  send({ type: 'failed', message: String(message && message.stack ? message.stack : message) });
  process.exit(1);
}

process.on('uncaughtException', fail);
process.on('unhandledRejection', fail);

const rl = readline.createInterface({ input: process.stdin });
const buffered = [];
let waiter = null;
rl.on('line', (line) => {
  if (waiter) {
    const resolve = waiter;
    waiter = null;
    resolve(line);
  } else {
    buffered.push(line);
  }
});
let stdinClosed = false;
rl.on('close', () => {
  stdinClosed = true;
  if (waiter) {
    const resolve = waiter;
    waiter = null;
    resolve(undefined);
  }
});
function nextLine() {
  if (buffered.length) return Promise.resolve(buffered.shift());
  if (stdinClosed) return Promise.resolve(undefined);
  return new Promise((resolve) => {
    waiter = resolve;
  });
}

const pending = new Map();
let toolSeq = 0;

function actionForTool(name, args) {
  args = args || {};
  switch (name) {
    case 'read_file':
      return { type: 'read-file', path: args.path };
    case 'write_file':
      return { type: 'write-file', path: args.path, content: args.content == null ? '' : String(args.content) };
    case 'run_command': {
      const command = Array.isArray(args.command)
        ? args.command.map(String)
        : [String(args.command == null ? '' : args.command)];
      return { type: 'run-command', command, cwd: args.cwd == null ? null : String(args.cwd) };
    }
    case 'mcp_tool':
      return { type: 'mcp-tool-call', server: String(args.server), tool: String(args.tool), arguments: args.arguments == null ? {} : args.arguments };
    case 'network':
      return { type: 'network-access', target: String(args.target), purpose: args.purpose == null ? null : String(args.purpose) };
    default:
      return { type: 'other', label: name, payload: args };
  }
}

function mapResult(result) {
  if (!result || typeof result !== 'object') {
    throw new Error('host rejected the tool call');
  }
  switch (result.type) {
    case 'file-content':
      return result.content;
    case 'write-accepted':
      return { ok: true };
    case 'command-output':
      return { exitCode: result.exit_code, stdout: result.stdout, stderr: result.stderr };
    case 'mcp-tool-result':
      return result.result;
    case 'network-access-ready':
      return { ok: true };
    case 'rejected':
      throw new Error('Codewith host rejected the action: ' + (result.reason || 'no reason'));
    default:
      throw new Error('unexpected host result: ' + result.type);
  }
}

function makeTool(name) {
  return {
    name,
    description: 'Codewith-mediated ' + name + ' (executed by Codewith, not by Cursor).',
    parameters: { type: 'object', additionalProperties: true },
    run: async (args) => {
      const id = 'tool-' + (++toolSeq);
      send({ type: 'tool-request', id, action: actionForTool(name, args) });
      const result = await new Promise((resolve) => pending.set(id, resolve));
      return mapResult(result);
    },
  };
}

async function main() {
  const configLine = await nextLine();
  if (configLine === undefined) {
    fail('missing sidecar config');
    return;
  }
  const config = JSON.parse(configLine);

  let sdk;
  try {
    sdk = await import('@cursor/sdk');
  } catch (err) {
    fail('@cursor/sdk is not installed or failed to load: ' + err);
    return;
  }
  const Agent = sdk.Agent || (sdk.default && sdk.default.Agent);
  if (!Agent) {
    fail('@cursor/sdk did not export an Agent constructor');
    return;
  }

  const customToolNames = Array.isArray(config.customTools) ? config.customTools : [];
  const tools = customToolNames.map(makeTool);

  // Drain stdin for tool responses and cancellation.
  let currentRun = null;
  (async () => {
    for (;;) {
      const line = await nextLine();
      if (line === undefined) return;
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        continue;
      }
      if (message.type === 'tool-response') {
        const resolve = pending.get(message.id);
        if (resolve) {
          pending.delete(message.id);
          resolve(message.result);
        }
      } else if (message.type === 'cancel') {
        if (currentRun && typeof currentRun.cancel === 'function') {
          try {
            await currentRun.cancel();
          } catch (err) {
            // best effort
          }
        }
      }
    }
  })();

  send({
    type: 'ready',
    runtime: config.runtime,
    builtinToolsDisabled: true,
    customTools: tools.map((tool) => tool.name),
  });

  const options = {
    model: config.model,
    cwd: config.cwd,
    tools,
    builtinTools: false,
    disabledTools: config.disabledBuiltinTools || ['shell', 'write', 'edit'],
    runtime: config.runtime === 'cloud' ? 'cloud' : 'local',
  };

  let agent;
  const session = config.session || { type: 'new' };
  if (session.type === 'resume' && session.externalSessionId) {
    agent = typeof Agent.resume === 'function'
      ? await Agent.resume(session.externalSessionId, options)
      : await Agent.create({ ...options, resume: session.externalSessionId });
  } else {
    agent = await Agent.create(options);
  }
  const agentId = agent.id || (agent.session && agent.session.id);
  if (agentId) {
    send({ type: 'session', agentId: String(agentId) });
  }

  const artifacts = [];
  let summary = '';

  currentRun = agent.send(config.task);
  const stream = typeof currentRun.stream === 'function' ? currentRun.stream() : currentRun;
  for await (const event of stream) {
    const kind = event.type || event.kind;
    if (kind === 'assistant' || kind === 'message' || kind === 'output' || kind === 'text') {
      const text = event.text || (event.delta && event.delta.text) || event.content;
      if (typeof text === 'string' && text.length) {
        summary += text;
        send({ type: 'output-delta', text });
      }
    } else if (kind === 'reasoning' || kind === 'thinking') {
      const text = event.text || (event.delta && event.delta.text);
      if (typeof text === 'string' && text.length) {
        send({ type: 'reasoning-delta', text });
      }
    } else if (kind === 'status' || kind === 'state') {
      send({ type: 'status', message: String(event.message || event.status || kind) });
    } else if (kind === 'artifact' || kind === 'pull_request' || kind === 'diff') {
      send({
        type: 'artifact',
        artifact: {
          label: String(event.label || event.title || kind),
          path: event.path || null,
          mimeType: event.mimeType || event.mime_type || null,
          uri: event.uri || event.url || null,
        },
      });
    }
  }

  if (typeof currentRun.result === 'function') {
    try {
      const finalResult = await currentRun.result();
      if (finalResult && typeof finalResult.summary === 'string') {
        summary = finalResult.summary;
      }
    } catch (err) {
      // Streaming already produced output; ignore result() errors.
    }
  }

  send({ type: 'completed', summary, artifacts });
  process.exit(0);
}

main().catch(fail);
"#;

/// Environment policy for Cursor Composer sidecar launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSdkEnvironmentPolicy {
    inherited_vars: Vec<String>,
}

impl CursorSdkEnvironmentPolicy {
    pub fn sanitized() -> Self {
        Self {
            inherited_vars: CURSOR_SDK_SAFE_ENV_VARS
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        }
    }

    fn sanitize(&self, source: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        for name in &self.inherited_vars {
            if let Some(value) = source.get(name) {
                env.insert(name.clone(), value.clone());
            }
        }
        for name in CURSOR_COMPOSER_SDK_AUTH_ENV_VARS {
            if let Some(value) = source.get(*name)
                && !value.trim().is_empty()
            {
                env.insert((*name).to_string(), value.clone());
            }
        }
        env.insert(
            "CODEWITH_EXTERNAL_AGENT_RUNTIME".to_string(),
            ExternalAgentRuntimeId::CURSOR.to_string(),
        );
        env
    }
}

impl Default for CursorSdkEnvironmentPolicy {
    fn default() -> Self {
        Self::sanitized()
    }
}

/// Whether any Cursor SDK auth credential is present in the environment.
pub fn has_cursor_sdk_auth_env(source_env: &BTreeMap<String, String>) -> bool {
    CURSOR_COMPOSER_SDK_AUTH_ENV_VARS
        .iter()
        .any(|name| source_env.get(*name).is_some_and(|value| !value.trim().is_empty()))
}

/// Custom Codewith tools exposed to the local SDK agent for a run.
///
/// Only the tools permitted by the run's capabilities are exposed; each maps to
/// an [`ExternalAgentActionRequest`] that is executed by the Codewith host.
pub fn cursor_sdk_custom_tools(capabilities: &ExternalAgentCapabilities) -> Vec<String> {
    let mut tools = Vec::new();
    if !matches!(capabilities.filesystem, FileSystemCapability::None) {
        tools.push("read_file".to_string());
    }
    if matches!(capabilities.filesystem, FileSystemCapability::ManagedReadWrite) {
        tools.push("write_file".to_string());
    }
    if matches!(capabilities.terminal, TerminalCapability::Managed) {
        tools.push("run_command".to_string());
    }
    if !matches!(capabilities.mcp, McpCapability::None) {
        tools.push("mcp_tool".to_string());
    }
    if !matches!(capabilities.network, NetworkCapability::None) {
        tools.push("network".to_string());
    }
    tools
}

/// The local `@cursor/sdk` execution backend.
#[derive(Debug, Clone)]
pub struct CursorSdkBackend {
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: CursorSdkEnvironmentPolicy,
}

impl CursorSdkBackend {
    pub fn new() -> Self {
        Self {
            descriptor: cursor_composer_runtime_descriptor(),
            env_policy: CursorSdkEnvironmentPolicy::default(),
        }
    }

    pub fn descriptor(&self) -> &'static ExternalAgentRuntimeDescriptor {
        self.descriptor
    }

    pub async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        cursor_sidecar_readiness_with_env(self.descriptor, &self.env_policy, source_env).await
    }

    pub async fn run_sandboxed_with_env(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
        source_env: BTreeMap<String, String>,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        validate_cursor_request(self.descriptor, &request)?;
        let config = self.local_config(&request);
        run_cursor_sidecar(
            self.descriptor,
            &self.env_policy,
            request,
            host,
            sandbox_config,
            source_env,
            config,
            /*expect_builtin_tools_disabled*/ true,
        )
        .await
    }

    /// Build the config payload sent to the local sidecar as its first line.
    pub fn local_config(&self, request: &ExternalAgentRequest) -> JsonValue {
        let model = resolve_cursor_composer_model(request.model.as_deref());
        json!({
            "type": "config",
            "runtime": "local",
            "task": request.task,
            "model": model,
            "cwd": request.cwd.to_string_lossy(),
            "mode": request.mode,
            "session": session_config(&request.session),
            "disabledBuiltinTools": CURSOR_SDK_DISABLED_BUILTIN_TOOLS,
            "customTools": cursor_sdk_custom_tools(&request.capabilities),
        })
    }
}

impl Default for CursorSdkBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize a session request into the sidecar config shape.
pub(crate) fn session_config(session: &ExternalAgentSessionRequest) -> JsonValue {
    match session {
        ExternalAgentSessionRequest::New => json!({ "type": "new" }),
        ExternalAgentSessionRequest::Resume {
            external_session_id,
        } => json!({ "type": "resume", "externalSessionId": external_session_id }),
    }
}

/// Validate a request targets this Cursor runtime with consistent capabilities.
///
/// Capability enforcement itself belongs to the Comp2 host guard/executor; this
/// only rejects requests that are malformed for the harness (wrong runtime id,
/// unsanitized environment, or capabilities that do not match the selected
/// mode).
pub(crate) fn validate_cursor_request(
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    request: &ExternalAgentRequest,
) -> Result<(), ExternalAgentError> {
    if request.runtime.as_str() != descriptor.id {
        return Err(runtime_error(
            &request.runtime,
            format!(
                "request runtime does not match harness runtime `{}`",
                descriptor.id
            ),
        ));
    }
    if !matches!(
        request.capabilities.environment,
        EnvironmentCapability::Sanitized
    ) {
        return Err(runtime_error(
            &request.runtime,
            "Cursor Composer runs require sanitized environment capabilities",
        ));
    }
    if request.capabilities != ExternalAgentCapabilities::for_mode(request.mode) {
        return Err(runtime_error(
            &request.runtime,
            "request capabilities must match the selected external-agent mode",
        ));
    }
    Ok(())
}

/// Readiness of the Cursor Composer sidecar for a given environment.
///
/// Reports [`ExternalAgentReadinessStatus::MissingRuntime`] when Node or
/// `@cursor/sdk` cannot be resolved, [`ExternalAgentReadinessStatus::MissingAuth`]
/// when no Cursor credential is present, and
/// [`ExternalAgentReadinessStatus::Ready`] otherwise.
pub(crate) async fn cursor_sidecar_readiness_with_env(
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: &CursorSdkEnvironmentPolicy,
    source_env: &BTreeMap<String, String>,
) -> ExternalAgentReadiness {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let node = match resolve_node_program(source_env, cwd.as_path()) {
        Ok(node) => node,
        Err(err) => return readiness(descriptor, ExternalAgentReadinessStatus::MissingRuntime, err),
    };
    if let Err(message) = probe_cursor_sdk_installed(&node, env_policy, source_env).await {
        return readiness(descriptor, ExternalAgentReadinessStatus::MissingRuntime, message);
    }
    if !has_cursor_sdk_auth_env(source_env) {
        return readiness(
            descriptor,
            ExternalAgentReadinessStatus::MissingAuth,
            format!(
                "no Cursor credential found; set one of: {}",
                CURSOR_COMPOSER_SDK_AUTH_ENV_VARS.join(", ")
            ),
        );
    }
    readiness(
        descriptor,
        ExternalAgentReadinessStatus::Ready,
        node.display().to_string(),
    )
}

fn readiness(
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    status: ExternalAgentReadinessStatus,
    detail: String,
) -> ExternalAgentReadiness {
    let mut readiness = descriptor.readiness(status);
    readiness.detail = Some(detail);
    readiness
}

fn resolve_node_program(
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let path = source_env.get("PATH").map(String::as_str);
    which::which_in(CURSOR_SDK_NODE_PROGRAM, path, cwd)
        .map_err(|err| format!("could not resolve `{CURSOR_SDK_NODE_PROGRAM}`: {err}"))
}

async fn probe_cursor_sdk_installed(
    node: &Path,
    env_policy: &CursorSdkEnvironmentPolicy,
    source_env: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut command = Command::new(node);
    command
        .args([
            "-e",
            "import('@cursor/sdk').then(()=>process.exit(0)).catch(()=>process.exit(3))",
        ])
        .env_clear()
        .envs(env_policy.sanitize(source_env))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output =
        match tokio::time::timeout(CURSOR_SDK_READINESS_PROBE_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(format!("Cursor SDK readiness probe failed: {err}")),
            // A slow probe should not hard-fail readiness; treat as available.
            Err(_) => return Ok(()),
        };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        Err("`@cursor/sdk` is not installed for the resolved Node runtime".to_string())
    } else {
        Err(format!("`@cursor/sdk` is not usable: {detail}"))
    }
}

/// Spawn the Codewith-owned Cursor sidecar and drive one run to completion.
///
/// Shared by the local ([`CursorSdkBackend`]) and cloud
/// ([`crate::CursorCloudBackend`]) backends. The `config` value is written as the
/// sidecar's first stdin line; `expect_builtin_tools_disabled` asserts the local
/// safety invariant (the cloud runtime runs builtin tools remotely on Cursor's
/// VM and does not require it).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_cursor_sidecar(
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: &CursorSdkEnvironmentPolicy,
    request: ExternalAgentRequest,
    host: impl ExternalAgentHost + Send + Sync,
    sandbox_config: &ExternalAgentSandboxConfig,
    source_env: BTreeMap<String, String>,
    config: JsonValue,
    expect_builtin_tools_disabled: bool,
) -> Result<ExternalAgentResult, ExternalAgentError> {
    let node = resolve_node_program(&source_env, &request.cwd).map_err(|reason| {
        ExternalAgentError::NotReady {
            runtime: request.runtime.as_str().to_string(),
            reason,
        }
    })?;
    let isolation = CursorSdkIsolation::create(&request.runtime)?;
    let launch = launch_spec(
        descriptor,
        env_policy,
        request.cwd.clone(),
        node,
        isolation.bridge_path(),
        &source_env,
    );
    let launch = platform_sandbox_external_agent_launch_with_writable_roots(
        launch,
        sandbox_config,
        vec![isolation.root.clone()],
    )?;
    let result = run_sandboxed_launch(request, host, launch, config, expect_builtin_tools_disabled)
        .await;
    let _ = std::fs::remove_dir_all(&isolation.root);
    result
}

fn launch_spec(
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: &CursorSdkEnvironmentPolicy,
    cwd: PathBuf,
    node: PathBuf,
    bridge: PathBuf,
    source_env: &BTreeMap<String, String>,
) -> ExternalAgentLaunchSpec {
    ExternalAgentLaunchSpec {
        runtime: ExternalAgentRuntimeId::from(descriptor.id),
        program: node,
        args: vec![bridge.display().to_string()],
        arg0: None,
        cwd,
        env: env_policy.sanitize(source_env),
        isolation: ExternalAgentLaunchIsolation::unenforced(
            "Cursor Composer sidecar launch has not been wrapped in a Codewith platform sandbox",
        ),
    }
}

async fn run_sandboxed_launch(
    request: ExternalAgentRequest,
    host: impl ExternalAgentHost + Send + Sync,
    launch: ExternalAgentSandboxedLaunchSpec,
    config: JsonValue,
    expect_builtin_tools_disabled: bool,
) -> Result<ExternalAgentResult, ExternalAgentError> {
    let mut process = CursorSdkProcess::spawn(launch, expect_builtin_tools_disabled)?;
    let result = process.run(request, &host, config).await;
    if let Err(err) = &result {
        let event = match err {
            ExternalAgentError::Cancelled => ExternalAgentEvent::Cancelled {
                reason: Some("cancelled by host".to_string()),
            },
            _ => ExternalAgentEvent::Failed {
                message: err.to_string(),
            },
        };
        let _ = host.emit(event).await;
    }
    process.shutdown().await;
    result
}

/// Temp directory holding the sidecar bridge script for one run.
struct CursorSdkIsolation {
    root: PathBuf,
}

impl CursorSdkIsolation {
    fn create(runtime: &ExternalAgentRuntimeId) -> Result<Self, ExternalAgentError> {
        let counter = CURSOR_SDK_ISOLATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "codewith-cursor-sdk-{}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).map_err(|err| ExternalAgentError::NotReady {
            runtime: runtime.as_str().to_string(),
            reason: format!("failed to create Cursor sidecar isolation dir: {err}"),
        })?;
        let bridge = root.join(CURSOR_SDK_BRIDGE_FILE);
        std::fs::write(&bridge, CURSOR_SDK_BRIDGE_JS).map_err(|err| {
            let _ = std::fs::remove_dir_all(&root);
            ExternalAgentError::NotReady {
                runtime: runtime.as_str().to_string(),
                reason: format!("failed to write Cursor sidecar bridge: {err}"),
            }
        })?;
        Ok(Self { root })
    }

    fn bridge_path(&self) -> PathBuf {
        self.root.join(CURSOR_SDK_BRIDGE_FILE)
    }
}

/// A running Cursor Composer sidecar process and its stdio protocol state.
pub(crate) struct CursorSdkProcess {
    runtime: ExternalAgentRuntimeId,
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: Option<JoinHandle<String>>,
    output_text: String,
    summary: Option<String>,
    artifacts: Vec<ExternalAgentArtifact>,
    external_session_id: Option<String>,
    expect_builtin_tools_disabled: bool,
    idle_timeout: Duration,
}

impl CursorSdkProcess {
    pub(crate) fn spawn(
        launch: ExternalAgentSandboxedLaunchSpec,
        expect_builtin_tools_disabled: bool,
    ) -> Result<Self, ExternalAgentError> {
        let launch = launch.into_launch_spec();
        let runtime = launch.runtime.clone();
        if let Some(reason) = launch.isolation.unenforced_reason() {
            return Err(ExternalAgentError::NotReady {
                runtime: runtime.as_str().to_string(),
                reason: reason.to_string(),
            });
        }
        let mut command = Command::new(&launch.program);
        #[cfg(unix)]
        if let Some(arg0) = launch.arg0 {
            command.arg0(arg0);
        }
        #[cfg(not(unix))]
        let _ = launch.arg0;
        #[cfg(unix)]
        unsafe {
            command.pre_exec(codex_utils_pty::process_group::set_process_group);
        }
        command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(&launch.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| protocol_error(&runtime, format!("spawn failed: {err}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stderr"))?;
        let stderr = tokio::spawn(read_bounded_stderr(stderr, CURSOR_SDK_STDERR_MAX_BYTES));

        Ok(Self {
            runtime,
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stderr: Some(stderr),
            output_text: String::new(),
            summary: None,
            artifacts: Vec::new(),
            external_session_id: None,
            expect_builtin_tools_disabled,
            idle_timeout: CURSOR_SDK_IDLE_TIMEOUT,
        })
    }

    pub(crate) async fn run<H>(
        &mut self,
        request: ExternalAgentRequest,
        host: &H,
        config: JsonValue,
    ) -> Result<ExternalAgentResult, ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        self.write_json(config).await?;

        let mut session = ExternalAgentSessionState {
            runtime: request.runtime.clone(),
            external_session_id: None,
            mode: request.mode,
            cwd: request.cwd.clone(),
        };
        host.emit(ExternalAgentEvent::RunStarted {
            session: session.clone(),
        })
        .await?;

        let mut completed = false;
        let mut last_activity = Instant::now();
        loop {
            if host.is_cancelled().await {
                let _ = self.write_json(json!({ "type": "cancel" })).await;
                self.shutdown().await;
                return Err(ExternalAgentError::Cancelled);
            }

            let line = match tokio::time::timeout(
                CURSOR_SDK_CANCEL_POLL_INTERVAL,
                self.stdout.next_line(),
            )
            .await
            {
                Ok(line) => line
                    .map_err(|err| protocol_error(&self.runtime, format!("read failed: {err}")))?,
                Err(_) => {
                    if last_activity.elapsed() >= self.idle_timeout {
                        self.shutdown().await;
                        return Err(self
                            .protocol_error_with_stderr("timed out waiting for Cursor SDK output")
                            .await);
                    }
                    continue;
                }
            };
            let Some(line) = line else {
                break;
            };
            last_activity = Instant::now();
            if line.trim().is_empty() {
                continue;
            }
            if self
                .handle_line(&line, &mut session, host, &mut completed)
                .await?
            {
                break;
            }
        }

        let status = self.wait_for_exit_bounded().await?;
        if !completed {
            match status {
                Some(status) if !status.success() => {
                    return Err(ExternalAgentError::Runtime {
                        runtime: self.runtime.as_str().to_string(),
                        message: self.take_stderr_or_default().await,
                    });
                }
                _ => {}
            }
        }

        let result = ExternalAgentResult {
            status: ExternalAgentRunStatus::Completed,
            session,
            summary: self.summary.clone().or_else(|| {
                let trimmed = self.output_text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }),
            artifacts: self.artifacts.clone(),
        };
        host.emit(ExternalAgentEvent::Completed {
            result: result.clone(),
        })
        .await?;
        Ok(result)
    }

    /// Handle one protocol line. Returns `Ok(true)` when the run is complete.
    async fn handle_line<H>(
        &mut self,
        line: &str,
        session: &mut ExternalAgentSessionState,
        host: &H,
        completed: &mut bool,
    ) -> Result<bool, ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let message = serde_json::from_str::<JsonValue>(line)
            .map_err(|err| protocol_error(&self.runtime, format!("invalid sidecar line: {err}")))?;
        let kind = message
            .get("type")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| protocol_error(&self.runtime, "sidecar message missing type"))?;

        match kind {
            "ready" => {
                if self.expect_builtin_tools_disabled
                    && message.get("builtinToolsDisabled").and_then(JsonValue::as_bool)
                        != Some(true)
                {
                    return Err(protocol_error(
                        &self.runtime,
                        "Cursor SDK sidecar did not disable built-in mutating tools",
                    ));
                }
                Ok(false)
            }
            "session" => {
                if let Some(agent_id) = message.get("agentId").and_then(JsonValue::as_str)
                    && self.external_session_id.as_deref() != Some(agent_id)
                {
                    self.external_session_id = Some(agent_id.to_string());
                    session.external_session_id = Some(agent_id.to_string());
                    host.emit(ExternalAgentEvent::SessionResolved {
                        session: session.clone(),
                    })
                    .await?;
                }
                Ok(false)
            }
            "output-delta" => {
                if let Some(text) = message.get("text").and_then(JsonValue::as_str) {
                    self.output_text.push_str(text);
                    host.emit(ExternalAgentEvent::OutputTextDelta {
                        text: text.to_string(),
                    })
                    .await?;
                }
                Ok(false)
            }
            "reasoning-delta" => {
                if let Some(text) = message.get("text").and_then(JsonValue::as_str) {
                    host.emit(ExternalAgentEvent::ReasoningDelta {
                        text: text.to_string(),
                    })
                    .await?;
                }
                Ok(false)
            }
            "status" => {
                if let Some(text) = message.get("message").and_then(JsonValue::as_str) {
                    host.emit(ExternalAgentEvent::Status {
                        message: text.to_string(),
                    })
                    .await?;
                }
                Ok(false)
            }
            "artifact" => {
                if let Some(value) = message.get("artifact")
                    && let Ok(artifact) =
                        serde_json::from_value::<ExternalAgentArtifact>(value.clone())
                {
                    self.artifacts.push(artifact.clone());
                    host.emit(ExternalAgentEvent::Artifact { artifact }).await?;
                }
                Ok(false)
            }
            "tool-request" => {
                self.bridge_tool_request(&message, host).await?;
                Ok(false)
            }
            "completed" => {
                if let Some(summary) = message
                    .get("summary")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|summary| !summary.is_empty())
                {
                    self.summary = Some(summary.to_string());
                }
                if let Some(values) = message.get("artifacts").and_then(JsonValue::as_array) {
                    for value in values {
                        if let Ok(artifact) =
                            serde_json::from_value::<ExternalAgentArtifact>(value.clone())
                            && !self.artifacts.contains(&artifact)
                        {
                            self.artifacts.push(artifact);
                        }
                    }
                }
                *completed = true;
                Ok(true)
            }
            "failed" => {
                let message = message
                    .get("message")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("Cursor SDK sidecar reported failure")
                    .to_string();
                Err(ExternalAgentError::Runtime {
                    runtime: self.runtime.as_str().to_string(),
                    message,
                })
            }
            _ => Ok(false),
        }
    }

    async fn bridge_tool_request<H>(
        &mut self,
        message: &JsonValue,
        host: &H,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let id = message
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("tool")
            .to_string();
        let Some(action_value) = message.get("action") else {
            return self
                .write_tool_response(
                    &id,
                    &ExternalAgentActionResult::Rejected {
                        reason: "sidecar tool request missing action".to_string(),
                    },
                )
                .await;
        };
        let action = match serde_json::from_value::<ExternalAgentActionRequest>(action_value.clone())
        {
            Ok(action) => action,
            Err(err) => {
                return self
                    .write_tool_response(
                        &id,
                        &ExternalAgentActionResult::Rejected {
                            reason: format!("invalid tool action: {err}"),
                        },
                    )
                    .await;
            }
        };

        // Record the proposal, then let the Comp2 host guard/executor decide and
        // execute. This process never runs the action itself.
        host.emit(ExternalAgentEvent::ProposedAction {
            proposal: action.clone(),
        })
        .await?;
        let permission = ExternalAgentPermissionRequest {
            id: id.clone(),
            action: action.clone(),
            options: vec![
                ExternalAgentPermissionOption::AllowOnce,
                ExternalAgentPermissionOption::RejectOnce,
            ],
        };
        host.emit(ExternalAgentEvent::PermissionRequested {
            request: permission.clone(),
        })
        .await?;

        let decision = host.request_permission(permission).await?;
        if decision != ExternalAgentPermissionDecision::AllowOnce {
            return self
                .write_tool_response(
                    &id,
                    &ExternalAgentActionResult::Rejected {
                        reason: "tool call rejected by host".to_string(),
                    },
                )
                .await;
        }

        let result = host.perform_action(action).await?;
        self.write_tool_response(&id, &result).await
    }

    async fn write_tool_response(
        &mut self,
        id: &str,
        result: &ExternalAgentActionResult,
    ) -> Result<(), ExternalAgentError> {
        let result = serde_json::to_value(result)
            .map_err(|err| protocol_error(&self.runtime, format!("encode result failed: {err}")))?;
        self.write_json(json!({
            "type": "tool-response",
            "id": id,
            "result": result,
        }))
        .await
    }

    async fn write_json(&mut self, value: JsonValue) -> Result<(), ExternalAgentError> {
        let mut line = serde_json::to_vec(&value)
            .map_err(|err| protocol_error(&self.runtime, format!("encode failed: {err}")))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|err| protocol_error(&self.runtime, format!("write failed: {err}")))
    }

    async fn wait_for_exit_bounded(
        &mut self,
    ) -> Result<Option<std::process::ExitStatus>, ExternalAgentError> {
        match tokio::time::timeout(CURSOR_SDK_SHUTDOWN_GRACE, self.child.wait()).await {
            Ok(status) => status
                .map(Some)
                .map_err(|err| protocol_error(&self.runtime, format!("wait failed: {err}"))),
            Err(_) => {
                self.shutdown().await;
                Ok(None)
            }
        }
    }

    async fn shutdown(&mut self) {
        let _ = codex_utils_pty::process_group::kill_child_process_group(&mut self.child);
        let _ = self.child.kill().await;
    }

    async fn protocol_error_with_stderr(&mut self, message: &str) -> ExternalAgentError {
        let runtime = self.runtime.clone();
        let stderr = self.take_stderr().await;
        protocol_error(&runtime, append_stderr(message, stderr.as_deref()))
    }

    async fn take_stderr(&mut self) -> Option<String> {
        let stderr = self.stderr.take()?;
        match stderr.await {
            Ok(stderr) if !stderr.trim().is_empty() => Some(stderr.trim().to_string()),
            Ok(_) => None,
            Err(err) => Some(format!("stderr reader failed: {err}")),
        }
    }

    async fn take_stderr_or_default(&mut self) -> String {
        self.take_stderr()
            .await
            .unwrap_or_else(|| "Cursor SDK sidecar exited unsuccessfully".to_string())
    }
}

async fn read_bounded_stderr<R>(mut reader: R, max_bytes: usize) -> String
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buf = [0_u8; 4096];
    let mut truncated = false;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = max_bytes.saturating_sub(output.len());
                if remaining > 0 {
                    output.extend_from_slice(&buf[..n.min(remaining)]);
                }
                truncated |= n > remaining;
            }
            Err(err) => return format!("failed to read stderr: {err}"),
        }
    }
    let mut text = String::from_utf8_lossy(&output).into_owned();
    if truncated {
        text.push_str("\n[stderr truncated]");
    }
    text
}

fn append_stderr(message: &str, stderr: Option<&str>) -> String {
    match stderr.map(str::trim).filter(|stderr| !stderr.is_empty()) {
        Some(stderr) => format!("{message}; stderr: {stderr}"),
        None => message.to_string(),
    }
}

fn protocol_error(
    runtime: &ExternalAgentRuntimeId,
    message: impl Into<String>,
) -> ExternalAgentError {
    ExternalAgentError::Protocol {
        runtime: runtime.as_str().to_string(),
        message: message.into(),
    }
}

fn runtime_error(
    runtime: &ExternalAgentRuntimeId,
    message: impl Into<String>,
) -> ExternalAgentError {
    ExternalAgentError::Runtime {
        runtime: runtime.as_str().to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExternalAgentMode;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct RecordingHost {
        events: Arc<Mutex<Vec<ExternalAgentEvent>>>,
        actions: Arc<Mutex<Vec<ExternalAgentActionRequest>>>,
        allow: bool,
        cancel_after: Option<usize>,
        polls: Arc<Mutex<usize>>,
    }

    impl RecordingHost {
        fn allowing() -> Self {
            Self {
                allow: true,
                ..Default::default()
            }
        }

        fn denying() -> Self {
            Self {
                allow: false,
                ..Default::default()
            }
        }

        fn cancelling_after(polls: usize) -> Self {
            Self {
                allow: true,
                cancel_after: Some(polls),
                ..Default::default()
            }
        }

        fn events(&self) -> Vec<ExternalAgentEvent> {
            self.events.lock().expect("events lock").clone()
        }

        fn actions(&self) -> Vec<ExternalAgentActionRequest> {
            self.actions.lock().expect("actions lock").clone()
        }

        fn output(&self) -> Vec<String> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    ExternalAgentEvent::OutputTextDelta { text } => Some(text),
                    _ => None,
                })
                .collect()
        }
    }

    impl ExternalAgentHost for RecordingHost {
        async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
            self.events.lock().expect("events lock").push(event);
            Ok(())
        }

        async fn request_permission(
            &self,
            _request: ExternalAgentPermissionRequest,
        ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
            Ok(if self.allow {
                ExternalAgentPermissionDecision::AllowOnce
            } else {
                ExternalAgentPermissionDecision::RejectOnce
            })
        }

        async fn perform_action(
            &self,
            action: ExternalAgentActionRequest,
        ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
            self.actions.lock().expect("actions lock").push(action);
            Ok(ExternalAgentActionResult::FileContent {
                content: "host file contents".to_string(),
            })
        }

        async fn is_cancelled(&self) -> bool {
            let Some(cancel_after) = self.cancel_after else {
                return false;
            };
            let mut polls = self.polls.lock().expect("polls lock");
            *polls += 1;
            *polls > cancel_after
        }
    }

    fn python() -> Option<PathBuf> {
        which::which("python3").ok()
    }

    fn fake_launch(script: &Path, cwd: &Path) -> ExternalAgentSandboxedLaunchSpec {
        let python = python().expect("python3 for fake sidecar");
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::CURSOR.into(),
            program: python,
            args: vec![script.display().to_string()],
            arg0: None,
            cwd: cwd.to_path_buf(),
            env: BTreeMap::from([("PATH".to_string(), path)]),
            isolation: ExternalAgentLaunchIsolation::test_only_unenforced(),
        };
        ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch)
    }

    #[test]
    fn custom_tools_track_capabilities() {
        assert_eq!(
            cursor_sdk_custom_tools(&ExternalAgentCapabilities::for_mode(ExternalAgentMode::Plan)),
            Vec::<String>::new()
        );
        assert_eq!(
            cursor_sdk_custom_tools(&ExternalAgentCapabilities::for_mode(
                ExternalAgentMode::Propose
            )),
            vec!["read_file".to_string(), "mcp_tool".to_string(), "network".to_string()]
        );
        assert_eq!(
            cursor_sdk_custom_tools(&ExternalAgentCapabilities::for_mode(
                ExternalAgentMode::Managed
            )),
            vec![
                "read_file".to_string(),
                "write_file".to_string(),
                "run_command".to_string(),
                "mcp_tool".to_string(),
                "network".to_string(),
            ]
        );
    }

    #[test]
    fn local_config_disables_builtins_and_selects_model() {
        let backend = CursorSdkBackend::new();
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "inspect repo",
            "/repo",
            ExternalAgentMode::Propose,
        )
        .with_model("composer-2");
        let config = backend.local_config(&request);

        assert_eq!(config["runtime"], "local");
        assert_eq!(config["model"], "composer-2");
        assert_eq!(
            config["disabledBuiltinTools"],
            serde_json::json!(["shell", "write", "edit"])
        );
        assert_eq!(config["customTools"], serde_json::json!(["read_file", "mcp_tool", "network"]));
        assert_eq!(config["session"], serde_json::json!({ "type": "new" }));
    }

    #[test]
    fn local_config_defaults_and_resumes() {
        let backend = CursorSdkBackend::new();
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "inspect repo",
            "/repo",
            ExternalAgentMode::Plan,
        )
        .with_resumed_session("agent-9");
        let config = backend.local_config(&request);
        assert_eq!(config["model"], "composer-2.5");
        assert_eq!(
            config["session"],
            serde_json::json!({ "type": "resume", "externalSessionId": "agent-9" })
        );
    }

    #[test]
    fn validate_request_rejects_wrong_runtime() {
        let descriptor = cursor_composer_runtime_descriptor();
        let request =
            ExternalAgentRequest::new("grok-build", "x", "/repo", ExternalAgentMode::Plan);
        let err = validate_cursor_request(descriptor, &request).expect_err("runtime mismatch");
        assert!(matches!(err, ExternalAgentError::Runtime { .. }));
    }

    #[tokio::test]
    async fn readiness_reports_missing_runtime_without_node() {
        let descriptor = cursor_composer_runtime_descriptor();
        let policy = CursorSdkEnvironmentPolicy::default();
        let env = BTreeMap::from([("PATH".to_string(), "/nonexistent-bin".to_string())]);
        let readiness = cursor_sidecar_readiness_with_env(descriptor, &policy, &env).await;
        assert_eq!(readiness.status, ExternalAgentReadinessStatus::MissingRuntime);
        assert_eq!(readiness.runtime, ExternalAgentRuntimeId::from("cursor"));
    }

    #[tokio::test]
    async fn sidecar_streams_output_session_and_bridges_a_read() {
        let Some(_python) = python() else {
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("fake_sidecar.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

config = json.loads(sys.stdin.readline())
send({"type": "ready", "runtime": config["runtime"], "builtinToolsDisabled": True, "customTools": config["customTools"]})
send({"type": "session", "agentId": "agent-123"})
send({"type": "reasoning-delta", "text": "thinking"})
send({"type": "output-delta", "text": "hello "})
send({"type": "tool-request", "id": "t1", "action": {"type": "read-file", "path": "README.md"}})
resp = json.loads(sys.stdin.readline())
assert resp["type"] == "tool-response"
send({"type": "output-delta", "text": resp["result"]["content"]})
send({"type": "completed", "summary": "done", "artifacts": []})
"#,
        )
        .expect("write fake sidecar");

        let launch = fake_launch(&script, temp.path());
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "inspect README",
            temp.path(),
            ExternalAgentMode::Managed,
        );
        let config = CursorSdkBackend::new().local_config(&request);
        let host = RecordingHost::allowing();
        let mut process = CursorSdkProcess::spawn(launch, true).expect("spawn");
        let result = process
            .run(request, &host, config)
            .await
            .expect("run completes");

        assert_eq!(result.status, ExternalAgentRunStatus::Completed);
        assert_eq!(result.summary.as_deref(), Some("done"));
        assert_eq!(
            result.session.external_session_id.as_deref(),
            Some("agent-123")
        );
        assert_eq!(
            host.actions(),
            vec![ExternalAgentActionRequest::ReadFile {
                path: PathBuf::from("README.md"),
            }]
        );
        assert_eq!(
            host.output(),
            vec!["hello ".to_string(), "host file contents".to_string()]
        );
        // SessionResolved carries the persisted agent id.
        assert!(host.events().iter().any(|event| matches!(
            event,
            ExternalAgentEvent::SessionResolved { session }
                if session.external_session_id.as_deref() == Some("agent-123")
        )));
    }

    #[tokio::test]
    async fn sidecar_rejects_tool_when_host_denies() {
        let Some(_python) = python() else {
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("fake_deny.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

config = json.loads(sys.stdin.readline())
send({"type": "ready", "runtime": config["runtime"], "builtinToolsDisabled": True, "customTools": config["customTools"]})
send({"type": "tool-request", "id": "t1", "action": {"type": "write-file", "path": "x.txt", "content": "hi"}})
resp = json.loads(sys.stdin.readline())
send({"type": "output-delta", "text": resp["result"]["type"]})
send({"type": "completed", "summary": "", "artifacts": []})
"#,
        )
        .expect("write fake sidecar");

        let launch = fake_launch(&script, temp.path());
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "edit",
            temp.path(),
            ExternalAgentMode::Managed,
        );
        let config = CursorSdkBackend::new().local_config(&request);
        let host = RecordingHost::denying();
        let mut process = CursorSdkProcess::spawn(launch, true).expect("spawn");
        process.run(request, &host, config).await.expect("run");

        // Host never executed the action because permission was rejected.
        assert!(host.actions().is_empty());
        assert_eq!(host.output(), vec!["rejected".to_string()]);
    }

    #[tokio::test]
    async fn sidecar_rejects_enabled_builtin_tools() {
        let Some(_python) = python() else {
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("fake_unsafe.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

config = json.loads(sys.stdin.readline())
send({"type": "ready", "runtime": config["runtime"], "builtinToolsDisabled": False, "customTools": []})
send({"type": "completed", "summary": "", "artifacts": []})
"#,
        )
        .expect("write fake sidecar");

        let launch = fake_launch(&script, temp.path());
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "edit",
            temp.path(),
            ExternalAgentMode::Managed,
        );
        let config = CursorSdkBackend::new().local_config(&request);
        let host = RecordingHost::allowing();
        let mut process = CursorSdkProcess::spawn(launch, true).expect("spawn");
        let err = process
            .run(request, &host, config)
            .await
            .expect_err("unsafe builtin surface must be rejected");
        assert!(matches!(err, ExternalAgentError::Protocol { .. }));
    }

    #[tokio::test]
    async fn sidecar_honors_host_cancellation() {
        let Some(_python) = python() else {
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("fake_hang.py");
        std::fs::write(
            &script,
            r#"
import json, sys, time

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

config = json.loads(sys.stdin.readline())
send({"type": "ready", "runtime": config["runtime"], "builtinToolsDisabled": True, "customTools": []})
send({"type": "output-delta", "text": "start"})
time.sleep(30)
"#,
        )
        .expect("write fake sidecar");

        let launch = fake_launch(&script, temp.path());
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "long task",
            temp.path(),
            ExternalAgentMode::Plan,
        );
        let config = CursorSdkBackend::new().local_config(&request);
        let host = RecordingHost::cancelling_after(1);
        let mut process = CursorSdkProcess::spawn(launch, true).expect("spawn");
        let err = process
            .run(request, &host, config)
            .await
            .expect_err("run should be cancelled");
        assert!(matches!(err, ExternalAgentError::Cancelled));
    }
}
