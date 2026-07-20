//! Extension seam for per-vendor ACP external-agent adapters.
//!
//! The shared [`crate::AcpStdioHarness`] owns every protocol mechanic that is
//! identical across ACP vendors: sanitized subprocess spawn, per-run filesystem
//! isolation, JSON-RPC stdio framing, the `initialize` / `authenticate` /
//! `session` lifecycle, permission and tool-approval bridging, idle timeout,
//! cancellation polling, replay filtering, and structured event normalization.
//!
//! The only pieces that differ between Cursor, Grok Build, Claude, or any future
//! ACP agent are a handful of narrow, mostly-static knobs: which program names
//! to resolve on `PATH`, which vendor auth environment variables to forward,
//! which advertised ACP `authMethods` to prefer, how Codewith modes map to
//! vendor `modeId`s, and whether a readiness probe should run before a session.
//!
//! This module captures exactly those knobs behind the [`AcpAgentAdapter`]
//! trait. A per-vendor adapter is a small, data-only type that plugs into the
//! harness through [`crate::AcpStdioHarness::with_adapter`]. Adding a new ACP
//! vendor therefore never requires editing the shared harness protocol code.

use std::time::Duration;

use crate::ExternalAgentMode;
use crate::ExternalAgentRuntimeDescriptor;
use crate::ExternalAgentRuntimeId;

/// Default timeout applied to an [`AcpReadinessProbe`] when a vendor adapter
/// does not override it. A probe that does not complete within this window is
/// treated as "ready" so a slow-to-print CLI never blocks readiness listing.
pub const ACP_READINESS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Auth methods the harness prefers when a vendor adapter does not narrow the
/// list. `cached_token` is the ACP-standard non-interactive method.
pub const DEFAULT_ACP_AUTH_METHODS: &[&str] = &["cached_token"];

/// A subprocess readiness probe run after a vendor program resolves on `PATH`
/// but before Codewith opens a session.
///
/// The harness runs `<program> <descriptor args> <extra_args>` with the
/// sanitized environment, discards stdout, and captures stderr. A non-zero exit
/// marks the runtime as missing (surfacing the captured stderr as the readiness
/// detail); a timeout is treated as ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpReadinessProbe {
    /// Extra arguments appended after the descriptor command args (for example
    /// `--help`).
    pub extra_args: Vec<String>,
    /// How long to wait for the probe before treating the runtime as ready.
    pub timeout: Duration,
}

impl AcpReadinessProbe {
    /// Build a probe from the given extra arguments using the default timeout.
    pub fn new(extra_args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            extra_args: extra_args.into_iter().map(Into::into).collect(),
            timeout: ACP_READINESS_PROBE_TIMEOUT,
        }
    }

    /// Override the probe timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Per-vendor plug-in for the shared ACP stdio harness.
///
/// Implementations are small, cheap, and generally stateless: they describe how
/// to reach one ACP agent family, not how to speak the protocol. The harness
/// calls these methods; it never lets an adapter touch the child process, the
/// transcript, or the approval flow directly.
///
/// # Contract for adapter authors
///
/// * All methods must be pure and side-effect free; the harness may call them
///   more than once per run (for example during readiness listing and again at
///   launch).
/// * [`auth_env_vars`](AcpAgentAdapter::auth_env_vars) names are only *forwarded
///   when present* in the caller's source environment. Adapters never inject
///   secret values — they only widen the sanitized allow-list by name.
/// * [`runtime_id`](AcpAgentAdapter::runtime_id) must equal the `id` of the
///   [`ExternalAgentRuntimeDescriptor`] the adapter is paired with in
///   [`crate::AcpStdioHarness::with_adapter`].
pub trait AcpAgentAdapter: Send + Sync {
    /// Stable runtime identifier this adapter serves (for example `cursor`).
    fn runtime_id(&self) -> ExternalAgentRuntimeId;

    /// Ordered program names to resolve on `PATH`; the first that resolves is
    /// launched. Defaults to the descriptor's primary command program. Override
    /// to add vendor-specific fallbacks (for example a wrapper binary).
    fn program_candidates(&self, descriptor: &ExternalAgentRuntimeDescriptor) -> Vec<String> {
        vec![descriptor.command.program.to_string()]
    }

    /// Vendor auth environment variable names to forward from the caller's
    /// source environment into the sanitized child environment. Only names that
    /// are actually set upstream are forwarded; values are never invented.
    fn auth_env_vars(&self) -> &'static [&'static str] {
        &[]
    }

    /// Advertised ACP `authMethods` ids to prefer, most-preferred first. The
    /// harness selects the first advertised method that appears in this list.
    fn preferred_auth_methods(&self) -> &'static [&'static str] {
        DEFAULT_ACP_AUTH_METHODS
    }

    /// Map a Codewith [`ExternalAgentMode`] to a vendor ACP `modeId`, if the
    /// vendor exposes an explicit mode for it. `None` leaves the vendor default.
    fn mode_id(&self, mode: ExternalAgentMode) -> Option<&'static str> {
        default_acp_mode_id(mode)
    }

    /// Optional readiness probe run before a session opens. `None` (the default)
    /// treats a resolvable program as ready.
    fn readiness_probe(&self) -> Option<AcpReadinessProbe> {
        None
    }

    /// Static default model id the harness advertises through a session's
    /// best-effort `_meta`, or `None` when the vendor exposes no model concept.
    ///
    /// This intentionally returns a `&'static str`: the harness wires it into
    /// `_meta` on a hot path where it cannot await Cursor's asynchronous model
    /// discovery. Vendors with a live catalog (see
    /// [`crate::discover_cursor_composer_models`]) keep this in lockstep with
    /// their discovery fallback so the advertised default never drifts from the
    /// list the model picker resolves.
    fn default_model(&self) -> Option<&'static str> {
        None
    }
}

/// Default Codewith-mode to ACP-`modeId` mapping shared by adapters that do not
/// override [`AcpAgentAdapter::mode_id`].
pub fn default_acp_mode_id(mode: ExternalAgentMode) -> Option<&'static str> {
    match mode {
        ExternalAgentMode::Plan => Some("plan"),
        ExternalAgentMode::Consult | ExternalAgentMode::Propose | ExternalAgentMode::Managed => None,
    }
}

/// Built-in reference adapter for Cursor's ACP agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorAcpAdapter;

impl AcpAgentAdapter for CursorAcpAdapter {
    fn runtime_id(&self) -> ExternalAgentRuntimeId {
        ExternalAgentRuntimeId::from(ExternalAgentRuntimeId::CURSOR)
    }

    fn program_candidates(&self, descriptor: &ExternalAgentRuntimeDescriptor) -> Vec<String> {
        vec![
            descriptor.command.program.to_string(),
            "cursor-agent".to_string(),
        ]
    }

    fn auth_env_vars(&self) -> &'static [&'static str] {
        &["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"]
    }

    fn preferred_auth_methods(&self) -> &'static [&'static str] {
        &["cursor_login", "cached_token"]
    }

    fn readiness_probe(&self) -> Option<AcpReadinessProbe> {
        Some(AcpReadinessProbe::new(["--help"]))
    }

    fn default_model(&self) -> Option<&'static str> {
        // Kept identical to the live-discovery fallback so the advertised model
        // and the discovered default agree even when discovery is unavailable.
        Some(crate::CURSOR_DEFAULT_COMPOSER_MODEL_ID)
    }
}

/// Built-in reference adapter for xAI's Grok Build ACP stdio agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrokBuildAcpAdapter;

impl AcpAgentAdapter for GrokBuildAcpAdapter {
    fn runtime_id(&self) -> ExternalAgentRuntimeId {
        ExternalAgentRuntimeId::from(ExternalAgentRuntimeId::GROK_BUILD)
    }

    fn auth_env_vars(&self) -> &'static [&'static str] {
        &["XAI_API_KEY"]
    }

    fn preferred_auth_methods(&self) -> &'static [&'static str] {
        &["cached_token", "xai.api_key"]
    }
}

/// Fallback adapter that applies only the shared ACP defaults. Used for runtimes
/// that do not ship a bespoke adapter, and as a minimal template for new ones.
#[derive(Debug, Clone)]
pub struct GenericAcpAdapter {
    runtime: ExternalAgentRuntimeId,
}

impl GenericAcpAdapter {
    pub fn new(runtime: impl Into<ExternalAgentRuntimeId>) -> Self {
        Self {
            runtime: runtime.into(),
        }
    }
}

impl AcpAgentAdapter for GenericAcpAdapter {
    fn runtime_id(&self) -> ExternalAgentRuntimeId {
        self.runtime.clone()
    }
}

/// Resolve the built-in ACP adapter for a runtime id, falling back to a
/// [`GenericAcpAdapter`] for ids without a bespoke adapter.
pub fn builtin_acp_adapter(runtime_id: &str) -> Box<dyn AcpAgentAdapter> {
    match runtime_id {
        ExternalAgentRuntimeId::CURSOR => Box::new(CursorAcpAdapter),
        ExternalAgentRuntimeId::GROK_BUILD => Box::new(GrokBuildAcpAdapter),
        other => Box::new(GenericAcpAdapter::new(ExternalAgentRuntimeId::from(other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_external_agent_runtime;
    use pretty_assertions::assert_eq;

    #[test]
    fn generic_adapter_applies_shared_defaults() {
        let adapter = GenericAcpAdapter::new("example");
        let descriptor = ExternalAgentRuntimeDescriptor {
            id: "example",
            display_name: "Example",
            description: "example",
            command: crate::ExternalAgentCommandSpec {
                program: "example-agent",
                args: &["acp"],
            },
            supported_modes: &[ExternalAgentMode::Plan],
            default_mode: ExternalAgentMode::Plan,
            execution_surfaces: &[crate::ExternalAgentExecutionSurface::Acp],
            default_execution_surface: crate::ExternalAgentExecutionSurface::Acp,
            models: &[],
            visible: false,
        };

        assert_eq!(adapter.runtime_id(), ExternalAgentRuntimeId::from("example"));
        assert_eq!(
            adapter.program_candidates(&descriptor),
            vec!["example-agent".to_string()]
        );
        assert_eq!(adapter.auth_env_vars(), &[] as &[&str]);
        assert_eq!(adapter.preferred_auth_methods(), DEFAULT_ACP_AUTH_METHODS);
        assert_eq!(adapter.mode_id(ExternalAgentMode::Plan), Some("plan"));
        assert_eq!(adapter.mode_id(ExternalAgentMode::Propose), None);
        assert!(adapter.readiness_probe().is_none());
        assert_eq!(adapter.default_model(), None);
    }

    #[test]
    fn cursor_adapter_adds_wrapper_fallback_auth_and_probe() {
        let descriptor =
            find_external_agent_runtime("cursor").unwrap_or_else(|| panic!("cursor runtime"));
        let adapter = CursorAcpAdapter;

        assert_eq!(
            adapter.program_candidates(descriptor),
            vec!["agent".to_string(), "cursor-agent".to_string()]
        );
        assert_eq!(
            adapter.auth_env_vars(),
            &["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"]
        );
        assert_eq!(
            adapter.preferred_auth_methods(),
            &["cursor_login", "cached_token"]
        );
        assert_eq!(
            adapter.readiness_probe(),
            Some(AcpReadinessProbe::new(["--help"]))
        );
    }

    #[test]
    fn cursor_adapter_default_model_matches_discovery_fallback() {
        let adapter = CursorAcpAdapter;

        // The advertised static default must equal the discovery fallback's
        // default id so `_meta` and the live model picker never disagree.
        assert_eq!(
            adapter.default_model(),
            Some(crate::CURSOR_DEFAULT_COMPOSER_MODEL_ID)
        );
        assert_eq!(adapter.default_model(), Some("composer-2.5"));

        let fallback = crate::cursor_composer_fallback_models();
        let fallback_default = fallback
            .iter()
            .find(|model| model.is_default)
            .unwrap_or_else(|| panic!("fallback must have a default model"));
        assert_eq!(adapter.default_model(), Some(fallback_default.id.as_str()));
    }

    #[test]
    fn grok_build_adapter_forwards_xai_auth() {
        let adapter = GrokBuildAcpAdapter;

        assert_eq!(adapter.runtime_id(), ExternalAgentRuntimeId::from("grok-build"));
        assert_eq!(adapter.auth_env_vars(), &["XAI_API_KEY"]);
        assert_eq!(
            adapter.preferred_auth_methods(),
            &["cached_token", "xai.api_key"]
        );
        assert!(adapter.readiness_probe().is_none());
        assert_eq!(adapter.default_model(), None);
    }

    #[test]
    fn builtin_registry_maps_known_ids_and_falls_back() {
        assert_eq!(
            builtin_acp_adapter("cursor").runtime_id(),
            ExternalAgentRuntimeId::from("cursor")
        );
        assert_eq!(
            builtin_acp_adapter("grok-build").runtime_id(),
            ExternalAgentRuntimeId::from("grok-build")
        );
        // Unknown ids resolve to a generic adapter carrying that id.
        let fallback = builtin_acp_adapter("unregistered");
        assert_eq!(
            fallback.runtime_id(),
            ExternalAgentRuntimeId::from("unregistered")
        );
        assert_eq!(fallback.preferred_auth_methods(), DEFAULT_ACP_AUTH_METHODS);
    }
}
