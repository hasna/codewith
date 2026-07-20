//! Cursor Composer external-agent runtime (design + scaffold).
//!
//! Cursor is modelled as an *external agent runtime*, not as an
//! OpenAI-compatible HTTP model endpoint. Provider execution therefore resolves
//! to `ExternalAgent { runtime: "cursor" }` and never to
//! `OpenAiCompatible { wire_api }`; `WireApi` stays limited to HTTP
//! responses/chat transports for real model providers.
//!
//! Cursor exposes two execution surfaces to a client such as Codewith:
//!
//! * **ACP stdio** (`cursor-agent … acp`) — the *shipping* path. It is
//!   implemented by the shared [`AcpStdioHarness`](crate::AcpStdioHarness) and
//!   reachable through [`cursor_acp_harness`](crate::cursor_acp_harness). ACP is
//!   the better Codewith-as-client fit: JSON-RPC over stdio, session lifecycle,
//!   modes, streaming `session/update`, cancellation, and explicit
//!   `session/request_permission`. All file, terminal, MCP, and network actions
//!   are mediated by Codewith through [`ExternalAgentHost`], never executed by
//!   Cursor directly.
//! * **Cursor Composer SDK** (`@cursor/sdk`, default model Composer 2.5) — the
//!   surface *designed* here and *scaffolded* by [`CursorComposerSdkHarness`],
//!   but intentionally **gated off** until local SDK tool execution can be
//!   proven equivalent to Codewith native enforcement. The SDK executes tools
//!   in-process without a per-call approval callback (only file-based hooks and
//!   an optional local sandbox), so it cannot yet satisfy the same host
//!   mediation guarantees the ACP path provides.
//!
//! This module deliberately keeps the SDK harness a compiling stub: its
//! readiness reports [`ExternalAgentReadinessStatus::Disabled`] and its `run`
//! returns [`ExternalAgentError::NotReady`]. It is *not* a working runtime. See
//! `codex-rs/docs/external_agent_cursor_composer.md` for the full design and
//! the designed / implemented / remaining breakdown.

use crate::ExternalAgentError;
use crate::ExternalAgentHarness;
use crate::ExternalAgentHarnessKind;
use crate::ExternalAgentHost;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentReadinessStatus;
use crate::ExternalAgentRequest;
use crate::ExternalAgentResult;
use crate::ExternalAgentRuntime;
use crate::ExternalAgentRuntimeId;
use crate::find_external_agent_runtime;
use serde::Deserialize;
use serde::Serialize;

/// Default model advertised by the Cursor Composer SDK (`@cursor/sdk`).
pub const CURSOR_COMPOSER_DEFAULT_MODEL: &str = "composer-2.5";

/// Environment variables that carry Cursor credentials for headless SDK/CLI
/// execution. These mirror the ACP harness auth surface so a single Cursor auth
/// profile drives both paths.
pub const CURSOR_COMPOSER_SDK_AUTH_ENV_VARS: &[&str] = &["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"];

/// Reason surfaced whenever the deferred Composer SDK runtime is probed or run.
///
/// Kept as a single constant so the readiness detail and the `run` error stay in
/// lock-step and callers can match on a stable message.
pub const CURSOR_COMPOSER_SDK_DEFERRED_REASON: &str = "Cursor Composer SDK execution is deferred: local @cursor/sdk runs execute tools in-process \
without per-call host approval and cannot yet match Codewith native enforcement. Use the Cursor \
ACP runtime instead.";

/// Where a Composer SDK run would execute.
///
/// `@cursor/sdk` auto-detects the runtime from the agent id prefix (`bc-` for
/// cloud); Codewith selects it explicitly so the capability and audit story is
/// never inferred from an opaque id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorComposerExecution {
    /// The agent loop runs inline in a local process with disk access.
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

/// A Cursor Composer model as it would be surfaced by model discovery.
///
/// The shipping design sources this from `Cursor.models.list()` at runtime; the
/// [`cursor_composer_seed_models`] helper only pins the default so the rest of
/// Codewith can be wired against the shape before discovery lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorComposerModel {
    /// Stable model id passed back to the SDK (for example `composer-2.5`).
    pub id: String,
    /// Human-readable label for pickers.
    pub display_name: String,
    /// Whether this is the runtime's default model.
    pub default: bool,
}

/// Seed model list used until live discovery via `Cursor.models.list()` lands.
///
/// This is intentionally minimal: it fixes the default (`composer-2.5`) and one
/// alternate so UI and config code can be exercised against
/// [`CursorComposerModel`] without a live Cursor account.
pub fn cursor_composer_seed_models() -> Vec<CursorComposerModel> {
    vec![
        CursorComposerModel {
            id: CURSOR_COMPOSER_DEFAULT_MODEL.to_string(),
            display_name: "Composer 2.5".to_string(),
            default: true,
        },
        CursorComposerModel {
            id: "composer-2".to_string(),
            display_name: "Composer 2".to_string(),
            default: false,
        },
    ]
}

/// Scaffold for the Cursor Composer SDK (`@cursor/sdk`) external-agent runtime.
///
/// This type exists to make the design concrete and type-checkable. It
/// implements the common [`ExternalAgentRuntime`]/[`ExternalAgentHarness`]
/// contract but is deliberately inert: it advertises itself as
/// [`ExternalAgentReadinessStatus::Disabled`] and refuses to run. Turning it
/// into a real runtime requires the host-mediation work tracked in the design
/// doc (per-call approval bridging over the SDK's in-process tool execution).
pub struct CursorComposerSdkHarness {
    execution: CursorComposerExecution,
}

impl CursorComposerSdkHarness {
    /// Build a scaffold harness for the given execution surface.
    pub fn new(execution: CursorComposerExecution) -> Self {
        Self { execution }
    }

    /// The execution surface this scaffold targets.
    pub fn execution(&self) -> CursorComposerExecution {
        self.execution
    }

    fn deferred_readiness(&self) -> ExternalAgentReadiness {
        let descriptor = find_external_agent_runtime(ExternalAgentRuntimeId::CURSOR);
        let display_name = descriptor
            .map(|descriptor| descriptor.display_name.to_string())
            .unwrap_or_else(|| "Cursor".to_string());
        let supported_modes = descriptor
            .map(|descriptor| descriptor.supported_modes.to_vec())
            .unwrap_or_default();
        ExternalAgentReadiness {
            runtime: self.id(),
            status: ExternalAgentReadinessStatus::Disabled,
            display_name,
            version: None,
            supported_modes,
            detail: Some(CURSOR_COMPOSER_SDK_DEFERRED_REASON.to_string()),
        }
    }
}

/// Convenience constructor mirroring [`cursor_acp_harness`](crate::cursor_acp_harness).
pub fn cursor_composer_sdk_harness(execution: CursorComposerExecution) -> CursorComposerSdkHarness {
    CursorComposerSdkHarness::new(execution)
}

impl ExternalAgentRuntime for CursorComposerSdkHarness {
    fn id(&self) -> ExternalAgentRuntimeId {
        ExternalAgentRuntimeId::from(ExternalAgentRuntimeId::CURSOR)
    }

    async fn readiness(&self) -> ExternalAgentReadiness {
        self.deferred_readiness()
    }

    async fn run(
        &self,
        request: ExternalAgentRequest,
        _host: impl ExternalAgentHost + Send + Sync,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        Err(ExternalAgentError::NotReady {
            runtime: request.runtime.as_str().to_string(),
            reason: CURSOR_COMPOSER_SDK_DEFERRED_REASON.to_string(),
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

    #[tokio::test]
    async fn sdk_runtime_is_gated_until_native_enforcement_parity() {
        let harness = cursor_composer_sdk_harness(CursorComposerExecution::LocalSdk);

        let readiness = harness.readiness().await;
        assert_eq!(readiness.status, ExternalAgentReadinessStatus::Disabled);
        assert_eq!(readiness.runtime, ExternalAgentRuntimeId::from("cursor"));

        let request =
            ExternalAgentRequest::new("cursor", "inspect repo", "/repo", ExternalAgentMode::Plan);
        let err = harness
            .run(request, DenyHost)
            .await
            .expect_err("Composer SDK runtime must stay gated");
        assert!(matches!(err, ExternalAgentError::NotReady { .. }));
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
    fn seed_models_default_to_composer_2_5() {
        let models = cursor_composer_seed_models();
        let default = models
            .iter()
            .find(|model| model.default)
            .expect("a default Composer model");

        assert_eq!(default.id, CURSOR_COMPOSER_DEFAULT_MODEL);
        assert_eq!(CURSOR_COMPOSER_DEFAULT_MODEL, "composer-2.5");
    }
}
