//! Lexical identity keys for managed-worktree display paths.
//!
//! This module intentionally does not read the filesystem. The managed-worktree
//! store preserves its display path separately; this codec derives a stable key
//! for future identity and collision checks from that display string alone.

#[cfg(any(target_os = "macos", test))]
use unicode_normalization::UnicodeNormalization;

/// Derives the platform identity key for an already-persisted display path.
///
/// This is key-only normalization: it does not alter the persisted display
/// path, resolve symlinks, or canonicalize any existing ancestor. Windows
/// folds case with a locale-independent Unicode upper-then-lower mapping.
/// macOS additionally normalizes to NFD to reflect the default APFS alias
/// behavior. Other Unix platforms preserve case and Unicode spelling.
pub(crate) fn managed_worktree_path_key_from_display(display_path: &str) -> String {
    #[cfg(windows)]
    {
        windows_path_key(display_path)
    }

    #[cfg(target_os = "macos")]
    {
        macos_path_key(display_path)
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        unix_path_key(display_path)
    }
}

#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn unix_path_key(display_path: &str) -> String {
    normalize_slash_path(display_path)
}

#[cfg(any(target_os = "macos", test))]
fn macos_path_key(display_path: &str) -> String {
    let decomposed = display_path.nfd().collect::<String>();
    fold_case(normalize_slash_path(&decomposed)).nfd().collect()
}

#[cfg(any(not(windows), test))]
fn normalize_slash_path(path: &str) -> String {
    let rooted = path.starts_with('/');
    let components = normalize_components(path.split('/'), /*root_floor*/ 0, rooted);
    format_path(components, "/", rooted, "")
}

#[cfg(any(windows, test))]
fn windows_path_key(display_path: &str) -> String {
    let path = strip_windows_verbatim_prefix(display_path).replace('/', "\\");
    let key = if let Some(path) = path.strip_prefix(r"\\") {
        normalize_windows_unc(path)
    } else if has_windows_drive_prefix(&path) {
        normalize_windows_drive(&path)
    } else {
        normalize_windows_relative(&path)
    };
    fold_case(key)
}

#[cfg(any(windows, test))]
fn strip_windows_verbatim_prefix(path: &str) -> String {
    if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        return format!(r"\\{}", path.get(8..).unwrap_or_default());
    }
    if path
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"))
    {
        return path.get(4..).unwrap_or_default().to_string();
    }
    path.to_string()
}

#[cfg(any(windows, test))]
fn has_windows_drive_prefix(path: &str) -> bool {
    path.as_bytes()
        .get(1)
        .is_some_and(|character| *character == b':')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
}

#[cfg(any(windows, test))]
fn normalize_windows_unc(path: &str) -> String {
    let components = normalize_components(path.split('\\'), /*root_floor*/ 2, true);
    let joined = components.join("\\");
    if joined.is_empty() {
        r"\\".to_string()
    } else {
        format!(r"\\{joined}")
    }
}

#[cfg(any(windows, test))]
fn normalize_windows_drive(path: &str) -> String {
    let (prefix, suffix) = path.split_at(2);
    let rooted = suffix.starts_with('\\');
    let components = normalize_components(suffix.split('\\'), /*root_floor*/ 0, rooted);
    format_path(components, "\\", rooted, prefix)
}

#[cfg(any(windows, test))]
fn normalize_windows_relative(path: &str) -> String {
    let rooted = path.starts_with('\\');
    let components = normalize_components(path.split('\\'), /*root_floor*/ 0, rooted);
    format_path(components, "\\", rooted, "")
}

fn normalize_components<'a>(
    components: impl Iterator<Item = &'a str>,
    root_floor: usize,
    rooted: bool,
) -> Vec<&'a str> {
    let mut normalized = Vec::new();
    for component in components {
        match component {
            "" | "." => {}
            ".." if normalized.len() > root_floor => {
                normalized.pop();
            }
            ".." if !rooted => normalized.push(component),
            ".." => {}
            _ => normalized.push(component),
        }
    }
    normalized
}

fn format_path(components: Vec<&str>, separator: &str, rooted: bool, prefix: &str) -> String {
    let joined = components.join(separator);
    match (prefix, rooted, joined.is_empty()) {
        ("", false, true) => ".".to_string(),
        ("", false, false) => joined,
        ("", true, true) => separator.to_string(),
        ("", true, false) => format!("{separator}{joined}"),
        (_, false, true) => prefix.to_string(),
        (_, false, false) => format!("{prefix}{joined}"),
        (_, true, true) => format!("{prefix}{separator}"),
        (_, true, false) => format!("{prefix}{separator}{joined}"),
    }
}

#[cfg(any(windows, target_os = "macos", test))]
fn fold_case(path: String) -> String {
    path.chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn unix_keys_normalize_lexically_without_changing_the_display_path() {
        let display_path = "./worktrees//run-a/../run-b";

        assert_eq!("worktrees/run-b", unix_path_key(display_path));
        assert_eq!("./worktrees//run-a/../run-b", display_path);
        assert_eq!("/run-b", unix_path_key("/worktrees/../run-b"));
        assert_eq!("../run-b", unix_path_key("worktrees/../../run-b"));
        assert_eq!(".", unix_path_key("worktrees/.."));
    }

    #[test]
    fn unix_keys_preserve_case_and_unicode_spelling() {
        assert_ne!(
            unix_path_key("/worktrees/RunA"),
            unix_path_key("/worktrees/runa")
        );
        assert_ne!(
            unix_path_key("/worktrees/x\u{00e5}"),
            unix_path_key("/worktrees/xa\u{030a}")
        );
    }

    #[test]
    fn macos_keys_fold_case_and_canonical_unicode_aliases() {
        assert_eq!(
            macos_path_key("/worktrees/X\u{00c5}/run"),
            macos_path_key("/worktrees/xa\u{030a}/RUN")
        );
    }

    #[test]
    fn windows_keys_normalize_separators_drive_roots_and_parents() {
        assert_eq!(
            r"c:\managed-worktrees\run-a",
            windows_path_key(r"C:/Managed-Worktrees\\Run-A\\child\\..")
        );
        assert_eq!(r"c:\", windows_path_key(r"C:\."));
        assert_eq!(r"c:run-a", windows_path_key(r"C:.\Run-A"));
        assert_ne!(windows_path_key(r"C:RunA"), windows_path_key(r"C:\RunA"));
    }

    #[test]
    fn windows_keys_collapse_unc_and_verbatim_aliases() {
        assert_eq!(
            windows_path_key(r"\\server\share\run-a"),
            windows_path_key(r"\\?\UNC\SERVER\Share\worktrees\..\run-a")
        );
        assert_eq!(
            windows_path_key(r"C:\worktrees\run-a"),
            windows_path_key(r"\\?\c:\WORKTREES\Run-A")
        );
        assert_eq!(
            r"\\server\share",
            windows_path_key(r"\\server\share\worktrees\..")
        );
    }

    #[test]
    fn windows_case_fold_handles_long_s_without_unicode_normalizing_names() {
        assert_eq!(
            windows_path_key(r"C:\worktrees\RunS"),
            windows_path_key("c:\\worktrees\\run\u{017f}")
        );
        assert_ne!(
            windows_path_key("C:\\worktrees\\x\u{00e5}"),
            windows_path_key("C:\\worktrees\\xa\u{030a}")
        );
    }

    #[test]
    fn current_platform_key_uses_its_documented_policy() {
        #[cfg(windows)]
        assert_eq!(
            windows_path_key(r"C:\worktrees\RunA"),
            managed_worktree_path_key_from_display(r"C:\worktrees\RunA")
        );

        #[cfg(target_os = "macos")]
        assert_eq!(
            macos_path_key("/worktrees/x\u{00e5}"),
            managed_worktree_path_key_from_display("/worktrees/x\u{00e5}")
        );

        #[cfg(not(any(windows, target_os = "macos")))]
        assert_eq!(
            unix_path_key("/worktrees/RunA"),
            managed_worktree_path_key_from_display("/worktrees/RunA")
        );
    }
}
