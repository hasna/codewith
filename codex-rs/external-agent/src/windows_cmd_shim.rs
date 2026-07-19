use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::path::Prefix;

/// A native invocation containing an absolute program and lossless OS arguments.
///
/// For non-batch inputs this preserves the native launch after canonicalization. This staged
/// primitive has no production Claude or ACP consumer yet: OPE2-00126 and OPE2-00127 must adopt
/// it before those launches are protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsNativeLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Prepares one of the static cmd-shim templates as a native invocation without `cmd.exe` or
/// `COMSPEC`.
///
/// Supported Node templates deliberately exclude shebang arguments and environment-variable
/// expansion. For a Node shim, only the caller-supplied `source_env` `PATH` is trusted to locate
/// an absolute `node.exe`; there is no sibling, host-environment, or `COMSPEC` fallback.
/// Canonical paths are a local-filesystem snapshot, so callers must revalidate immediately before
/// spawn to retain their reparse/TOCTOU boundary.
pub fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<OsString>,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<WindowsNativeLaunch, WindowsBatchLaunchError> {
    if !program.is_absolute() {
        return Err(WindowsBatchLaunchError::ProgramNotAbsolute);
    }
    if ambiguous_windows_program_path(&program) {
        return Err(WindowsBatchLaunchError::AmbiguousProgramPath);
    }
    let canonical_program = program
        .canonicalize()
        .map_err(WindowsBatchLaunchError::CanonicalizeProgram)?;
    if !canonical_program.is_absolute() {
        return Err(WindowsBatchLaunchError::CanonicalProgramNotAbsolute);
    }
    if !is_windows_batch_program(&canonical_program) {
        return Ok(WindowsNativeLaunch {
            program: canonical_program,
            args,
        });
    }
    let (target, runtime) = shim_target(&canonical_program)?;
    match runtime {
        ShimRuntime::DirectNative => Ok(WindowsNativeLaunch {
            program: target,
            args,
        }),
        ShimRuntime::Node => {
            let mut launch_args = Vec::with_capacity(args.len() + 1);
            launch_args.push(target.into_os_string());
            launch_args.extend(args);
            Ok(WindowsNativeLaunch {
                program: native_node(source_env, cwd)?,
                args: launch_args,
            })
        }
    }
}

fn shim_target(canonical_shim: &Path) -> Result<(PathBuf, ShimRuntime), WindowsBatchLaunchError> {
    let canonical_parent = canonical_shim
        .parent()
        .ok_or(WindowsBatchLaunchError::UnsupportedShim)?;
    let shim = read_bounded_shim(canonical_shim).map_err(WindowsBatchLaunchError::ReadShim)?;
    let (target, runtime) =
        recognized_shim(&shim).ok_or(WindowsBatchLaunchError::UnsupportedShim)?;
    let (canonical_root, layout) = trusted_shim_root(canonical_parent);
    let target = resolve_shim_target(target, &canonical_root, layout)?;
    if !target.is_file() {
        return Err(WindowsBatchLaunchError::MissingShimTarget { target });
    }
    let target = target
        .canonicalize()
        .map_err(WindowsBatchLaunchError::CanonicalizeTarget)?;
    if !target.is_absolute() {
        return Err(WindowsBatchLaunchError::CanonicalTargetNotAbsolute);
    }
    if !target.starts_with(&canonical_root) {
        return Err(WindowsBatchLaunchError::TargetEscapesShimRoot { target });
    }
    match runtime {
        ShimRuntime::DirectNative if native_executable(&target) => Ok((target, runtime)),
        ShimRuntime::Node if node_script(&target) => Ok((target, runtime)),
        _ => Err(WindowsBatchLaunchError::UnsupportedShim),
    }
}

// Published cmd-shim templates are small. Keep this generous limit while rejecting malformed
// configured shims before they can force an unbounded allocation or line-vector construction.
const MAX_CMD_SHIM_BYTES: usize = 64 * 1024;

fn read_bounded_shim(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(MAX_CMD_SHIM_BYTES);
    file.by_ref()
        .take(MAX_CMD_SHIM_BYTES as u64)
        .read_to_end(&mut bytes)?;
    if file.read(&mut [0])? != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Windows batch shim exceeds {MAX_CMD_SHIM_BYTES} bytes"),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.utf8_error()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShimRuntime {
    DirectNative,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShimLayout {
    NodeModulesBin,
    Prefix,
}

fn trusted_shim_root(canonical_parent: &Path) -> (PathBuf, ShimLayout) {
    let is_bin = canonical_parent
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(".bin"));
    let node_modules = canonical_parent.parent().filter(|parent| {
        parent
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("node_modules"))
    });
    if is_bin && let Some(node_modules) = node_modules {
        return (node_modules.to_path_buf(), ShimLayout::NodeModulesBin);
    }

    // A non-.bin shim is only accepted as a prefix/global shim. Its exact static template must
    // name a child under `node_modules`, so the shim's canonical parent is the narrow root.
    (canonical_parent.to_path_buf(), ShimLayout::Prefix)
}

fn resolve_shim_target(
    target: &str,
    canonical_root: &Path,
    layout: ShimLayout,
) -> Result<PathBuf, WindowsBatchLaunchError> {
    let components = strict_target_components(target)?;
    let components = match layout {
        ShimLayout::NodeModulesBin => match components.as_slice() {
            ["..", components @ ..] if !components.is_empty() => components,
            _ => return Err(WindowsBatchLaunchError::InvalidShimTarget),
        },
        ShimLayout::Prefix => {
            if !components
                .first()
                .is_some_and(|component| component.eq_ignore_ascii_case("node_modules"))
            {
                return Err(WindowsBatchLaunchError::InvalidShimTarget);
            }
            components.as_slice()
        }
    };
    Ok(components
        .iter()
        .fold(canonical_root.to_path_buf(), |path, component| {
            path.join(OsStr::new(component))
        }))
}

fn strict_target_components(target: &str) -> Result<Vec<&str>, WindowsBatchLaunchError> {
    if target.is_empty() || target.contains('/') || target.contains(['%', '!']) {
        return Err(WindowsBatchLaunchError::InvalidShimTarget);
    }
    let components = target.split('\\').collect::<Vec<_>>();
    if components.iter().any(|component| {
        component.is_empty()
            || component == &"."
            || (component != &".." && !normal_windows_component(component))
    }) {
        return Err(WindowsBatchLaunchError::InvalidShimTarget);
    }
    if components
        .iter()
        .enumerate()
        .any(|(index, component)| component == &".." && index != 0)
    {
        return Err(WindowsBatchLaunchError::InvalidShimTarget);
    }
    Ok(components)
}

fn normal_windows_component(component: &str) -> bool {
    !component.ends_with([' ', '.'])
        && !component.contains(['<', '>', ':', '"', '|', '?', '*'])
        && !component.chars().any(char::is_control)
        && !windows_device_name(component)
}

/// Rejects spellings Win32 can normalize into a different program after we classify it.
///
/// This rejects path forms that make a lexical `.cmd`/`.bat` decision unsafe, including
/// verbatim/device namespaces and components whose trailing dots or spaces Win32 ignores.
fn ambiguous_windows_program_path(program: &Path) -> bool {
    program.components().any(|component| match component {
        Component::Prefix(prefix) => matches!(
            prefix.kind(),
            Prefix::Verbatim(_)
                | Prefix::VerbatimDisk(_)
                | Prefix::VerbatimUNC(_, _)
                | Prefix::DeviceNS(_)
        ),
        Component::RootDir => false,
        Component::CurDir | Component::ParentDir => true,
        Component::Normal(component) => component
            .to_str()
            .is_none_or(|component| !normal_windows_component(component)),
    })
}

fn windows_device_name(component: &str) -> bool {
    let name = component.split('.').next().unwrap_or_default();
    ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|device| name.eq_ignore_ascii_case(device))
        || (name.get(..3).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT")
        }) && matches!(
            name.get(3..),
            Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        ))
}

fn recognized_shim(shim: &str) -> Option<(&str, ShimRuntime)> {
    let lines = normalized_batch_lines(shim)?;
    npm_cmd_shim_v9_target(&lines)
        .or_else(|| npm_cmd_shim_v8_target(&lines))
        .or_else(|| legacy_seven_line_corepack_target(&lines))
}

const CMD_SHIM_HEAD: [&str; 8] = [
    "@ECHO off",
    "GOTO start",
    ":find_dp0",
    "SET dp0=%~dp0",
    "EXIT /b",
    ":start",
    "SETLOCAL",
    "CALL :find_dp0",
];

fn npm_cmd_shim_v9_target<'a>(lines: &[&'a str]) -> Option<(&'a str, ShimRuntime)> {
    (lines.starts_with(&CMD_SHIM_HEAD)).then_some(())?;
    if lines.len() == CMD_SHIM_HEAD.len() + 1 {
        return target_from_line(lines.last()?, "\"%dp0%\\", "\"   %*")
            .map(|target| (target, ShimRuntime::DirectNative));
    }
    const BODY: [&str; 7] = [
        "",
        "IF EXIST \"%dp0%\\node.exe\" (",
        "  SET \"_prog=%dp0%\\node.exe\"",
        ") ELSE (",
        "  SET \"_prog=node\"",
        ")",
        "",
    ];
    (lines.len() == CMD_SHIM_HEAD.len() + BODY.len() + 1).then_some(())?;
    lines[CMD_SHIM_HEAD.len()..]
        .iter()
        .copied()
        .zip(BODY)
        .all(|(line, expected)| line == expected)
        .then_some(())?;
    target_from_line(
        lines.last()?,
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & set PATHEXT=%PATHEXT:;.JS;=;% & \"%_prog%\"  \"%dp0%\\",
        "\" %*",
    )
    .map(|target| (target, ShimRuntime::Node))
}

fn npm_cmd_shim_v8_target<'a>(lines: &[&'a str]) -> Option<(&'a str, ShimRuntime)> {
    (lines.starts_with(&CMD_SHIM_HEAD)).then_some(())?;
    const BODY: [&str; 8] = [
        "",
        "IF EXIST \"%dp0%\\node.exe\" (",
        "  SET \"_prog=%dp0%\\node.exe\"",
        ") ELSE (",
        "  SET \"_prog=node\"",
        "  SET PATHEXT=%PATHEXT:;.JS;=;%",
        ")",
        "",
    ];
    (lines.len() == CMD_SHIM_HEAD.len() + BODY.len() + 1).then_some(())?;
    lines[CMD_SHIM_HEAD.len()..]
        .iter()
        .copied()
        .zip(BODY)
        .all(|(line, expected)| line == expected)
        .then_some(())?;
    target_from_line(
        lines.last()?,
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\",
        "\" %*",
    )
    .map(|target| (target, ShimRuntime::Node))
}

fn legacy_seven_line_corepack_target<'a>(lines: &[&'a str]) -> Option<(&'a str, ShimRuntime)> {
    (lines.len() == 7).then_some(())?;
    (lines[0] == "@SETLOCAL").then_some(())?;
    (lines[1] == "@IF EXIST \"%~dp0\\node.exe\" (").then_some(())?;
    (lines[3] == ") ELSE (").then_some(())?;
    (lines[4] == "  @SET PATHEXT=%PATHEXT:;.JS;=;%").then_some(())?;
    (lines[6] == ")").then_some(())?;
    let first = target_from_line(lines[2], "  \"%~dp0\\node.exe\"  \"%~dp0\\", "\" %*")?;
    let second = target_from_line(lines[5], "  node  \"%~dp0\\", "\" %*")?;
    (first == second).then_some((first, ShimRuntime::Node))
}

fn normalized_batch_lines(shim: &str) -> Option<Vec<&str>> {
    let body = shim.strip_suffix("\r\n")?;
    let lines = body.split("\r\n").collect::<Vec<_>>();
    (!lines.iter().any(|line| line.contains(['\r', '\n']))).then_some(lines)
}

fn target_from_line<'a>(line: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?.strip_suffix(suffix)
}

fn native_node(
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, WindowsBatchLaunchError> {
    let path = environment_value(source_env, "PATH").ok_or_else(|| {
        WindowsBatchLaunchError::NodeNotFound("source environment has no PATH".into())
    })?;
    let cwd = absolute_cwd(cwd).map_err(WindowsBatchLaunchError::NodeNotFound)?;
    for directory in std::env::split_paths(path).filter(|path| !path.as_os_str().is_empty()) {
        let has_prefix = directory
            .components()
            .any(|component| matches!(component, Component::Prefix(_)));
        if has_prefix != directory.has_root() {
            continue;
        }
        let node = if has_prefix {
            directory
        } else {
            cwd.join(directory)
        }
        .join("node.exe");
        if node.is_file() {
            let node = node
                .canonicalize()
                .map_err(WindowsBatchLaunchError::CanonicalizeNode)?;
            return native_node_exe(&node)
                .then_some(node)
                .ok_or(WindowsBatchLaunchError::NodeNotNative);
        }
    }
    Err(WindowsBatchLaunchError::NodeNotFound(
        "source PATH contains no node.exe".into(),
    ))
}

fn absolute_cwd(cwd: &Path) -> Result<PathBuf, String> {
    if cwd.is_absolute() {
        Ok(cwd.to_path_buf())
    } else {
        std::env::current_dir()
            .map_err(|err| format!("could not resolve launch cwd: {err}"))
            .map(|current_dir| current_dir.join(cwd))
    }
}

fn environment_value<'a>(
    source_env: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    source_env
        .iter()
        .rfind(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn node_script(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["js", "cjs", "mjs"]
            .iter()
            .any(|suffix| extension.eq_ignore_ascii_case(suffix))
    })
}

fn native_executable(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn native_node_exe(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("node.exe"))
}

fn is_windows_batch_program(program: &Path) -> bool {
    program.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    })
}

/// Errors returned while preparing a Windows native launch plan.
#[derive(Debug, thiserror::Error)]
pub enum WindowsBatchLaunchError {
    #[error("launch programs must be absolute")]
    ProgramNotAbsolute,
    #[error("Windows launch program uses an ambiguous normalized path spelling")]
    AmbiguousProgramPath,
    #[error("could not canonicalize Windows launch program: {0}")]
    CanonicalizeProgram(std::io::Error),
    #[error("canonical Windows launch program is not absolute")]
    CanonicalProgramNotAbsolute,
    #[error(
        "unsupported Windows batch shim; configure a native executable or an exact static cmd-shim form"
    )]
    UnsupportedShim,
    #[error("could not read Windows batch shim: {0}")]
    ReadShim(std::io::Error),
    #[error("batch shim target is not an accepted bounded Windows relative path")]
    InvalidShimTarget,
    #[error("batch shim target does not exist: {}", .target.display())]
    MissingShimTarget { target: PathBuf },
    #[error("could not canonicalize batch shim target: {0}")]
    CanonicalizeTarget(std::io::Error),
    #[error("canonical batch shim target is not absolute")]
    CanonicalTargetNotAbsolute,
    #[error("batch shim target escapes the trusted shim root: {}", .target.display())]
    TargetEscapesShimRoot { target: PathBuf },
    #[error("could not canonicalize node.exe: {0}")]
    CanonicalizeNode(std::io::Error),
    #[error("could not find node.exe for npm batch shim: {0}")]
    NodeNotFound(String),
    #[error("source PATH resolved node to a non-native program")]
    NodeNotNative,
}

#[cfg(test)]
#[path = "windows_cmd_shim_tests.rs"]
mod tests;
