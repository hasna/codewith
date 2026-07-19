//! Conservative lexical identity key staging specification.
//!
//! This test-only module intentionally does not read the filesystem. It proves
//! the byte-safe key format for a later persistence and admission stage without
//! pretending that the codec has production callers today. Keys are opaque byte
//! sequences derived from native path data, so they never rely on lossy display
//! text. They normalize only lexical syntax that is unambiguous across
//! filesystem policies. Case and Unicode alias checks require verified
//! filesystem policy and belong to later admission code.

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

const KEY_VERSION: &[u8] = b"managed-worktree-path-key-v1";

#[cfg(unix)]
fn unix_path_key(path: &OsStr) -> Vec<u8> {
    let path = path.as_bytes();
    let rooted = path.starts_with(b"/");
    let components = normalize_byte_components(path.split(|byte| *byte == b'/'), rooted);
    encode_byte_key(
        if path.starts_with(b"//") && !path.starts_with(b"///") {
            b"unix-double-slash-absolute"
        } else if rooted {
            b"unix-absolute"
        } else {
            b"unix-relative"
        },
        components,
    )
}

#[cfg(any(windows, test))]
fn windows_path_key(path: &[u16]) -> Vec<u8> {
    if has_windows_verbatim_prefix(path) {
        return encode_wide_key(verbatim_kind(path), path, Vec::new());
    }
    if has_windows_device_prefix(path) {
        return encode_wide_key(b"windows-device", path, Vec::new());
    }
    if has_windows_unc_prefix(path) {
        let components = normalize_wide_components(
            split_windows_components(&path[2..]),
            /*rooted*/ true,
            /*root_floor*/ 2,
        );
        return encode_wide_key(b"windows-unc", &[], components);
    }
    if has_windows_drive_prefix(path) {
        let rooted = path.get(2).is_some_and(|unit| is_windows_separator(*unit));
        let components = normalize_wide_components(
            split_windows_components(&path[2..]),
            rooted,
            /*root_floor*/ 0,
        );
        return encode_wide_key(
            if rooted {
                b"windows-drive-absolute"
            } else {
                b"windows-drive-relative"
            },
            &path[..2],
            components,
        );
    }

    let rooted = path.first().is_some_and(|unit| is_windows_separator(*unit));
    let components = normalize_wide_components(
        split_windows_components(path),
        rooted,
        /*root_floor*/ 0,
    );
    encode_wide_key(
        if rooted {
            b"windows-rooted"
        } else {
            b"windows-relative"
        },
        &[],
        components,
    )
}

#[cfg(any(windows, test))]
fn has_windows_verbatim_prefix(path: &[u16]) -> bool {
    path.starts_with(&[
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ])
}

#[cfg(any(windows, test))]
fn has_windows_device_prefix(path: &[u16]) -> bool {
    path.starts_with(&[
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'.'),
        u16::from(b'\\'),
    ])
}

#[cfg(any(windows, test))]
fn has_windows_unc_prefix(path: &[u16]) -> bool {
    matches!(path, [first, second, ..] if is_windows_separator(*first) && is_windows_separator(*second))
}

#[cfg(any(windows, test))]
fn has_windows_drive_prefix(path: &[u16]) -> bool {
    path.first().is_some_and(|drive| is_ascii_alpha(*drive))
        && path.get(1).is_some_and(|unit| *unit == u16::from(b':'))
}

#[cfg(any(windows, test))]
fn verbatim_kind(path: &[u16]) -> &'static [u8] {
    let namespace = &path[4..];
    if has_ascii_prefix_ignore_case(namespace, b"UNC")
        && namespace
            .get(3)
            .is_some_and(|unit| is_windows_separator(*unit))
    {
        b"windows-verbatim-unc"
    } else if has_windows_drive_prefix(namespace) {
        b"windows-verbatim-drive"
    } else if has_ascii_prefix_ignore_case(namespace, b"Volume{") {
        b"windows-verbatim-volume"
    } else {
        b"windows-verbatim"
    }
}

#[cfg(any(windows, test))]
fn has_ascii_prefix_ignore_case(path: &[u16], prefix: &[u8]) -> bool {
    path.get(..prefix.len()).is_some_and(|units| {
        units.iter().zip(prefix).all(|(unit, expected)| {
            u8::try_from(*unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(expected))
        })
    })
}

#[cfg(any(windows, test))]
fn is_ascii_alpha(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|unit| unit.is_ascii_alphabetic())
}

#[cfg(any(windows, test))]
fn is_windows_separator(unit: u16) -> bool {
    unit == u16::from(b'\\') || unit == u16::from(b'/')
}

#[cfg(any(windows, test))]
fn split_windows_components(path: &[u16]) -> impl Iterator<Item = &[u16]> {
    path.split(|unit| is_windows_separator(*unit))
}

#[cfg(any(windows, test))]
fn normalize_wide_components<'a>(
    components: impl Iterator<Item = &'a [u16]>,
    rooted: bool,
    root_floor: usize,
) -> Vec<&'a [u16]> {
    normalize_components(
        components,
        rooted,
        root_floor,
        |component| component.is_empty() || component == [b'.' as u16],
        |component| component == [b'.' as u16, b'.' as u16],
    )
}

#[cfg(unix)]
fn normalize_byte_components<'a>(
    components: impl Iterator<Item = &'a [u8]>,
    rooted: bool,
) -> Vec<&'a [u8]> {
    normalize_components(
        components,
        rooted,
        /*root_floor*/ 0,
        |component| component.is_empty() || component == b".",
        |component| component == b"..",
    )
}

fn normalize_components<'a, T>(
    components: impl Iterator<Item = &'a [T]>,
    rooted: bool,
    root_floor: usize,
    is_ignored: impl Fn(&[T]) -> bool,
    is_parent: impl Fn(&[T]) -> bool,
) -> Vec<&'a [T]> {
    let mut normalized = Vec::new();
    for component in components {
        if is_ignored(component) {
            continue;
        }
        if is_parent(component) {
            // `component/..` may traverse through a symlink, so it cannot be
            // erased without consulting the filesystem. Only a parent already
            // at a syntactic root is safe to discard.
            if !rooted || normalized.len() != root_floor {
                normalized.push(component);
            }
        } else {
            normalized.push(component);
        }
    }
    normalized
}

#[cfg(unix)]
fn encode_byte_key(kind: &[u8], components: Vec<&[u8]>) -> Vec<u8> {
    let mut key = key_prefix(kind);
    push_length(&mut key, components.len() as u64);
    for component in components {
        push_bytes(&mut key, component);
    }
    key
}

#[cfg(any(windows, test))]
fn encode_wide_key(kind: &[u8], prefix: &[u16], components: Vec<&[u16]>) -> Vec<u8> {
    let mut key = key_prefix(kind);
    push_wide_units(&mut key, prefix);
    push_length(&mut key, components.len() as u64);
    for component in components {
        push_wide_units(&mut key, component);
    }
    key
}

fn key_prefix(kind: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(KEY_VERSION.len() + kind.len() + 16);
    push_bytes(&mut key, KEY_VERSION);
    push_bytes(&mut key, kind);
    key
}

fn push_bytes(key: &mut Vec<u8>, value: &[u8]) {
    push_length(key, value.len() as u64);
    key.extend_from_slice(value);
}

#[cfg(any(windows, test))]
fn push_wide_units(key: &mut Vec<u8>, value: &[u16]) {
    push_length(key, value.len() as u64);
    for unit in value {
        key.extend_from_slice(&unit.to_be_bytes());
    }
}

fn push_length(key: &mut Vec<u8>, length: u64) {
    key.extend_from_slice(&length.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;

    #[cfg(any(windows, test))]
    fn wide(path: &str) -> Vec<u16> {
        path.encode_utf16().collect()
    }

    #[cfg(unix)]
    #[test]
    fn unix_keys_normalize_only_safe_lexical_syntax() {
        assert_eq!(
            unix_path_key(OsStr::new("worktrees/run-b")),
            unix_path_key(OsStr::new("./worktrees//run-b"))
        );
        assert_eq!(
            unix_path_key(OsStr::new("/run-b")),
            unix_path_key(OsStr::new("/../run-b"))
        );
        assert_ne!(
            unix_path_key(OsStr::new("run-b")),
            unix_path_key(OsStr::new("link/../run-b"))
        );
        assert_ne!(
            unix_path_key(OsStr::new("/run-b")),
            unix_path_key(OsStr::new("/link/../run-b"))
        );
        assert_ne!(
            unix_path_key(OsStr::new("../run-b")),
            unix_path_key(OsStr::new("worktrees/../../run-b"))
        );
        assert_ne!(
            unix_path_key(OsStr::new("run-b")),
            unix_path_key(OsStr::new("/run-b"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_keys_preserve_exactly_two_leading_slashes() {
        let one_slash = unix_path_key(OsStr::new("/host/share"));
        let exactly_two_slashes = unix_path_key(OsStr::new("//host/share"));
        let three_slashes = unix_path_key(OsStr::new("///host/share"));
        let four_slashes = unix_path_key(OsStr::new("////host/share"));

        assert_ne!(one_slash, exactly_two_slashes);
        assert_eq!(one_slash, three_slashes);
        assert_eq!(three_slashes, four_slashes);
    }

    #[cfg(unix)]
    #[test]
    fn unix_keys_preserve_case_and_unicode_spelling() {
        assert_ne!(
            unix_path_key(OsStr::new("/worktrees/RunA")),
            unix_path_key(OsStr::new("/worktrees/runa"))
        );
        assert_ne!(
            unix_path_key(OsStr::new("/worktrees/x\u{00e5}")),
            unix_path_key(OsStr::new("/worktrees/xa\u{030a}"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_keys_preserve_non_utf8_os_string_bytes() {
        let invalid = OsStr::from_bytes(b"/worktrees/\xff");
        let replacement = OsStr::new("/worktrees/\u{fffd}");

        assert_ne!(unix_path_key(invalid), unix_path_key(replacement));
    }

    #[test]
    fn ordinary_windows_keys_normalize_separators_and_safe_root_parents_only() {
        assert_eq!(
            windows_path_key(&wide(r"C:\managed-worktrees\run-a")),
            windows_path_key(&wide(r"C:/managed-worktrees\\.\\run-a"))
        );
        assert_eq!(
            windows_path_key(&wide(r"\\server\share")),
            windows_path_key(&wide(r"\\server\share\.."))
        );
        assert_eq!(
            windows_path_key(&wide(r"C:\run-b")),
            windows_path_key(&wide(r"C:\..\run-b"))
        );
        assert_eq!(
            windows_path_key(&wide(r"\run-b")),
            windows_path_key(&wide(r"\..\run-b"))
        );
        assert_ne!(
            windows_path_key(&wide(r"C:\run-b")),
            windows_path_key(&wide(r"C:\link\..\run-b"))
        );
        assert_ne!(
            windows_path_key(&wide(r"C:run-b")),
            windows_path_key(&wide(r"C:link\..\run-b"))
        );
        assert_ne!(
            windows_path_key(&wide(r"\\server\share\run-b")),
            windows_path_key(&wide(r"\\server\share\link\..\run-b"))
        );
        assert_ne!(
            windows_path_key(&wide(r"C:run-a")),
            windows_path_key(&wide(r"C:\run-a"))
        );
        assert_ne!(
            windows_path_key(&wide(r"C:\RunA")),
            windows_path_key(&wide(r"C:\runa"))
        );
    }

    #[test]
    fn windows_verbatim_namespaces_preserve_raw_drive_unc_and_volume_paths() {
        assert_ne!(
            windows_path_key(&wide(r"\\?\C:\run. ")),
            windows_path_key(&wide(r"C:\run. "))
        );
        assert_ne!(
            windows_path_key(&wide(r"\\?\C:\run. ")),
            windows_path_key(&wide(r"\\?\C:\run"))
        );
        assert_ne!(
            windows_path_key(&wide(r"\\?\C:\dir.\..\run. ")),
            windows_path_key(&wide(r"\\?\C:\run. "))
        );
        assert_ne!(
            windows_path_key(&wide(r"\\?\UNC\server\share\run. ")),
            windows_path_key(&wide(r"\\server\share\run. "))
        );
        assert_ne!(
            windows_path_key(&wide(
                r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\run. "
            )),
            windows_path_key(&wide(r"\\?\C:\run. "))
        );
    }

    #[test]
    fn windows_keys_do_not_assume_case_or_expanding_unicode_aliases() {
        assert_ne!(
            windows_path_key(&wide(r"C:\worktrees\RunA")),
            windows_path_key(&wide(r"C:\worktrees\runa"))
        );
        assert_ne!(
            windows_path_key(&wide(r"C:\worktrees\Straße")),
            windows_path_key(&wide(r"C:\worktrees\Strasse"))
        );
        assert_ne!(
            windows_path_key(&wide("C:\\worktrees\\x\u{00e5}")),
            windows_path_key(&wide("C:\\worktrees\\xa\u{030a}"))
        );
    }

    #[test]
    fn windows_keys_preserve_non_unicode_native_units() {
        let lone_surrogate = vec![b'C' as u16, b':' as u16, b'\\' as u16, 0xd800];
        let other_surrogate = vec![b'C' as u16, b':' as u16, b'\\' as u16, 0xd801];

        assert_eq!(
            windows_path_key(&lone_surrogate),
            windows_path_key(&lone_surrogate)
        );
        assert_ne!(
            windows_path_key(&lone_surrogate),
            windows_path_key(&other_surrogate)
        );
    }

    #[test]
    fn length_framing_preserves_values_beyond_u32() {
        let mut u32_max = Vec::new();
        let mut next_value = Vec::new();

        push_length(&mut u32_max, u64::from(u32::MAX));
        push_length(&mut next_value, u64::from(u32::MAX) + 1);

        assert_eq!(u32_max, u64::from(u32::MAX).to_be_bytes());
        assert_eq!(next_value, (u64::from(u32::MAX) + 1).to_be_bytes());
        assert_ne!(u32_max, next_value);
    }
}
