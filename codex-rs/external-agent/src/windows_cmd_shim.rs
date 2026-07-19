use std::collections::BTreeMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
/// A native Windows process invocation prepared without invoking a batch shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsNativeLaunch {
    /// Absolute native program that a caller can pass to [`std::process::Command::new`].
    pub program: PathBuf,
    /// Arguments, in order, for [`std::process::Command::args`].
    pub args: Vec<String>,
}
/// Resolves `program` from source `PATH` and `PATHEXT`, resolving relative entries from `cwd`.
fn resolve_windows_program_from_source_env(
    program: &str,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let path = environment_value(source_env, "PATH")
        .ok_or_else(|| format!("source environment does not define PATH for `{program}`"))?;
    let extensions = environment_value(source_env, "PATHEXT")
        .map(String::as_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter(valid_pathext)
        .collect::<Vec<_>>();
    let cwd = absolute_cwd(cwd, program)?;
    let requested = Path::new(program);
    let candidates = if program.contains(['/', '\\']) {
        vec![if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            cwd.join(requested)
        }]
    } else {
        std::env::split_paths(path)
            .filter(|entry| !entry.as_os_str().is_empty())
            .map(|entry| {
                if entry.is_absolute() {
                    entry
                } else {
                    cwd.join(entry)
                }
                .join(requested)
            })
            .collect()
    };

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
        if candidate.extension().is_none() {
            for extension in &extensions {
                let candidate = PathBuf::from(format!("{}{}", candidate.display(), extension));
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(format!(
        "could not resolve `{program}` from source PATH using source PATHEXT"
    ))
}
/// Prepares a native plan without `cmd.exe`, `COMSPEC`, delayed expansion, or transport env.
/// It rejects existing reparse escapes; callers must trust the local filesystem and revalidate before spawn.
pub fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<String>,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<WindowsNativeLaunch, WindowsBatchLaunchError> {
    if !is_windows_batch_program(&program) {
        return Ok(WindowsNativeLaunch { program, args });
    }
    validate_component("program", program.to_string_lossy().as_ref())?;
    for argument in &args {
        validate_component("argument", argument)?;
    }
    let parent = program
        .parent()
        .ok_or(WindowsBatchLaunchError::UnsupportedShim)?
        .canonicalize()
        .map_err(WindowsBatchLaunchError::CanonicalizeShimDirectory)?;
    let target = shim_target(&program, &parent)?;
    if native_executable(&target) {
        return Ok(WindowsNativeLaunch {
            program: target,
            args,
        });
    }
    let node = native_node(&parent, source_env, cwd)?;
    let mut native_args = Vec::with_capacity(args.len() + 1);
    native_args.push(target.to_string_lossy().into_owned());
    native_args.extend(args);
    Ok(WindowsNativeLaunch {
        program: node,
        args: native_args,
    })
}
fn shim_target(
    program: &Path,
    canonical_parent: &Path,
) -> Result<PathBuf, WindowsBatchLaunchError> {
    let shim = std::fs::read_to_string(program).map_err(WindowsBatchLaunchError::ReadShim)?;
    let relative = npm_cmd_shim_v1_target(&shim)
        .or_else(|| corepack_cmd_shim_v1_target(&shim))
        .ok_or(WindowsBatchLaunchError::UnsupportedShim)?;
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
    if !node_script(&target) && !native_executable(&target) {
        Err(WindowsBatchLaunchError::UnsupportedShim)
    } else {
        Ok(target)
    }
}
fn npm_cmd_shim_v1_target(shim: &str) -> Option<&str> {
    const HEADER: [&str; 8] = [
        "@ECHO off",
        "GOTO start",
        ":find_dp0",
        "SET dp0=%~dp0",
        "EXIT /b",
        ":start",
        "SETLOCAL",
        "CALL :find_dp0",
    ];
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
    let lines = normalized_batch_lines(shim)?;
    (lines.len() == HEADER.len() + BODY.len() + 1).then_some(())?;
    for (line, expected) in lines.iter().zip(HEADER.into_iter().chain(BODY)) {
        (*line == expected).then_some(())?;
    }
    target_from_line(
        lines.last()?,
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\",
    )
}
fn corepack_cmd_shim_v1_target(shim: &str) -> Option<&str> {
    let lines = normalized_batch_lines(shim)?;
    (lines.len() == 7).then_some(())?;
    (lines[0] == "@SETLOCAL").then_some(())?;
    (lines[1] == "@IF EXIST \"%~dp0\\node.exe\" (").then_some(())?;
    (lines[3] == ") ELSE (").then_some(())?;
    (lines[4] == "  @SET PATHEXT=%PATHEXT:;.JS;=;%").then_some(())?;
    (lines[6] == ")").then_some(())?;
    let first = target_from_line(lines[2], "  \"%~dp0\\node.exe\"  \"%~dp0\\")?;
    let second = target_from_line(lines[5], "  node  \"%~dp0\\")?;
    (first == second).then_some(first)
}
fn normalized_batch_lines(shim: &str) -> Option<Vec<&str>> {
    let body = shim.strip_suffix('\n')?;
    let body = body.strip_suffix('\r').unwrap_or(body);
    let lines = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    (!lines.iter().any(|line| line.contains('\r'))).then_some(lines)
}
fn target_from_line<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?.strip_suffix("\" %*")
}
fn native_node(
    canonical_parent: &Path,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, WindowsBatchLaunchError> {
    let sibling = canonical_parent.join("node.exe");
    if sibling.is_file() {
        let sibling = sibling
            .canonicalize()
            .map_err(WindowsBatchLaunchError::CanonicalizeNode)?;
        if !sibling.starts_with(canonical_parent) {
            return Err(WindowsBatchLaunchError::TargetEscapesShimDirectory { target: sibling });
        }
        return Ok(sibling);
    }
    let node = resolve_windows_program_from_source_env("node", source_env, cwd)
        .map_err(WindowsBatchLaunchError::NodeNotFound)?;
    native_node_exe(&node)
        .then_some(node)
        .ok_or(WindowsBatchLaunchError::NodeNotNative)
}
fn absolute_cwd(cwd: &Path, program: &str) -> Result<PathBuf, String> {
    if cwd.is_absolute() {
        Ok(cwd.to_path_buf())
    } else {
        std::env::current_dir()
            .map_err(|err| format!("could not resolve launch cwd for `{program}`: {err}"))
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
fn valid_pathext(extension: &&str) -> bool {
    extension.len() > 1
        && extension.starts_with('.')
        && !extension.ends_with(['.', ' '])
        && !extension.contains(['/', '\\'])
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
fn validate_component(component: &'static str, value: &str) -> Result<(), WindowsBatchLaunchError> {
    if value.contains(['\r', '\n']) {
        return Err(WindowsBatchLaunchError::LineBreak { component });
    }
    Ok(())
}
/// Errors returned while preparing a Windows native launch plan.
#[derive(Debug, thiserror::Error)]
pub enum WindowsBatchLaunchError {
    #[error("Windows batch launch rejects {component} containing CR or LF")]
    LineBreak { component: &'static str },
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
    #[error("could not canonicalize npm shim node.exe: {0}")]
    CanonicalizeNode(std::io::Error),
    #[error("could not find node.exe for npm batch shim: {0}")]
    NodeNotFound(String),
    #[error("npm batch shim resolved node to a non-native program")]
    NodeNotNative,
}
#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // Derived from cmd-shim v8's writeShim output with the Node program and no extra args.
    fn npm_cmd_shim_output(target: &str) -> String {
        format!(
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\{target}\" %*\r\n"
        )
    }

    // Derived from Corepack's generateCmdShim longProg template with no optional paths or args.
    fn corepack_cmd_shim_output(target: &str) -> String {
        format!(
            "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\{target}\" %*\r\n) ELSE (\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\{target}\" %*\r\n)\r\n"
        )
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture");
        std::fs::write(path, contents).expect("write fixture");
    }

    fn write_npm_shim(path: &Path, target: &str) {
        write(path, &npm_cmd_shim_output(target));
    }

    #[test]
    fn resolves_windows_paths_case_insensitively() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let cwd = temp_dir.path().join("launch-cwd");
        let program = cwd.join("relative-bin").join("agent.cmd");
        write(&program, "not executed");
        let source_env = BTreeMap::from([
            ("pAtH".to_string(), "relative-bin".to_string()),
            ("PaThExT".to_string(), ".CMD".to_string()),
        ]);

        let resolved = resolve_windows_program_from_source_env("agent", &source_env, &cwd)
            .expect("resolve relative PATH from launch cwd");

        assert!(
            resolved
                .as_os_str()
                .eq_ignore_ascii_case(program.as_os_str())
        );
        assert!(resolved.is_absolute());
    }

    #[test]
    fn accepts_exact_npm_and_corepack_generators() {
        let npm_target = "node_modules\\@cursor\\agent\\bin\\agent.js";
        let corepack_target = "node_modules\\npm\\bin\\npm-cli.js";
        assert_eq!(
            npm_cmd_shim_v1_target(&npm_cmd_shim_output(npm_target)),
            Some(npm_target)
        );
        assert_eq!(
            corepack_cmd_shim_v1_target(&corepack_cmd_shim_output(corepack_target)),
            Some(corepack_target)
        );
    }

    #[test]
    fn rejects_unknown_rewriting_and_noncanonical_whitespace() {
        let target = "agent.js";
        let npm = npm_cmd_shim_output(target);
        let corepack = corepack_cmd_shim_output(target);
        let invalid_npm = [
            "@echo off\r\ncall :launch %*\r\n".to_string(),
            npm.replace(
                "\"%_prog%\" \"%dp0%\\agent.js\"",
                "\"%_prog%\" --inspect \"%dp0%\\agent.js\"",
            ),
            npm.replace("@ECHO off", "@ECHO off "),
        ];
        for shim in invalid_npm {
            assert_eq!(npm_cmd_shim_v1_target(&shim), None);
        }
        assert_eq!(
            corepack_cmd_shim_v1_target(&corepack.replace("  node  ", " node  ")),
            None
        );
    }

    #[test]
    fn launches_claude_direct_executable_without_node_or_a_shell() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("claude.cmd");
        let target = temp_dir
            .path()
            .join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
        write_npm_shim(
            &shim,
            "node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe",
        );
        write(&target, "fixture");
        let args = vec!["--resume".to_string(), "task".to_string()];

        let launch = prepare_windows_batch_launch_from_source_env(
            shim,
            args.clone(),
            &BTreeMap::new(),
            temp_dir.path(),
        )
        .expect("Claude executable shim should produce a direct plan");

        assert_eq!(
            launch.program,
            target.canonicalize().expect("canonical executable")
        );
        assert_eq!(launch.args, args);
    }

    #[test]
    fn rejects_current_parent_and_absolute_target_components() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        for target in [
            ".\\agent.js",
            "..\\agent.js",
            "node_modules\\..\\agent.js",
            "C:\\agent.js",
        ] {
            let shim = temp_dir.path().join(format!("{}.cmd", target.len()));
            write_npm_shim(&shim, target);
            let error = prepare_windows_batch_launch_from_source_env(
                shim,
                Vec::new(),
                &BTreeMap::new(),
                temp_dir.path(),
            )
            .expect_err("non-normal target must fail before filesystem access");
            assert!(matches!(error, WindowsBatchLaunchError::InvalidShimTarget));
        }
    }

    #[test]
    fn poisoned_path_comspec_and_hostile_argv_stay_data_in_the_native_plan() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("agent.cmd");
        let target = temp_dir.path().join("agent.js");
        let node = temp_dir.path().join("node.exe");
        write_npm_shim(&shim, "agent.js");
        write(&target, "// fixture");
        write(&node, "fixture");
        let args = [
            "",
            "quotes: \"double\" and 'single'",
            "&|<>()^",
            "%PERCENT%",
            "!BANG!",
            "one",
            "two",
            "three",
            "four",
            "five",
            "six",
        ]
        .map(String::from)
        .to_vec();
        let source_env = BTreeMap::from([
            (
                "PATH".to_string(),
                temp_dir.path().join("poisoned").display().to_string(),
            ),
            (
                "COMSPEC".to_string(),
                temp_dir.path().join("poisoned.cmd").display().to_string(),
            ),
        ]);

        let launch = prepare_windows_batch_launch_from_source_env(
            shim,
            args.clone(),
            &source_env,
            temp_dir.path(),
        )
        .expect("sibling node.exe should make a native plan");

        assert!(
            launch
                .program
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("node.exe"))
        );
        assert_ne!(launch.program, temp_dir.path().join("poisoned.cmd"));
        let mut expected = vec![
            target
                .canonicalize()
                .expect("canonical target")
                .display()
                .to_string(),
        ];
        expected.extend(args);
        assert_eq!(launch.args, expected);
    }
}
