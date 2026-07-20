//! SPIKE: Claude Code external-agent adapter driven over ACP.
//!
//! This module is a **proof-of-concept slice**, not a finished adapter. It
//! exists to answer one question from the task: can Codewith enforce approvals
//! for Claude Code through host callbacks, or must Claude be treated as a
//! high-trust delegate?
//!
//! # Two Claude paths, deliberately distinct
//!
//! * [`crate::ClaudeCodeHarness`] (module `claude`) drives the `claude` CLI in
//!   print mode (`claude -p --output-format stream-json`). Print mode has **no
//!   permission callback channel**: safety comes from constraining the run to
//!   `--permission-mode plan` with a read-only tool allowlist and no MCP. That
//!   is *delegate* enforcement (deny-by-construction), not *callback*
//!   enforcement. It is fine for Consult/Plan, but cannot mediate writes,
//!   terminals, or MCP at runtime.
//! * This spike drives Claude Code over the **Agent Client Protocol (ACP)**,
//!   reusing the shipped [`crate::AcpStdioHarness`] JSON-RPC stdio driver that
//!   already mediates Cursor and Grok Build. Over ACP the agent calls back into
//!   the host for `session/request_permission`, `fs/read_text_file`,
//!   `fs/write_text_file`, and `terminal/create`. Those callbacks are routed
//!   through [`crate::ExternalAgentHost`] exactly like every other ACP runtime,
//!   so Codewith keeps approval, path-confinement, and audit ownership.
//!
//! # Finding
//!
//! Claude Code does **not** speak ACP natively. A Node bridge is required
//! (`@zed-industries/claude-agent-acp`, the successor to `claude-code-acp`,
//! Apache-2.0), which wraps the Claude Agent SDK and exposes an ACP stdio
//! server. With that bridge in front of it, Claude plugs into the existing ACP
//! callback path **unchanged** — the same driver and the same host-mediated
//! permission flow proven by
//! `acp::tests::acp_process_handles_session_updates_and_host_file_reads`.
//!
//! So the answer is: **callback-based approval enforcement is achievable for
//! Claude via the ACP bridge**, and it does not require reimplementing the ACP
//! driver. The remaining work is wiring, auth, and bridge provisioning — not a
//! new protocol.
//!
//! # What this spike wires up
//!
//! * A hidden `claude-acp` runtime descriptor whose command launches the bridge
//!   (`npx -y @zed-industries/claude-agent-acp` by default).
//! * Anthropic auth passthrough through the ACP sanitized-env seam
//!   ([`CLAUDE_ACP_AUTH_ENV_VARS`], consumed by `acp::acp_runtime_auth_env_vars`).
//! * A Claude-aware readiness check (bridge present + Claude auth present).
//! * A best-effort ACP auth-method selection for the bridge
//!   (`acp::acp_auth_method`).
//!
//! # Known gaps / remaining work (why this is a spike, not a product)
//!
//! 1. **`HOME` isolation vs. subscription login.** [`crate::AcpProcessIsolation`]
//!    repoints `HOME`/XDG dirs at a per-run temp dir, so a bridge that reads
//!    `~/.claude/.credentials.json` (Pro/Max OAuth login) sees an empty home and
//!    fails. This spike keeps `HOME` isolated but passes `CLAUDE_CONFIG_DIR`
//!    through so an operator can point the bridge at the real credential dir.
//!    A production adapter should decide policy: env/API-key auth only for
//!    managed runs, or an explicit read-only credential mount.
//! 2. **Bridge provisioning.** `npx` fetches at runtime (network + supply-chain
//!    surface). Production should vendor/pin the bridge and resolve a fixed
//!    binary instead of `npx`.
//! 3. **Dynamic bridge command.** The live harness run path uses the pinned
//!    static descriptor command. [`ClaudeAcpBridge::resolve_from_env`] shows the
//!    override shape (`CODEWITH_CLAUDE_ACP_BRIDGE`) but is not yet threaded into
//!    the launch (the descriptor command is `&'static`).
//! 4. **Auth-method contract.** The bridge's ACP `authMethods` ids are not
//!    frozen; `acp::acp_auth_method` falls back to the first advertised method
//!    for `claude-acp` instead of failing closed. Revisit before promotion.
//! 5. **Cloud providers.** Only the direct Anthropic/self-hosted-gateway auth
//!    vars are passed through here. The Bedrock/Vertex/Foundry matrix in
//!    `claude::add_agent_sdk_auth_env` should be reused for a full adapter.
//! 6. **Not wired into the app-server.** `claude-acp` is intentionally hidden
//!    (`visible: false`) and not registered in `BUILTIN_EXTERNAL_AGENT_RUNTIMES`
//!    or the app-server runner, so this spike has zero blast radius on shipped
//!    Cursor/Grok/Claude behavior.
//!
//! # Recommendation
//!
//! Adopt the ACP-bridge path as the route to *managed* Claude execution, layered
//! on top of (not replacing) the existing print-mode delegate for Consult/Plan.
//! Promotion checklist: pin/vendor the bridge, settle the `HOME`/credential
//! policy, freeze the auth-method mapping, add an end-to-end permission test
//! against the real bridge, then register a visible runtime + app-server runner.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use crate::AcpStdioHarness;
use crate::ExternalAgentCommandSpec;
use crate::ExternalAgentError;
use crate::ExternalAgentHarness;
use crate::ExternalAgentHarnessKind;
use crate::ExternalAgentHost;
use crate::ExternalAgentMode;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentReadinessStatus;
use crate::ExternalAgentRequest;
use crate::ExternalAgentResult;
use crate::ExternalAgentRuntime;
use crate::ExternalAgentRuntimeDescriptor;
use crate::ExternalAgentRuntimeId;
use crate::ExternalAgentSandboxConfig;
use crate::has_agent_sdk_auth_env;

/// Env var an operator can set to override the ACP bridge command line
/// (whitespace-separated `program arg…`). SPIKE-only; see module docs, gap #3.
pub const CLAUDE_ACP_BRIDGE_OVERRIDE_ENV: &str = "CODEWITH_CLAUDE_ACP_BRIDGE";

/// Default program used to launch the Claude ACP bridge.
pub const DEFAULT_CLAUDE_ACP_BRIDGE_PROGRAM: &str = "npx";

/// Default arguments used to launch the Claude ACP bridge.
///
/// `@zed-industries/claude-agent-acp` is the maintained ACP adapter for the
/// Claude Agent SDK (Apache-2.0).
pub const DEFAULT_CLAUDE_ACP_BRIDGE_ARGS: &[&str] = &["-y", "@zed-industries/claude-agent-acp"];

/// Anthropic auth environment forwarded to the ACP bridge subprocess.
///
/// This list is consumed by `acp::acp_runtime_auth_env_vars` so the bridge
/// receives Claude credentials through the same sanitized-env seam Cursor and
/// Grok use. `CLAUDE_CONFIG_DIR` is included so subscription credentials remain
/// reachable even though [`crate::AcpProcessIsolation`] isolates `HOME`
/// (module docs, gap #1). The Bedrock/Vertex/Foundry matrix is intentionally
/// omitted from this spike (gap #5).
pub(crate) const CLAUDE_ACP_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "CLAUDE_CONFIG_DIR",
];

const CLAUDE_ACP_SUPPORTED_MODES: &[ExternalAgentMode] =
    &[ExternalAgentMode::Plan, ExternalAgentMode::Propose];

/// Hidden descriptor for the Claude ACP bridge runtime.
///
/// It is deliberately kept out of `BUILTIN_EXTERNAL_AGENT_RUNTIMES` (and thus
/// invisible to the UI and app-server) while this remains a spike.
static CLAUDE_ACP_DESCRIPTOR: ExternalAgentRuntimeDescriptor = ExternalAgentRuntimeDescriptor {
    id: ExternalAgentRuntimeId::CLAUDE_ACP,
    display_name: "Claude Code (ACP spike)",
    description: "SPIKE: run Claude Code over ACP via the @zed-industries/claude-agent-acp bridge.",
    command: ExternalAgentCommandSpec {
        program: DEFAULT_CLAUDE_ACP_BRIDGE_PROGRAM,
        args: DEFAULT_CLAUDE_ACP_BRIDGE_ARGS,
    },
    supported_modes: CLAUDE_ACP_SUPPORTED_MODES,
    default_mode: ExternalAgentMode::Plan,
    visible: false,
};

/// Resolved ACP bridge command (program + args).
///
/// Represents the production resolution shape (default vs. operator override).
/// The live harness run path currently uses the pinned static descriptor
/// command; see module docs, gap #3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeAcpBridge {
    pub program: String,
    pub args: Vec<String>,
}

impl Default for ClaudeAcpBridge {
    fn default() -> Self {
        Self {
            program: DEFAULT_CLAUDE_ACP_BRIDGE_PROGRAM.to_string(),
            args: DEFAULT_CLAUDE_ACP_BRIDGE_ARGS
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        }
    }
}

impl ClaudeAcpBridge {
    /// Resolve the bridge command, honoring the `CODEWITH_CLAUDE_ACP_BRIDGE`
    /// override when it is present and non-empty.
    ///
    /// SPIKE simplification: the override is split on ASCII whitespace and does
    /// not honor shell quoting.
    pub fn resolve_from_env(source_env: &BTreeMap<String, String>) -> Self {
        let Some(raw) = source_env
            .get(CLAUDE_ACP_BRIDGE_OVERRIDE_ENV)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            return Self::default();
        };
        let mut parts = raw.split_whitespace().map(std::string::ToString::to_string);
        match parts.next() {
            Some(program) => Self {
                program,
                args: parts.collect(),
            },
            None => Self::default(),
        }
    }

    /// Copy the Anthropic auth vars that are set (and non-empty) in `source_env`.
    ///
    /// This mirrors what `acp::acp_runtime_auth_env_vars` forwards for the
    /// `claude-acp` runtime, exposed here so callers/tests can reason about the
    /// passthrough directly.
    pub fn auth_env(source_env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        for name in CLAUDE_ACP_AUTH_ENV_VARS {
            if let Some(value) = source_env.get(*name)
                && !value.trim().is_empty()
            {
                env.insert((*name).to_string(), value.clone());
            }
        }
        env
    }
}

/// True when `source_env` carries Claude Agent SDK auth (API key, token, or a
/// cloud-provider flag). Reuses the same detection as the print-mode adapter.
pub fn claude_acp_is_authenticated(source_env: &BTreeMap<String, String>) -> bool {
    has_agent_sdk_auth_env(source_env)
}

/// SPIKE harness that drives Claude Code over ACP via the Node bridge.
///
/// It composes the shipped [`AcpStdioHarness`] (all JSON-RPC, permission, and
/// sandbox mechanics are reused unchanged) and only adds Claude-specific
/// readiness on top.
pub struct ClaudeAcpHarness {
    inner: AcpStdioHarness,
}

impl ClaudeAcpHarness {
    pub fn new() -> Self {
        Self {
            inner: AcpStdioHarness::new(&CLAUDE_ACP_DESCRIPTOR),
        }
    }

    pub fn harness_kind(&self) -> ExternalAgentHarnessKind {
        self.inner.harness_kind()
    }

    pub fn descriptor(&self) -> &'static ExternalAgentRuntimeDescriptor {
        self.inner.descriptor()
    }

    pub fn runtime_id(&self) -> ExternalAgentRuntimeId {
        self.inner.id()
    }

    /// Claude-aware readiness: the bridge program must resolve on `PATH` and
    /// Claude auth must be present. Unlike the generic ACP readiness, this
    /// reports `MissingAuth` when the bridge exists but no credentials do.
    pub async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match self.resolve_bridge_program(source_env, &cwd) {
            Ok(program) => {
                if claude_acp_is_authenticated(source_env)
                    || source_env
                        .get("CLAUDE_CONFIG_DIR")
                        .is_some_and(|value| !value.trim().is_empty())
                {
                    self.readiness(
                        ExternalAgentReadinessStatus::Ready,
                        Some(program.display().to_string()),
                    )
                } else {
                    self.readiness(
                        ExternalAgentReadinessStatus::MissingAuth,
                        Some(
                            "Claude ACP bridge resolved but no Claude auth was found (set ANTHROPIC_API_KEY/ANTHROPIC_AUTH_TOKEN or CLAUDE_CONFIG_DIR)"
                                .to_string(),
                        ),
                    )
                }
            }
            Err(detail) => {
                self.readiness(ExternalAgentReadinessStatus::MissingRuntime, Some(detail))
            }
        }
    }

    /// Run Claude over ACP inside Codewith's platform sandbox, delegating to the
    /// shipped ACP driver (which mediates every permission/file/terminal
    /// callback through `host`).
    pub async fn run_sandboxed_with_env(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
        source_env: BTreeMap<String, String>,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        self.inner
            .run_sandboxed_with_env(request, host, sandbox_config, source_env)
            .await
    }

    fn resolve_bridge_program(
        &self,
        source_env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<PathBuf, String> {
        let program = self.descriptor().command.program;
        let path = source_env.get("PATH").map(String::as_str);
        which::which_in(program, path, cwd)
            .map_err(|err| format!("could not resolve Claude ACP bridge `{program}`: {err}"))
    }

    fn readiness(
        &self,
        status: ExternalAgentReadinessStatus,
        detail: Option<String>,
    ) -> ExternalAgentReadiness {
        let descriptor = self.descriptor();
        ExternalAgentReadiness {
            runtime: self.runtime_id(),
            status,
            display_name: descriptor.display_name.to_string(),
            version: None,
            supported_modes: descriptor.supported_modes.to_vec(),
            detail,
        }
    }
}

impl Default for ClaudeAcpHarness {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct the SPIKE Claude ACP harness.
pub fn claude_acp_harness() -> ClaudeAcpHarness {
    ClaudeAcpHarness::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn harness_exposes_acp_stdio_kind_and_hidden_bridge_descriptor() {
        let harness = claude_acp_harness();
        let descriptor = harness.descriptor();

        assert_eq!(harness.harness_kind(), ExternalAgentHarnessKind::AcpStdio);
        assert_eq!(harness.runtime_id().as_str(), "claude-acp");
        assert_eq!(descriptor.command.program, "npx");
        assert_eq!(
            descriptor.command.args,
            ["-y", "@zed-industries/claude-agent-acp"]
        );
        assert!(
            !descriptor.visible,
            "the claude-acp spike runtime must stay hidden"
        );
        assert_eq!(
            descriptor.supported_modes,
            [ExternalAgentMode::Plan, ExternalAgentMode::Propose]
        );
    }

    #[test]
    fn bridge_defaults_to_pinned_zed_adapter() {
        let bridge = ClaudeAcpBridge::default();

        assert_eq!(bridge.program, "npx");
        assert_eq!(
            bridge.args,
            vec![
                "-y".to_string(),
                "@zed-industries/claude-agent-acp".to_string()
            ]
        );
    }

    #[test]
    fn bridge_resolution_honors_override_env() {
        let source_env = BTreeMap::from([(
            CLAUDE_ACP_BRIDGE_OVERRIDE_ENV.to_string(),
            "  /opt/claude-acp/bin/bridge --stdio  ".to_string(),
        )]);

        let bridge = ClaudeAcpBridge::resolve_from_env(&source_env);

        assert_eq!(bridge.program, "/opt/claude-acp/bin/bridge");
        assert_eq!(bridge.args, vec!["--stdio".to_string()]);
    }

    #[test]
    fn bridge_resolution_falls_back_to_default_when_override_blank() {
        let source_env = BTreeMap::from([(
            CLAUDE_ACP_BRIDGE_OVERRIDE_ENV.to_string(),
            "   ".to_string(),
        )]);

        assert_eq!(
            ClaudeAcpBridge::resolve_from_env(&source_env),
            ClaudeAcpBridge::default()
        );
    }

    #[test]
    fn auth_env_forwards_anthropic_vars_and_drops_unrelated_secrets() {
        let source_env = BTreeMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://gateway.example.invalid".to_string(),
            ),
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                "/home/alex/.claude".to_string(),
            ),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), "  ".to_string()),
            ("OPENAI_API_KEY".to_string(), "test-value".to_string()),
            ("CURSOR_API_KEY".to_string(), "test-value".to_string()),
            ("XAI_API_KEY".to_string(), "test-value".to_string()),
            ("HOME".to_string(), "/home/alex".to_string()),
        ]);

        let env = ClaudeAcpBridge::auth_env(&source_env);

        assert_eq!(
            env,
            BTreeMap::from([
                ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "https://gateway.example.invalid".to_string()
                ),
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    "/home/alex/.claude".to_string()
                ),
            ]),
            "only non-empty Anthropic auth vars should be forwarded; blank and unrelated secrets are dropped"
        );
    }

    #[test]
    fn is_authenticated_tracks_claude_credentials() {
        assert!(!claude_acp_is_authenticated(&BTreeMap::new()));
        assert!(claude_acp_is_authenticated(&BTreeMap::from([(
            "ANTHROPIC_API_KEY".to_string(),
            "test-value".to_string(),
        )])));
        assert!(claude_acp_is_authenticated(&BTreeMap::from([(
            "CLAUDE_CODE_USE_BEDROCK".to_string(),
            "1".to_string(),
        )])));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_reports_missing_runtime_without_bridge() {
        let harness = claude_acp_harness();
        let source_env = BTreeMap::from([("PATH".to_string(), "/nonexistent-bin".to_string())]);

        let readiness = harness.readiness_with_env(&source_env).await;

        assert_eq!(
            readiness.status,
            ExternalAgentReadinessStatus::MissingRuntime
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_reports_missing_auth_when_bridge_present_without_creds() {
        let (_temp_dir, env) = fake_bridge_env();
        let harness = claude_acp_harness();

        let readiness = harness.readiness_with_env(&env).await;

        assert_eq!(readiness.status, ExternalAgentReadinessStatus::MissingAuth);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_reports_ready_with_bridge_and_api_key() {
        let (_temp_dir, mut env) = fake_bridge_env();
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-value".to_string());
        let harness = claude_acp_harness();

        let readiness = harness.readiness_with_env(&env).await;

        assert_eq!(readiness.status, ExternalAgentReadinessStatus::Ready);
    }

    #[cfg(unix)]
    fn fake_bridge_env() -> (tempfile::TempDir, BTreeMap<String, String>) {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let bridge = bin_dir.join(DEFAULT_CLAUDE_ACP_BRIDGE_PROGRAM);
        std::fs::write(&bridge, "#!/bin/sh\nexit 0\n").expect("write fake bridge");
        let mut permissions = std::fs::metadata(&bridge).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&bridge, permissions).expect("chmod fake bridge");
        let path = bin_dir.display().to_string();
        (temp_dir, BTreeMap::from([("PATH".to_string(), path)]))
    }
}
