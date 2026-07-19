use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use crate::WindowsBatchLaunchError;
use crate::WindowsNativeLaunch;
use crate::prepare_windows_batch_launch_from_source_env;

pub(crate) const INHERITED_ENV_VARS: &[&str] = &["PATHEXT", "SYSTEMROOT"];

/// Normalizes Windows environment keys so source lookup and child construction
/// use the same case-insensitive values. Overrides always win.
pub(crate) fn merge_environment(
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

pub(crate) fn resolve_program_from_source_env(
    program: &str,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let source_env = merge_environment(source_env, &BTreeMap::new());
    let path = source_env
        .get("PATH")
        .ok_or_else(|| format!("source environment does not define PATH for `{program}`"))?;
    let path_extensions = source_env
        .get("PATHEXT")
        .map(String::as_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(|extension| {
            valid_pathext(extension)
                .then_some(extension)
                .ok_or_else(|| format!("source PATHEXT contains invalid extension `{extension}`"))
        })
        .collect::<Result<Vec<_>, _>>()?;
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
            .map(|directory| {
                let directory = if directory.is_absolute() {
                    directory
                } else {
                    cwd.join(directory)
                };
                directory.join(program_path)
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

pub(crate) fn prepare_native_launch(
    program: PathBuf,
    args: Vec<String>,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<WindowsNativeLaunch, WindowsBatchLaunchError> {
    prepare_windows_batch_launch_from_source_env(
        program,
        args.into_iter().map(Into::into).collect(),
        source_env,
        cwd,
    )
}

/// Converts the native plan into the current external-agent contract without
/// lossy path or argument conversion. A future non-Unicode contract must be
/// added explicitly rather than silently changing the command line.
pub(crate) fn native_launch_parts(
    launch: WindowsNativeLaunch,
) -> Result<(PathBuf, Vec<String>), String> {
    let args = launch
        .args
        .into_iter()
        .map(|argument| {
            argument.into_string().map_err(|_| {
                "Windows native launch has a non-Unicode argument that the external-agent launch contract cannot represent losslessly".to_string()
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((launch.program, args))
}

fn valid_pathext(extension: &str) -> bool {
    extension.len() > 1
        && extension.starts_with('.')
        && !extension.ends_with(['.', ' '])
        && !extension.contains(['/', '\\', ':'])
        && !extension.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn source_environment_merges_case_insensitively_with_overrides() {
        let source = BTreeMap::from([
            ("Path".to_string(), r"C:\first".to_string()),
            ("PATHEXT".to_string(), ".CMD".to_string()),
        ]);
        let overrides = BTreeMap::from([("path".to_string(), r"C:\override".to_string())]);

        assert_eq!(
            merge_environment(&source, &overrides),
            BTreeMap::from([
                ("PATH".to_string(), r"C:\override".to_string()),
                ("PATHEXT".to_string(), ".CMD".to_string()),
            ])
        );
    }

    #[test]
    fn resolver_uses_only_case_insensitive_source_path_and_pathext() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin directory");
        let executable = bin_dir.join("agent.CUSTOM");
        std::fs::write(&executable, "not executed").expect("write fake executable");
        let source_env = BTreeMap::from([
            ("pAtH".to_string(), bin_dir.display().to_string()),
            ("pAtHeXt".to_string(), ".CUSTOM;.CMD".to_string()),
        ]);

        assert_eq!(
            resolve_program_from_source_env("agent", &source_env, temp_dir.path())
                .expect("source PATHEXT resolves executable"),
            executable
        );
    }

    #[test]
    fn resolver_rejects_trailing_dot_pathext_aliases() {
        assert!(!valid_pathext(".CMD."));
        assert!(!valid_pathext(".BAT "));
        assert!(valid_pathext(".CMD"));
    }

    #[test]
    fn resolver_fails_closed_for_malformed_source_pathext() {
        let source_env = BTreeMap::from([
            ("PATH".to_string(), r"C:\tools".to_string()),
            ("PATHEXT".to_string(), ".EXE;.CMD.".to_string()),
        ]);

        let err = resolve_program_from_source_env("agent", &source_env, Path::new(r"C:\cwd"))
            .expect_err("malformed PATHEXT must not be ignored");

        assert_eq!(err, "source PATHEXT contains invalid extension `.CMD.`");
    }
}
