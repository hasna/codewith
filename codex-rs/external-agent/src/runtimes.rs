use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::ExternalAgentMode;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentReadinessStatus;
use crate::ExternalAgentRuntimeId;

/// Execution surface a runtime can be reached through.
///
/// `Acp` runs the runtime as a local ACP stdio subprocess, `SdkLocal` runs the
/// runtime's local SDK/CLI stream on this machine, and `Cloud` delegates the run
/// to the runtime's hosted (cloud) agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAgentExecutionSurface {
    Acp,
    SdkLocal,
    Cloud,
}

impl ExternalAgentExecutionSurface {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Acp => "acp",
            Self::SdkLocal => "sdk-local",
            Self::Cloud => "cloud",
        }
    }
}

/// A model an external-agent runtime advertises as runnable.
///
/// This is the built-in, statically-known set. Live per-account discovery
/// (`thread/externalAgent/models/list`) may refine or extend it, but the
/// descriptor set is always a safe, offline default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalAgentModelDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub execution_surfaces: &'static [ExternalAgentExecutionSurface],
}

/// User-facing metadata for a built-in external-agent runtime adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentRuntimeDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub command: ExternalAgentCommandSpec,
    pub supported_modes: &'static [ExternalAgentMode],
    pub default_mode: ExternalAgentMode,
    pub execution_surfaces: &'static [ExternalAgentExecutionSurface],
    pub default_execution_surface: ExternalAgentExecutionSurface,
    pub models: &'static [ExternalAgentModelDescriptor],
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

    /// Whether this runtime advertises support for `mode`.
    pub fn supports_mode(&self, mode: ExternalAgentMode) -> bool {
        self.supported_modes.contains(&mode)
    }

    /// Whether this runtime can be reached through `surface`.
    pub fn supports_execution_surface(&self, surface: ExternalAgentExecutionSurface) -> bool {
        self.execution_surfaces.contains(&surface)
    }

    /// The model used when a run omits an explicit selection.
    pub fn default_model(&self) -> Option<&'static ExternalAgentModelDescriptor> {
        self.models.first()
    }

    /// Look up an advertised model by id.
    pub fn find_model(&self, id: &str) -> Option<&'static ExternalAgentModelDescriptor> {
        self.models.iter().find(|model| model.id == id)
    }

    /// Advertised models runnable on `surface`.
    pub fn models_for_surface(
        &self,
        surface: ExternalAgentExecutionSurface,
    ) -> impl Iterator<Item = &'static ExternalAgentModelDescriptor> {
        self.models
            .iter()
            .filter(move |model| model.execution_surfaces.contains(&surface))
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

/// Cursor's action executor (Codewith-mediated) has landed, so Cursor advertises
/// managed mode alongside the safe plan/propose defaults.
const PLAN_PROPOSE_MANAGED: &[ExternalAgentMode] = &[
    ExternalAgentMode::Plan,
    ExternalAgentMode::Propose,
    ExternalAgentMode::Managed,
];

const ACP_SDK_CLOUD_SURFACES: &[ExternalAgentExecutionSurface] = &[
    ExternalAgentExecutionSurface::Acp,
    ExternalAgentExecutionSurface::SdkLocal,
    ExternalAgentExecutionSurface::Cloud,
];
const ACP_CLOUD_SURFACES: &[ExternalAgentExecutionSurface] = &[
    ExternalAgentExecutionSurface::Acp,
    ExternalAgentExecutionSurface::Cloud,
];
const SDK_CLOUD_SURFACES: &[ExternalAgentExecutionSurface] = &[
    ExternalAgentExecutionSurface::SdkLocal,
    ExternalAgentExecutionSurface::Cloud,
];

const CURSOR_MODELS: &[ExternalAgentModelDescriptor] = &[
    ExternalAgentModelDescriptor {
        id: "auto",
        display_name: "Auto (Cursor-selected)",
        description: "Let Cursor pick the best available model for the task.",
        execution_surfaces: ACP_SDK_CLOUD_SURFACES,
    },
    ExternalAgentModelDescriptor {
        id: "gpt-5-codex",
        display_name: "GPT-5 Codex",
        description: "OpenAI GPT-5 Codex, served through Cursor.",
        execution_surfaces: ACP_SDK_CLOUD_SURFACES,
    },
    ExternalAgentModelDescriptor {
        id: "claude-sonnet-4.5",
        display_name: "Claude Sonnet 4.5",
        description: "Anthropic Claude Sonnet 4.5, served through Cursor.",
        execution_surfaces: ACP_SDK_CLOUD_SURFACES,
    },
];

const GROK_BUILD_MODELS: &[ExternalAgentModelDescriptor] = &[
    ExternalAgentModelDescriptor {
        id: "auto",
        display_name: "Auto (Grok-selected)",
        description: "Let Grok Build pick the best available model for the task.",
        execution_surfaces: ACP_CLOUD_SURFACES,
    },
    ExternalAgentModelDescriptor {
        id: "grok-code",
        display_name: "Grok Code",
        description: "xAI Grok coding model, served through Grok Build.",
        execution_surfaces: ACP_CLOUD_SURFACES,
    },
];

const CLAUDE_MODELS: &[ExternalAgentModelDescriptor] = &[
    ExternalAgentModelDescriptor {
        id: "default",
        display_name: "Default (Claude-selected)",
        description: "Use the model configured for the active Claude CLI session.",
        execution_surfaces: SDK_CLOUD_SURFACES,
    },
    ExternalAgentModelDescriptor {
        id: "claude-opus-4.5",
        display_name: "Claude Opus 4.5",
        description: "Anthropic Claude Opus 4.5, served through Claude Code.",
        execution_surfaces: SDK_CLOUD_SURFACES,
    },
    ExternalAgentModelDescriptor {
        id: "claude-sonnet-4.5",
        display_name: "Claude Sonnet 4.5",
        description: "Anthropic Claude Sonnet 4.5, served through Claude Code.",
        execution_surfaces: SDK_CLOUD_SURFACES,
    },
];

pub const BUILTIN_EXTERNAL_AGENT_RUNTIMES: &[ExternalAgentRuntimeDescriptor] = &[
    ExternalAgentRuntimeDescriptor {
        id: ExternalAgentRuntimeId::CURSOR,
        display_name: "Cursor",
        description: "Run Cursor's agent through an ACP-compatible harness.",
        command: ExternalAgentCommandSpec {
            program: "agent",
            args: &["acp"],
        },
        supported_modes: PLAN_PROPOSE_MANAGED,
        default_mode: ExternalAgentMode::Plan,
        execution_surfaces: ACP_SDK_CLOUD_SURFACES,
        default_execution_surface: ExternalAgentExecutionSurface::Acp,
        models: CURSOR_MODELS,
        visible: true,
    },
    ExternalAgentRuntimeDescriptor {
        id: ExternalAgentRuntimeId::GROK_BUILD,
        display_name: "Grok Build",
        description: "Run Grok Build through xAI's ACP stdio agent.",
        command: ExternalAgentCommandSpec {
            program: "grok",
            args: &["--no-auto-update", "agent", "stdio"],
        },
        supported_modes: PLAN_PROPOSE,
        default_mode: ExternalAgentMode::Plan,
        execution_surfaces: ACP_CLOUD_SURFACES,
        default_execution_surface: ExternalAgentExecutionSurface::Acp,
        models: GROK_BUILD_MODELS,
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
        execution_surfaces: SDK_CLOUD_SURFACES,
        default_execution_surface: ExternalAgentExecutionSurface::SdkLocal,
        models: CLAUDE_MODELS,
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
    fn cursor_advertises_managed_mode_once_executor_enforcement_lands() {
        let cursor = find_external_agent_runtime("cursor").expect("cursor runtime");
        assert!(
            cursor.supports_mode(ExternalAgentMode::Managed),
            "cursor advertises managed mode now that Codewith action mediation exists"
        );

        for runtime in visible_external_agent_runtimes() {
            if runtime.id == ExternalAgentRuntimeId::CURSOR {
                continue;
            }
            assert!(
                !runtime.supports_mode(ExternalAgentMode::Managed),
                "{} must stay gated until its managed executor lands",
                runtime.id
            );
        }
    }

    #[test]
    fn runtimes_register_execution_surfaces_and_models() {
        let cursor = find_external_agent_runtime("cursor").expect("cursor runtime");
        assert_eq!(
            cursor.default_execution_surface,
            ExternalAgentExecutionSurface::Acp
        );
        for surface in [
            ExternalAgentExecutionSurface::Acp,
            ExternalAgentExecutionSurface::SdkLocal,
            ExternalAgentExecutionSurface::Cloud,
        ] {
            assert!(
                cursor.supports_execution_surface(surface),
                "cursor should register the {} surface",
                surface.as_str()
            );
        }
        assert_eq!(cursor.default_model().map(|model| model.id), Some("auto"));
        assert!(cursor.find_model("gpt-5-codex").is_some());
        assert!(cursor.find_model("does-not-exist").is_none());
        assert!(
            cursor
                .models_for_surface(ExternalAgentExecutionSurface::Cloud)
                .any(|model| model.id == "auto")
        );

        let claude = find_external_agent_runtime("claude").expect("claude runtime");
        assert_eq!(
            claude.default_execution_surface,
            ExternalAgentExecutionSurface::SdkLocal
        );
        assert!(!claude.supports_execution_surface(ExternalAgentExecutionSurface::Acp));
        assert!(claude.supports_execution_surface(ExternalAgentExecutionSurface::SdkLocal));
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
        assert_eq!(readiness.supported_modes, PLAN_PROPOSE_MANAGED);
    }
}
