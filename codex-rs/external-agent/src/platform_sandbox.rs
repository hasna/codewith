use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;

use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::models::PermissionProfile;
use codex_sandboxing::SandboxCommand;
use codex_sandboxing::SandboxManager;
use codex_sandboxing::SandboxTransformRequest;
use codex_sandboxing::SandboxType;
use codex_sandboxing::SandboxablePreference;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::ExternalAgentError;
use crate::ExternalAgentLaunchIsolation;
use crate::ExternalAgentLaunchSpec;
use crate::ExternalAgentPlatformSandbox;
use crate::ExternalAgentRuntimeId;
use crate::ExternalAgentSandboxedLaunchSpec;

/// Configuration used to wrap an external-agent subprocess with Codewith's
/// platform sandbox before it is eligible for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentSandboxConfig {
    pub permission_profile: PermissionProfile,
    pub codex_linux_sandbox_exe: Option<PathBuf>,
    pub use_legacy_landlock: bool,
    pub windows_sandbox_level: WindowsSandboxLevel,
    pub windows_sandbox_private_desktop: bool,
}

impl ExternalAgentSandboxConfig {
    pub fn new(permission_profile: PermissionProfile) -> Self {
        Self {
            permission_profile,
            codex_linux_sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        }
    }
}

/// Convert an unenforced launch spec into a platform-sandboxed command.
///
/// The returned launch can be passed to [`crate::AcpStdioProcess::spawn`].
/// The function rejects unsupported platforms and incomplete sandbox runtime
/// paths instead of downgrading to a direct child-process launch.
pub fn platform_sandbox_external_agent_launch(
    launch: ExternalAgentLaunchSpec,
    config: &ExternalAgentSandboxConfig,
) -> Result<ExternalAgentSandboxedLaunchSpec, ExternalAgentError> {
    platform_sandbox_external_agent_launch_with_writable_roots(launch, config, Vec::new())
}

pub(crate) fn platform_sandbox_external_agent_launch_with_writable_roots(
    launch: ExternalAgentLaunchSpec,
    config: &ExternalAgentSandboxConfig,
    writable_roots: Vec<PathBuf>,
) -> Result<ExternalAgentSandboxedLaunchSpec, ExternalAgentError> {
    let runtime = launch.runtime.clone();
    #[cfg(windows)]
    if launch.program.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    }) {
        return Err(not_ready(
            &runtime,
            "Windows external-agent sandbox launches must use a verified native program",
        ));
    }
    if matches!(config.permission_profile, PermissionProfile::Disabled) {
        return Err(not_ready(
            &runtime,
            "external-agent sandbox permissions must not be disabled",
        ));
    }
    let cwd = AbsolutePathBuf::from_absolute_path_checked(&launch.cwd).map_err(|err| {
        not_ready(
            &runtime,
            format!("external-agent cwd must be absolute for sandboxing: {err}"),
        )
    })?;
    let manager = SandboxManager::new();
    let (file_system_policy, network_policy) = config.permission_profile.to_runtime_permissions();
    let sandbox = manager.select_initial(
        &file_system_policy,
        network_policy,
        SandboxablePreference::Require,
        config.windows_sandbox_level,
        /*has_managed_network_requirements*/ false,
    );

    match sandbox {
        SandboxType::None => {
            return Err(not_ready(
                &runtime,
                "platform sandbox is not available for external-agent subprocesses",
            ));
        }
        SandboxType::WindowsRestrictedToken => {
            return Err(not_ready(
                &runtime,
                "Windows external-agent sandboxing must use the restricted-token executor",
            ));
        }
        SandboxType::MacosSeatbelt | SandboxType::LinuxSeccomp => {}
    }

    let command = SandboxCommand {
        program: launch.program.into_os_string(),
        args: launch.args,
        cwd: cwd.clone(),
        env: launch.env.into_iter().collect::<HashMap<_, _>>(),
        additional_permissions: additional_permissions(&runtime, writable_roots)?,
    };
    let request = manager
        .transform(SandboxTransformRequest {
            command,
            permissions: &config.permission_profile,
            sandbox,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: cwd.as_path(),
            codex_linux_sandbox_exe: config.codex_linux_sandbox_exe.as_deref(),
            use_legacy_landlock: config.use_legacy_landlock,
            windows_sandbox_level: config.windows_sandbox_level,
            windows_sandbox_private_desktop: config.windows_sandbox_private_desktop,
        })
        .map_err(|err| not_ready(&runtime, format!("sandbox transform failed: {err}")))?;

    let mut argv = request.command.into_iter();
    let Some(program) = argv.next() else {
        return Err(not_ready(
            &runtime,
            "sandbox transform returned an empty command",
        ));
    };

    let launch = ExternalAgentLaunchSpec {
        runtime,
        program: PathBuf::from(program),
        args: argv.collect(),
        arg0: request.arg0,
        cwd: request.cwd.into_path_buf(),
        env: request.env.into_iter().collect::<BTreeMap<_, _>>(),
        isolation: ExternalAgentLaunchIsolation::PlatformSandboxed(
            ExternalAgentPlatformSandbox::new(format!(
                "Codewith {} platform sandbox",
                sandbox.as_metric_tag()
            )),
        ),
    };
    Ok(ExternalAgentSandboxedLaunchSpec::new_platform_sandboxed(
        launch,
    ))
}

fn additional_permissions(
    runtime: &ExternalAgentRuntimeId,
    writable_roots: Vec<PathBuf>,
) -> Result<Option<AdditionalPermissionProfile>, ExternalAgentError> {
    if writable_roots.is_empty() {
        return Ok(None);
    }
    let mut roots = Vec::with_capacity(writable_roots.len());
    for root in writable_roots {
        roots.push(
            AbsolutePathBuf::from_absolute_path_checked(&root).map_err(|err| {
                not_ready(
                    runtime,
                    format!(
                        "external-agent writable sandbox root must be absolute: {} ({err})",
                        root.display()
                    ),
                )
            })?,
        );
    }
    Ok(Some(AdditionalPermissionProfile {
        network: None,
        file_system: Some(FileSystemPermissions::from_read_write_roots(
            /*read*/ None,
            Some(roots),
        )),
    }))
}

fn not_ready(runtime: &ExternalAgentRuntimeId, reason: impl Into<String>) -> ExternalAgentError {
    ExternalAgentError::NotReady {
        runtime: runtime.as_str().to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::permissions::NetworkSandboxPolicy;
    use pretty_assertions::assert_eq;

    fn launch_spec() -> ExternalAgentLaunchSpec {
        ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from("fake"),
            program: PathBuf::from("/bin/echo"),
            args: vec!["hello".to_string()],
            arg0: None,
            cwd: PathBuf::from("/tmp"),
            env: BTreeMap::from([("PATH".to_string(), "/bin".to_string())]),
            isolation: ExternalAgentLaunchIsolation::unenforced("not wrapped"),
        }
    }

    #[test]
    fn rejects_disabled_sandbox_permissions() {
        let config = ExternalAgentSandboxConfig::new(PermissionProfile::Disabled);

        let err = platform_sandbox_external_agent_launch(launch_spec(), &config)
            .expect_err("disabled permissions should be rejected");

        assert_eq!(
            err.to_string(),
            "external agent runtime `fake` is not ready: external-agent sandbox permissions must not be disabled"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_batch_programs_before_the_sandbox_can_transform_them() {
        let config = ExternalAgentSandboxConfig::new(PermissionProfile::External {
            network: NetworkSandboxPolicy::Restricted,
        });
        let mut launch = launch_spec();
        launch.program = PathBuf::from(r"C:\\tools\\agent.cmd");
        launch.cwd = PathBuf::from(r"C:\\tmp");

        let err = platform_sandbox_external_agent_launch(launch, &config)
            .expect_err("batch programs must not enter the sandbox command transform");

        assert_eq!(
            err.to_string(),
            "external agent runtime `fake` is not ready: Windows external-agent sandbox launches must use a verified native program"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_missing_linux_sandbox_helper() {
        let config = ExternalAgentSandboxConfig::new(PermissionProfile::External {
            network: NetworkSandboxPolicy::Restricted,
        });

        let err = platform_sandbox_external_agent_launch(launch_spec(), &config)
            .expect_err("missing Linux sandbox helper should be rejected");

        assert_eq!(
            err.to_string(),
            "external agent runtime `fake` is not ready: sandbox transform failed: missing codex-linux-sandbox executable path"
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn rejects_missing_platform_sandbox() {
        let config = ExternalAgentSandboxConfig::new(PermissionProfile::External {
            network: NetworkSandboxPolicy::Restricted,
        });
        let mut launch = launch_spec();
        #[cfg(windows)]
        {
            launch.cwd = PathBuf::from(r"C:\tmp");
        }

        let err = platform_sandbox_external_agent_launch(launch, &config).expect_err("error");
        assert_eq!(
            err.to_string(),
            "external agent runtime `fake` is not ready: platform sandbox is not available for external-agent subprocesses"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wraps_macos_launch_with_platform_sandbox() {
        let config = ExternalAgentSandboxConfig::new(PermissionProfile::External {
            network: NetworkSandboxPolicy::Restricted,
        });

        let spec = platform_sandbox_external_agent_launch(launch_spec(), &config)
            .expect("sandboxed launch");

        let spec = spec.as_launch_spec();
        assert_eq!(
            spec.program.file_name().and_then(|name| name.to_str()),
            Some("sandbox-exec")
        );
        assert!(spec.args.iter().any(|arg| arg == "/bin/echo"));
        assert!(spec.isolation.is_process_enforced());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wraps_linux_launch_with_platform_sandbox() {
        let config = ExternalAgentSandboxConfig {
            permission_profile: PermissionProfile::External {
                network: NetworkSandboxPolicy::Restricted,
            },
            codex_linux_sandbox_exe: Some(PathBuf::from("/tmp/codex-linux-sandbox")),
            use_legacy_landlock: true,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        };

        let spec = platform_sandbox_external_agent_launch(launch_spec(), &config)
            .expect("sandboxed launch");

        let spec = spec.as_launch_spec();
        assert_eq!(spec.program, PathBuf::from("/tmp/codex-linux-sandbox"));
        assert_eq!(spec.arg0, Some("/tmp/codex-linux-sandbox".to_string()));
        assert!(spec.args.iter().any(|arg| arg == "/bin/echo"));
        assert!(spec.isolation.is_process_enforced());
        assert_eq!(
            match &spec.isolation {
                ExternalAgentLaunchIsolation::PlatformSandboxed(sandbox) =>
                    sandbox.summary().to_string(),
                _ => panic!("expected platform sandbox"),
            },
            "Codewith seccomp platform sandbox"
        );
    }
}
