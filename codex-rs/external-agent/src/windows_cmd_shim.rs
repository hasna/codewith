use std::collections::BTreeMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsNativeLaunch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
}
/// Collapses Windows environment keys; overrides win case-insensitively.
pub(crate) fn merge_windows_environment(
    source: &BTreeMap<String, String>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    for (name, value) in source {
        environment.insert(name.to_ascii_uppercase(), value.clone());
    }
    for (name, value) in overrides {
        environment.insert(name.to_ascii_uppercase(), value.clone());
    }
    environment
}
/// Resolves an absolute Windows command path from source env and launch CWD.
pub(crate) fn resolve_windows_program_from_source_env(
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
        .filter(|extension| valid_pathext(extension))
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

pub(crate) fn prepare_windows_batch_launch_from_source_env(
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
    let target = shim_target(&program)?;
    let node = native_node(&program, source_env, cwd)?;
    let mut native_args = Vec::with_capacity(args.len() + 1);
    native_args.push(target.to_string_lossy().into_owned());
    native_args.extend(args);
    Ok(WindowsNativeLaunch {
        program: node,
        args: native_args,
    })
}

fn shim_target(program: &Path) -> Result<PathBuf, WindowsBatchLaunchError> {
    let shim = std::fs::read_to_string(program).map_err(WindowsBatchLaunchError::ReadShim)?;
    let relative = npm_cmd_shim_v1_target(&shim)
        .or_else(|| corepack_cmd_shim_v1_target(&shim))
        .ok_or(WindowsBatchLaunchError::UnsupportedShim)?;
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
    {
        return Err(WindowsBatchLaunchError::UnsupportedShim);
    }
    let target = program
        .parent()
        .expect("batch program paths have a parent")
        .join(relative);
    if !node_script(&target) || !target.is_file() {
        return Err(WindowsBatchLaunchError::MissingShimTarget { target });
    }
    target
        .canonicalize()
        .map_err(WindowsBatchLaunchError::CanonicalizeTarget)
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
    const NODE_BODY: [&str; 8] = [
        "",
        "IF EXIST \"%dp0%\\node.exe\" (",
        "  SET \"_prog=%dp0%\\node.exe\"",
        ") ELSE (",
        "  SET \"_prog=node\"",
        "  SET PATHEXT=%PATHEXT:;.JS;=%",
        ")",
        "",
    ];
    let lines = shim.lines().collect::<Vec<_>>();
    (lines.len() == HEADER.len() + NODE_BODY.len() + 1).then_some(())?;
    for (index, expected) in HEADER.into_iter().enumerate() {
        (lines[index] == expected).then_some(())?;
    }
    for (index, expected) in NODE_BODY.into_iter().enumerate() {
        (lines[HEADER.len() + index] == expected).then_some(())?;
    }
    lines
        .last()?
        .strip_prefix("endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & ")
        .and_then(|line| node_invocation_target(line, "\"%_prog%\""))
}

fn corepack_cmd_shim_v1_target(shim: &str) -> Option<&str> {
    let lines = shim.lines().collect::<Vec<_>>();
    (lines.len() == 5).then_some(())?;
    (lines[0] == "@IF EXIST \"%~dp0\\node.exe\" (").then_some(())?;
    (lines[2] == ") ELSE (").then_some(())?;
    (lines[4] == ")").then_some(())?;
    let first = node_invocation_target(lines[1].trim(), "\"%~dp0\\node.exe\"")?;
    let second = node_invocation_target(lines[3].trim(), "node")?;
    (first == second).then_some(first)
}

fn node_invocation_target<'a>(line: &'a str, interpreter: &str) -> Option<&'a str> {
    let target = line.strip_prefix(interpreter)?.trim_start();
    ["\"%dp0%\\", "\"%~dp0\\"].into_iter().find_map(|prefix| {
        let target = target.strip_prefix(prefix)?;
        let (target, suffix) = target.split_once('"')?;
        (suffix.trim() == "%*").then_some(target)
    })
}

fn native_node(
    shim: &Path,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, WindowsBatchLaunchError> {
    let sibling = shim
        .parent()
        .expect("batch program paths have a parent")
        .join("node.exe");
    if sibling.is_file() {
        return sibling
            .canonicalize()
            .map_err(WindowsBatchLaunchError::CanonicalizeNode);
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
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "cjs" | "mjs"
            )
        })
}

fn native_node_exe(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("node.exe"))
}

pub(crate) fn is_windows_batch_program(program: &Path) -> bool {
    program
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

fn validate_component(component: &'static str, value: &str) -> Result<(), WindowsBatchLaunchError> {
    if value.contains(['\r', '\n']) {
        return Err(WindowsBatchLaunchError::LineBreak { component });
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tokio::process::Command;

    fn npm_shim_source(target: &str) -> String {
        format!(
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\{target}\" %*\r\n"
        )
    }

    fn npm_shim(path: &Path, target: &str) {
        std::fs::write(path, npm_shim_source(target)).expect("write npm cmd shim");
    }

    #[test]
    fn merges_environment_case_insensitively() {
        let source = BTreeMap::from([
            ("Path".to_string(), "source-path".to_string()),
            ("PathExt".to_string(), ".CMD".to_string()),
        ]);
        let overrides = BTreeMap::from([("PATH".to_string(), "override-path".to_string())]);
        assert_eq!(
            merge_windows_environment(&source, &overrides),
            BTreeMap::from([
                ("PATH".to_string(), "override-path".to_string()),
                ("PATHEXT".to_string(), ".CMD".to_string()),
            ])
        );
    }

    #[test]
    fn resolves_relative_path_entries_from_launch_cwd_as_absolute_paths() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let cwd = temp_dir.path().join("launch-cwd");
        let bin = cwd.join("relative-bin");
        std::fs::create_dir_all(&bin).expect("create relative PATH entry");
        let program = bin.join("agent.cmd");
        std::fs::write(&program, "not executed").expect("write candidate");
        let source_env = BTreeMap::from([
            ("pAtH".to_string(), "relative-bin".to_string()),
            ("PaThExT".to_string(), ".CMD".to_string()),
        ]);

        let resolved = resolve_windows_program_from_source_env("agent", &source_env, &cwd)
            .expect("resolve relative PATH from launch cwd");

        assert_eq!(resolved, program);
        assert!(resolved.is_absolute());
    }

    #[test]
    fn recognizes_npm_shims_for_claude_cursor_grok_and_npm() {
        for target in [
            "node_modules\\@anthropic-ai\\claude-code\\cli.js",
            "node_modules\\@cursor\\agent\\bin\\agent.js",
            "node_modules\\@xai\\grok\\bin\\grok.js",
            "node_modules\\npm\\bin\\npm-cli.js",
        ] {
            let shim = npm_shim_source(target);
            assert_eq!(npm_cmd_shim_v1_target(&shim), Some(target));
        }
    }

    #[test]
    fn rejects_unknown_or_argument_rewriting_batch_shims() {
        let unknown = "@echo off\r\nset \"ARG1=%~1\"\r\nshift /1\r\ncall :launch %*\r\n";
        let rewritten = npm_shim_source("agent.js").replace(
            "\"%_prog%\" \"%dp0%\\agent.js\"",
            "\"%_prog%\" --inspect \"%dp0%\\agent.js\"",
        );
        assert_eq!(npm_cmd_shim_v1_target(unknown), None);
        assert_eq!(corepack_cmd_shim_v1_target(&rewritten), None);
        assert_eq!(npm_cmd_shim_v1_target(&rewritten), None);
    }

    #[test]
    fn unknown_batch_shims_fail_closed_with_remediation() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("unknown.cmd");
        std::fs::write(&shim, "@echo off\r\nshift /1\r\ncall :launch %*\r\n")
            .expect("write unknown shim");

        let error = prepare_windows_batch_launch_from_source_env(
            shim,
            vec!["safe argument".to_string()],
            &BTreeMap::new(),
            temp_dir.path(),
        )
        .expect_err("unknown batch shims must not execute");

        assert!(matches!(error, WindowsBatchLaunchError::UnsupportedShim));
        assert!(error.to_string().contains("configure a native executable"));
    }

    #[test]
    fn prefers_a_sibling_node_exe_over_source_path() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("agent.cmd");
        let target = temp_dir.path().join("agent.js");
        let node = temp_dir.path().join("node.exe");
        npm_shim(&shim, "agent.js");
        std::fs::write(&target, "// fixture\n").expect("write target");
        std::fs::write(&node, "fixture").expect("write node fixture");

        let launch = prepare_windows_batch_launch_from_source_env(
            shim,
            Vec::new(),
            &BTreeMap::new(),
            temp_dir.path(),
        )
        .expect("sibling node.exe should prepare without PATH");

        assert_eq!(
            launch.program,
            node.canonicalize().expect("canonical sibling node")
        );
    }

    #[tokio::test]
    async fn native_node_launch_preserves_hostile_argv_without_cmd_exe() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("capture.cmd");
        let target = temp_dir.path().join("capture.js");
        let capture = temp_dir.path().join("captured.json");
        let marker = temp_dir.path().join("injected.txt");
        let poisoned_comspec = temp_dir.path().join("poisoned-cmd.exe");
        npm_shim(&shim, "capture.js");
        std::fs::write(
            &target,
            r#"require("fs").writeFileSync(process.env.CODEWITH_CAPTURE, JSON.stringify(process.argv.slice(2)));
"#,
        )
        .expect("write JavaScript entrypoint");
        let hostile_args = vec![
            "spaces stay one argument".to_string(),
            format!("embedded \" & type nul > \"{}\" & rem", marker.display()),
            "pipe | command".to_string(),
            "input < source".to_string(),
            "output > destination".to_string(),
            "parentheses ( ) literal".to_string(),
            "caret ^ literal".to_string(),
            "%CODEWITH_TEST_PERCENT%".to_string(),
            "!CODEWITH_TEST_BANG!".to_string(),
            String::new(),
            "eleventh argument survives too".to_string(),
        ];
        let mut source_env = std::env::vars().collect::<BTreeMap<_, _>>();
        source_env.retain(|key, _| !key.eq_ignore_ascii_case("PATHEXT"));
        source_env.insert("PaThExT".to_string(), ".EXE;.CMD".to_string());
        source_env.insert(
            "cOmSpEc".to_string(),
            poisoned_comspec.display().to_string(),
        );
        let launch = prepare_windows_batch_launch_from_source_env(
            shim,
            hostile_args.clone(),
            &source_env,
            temp_dir.path(),
        )
        .expect("recognized npm shim should prepare native node");

        assert_eq!(
            launch.program.file_name().and_then(|name| name.to_str()),
            Some("node.exe")
        );
        assert_ne!(launch.program, poisoned_comspec);
        let target = target
            .canonicalize()
            .expect("target path")
            .display()
            .to_string();
        assert_eq!(launch.args.first(), Some(&target));
        let status = Command::new(&launch.program)
            .args(&launch.args)
            .envs(source_env)
            .env("CODEWITH_CAPTURE", &capture)
            .env("CODEWITH_TEST_PERCENT", "expanded-in-test")
            .env("CODEWITH_TEST_BANG", "expanded-in-test")
            .status()
            .await
            .expect("launch native node entrypoint");
        assert!(status.success());
        assert!(!marker.exists(), "hostile argv must not become a command");
        let received = serde_json::from_str::<Vec<String>>(
            &std::fs::read_to_string(capture).expect("read captured argv"),
        )
        .expect("capture JavaScript should serialize argv");
        assert_eq!(received, hostile_args);
    }

    #[test]
    fn rejects_line_break_arguments_before_parsing_the_shim() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let shim = temp_dir.path().join("capture.cmd");
        std::fs::write(&shim, "@echo off\r\nexit /b 0\r\n").expect("write shim");
        for line_break in ["\r", "\n", "\r\n"] {
            let error = prepare_windows_batch_launch_from_source_env(
                shim.clone(),
                vec![format!("task{line_break}next")],
                &BTreeMap::new(),
                temp_dir.path(),
            )
            .expect_err("line breaks must be rejected before batch parsing");
            assert!(matches!(
                error,
                WindowsBatchLaunchError::LineBreak {
                    component: "argument"
                }
            ));
        }
    }
}
