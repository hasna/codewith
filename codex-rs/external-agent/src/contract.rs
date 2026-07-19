use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;

/// Stable identifier for an external-agent runtime adapter.
///
/// Values should be short kebab-case identifiers such as `cursor`,
/// `grok-build`, or `claude`. The identifier selects a Codewith adapter,
/// not a model API transport.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalAgentRuntimeId(String);

impl ExternalAgentRuntimeId {
    pub const CURSOR: &'static str = "cursor";
    pub const GROK_BUILD: &'static str = "grok-build";
    pub const CLAUDE: &'static str = "claude";

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ExternalAgentRuntimeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ExternalAgentRuntimeId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// User-visible mode for an external-agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum ExternalAgentMode {
    /// Ask for analysis only. No project mutation should be requested.
    Consult,
    /// Produce a plan. This is the safest default for unknown runtimes.
    #[default]
    Plan,
    /// Inspect and propose actions that Codewith may review or execute.
    Propose,
    /// Let the runtime request file, terminal, MCP, or network actions through
    /// Codewith approval and sandbox paths.
    Managed,
}

/// Capability policy Codewith grants to an external runtime for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentCapabilities {
    pub filesystem: FileSystemCapability,
    pub terminal: TerminalCapability,
    pub mcp: McpCapability,
    pub network: NetworkCapability,
    pub environment: EnvironmentCapability,
}

impl ExternalAgentCapabilities {
    pub fn for_mode(mode: ExternalAgentMode) -> Self {
        match mode {
            ExternalAgentMode::Consult | ExternalAgentMode::Plan => Self {
                filesystem: FileSystemCapability::None,
                terminal: TerminalCapability::None,
                mcp: McpCapability::None,
                network: NetworkCapability::None,
                environment: EnvironmentCapability::Sanitized,
            },
            ExternalAgentMode::Propose => Self {
                filesystem: FileSystemCapability::ReadOnly,
                terminal: TerminalCapability::None,
                mcp: McpCapability::ManagedReadOnly,
                network: NetworkCapability::Managed,
                environment: EnvironmentCapability::Sanitized,
            },
            ExternalAgentMode::Managed => Self {
                filesystem: FileSystemCapability::ManagedReadWrite,
                terminal: TerminalCapability::Managed,
                mcp: McpCapability::ManagedReadWrite,
                network: NetworkCapability::Managed,
                environment: EnvironmentCapability::Sanitized,
            },
        }
    }

    pub fn requires_host_mediation(&self) -> bool {
        !matches!(self.filesystem, FileSystemCapability::None)
            || !matches!(self.terminal, TerminalCapability::None)
            || !matches!(self.mcp, McpCapability::None)
            || !matches!(self.network, NetworkCapability::None)
    }
}

impl Default for ExternalAgentCapabilities {
    fn default() -> Self {
        Self::for_mode(ExternalAgentMode::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileSystemCapability {
    None,
    ReadOnly,
    ManagedReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalCapability {
    None,
    Managed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpCapability {
    None,
    ManagedReadOnly,
    ManagedReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkCapability {
    None,
    Managed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentCapability {
    Sanitized,
}

/// Request sent from Codewith to an external-agent runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentRequest {
    pub runtime: ExternalAgentRuntimeId,
    pub task: String,
    pub cwd: PathBuf,
    pub mode: ExternalAgentMode,
    pub capabilities: ExternalAgentCapabilities,
    pub session: ExternalAgentSessionRequest,
    pub context: ExternalAgentContext,
    pub metadata: BTreeMap<String, String>,
}

impl ExternalAgentRequest {
    pub fn new(
        runtime: impl Into<ExternalAgentRuntimeId>,
        task: impl Into<String>,
        cwd: impl Into<PathBuf>,
        mode: ExternalAgentMode,
    ) -> Self {
        Self {
            runtime: runtime.into(),
            task: task.into(),
            cwd: cwd.into(),
            mode,
            capabilities: ExternalAgentCapabilities::for_mode(mode),
            session: ExternalAgentSessionRequest::New,
            context: ExternalAgentContext::default(),
            metadata: BTreeMap::new(),
        }
    }
}

/// Context Codewith chooses to share with the external agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentContext {
    pub conversation_summary: Option<String>,
    pub selected_text: Option<String>,
    pub mentioned_files: Vec<PathBuf>,
    pub mcp_facades: Vec<ExternalAgentMcpFacade>,
}

/// Codewith-owned MCP facade intentionally scoped to one external-agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentMcpFacade {
    pub name: String,
    pub tools: Vec<String>,
}

/// Requested external-session behavior for a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ExternalAgentSessionRequest {
    New,
    Resume { external_session_id: String },
}

/// Runtime session state resolved during or after a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentSessionState {
    pub runtime: ExternalAgentRuntimeId,
    pub external_session_id: Option<String>,
    pub mode: ExternalAgentMode,
    pub cwd: PathBuf,
}

/// Readiness metadata for a configured runtime adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentReadiness {
    pub runtime: ExternalAgentRuntimeId,
    pub status: ExternalAgentReadinessStatus,
    pub display_name: String,
    pub version: Option<String>,
    pub supported_modes: Vec<ExternalAgentMode>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAgentReadinessStatus {
    Ready,
    MissingRuntime,
    MissingAuth,
    Unsupported,
    Disabled,
}

/// Structured events emitted by external-agent runtimes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ExternalAgentEvent {
    RunStarted {
        session: ExternalAgentSessionState,
    },
    SessionResolved {
        session: ExternalAgentSessionState,
    },
    OutputTextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    PlanUpdated {
        plan: String,
    },
    PermissionRequested {
        request: ExternalAgentPermissionRequest,
    },
    ProposedAction {
        proposal: ExternalAgentActionRequest,
    },
    Artifact {
        artifact: ExternalAgentArtifact,
    },
    Status {
        message: String,
    },
    Completed {
        result: ExternalAgentResult,
    },
    Failed {
        message: String,
    },
    Cancelled {
        reason: Option<String>,
    },
}

/// A permission request surfaced by a runtime and answered by Codewith.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentPermissionRequest {
    pub id: String,
    pub action: ExternalAgentActionRequest,
    pub options: Vec<ExternalAgentPermissionOption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAgentPermissionOption {
    AllowOnce,
    RejectOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAgentPermissionDecision {
    AllowOnce,
    RejectOnce,
}

/// File, terminal, MCP, or network action requested or proposed by a runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ExternalAgentActionRequest {
    ReadFile {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        content: String,
    },
    RunCommand {
        command: Vec<String>,
        cwd: Option<PathBuf>,
    },
    McpToolCall {
        server: String,
        tool: String,
        arguments: JsonValue,
    },
    NetworkAccess {
        target: String,
        purpose: Option<String>,
    },
    Other {
        label: String,
        payload: JsonValue,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ExternalAgentActionResult {
    FileContent {
        content: String,
    },
    WriteAccepted,
    CommandOutput {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    McpToolResult {
        result: JsonValue,
    },
    NetworkAccessReady,
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentArtifact {
    pub label: String,
    pub path: Option<PathBuf>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentResult {
    pub status: ExternalAgentRunStatus,
    pub session: ExternalAgentSessionState,
    pub summary: Option<String>,
    pub artifacts: Vec<ExternalAgentArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAgentRunStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalAgentError {
    #[error("external agent runtime `{runtime}` rejected launch request: {message}")]
    InvalidRequest { runtime: String, message: String },
    #[error("external agent runtime `{runtime}` is not ready: {reason}")]
    NotReady { runtime: String, reason: String },
    #[error("external agent runtime `{runtime}` protocol error: {message}")]
    Protocol { runtime: String, message: String },
    #[error("external agent runtime `{runtime}` failed: {message}")]
    Runtime { runtime: String, message: String },
    #[error("external agent run was cancelled")]
    Cancelled,
}

/// Host services used by runtimes to report progress and request managed work.
///
/// Implementations should append events to the current transcript/audit stream
/// before updating UI state so replay remains deterministic after a crash.
/// Managed actions must be executed by Codewith through this host instead of
/// directly by the external runtime.
pub trait ExternalAgentHost {
    fn emit(
        &self,
        event: ExternalAgentEvent,
    ) -> impl Future<Output = Result<(), ExternalAgentError>> + Send;

    fn request_permission(
        &self,
        request: ExternalAgentPermissionRequest,
    ) -> impl Future<Output = Result<ExternalAgentPermissionDecision, ExternalAgentError>> + Send;

    fn perform_action(
        &self,
        action: ExternalAgentActionRequest,
    ) -> impl Future<Output = Result<ExternalAgentActionResult, ExternalAgentError>> + Send;

    fn is_cancelled(&self) -> impl Future<Output = bool> + Send;
}

/// Runtime adapter for one external coding-agent family.
///
/// Implementations own runtime-specific process, SDK, ACP, or cloud-agent
/// details. They must respect the `ExternalAgentCapabilities` on each request
/// and route managed actions through Codewith approval and sandbox services.
pub trait ExternalAgentRuntime {
    fn id(&self) -> ExternalAgentRuntimeId;

    fn readiness(&self) -> impl Future<Output = ExternalAgentReadiness> + Send;

    fn run(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
    ) -> impl Future<Output = Result<ExternalAgentResult, ExternalAgentError>> + Send;
}

/// Concrete harness family used by an external-agent adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAgentHarnessKind {
    AcpStdio,
    Sdk,
    Cloud,
}

/// Adapter that wraps a concrete external-agent harness.
///
/// The harness owns process, ACP, SDK, or cloud-agent mechanics while exposing
/// the common [`ExternalAgentRuntime`] contract to the rest of Codewith.
pub trait ExternalAgentHarness: ExternalAgentRuntime {
    fn harness_kind(&self) -> ExternalAgentHarnessKind;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[test]
    fn request_defaults_capabilities_from_mode() {
        let request = ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "inspect this repo",
            "/repo",
            ExternalAgentMode::Managed,
        );

        let expected = ExternalAgentRequest {
            runtime: ExternalAgentRuntimeId::from("cursor"),
            task: "inspect this repo".to_string(),
            cwd: PathBuf::from("/repo"),
            mode: ExternalAgentMode::Managed,
            capabilities: ExternalAgentCapabilities {
                filesystem: FileSystemCapability::ManagedReadWrite,
                terminal: TerminalCapability::Managed,
                mcp: McpCapability::ManagedReadWrite,
                network: NetworkCapability::Managed,
                environment: EnvironmentCapability::Sanitized,
            },
            session: ExternalAgentSessionRequest::New,
            context: ExternalAgentContext::default(),
            metadata: BTreeMap::new(),
        };

        assert_eq!(request, expected);
    }

    #[test]
    fn mvp_modes_never_grant_direct_mutation() {
        for mode in [
            ExternalAgentMode::Consult,
            ExternalAgentMode::Plan,
            ExternalAgentMode::Propose,
            ExternalAgentMode::Managed,
        ] {
            let capabilities = ExternalAgentCapabilities::for_mode(mode);
            let value = serde_json::to_value(capabilities)
                .unwrap_or_else(|err| panic!("serialize capabilities: {err}"));
            let text = value.to_string();

            assert!(
                !text.contains("direct"),
                "{mode:?} should not serialize any direct capability: {text}"
            );
        }
    }

    #[test]
    fn event_serialization_uses_stable_kebab_case_tags() {
        let event = ExternalAgentEvent::ProposedAction {
            proposal: ExternalAgentActionRequest::RunCommand {
                command: vec!["cargo".to_string(), "test".to_string()],
                cwd: Some(PathBuf::from("/repo")),
            },
        };

        let value =
            serde_json::to_value(event).unwrap_or_else(|err| panic!("serialize event: {err}"));

        assert_eq!(
            value,
            serde_json::json!({
                "type": "proposed-action",
                "proposal": {
                    "type": "run-command",
                    "command": ["cargo", "test"],
                    "cwd": "/repo"
                }
            })
        );
    }

    #[tokio::test]
    async fn fake_runtime_can_emit_contract_events() {
        #[derive(Clone, Default)]
        struct RecordingHost {
            events: Arc<Mutex<Vec<ExternalAgentEvent>>>,
        }

        impl RecordingHost {
            fn events(&self) -> Vec<ExternalAgentEvent> {
                self.events
                    .lock()
                    .unwrap_or_else(|err| panic!("events lock: {err}"))
                    .clone()
            }
        }

        impl ExternalAgentHost for RecordingHost {
            async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
                self.events
                    .lock()
                    .unwrap_or_else(|err| panic!("events lock: {err}"))
                    .push(event);
                Ok(())
            }

            async fn request_permission(
                &self,
                _request: ExternalAgentPermissionRequest,
            ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
                Ok(ExternalAgentPermissionDecision::RejectOnce)
            }

            async fn perform_action(
                &self,
                _action: ExternalAgentActionRequest,
            ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
                Ok(ExternalAgentActionResult::Rejected {
                    reason: "fake host does not execute actions".to_string(),
                })
            }

            async fn is_cancelled(&self) -> bool {
                false
            }
        }

        struct FakeRuntime;

        impl ExternalAgentRuntime for FakeRuntime {
            fn id(&self) -> ExternalAgentRuntimeId {
                ExternalAgentRuntimeId::from("fake")
            }

            async fn readiness(&self) -> ExternalAgentReadiness {
                ExternalAgentReadiness {
                    runtime: self.id(),
                    status: ExternalAgentReadinessStatus::Ready,
                    display_name: "Fake".to_string(),
                    version: None,
                    supported_modes: vec![ExternalAgentMode::Plan],
                    detail: None,
                }
            }

            async fn run(
                &self,
                request: ExternalAgentRequest,
                host: impl ExternalAgentHost + Send + Sync,
            ) -> Result<ExternalAgentResult, ExternalAgentError> {
                let session = ExternalAgentSessionState {
                    runtime: request.runtime,
                    external_session_id: Some("fake-session".to_string()),
                    mode: request.mode,
                    cwd: request.cwd,
                };
                host.emit(ExternalAgentEvent::RunStarted {
                    session: session.clone(),
                })
                .await?;
                let permission = host
                    .request_permission(ExternalAgentPermissionRequest {
                        id: "permission-1".to_string(),
                        action: ExternalAgentActionRequest::RunCommand {
                            command: vec!["cargo".to_string(), "test".to_string()],
                            cwd: Some(session.cwd.clone()),
                        },
                        options: vec![ExternalAgentPermissionOption::AllowOnce],
                    })
                    .await?;
                if permission == ExternalAgentPermissionDecision::AllowOnce {
                    host.perform_action(ExternalAgentActionRequest::RunCommand {
                        command: vec!["cargo".to_string(), "test".to_string()],
                        cwd: Some(session.cwd.clone()),
                    })
                    .await?;
                }
                Ok(ExternalAgentResult {
                    status: ExternalAgentRunStatus::Completed,
                    session,
                    summary: Some("fake runtime completed".to_string()),
                    artifacts: Vec::new(),
                })
            }
        }

        let runtime = FakeRuntime;
        let host = RecordingHost::default();
        let request =
            ExternalAgentRequest::new("fake", "plan work", "/repo", ExternalAgentMode::Plan);

        let result = runtime
            .run(request, host.clone())
            .await
            .unwrap_or_else(|err| panic!("fake runtime should complete: {err}"));

        assert_eq!(result.status, ExternalAgentRunStatus::Completed);
        assert_eq!(
            host.events(),
            vec![ExternalAgentEvent::RunStarted {
                session: ExternalAgentSessionState {
                    runtime: ExternalAgentRuntimeId::from("fake"),
                    external_session_id: Some("fake-session".to_string()),
                    mode: ExternalAgentMode::Plan,
                    cwd: PathBuf::from("/repo"),
                }
            }]
        );
    }
}
