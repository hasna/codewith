//! Cursor Composer external-agent runtime (host-mediated).
//!
//! Cursor is modelled as an *external agent runtime*, not as an
//! OpenAI-compatible HTTP model endpoint. Provider execution therefore resolves
//! to `ExternalAgent { runtime: "cursor" }` and never to
//! `OpenAiCompatible { wire_api }`; `WireApi` stays limited to HTTP
//! responses/chat transports for real model providers.
//!
//! Cursor exposes three execution surfaces to a client such as Codewith:
//!
//! * **ACP stdio** (`cursor-agent … acp`) — implemented by the shared
//!   [`AcpStdioHarness`](crate::AcpStdioHarness) and reachable through
//!   [`cursor_acp_harness`](crate::cursor_acp_harness).
//! * **Cursor Composer SDK, local** (`@cursor/sdk`, default model Composer 2.5)
//!   — a real, host-mediated runtime implemented by [`crate::cursor_sdk`]. The
//!   SDK runs inside a Codewith-owned Node sidecar with its built-in mutating
//!   tools (`shell`/`write`/`edit`) disabled and only Codewith custom tools
//!   exposed; every tool call is bridged to
//!   [`ExternalAgentHost::request_permission`](crate::ExternalAgentHost) +
//!   [`ExternalAgentHost::perform_action`](crate::ExternalAgentHost) so the Comp2
//!   guard/executor stays the sole enforcer.
//! * **Cursor Composer SDK, cloud** (`bc-` background agents) — implemented by
//!   [`crate::cursor_cloud`], gated behind explicit data-egress consent.
//!
//! [`CursorComposerSdkHarness`] is the single harness the runner/picker (Comp4)
//! registers; it selects a backend from [`CursorComposerExecution`] and exposes
//! the same `readiness_with_env` / `run_sandboxed_with_env` entry points as the
//! other SDK/ACP harnesses in this crate. The real execution entry is
//! [`CursorComposerSdkHarness::run_sandboxed_with_env`]; the trait
//! [`ExternalAgentRuntime::run`] is a routing shim (matching
//! [`ClaudeCodeHarness`](crate::ClaudeCodeHarness)) because subprocess execution
//! must be wrapped in Codewith's platform sandbox, whose config is supplied by
//! the caller.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::CursorCloudBackend;
use crate::CursorCloudEgressConsent;
use crate::CursorSdkBackend;
use crate::ExternalAgentError;
use crate::ExternalAgentHarness;
use crate::ExternalAgentHarnessKind;
use crate::ExternalAgentHost;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentRequest;
use crate::ExternalAgentResult;
use crate::ExternalAgentRuntime;
use crate::ExternalAgentRuntimeDescriptor;
use crate::ExternalAgentRuntimeId;
use crate::ExternalAgentSandboxConfig;
use crate::find_external_agent_runtime;

/// Environment variables that carry Cursor credentials for headless SDK/cloud
/// execution. These mirror the ACP harness auth surface so a single Cursor auth
/// profile drives every Cursor path.
pub const CURSOR_COMPOSER_SDK_AUTH_ENV_VARS: &[&str] = &["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"];

/// The built-in descriptor for the Cursor runtime.
///
/// The Cursor runtime is always present in
/// [`BUILTIN_EXTERNAL_AGENT_RUNTIMES`](crate::BUILTIN_EXTERNAL_AGENT_RUNTIMES),
/// so this cannot fail.
pub fn cursor_composer_runtime_descriptor() -> &'static ExternalAgentRuntimeDescriptor {
    find_external_agent_runtime(ExternalAgentRuntimeId::CURSOR)
        .unwrap_or_else(|| unreachable!("the built-in Cursor runtime descriptor is always present"))
}

/// Where a Composer SDK run executes.
///
/// `@cursor/sdk` auto-detects the runtime from the agent id prefix (`bc-` for
/// cloud); Codewith selects it explicitly so the capability and audit story is
/// never inferred from an opaque id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorComposerExecution {
    /// The agent loop runs inline in a local Codewith-owned Node sidecar, with
    /// every tool call bridged to the Codewith host.
    LocalSdk,
    /// The agent loop runs on a Cursor-hosted VM against a cloned repo.
    Cloud,
}

impl CursorComposerExecution {
    /// Map the execution surface to the harness family Codewith records for it.
    pub fn harness_kind(self) -> ExternalAgentHarnessKind {
        match self {
            Self::LocalSdk => ExternalAgentHarnessKind::Sdk,
            Self::Cloud => ExternalAgentHarnessKind::Cloud,
        }
    }
}

/// Host-mediated Cursor Composer runtime over `@cursor/sdk`.
///
/// Holds both backends and dispatches on [`CursorComposerExecution`]. A cloud
/// harness carries an explicit [`CursorCloudEgressConsent`]; the local backend
/// ignores it.
#[derive(Debug, Clone)]
pub struct CursorComposerSdkHarness {
    execution: CursorComposerExecution,
    local: CursorSdkBackend,
    cloud: CursorCloudBackend,
}

impl CursorComposerSdkHarness {
    /// Build a harness for the given execution surface.
    ///
    /// Cloud harnesses built through `new` default to
    /// [`CursorCloudEgressConsent::Denied`]; use [`CursorComposerSdkHarness::cloud`]
    /// to grant consent.
    pub fn new(execution: CursorComposerExecution) -> Self {
        Self {
            execution,
            local: CursorSdkBackend::new(),
            cloud: CursorCloudBackend::new(CursorCloudEgressConsent::default()),
        }
    }

    /// Build a local `@cursor/sdk` harness.
    pub fn local() -> Self {
        Self::new(CursorComposerExecution::LocalSdk)
    }

    /// Build a cloud (`bc-`) harness with an explicit egress-consent decision.
    pub fn cloud(consent: CursorCloudEgressConsent) -> Self {
        Self {
            execution: CursorComposerExecution::Cloud,
            local: CursorSdkBackend::new(),
            cloud: CursorCloudBackend::new(consent),
        }
    }

    /// The execution surface this harness targets.
    pub fn execution(&self) -> CursorComposerExecution {
        self.execution
    }

    /// Readiness for a given source environment.
    pub async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        match self.execution {
            CursorComposerExecution::LocalSdk => self.local.readiness_with_env(source_env).await,
            CursorComposerExecution::Cloud => self.cloud.readiness_with_env(source_env).await,
        }
    }

    /// Execute a run inside Codewith's platform sandbox. This is the real entry
    /// point the runner uses.
    pub async fn run_sandboxed_with_env(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
        source_env: BTreeMap<String, String>,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        match self.execution {
            CursorComposerExecution::LocalSdk => {
                self.local
                    .run_sandboxed_with_env(request, host, sandbox_config, source_env)
                    .await
            }
            CursorComposerExecution::Cloud => {
                self.cloud
                    .run_sandboxed_with_env(request, host, sandbox_config, source_env)
                    .await
            }
        }
    }

    /// Execute a run resolving the source environment from the current process.
    pub async fn run_sandboxed(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        let source_env = std::env::vars().collect::<BTreeMap<_, _>>();
        self.run_sandboxed_with_env(request, host, sandbox_config, source_env)
            .await
    }
}

/// Convenience constructor mirroring [`cursor_acp_harness`](crate::cursor_acp_harness).
pub fn cursor_composer_sdk_harness(execution: CursorComposerExecution) -> CursorComposerSdkHarness {
    CursorComposerSdkHarness::new(execution)
}

/// Convenience constructor for the local `@cursor/sdk` harness.
pub fn cursor_composer_local_harness() -> CursorComposerSdkHarness {
    CursorComposerSdkHarness::local()
}

/// Convenience constructor for the cloud (`bc-`) harness.
pub fn cursor_composer_cloud_harness(
    consent: CursorCloudEgressConsent,
) -> CursorComposerSdkHarness {
    CursorComposerSdkHarness::cloud(consent)
}

impl ExternalAgentRuntime for CursorComposerSdkHarness {
    fn id(&self) -> ExternalAgentRuntimeId {
        ExternalAgentRuntimeId::from(ExternalAgentRuntimeId::CURSOR)
    }

    async fn readiness(&self) -> ExternalAgentReadiness {
        let source_env = std::env::vars().collect::<BTreeMap<_, _>>();
        self.readiness_with_env(&source_env).await
    }

    async fn run(
        &self,
        request: ExternalAgentRequest,
        _host: impl ExternalAgentHost + Send + Sync,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        // Subprocess execution must be wrapped in Codewith's platform sandbox,
        // whose config is not available on this trait method. Callers use
        // `run_sandboxed_with_env`, exactly like `ClaudeCodeHarness`.
        Err(ExternalAgentError::NotReady {
            runtime: request.runtime.as_str().to_string(),
            reason: "Cursor Composer must be executed with \
CursorComposerSdkHarness::run_sandboxed_with_env"
                .to_string(),
        })
    }
}

impl ExternalAgentHarness for CursorComposerSdkHarness {
    fn harness_kind(&self) -> ExternalAgentHarnessKind {
        self.execution.harness_kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExternalAgentActionRequest;
    use crate::ExternalAgentActionResult;
    use crate::ExternalAgentEvent;
    use crate::ExternalAgentMode;
    use crate::ExternalAgentPermissionDecision;
    use crate::ExternalAgentPermissionRequest;
    use crate::ExternalAgentReadinessStatus;
    use pretty_assertions::assert_eq;

    struct DenyHost;

    impl ExternalAgentHost for DenyHost {
        async fn emit(&self, _event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
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
                reason: "deny host does not execute actions".to_string(),
            })
        }

        async fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn execution_maps_to_expected_harness_kind() {
        assert_eq!(
            cursor_composer_sdk_harness(CursorComposerExecution::LocalSdk).harness_kind(),
            ExternalAgentHarnessKind::Sdk
        );
        assert_eq!(
            cursor_composer_sdk_harness(CursorComposerExecution::Cloud).harness_kind(),
            ExternalAgentHarnessKind::Cloud
        );
    }

    #[test]
    fn constructors_select_execution() {
        assert_eq!(
            cursor_composer_local_harness().execution(),
            CursorComposerExecution::LocalSdk
        );
        let cloud = cursor_composer_cloud_harness(CursorCloudEgressConsent::Granted);
        assert_eq!(cloud.execution(), CursorComposerExecution::Cloud);
        assert_eq!(cloud.harness_kind(), ExternalAgentHarnessKind::Cloud);
    }

    #[test]
    fn harness_reports_cursor_runtime_id() {
        let harness = cursor_composer_local_harness();
        assert_eq!(harness.id(), ExternalAgentRuntimeId::from("cursor"));
    }

    #[tokio::test]
    async fn run_routes_callers_to_the_sandboxed_entry_point() {
        let harness = cursor_composer_local_harness();
        let request =
            ExternalAgentRequest::new("cursor", "inspect repo", "/repo", ExternalAgentMode::Plan);
        let err = harness
            .run(request, DenyHost)
            .await
            .expect_err("trait run() is a routing shim");
        match err {
            ExternalAgentError::NotReady { reason, .. } => {
                assert!(reason.contains("run_sandboxed_with_env"), "{reason}");
                assert!(!reason.to_lowercase().contains("deferred"), "{reason}");
            }
            other => panic!("expected NotReady routing error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn readiness_never_reports_disabled() {
        // The runtime is real: readiness reflects environment availability
        // (Ready / MissingRuntime / MissingAuth), never a gated Disabled status.
        let harness = cursor_composer_local_harness();
        let env = BTreeMap::from([("PATH".to_string(), "/nonexistent-bin".to_string())]);
        let readiness = harness.readiness_with_env(&env).await;
        assert_ne!(readiness.status, ExternalAgentReadinessStatus::Disabled);
        assert_eq!(readiness.status, ExternalAgentReadinessStatus::MissingRuntime);
    }

    #[tokio::test]
    async fn cloud_harness_from_new_defaults_to_denied_consent() {
        let harness = CursorComposerSdkHarness::new(CursorComposerExecution::Cloud);
        let sandbox = ExternalAgentSandboxConfig::new(
            codex_protocol::models::PermissionProfile::External {
                network: codex_protocol::permissions::NetworkSandboxPolicy::Restricted,
            },
        );
        let request =
            ExternalAgentRequest::new("cursor", "ship", "/repo", ExternalAgentMode::Plan);
        let err = harness
            .run_sandboxed_with_env(request, DenyHost, &sandbox, BTreeMap::new())
            .await
            .expect_err("cloud run without consent must refuse");
        assert!(matches!(err, ExternalAgentError::NotReady { .. }));
    }
}
