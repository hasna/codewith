use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use chrono::DateTime;
use chrono::Utc;
use codex_mcp::ToolInfo;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use ed25519_dalek::Signature;
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::de::DeserializeSeed;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

const POLICY_SCHEMA_VERSION: &str = "codewith.tool-policy/v1";
const POLICY_ENVELOPE_SCHEMA_VERSION: &str = "codewith.signed-tool-policy-envelope/v1";
const TRUST_KEY_SCHEMA_VERSION: &str = "codewith.trust-key/v1";
const BINDINGS_SCHEMA_VERSION: &str = "codewith.launch-bindings/v1";
const POLICY_AUDIENCE: &str = "infinity-auth-capsule";
const BINDINGS_FD: i32 = 3;
const POLICY_FD: i32 = 4;
const POLICY_SIGNATURE_CONTEXT: &[u8] = b"hasna.infinity.codewith-tool-policy-signature/v1\0";
const MAX_POLICY_BYTES: usize = 1024 * 1024;
const MAX_REQUIREMENTS_BYTES: usize = 1024 * 1024;
const MAX_KEY_BYTES: usize = 4096;
const MAX_BINDINGS_BYTES: usize = 16 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 256;
const INFINITY_AGENT_PUBLIC_TOOL_NAMES: &[&str] = &[
    "infinity_version_get",
    "infinity_capabilities_list",
    "infinity_doctor_run",
    "infinity_run_validate",
    "infinity_run_plan",
    "infinity_run_submit",
    "infinity_run_get",
    "infinity_runs_list",
    "infinity_run_wait",
    "infinity_run_events_read",
    "infinity_run_steer",
    "infinity_run_cancel",
    "infinity_run_retry",
    "infinity_checkpoint_request",
    "infinity_checkpoint_get",
    "infinity_checkpoint_list",
    "infinity_checkpoint_verify",
    "infinity_evidence_get",
    "infinity_evidence_list",
    "infinity_result_get",
    "infinity_approval_request",
    "infinity_approval_get",
    "infinity_approval_list",
    "infinity_promotion_get",
];
const INFINITY_AGENT_DENIED_CAPABILITIES: &[&str] = &[
    "apply-patch",
    "auth-profile-control",
    "background-agents",
    "browser-and-computer-use",
    "code-mode",
    "host-filesystem",
    "host-shell",
    "hosted-tools",
    "hooks-and-notify",
    "mcp-oauth-and-credentials",
    "multi-agent",
    "plugins-and-extensions",
    "skills-and-external-instructions",
    "tool-search-and-deferred-tools",
    "unified-exec",
    "usage-control",
    "view-image",
];

/// Derive the `codewith.auth-capsule-policy-capabilities/v1` capability document
/// from the SAME enforcement constants the binary applies for
/// `tools.policy = "infinity-agent"`.
///
/// SECURITY INVARIANT — probe DERIVES from enforcement. This is the single
/// source of truth for the capability advertisement emitted by
/// `codewith debug auth-capsule-policy`. It is computed from
/// [`INFINITY_AGENT_PUBLIC_TOOL_NAMES`] (the exact allowlist that
/// `validate_payload` requires every signed tool to be a member of) and
/// [`INFINITY_AGENT_DENIED_CAPABILITIES`] (the denied-capability set reported by
/// [`VerifiedToolPolicy::safety_attestation`]). The `config` crate owns only the
/// wire shape ([`codex_config::AuthCapsulePolicyCapabilities`]); it cannot own
/// these values because that would require depending on `core` (a dependency
/// cycle). Because the probe emits exactly this computed document, the probe
/// output cannot diverge from what the binary actually enforces: weaken the
/// allowlist or the denied set and this document changes with it. Equivalence is
/// pinned by `infinity_agent_auth_capsule_capabilities_match_enforcement`.
pub fn infinity_agent_auth_capsule_capabilities() -> codex_config::AuthCapsulePolicyCapabilities {
    codex_config::AuthCapsulePolicyCapabilities {
        schema_version: codex_config::AUTH_CAPSULE_POLICY_CAPABILITIES_SCHEMA_VERSION,
        // The verified tool policy engine exists in this binary and is applied
        // whenever `tools.policy = "infinity-agent"` is effective.
        native_policy_enforcement: true,
        // A tool family is EXPOSED (`true`) only if it is NOT in the enforced
        // denied-capability set. These are the `false` security guarantees.
        host_filesystem_tools: infinity_agent_capability_exposed("host-filesystem"),
        host_shell_tools: infinity_agent_capability_exposed("host-shell"),
        auth_profile_control: infinity_agent_capability_exposed("auth-profile-control"),
        // The protected-remote-tool-bridge guarantee holds iff every tool the
        // policy can ever admit is an Infinity bridge tool with no direct host
        // access.
        protected_remote_tool_bridge: infinity_agent_allowlist_is_pure_bridge(),
    }
}

/// Whether the named capability is exposed by the enforced policy, i.e. it is NOT
/// a member of the denied-capability set the binary enforces.
fn infinity_agent_capability_exposed(capability: &str) -> bool {
    !INFINITY_AGENT_DENIED_CAPABILITIES.contains(&capability)
}

/// Whether the enforced public-tool allowlist is exclusively Infinity bridge
/// tools (no direct host access surface).
fn infinity_agent_allowlist_is_pure_bridge() -> bool {
    !INFINITY_AGENT_PUBLIC_TOOL_NAMES.is_empty()
        && INFINITY_AGENT_PUBLIC_TOOL_NAMES
            .iter()
            .all(|name| name.starts_with("infinity_"))
}

static PROCESS_POLICY: OnceLock<Result<Arc<VerifiedToolPolicy>, String>> = OnceLock::new();

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum PolicyError {
    #[error("invalid Infinity Agent policy: {0}")]
    Invalid(String),
    #[error("Infinity Agent policy I/O failed: {0}")]
    Io(String),
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PolicyMode {
    DynamicCliOnly,
    McpOnly,
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum PolicySource {
    Dynamic,
    Mcp,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalToolName {
    namespace: Option<String>,
    name: String,
}

impl CanonicalToolName {
    fn into_tool_name(self) -> ToolName {
        ToolName::new(self.namespace, self.name)
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedPolicyEntry {
    source: PolicySource,
    source_id: String,
    raw_tool_name: String,
    canonical_tool_name: CanonicalToolName,
    input_schema_sha256: String,
    tool_description_sha256: String,
    namespace_description_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedPolicyPayload {
    schema_version: String,
    audience: String,
    capsule_id: String,
    principal_sha256: String,
    lane_id: String,
    launch_nonce: String,
    codewith_sha256: String,
    mode: PolicyMode,
    issued_at: DateTime<Utc>,
    not_before: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    entries: Vec<SignedPolicyEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedPolicyEnvelope {
    schema_version: String,
    key_id: String,
    payload_b64url: String,
    signature_b64url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustKeyRecord {
    schema_version: String,
    key_id: String,
    public_key_b64url: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct LaunchBindingsRecord {
    schema_version: String,
    capsule_id: String,
    principal_sha256: String,
    lane_id: String,
    launch_nonce: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ExpectedLaunchBindings {
    capsule_id: String,
    principal_sha256: String,
    lane_id: String,
    launch_nonce: String,
}

impl TryFrom<LaunchBindingsRecord> for ExpectedLaunchBindings {
    type Error = PolicyError;

    fn try_from(record: LaunchBindingsRecord) -> Result<Self, Self::Error> {
        if record.schema_version != BINDINGS_SCHEMA_VERSION {
            return Err(invalid("unsupported launch-bindings schema version"));
        }
        validate_identifier("capsule_id", &record.capsule_id)?;
        validate_sha256_claim("principal_sha256", &record.principal_sha256)?;
        validate_identifier("lane_id", &record.lane_id)?;
        validate_identifier("launch_nonce", &record.launch_nonce)?;
        Ok(Self {
            capsule_id: record.capsule_id,
            principal_sha256: record.principal_sha256,
            lane_id: record.lane_id,
            launch_nonce: record.launch_nonce,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct VerifiedPolicyEntry {
    source: PolicySource,
    source_id: String,
    raw_tool_name: String,
    input_schema_sha256: String,
    tool_description_sha256: String,
    namespace_description_sha256: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VerifiedToolPolicy {
    digest: String,
    mode: PolicyMode,
    expires_at: DateTime<Utc>,
    entries: BTreeMap<ToolName, VerifiedPolicyEntry>,
}

/// Machine-readable proof that this process loaded the fail-closed Infinity
/// Agent policy and reduced its effective runtime to the signed bridge surface.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfinityAgentSafetyAttestation {
    pub schema_version: &'static str,
    pub safe: bool,
    pub profile: &'static str,
    pub codewith_version: &'static str,
    pub binary_sha256: String,
    pub policy_sha256: String,
    pub effective_config_sha256: String,
    pub route_mode: &'static str,
    pub policy_expires_at: String,
    pub bridge_protection: &'static str,
    pub bridge_sources: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub denied_capabilities: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct EffectiveSafetyState {
    pub all_optional_features_disabled: bool,
    pub ephemeral_session: bool,
    pub named_auth_profile_absent: bool,
    pub external_instructions_disabled: bool,
    pub mcp_credentials_forbidden: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EffectiveSafetyConfiguration<'a> {
    pub profile: &'static str,
    pub route_mode: &'static str,
    pub binary_sha256: &'a str,
    pub policy_sha256: &'a str,
    pub bridge_sources: &'a [String],
    pub allowed_tools: &'a [String],
    pub denied_capabilities: &'a [&'static str],
    pub all_optional_features_disabled: bool,
    pub ephemeral_session: bool,
    pub named_auth_profile_absent: bool,
    pub external_instructions_disabled: bool,
    pub mcp_credentials_forbidden: bool,
}

impl VerifiedToolPolicy {
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    pub(crate) fn mode(&self) -> PolicyMode {
        self.mode
    }

    pub(crate) fn allowed_tool_names(&self) -> Vec<ToolName> {
        self.entries.keys().cloned().collect()
    }

    pub(crate) fn mcp_source_ids(&self) -> BTreeSet<String> {
        self.entries
            .values()
            .filter(|entry| entry.source == PolicySource::Mcp)
            .map(|entry| entry.source_id.clone())
            .collect()
    }

    pub(crate) fn mcp_raw_tool_names(&self, source_id: &str) -> BTreeSet<String> {
        self.entries
            .values()
            .filter(|entry| entry.source == PolicySource::Mcp && entry.source_id == source_id)
            .map(|entry| entry.raw_tool_name.clone())
            .collect()
    }

    pub(crate) fn safety_attestation(
        &self,
        state: EffectiveSafetyState,
    ) -> Result<InfinityAgentSafetyAttestation, PolicyError> {
        let binary_sha256 = current_executable_sha256()?;
        self.safety_attestation_with_binary_sha256(state, binary_sha256)
    }

    fn safety_attestation_with_binary_sha256(
        &self,
        state: EffectiveSafetyState,
        binary_sha256: String,
    ) -> Result<InfinityAgentSafetyAttestation, PolicyError> {
        self.ensure_active(Utc::now())?;
        if !state.all_optional_features_disabled
            || !state.ephemeral_session
            || !state.named_auth_profile_absent
            || !state.external_instructions_disabled
            || !state.mcp_credentials_forbidden
        {
            return Err(invalid(
                "the effective configuration does not preserve the Infinity Agent safety boundary",
            ));
        }

        let route_mode = match self.mode {
            PolicyMode::DynamicCliOnly => "dynamic-cli-only",
            PolicyMode::McpOnly => "mcp-only",
        };
        let bridge_sources = self
            .entries
            .values()
            .map(|entry| entry.source_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let allowed_tools = self
            .entries
            .keys()
            .map(|name| match &name.namespace {
                Some(namespace) => format!("{namespace}/{}", name.name),
                None => name.name.clone(),
            })
            .collect::<Vec<_>>();
        let denied_capabilities = INFINITY_AGENT_DENIED_CAPABILITIES.to_vec();
        let effective = EffectiveSafetyConfiguration {
            profile: "infinity-agent",
            route_mode,
            binary_sha256: &binary_sha256,
            policy_sha256: &self.digest,
            bridge_sources: &bridge_sources,
            allowed_tools: &allowed_tools,
            denied_capabilities: &denied_capabilities,
            all_optional_features_disabled: state.all_optional_features_disabled,
            ephemeral_session: state.ephemeral_session,
            named_auth_profile_absent: state.named_auth_profile_absent,
            external_instructions_disabled: state.external_instructions_disabled,
            mcp_credentials_forbidden: state.mcp_credentials_forbidden,
        };
        let effective_value = serde_json::to_value(effective).map_err(|error| {
            invalid(format!(
                "effective safety configuration serialization failed: {error}"
            ))
        })?;
        let effective_bytes =
            serde_json_canonicalizer::to_vec(&effective_value).map_err(|error| {
                invalid(format!(
                    "effective safety configuration canonicalization failed: {error}"
                ))
            })?;

        Ok(InfinityAgentSafetyAttestation {
            schema_version: "codewith.infinity-agent-safety-attestation/v1",
            safe: true,
            profile: "infinity-agent",
            codewith_version: env!("CARGO_PKG_VERSION"),
            binary_sha256,
            policy_sha256: self.digest.clone(),
            effective_config_sha256: sha256_claim(&effective_bytes),
            route_mode,
            policy_expires_at: self.expires_at.to_rfc3339(),
            bridge_protection: "signed-exact-manifest-and-dispatch-gate",
            bridge_sources,
            allowed_tools,
            denied_capabilities,
        })
    }

    pub(crate) fn ensure_active(&self, now: DateTime<Utc>) -> Result<(), PolicyError> {
        if now >= self.expires_at {
            return Err(invalid("the verified policy has expired"));
        }
        Ok(())
    }

    pub(crate) fn authorize_dynamic(&self, tool: &DynamicToolSpec) -> Result<(), PolicyError> {
        if tool.defer_loading {
            return Err(invalid(
                "deferred dynamic tools are forbidden by the Infinity Agent policy",
            ));
        }
        let name = ToolName::new(tool.namespace.clone(), tool.name.clone());
        let source_id = tool
            .namespace
            .as_deref()
            .ok_or_else(|| invalid("dynamic tools require a namespace"))?;
        self.authorize_candidate_identity(
            PolicySource::Dynamic,
            source_id,
            tool.name.as_str(),
            &name,
        )
    }

    pub(crate) fn authorize_mcp(&self, tool: &ToolInfo) -> Result<(), PolicyError> {
        self.authorize_candidate_identity(
            PolicySource::Mcp,
            &tool.server_name,
            tool.tool.name.as_ref(),
            &tool.canonical_tool_name(),
        )
    }

    pub(crate) fn validate_dynamic_manifest(
        &self,
        tools: &[DynamicToolSpec],
    ) -> Result<(), PolicyError> {
        if self.mode != PolicyMode::DynamicCliOnly {
            return Err(invalid(
                "dynamic tools are forbidden by the selected route mode",
            ));
        }
        let mut seen = BTreeSet::new();
        for tool in tools {
            self.authorize_dynamic(tool)?;
            if !seen.insert(ToolName::new(tool.namespace.clone(), tool.name.clone())) {
                return Err(invalid(
                    "the dynamic tool manifest contains a duplicate name",
                ));
            }
        }
        self.require_exact_names(&seen)
    }

    pub(crate) fn validate_mcp_manifest(&self, tools: &[ToolInfo]) -> Result<(), PolicyError> {
        if self.mode != PolicyMode::McpOnly {
            return Err(invalid(
                "MCP tools are forbidden by the selected route mode",
            ));
        }
        let mut seen = BTreeSet::new();
        for tool in tools {
            self.authorize_mcp(tool)?;
            if !seen.insert(tool.canonical_tool_name()) {
                return Err(invalid("the MCP tool manifest contains a duplicate name"));
            }
        }
        self.require_exact_names(&seen)
    }

    pub(crate) fn authorize_dispatch(
        &self,
        tool_name: &ToolName,
        now: DateTime<Utc>,
    ) -> Result<(), PolicyError> {
        self.ensure_active(now)?;
        if !self.entries.contains_key(tool_name) {
            return Err(invalid(
                "the requested tool is not in the verified allowlist",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_model_visible_manifest(
        &self,
        specs: &[ToolSpec],
    ) -> Result<Vec<ToolName>, PolicyError> {
        let mut seen = BTreeSet::new();
        for spec in specs {
            let ToolSpec::Namespace(namespace) = spec else {
                return Err(invalid(
                    "Infinity Agent model manifest contains a non-namespaced tool",
                ));
            };
            for tool in &namespace.tools {
                let ResponsesApiNamespaceTool::Function(tool) = tool;
                let name = ToolName::namespaced(namespace.name.clone(), tool.name.clone());
                if !seen.insert(name.clone()) {
                    return Err(invalid(
                        "Infinity Agent model manifest contains a duplicate tool name",
                    ));
                }
                let entry = self.entries.get(&name).ok_or_else(|| {
                    invalid("a model-visible tool is not in the verified allowlist")
                })?;
                let schema = serde_json::to_value(&tool.parameters).map_err(|error| {
                    invalid(format!(
                        "model-visible schema serialization failed: {error}"
                    ))
                })?;
                if entry.input_schema_sha256 != schema_sha256(&schema)?
                    || entry.tool_description_sha256 != sha256_claim(tool.description.as_bytes())
                    || entry.namespace_description_sha256
                        != sha256_claim(namespace.description.as_bytes())
                {
                    return Err(invalid(
                        "a model-visible tool definition does not match the verified policy",
                    ));
                }
            }
        }
        self.require_exact_names(&seen)?;
        Ok(seen.into_iter().collect())
    }

    fn authorize_candidate_identity(
        &self,
        source: PolicySource,
        source_id: &str,
        raw_tool_name: &str,
        tool_name: &ToolName,
    ) -> Result<(), PolicyError> {
        let entry = self
            .entries
            .get(tool_name)
            .ok_or_else(|| invalid("a runtime tool is not in the verified allowlist"))?;
        if entry.source != source
            || entry.source_id != source_id
            || entry.raw_tool_name != raw_tool_name
        {
            return Err(invalid(
                "a runtime tool origin does not match the verified policy",
            ));
        }
        Ok(())
    }

    fn require_exact_names(&self, actual: &BTreeSet<ToolName>) -> Result<(), PolicyError> {
        let expected = self.entries.keys().cloned().collect::<BTreeSet<_>>();
        if actual != &expected {
            return Err(invalid(
                "the runtime tool manifest is not the exact signed allowlist",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_dynamic_policy(tools: &[DynamicToolSpec]) -> Arc<VerifiedToolPolicy> {
    let mut entries = BTreeMap::new();
    for tool in tools {
        let namespace = tool
            .namespace
            .as_deref()
            .expect("test Infinity dynamic tool requires namespace");
        let model_tool = codex_tools::dynamic_tool_to_responses_api_tool(tool)
            .expect("test Infinity dynamic model tool");
        let schema = serde_json::to_value(&model_tool.parameters).expect("test model schema");
        entries.insert(
            ToolName::new(tool.namespace.clone(), tool.name.clone()),
            VerifiedPolicyEntry {
                source: PolicySource::Dynamic,
                source_id: namespace.to_string(),
                raw_tool_name: tool.name.clone(),
                input_schema_sha256: schema_sha256(&schema).expect("test schema digest"),
                tool_description_sha256: sha256_claim(model_tool.description.as_bytes()),
                namespace_description_sha256: sha256_claim(
                    codex_tools::default_namespace_description(namespace).as_bytes(),
                ),
            },
        );
    }
    Arc::new(VerifiedToolPolicy {
        digest: sha256_claim(b"test-infinity-agent-policy"),
        mode: PolicyMode::DynamicCliOnly,
        expires_at: Utc::now() + chrono::Duration::hours(1),
        entries,
    })
}

#[cfg(test)]
pub(crate) fn test_mcp_policy(source_id: &str, raw_tool_names: &[&str]) -> Arc<VerifiedToolPolicy> {
    let entries = raw_tool_names
        .iter()
        .map(|raw_tool_name| {
            (
                ToolName::new(Some(source_id.to_string()), (*raw_tool_name).to_string()),
                VerifiedPolicyEntry {
                    source: PolicySource::Mcp,
                    source_id: source_id.to_string(),
                    raw_tool_name: (*raw_tool_name).to_string(),
                    input_schema_sha256: sha256_claim(b"test-input-schema"),
                    tool_description_sha256: sha256_claim(b"test-tool-description"),
                    namespace_description_sha256: sha256_claim(b"test-namespace-description"),
                },
            )
        })
        .collect();
    Arc::new(VerifiedToolPolicy {
        digest: sha256_claim(b"test-infinity-agent-mcp-policy"),
        mode: PolicyMode::McpOnly,
        expires_at: Utc::now() + chrono::Duration::hours(1),
        entries,
    })
}

pub(crate) fn load_process_policy(
    trust_key_path: &Path,
) -> Result<Arc<VerifiedToolPolicy>, PolicyError> {
    PROCESS_POLICY
        .get_or_init(|| {
            load_process_policy_uncached(trust_key_path)
                .map(Arc::new)
                .map_err(|error| error.to_string())
        })
        .clone()
        .map_err(PolicyError::Invalid)
}

fn load_process_policy_uncached(trust_key_path: &Path) -> Result<VerifiedToolPolicy, PolicyError> {
    let key_bytes =
        read_secure_regular_file(trust_key_path, SecureFileKind::RootTrustKey, MAX_KEY_BYTES)?;
    let expected = read_launch_bindings_from_fd()?;
    let envelope_bytes = read_policy_envelope_from_fd()?;
    let codewith_sha256 = current_executable_sha256()?;
    verify_policy_material(
        &envelope_bytes,
        &key_bytes,
        &expected,
        &codewith_sha256,
        Utc::now(),
    )
}

fn verify_policy_material(
    envelope_bytes: &[u8],
    key_bytes: &[u8],
    expected: &ExpectedLaunchBindings,
    codewith_sha256: &str,
    now: DateTime<Utc>,
) -> Result<VerifiedToolPolicy, PolicyError> {
    let envelope: SignedPolicyEnvelope =
        parse_json_no_duplicates(envelope_bytes, "policy envelope")?;
    if envelope.schema_version != POLICY_ENVELOPE_SCHEMA_VERSION {
        return Err(invalid("unsupported policy-envelope schema version"));
    }
    validate_identifier("key_id", &envelope.key_id)?;
    let payload_bytes = decode_canonical_base64url("payload_b64url", &envelope.payload_b64url)?;
    let signature_bytes =
        decode_canonical_base64url("signature_b64url", &envelope.signature_b64url)?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| invalid("the Ed25519 signature must be exactly 64 bytes"))?;
    let (trusted_key_id, verifying_key) = parse_verifying_key(key_bytes)?;
    if envelope.key_id != trusted_key_id {
        return Err(invalid(
            "the policy envelope key ID is not the system trust-key ID",
        ));
    }

    let payload_value = parse_json_value_no_duplicates(&payload_bytes, "signed policy payload")?;
    let canonical_payload = serde_json_canonicalizer::to_vec(&payload_value)
        .map_err(|error| invalid(format!("policy JCS encoding failed: {error}")))?;
    if canonical_payload != payload_bytes {
        return Err(invalid(
            "the signed payload is not its exact RFC 8785/JCS encoding",
        ));
    }
    let signature_preimage = policy_signature_preimage(&payload_bytes);
    verifying_key
        .verify_strict(&signature_preimage, &signature)
        .map_err(|_| invalid("the detached Ed25519 signature is invalid"))?;
    let payload: SignedPolicyPayload = serde_json::from_value(payload_value)
        .map_err(|error| invalid(format!("the signed policy schema is invalid: {error}")))?;
    validate_payload(payload, expected, codewith_sha256, now, &payload_bytes)
}

fn validate_payload(
    payload: SignedPolicyPayload,
    expected: &ExpectedLaunchBindings,
    codewith_sha256: &str,
    now: DateTime<Utc>,
    payload_bytes: &[u8],
) -> Result<VerifiedToolPolicy, PolicyError> {
    if payload.schema_version != POLICY_SCHEMA_VERSION {
        return Err(invalid("unsupported policy schema version"));
    }
    if payload.audience != POLICY_AUDIENCE {
        return Err(invalid("the policy audience is invalid"));
    }
    if payload.capsule_id != expected.capsule_id
        || payload.principal_sha256 != expected.principal_sha256
        || payload.lane_id != expected.lane_id
        || payload.launch_nonce != expected.launch_nonce
    {
        return Err(invalid(
            "the policy launch bindings do not match the launcher channel",
        ));
    }
    validate_identifier("capsule_id", &payload.capsule_id)?;
    validate_sha256_claim("principal_sha256", &payload.principal_sha256)?;
    validate_identifier("lane_id", &payload.lane_id)?;
    validate_identifier("launch_nonce", &payload.launch_nonce)?;
    validate_sha256_claim("codewith_sha256", &payload.codewith_sha256)?;
    if payload.codewith_sha256 != codewith_sha256 {
        return Err(invalid(
            "the policy is bound to a different Codewith executable",
        ));
    }
    if payload.issued_at > payload.not_before
        || payload.not_before >= payload.expires_at
        || payload.issued_at > now
        || payload.not_before > now
        || payload.expires_at <= now
    {
        return Err(invalid("the policy time bounds are invalid or inactive"));
    }
    if payload.entries.is_empty() || payload.entries.len() > MAX_ENTRIES {
        return Err(invalid(
            "the policy must contain a bounded non-empty tool allowlist",
        ));
    }

    let mut entries = BTreeMap::new();
    for signed_entry in payload.entries {
        validate_identifier("source_id", &signed_entry.source_id)?;
        validate_identifier("raw_tool_name", &signed_entry.raw_tool_name)?;
        validate_sha256_claim("input_schema_sha256", &signed_entry.input_schema_sha256)?;
        validate_sha256_claim(
            "tool_description_sha256",
            &signed_entry.tool_description_sha256,
        )?;
        validate_sha256_claim(
            "namespace_description_sha256",
            &signed_entry.namespace_description_sha256,
        )?;
        validate_tool_name(&signed_entry.canonical_tool_name)?;
        if !INFINITY_AGENT_PUBLIC_TOOL_NAMES.contains(&signed_entry.raw_tool_name.as_str())
            || signed_entry.canonical_tool_name.name != signed_entry.raw_tool_name
        {
            return Err(invalid("the policy contains a non-agent public tool name"));
        }
        match (payload.mode, signed_entry.source) {
            (PolicyMode::DynamicCliOnly, PolicySource::Dynamic) => {
                if signed_entry.source_id != "infinity_cli"
                    || signed_entry.canonical_tool_name.namespace.as_deref() != Some("infinity_cli")
                {
                    return Err(invalid(
                        "a dynamic route must use the fixed infinity_cli source and namespace",
                    ));
                }
            }
            (PolicyMode::McpOnly, PolicySource::Mcp) => {
                if signed_entry.canonical_tool_name.namespace.is_none() {
                    return Err(invalid("an MCP route must use a callable namespace"));
                }
            }
            _ => {
                return Err(invalid(
                    "a tool source is incompatible with the selected route mode",
                ));
            }
        }
        let tool_name = signed_entry.canonical_tool_name.into_tool_name();
        let entry = VerifiedPolicyEntry {
            source: signed_entry.source,
            source_id: signed_entry.source_id,
            raw_tool_name: signed_entry.raw_tool_name,
            input_schema_sha256: signed_entry.input_schema_sha256,
            tool_description_sha256: signed_entry.tool_description_sha256,
            namespace_description_sha256: signed_entry.namespace_description_sha256,
        };
        if entries.insert(tool_name, entry).is_some() {
            return Err(invalid(
                "the policy contains a duplicate canonical tool name",
            ));
        }
    }

    if payload.mode == PolicyMode::McpOnly
        && entries
            .values()
            .map(|entry| entry.source_id.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            != 1
    {
        return Err(invalid(
            "an MCP-only policy must bind exactly one protected bridge source",
        ));
    }

    Ok(VerifiedToolPolicy {
        digest: sha256_claim(payload_bytes),
        mode: payload.mode,
        expires_at: payload.expires_at,
        entries,
    })
}

fn validate_tool_name(name: &CanonicalToolName) -> Result<(), PolicyError> {
    validate_identifier("canonical_tool_name.name", &name.name)?;
    if let Some(namespace) = &name.namespace {
        validate_identifier("canonical_tool_name.namespace", namespace)?;
    }
    Ok(())
}

fn validate_identifier(field: &str, value: &str) -> Result<(), PolicyError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(invalid(format!(
            "{field} is empty, oversized, or contains controls"
        )));
    }
    Ok(())
}

fn validate_sha256_claim(field: &str, value: &str) -> Result<(), PolicyError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid(format!("{field} is not a sha256 claim")));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(format!(
            "{field} is not lowercase canonical SHA-256"
        )));
    }
    Ok(())
}

fn schema_sha256(schema: &Value) -> Result<String, PolicyError> {
    let canonical = serde_json_canonicalizer::to_vec(schema)
        .map_err(|error| invalid(format!("schema JCS encoding failed: {error}")))?;
    Ok(sha256_claim(&canonical))
}

fn sha256_claim(bytes: &[u8]) -> String {
    format_sha256_digest(Sha256::digest(bytes))
}

fn format_sha256_digest(digest: impl AsRef<[u8]>) -> String {
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn decode_canonical_base64url(field: &str, value: &str) -> Result<Vec<u8>, PolicyError> {
    let decoded = BASE64_URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid(format!("{field} is not strict unpadded base64url")))?;
    if BASE64_URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(invalid(format!(
            "{field} is not canonical unpadded base64url"
        )));
    }
    Ok(decoded)
}

fn parse_verifying_key(bytes: &[u8]) -> Result<(String, VerifyingKey), PolicyError> {
    let record: TrustKeyRecord = parse_json_no_duplicates(bytes, "trust key")?;
    if record.schema_version != TRUST_KEY_SCHEMA_VERSION {
        return Err(invalid("unsupported trust-key schema version"));
    }
    validate_identifier("key_id", &record.key_id)?;
    let decoded = decode_canonical_base64url("public_key_b64url", &record.public_key_b64url)?;
    let key: [u8; 32] = decoded
        .try_into()
        .map_err(|_| invalid("the Ed25519 trust key must be exactly 32 bytes"))?;
    let key =
        VerifyingKey::from_bytes(&key).map_err(|_| invalid("the Ed25519 trust key is invalid"))?;
    Ok((record.key_id, key))
}

fn policy_signature_preimage(payload_bytes: &[u8]) -> Vec<u8> {
    let mut preimage = Vec::with_capacity(POLICY_SIGNATURE_CONTEXT.len() + payload_bytes.len());
    preimage.extend_from_slice(POLICY_SIGNATURE_CONTEXT);
    preimage.extend_from_slice(payload_bytes);
    preimage
}

#[cfg(unix)]
fn current_executable_sha256() -> Result<String, PolicyError> {
    #[cfg(target_os = "linux")]
    let mut file = File::open("/proc/self/exe").map_err(|error| {
        PolicyError::Io(format!("cannot open the running executable image: {error}"))
    })?;
    #[cfg(all(unix, not(target_os = "linux")))]
    let mut file = {
        let path = std::env::current_exe().map_err(|error| {
            PolicyError::Io(format!("cannot locate current executable: {error}"))
        })?;
        open_nofollow_regular(&path)?
    };
    validate_root_owned_nonwritable_file(&file, "running Codewith executable")?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| PolicyError::Io(format!("cannot hash current executable: {error}")))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format_sha256_digest(digest.finalize()))
}

#[cfg(not(unix))]
fn current_executable_sha256() -> Result<String, PolicyError> {
    Err(invalid(
        "Infinity Agent executable identity requires a Unix executable handle",
    ))
}

#[derive(Clone, Copy)]
enum SecureFileKind {
    RootTrustKey,
    RootRequirements,
    ServiceEnvelope,
}

pub(crate) fn read_secure_system_requirements(path: &Path) -> Result<Vec<u8>, PolicyError> {
    read_secure_regular_file(
        path,
        SecureFileKind::RootRequirements,
        MAX_REQUIREMENTS_BYTES,
    )
}

fn read_secure_regular_file(
    path: &Path,
    kind: SecureFileKind,
    limit: usize,
) -> Result<Vec<u8>, PolicyError> {
    if !path.is_absolute() {
        return Err(invalid("a security-sensitive file path is not absolute"));
    }
    let mut file = open_nofollow_regular(path)?;
    validate_secure_file_metadata(&file, kind)?;
    read_bounded(&mut file, limit)
}

#[cfg(unix)]
fn platform_security_path(path: &Path) -> std::borrow::Cow<'_, Path> {
    #[cfg(target_os = "macos")]
    {
        path.strip_prefix("/etc")
            .ok()
            .map(|suffix| std::borrow::Cow::Owned(Path::new("/private/etc").join(suffix)))
            .unwrap_or_else(|| std::borrow::Cow::Borrowed(path))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::borrow::Cow::Borrowed(path)
    }
}

fn open_nofollow_regular(path: &Path) -> Result<File, PolicyError> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::fd::FromRawFd;
        use std::os::unix::ffi::OsStrExt;

        if !path.is_absolute() {
            return Err(invalid("a security-sensitive file path is not absolute"));
        }
        let platform_path = platform_security_path(path);
        let path = platform_path.as_ref();
        let components = path
            .components()
            .filter_map(|component| match component {
                std::path::Component::RootDir => None,
                std::path::Component::Normal(value) => Some(Ok(value)),
                _ => Some(Err(invalid(
                    "a security-sensitive path contains a non-canonical component",
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if components.is_empty() {
            return Err(invalid("a security-sensitive path has no file component"));
        }

        let root_fd = unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY,
            )
        };
        if root_fd < 0 {
            return Err(PolicyError::Io(format!(
                "cannot open filesystem root: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut current = unsafe { File::from_raw_fd(root_fd) };
        validate_secure_directory(&current, Path::new("/"))?;

        for (index, component) in components.iter().enumerate() {
            let name = CString::new(component.as_bytes())
                .map_err(|_| invalid("a security-sensitive path component contains NUL"))?;
            let is_last = index + 1 == components.len();
            let flags = if is_last {
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW
            } else {
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY
            };
            let next_fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
            if next_fd < 0 {
                return Err(PolicyError::Io(format!(
                    "cannot securely open {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                )));
            }
            current = unsafe { File::from_raw_fd(next_fd) };
            if !is_last {
                validate_secure_directory(&current, path)?;
            }
        }
        let file = current;
        let metadata = file.metadata().map_err(|error| {
            PolicyError::Io(format!("cannot inspect {}: {error}", path.display()))
        })?;
        if !metadata.file_type().is_file() {
            return Err(invalid("a security-sensitive path is not a regular file"));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(invalid(
            "Infinity Agent policy files require Unix no-follow semantics",
        ))
    }
}

#[cfg(unix)]
fn validate_secure_directory(directory: &File, path: &Path) -> Result<(), PolicyError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = directory.metadata().map_err(|error| {
        PolicyError::Io(format!(
            "cannot inspect directory for {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(invalid(format!(
            "the directory chain for {} must be root-owned and not group/world writable",
            path.display()
        )));
    }
    Ok(())
}

fn validate_root_owned_nonwritable_file(file: &File, label: &str) -> Result<(), PolicyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file
            .metadata()
            .map_err(|error| PolicyError::Io(format!("cannot inspect {label}: {error}")))?;
        if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o222 != 0 {
            return Err(invalid(format!(
                "the {label} must be a root-owned non-writable regular file"
            )));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (file, label);
        Err(invalid("executable ownership validation requires Unix"))
    }
}

fn validate_secure_file_metadata(file: &File, kind: SecureFileKind) -> Result<(), PolicyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file
            .metadata()
            .map_err(|error| PolicyError::Io(format!("cannot inspect secure file: {error}")))?;
        if !metadata.is_file() {
            return Err(invalid("security material must be a regular file"));
        }
        let mode = metadata.mode() & 0o777;
        match kind {
            SecureFileKind::RootTrustKey | SecureFileKind::RootRequirements => {
                if metadata.uid() != 0 || mode & 0o222 != 0 {
                    return Err(invalid("the trust key must be root-owned and non-writable"));
                }
            }
            SecureFileKind::ServiceEnvelope => {
                if metadata.uid() != unsafe { libc::geteuid() } || mode != 0o400 {
                    return Err(invalid(
                        "the policy envelope must be service-owned mode 0400",
                    ));
                }
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (file, kind);
        Err(invalid("Infinity Agent file ownership checks require Unix"))
    }
}

fn read_bounded(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, PolicyError> {
    let max = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::new();
    reader
        .take(max)
        .read_to_end(&mut bytes)
        .map_err(|error| PolicyError::Io(format!("cannot read security material: {error}")))?;
    if bytes.len() > limit {
        return Err(invalid("security material exceeds its size limit"));
    }
    Ok(bytes)
}

fn read_launch_bindings_from_fd() -> Result<ExpectedLaunchBindings, PolicyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        let mut file = duplicate_and_close_reserved_fd(BINDINGS_FD, "launch-bindings")?;
        let file_type = file
            .metadata()
            .map_err(|error| {
                PolicyError::Io(format!(
                    "cannot inspect launch-bindings descriptor type: {error}"
                ))
            })?
            .file_type();
        if !file_type.is_file() && !file_type.is_fifo() {
            return Err(invalid(
                "the launch-bindings descriptor must be a regular file or pipe",
            ));
        }
        validate_read_only_fd(&file, "launch-bindings")?;
        let bytes = read_bounded(&mut file, MAX_BINDINGS_BYTES)?;
        parse_launch_bindings(&bytes)
    }
    #[cfg(not(unix))]
    {
        Err(invalid(
            "the launch-bindings channel requires Unix descriptors",
        ))
    }
}

fn read_policy_envelope_from_fd() -> Result<Vec<u8>, PolicyError> {
    #[cfg(unix)]
    {
        let mut file = duplicate_and_close_reserved_fd(POLICY_FD, "policy envelope")?;
        validate_read_only_fd(&file, "policy envelope")?;
        validate_secure_file_metadata(&file, SecureFileKind::ServiceEnvelope)?;
        read_bounded(&mut file, MAX_POLICY_BYTES)
    }
    #[cfg(not(unix))]
    {
        Err(invalid(
            "the policy-envelope channel requires Unix descriptors",
        ))
    }
}

#[cfg(unix)]
fn duplicate_and_close_reserved_fd(fd: i32, label: &str) -> Result<File, PolicyError> {
    use std::os::fd::FromRawFd;

    // Keep duplicates above the reserved launch range so duplicating FD 3 can
    // never accidentally populate a missing FD 4 and make the two channels
    // alias one another.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, POLICY_FD + 1) };
    if duplicate < 0 {
        return Err(PolicyError::Io(format!(
            "cannot duplicate {label} descriptor: {}",
            std::io::Error::last_os_error()
        )));
    }
    if unsafe { libc::close(fd) } != 0 {
        let close_error = std::io::Error::last_os_error();
        unsafe {
            libc::close(duplicate);
        }
        return Err(PolicyError::Io(format!(
            "cannot consume {label} descriptor: {close_error}"
        )));
    }
    // SAFETY: `fcntl(F_DUPFD_CLOEXEC)` returned a fresh, uniquely owned valid
    // descriptor and the original reserved descriptor has been consumed.
    Ok(unsafe { File::from_raw_fd(duplicate) })
}

#[cfg(unix)]
fn validate_read_only_fd(file: &File, label: &str) -> Result<(), PolicyError> {
    use std::os::fd::AsRawFd;

    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(PolicyError::Io(format!(
            "cannot inspect {label} descriptor flags: {}",
            std::io::Error::last_os_error()
        )));
    }
    if flags & libc::O_ACCMODE != libc::O_RDONLY {
        return Err(invalid(format!("the {label} descriptor is not read-only")));
    }
    Ok(())
}

fn parse_launch_bindings(bytes: &[u8]) -> Result<ExpectedLaunchBindings, PolicyError> {
    let record: LaunchBindingsRecord = parse_json_no_duplicates(bytes, "launch bindings")?;
    record.try_into()
}

fn parse_json_no_duplicates<T: DeserializeOwned>(
    bytes: &[u8],
    label: &str,
) -> Result<T, PolicyError> {
    let value = parse_json_value_no_duplicates(bytes, label)?;
    serde_json::from_value(value)
        .map_err(|error| invalid(format!("{label} has an invalid closed schema: {error}")))
}

fn parse_json_value_no_duplicates(bytes: &[u8], label: &str) -> Result<Value, PolicyError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = NoDuplicateValueSeed
        .deserialize(&mut deserializer)
        .map_err(|error| invalid(format!("{label} is invalid JSON: {error}")))?;
    deserializer
        .end()
        .map_err(|error| invalid(format!("{label} has trailing data: {error}")))?;
    Ok(value)
}

struct NoDuplicateValueSeed;

impl<'de> DeserializeSeed<'de> for NoDuplicateValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateValueVisitor)
    }
}

struct NoDuplicateValueVisitor;

impl<'de> Visitor<'de> for NoDuplicateValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        NoDuplicateValueSeed.deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(NoDuplicateValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate object key {key:?}"
                )));
            }
            let value = object.next_value_seed(NoDuplicateValueSeed)?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

fn invalid(message: impl Into<String>) -> PolicyError {
    PolicyError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_tools::ToolExecutor;
    use ed25519_dalek::Signer;
    use ed25519_dalek::SigningKey;
    use serde_json::json;

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7_u8; 32])
    }

    fn expected() -> ExpectedLaunchBindings {
        ExpectedLaunchBindings {
            capsule_id: "capsule-1".to_string(),
            principal_sha256: format!("sha256:{}", "1".repeat(64)),
            lane_id: "lane-1".to_string(),
            launch_nonce: "nonce-1".to_string(),
        }
    }

    fn codewith_digest() -> String {
        format!("sha256:{}", "2".repeat(64))
    }

    /// SECURITY CONTRACT — the `codewith debug auth-capsule-policy` probe output
    /// is DERIVED from the enforcement layer, never hand-copied. This asserts
    /// derivation equivalence: the emitted capability document equals the set
    /// independently recomputed from the enforcement constants
    /// (`INFINITY_AGENT_DENIED_CAPABILITIES` and `INFINITY_AGENT_PUBLIC_TOOL_NAMES`)
    /// that `VerifiedToolPolicy` actually applies. If the enforced denied set or
    /// allowlist changes, the probe output changes with it, so the probe can
    /// never advertise a guarantee the binary does not enforce.
    #[test]
    fn infinity_agent_auth_capsule_capabilities_match_enforcement() {
        let probe = infinity_agent_auth_capsule_capabilities();

        // Independently recompute the expected document straight from the
        // enforcement constants (NOT from a hand-copied JSON literal).
        let expected = codex_config::AuthCapsulePolicyCapabilities {
            schema_version: codex_config::AUTH_CAPSULE_POLICY_CAPABILITIES_SCHEMA_VERSION,
            native_policy_enforcement: true,
            host_filesystem_tools: !INFINITY_AGENT_DENIED_CAPABILITIES.contains(&"host-filesystem"),
            host_shell_tools: !INFINITY_AGENT_DENIED_CAPABILITIES.contains(&"host-shell"),
            auth_profile_control: !INFINITY_AGENT_DENIED_CAPABILITIES
                .contains(&"auth-profile-control"),
            protected_remote_tool_bridge: !INFINITY_AGENT_PUBLIC_TOOL_NAMES.is_empty()
                && INFINITY_AGENT_PUBLIC_TOOL_NAMES
                    .iter()
                    .all(|name| name.starts_with("infinity_")),
        };
        assert_eq!(
            probe, expected,
            "probe document must be derived from enforcement"
        );

        // The schema version string is contractually fixed for the Infinity lane.
        assert_eq!(
            probe.schema_version,
            "codewith.auth-capsule-policy-capabilities/v1"
        );

        // The three host-access guarantees must be `false` BECAUSE the enforced
        // denied-capability set contains the matching capability, and the bridge
        // guarantee must be `true` BECAUSE the enforced allowlist is host-free.
        assert!(INFINITY_AGENT_DENIED_CAPABILITIES.contains(&"host-filesystem"));
        assert!(INFINITY_AGENT_DENIED_CAPABILITIES.contains(&"host-shell"));
        assert!(INFINITY_AGENT_DENIED_CAPABILITIES.contains(&"auth-profile-control"));
        assert!(!probe.host_filesystem_tools);
        assert!(!probe.host_shell_tools);
        assert!(!probe.auth_profile_control);
        assert!(probe.protected_remote_tool_bridge);
        assert!(probe.native_policy_enforcement);

        // Bind the host-access `false` guarantees to the REAL admission gate, not
        // merely to the descriptive denied-capability set. `validate_payload`
        // rejects any signed tool whose raw name is not in
        // `INFINITY_AGENT_PUBLIC_TOOL_NAMES`, so that constant is the actual
        // boundary of what the binary can ever admit. It must contain NO
        // host-access tool and nothing but Infinity bridge tools; if a future
        // change widened the admission gate to a host tool, this fails.
        for host_tool in [
            "exec_command",
            "write_stdin",
            "shell_command",
            "unified_exec",
            "apply_patch",
            "view_image",
            "read_file",
            "write_file",
            "manage_auth_profiles",
            "get_usage",
            "tool_search",
            "read_mcp_resource",
        ] {
            assert!(
                !INFINITY_AGENT_PUBLIC_TOOL_NAMES.contains(&host_tool),
                "the admission allowlist must never contain host-access tool `{host_tool}`"
            );
        }
        assert!(
            INFINITY_AGENT_PUBLIC_TOOL_NAMES
                .iter()
                .all(|name| name.starts_with("infinity_")),
            "every admissible tool must be an Infinity bridge tool with no direct host access"
        );

        // Serialized shape must still match the wire contract the lane parses.
        let value = serde_json::to_value(probe).expect("serialize capabilities");
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

    fn schema() -> Value {
        json!({
            "additionalProperties": false,
            "properties": {"run_spec": {"type": "object"}},
            "required": ["run_spec"],
            "type": "object"
        })
    }

    fn dynamic_tool(name: &str) -> DynamicToolSpec {
        DynamicToolSpec {
            namespace: Some("infinity_cli".to_string()),
            name: name.to_string(),
            description: "submit".to_string(),
            input_schema: schema(),
            defer_loading: false,
        }
    }

    fn dynamic_model_spec(name: &str) -> ToolSpec {
        let tool = codex_tools::dynamic_tool_to_responses_api_tool(&dynamic_tool(name))
            .expect("dynamic model tool");
        ToolSpec::Namespace(codex_tools::ResponsesApiNamespace {
            name: "infinity_cli".to_string(),
            description: codex_tools::default_namespace_description("infinity_cli"),
            tools: vec![ResponsesApiNamespaceTool::Function(tool)],
        })
    }

    fn dynamic_entry_for_tool(tool: &DynamicToolSpec) -> Value {
        let namespace = tool.namespace.as_deref().expect("dynamic namespace");
        let model_tool =
            codex_tools::dynamic_tool_to_responses_api_tool(tool).expect("dynamic model tool");
        let model_schema =
            serde_json::to_value(&model_tool.parameters).expect("model schema value");
        let namespace_description = codex_tools::default_namespace_description(namespace);
        json!({
            "source": "dynamic",
            "source_id": namespace,
            "raw_tool_name": tool.name,
            "canonical_tool_name": {"namespace": namespace, "name": tool.name},
            "input_schema_sha256": schema_sha256(&model_schema).expect("schema digest"),
            "tool_description_sha256": sha256_claim(model_tool.description.as_bytes()),
            "namespace_description_sha256": sha256_claim(namespace_description.as_bytes())
        })
    }

    fn mcp_tool_info(
        server: &str,
        namespace: &str,
        raw_name: &str,
        callable_name: &str,
    ) -> ToolInfo {
        ToolInfo {
            server_name: server.to_string(),
            supports_parallel_tool_calls: false,
            server_origin: None,
            callable_name: callable_name.to_string(),
            callable_namespace: namespace.to_string(),
            namespace_description: Some(format!("Tools from {server}.")),
            tool: rmcp::model::Tool::new(
                raw_name.to_string(),
                "submit".to_string(),
                Arc::new(rmcp::model::object(schema())),
            ),
            connector_id: None,
            connector_name: None,
            plugin_display_names: Vec::new(),
        }
    }

    fn mcp_entry(server: &str, namespace: &str, name: &str) -> Value {
        let info = mcp_tool_info(server, namespace, name, name);
        let handler = crate::tools::handlers::McpHandler::new_infinity_agent_serial(info)
            .expect("MCP handler");
        let ToolSpec::Namespace(spec) = handler.spec() else {
            panic!("MCP handler must expose a namespace");
        };
        let ResponsesApiNamespaceTool::Function(tool) = &spec.tools[0];
        let schema = serde_json::to_value(&tool.parameters).expect("MCP model schema");
        json!({
            "source": "mcp",
            "source_id": server,
            "raw_tool_name": name,
            "canonical_tool_name": {"namespace": namespace, "name": name},
            "input_schema_sha256": schema_sha256(&schema).expect("schema digest"),
            "tool_description_sha256": sha256_claim(tool.description.as_bytes()),
            "namespace_description_sha256": sha256_claim(spec.description.as_bytes())
        })
    }

    fn entry(source: &str, source_id: &str, namespace: &str, name: &str) -> Value {
        let mut value = dynamic_entry_for_tool(&DynamicToolSpec {
            namespace: Some(namespace.to_string()),
            name: name.to_string(),
            description: "submit".to_string(),
            input_schema: schema(),
            defer_loading: false,
        });
        value["source"] = json!(source);
        value["source_id"] = json!(source_id);
        value
    }

    fn payload(entries: Vec<Value>, mode: &str) -> Value {
        json!({
            "schema_version": POLICY_SCHEMA_VERSION,
            "audience": POLICY_AUDIENCE,
            "capsule_id": "capsule-1",
            "principal_sha256": format!("sha256:{}", "1".repeat(64)),
            "lane_id": "lane-1",
            "launch_nonce": "nonce-1",
            "codewith_sha256": codewith_digest(),
            "mode": mode,
            "issued_at": "2026-07-10T00:00:00Z",
            "not_before": "2026-07-10T00:00:00Z",
            "expires_at": "2026-07-10T01:00:00Z",
            "entries": entries
        })
    }

    fn envelope(payload: &Value) -> Vec<u8> {
        let payload_bytes = serde_json_canonicalizer::to_vec(payload).expect("canonical payload");
        envelope_for_raw_payload(&payload_bytes)
    }

    fn envelope_for_raw_payload(payload_bytes: &[u8]) -> Vec<u8> {
        let key = signing_key();
        let signature = key.sign(&policy_signature_preimage(payload_bytes));
        serde_json::to_vec(&json!({
            "schema_version": POLICY_ENVELOPE_SCHEMA_VERSION,
            "key_id": "auth-key-1",
            "payload_b64url": BASE64_URL_SAFE_NO_PAD.encode(payload_bytes),
            "signature_b64url": BASE64_URL_SAFE_NO_PAD.encode(signature.to_bytes())
        }))
        .expect("serialize envelope")
    }

    fn key_bytes() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": TRUST_KEY_SCHEMA_VERSION,
            "key_id": "auth-key-1",
            "public_key_b64url": BASE64_URL_SAFE_NO_PAD.encode(signing_key().verifying_key().to_bytes())
        }))
        .expect("serialize trust key")
    }

    fn now() -> DateTime<Utc> {
        "2026-07-10T00:30:00Z".parse().expect("timestamp")
    }

    #[test]
    fn infinity_agent_policy_verifies_and_authorizes_exact_dynamic_schema() {
        let bytes = envelope(&payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        ));
        let policy =
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .expect("valid policy");
        let dynamic = dynamic_tool("infinity_run_submit");
        assert_eq!(policy.validate_dynamic_manifest(&[dynamic]), Ok(()));
        assert_eq!(
            policy.validate_model_visible_manifest(&[dynamic_model_spec("infinity_run_submit")]),
            Ok(vec![ToolName::namespaced(
                "infinity_cli",
                "infinity_run_submit"
            )])
        );
        assert_eq!(policy.mode(), PolicyMode::DynamicCliOnly);
    }

    #[test]
    fn infinity_agent_policy_binds_large_schema_after_model_normalization() {
        let properties = (0..96)
            .map(|index| {
                (
                    format!("field_{index}"),
                    json!({
                        "type": "string",
                        "description": "bounded field description ".repeat(12)
                    }),
                )
            })
            .collect::<Map<String, Value>>();
        let tool = DynamicToolSpec {
            namespace: Some("infinity_cli".to_string()),
            name: "infinity_run_submit".to_string(),
            description: "submit".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "additionalProperties": false
            }),
            defer_loading: false,
        };
        let signed_entry = dynamic_entry_for_tool(&tool);
        assert_ne!(
            signed_entry["input_schema_sha256"],
            json!(schema_sha256(&tool.input_schema).expect("raw schema digest")),
            "fixture must exercise model schema normalization rather than raw hashing"
        );
        let bytes = envelope(&payload(vec![signed_entry], "dynamic-cli-only"));
        let policy =
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .expect("valid large-schema policy");
        assert_eq!(
            policy.validate_dynamic_manifest(std::slice::from_ref(&tool)),
            Ok(())
        );

        let model_tool =
            codex_tools::dynamic_tool_to_responses_api_tool(&tool).expect("model tool");
        let spec = ToolSpec::Namespace(codex_tools::ResponsesApiNamespace {
            name: "infinity_cli".to_string(),
            description: codex_tools::default_namespace_description("infinity_cli"),
            tools: vec![ResponsesApiNamespaceTool::Function(model_tool)],
        });
        assert!(policy.validate_model_visible_manifest(&[spec]).is_ok());

        let mut tampered = tool;
        tampered.description = "untrusted prompt injection".to_string();
        let tampered_model = codex_tools::dynamic_tool_to_responses_api_tool(&tampered)
            .expect("tampered model tool");
        let tampered_spec = ToolSpec::Namespace(codex_tools::ResponsesApiNamespace {
            name: "infinity_cli".to_string(),
            description: codex_tools::default_namespace_description("infinity_cli"),
            tools: vec![ResponsesApiNamespaceTool::Function(tampered_model)],
        });
        assert!(
            policy
                .validate_model_visible_manifest(&[tampered_spec])
                .is_err()
        );
    }

    #[test]
    fn infinity_agent_policy_rejects_outer_duplicate_keys_before_extraction() {
        let error = parse_json_no_duplicates::<SignedPolicyEnvelope>(
            br#"{"schema_version":"codewith.signed-tool-policy-envelope/v1","key_id":"auth-key-1","payload_b64url":"AA","payload_b64url":"AQ","signature_b64url":"AA"}"#,
            "policy envelope",
        )
        .expect_err("duplicate must fail");
        assert!(error.to_string().contains("duplicate object key"));
    }

    #[test]
    fn infinity_agent_policy_binds_key_id_base64url_and_signature_context() {
        let policy_payload = payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        );
        let valid = envelope(&policy_payload);

        let mut wrong_key: Value = serde_json::from_slice(&valid).expect("envelope value");
        wrong_key["key_id"] = json!("other-key");
        let error = verify_policy_material(
            &serde_json::to_vec(&wrong_key).expect("wrong key envelope"),
            &key_bytes(),
            &expected(),
            &codewith_digest(),
            now(),
        )
        .expect_err("outer key ID must match the trust record");
        assert!(error.to_string().contains("key ID"));

        let mut padded: Value = serde_json::from_slice(&valid).expect("envelope value");
        let encoded = padded["payload_b64url"]
            .as_str()
            .expect("payload string")
            .to_string();
        padded["payload_b64url"] = json!(format!("{encoded}="));
        assert!(
            verify_policy_material(
                &serde_json::to_vec(&padded).expect("padded envelope"),
                &key_bytes(),
                &expected(),
                &codewith_digest(),
                now(),
            )
            .is_err()
        );

        let payload_bytes =
            serde_json_canonicalizer::to_vec(&policy_payload).expect("canonical payload");
        let wrong_signature = signing_key().sign(&payload_bytes);
        let wrong_context = serde_json::to_vec(&json!({
            "schema_version": POLICY_ENVELOPE_SCHEMA_VERSION,
            "key_id": "auth-key-1",
            "payload_b64url": BASE64_URL_SAFE_NO_PAD.encode(&payload_bytes),
            "signature_b64url": BASE64_URL_SAFE_NO_PAD.encode(wrong_signature.to_bytes())
        }))
        .expect("wrong context envelope");
        assert!(
            verify_policy_material(
                &wrong_context,
                &key_bytes(),
                &expected(),
                &codewith_digest(),
                now(),
            )
            .is_err()
        );
    }

    #[test]
    fn infinity_agent_policy_rejects_duplicate_payload_keys() {
        let raw = br#"{"audience":"infinity-auth-capsule","audience":"other"}"#;
        let bytes = envelope_for_raw_payload(raw);
        let error =
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .expect_err("duplicate payload must fail");
        assert!(error.to_string().contains("duplicate object key"));
    }

    #[test]
    fn infinity_agent_policy_rejects_noncanonical_signed_payload() {
        let value = payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        );
        let raw = serde_json::to_vec_pretty(&value).expect("pretty payload");
        let error = verify_policy_material(
            &envelope_for_raw_payload(&raw),
            &key_bytes(),
            &expected(),
            &codewith_digest(),
            now(),
        )
        .expect_err("noncanonical payload must fail");
        assert!(error.to_string().contains("RFC 8785/JCS"));
    }

    #[test]
    fn infinity_agent_policy_rejects_tamper_and_wrong_binding() {
        let mut bytes = envelope(&payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        ));
        let position = bytes
            .iter()
            .position(|byte| *byte == b'A')
            .expect("base64 byte");
        bytes[position] = b'B';
        assert!(
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .is_err()
        );

        let mut wrong = expected();
        wrong.lane_id = "lane-2".to_string();
        let clean = envelope(&payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        ));
        assert!(
            verify_policy_material(&clean, &key_bytes(), &wrong, &codewith_digest(), now())
                .is_err()
        );
    }

    #[test]
    fn infinity_agent_policy_rejects_nonce_replay_for_a_new_launch() {
        let bytes = envelope(&payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        ));
        let mut second_launch = expected();
        second_launch.launch_nonce = "nonce-2".to_string();
        let error = verify_policy_material(
            &bytes,
            &key_bytes(),
            &second_launch,
            &codewith_digest(),
            now(),
        )
        .expect_err("old envelope must not bind to a fresh launch");
        assert!(error.to_string().contains("launch bindings"));
    }

    #[test]
    fn infinity_agent_policy_rejects_expired_duplicate_and_unknown_data() {
        let signed_entry = entry(
            "dynamic",
            "infinity_cli",
            "infinity_cli",
            "infinity_run_submit",
        );
        let duplicate = envelope(&payload(
            vec![signed_entry.clone(), signed_entry],
            "dynamic-cli-only",
        ));
        assert!(
            verify_policy_material(
                &duplicate,
                &key_bytes(),
                &expected(),
                &codewith_digest(),
                now()
            )
            .is_err()
        );

        let mut expired_payload = payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        );
        expired_payload["expires_at"] = json!("2026-07-10T00:15:00Z");
        assert!(
            verify_policy_material(
                &envelope(&expired_payload),
                &key_bytes(),
                &expected(),
                &codewith_digest(),
                now()
            )
            .is_err()
        );

        let mut unknown = payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        );
        unknown["extra"] = json!(true);
        assert!(
            verify_policy_material(
                &envelope(&unknown),
                &key_bytes(),
                &expected(),
                &codewith_digest(),
                now()
            )
            .is_err()
        );
    }

    #[test]
    fn infinity_agent_policy_rejects_non_agent_and_core_tool_names() {
        for name in [
            "infinity_operation_resolve",
            "infinity_promotion_propose",
            "infinity_promotion_cancel",
            "infinity_run_discard",
            "infinity_approval_decide",
            "infinity_cleanup_apply",
            "infinity_restore_apply",
            "exec_command",
            "manage_auth_profiles",
        ] {
            let bytes = envelope(&payload(
                vec![entry("dynamic", "infinity_cli", "infinity_cli", name)],
                "dynamic-cli-only",
            ));
            let error = verify_policy_material(
                &bytes,
                &key_bytes(),
                &expected(),
                &codewith_digest(),
                now(),
            )
            .expect_err("forbidden tool must fail");
            assert!(error.to_string().contains("non-agent public tool"));
        }
    }

    #[test]
    fn infinity_agent_policy_rechecks_expiry_before_each_turn() {
        let bytes = envelope(&payload(
            vec![entry(
                "dynamic",
                "infinity_cli",
                "infinity_cli",
                "infinity_run_submit",
            )],
            "dynamic-cli-only",
        ));
        let policy =
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .expect("valid policy");

        assert!(
            policy
                .ensure_active("2026-07-10T01:00:00Z".parse().expect("timestamp"))
                .is_err()
        );
    }

    #[test]
    fn infinity_agent_policy_binds_exact_mcp_raw_name_and_model_manifest() {
        let bytes = envelope(&payload(
            vec![mcp_entry("infinity", "mcp__infinity", "infinity_run_get")],
            "mcp-only",
        ));
        let policy =
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .expect("valid MCP policy");
        let info = mcp_tool_info(
            "infinity",
            "mcp__infinity",
            "infinity_run_get",
            "infinity_run_get",
        );
        assert_eq!(policy.validate_mcp_manifest(&[info.clone()]), Ok(()));
        let handler = crate::tools::handlers::McpHandler::new_infinity_agent_serial(info)
            .expect("MCP handler");
        assert_eq!(
            policy.validate_model_visible_manifest(&[handler.spec()]),
            Ok(vec![ToolName::namespaced(
                "mcp__infinity",
                "infinity_run_get"
            )])
        );

        let normalized_collision = mcp_tool_info(
            "infinity",
            "mcp__infinity",
            "infinity-run-get",
            "infinity_run_get",
        );
        assert!(
            policy
                .validate_mcp_manifest(&[normalized_collision])
                .is_err()
        );
    }

    #[test]
    fn infinity_agent_policy_rejects_multiple_mcp_bridge_sources() {
        let bytes = envelope(&payload(
            vec![
                mcp_entry("infinity-a", "mcp__infinity_a", "infinity_run_get"),
                mcp_entry("infinity-b", "mcp__infinity_b", "infinity_result_get"),
            ],
            "mcp-only",
        ));
        let error =
            verify_policy_material(&bytes, &key_bytes(), &expected(), &codewith_digest(), now())
                .expect_err("two protected bridge sources must fail closed");
        assert!(error.to_string().contains("exactly one protected bridge"));
    }

    #[test]
    fn infinity_agent_policy_launch_bindings_are_closed_and_duplicate_rejecting() {
        let valid = format!(
            r#"{{"schema_version":"{BINDINGS_SCHEMA_VERSION}","capsule_id":"capsule-1","principal_sha256":"sha256:{}","lane_id":"lane-1","launch_nonce":"nonce-1"}}"#,
            "1".repeat(64)
        );
        assert_eq!(parse_launch_bindings(valid.as_bytes()), Ok(expected()));
        assert!(parse_launch_bindings(format!("{valid}\n{{}}").as_bytes()).is_err());
        assert!(
            parse_launch_bindings(
                valid
                    .replace(
                        "\"capsule_id\":\"capsule-1\"",
                        "\"capsule_id\":\"capsule-1\",\"capsule_id\":\"capsule-2\""
                    )
                    .as_bytes()
            )
            .is_err()
        );
    }

    #[test]
    fn infinity_agent_safety_attestation_is_machine_readable_and_digest_bound() {
        let policy = test_dynamic_policy(&[dynamic_tool("infinity_run_submit")]);
        let attestation = policy
            .safety_attestation_with_binary_sha256(
                EffectiveSafetyState {
                    all_optional_features_disabled: true,
                    ephemeral_session: true,
                    named_auth_profile_absent: true,
                    external_instructions_disabled: true,
                    mcp_credentials_forbidden: true,
                },
                codewith_digest(),
            )
            .expect("safe attestation");
        let value = serde_json::to_value(&attestation).expect("JSON attestation");

        assert!(attestation.safe);
        assert_eq!(attestation.profile, "infinity-agent");
        assert_eq!(attestation.route_mode, "dynamic-cli-only");
        assert_eq!(
            attestation.allowed_tools,
            vec!["infinity_cli/infinity_run_submit"]
        );
        assert!(attestation.binary_sha256.starts_with("sha256:"));
        assert!(attestation.policy_sha256.starts_with("sha256:"));
        assert!(attestation.effective_config_sha256.starts_with("sha256:"));
        assert_eq!(
            value.get("bridgeProtection").and_then(Value::as_str),
            Some("signed-exact-manifest-and-dispatch-gate")
        );
        assert!(attestation.denied_capabilities.contains(&"host-shell"));
        assert!(attestation.denied_capabilities.contains(&"host-filesystem"));
        assert!(attestation.denied_capabilities.contains(&"unified-exec"));
    }

    #[test]
    fn infinity_agent_safety_attestation_fails_on_effective_config_drift() {
        let policy = test_dynamic_policy(&[dynamic_tool("infinity_run_submit")]);

        let error = policy
            .safety_attestation_with_binary_sha256(
                EffectiveSafetyState {
                    all_optional_features_disabled: false,
                    ephemeral_session: true,
                    named_auth_profile_absent: true,
                    external_instructions_disabled: true,
                    mcp_credentials_forbidden: true,
                },
                codewith_digest(),
            )
            .expect_err("feature drift must fail closed");
        assert!(error.to_string().contains("does not preserve"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn infinity_agent_policy_uses_the_platform_canonical_private_etc_chain() {
        assert_eq!(
            platform_security_path(Path::new("/etc/codewith/requirements.toml")).as_ref(),
            Path::new("/private/etc/codewith/requirements.toml")
        );
        assert_eq!(
            platform_security_path(Path::new("/opt/codewith/bin/codewith")).as_ref(),
            Path::new("/opt/codewith/bin/codewith")
        );
    }
}
