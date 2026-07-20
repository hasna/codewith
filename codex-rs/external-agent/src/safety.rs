//! Managed-mode action safety policy for external-agent runtimes.
//!
//! Codewith never lets an external runtime perform host side effects directly.
//! Every file, terminal, MCP, or network action an external agent asks for is
//! first classified here into an [`ExternalAgentActionDecision`]. The decision
//! tells the Codewith host *how it* (never the runtime) should realize the
//! action: perform a confined read, promote a write through `apply_patch`,
//! delegate a command to the native sandboxed exec path, or route an MCP call
//! through the native approval path -- or simply record the request as a
//! reviewable proposal or deny it outright.
//!
//! The single invariant this module enforces is that
//! [`ExternalAgentActionDecision::authorizes_runtime_side_effect`] is *always*
//! false: no classification, in any mode, ever authorizes the runtime itself to
//! mutate the host. Codewith executes; the runtime only asks. The delegation
//! decisions ([`ExternalAgentActionDecision::DelegateCommand`],
//! [`ExternalAgentActionDecision::PromoteWrite`], and
//! [`ExternalAgentActionDecision::DelegateMcp`]) are only ever granted when the
//! run's mode and capabilities are managed.

use crate::ExternalAgentActionRequest;
use crate::ExternalAgentCapabilities;
use crate::ExternalAgentMode;
use crate::ExternalAgentRequest;
use crate::FileSystemCapability;
use crate::McpCapability;
use crate::NetworkCapability;
use crate::TerminalCapability;

/// How the Codewith host should realize a runtime-requested action.
///
/// This is a *classification*, not an execution: the host consumes the decision
/// and performs (or refuses) the effect itself. No variant authorizes the
/// external runtime to touch the host -- see
/// [`ExternalAgentActionDecision::authorizes_runtime_side_effect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAgentActionDecision {
    /// Codewith performs a confined, read-only file read on the runtime's
    /// behalf. The host still applies canonicalize + `can_read` recheck + a
    /// byte cap before returning any content.
    PerformRead,
    /// Codewith records the request as a reviewable proposal in the transcript
    /// but does not execute it. This is the safe behavior for non-managed modes
    /// (`Consult`/`Plan`/`Propose`) where the runtime may only suggest work.
    RecordProposal,
    /// Codewith refuses the action outright. `reason` is surfaced to the
    /// runtime and recorded in the audit trail.
    Deny { reason: String },
    /// Managed delegation: Codewith runs the command itself through the native
    /// sandboxed exec / `ToolOrchestrator` path and returns its output. Granted
    /// only in managed mode with a managed terminal capability.
    DelegateCommand,
    /// Managed delegation: Codewith promotes the write into a staged-and-applied
    /// patch through its native `apply_patch` pipeline. Granted only in managed
    /// mode with a managed read-write filesystem capability.
    PromoteWrite,
    /// Managed delegation: Codewith routes the MCP tool call through the native
    /// MCP approval path. Granted only in managed mode with an MCP capability.
    DelegateMcp,
}

impl ExternalAgentActionDecision {
    fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
        }
    }

    /// Core safety invariant: **no** decision ever authorizes the external
    /// *runtime* to perform a host side effect. Even the delegation decisions
    /// mean *Codewith* performs the effect on the runtime's behalf, never the
    /// runtime itself. This must always return `false`; a test asserts it for
    /// every variant so a future variant cannot silently opt in.
    #[must_use]
    pub const fn authorizes_runtime_side_effect(&self) -> bool {
        match self {
            Self::PerformRead
            | Self::RecordProposal
            | Self::Deny { .. }
            | Self::DelegateCommand
            | Self::PromoteWrite
            | Self::DelegateMcp => false,
        }
    }

    /// Whether the decision requires Codewith to run managed delegation
    /// machinery (native exec, `apply_patch`, or MCP). Reads, proposals, and
    /// denials do not.
    #[must_use]
    pub const fn is_managed_delegation(&self) -> bool {
        matches!(
            self,
            Self::DelegateCommand | Self::PromoteWrite | Self::DelegateMcp
        )
    }

    /// Whether the decision permits Codewith to actually realize an effect
    /// (a read or a managed delegation), as opposed to merely recording a
    /// proposal or denying the request.
    #[must_use]
    pub const fn is_executable(&self) -> bool {
        matches!(
            self,
            Self::PerformRead | Self::DelegateCommand | Self::PromoteWrite | Self::DelegateMcp
        )
    }
}

/// Classifies runtime action requests into [`ExternalAgentActionDecision`]s for
/// a single run.
///
/// The guard captures only the run's mode and capability grant, both fixed for
/// the lifetime of a run, so it is cheap to clone and hold alongside the host.
/// It performs *policy* decisions only; the physical confinement of a read
/// (canonicalize + `can_read` recheck + byte cap) and the native delegation of
/// writes/commands/MCP live in the app-server executor that consumes these
/// decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentActionGuard {
    mode: ExternalAgentMode,
    capabilities: ExternalAgentCapabilities,
}

impl ExternalAgentActionGuard {
    /// Build a guard from an explicit mode and capability grant.
    #[must_use]
    pub fn new(mode: ExternalAgentMode, capabilities: ExternalAgentCapabilities) -> Self {
        Self { mode, capabilities }
    }

    /// Build a guard from the mode and capabilities carried by a run request.
    #[must_use]
    pub fn for_request(request: &ExternalAgentRequest) -> Self {
        Self::new(request.mode, request.capabilities.clone())
    }

    #[must_use]
    pub fn mode(&self) -> ExternalAgentMode {
        self.mode
    }

    #[must_use]
    pub fn capabilities(&self) -> &ExternalAgentCapabilities {
        &self.capabilities
    }

    /// A run is "managed" only in [`ExternalAgentMode::Managed`]. Delegation
    /// decisions are gated on this in addition to the specific capability.
    #[must_use]
    pub fn is_managed(&self) -> bool {
        matches!(self.mode, ExternalAgentMode::Managed)
    }

    /// Classify a single action request.
    #[must_use]
    pub fn decide(&self, action: &ExternalAgentActionRequest) -> ExternalAgentActionDecision {
        match action {
            ExternalAgentActionRequest::ReadFile { .. } => self.decide_read(),
            ExternalAgentActionRequest::WriteFile { .. } => self.decide_write(),
            ExternalAgentActionRequest::RunCommand { .. } => self.decide_command(),
            ExternalAgentActionRequest::McpToolCall { .. } => self.decide_mcp(),
            ExternalAgentActionRequest::NetworkAccess { .. } => self.decide_network(),
            ExternalAgentActionRequest::Other { label, .. } => ExternalAgentActionDecision::deny(
                format!("unsupported external-agent action `{label}`"),
            ),
        }
    }

    /// Reads are always host-performed and confined. They are allowed whenever
    /// the run holds any read capability (`Propose` grants read-only, `Managed`
    /// grants read-write). No read is ever a runtime side effect.
    fn decide_read(&self) -> ExternalAgentActionDecision {
        match self.capabilities.filesystem {
            FileSystemCapability::ReadOnly | FileSystemCapability::ManagedReadWrite => {
                ExternalAgentActionDecision::PerformRead
            }
            FileSystemCapability::None => {
                ExternalAgentActionDecision::deny("no filesystem read capability granted")
            }
        }
    }

    /// Writes are promoted through `apply_patch` only in managed mode with a
    /// managed read-write filesystem capability. In propose mode a write is
    /// recorded as a proposal; otherwise it is denied.
    fn decide_write(&self) -> ExternalAgentActionDecision {
        match (self.mode, self.capabilities.filesystem) {
            (ExternalAgentMode::Managed, FileSystemCapability::ManagedReadWrite) => {
                ExternalAgentActionDecision::PromoteWrite
            }
            (ExternalAgentMode::Managed, _) => ExternalAgentActionDecision::deny(
                "managed writes require a managed read-write filesystem capability",
            ),
            (ExternalAgentMode::Propose, _) => ExternalAgentActionDecision::RecordProposal,
            (ExternalAgentMode::Consult | ExternalAgentMode::Plan, _) => {
                ExternalAgentActionDecision::deny(
                    "filesystem writes require propose or managed mode",
                )
            }
        }
    }

    /// Commands are delegated to the native sandboxed exec path only in managed
    /// mode with a managed terminal capability. In propose mode a command is
    /// recorded as a proposal; otherwise it is denied.
    fn decide_command(&self) -> ExternalAgentActionDecision {
        match (self.mode, self.capabilities.terminal) {
            (ExternalAgentMode::Managed, TerminalCapability::Managed) => {
                ExternalAgentActionDecision::DelegateCommand
            }
            (ExternalAgentMode::Managed, TerminalCapability::None) => {
                ExternalAgentActionDecision::deny(
                    "managed command execution requires a managed terminal capability",
                )
            }
            (ExternalAgentMode::Propose, _) => ExternalAgentActionDecision::RecordProposal,
            (ExternalAgentMode::Consult | ExternalAgentMode::Plan, _) => {
                ExternalAgentActionDecision::deny("terminal commands require managed mode")
            }
        }
    }

    /// MCP tool calls are delegated to the native MCP approval path only in
    /// managed mode with any MCP capability. In non-managed modes with an MCP
    /// capability the call is recorded as a proposal; with no capability it is
    /// denied.
    fn decide_mcp(&self) -> ExternalAgentActionDecision {
        if matches!(self.capabilities.mcp, McpCapability::None) {
            return ExternalAgentActionDecision::deny("no MCP capability granted");
        }
        if self.is_managed() {
            ExternalAgentActionDecision::DelegateMcp
        } else {
            ExternalAgentActionDecision::RecordProposal
        }
    }

    /// Network egress is enforced by the run sandbox's network policy, not by
    /// host-side execution, so Codewith never performs a network call on the
    /// runtime's behalf. When the run holds a network capability the intent is
    /// recorded as a proposal; otherwise it is denied. This keeps the
    /// runtime-side-effect invariant intact for network too.
    fn decide_network(&self) -> ExternalAgentActionDecision {
        match self.capabilities.network {
            NetworkCapability::Managed => ExternalAgentActionDecision::RecordProposal,
            NetworkCapability::None => {
                ExternalAgentActionDecision::deny("no network capability granted")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn guard(mode: ExternalAgentMode) -> ExternalAgentActionGuard {
        ExternalAgentActionGuard::new(mode, ExternalAgentCapabilities::for_mode(mode))
    }

    fn read() -> ExternalAgentActionRequest {
        ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("src/main.rs"),
        }
    }

    fn write() -> ExternalAgentActionRequest {
        ExternalAgentActionRequest::WriteFile {
            path: PathBuf::from("src/main.rs"),
            content: "fn main() {}".to_string(),
        }
    }

    fn command() -> ExternalAgentActionRequest {
        ExternalAgentActionRequest::RunCommand {
            command: vec!["cargo".to_string(), "test".to_string()],
            cwd: None,
        }
    }

    fn mcp() -> ExternalAgentActionRequest {
        ExternalAgentActionRequest::McpToolCall {
            server: "docs".to_string(),
            tool: "search".to_string(),
            arguments: json!({"q": "safety"}),
        }
    }

    fn network() -> ExternalAgentActionRequest {
        ExternalAgentActionRequest::NetworkAccess {
            target: "https://example.com".to_string(),
            purpose: None,
        }
    }

    fn other() -> ExternalAgentActionRequest {
        ExternalAgentActionRequest::Other {
            label: "teleport".to_string(),
            payload: json!({}),
        }
    }

    #[test]
    fn reads_are_host_performed_whenever_a_read_capability_exists() {
        assert_eq!(
            guard(ExternalAgentMode::Propose).decide(&read()),
            ExternalAgentActionDecision::PerformRead
        );
        assert_eq!(
            guard(ExternalAgentMode::Managed).decide(&read()),
            ExternalAgentActionDecision::PerformRead
        );
    }

    #[test]
    fn reads_without_a_filesystem_capability_are_denied() {
        for mode in [ExternalAgentMode::Consult, ExternalAgentMode::Plan] {
            assert!(matches!(
                guard(mode).decide(&read()),
                ExternalAgentActionDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn writes_promote_only_in_managed_mode() {
        assert_eq!(
            guard(ExternalAgentMode::Managed).decide(&write()),
            ExternalAgentActionDecision::PromoteWrite
        );
        assert_eq!(
            guard(ExternalAgentMode::Propose).decide(&write()),
            ExternalAgentActionDecision::RecordProposal
        );
        for mode in [ExternalAgentMode::Consult, ExternalAgentMode::Plan] {
            assert!(matches!(
                guard(mode).decide(&write()),
                ExternalAgentActionDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn commands_delegate_only_in_managed_mode() {
        assert_eq!(
            guard(ExternalAgentMode::Managed).decide(&command()),
            ExternalAgentActionDecision::DelegateCommand
        );
        assert_eq!(
            guard(ExternalAgentMode::Propose).decide(&command()),
            ExternalAgentActionDecision::RecordProposal
        );
        for mode in [ExternalAgentMode::Consult, ExternalAgentMode::Plan] {
            assert!(matches!(
                guard(mode).decide(&command()),
                ExternalAgentActionDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn mcp_delegates_only_in_managed_mode() {
        assert_eq!(
            guard(ExternalAgentMode::Managed).decide(&mcp()),
            ExternalAgentActionDecision::DelegateMcp
        );
        assert_eq!(
            guard(ExternalAgentMode::Propose).decide(&mcp()),
            ExternalAgentActionDecision::RecordProposal
        );
        // Consult/Plan have no MCP capability.
        for mode in [ExternalAgentMode::Consult, ExternalAgentMode::Plan] {
            assert!(matches!(
                guard(mode).decide(&mcp()),
                ExternalAgentActionDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn network_is_recorded_not_delegated_when_permitted() {
        assert_eq!(
            guard(ExternalAgentMode::Managed).decide(&network()),
            ExternalAgentActionDecision::RecordProposal
        );
        assert_eq!(
            guard(ExternalAgentMode::Propose).decide(&network()),
            ExternalAgentActionDecision::RecordProposal
        );
        assert!(matches!(
            guard(ExternalAgentMode::Plan).decide(&network()),
            ExternalAgentActionDecision::Deny { .. }
        ));
    }

    #[test]
    fn unknown_actions_are_denied() {
        assert!(matches!(
            guard(ExternalAgentMode::Managed).decide(&other()),
            ExternalAgentActionDecision::Deny { .. }
        ));
    }

    #[test]
    fn managed_delegation_requires_the_specific_capability_even_in_managed_mode() {
        // A managed run whose capabilities were narrowed must not silently
        // upgrade back to delegation.
        let downgraded = ExternalAgentCapabilities {
            filesystem: FileSystemCapability::ReadOnly,
            terminal: TerminalCapability::None,
            mcp: McpCapability::None,
            ..ExternalAgentCapabilities::for_mode(ExternalAgentMode::Managed)
        };
        let guard = ExternalAgentActionGuard::new(ExternalAgentMode::Managed, downgraded);
        assert!(matches!(
            guard.decide(&write()),
            ExternalAgentActionDecision::Deny { .. }
        ));
        assert!(matches!(
            guard.decide(&command()),
            ExternalAgentActionDecision::Deny { .. }
        ));
        assert!(matches!(
            guard.decide(&mcp()),
            ExternalAgentActionDecision::Deny { .. }
        ));
        // A read capability survives, so reads still work.
        assert_eq!(
            guard.decide(&read()),
            ExternalAgentActionDecision::PerformRead
        );
    }

    #[test]
    fn no_decision_ever_authorizes_a_runtime_side_effect() {
        let decisions = [
            ExternalAgentActionDecision::PerformRead,
            ExternalAgentActionDecision::RecordProposal,
            ExternalAgentActionDecision::deny("nope"),
            ExternalAgentActionDecision::DelegateCommand,
            ExternalAgentActionDecision::PromoteWrite,
            ExternalAgentActionDecision::DelegateMcp,
        ];
        for decision in decisions {
            assert!(
                !decision.authorizes_runtime_side_effect(),
                "{decision:?} must not authorize a runtime side effect"
            );
        }
    }

    #[test]
    fn every_decision_reachable_from_the_guard_upholds_the_invariant() {
        // Cross-check the invariant against decisions actually produced by the
        // guard across all modes and action kinds, not just hand-built ones.
        let actions = [read(), write(), command(), mcp(), network(), other()];
        for mode in [
            ExternalAgentMode::Consult,
            ExternalAgentMode::Plan,
            ExternalAgentMode::Propose,
            ExternalAgentMode::Managed,
        ] {
            let guard = guard(mode);
            for action in &actions {
                let decision = guard.decide(action);
                assert!(!decision.authorizes_runtime_side_effect());
                // Delegation must only ever appear in managed mode.
                if decision.is_managed_delegation() {
                    assert!(
                        guard.is_managed(),
                        "{decision:?} for {action:?} leaked outside managed mode"
                    );
                }
            }
        }
    }
}
