use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// A native invocation containing an absolute program and lossless OS arguments.
///
/// For non-batch inputs this is an exact pass-through; callers are responsible for selecting a
/// directly invokable native program. This staged primitive has no production Claude or ACP
/// consumer yet: OPE2-00126 and OPE2-00127 must adopt it before those launches are protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsNativeLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Prepares a recognized batch shim as a native invocation without `cmd.exe` or `COMSPEC`.
///
/// For a Node shim, only the caller-supplied `source_env` `PATH` is trusted to locate an
/// absolute `node.exe`; there is no sibling, host-environment, or `COMSPEC` fallback. Canonical
/// paths are a local-filesystem snapshot, so callers must revalidate immediately before spawn to
/// retain their reparse/TOCTOU boundary.
pub fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<OsString>,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<WindowsNativeLaunch, WindowsBatchLaunchError> {
    if !is_windows_batch_program(&program) {
        return program
            .is_absolute()
            .then_some(WindowsNativeLaunch { program, args })
            .ok_or(WindowsBatchLaunchError::ProgramNotAbsolute);
    }
    let parent = program
        .parent()
        .ok_or(WindowsBatchLaunchError::UnsupportedShim)?
        .canonicalize()
        .map_err(WindowsBatchLaunchError::CanonicalizeShimDirectory)?;
    let (target, runtime) = shim_target(&program, &parent)?;
    match runtime {
        ShimRuntime::DirectNative => Ok(WindowsNativeLaunch {
            program: target,
            args,
        }),
        ShimRuntime::Node => {
            let mut args = Vec::with_capacity(args.len() + 1);
            args.push(target.into_os_string());
            args.extend(args);
            Ok(WindowsNativeLaunch {
                program: native_node(source_env, cwd)?,
                args,
            })
        }
    }
}

fn shim_target(
    program: &Path,
    canonical_parent: &Path,
) -> Result<(PathBuf, ShimRuntime), WindowsBatchLaunchError> {
    let shim = std::fs::read_to_string(program).map_err(WindowsBatchLaunchError::ReadShim)?;
    let (relative, runtime) =
        recognized_shim(&shim).ok_or(WindowsBatchLaunchError::UnsupportedShim)?;
    let relative = Path::new(relative);
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(WindowsBatchLaunchError::InvalidShimTarget);
    }
    let target = canonical_parent.join(relative);
    if !target.is_file() {
        return Err(WindowsBatchLaunchError::MissingShimTarget { target });
    }
    let target = target
        .canonicalize()
        .map_err(WindowsBatchLaunchError::CanonicalizeTarget)?;
    if !target.starts_with(canonical_parent) {
        return Err(WindowsBatchLaunchError::TargetEscapesShimDirectory { target });
    }
    match runtime {
        ShimRuntime::DirectNative if native_executable(&target) => Ok((target, runtime)),
        ShimRuntime::Node if node_script(&target) => Ok((target, runtime)),
        _ => Err(WindowsBatchLaunchError::UnsupportedShim),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShimRuntime {
    DirectNative,
    Node,
}

fn recognized_shim(shim: &str) -> Option<(&str, ShimRuntime)> {
    let lines = normalized_batch_lines(shim)?;
    npm_cmd_shim_v9_target(&lines)
        .or_else(|| npm_cmd_shim_v8_target(&lines))
        .or_else(|| corepack_cmd_shim_v1_target(&lines))
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

fn npm_cmd_shim_v9_target(lines: &[&str]) -> Option<(&str, ShimRuntime)> {
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

fn npm_cmd_shim_v8_target(lines: &[&str]) -> Option<(&str, ShimRuntime)> {
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
    lines[..CMD_SHIM_HEAD.len()]
        .iter()
        .copied()
        .chain(lines[CMD_SHIM_HEAD.len()..].iter().copied())
        .zip(CMD_SHIM_HEAD.into_iter().chain(BODY))
        .all(|(line, expected)| line == expected)
        .then_some(())?;
    target_from_line(
        lines.last()?,
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\",
        "\" %*",
    )
    .map(|target| (target, ShimRuntime::Node))
}

fn corepack_cmd_shim_v1_target(lines: &[&str]) -> Option<(&str, ShimRuntime)> {
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
    let body = shim
        .strip_suffix('\n')?
        .strip_suffix('\r')
        .unwrap_or(shim.strip_suffix('\n')?);
    let lines = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    (!lines.iter().any(|line| line.contains('\r'))).then_some(lines)
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
        let node = if directory.is_absolute() {
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
    #[error("non-batch launch programs must be absolute")]
    ProgramNotAbsolute,
    #[error(
        "unsupported Windows batch shim; install the agent through npm or configure a native executable"
    )]
    UnsupportedShim,
    #[error("could not read Windows batch shim: {0}")]
    ReadShim(std::io::Error),
    #[error("could not canonicalize batch shim directory: {0}")]
    CanonicalizeShimDirectory(std::io::Error),
    #[error("batch shim target must be a relative path of normal components")]
    InvalidShimTarget,
    #[error("batch shim target does not exist: {}", .target.display())]
    MissingShimTarget { target: PathBuf },
    #[error("could not canonicalize batch shim target: {0}")]
    CanonicalizeTarget(std::io::Error),
    #[error("batch shim target escapes the canonical shim directory: {}", .target.display())]
    TargetEscapesShimDirectory { target: PathBuf },
    #[error("could not canonicalize node.exe: {0}")]
    CanonicalizeNode(std::io::Error),
    #[error("could not find node.exe for npm batch shim: {0}")]
    NodeNotFound(String),
    #[error("source PATH resolved node to a non-native program")]
    NodeNotNative,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn npm_v8(target: &str) -> String {
        format!(
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\{target}\" %*\r\n"
        )
    }

    // Pinned from cmd-shim@9.0.2 lib/index.js with no shebang args or variables.
    fn npm_v9_node(target: &str) -> String {
        format!(
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & set PATHEXT=%PATHEXT:;.JS;=;% & \"%_prog%\"  \"%dp0%\\{target}\" %*\r\n"
        )
    }

    // Pinned from cmd-shim@9.0.2's no-shebang direct executable form.
    fn npm_v9_direct(target: &str) -> String {
        format!(
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\{target}\"   %*\r\n"
        )
    }

    fn corepack(target: &str) -> String {
        format!(
            "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\{target}\" %*\r\n) ELSE (\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\{target}\" %*\r\n)\r\n"
        )
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture");
        std::fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn recognizes_pinned_generators_and_rejects_rewrites() {
        let node = "node_modules\\agent\\bin\\agent.js";
        let claude = "node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe";
        assert_eq!(
            recognized_shim(&npm_v8(node)),
            Some((node, ShimRuntime::Node))
        );
        assert_eq!(
            recognized_shim(&npm_v9_node(node)),
            Some((node, ShimRuntime::Node))
        );
        assert_eq!(
            recognized_shim(&npm_v9_direct(claude)),
            Some((claude, ShimRuntime::DirectNative))
        );
        assert_eq!(
            recognized_shim(&corepack(node)),
            Some((node, ShimRuntime::Node))
        );
        assert_eq!(
            recognized_shim(&npm_v9_node(node).replace("@ECHO off", "@ECHO off ")),
            None
        );
        assert_eq!(
            recognized_shim(&npm_v9_node(node).replace("\"%_prog%\"  ", "\"%_prog%\" --inspect ")),
            None
        );
    }

    #[test]
    fn direct_claude_shim_and_non_batch_contract_are_native() {
        let temp = tempfile::tempdir().expect("tempdir");
        let shim = temp.path().join("claude.cmd");
        let target = temp
            .path()
            .join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
        write(
            &shim,
            &npm_v9_direct("node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe"),
        );
        write(&target, "fixture");
        let args = vec![OsString::from("--resume"), OsString::from("task")];
        let launch = prepare_windows_batch_launch_from_source_env(
            shim,
            args.clone(),
            &BTreeMap::new(),
            temp.path(),
        )
        .expect("direct plan");
        assert_eq!(
            launch,
            WindowsNativeLaunch {
                program: target.canonicalize().expect("canonical target"),
                args
            }
        );
        assert!(matches!(
            prepare_windows_batch_launch_from_source_env(
                PathBuf::from("agent.exe"),
                Vec::new(),
                &BTreeMap::new(),
                temp.path()
            ),
            Err(WindowsBatchLaunchError::ProgramNotAbsolute)
        ));
    }

    #[test]
    fn rejects_noncanonical_targets_before_filesystem_access() {
        let temp = tempfile::tempdir().expect("tempdir");
        for target in [
            ".\\agent.js",
            "..\\agent.js",
            "node_modules\\..\\agent.js",
            "C:\\agent.js",
        ] {
            let shim = temp.path().join(format!("{}.cmd", target.len()));
            write(&shim, &npm_v9_node(target));
            let error = prepare_windows_batch_launch_from_source_env(
                shim,
                Vec::new(),
                &BTreeMap::new(),
                temp.path(),
            )
            .expect_err("reject non-normal target");
            assert!(matches!(error, WindowsBatchLaunchError::InvalidShimTarget));
        }
    }

    #[test]
    fn source_path_node_keeps_multiline_os_argv_and_ignores_comspec() {
        let temp = tempfile::tempdir().expect("tempdir");
        let shim = temp.path().join("agent.cmd");
        let target = temp.path().join("agent.js");
        let node = temp.path().join("trusted-node/node.EXE");
        write(&shim, &npm_v9_node("agent.js"));
        write(&target, "fixture");
        write(&node, "fixture");
        assert!(!temp.path().join("node.exe").exists());
        let args = [
            "",
            "quotes: \"double\" and 'single'",
            "&|<>()^",
            "%PERCENT%",
            "!BANG!",
            "first\r\nsecond",
        ]
        .map(OsString::from)
        .to_vec();
        let source_env = BTreeMap::from([
            (
                "PATH".into(),
                temp.path().join("trusted-node").display().to_string(),
            ),
            (
                "COMSPEC".into(),
                temp.path().join("poisoned.cmd").display().to_string(),
            ),
        ]);
        let launch = prepare_windows_batch_launch_from_source_env(
            shim,
            args.clone(),
            &source_env,
            temp.path(),
        )
        .expect("source PATH plan");
        let mut expected = vec![
            target
                .canonicalize()
                .expect("canonical target")
                .into_os_string(),
        ];
        expected.extend(args);
        assert_eq!(launch.args, expected);
        assert_eq!(launch.program, node.canonicalize().expect("canonical node"));
        assert_ne!(launch.program, temp.path().join("poisoned.cmd"));
    }
}
