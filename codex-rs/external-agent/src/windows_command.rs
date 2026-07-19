#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use tokio::process::Command;

/// Collapses Windows environment keys before a launcher resolves or copies
/// them. Source collisions use BTreeMap's stable lexical order, while values
/// supplied by `overrides` always win over the inherited environment.
#[cfg(windows)]
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

/// Resolves a Windows command using only the supplied source environment.
///
/// `which` reads `PATHEXT` from the host process, which can differ from the
/// sanitized source environment that an external-agent runtime will receive.
/// This resolver keeps readiness and launch discovery aligned without changing
/// global process state.
#[cfg(windows)]
pub(crate) fn resolve_windows_program_from_source_env(
    program: &str,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let path = windows_source_env_value(source_env, "PATH")
        .ok_or_else(|| format!("source environment does not define PATH for `{program}`"))?;
    let path_extensions = windows_source_env_value(source_env, "PATHEXT")
        .map(String::as_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter(|extension| is_valid_windows_pathext(extension))
        .collect::<Vec<_>>();
    let program_path = Path::new(program);
    let bases = if program.contains(['/', '\\']) {
        vec![if program_path.is_absolute() {
            program_path.to_path_buf()
        } else {
            cwd.join(program_path)
        }]
    } else {
        std::env::split_paths(path)
            .filter(|directory| !directory.as_os_str().is_empty())
            .map(|directory| directory.join(program_path))
            .collect::<Vec<_>>()
    };

    for base in bases {
        if base.is_file() {
            return Ok(base);
        }
        if base.extension().is_none() {
            for extension in &path_extensions {
                let candidate = PathBuf::from(format!("{}{}", base.display(), extension));
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

#[cfg(any(windows, test))]
fn is_valid_windows_pathext(extension: &str) -> bool {
    extension.len() > 1
        && extension.starts_with('.')
        && !extension.ends_with(['.', ' '])
        && !extension.contains(['/', '\\'])
}

/// Rewrites a resolved `.cmd` or `.bat` program into a `cmd.exe /c` launch.
///
/// `CreateProcess` does not provide the same executable contract for batch
/// shims as it does for native binaries. Keep the resolved source-environment
/// path, but execute it through the source `COMSPEC` (or `cmd.exe` when the
/// source omits it). The `/c` command string contains only fixed delayed-
/// expansion references; the program and arguments are stored in child-only
/// environment variables. CMD expands those references after it has parsed
/// command metacharacters, so caller data never becomes batch syntax.
#[cfg(windows)]
pub(crate) fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<String>,
    source_env: &mut BTreeMap<String, String>,
) -> Result<(PathBuf, Vec<String>), WindowsBatchLaunchError> {
    if !is_windows_batch_program(&program) {
        return Ok((program, args));
    }

    let source_environment = merge_windows_environment(source_env, &BTreeMap::new());
    let program_value = program.to_string_lossy().into_owned();
    validate_windows_batch_command_component("program", program_value.as_str())?;
    for argument in &args {
        validate_windows_batch_command_component("argument", argument.as_str())?;
    }

    let argument_count = args.len();
    let transport_values = std::iter::once((WINDOWS_BATCH_PROGRAM_ENV.to_string(), program_value))
        .chain(
            args.into_iter()
                .enumerate()
                .map(|(index, argument)| (windows_batch_argument_env_name(index), argument)),
        );
    for (name, value) in transport_values {
        source_env.retain(|key, _| !key.eq_ignore_ascii_case(name.as_str()));
        source_env.insert(name, value);
    }

    let command_interpreter = windows_source_env_value(&source_environment, "COMSPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    Ok((
        command_interpreter,
        vec![
            "/d".to_string(),
            "/v:on".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            windows_batch_transport_command_line(argument_count),
        ],
    ))
}

#[cfg(any(windows, test))]
const WINDOWS_BATCH_PROGRAM_ENV: &str = "CODEWITH_BATCH_PROGRAM";

#[cfg(any(windows, test))]
const WINDOWS_BATCH_ARGUMENT_ENV_PREFIX: &str = "CODEWITH_BATCH_ARGUMENT_";

#[cfg(any(windows, test))]
fn windows_batch_argument_env_name(index: usize) -> String {
    format!("{WINDOWS_BATCH_ARGUMENT_ENV_PREFIX}{index}")
}

#[cfg(any(windows, test))]
fn windows_batch_transport_command_line(argument_count: usize) -> String {
    let command_line = std::iter::once(format!("\"!{WINDOWS_BATCH_PROGRAM_ENV}!\""))
        .chain((0..argument_count).map(|index| {
            let name = windows_batch_argument_env_name(index);
            format!("\"!{name}!\"")
        }))
        .collect::<Vec<_>>()
        .join(" ");
    format!("\"{command_line}\"")
}

/// Appends an external-agent launch specification to a Tokio command.
///
/// `cmd.exe /c` consumes its final value from the raw process command line,
/// not through the usual Windows argv parser. The batch preparation function
/// has already validated and quoted that command string, so passing it through
/// the normal `args` API would quote it a second time and alter its meaning.
#[cfg(windows)]
pub(crate) fn configure_windows_batch_launch(command: &mut Command, args: &[String]) {
    const CMD_PREFIX: [&str; 4] = ["/d", "/v:on", "/s", "/c"];

    if args.len() == CMD_PREFIX.len() + 1
        && args
            .iter()
            .take(CMD_PREFIX.len())
            .map(String::as_str)
            .eq(CMD_PREFIX)
    {
        command
            .args(&args[..CMD_PREFIX.len()])
            .raw_arg(&args[CMD_PREFIX.len()]);
    } else {
        command.args(args);
    }
}

/// Rejects physical command-line boundaries before a batch launch reaches
/// `cmd.exe`. CMD parses CR and LF as command separators, so preserving them
/// in a `/c` string would permit a caller to append another command.
#[cfg(any(windows, test))]
fn validate_windows_batch_command_component(
    component: &'static str,
    value: &str,
) -> Result<(), WindowsBatchLaunchError> {
    if value.contains(['\r', '\n']) {
        return Err(WindowsBatchLaunchError::LineBreak { component });
    }
    Ok(())
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WindowsBatchLaunchError {
    #[error("Windows batch launch rejects {component} containing CR or LF")]
    LineBreak { component: &'static str },
}

#[cfg(windows)]
fn is_windows_batch_program(program: &Path) -> bool {
    program
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

#[cfg(windows)]
fn windows_source_env_value<'a>(
    source_env: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    source_env
        .iter()
        .rfind(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn batch_launch_preserves_hostile_arguments_without_executing_them() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let batch_path = temp_dir.path().join("capture.cmd");
        let capture_path = temp_dir.path().join("captured.txt");
        let marker_path = temp_dir.path().join("injected.txt");
        std::fs::write(
            &batch_path,
            r#"@echo off
setlocal DisableDelayedExpansion
set "ARG1=%~1"
set "ARG2=%~2"
set "ARG3=%~3"
set "ARG4=%~4"
set "ARG5=%~5"
set "ARG6=%~6"
set "ARG7=%~7"
set "ARG8=%~8"
set "ARG9=%~9"
set ARG > "%~dp0captured.txt"
exit /b 0
"#,
        )
        .expect("write batch capture shim");
        let hostile_args = vec![
            format!(
                "embedded \" & type nul > \"{}\" & rem",
                marker_path.display()
            ),
            "pipe | command".to_string(),
            "input < source".to_string(),
            "output > destination".to_string(),
            "open ( parenthesis".to_string(),
            "close ) parenthesis".to_string(),
            "caret ^ literal".to_string(),
            "%CODEWITH_BATCH_TEST_PERCENT%".to_string(),
            "bang ! literal".to_string(),
            "!CODEWITH_BATCH_TEST_BANG!".to_string(),
        ];
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let mut source_env = BTreeMap::from([("cOmSpEc".to_string(), comspec)]);
        let (program, args) = prepare_windows_batch_launch_from_source_env(
            batch_path,
            hostile_args.clone(),
            &mut source_env,
        )
        .expect("hostile single-line arguments should prepare");

        let mut command = Command::new(program);
        configure_windows_batch_launch(&mut command, &args);
        let status = command
            .envs(source_env)
            .env("CODEWITH_BATCH_TEST_PERCENT", "expanded-in-test")
            .env("CODEWITH_BATCH_TEST_BANG", "expanded-in-test")
            .status()
            .await
            .expect("launch batch capture shim");
        assert!(status.success());
        assert!(
            !marker_path.exists(),
            "hostile argument escaped the batch command line"
        );
        let captured = std::fs::read_to_string(capture_path).expect("read captured arguments");
        let received = (1..=hostile_args.len())
            .map(|index| {
                let prefix = format!("ARG{index}=");
                captured
                    .lines()
                    .find_map(|line| line.strip_prefix(prefix.as_str()))
                    .expect("capture shim should receive every argument")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(received, hostile_args);
    }

    #[test]
    fn trailing_dot_pathext_rejects_hostile_arguments_before_batch_protection_can_be_bypassed() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin directory");
        std::fs::write(bin_dir.join("claude.cmd"), "@echo off\r\nexit /b 0\r\n")
            .expect("write batch shim");
        let marker_path = temp_dir.path().join("injected.txt");
        let source_env = BTreeMap::from([
            ("PATH".to_string(), bin_dir.display().to_string()),
            ("PATHEXT".to_string(), ".CMD.".to_string()),
            (
                "COMSPEC".to_string(),
                std::env::var("COMSPEC").expect("Windows supplies COMSPEC"),
            ),
        ]);

        let hostile_argument =
            format!("review \" & type nul > \"{}\" & rem", marker_path.display());
        let mut launch_env = source_env.clone();
        let error = resolve_windows_program_from_source_env("claude", &source_env, temp_dir.path())
            .and_then(|program| {
                prepare_windows_batch_launch_from_source_env(
                    program,
                    vec![hostile_argument],
                    &mut launch_env,
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            })
            .expect_err(
                "trailing-dot PATHEXT must reject hostile arguments before batch classification",
            );

        assert!(error.contains("could not resolve"));
        assert!(
            !marker_path.exists(),
            "trailing-dot PATHEXT must not permit an injected command to execute"
        );
    }

    #[test]
    fn batch_launch_rejects_line_break_arguments_before_spawning() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let batch_path = temp_dir.path().join("capture.cmd");
        let marker_path = temp_dir.path().join("injected.txt");
        std::fs::write(&batch_path, "@echo off\r\nexit /b 0\r\n")
            .expect("write batch capture shim");
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let mut source_env = BTreeMap::from([("COMSPEC".to_string(), comspec)]);

        for line_break in ["\r", "\n", "\r\n"] {
            let task = format!(
                "review{line_break}& type nul > \"{}\" & rem",
                marker_path.display()
            );
            let error = prepare_windows_batch_launch_from_source_env(
                batch_path.clone(),
                vec![task],
                &mut source_env,
            )
            .expect_err("line breaks must be rejected before launching cmd.exe");
            assert_eq!(
                error,
                WindowsBatchLaunchError::LineBreak {
                    component: "argument"
                }
            );
            assert!(
                !marker_path.exists(),
                "rejected argument must never execute an injected command"
            );
        }

        let native_program = PathBuf::from("native.exe");
        let native_args = vec!["line\r\nbearing argument".to_string()];
        assert_eq!(
            prepare_windows_batch_launch_from_source_env(
                native_program.clone(),
                native_args.clone(),
                &mut source_env,
            ),
            Ok((native_program, native_args))
        );

        let error = prepare_windows_batch_launch_from_source_env(
            PathBuf::from("capture\r\n.cmd"),
            Vec::new(),
            &mut source_env,
        )
        .expect_err("batch program names with line breaks must be rejected");
        assert_eq!(
            error,
            WindowsBatchLaunchError::LineBreak {
                component: "program"
            }
        );
    }
}

#[cfg(test)]
mod command_line_validation_tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn rejects_cr_lf_and_crlf_without_rewriting_them() {
        for value in ["task\rnext", "task\nnext", "task\r\nnext"] {
            assert_eq!(
                validate_windows_batch_command_component("argument", value),
                Err(WindowsBatchLaunchError::LineBreak {
                    component: "argument"
                })
            );
        }
    }

    #[test]
    fn cmd_transport_contains_only_fixed_environment_references() {
        assert_eq!(
            windows_batch_transport_command_line(2),
            "\"\"!CODEWITH_BATCH_PROGRAM!\" \"!CODEWITH_BATCH_ARGUMENT_0!\" \"!CODEWITH_BATCH_ARGUMENT_1!\"\""
        );
    }

    #[test]
    fn rejects_trailing_dot_or_space_pathext_aliases() {
        assert!(is_valid_windows_pathext(".CMD"));
        for invalid in [".CMD.", ".BAT.", ".CMD ", ".BAT ", "CMD", ".CMD/", ".BAT\\"] {
            assert!(!is_valid_windows_pathext(invalid));
        }
    }
}
