//! Windows npm batch-shim parsing for external-agent launches.
//!
//! Batch files do not expose a safe general argv boundary because `cmd.exe`
//! can parse expanded text as command syntax. This module recognizes only the
//! stable Node/npm shim grammars below, extracts their static JavaScript target
//! as data, and invokes `node.exe` through the ordinary process-argument API.
//! Unknown shims fail closed rather than being executed by `cmd.exe`.

#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::path::Component;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use crate::windows_command::resolve_windows_program_from_source_env;

/// Converts a standard Node/npm `.cmd` or `.bat` shim into a native launch.
#[cfg(windows)]
pub(crate) fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<String>,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<(PathBuf, Vec<String>), WindowsBatchLaunchError> {
    if !is_windows_batch_program(&program) {
        return Ok((program, args));
    }

    validate_windows_batch_command_component("program", program.to_string_lossy().as_ref())?;
    for argument in &args {
        validate_windows_batch_command_component("argument", argument)?;
    }

    let target = npm_node_shim_target(&program)?;
    let node = native_node_from_shim(&program, source_env, cwd)?;
    let mut native_args = Vec::with_capacity(args.len() + 1);
    native_args.push(target.to_string_lossy().into_owned());
    native_args.extend(args);
    Ok((node, native_args))
}

#[cfg(windows)]
fn npm_node_shim_target(program: &Path) -> Result<PathBuf, WindowsBatchLaunchError> {
    let shim = std::fs::read_to_string(program).map_err(WindowsBatchLaunchError::ReadShim)?;
    let target = npm_cmd_shim_v1_target(&shim)
        .or_else(|| corepack_cmd_shim_v1_target(&shim))
        .ok_or(WindowsBatchLaunchError::UnsupportedShim)?;
    let target_path = Path::new(target);
    if target_path.is_absolute()
        || target_path
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        return Err(WindowsBatchLaunchError::UnsupportedShim);
    }

    let target = program
        .parent()
        .expect("batch program path has a parent")
        .join(target_path);
    if !is_node_script(&target) || !target.is_file() {
        return Err(WindowsBatchLaunchError::MissingShimTarget { target });
    }
    target.canonicalize().map_err(WindowsBatchLaunchError::CanonicalizeTarget)
}

#[cfg(any(windows, test))]
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

    let mut lines = shim.lines();
    for expected in HEADER {
        (lines.next()? == expected).then_some(())?;
    }
    lines.find_map(|line| {
        line.strip_prefix("endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & ")
            .and_then(|line| npm_node_invocation_target(line, "\"%_prog%\""))
    })
}

#[cfg(any(windows, test))]
fn corepack_cmd_shim_v1_target(shim: &str) -> Option<&str> {
    let lines = shim.lines().collect::<Vec<_>>();
    (lines.len() == 5).then_some(())?;
    (lines[0] == "@IF EXIST \"%~dp0\\node.exe\" (").then_some(())?;
    (lines[2] == ") ELSE (").then_some(())?;
    (lines[4] == ")").then_some(())?;
    let first = npm_node_invocation_target(lines[1].trim(), "\"%~dp0\\node.exe\"")?;
    let second = npm_node_invocation_target(lines[3].trim(), "node")?;
    (first == second).then_some(first)
}

#[cfg(any(windows, test))]
fn npm_node_invocation_target<'a>(line: &'a str, interpreter: &str) -> Option<&'a str> {
    let target = line.strip_prefix(interpreter)?.trim_start();
    ["\"%dp0%\\", "\"%~dp0\\"]
        .into_iter()
        .find_map(|prefix| {
            let target = target.strip_prefix(prefix)?;
            let (target, suffix) = target.split_once('"')?;
            (suffix.trim() == "%*").then_some(target)
        })
}

#[cfg(windows)]
fn native_node_from_shim(
    program: &Path,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, WindowsBatchLaunchError> {
    let sibling = program
        .parent()
        .expect("batch program path has a parent")
        .join("node.exe");
    if sibling.is_file() {
        return sibling
            .canonicalize()
            .map_err(WindowsBatchLaunchError::CanonicalizeNode);
    }

    let node = resolve_windows_program_from_source_env("node", source_env, cwd)
        .map_err(WindowsBatchLaunchError::NodeNotFound)?;
    is_native_node_exe(&node)
        .then_some(node)
        .ok_or(WindowsBatchLaunchError::NodeNotNative)
}

#[cfg(windows)]
fn is_node_script(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "js" | "cjs" | "mjs"
        )
    })
}

#[cfg(windows)]
fn is_native_node_exe(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("node.exe"))
}

/// Rejects physical command-line boundaries before a batch shim is parsed.
#[cfg(windows)]
fn validate_windows_batch_command_component(
    component: &'static str,
    value: &str,
) -> Result<(), WindowsBatchLaunchError> {
    if value.contains(['\r', '\n']) {
        return Err(WindowsBatchLaunchError::LineBreak { component });
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn is_windows_batch_program(program: &Path) -> bool {
    program
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

#[cfg(windows)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum WindowsBatchLaunchError {
    #[error("Windows batch launch rejects {component} containing CR or LF")]
    LineBreak { component: &'static str },
    #[error(
        "unsupported Windows batch shim; install the agent through npm or configure a native executable"
    )]
    UnsupportedShim,
    #[error("could not read npm batch shim: {0}")]
    ReadShim(std::io::Error),
    #[error("npm batch shim JavaScript target does not exist: {}", .target.display())]
    MissingShimTarget { target: PathBuf },
    #[error("could not canonicalize npm batch shim JavaScript target: {0}")]
    CanonicalizeTarget(std::io::Error),
    #[error("could not canonicalize npm shim node.exe: {0}")]
    CanonicalizeNode(std::io::Error),
    #[error("could not find node.exe for npm batch shim: {0}")]
    NodeNotFound(String),
    #[error("npm batch shim resolved node to a non-native program")]
    NodeNotNative,
}

// The npm/corepack shim grammar is pure text parsing that is identical on every
// host, so it is verified on all platforms even though the launch it prepares is
// Windows-only. This keeps the exact-forwarding contract (recognize only the
// known safe passthrough shims, reject anything that could rewrite arguments)
// covered by the Linux CI job as well as the Windows one.
#[cfg(test)]
mod grammar_tests {
    use super::corepack_cmd_shim_v1_target;
    use super::npm_cmd_shim_v1_target;
    use pretty_assertions::assert_eq;

    #[test]
    fn recognizes_cmd_shim_v1_for_claude_cursor_grok_and_npm() {
        for target in [
            "node_modules\\@anthropic-ai\\claude-code\\cli.js",
            "node_modules\\@cursor\\agent\\bin\\agent.js",
            "node_modules\\@xai\\grok\\bin\\grok.js",
            "node_modules\\npm\\bin\\npm-cli.js",
        ] {
            let shim = format!(
                "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\nSET \"_prog=node\"\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\{target}\" %*\r\n"
            );
            assert_eq!(npm_cmd_shim_v1_target(&shim), Some(target));
        }
    }

    #[test]
    fn recognizes_corepack_cmd_shim_v1() {
        let shim = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\..\\dist\\npm.js\" %*\r\n) ELSE (\r\n  node  \"%~dp0\\..\\dist\\npm.js\" %*\r\n)\r\n";
        assert_eq!(corepack_cmd_shim_v1_target(shim), Some("..\\dist\\npm.js"));
    }

    #[test]
    fn rejects_unknown_or_argument_rewriting_batch_shims() {
        for shim in [
            "@echo off\r\nnode \"%~dp0\\agent.js\" %*\r\n",
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" --inspect \"%dp0%\\agent.js\" %*\r\n",
            "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\" \"%~dp0\\agent.js\" %*\r\n) ELSE (\r\n  node \"%~dp0\\other.js\" %*\r\n)\r\n",
        ] {
            assert_eq!(npm_cmd_shim_v1_target(shim), None);
            assert_eq!(corepack_cmd_shim_v1_target(shim), None);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use tokio::process::Command;

    fn write_npm_cmd_shim_v1(path: &Path, target: &str) {
        std::fs::write(
            path,
            format!(
                "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\nSET \"_prog=node\"\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\{target}\" %*\r\n"
            ),
        )
        .expect("write npm cmd-shim v1 fixture");
    }

    fn source_environment() -> BTreeMap<String, String> {
        let path = std::env::var("PATH").expect("Windows CI supplies PATH with node.exe");
        let mut environment = std::env::vars().collect::<BTreeMap<_, _>>();
        environment.retain(|key, _| !key.eq_ignore_ascii_case("PATH"));
        environment.retain(|key, _| !key.eq_ignore_ascii_case("PATHEXT"));
        environment.insert("pAtH".to_string(), path);
        environment.insert("PaThExT".to_string(), ".EXE;.CMD".to_string());
        environment
    }

    #[test]
    fn unknown_batch_shims_fail_closed_with_remediation() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("unknown.cmd");
        std::fs::write(
            &shim,
            "@echo off\r\nset \"ARG1=%~1\"\r\nshift /1\r\ncall :launch %*\r\n",
        )
        .expect("write unknown batch shim");

        let error = prepare_windows_batch_launch_from_source_env(
            shim,
            vec!["safe argument".to_string()],
            &BTreeMap::new(),
            temp_dir.path(),
        )
        .expect_err("arbitrary batch files must not be launched through cmd.exe");

        assert!(matches!(error, WindowsBatchLaunchError::UnsupportedShim));
        assert!(error.to_string().contains("configure a native executable"));
    }

    #[tokio::test]
    async fn standard_npm_shim_forwards_exact_hostile_argv_without_cmd_exe() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("capture.cmd");
        let target = temp_dir.path().join("capture.js");
        let capture = temp_dir.path().join("captured.json");
        let marker = temp_dir.path().join("injected.txt");
        let poisoned_comspec = temp_dir.path().join("poisoned-cmd.exe");
        write_npm_cmd_shim_v1(&shim, "capture.js");
        std::fs::write(
            &target,
            r#"require("fs").writeFileSync(process.env.CODEWITH_BATCH_CAPTURE, JSON.stringify(process.argv.slice(2)));
"#,
        )
        .expect("write JavaScript capture target");
        let hostile_args = vec![
            "spaces stay one argument".to_string(),
            format!(
                "embedded \" & type nul > \"{}\" & rem",
                marker.display()
            ),
            "pipe | command".to_string(),
            "input < source".to_string(),
            "output > destination".to_string(),
            "parentheses ( ) literal".to_string(),
            "caret ^ literal".to_string(),
            "%CODEWITH_BATCH_TEST_PERCENT%".to_string(),
            "!CODEWITH_BATCH_TEST_BANG!".to_string(),
            String::new(),
            "eleventh argument survives too".to_string(),
        ];
        let mut source_env = source_environment();
        source_env.insert(
            "cOmSpEc".to_string(),
            poisoned_comspec.display().to_string(),
        );
        let (program, args) = prepare_windows_batch_launch_from_source_env(
            shim,
            hostile_args.clone(),
            &source_env,
            temp_dir.path(),
        )
        .expect("recognized npm shim should prepare a native node launch");

        assert_eq!(
            program.file_name().and_then(|name| name.to_str()),
            Some("node.exe")
        );
        assert_ne!(program, poisoned_comspec);
        assert_eq!(args.first(), Some(&target.canonicalize().expect("target path").display().to_string()));
        assert!(!args.iter().any(|arg| matches!(arg.as_str(), "/c" | "/v:on")));
        let status = Command::new(program)
            .args(&args)
            .envs(source_env)
            .env("CODEWITH_BATCH_CAPTURE", &capture)
            .env("CODEWITH_BATCH_TEST_PERCENT", "expanded-in-test")
            .env("CODEWITH_BATCH_TEST_BANG", "expanded-in-test")
            .status()
            .await
            .expect("launch native node target");
        assert!(status.success());
        assert!(
            !marker.exists(),
            "hostile argv must not be reparsed as a batch command"
        );
        let received = serde_json::from_str::<Vec<String>>(
            &std::fs::read_to_string(capture).expect("read captured argv"),
        )
        .expect("capture target should serialize argv as JSON");
        assert_eq!(received, hostile_args);
    }

    #[test]
    fn batch_launch_rejects_line_break_arguments_before_parsing_the_shim() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("capture.cmd");
        std::fs::write(&shim, "@echo off\r\nexit /b 0\r\n").expect("write batch shim");

        for line_break in ["\r", "\n", "\r\n"] {
            let error = prepare_windows_batch_launch_from_source_env(
                shim.clone(),
                vec![format!("task{line_break}next")],
                &BTreeMap::new(),
                temp_dir.path(),
            )
            .expect_err("line breaks must be rejected before a batch shim is inspected");
            assert!(matches!(
                error,
                WindowsBatchLaunchError::LineBreak {
                    component: "argument"
                }
            ));
        }
    }

    #[test]
    fn rejects_cr_lf_and_crlf_without_rewriting_them() {
        for value in ["task\rnext", "task\nnext", "task\r\nnext"] {
            assert!(matches!(
                validate_windows_batch_command_component("argument", value),
                Err(WindowsBatchLaunchError::LineBreak {
                    component: "argument"
                })
            ));
        }
    }
}
