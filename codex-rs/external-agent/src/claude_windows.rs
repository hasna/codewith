use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::prepare_windows_batch_launch_from_source_env;

/// Normalizes a Windows environment before Claude resolves executables or
/// copies credentials. Windows environment keys are case-insensitive; crafted
/// duplicate keys use `BTreeMap`'s stable lexical order and explicit overrides
/// always win.
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

/// Resolves Claude using only the environment that will be sanitized for its
/// launch. `which` reads the host `PATHEXT`, which can disagree with a source
/// environment supplied by the external-agent caller.
pub(crate) fn resolve_windows_program_from_source_env(
    program: &str,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let source_env = merge_windows_environment(source_env, &BTreeMap::new());
    let path = source_env
        .get("PATH")
        .ok_or_else(|| format!("source environment does not define PATH for `{program}`"))?;
    let path_extensions = source_env
        .get("PATHEXT")
        .map(String::as_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter(|extension| valid_windows_pathext(extension))
        .collect::<Vec<_>>();
    let cwd = absolute_cwd(cwd)?;
    let program = Path::new(program);
    let program_has_extension = program.extension().is_some();

    for directory in std::env::split_paths(path) {
        let Some(directory) = resolve_path_entry(&directory, &cwd) else {
            continue;
        };
        let base = directory.join(program);
        if base.is_file() && base.is_absolute() {
            return Ok(base);
        }
        if !program_has_extension {
            for extension in &path_extensions {
                let candidate = PathBuf::from(format!("{}{}", base.display(), extension));
                if candidate.is_file() && candidate.is_absolute() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(format!(
        "could not resolve `{}` from source PATH using source PATHEXT",
        program.display()
    ))
}

/// Converts a resolved Claude command into the native launch prescribed by
/// the bounded cmd-shim primitive. No command interpreter fallback exists.
pub(crate) fn prepare_windows_claude_launch(
    program: PathBuf,
    args: Vec<String>,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<(PathBuf, Vec<String>), String> {
    let source_env = merge_windows_environment(source_env, &BTreeMap::new());
    let launch = prepare_windows_batch_launch_from_source_env(
        program,
        args.into_iter().map(OsString::from).collect(),
        &source_env,
        cwd,
    )
    .map_err(|error| error.to_string())?;
    let args = launch
        .args
        .into_iter()
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| "Windows Claude launch contains a non-Unicode argument".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((launch.program, args))
}

fn valid_windows_pathext(extension: &str) -> bool {
    extension.len() > 1
        && extension.starts_with('.')
        && !extension.ends_with(['.', ' '])
        && !extension.contains(['/', '\\', '<', '>', ':', '"', '|', '?', '*'])
        && !extension.chars().any(char::is_control)
}

fn absolute_cwd(cwd: &Path) -> Result<PathBuf, String> {
    if cwd.is_absolute() {
        return Ok(cwd.to_path_buf());
    }
    std::env::current_dir()
        .map_err(|error| format!("could not resolve Windows launch cwd: {error}"))
        .map(|current_dir| current_dir.join(cwd))
}

fn resolve_path_entry(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let has_prefix = matches!(path.components().next(), Some(Component::Prefix(_)));
    if has_prefix != path.has_root()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return None;
    }
    Some(if has_prefix {
        path.to_path_buf()
    } else {
        cwd.join(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn explicit_overrides_win_after_case_insensitive_normalization() {
        let source = BTreeMap::from([("Path".to_string(), "source".to_string())]);
        let overrides = BTreeMap::from([("PATH".to_string(), "override".to_string())]);

        assert_eq!(
            merge_windows_environment(&source, &overrides),
            BTreeMap::from([("PATH".to_string(), "override".to_string())])
        );
    }

    #[test]
    fn rejects_unsafe_windows_pathext_entries() {
        assert!(valid_windows_pathext(".CMD"));
        for extension in [
            ".CMD.",
            ".CMD ",
            "CMD",
            ".CMD/",
            ".CMD\\",
            ".CMD:stream",
            ".CMD*",
        ] {
            assert!(!valid_windows_pathext(extension));
        }
    }
}
