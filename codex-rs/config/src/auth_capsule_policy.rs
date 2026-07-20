//! AuthCapsule policy capability advertisement.
//!
//! `codewith debug auth-capsule-policy` emits the [`AuthCapsulePolicyCapabilities`]
//! document so the Infinity subscription lane can prove — before it ever launches
//! a subscription task — that this binary natively enforces the `infinity-agent`
//! AuthCapsule policy. The lane refuses to launch unless the document matches the
//! exact contract below.

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
///   tool planner in `codex-core`), so a task cannot touch the host filesystem,
///   spawn host shell subprocesses, or read/switch auth profiles.
/// - `protected_remote_tool_bridge` — under the policy the model toolset is
///   reduced by a single allowlist choke point (`INFINITY_AGENT_TOOL_ALLOWLIST`
///   in `codex-core`) to policy-safe tools with no direct host access; the binary
///   exposes no in-binary host tool once the policy is active, so any
///   host-affecting effect must be brokered externally through the Infinity
///   protected remote-tool bridge.
///
/// Never emit a value that is not backed by enforcement. The
/// `infinity_agent_policy_removes_host_tools_from_plan` and
/// `infinity_agent_allowlist_excludes_host_access` tests in `codex-core` verify
/// the `false` guarantees against real tool planning, and
/// `infinity_agent_capabilities_match_infinity_contract` (below) pins this
/// document to the exact Infinity contract, so it cannot drift into dishonesty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AuthCapsulePolicyCapabilities {
    pub schema_version: &'static str,
    pub native_policy_enforcement: bool,
    pub host_filesystem_tools: bool,
    pub host_shell_tools: bool,
    pub auth_profile_control: bool,
    pub protected_remote_tool_bridge: bool,
}

impl AuthCapsulePolicyCapabilities {
    /// The capabilities the binary enforces when `tools.policy = "infinity-agent"`
    /// is active. This is the exact document the Infinity lane requires.
    pub const fn infinity_agent() -> Self {
        Self {
            schema_version: AUTH_CAPSULE_POLICY_CAPABILITIES_SCHEMA_VERSION,
            native_policy_enforcement: true,
            host_filesystem_tools: false,
            host_shell_tools: false,
            auth_profile_control: false,
            protected_remote_tool_bridge: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infinity_agent_capabilities_match_infinity_contract() {
        let caps = AuthCapsulePolicyCapabilities::infinity_agent();
        let value = serde_json::to_value(caps).expect("serialize capabilities");
        assert_eq!(
            value,
            serde_json::json!({
                "schema_version": "codewith.auth-capsule-policy-capabilities/v1",
                "native_policy_enforcement": true,
                "host_filesystem_tools": false,
                "host_shell_tools": false,
                "auth_profile_control": false,
                "protected_remote_tool_bridge": true,
            })
        );
    }
}
