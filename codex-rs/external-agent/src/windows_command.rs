#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

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
/// Relative `PATH` entries are anchored to the requested launch directory, so
/// discovery validates the same program that a launch from that directory uses.
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
    let launch_cwd = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("could not resolve launch cwd for `{program}`: {err}"))?
            .join(cwd)
    };
    let bases = if program.contains(['/', '\\']) {
        vec![if program_path.is_absolute() {
            program_path.to_path_buf()
        } else {
            launch_cwd.join(program_path)
        }]
    } else {
        std::env::split_paths(path)
            .filter(|directory| !directory.as_os_str().is_empty())
            .map(|directory| {
                if directory.is_absolute() {
                    directory
                } else {
                    launch_cwd.join(directory)
                }
                .join(program_path)
            })
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

    #[test]
    fn resolves_relative_path_entries_from_launch_cwd_as_absolute_paths() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let launch_cwd = temp_dir.path().join("launch-cwd");
        let relative_bin = launch_cwd.join("relative-bin");
        std::fs::create_dir_all(&relative_bin).expect("create relative PATH directory");
        let agent = relative_bin.join("claude.cmd");
        std::fs::write(&agent, "@echo off\r\nexit /b 0\r\n").expect("write batch shim");
        let source_env = BTreeMap::from([
            ("PATH".to_string(), "relative-bin".to_string()),
            ("PATHEXT".to_string(), ".CMD".to_string()),
        ]);

        let resolved = resolve_windows_program_from_source_env("claude", &source_env, &launch_cwd)
            .expect("relative source PATH should resolve against launch cwd");

        assert_eq!(resolved, agent);
        assert!(resolved.is_absolute());
    }
}

#[cfg(test)]
mod pathext_tests {
    use super::*;

    #[test]
    fn rejects_trailing_dot_or_space_pathext_aliases() {
        assert!(is_valid_windows_pathext(".CMD"));
        for invalid in [".CMD.", ".BAT.", ".CMD ", ".BAT ", "CMD", ".CMD/", ".BAT\\"] {
            assert!(!is_valid_windows_pathext(invalid));
        }
    }
}
