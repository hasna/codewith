//! Safety guard mapping external-agent action requests onto Codewith's
//! sandbox and approval model.
//!
//! External coding agents such as Cursor are treated as *untrusted* runtimes:
//! they may *propose* reads, writes, shell commands, MCP calls, and network
//! access, but Codewith owns the permission boundary. This module is the single
//! enforcement authority that decides, for one run, how each proposed action is
//! handled.
//!
//! The guard is intentionally pure and free of I/O so it can be exhaustively
//! tested and reused by every runtime adapter (ACP, SDK, cloud). It never
//! authorizes a runtime-side mutation or execution. The strongest invariant it
//! upholds is:
//!
//! > The only action a runtime is ever allowed to perform directly is a
//! > sandbox-confined, read-only filesystem read. Writes become patch proposals
//! > that Codewith promotes; shell/MCP/network actions are denied unless and
//! > until they are delegated to a Codewith native tool.
//!
//! This matches the first safe integration phase (read-only / plan / proposal
//! mode). Managed shell/MCP/network delegation is a later phase; when it lands
//! it must route through the native `ToolOrchestrator`/exec/apply_patch paths,
//! not through the runtime.

use std::path::Path;
use std::path::PathBuf;

use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::permissions::ReadDenyMatcher;

use crate::ExternalAgentActionRequest;
use crate::ExternalAgentCapabilities;
use crate::ExternalAgentMode;
use crate::FileSystemCapability;

/// Default cap on a single guarded read so a runtime cannot exfiltrate an
/// unbounded file through the read channel. Callers perform the actual read and
/// enforce this limit.
pub const DEFAULT_MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Stable machine-readable reason a guarded action was refused.
///
/// The code is surfaced alongside a human-readable message so a runtime (and a
/// transcript audit) can tell *why* an action was refused without string
/// matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAgentGuardDenial {
    /// The run's capabilities do not grant the requested class of action.
    CapabilityDisabled,
    /// The read target resolves outside the sandbox's readable roots.
    OutsideSandbox,
    /// The read target is explicitly deny-listed by the sandbox policy.
    ReadDenied,
    /// A proposed edit resolves outside the workspace and cannot be staged.
    PathEscape,
    /// Shell execution must be delegated to Codewith's native exec runtime.
    CommandNotDelegated,
    /// MCP tool calls must be delegated to Codewith's native MCP approval path.
    McpNotDelegated,
    /// Network access must be delegated to Codewith's native network approval.
    NetworkNotDelegated,
    /// The sandbox is disabled or too broad to safely host an untrusted agent.
    SandboxUnavailable,
    /// The action is unrecognized; the guard fails closed.
    Unsupported,
}

impl ExternalAgentGuardDenial {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CapabilityDisabled => "capability-disabled",
            Self::OutsideSandbox => "outside-sandbox",
            Self::ReadDenied => "read-denied",
            Self::PathEscape => "path-escape",
            Self::CommandNotDelegated => "command-not-delegated",
            Self::McpNotDelegated => "mcp-not-delegated",
            Self::NetworkNotDelegated => "network-not-delegated",
            Self::SandboxUnavailable => "sandbox-unavailable",
            Self::Unsupported => "unsupported",
        }
    }
}

/// A workspace edit proposed by a runtime that Codewith must stage and promote
/// (never a direct runtime write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedWorkspaceEdit {
    pub path: PathBuf,
    pub content: String,
}

/// How Codewith handles a single action an untrusted runtime asked it to run.
///
/// Every variant keeps execution authority with Codewith. There is deliberately
/// no variant that tells the host to let the runtime write, exec, call MCP, or
/// open the network on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAgentActionDecision {
    /// Codewith performs a confined, read-only filesystem read through its own
    /// native path and returns the content to the runtime.
    PerformRead { path: PathBuf },
    /// The runtime asked to mutate the workspace. Codewith records the request
    /// as a patch proposal for review/promotion and refuses to let the runtime
    /// mutate anything itself. `reason` is surfaced back to the runtime.
    RecordProposal {
        edit: ProposedWorkspaceEdit,
        reason: String,
    },
    /// Codewith refuses the action outright and surfaces `reason` to the runtime.
    Deny {
        code: ExternalAgentGuardDenial,
        reason: String,
    },
}

impl ExternalAgentActionDecision {
    fn deny(code: ExternalAgentGuardDenial, reason: impl Into<String>) -> Self {
        Self::Deny {
            code,
            reason: reason.into(),
        }
    }

    /// Returns the denial reason, if this decision refuses the action. Both
    /// `Deny` and `RecordProposal` refuse the *runtime-side* action, so both
    /// return a reason the host surfaces back to the runtime.
    pub fn refusal_reason(&self) -> Option<&str> {
        match self {
            Self::PerformRead { .. } => None,
            Self::RecordProposal { reason, .. } | Self::Deny { reason, .. } => Some(reason),
        }
    }

    /// Whether the decision allows the runtime to directly perform a mutating or
    /// executing action. This must always be `false`: it exists so tests can
    /// assert the guard never authorizes a runtime-side side effect.
    pub fn authorizes_runtime_side_effect(&self) -> bool {
        false
    }
}

/// Resolved sandbox view for one run.
#[derive(Debug, Clone)]
enum GuardSandbox {
    /// Reads are checked against an explicit, restricted policy.
    Restricted {
        file_system: FileSystemSandboxPolicy,
    },
    /// No safe read policy is available (disabled or full-disk); deny reads.
    Unavailable { reason: String },
}

/// Per-run safety guard. Cheap to clone; holds no I/O handles.
#[derive(Debug, Clone)]
pub struct ExternalAgentActionGuard {
    mode: ExternalAgentMode,
    capabilities: ExternalAgentCapabilities,
    sandbox: GuardSandbox,
    #[allow(dead_code)]
    network: NetworkSandboxPolicy,
    cwd: PathBuf,
    max_read_bytes: u64,
}

impl ExternalAgentActionGuard {
    /// Build a guard from an explicit filesystem/network policy view.
    ///
    /// The policy must be a `Restricted` policy scoped to explicit readable
    /// roots. A policy that grants full-disk read access is rejected as
    /// unavailable: an untrusted external agent must never be handed a
    /// full-disk read channel even if the surrounding process sandbox happens to
    /// be looser.
    pub fn new(
        mode: ExternalAgentMode,
        capabilities: ExternalAgentCapabilities,
        file_system: FileSystemSandboxPolicy,
        network: NetworkSandboxPolicy,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        let sandbox = if file_system.has_full_disk_read_access() {
            GuardSandbox::Unavailable {
                reason: "external-agent sandbox must be restricted to explicit readable roots"
                    .to_string(),
            }
        } else {
            GuardSandbox::Restricted { file_system }
        };
        Self {
            mode,
            capabilities,
            sandbox,
            network,
            cwd: cwd.into(),
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
        }
    }

    /// Build a guard from the run's `PermissionProfile`. A disabled profile is
    /// treated as an unavailable sandbox (deny reads), matching the fail-closed
    /// posture of the process sandbox layer.
    pub fn from_permission_profile(
        mode: ExternalAgentMode,
        capabilities: ExternalAgentCapabilities,
        profile: &PermissionProfile,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        if matches!(profile, PermissionProfile::Disabled) {
            return Self {
                mode,
                capabilities,
                sandbox: GuardSandbox::Unavailable {
                    reason: "external-agent sandbox permissions are disabled".to_string(),
                },
                network: profile.network_sandbox_policy(),
                cwd: cwd.into(),
                max_read_bytes: DEFAULT_MAX_READ_BYTES,
            };
        }
        let (file_system, network) = profile.to_runtime_permissions();
        Self::new(mode, capabilities, file_system, network, cwd)
    }

    pub fn mode(&self) -> ExternalAgentMode {
        self.mode
    }

    pub fn max_read_bytes(&self) -> u64 {
        self.max_read_bytes
    }

    #[cfg(test)]
    pub fn with_max_read_bytes(mut self, max_read_bytes: u64) -> Self {
        self.max_read_bytes = max_read_bytes;
        self
    }

    /// Whether a fully-resolved path may be read under this run's sandbox.
    ///
    /// The host calls this again on the canonicalized path before reading so a
    /// symlink inside a readable root cannot smuggle an out-of-sandbox target
    /// past the lexical check.
    pub fn can_read(&self, path: &Path) -> bool {
        let GuardSandbox::Restricted { file_system } = &self.sandbox else {
            return false;
        };
        if !file_system.can_read_path_with_cwd(path, &self.cwd) {
            return false;
        }
        if let Some(matcher) = ReadDenyMatcher::new(file_system, &self.cwd)
            && matcher.is_read_denied(path)
        {
            return false;
        }
        true
    }

    /// Whether a path is inside the workspace, used to scope proposal targets.
    fn within_workspace(&self, path: &Path) -> bool {
        // A restricted read policy's readable roots define the workspace we are
        // willing to record proposals against.
        self.can_read(path)
    }

    /// Decide how the host must handle `action`.
    pub fn decide(&self, action: &ExternalAgentActionRequest) -> ExternalAgentActionDecision {
        match action {
            ExternalAgentActionRequest::ReadFile { path } => self.decide_read(path),
            ExternalAgentActionRequest::WriteFile { path, content } => {
                self.decide_write(path, content)
            }
            ExternalAgentActionRequest::RunCommand { .. } => ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::CommandNotDelegated,
                format!(
                    "shell commands are not executed by the runtime in {} mode; \
                     they must be delegated to Codewith's native exec runtime",
                    mode_label(self.mode)
                ),
            ),
            ExternalAgentActionRequest::McpToolCall { server, tool, .. } => {
                ExternalAgentActionDecision::deny(
                    ExternalAgentGuardDenial::McpNotDelegated,
                    format!(
                        "MCP tool `{server}/{tool}` must be delegated to Codewith's native MCP \
                         approval path; the runtime may not call it directly"
                    ),
                )
            }
            ExternalAgentActionRequest::NetworkAccess { target, .. } => {
                ExternalAgentActionDecision::deny(
                    ExternalAgentGuardDenial::NetworkNotDelegated,
                    format!(
                        "network access to `{target}` must be delegated to Codewith's native \
                         network approval; the runtime may not open it directly"
                    ),
                )
            }
            ExternalAgentActionRequest::Other { label, .. } => ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::Unsupported,
                format!("unsupported external-agent action `{label}`"),
            ),
        }
    }

    fn decide_read(&self, path: &Path) -> ExternalAgentActionDecision {
        if matches!(self.capabilities.filesystem, FileSystemCapability::None) {
            return ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::CapabilityDisabled,
                format!(
                    "filesystem reads are not permitted in {} mode",
                    mode_label(self.mode)
                ),
            );
        }
        if let GuardSandbox::Unavailable { reason } = &self.sandbox {
            return ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::SandboxUnavailable,
                reason.clone(),
            );
        }
        let resolved = self.resolve(path);
        // The read-deny matcher takes precedence so an explicitly protected path
        // is reported as deny-listed rather than merely out of scope.
        if self.is_read_denied(&resolved) {
            return ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::ReadDenied,
                format!(
                    "path `{}` is deny-listed by the sandbox policy",
                    resolved.display()
                ),
            );
        }
        if !self.can_read(&resolved) {
            return ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::OutsideSandbox,
                format!(
                    "path `{}` is outside the sandbox readable roots",
                    resolved.display()
                ),
            );
        }
        ExternalAgentActionDecision::PerformRead { path: resolved }
    }

    fn decide_write(&self, path: &Path, content: &str) -> ExternalAgentActionDecision {
        if matches!(self.capabilities.filesystem, FileSystemCapability::None) {
            return ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::CapabilityDisabled,
                format!(
                    "filesystem writes are not permitted in {} mode",
                    mode_label(self.mode)
                ),
            );
        }
        let resolved = self.resolve(path);
        if !self.within_workspace(&resolved) {
            return ExternalAgentActionDecision::deny(
                ExternalAgentGuardDenial::PathEscape,
                format!(
                    "cannot stage an edit to `{}`: it is outside the workspace",
                    resolved.display()
                ),
            );
        }
        // Writes are NEVER applied by the runtime, even in a managed capability.
        // They are recorded as proposals that Codewith stages and promotes.
        ExternalAgentActionDecision::RecordProposal {
            edit: ProposedWorkspaceEdit {
                path: resolved,
                content: content.to_string(),
            },
            reason: "workspace edits are recorded as patch proposals and promoted by Codewith; \
                     the runtime may not write files directly"
                .to_string(),
        }
    }

    fn is_read_denied(&self, path: &Path) -> bool {
        let GuardSandbox::Restricted { file_system } = &self.sandbox else {
            return false;
        };
        ReadDenyMatcher::new(file_system, &self.cwd)
            .is_some_and(|matcher| matcher.is_read_denied(path))
    }

    /// Resolve a runtime-supplied path against cwd and collapse `.`/`..`
    /// lexically so traversal segments cannot escape the sandbox check.
    fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            normalize_lexical(path)
        } else {
            normalize_lexical(&self.cwd.join(path))
        }
    }
}

fn mode_label(mode: ExternalAgentMode) -> &'static str {
    match mode {
        ExternalAgentMode::Consult => "consult",
        ExternalAgentMode::Plan => "plan",
        ExternalAgentMode::Propose => "propose",
        ExternalAgentMode::Managed => "managed",
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::permissions::FileSystemAccessMode;
    use codex_protocol::permissions::FileSystemPath;
    use codex_protocol::permissions::FileSystemSandboxEntry;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn readable_root_policy(roots: &[&Path]) -> FileSystemSandboxPolicy {
        FileSystemSandboxPolicy::restricted(
            roots
                .iter()
                .map(|root| FileSystemSandboxEntry {
                    path: FileSystemPath::Path {
                        path: AbsolutePathBuf::from_absolute_path(root)
                            .expect("absolute readable root"),
                    },
                    access: FileSystemAccessMode::Read,
                })
                .collect(),
        )
    }

    fn propose_guard(cwd: &Path) -> ExternalAgentActionGuard {
        ExternalAgentActionGuard::new(
            ExternalAgentMode::Propose,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
            readable_root_policy(&[cwd]),
            NetworkSandboxPolicy::Restricted,
            cwd,
        )
    }

    #[test]
    fn confined_read_within_root_is_performed() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/repo/src/lib.rs"),
        });

        assert_eq!(
            decision,
            ExternalAgentActionDecision::PerformRead {
                path: PathBuf::from("/repo/src/lib.rs"),
            }
        );
        assert!(!decision.authorizes_runtime_side_effect());
    }

    #[test]
    fn relative_read_resolves_against_cwd() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("src/main.rs"),
        });

        assert_eq!(
            decision,
            ExternalAgentActionDecision::PerformRead {
                path: PathBuf::from("/repo/src/main.rs"),
            }
        );
    }

    #[test]
    fn traversal_escape_is_denied_as_outside_sandbox() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        // Lexical `..` collapse turns this into /etc/passwd, outside the root.
        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/repo/../etc/passwd"),
        });

        match decision {
            ExternalAgentActionDecision::Deny { code, .. } => {
                assert_eq!(code, ExternalAgentGuardDenial::OutsideSandbox);
            }
            other => panic!("expected OutsideSandbox deny, got {other:?}"),
        }
    }

    #[test]
    fn relative_traversal_escape_is_denied() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("../../etc/shadow"),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::OutsideSandbox,
                ..
            }
        ));
    }

    #[test]
    fn read_outside_readable_roots_is_denied() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/etc/passwd"),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::OutsideSandbox,
                ..
            }
        ));
    }

    #[test]
    fn plan_mode_disables_all_filesystem_reads() {
        let cwd = Path::new("/repo");
        let guard = ExternalAgentActionGuard::new(
            ExternalAgentMode::Plan,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Plan),
            readable_root_policy(&[cwd]),
            NetworkSandboxPolicy::Restricted,
            cwd,
        );

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/repo/src/lib.rs"),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::CapabilityDisabled,
                ..
            }
        ));
    }

    #[test]
    fn deny_listed_read_reports_read_denied() {
        let cwd = Path::new("/repo");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(cwd).expect("cwd"),
                },
                access: FileSystemAccessMode::Read,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(Path::new("/repo/.git"))
                        .expect("git dir"),
                },
                access: FileSystemAccessMode::Deny,
            },
        ]);
        let guard = ExternalAgentActionGuard::new(
            ExternalAgentMode::Propose,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
            policy,
            NetworkSandboxPolicy::Restricted,
            cwd,
        );

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/repo/.git/config"),
        });

        assert!(
            matches!(
                decision,
                ExternalAgentActionDecision::Deny {
                    code: ExternalAgentGuardDenial::ReadDenied,
                    ..
                }
            ),
            "deny-listed subtree read must report ReadDenied, got {decision:?}"
        );
    }

    #[test]
    fn write_is_recorded_as_proposal_never_executed() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::WriteFile {
            path: PathBuf::from("/repo/src/lib.rs"),
            content: "fn main() {}".to_string(),
        });

        assert!(!decision.authorizes_runtime_side_effect());
        match decision {
            ExternalAgentActionDecision::RecordProposal { edit, reason } => {
                assert_eq!(edit.path, PathBuf::from("/repo/src/lib.rs"));
                assert_eq!(edit.content, "fn main() {}");
                assert!(reason.contains("promoted by Codewith"));
            }
            other => panic!("expected RecordProposal, got {other:?}"),
        }
    }

    #[test]
    fn managed_capability_still_only_proposes_writes() {
        // Even if a future managed capability is granted, the guard must never
        // authorize a direct runtime write.
        let cwd = Path::new("/repo");
        let guard = ExternalAgentActionGuard::new(
            ExternalAgentMode::Managed,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Managed),
            readable_root_policy(&[cwd]),
            NetworkSandboxPolicy::Enabled,
            cwd,
        );

        let decision = guard.decide(&ExternalAgentActionRequest::WriteFile {
            path: PathBuf::from("/repo/x"),
            content: "data".to_string(),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::RecordProposal { .. }
        ));
    }

    #[test]
    fn write_outside_workspace_is_path_escape() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::WriteFile {
            path: PathBuf::from("/etc/cron.d/evil"),
            content: "* * * * * root sh".to_string(),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::PathEscape,
                ..
            }
        ));
    }

    #[test]
    fn shell_command_is_denied_as_not_delegated() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::RunCommand {
            command: vec!["rm".to_string(), "-rf".to_string(), "/".to_string()],
            cwd: None,
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::CommandNotDelegated,
                ..
            }
        ));
    }

    #[test]
    fn mcp_call_is_denied_as_not_delegated() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::McpToolCall {
            server: "github".to_string(),
            tool: "create_pr".to_string(),
            arguments: json!({"title": "x"}),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::McpNotDelegated,
                ..
            }
        ));
    }

    #[test]
    fn network_access_is_denied_as_not_delegated() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::NetworkAccess {
            target: "https://evil.example".to_string(),
            purpose: None,
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::NetworkNotDelegated,
                ..
            }
        ));
    }

    #[test]
    fn unknown_action_fails_closed() {
        let cwd = Path::new("/repo");
        let guard = propose_guard(cwd);

        let decision = guard.decide(&ExternalAgentActionRequest::Other {
            label: "cursor.applyEditImmediately".to_string(),
            payload: json!({"path": "/repo/x"}),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::Unsupported,
                ..
            }
        ));
    }

    #[test]
    fn disabled_sandbox_denies_reads() {
        let cwd = Path::new("/repo");
        let guard = ExternalAgentActionGuard::from_permission_profile(
            ExternalAgentMode::Propose,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
            &PermissionProfile::Disabled,
            cwd,
        );

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/repo/src/lib.rs"),
        });

        assert!(matches!(
            decision,
            ExternalAgentActionDecision::Deny {
                code: ExternalAgentGuardDenial::SandboxUnavailable,
                ..
            }
        ));
        assert!(!guard.can_read(Path::new("/repo/src/lib.rs")));
    }

    #[test]
    fn full_disk_read_policy_is_rejected_as_unavailable() {
        let cwd = Path::new("/repo");
        let guard = ExternalAgentActionGuard::new(
            ExternalAgentMode::Propose,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
            FileSystemSandboxPolicy::unrestricted(),
            NetworkSandboxPolicy::Enabled,
            cwd,
        );

        let decision = guard.decide(&ExternalAgentActionRequest::ReadFile {
            path: PathBuf::from("/repo/src/lib.rs"),
        });

        assert!(
            matches!(
                decision,
                ExternalAgentActionDecision::Deny {
                    code: ExternalAgentGuardDenial::SandboxUnavailable,
                    ..
                }
            ),
            "an untrusted agent must not be handed a full-disk read channel, got {decision:?}"
        );
    }

    #[test]
    fn from_permission_profile_scopes_reads_to_profile_roots() {
        let cwd = Path::new("/repo");
        let profile = PermissionProfile::from_runtime_permissions(
            &readable_root_policy(&[cwd]),
            NetworkSandboxPolicy::Enabled,
        );
        let guard = ExternalAgentActionGuard::from_permission_profile(
            ExternalAgentMode::Propose,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
            &profile,
            cwd,
        );

        assert!(guard.can_read(Path::new("/repo/src/lib.rs")));
        assert!(!guard.can_read(Path::new("/etc/passwd")));
    }

    #[test]
    fn guard_never_authorizes_runtime_side_effects_across_modes_and_actions() {
        let cwd = Path::new("/repo");
        let actions = [
            ExternalAgentActionRequest::ReadFile {
                path: PathBuf::from("/repo/a"),
            },
            ExternalAgentActionRequest::ReadFile {
                path: PathBuf::from("/etc/passwd"),
            },
            ExternalAgentActionRequest::WriteFile {
                path: PathBuf::from("/repo/a"),
                content: "x".to_string(),
            },
            ExternalAgentActionRequest::RunCommand {
                command: vec!["ls".to_string()],
                cwd: None,
            },
            ExternalAgentActionRequest::McpToolCall {
                server: "s".to_string(),
                tool: "t".to_string(),
                arguments: json!({}),
            },
            ExternalAgentActionRequest::NetworkAccess {
                target: "https://x".to_string(),
                purpose: None,
            },
            ExternalAgentActionRequest::Other {
                label: "x".to_string(),
                payload: json!({}),
            },
        ];
        for mode in [
            ExternalAgentMode::Consult,
            ExternalAgentMode::Plan,
            ExternalAgentMode::Propose,
            ExternalAgentMode::Managed,
        ] {
            let guard = ExternalAgentActionGuard::new(
                mode,
                ExternalAgentCapabilities::for_mode(mode),
                readable_root_policy(&[cwd]),
                NetworkSandboxPolicy::Enabled,
                cwd,
            );
            for action in &actions {
                let decision = guard.decide(action);
                assert!(
                    !decision.authorizes_runtime_side_effect(),
                    "{mode:?} / {action:?} must not authorize a runtime side effect"
                );
                // The only "allow" a runtime ever gets is a read.
                if let ExternalAgentActionDecision::PerformRead { .. } = decision {
                    assert!(
                        matches!(action, ExternalAgentActionRequest::ReadFile { .. }),
                        "only reads may resolve to PerformRead"
                    );
                }
            }
        }
    }
}
