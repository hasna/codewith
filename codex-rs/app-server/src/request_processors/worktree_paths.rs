use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn path_to_api_string(path: &Path) -> String {
    path_to_string(&normalize_path(path))
}

pub(super) fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let left_string = path_to_api_string(left);
    let right_string = path_to_api_string(right);
    if paths_equivalent_strings(&left_string, &right_string) {
        return true;
    }

    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => {
            let left = path_to_string(left.as_path());
            let right = path_to_string(right.as_path());
            paths_equivalent_strings(&left, &right)
        }
        _ => false,
    }
}

#[cfg(windows)]
fn paths_equivalent_strings(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg(not(windows))]
fn paths_equivalent_strings(left: &str, right: &str) -> bool {
    left == right
}

fn normalize_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(not(windows))]
    let path = path.to_path_buf();

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn path_to_string(path: &Path) -> String {
    let path = path.to_string_lossy().into_owned();
    strip_windows_verbatim_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: String) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_owned();
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: String) -> String {
    path
}
