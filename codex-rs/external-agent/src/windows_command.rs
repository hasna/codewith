#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

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
/// source omits it). The command string quotes every argument, disables AutoRun
/// and delayed expansion, and doubles `%` so a batch shim cannot expand a
/// caller-supplied environment reference.
#[cfg(windows)]
pub(crate) fn prepare_windows_batch_launch_from_source_env(
    program: PathBuf,
    args: Vec<String>,
    source_env: &BTreeMap<String, String>,
) -> (PathBuf, Vec<String>) {
    if !is_windows_batch_program(&program) {
        return (program, args);
    }

    let command_line = std::iter::once(program.to_string_lossy().into_owned())
        .chain(args)
        .map(|argument| quote_windows_cmd_argument(argument.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    let command_interpreter = windows_source_env_value(source_env, "COMSPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    (
        command_interpreter,
        vec![
            "/d".to_string(),
            "/v:off".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            format!("\"{command_line}\""),
        ],
    )
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
    let mut quoted = String::from("\"");
    let mut backslash_count = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslash_count += 1;
            continue;
        }
        if character == '\"' {
            quoted.push_str(&"\\".repeat(backslash_count.saturating_mul(2).saturating_add(1)));
            quoted.push('\"');
            backslash_count = 0;
            continue;
        }
        quoted.push_str(&"\\".repeat(backslash_count));
        backslash_count = 0;
        quoted.push(character);
    }
    quoted.push_str(&"\\".repeat(backslash_count.saturating_mul(2)));
    quoted.push('\"');
    quoted
}

#[cfg(windows)]
fn windows_source_env_value<'a>(
    source_env: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    source_env
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}
