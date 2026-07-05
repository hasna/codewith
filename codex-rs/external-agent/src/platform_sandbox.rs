use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSpecialPath;
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

    let executable_read_roots =
        executable_read_roots(&launch.program, config.codex_linux_sandbox_exe.as_deref());
    let command = SandboxCommand {
        program: launch.program.into_os_string(),
        args: launch.args,
        cwd: cwd.clone(),
        env: launch.env.into_iter().collect::<HashMap<_, _>>(),
        additional_permissions: additional_permissions(
            &runtime,
            executable_read_roots,
            writable_roots,
        )?,
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
    read_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
) -> Result<Option<AdditionalPermissionProfile>, ExternalAgentError> {
    let mut entries = vec![FileSystemSandboxEntry {
        path: FileSystemPath::Special {
            value: FileSystemSpecialPath::Minimal,
        },
        access: FileSystemAccessMode::Read,
    }];

    for root in read_roots {
        let path = AbsolutePathBuf::from_absolute_path_checked(&root).map_err(|err| {
            not_ready(
                runtime,
                format!(
                    "external-agent executable read root must be absolute: {} ({err})",
                    root.display()
                ),
            )
        })?;
        entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::Path { path },
            access: FileSystemAccessMode::Read,
        });
    }

    for root in writable_roots {
        let path = AbsolutePathBuf::from_absolute_path_checked(&root).map_err(|err| {
            not_ready(
                runtime,
                format!(
                    "external-agent writable sandbox root must be absolute: {} ({err})",
                    root.display()
                ),
            )
        })?;
        entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::Path { path },
            access: FileSystemAccessMode::Write,
        });
    }
    Ok(Some(AdditionalPermissionProfile {
        network: None,
        file_system: Some(FileSystemPermissions {
            entries,
            glob_scan_max_depth: None,
        }),
    }))
}

fn executable_read_roots(program: &Path, codex_linux_sandbox_exe: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_executable_parent_roots(&mut roots, program);
    if let Some(codex_linux_sandbox_exe) = codex_linux_sandbox_exe {
        push_executable_parent_roots(&mut roots, codex_linux_sandbox_exe);
    }
    roots
}

fn push_executable_parent_roots(roots: &mut Vec<PathBuf>, executable: &Path) {
    push_parent_root(roots, executable);
    if let Ok(canonical) = executable.canonicalize() {
        push_parent_root(roots, canonical.as_path());
    }
}

fn push_parent_root(roots: &mut Vec<PathBuf>, executable: &Path) {
    let Some(parent) = executable.parent() else {
        return;
    };
    if parent.as_os_str().is_empty() {
        return;
    }
    let parent = parent.to_path_buf();
    if !roots.iter().any(|root| root == &parent) {
        roots.push(parent);
    }
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
    use codex_protocol::permissions::FileSystemAccessMode;
    use codex_protocol::permissions::FileSystemPath;
    use codex_protocol::permissions::FileSystemSandboxEntry;
    use codex_protocol::permissions::FileSystemSandboxPolicy;
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
        let file_system = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(Path::new("/workspace"))
                    .expect("absolute path"),
            },
            access: FileSystemAccessMode::Read,
        }]);
        let config = ExternalAgentSandboxConfig {
            permission_profile: PermissionProfile::from_runtime_permissions(
                &file_system,
                NetworkSandboxPolicy::Restricted,
            ),
            codex_linux_sandbox_exe: Some(PathBuf::from("/tmp/codewith-bin/codex-linux-sandbox")),
            use_legacy_landlock: true,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        };

        let spec = platform_sandbox_external_agent_launch(launch_spec(), &config)
            .expect("sandboxed launch");

        let spec = spec.as_launch_spec();
        assert_eq!(
            spec.program,
            PathBuf::from("/tmp/codewith-bin/codex-linux-sandbox")
        );
        assert_eq!(
            spec.arg0,
            Some("/tmp/codewith-bin/codex-linux-sandbox".to_string())
        );
        assert!(spec.args.iter().any(|arg| arg == "/bin/echo"));
        let permission_profile = transformed_permission_profile(spec);
        let (file_system_policy, _) = permission_profile.to_runtime_permissions();
        assert!(file_system_policy.include_platform_defaults());
        let readable_roots = file_system_policy.get_readable_roots_with_cwd(Path::new("/tmp"));
        let program_parent = std::fs::canonicalize("/bin/echo")
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("/bin"));
        assert!(
            readable_roots
                .iter()
                .any(|root| root.as_path() == program_parent.as_path()),
            "launch program parent should be readable: {readable_roots:?}"
        );
        assert!(
            readable_roots
                .iter()
                .any(|root| root.as_path() == Path::new("/tmp/codewith-bin")),
            "Linux sandbox helper parent should be readable: {readable_roots:?}"
        );
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

    #[cfg(target_os = "linux")]
    fn transformed_permission_profile(spec: &ExternalAgentLaunchSpec) -> PermissionProfile {
        let profile_index = spec
            .args
            .iter()
            .position(|arg| arg == "--permission-profile")
            .expect("permission profile arg");
        serde_json::from_str(&spec.args[profile_index + 1]).expect("permission profile json")
    }
}
