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
