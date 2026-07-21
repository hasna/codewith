//! AuthCapsule policy capability advertisement.
//!
//! `codewith debug auth-capsule-policy` emits the [`AuthCapsulePolicyCapabilities`]
//! document so the Infinity subscription lane can prove — before it ever launches
//! a subscription task — that this binary natively enforces the `infinity-agent`
//! AuthCapsule policy. The lane refuses to launch unless the document matches the
//! exact contract below.
//!
//! SECURITY INVARIANT — probe DERIVES from enforcement.
//! The capability document is NOT a hand-maintained constant. The `config` crate
//! only owns the wire *shape* ([`AuthCapsulePolicyCapabilities`] +
//! [`AUTH_CAPSULE_POLICY_CAPABILITIES_SCHEMA_VERSION`]). The single source of
//! truth for the *values* is the fail-closed enforcement layer
//! (`codex_core::tools::policy::VerifiedToolPolicy` and its
//! `INFINITY_AGENT_PUBLIC_TOOL_NAMES` / `INFINITY_AGENT_DENIED_CAPABILITIES`
//! constants). `codex_core::tools::policy::infinity_agent_auth_capsule_capabilities`
//! computes this document from those constants, and the
//! `codewith debug auth-capsule-policy` probe emits exactly that computed
//! document. Because the `config` crate cannot depend on `core` (that would form
//! a dependency cycle), the derivation lives in `core` where both the probe and
//! the enforcement can consume it. This guarantees the probe output cannot
//! diverge from what the binary actually enforces: change the enforced allowlist
//! or denied-capability set and the probe changes with it. The derivation
//! equivalence is pinned by
//! `infinity_agent_auth_capsule_capabilities_match_enforcement` in `codex-core`.

use serde::Serialize;

/// Schema version required by the Infinity probe
/// (`probeNativeInfinityAgentPolicy`). Emitting any other value makes the lane
/// reject the binary.
pub const AUTH_CAPSULE_POLICY_CAPABILITIES_SCHEMA_VERSION: &str =
    "codewith.auth-capsule-policy-capabilities/v1";

/// Capability advertisement for the native AuthCapsule (`infinity-agent`) policy.
///
/// Every field here is a hard claim that MUST correspond to behavior the binary
/// actually enforces:
/// - `native_policy_enforcement` — the policy engine exists and is applied.
/// - `host_filesystem_tools` / `host_shell_tools` / `auth_profile_control` — these
///   are `false` **security guarantees**: under `tools.policy = "infinity-agent"`
///   these tool families are removed from the model toolset entirely (see the
///   verified tool policy in `codex-core`), so a task cannot touch the host
///   filesystem, spawn host shell subprocesses, or read/switch auth profiles.
/// - `protected_remote_tool_bridge` — under the policy the model toolset is
///   reduced to the signed Infinity bridge allowlist with no direct host access;
///   the binary exposes no in-binary host tool once the policy is active, so any
///   host-affecting effect must be brokered externally through the Infinity
///   protected remote-tool bridge.
///
/// This struct is the wire shape only. Never construct it with hand-copied
/// booleans: the authoritative values are computed by
/// `codex_core::tools::policy::infinity_agent_auth_capsule_capabilities` from the
/// same constants the enforcement layer uses. See the module-level SECURITY
/// INVARIANT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AuthCapsulePolicyCapabilities {
    pub schema_version: &'static str,
    pub native_policy_enforcement: bool,
    pub host_filesystem_tools: bool,
    pub host_shell_tools: bool,
    pub auth_profile_control: bool,
    pub protected_remote_tool_bridge: bool,
}
