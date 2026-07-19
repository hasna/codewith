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
        .filter(|extension| {
            extension.len() > 1 && extension.starts_with('.') && !extension.contains(['/', '\\'])
        })
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

/// Rewrites a resolved `.cmd` or `.bat` program into a `cmd.exe /c` launch.
///
/// `CreateProcess` does not provide the same executable contract for batch
/// shims as it does for native binaries. Keep the resolved source-environment
/// path, but execute it through the source `COMSPEC` (or `cmd.exe` when the
/// source omits it). CMD uses syntactic quotes to group each argument. An
/// embedded quote closes that group, emits a caret-escaped literal quote, and
/// reopens the group; every command metacharacter therefore stays quoted.
/// Delayed expansion remains disabled and `%` is doubled so caller-supplied
/// environment references remain literal.
#[cfg(windows)]
pub(crate) fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<String>,
    source_env: &BTreeMap<String, String>,
) -> Result<(PathBuf, Vec<String>), WindowsBatchLaunchError> {
    if !is_windows_batch_program(&program) {
        return Ok((program, args));
    }

    let source_env = merge_windows_environment(source_env, &BTreeMap::new());
    let command_line = std::iter::once(("program", program.to_string_lossy().into_owned()))
        .chain(args.into_iter().map(|argument| ("argument", argument)))
        .map(|(component, argument)| {
            validate_windows_batch_command_component(component, argument.as_str())?;
            Ok(quote_windows_cmd_argument(argument.as_str()))
        })
        .collect::<Result<Vec<_>, WindowsBatchLaunchError>>()?
        .join(" ");
    let command_interpreter = windows_source_env_value(source_env, "COMSPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    Ok((
        command_interpreter,
        vec![
            "/d".to_string(),
            "/v:off".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            format!("\"{command_line}\""),
        ],
    ))
}

/// Appends an external-agent launch specification to a Tokio command.
///
/// `cmd.exe /c` consumes its final value from the raw process command line,
/// not through the usual Windows argv parser. The batch preparation function
/// has already validated and quoted that command string, so passing it through
/// the normal `args` API would quote it a second time and alter its meaning.
#[cfg(windows)]
pub(crate) fn configure_windows_batch_launch(command: &mut Command, args: &[String]) {
    const CMD_PREFIX: [&str; 4] = ["/d", "/v:off", "/s", "/c"];

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
fn quote_windows_cmd_argument(argument: &str) -> String {
    let argument = argument.replace('%', "%%");
    let mut quoted = String::from('"');
    for character in argument.chars() {
        if character == '\"' {
            quoted.push_str("\"^\"\"");
            continue;
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
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
        ];
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let source_env = BTreeMap::from([("cOmSpEc".to_string(), comspec)]);
        let (program, args) = prepare_windows_batch_launch_from_source_env(
            batch_path,
            hostile_args.clone(),
            &source_env,
        )
        .expect("hostile single-line arguments should prepare");

        let mut command = Command::new(program);
        configure_windows_batch_launch(&mut command, &args);
        let status = command
            .env("CODEWITH_BATCH_TEST_PERCENT", "expanded-in-test")
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
    fn batch_launch_rejects_line_break_arguments_before_spawning() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let batch_path = temp_dir.path().join("capture.cmd");
        let marker_path = temp_dir.path().join("injected.txt");
        std::fs::write(&batch_path, "@echo off\r\nexit /b 0\r\n")
            .expect("write batch capture shim");
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let source_env = BTreeMap::from([("COMSPEC".to_string(), comspec)]);

        for line_break in ["\r", "\n", "\r\n"] {
            let task = format!(
                "review{line_break}& type nul > \"{}\" & rem",
                marker_path.display()
            );
            let error = prepare_windows_batch_launch_from_source_env(
                batch_path.clone(),
                vec![task],
                &source_env,
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
                &source_env,
            ),
            Ok((native_program, native_args))
        );

        let error = prepare_windows_batch_launch_from_source_env(
            PathBuf::from("capture\r\n.cmd"),
            Vec::new(),
            &source_env,
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
}
