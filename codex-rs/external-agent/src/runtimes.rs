use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::ExternalAgentMode;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentReadinessStatus;
use crate::ExternalAgentRuntimeId;

/// User-facing metadata for a built-in external-agent runtime adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentRuntimeDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub command: ExternalAgentCommandSpec,
    pub supported_modes: &'static [ExternalAgentMode],
    pub default_mode: ExternalAgentMode,
    pub visible: bool,
}

impl ExternalAgentRuntimeDescriptor {
    pub fn readiness(&self, status: ExternalAgentReadinessStatus) -> ExternalAgentReadiness {
        ExternalAgentReadiness {
            runtime: ExternalAgentRuntimeId::from(self.id),
            status,
            display_name: self.display_name.to_string(),
            version: None,
            supported_modes: self.supported_modes.to_vec(),
            detail: None,
        }
    }
}

/// Process command used to reach an external runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentCommandSpec {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// Launch plan after a runtime command has been resolved by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentLaunchSpec {
    pub runtime: ExternalAgentRuntimeId,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub arg0: Option<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub isolation: ExternalAgentLaunchIsolation,
}

/// Launch plan that has been wrapped by Codewith's platform sandbox.
///
/// The inner launch spec is intentionally private so callers cannot forge a
/// sandbox proof by copying a marker onto a direct child-process command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentSandboxedLaunchSpec {
    launch: ExternalAgentLaunchSpec,
}

impl ExternalAgentSandboxedLaunchSpec {
    pub(crate) fn new_platform_sandboxed(launch: ExternalAgentLaunchSpec) -> Self {
        debug_assert!(matches!(
            launch.isolation,
            ExternalAgentLaunchIsolation::PlatformSandboxed(_)
        ));
        Self { launch }
    }

    pub fn as_launch_spec(&self) -> &ExternalAgentLaunchSpec {
        &self.launch
    }

    pub(crate) fn into_launch_spec(self) -> ExternalAgentLaunchSpec {
        self.launch
    }

    #[cfg(test)]
    pub(crate) fn test_only_unenforced(mut launch: ExternalAgentLaunchSpec) -> Self {
        launch.isolation = ExternalAgentLaunchIsolation::test_only_unenforced();
        Self { launch }
    }
}

/// Evidence that an external-agent subprocess launch is safe to execute.
///
/// The default ACP harness builds [`ExternalAgentLaunchIsolation::Unenforced`]
/// launch specs until a caller wraps the process command with Codewith's
/// platform sandbox. The process manager refuses those specs so new call sites
/// cannot accidentally run a high-trust external CLI directly in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAgentLaunchIsolation {
    Unenforced {
        reason: String,
    },
    PlatformSandboxed(ExternalAgentPlatformSandbox),
    #[cfg(test)]
    TestOnlyUnenforced,
}

impl ExternalAgentLaunchIsolation {
    pub fn unenforced(reason: impl Into<String>) -> Self {
        Self::Unenforced {
            reason: reason.into(),
        }
    }

    pub fn is_process_enforced(&self) -> bool {
        match self {
            Self::Unenforced { .. } => false,
            Self::PlatformSandboxed(_) => true,
            #[cfg(test)]
            Self::TestOnlyUnenforced => true,
        }
    }

    pub fn unenforced_reason(&self) -> Option<&str> {
        match self {
            Self::Unenforced { reason } => Some(reason),
            Self::PlatformSandboxed(_) => None,
            #[cfg(test)]
            Self::TestOnlyUnenforced => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_only_unenforced() -> Self {
        Self::TestOnlyUnenforced
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentPlatformSandbox {
    summary: String,
    _private: (),
}

impl ExternalAgentPlatformSandbox {
    pub(crate) fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            _private: (),
        }
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }
}

const PLAN_PROPOSE: &[ExternalAgentMode] = &[ExternalAgentMode::Plan, ExternalAgentMode::Propose];

pub const BUILTIN_EXTERNAL_AGENT_RUNTIMES: &[ExternalAgentRuntimeDescriptor] = &[
    ExternalAgentRuntimeDescriptor {
        id: ExternalAgentRuntimeId::CURSOR,
        display_name: "Cursor",
        description: "Run Cursor's agent through an ACP-compatible harness.",
        command: ExternalAgentCommandSpec {
            program: "cursor-agent",
            args: &["acp"],
        },
        supported_modes: PLAN_PROPOSE,
        default_mode: ExternalAgentMode::Plan,
        visible: true,
    },
    ExternalAgentRuntimeDescriptor {
        id: ExternalAgentRuntimeId::GROK_BUILD,
        display_name: "Grok Build",
        description: "Run Grok Build through xAI's ACP stdio agent.",
        command: ExternalAgentCommandSpec {
            program: "grok",
            args: &["agent", "stdio"],
        },
        supported_modes: PLAN_PROPOSE,
        default_mode: ExternalAgentMode::Plan,
        visible: true,
    },
    ExternalAgentRuntimeDescriptor {
        id: ExternalAgentRuntimeId::CLAUDE,
        display_name: "Claude Code",
        description: "Run Claude Code through Claude's CLI/Agent SDK stream.",
        command: ExternalAgentCommandSpec {
            program: "claude",
            args: &[],
        },
        supported_modes: PLAN_PROPOSE,
        default_mode: ExternalAgentMode::Plan,
        visible: true,
    },
];

pub fn builtin_external_agent_runtimes() -> &'static [ExternalAgentRuntimeDescriptor] {
    BUILTIN_EXTERNAL_AGENT_RUNTIMES
}

pub fn visible_external_agent_runtimes()
-> impl Iterator<Item = &'static ExternalAgentRuntimeDescriptor> {
    BUILTIN_EXTERNAL_AGENT_RUNTIMES
        .iter()
        .filter(|runtime| runtime.visible)
}

pub fn find_external_agent_runtime(id: &str) -> Option<&'static ExternalAgentRuntimeDescriptor> {
    BUILTIN_EXTERNAL_AGENT_RUNTIMES
        .iter()
        .find(|runtime| runtime.id == id)
}

pub fn external_agent_runtime_readiness(
    runtime: &'static ExternalAgentRuntimeDescriptor,
) -> ExternalAgentReadiness {
    match which::which(runtime.command.program) {
        Ok(program) => {
            let mut readiness = runtime.readiness(ExternalAgentReadinessStatus::Ready);
            readiness.detail = Some(program.display().to_string());
            readiness
        }
        Err(err) => {
            let mut readiness = runtime.readiness(ExternalAgentReadinessStatus::MissingRuntime);
            readiness.detail = Some(err.to_string());
            readiness
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn visible_runtimes_include_subscription_external_agents() {
        let visible = visible_external_agent_runtimes()
            .map(|runtime| runtime.id)
            .collect::<Vec<_>>();

        assert_eq!(visible, vec!["cursor", "grok-build", "claude"]);
    }

    #[test]
    fn visible_runtimes_do_not_advertise_managed_mode() {
        for runtime in visible_external_agent_runtimes() {
            assert!(
                !runtime
                    .supported_modes
                    .contains(&ExternalAgentMode::Managed),
                "{} must stay gated until process sandbox enforcement lands",
                runtime.id
            );
        }
    }

    #[test]
    fn grok_build_is_the_canonical_runtime_id() {
        let Some(runtime) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };

        assert_eq!(runtime.display_name, "Grok Build");
        assert!(find_external_agent_runtime("grok").is_none());
    }

    #[test]
    fn claude_is_the_canonical_runtime_id() {
        let Some(runtime) = find_external_agent_runtime("claude") else {
            panic!("claude runtime");
        };

        assert_eq!(runtime.display_name, "Claude Code");
        assert_eq!(runtime.command.program, "claude");
        assert!(find_external_agent_runtime("claude-code").is_none());
    }

    #[test]
    fn readiness_reports_the_runtime_identity() {
        let Some(runtime) = find_external_agent_runtime("cursor") else {
            panic!("cursor runtime");
        };
        let readiness = external_agent_runtime_readiness(runtime);

        assert_eq!(readiness.runtime, ExternalAgentRuntimeId::from("cursor"));
        assert_eq!(readiness.display_name, "Cursor");
        assert_eq!(readiness.supported_modes, PLAN_PROPOSE);
    }
}
