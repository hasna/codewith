//! Server-side authorization scope for app-server `fs/*` filesystem RPCs.
//!
//! The `fs/*` methods accept absolute host paths chosen by the client. Without a
//! server-side scope check, a generic initialized client could read, write,
//! copy, watch, or delete arbitrary host paths outside the session's workspace
//! (CWE-862 Missing Authorization, CWE-22 Path Traversal, CWE-284 Improper
//! Access Control). [`FsScope`] pins those operations to an explicit set of
//! authorized workspace roots derived from server-held state (the session cwd
//! plus configured workspace roots) rather than trusting the request path.
//!
//! Every target is fully canonicalized before the containment check so symlink
//! escapes (a link inside a root whose real target is outside it) are rejected.
//! For write/mkdir/copy-destination targets that do not exist yet, the deepest
//! existing ancestor is canonicalized and the missing suffix re-attached, so a
//! symlinked ancestor cannot be used to escape the scope.

use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::config::Config;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Authorized filesystem roots for app-server `fs/*` operations.
///
/// Cloning is cheap; the canonicalized roots are shared behind an `Arc`.
#[derive(Clone)]
pub(crate) struct FsScope {
    /// Canonicalized allowed roots. Empty means default-deny.
    roots: Arc<Vec<PathBuf>>,
}

impl FsScope {
    /// Builds a scope from an explicit set of authorized roots. Roots that can
    /// be canonicalized are stored canonicalized so containment checks compare
    /// resolved paths on both sides; roots that cannot be canonicalized (for
    /// example a configured directory that does not exist yet) fall back to
    /// their normalized logical path.
    pub(crate) fn new(roots: impl IntoIterator<Item = AbsolutePathBuf>) -> Self {
        let mut canonical_roots: Vec<PathBuf> = Vec::new();
        for root in roots {
            let canonical = match root.canonicalize() {
                Ok(canonical) => canonical.into_path_buf(),
                Err(_) => root.into_path_buf(),
            };
            if !canonical_roots.contains(&canonical) {
                canonical_roots.push(canonical);
            }
        }
        Self {
            roots: Arc::new(canonical_roots),
        }
    }

    /// Derives the authorized roots from server-held session state: the active
    /// working directory plus the effective workspace roots. This is the trust
    /// boundary for `fs/*`; it never trusts a client-supplied root.
    pub(crate) fn from_config(config: &Config) -> Self {
        let mut roots = Vec::new();
        roots.push(config.cwd.clone());
        roots.extend(config.effective_workspace_roots());
        Self::new(roots)
    }

    /// Authorizes a target path that is expected to exist (read, metadata,
    /// readdir, remove, copy source, watch). Rejects paths whose canonical
    /// location is outside every authorized root, including symlink escapes.
    pub(crate) fn authorize_existing(
        &self,
        path: &AbsolutePathBuf,
    ) -> Result<(), JSONRPCErrorError> {
        self.authorize(path)
    }

    /// Authorizes a target path that may not exist yet (write, mkdir, copy
    /// destination). The deepest existing ancestor is canonicalized so a
    /// symlinked ancestor cannot escape the scope.
    pub(crate) fn authorize_target(&self, path: &AbsolutePathBuf) -> Result<(), JSONRPCErrorError> {
        self.authorize(path)
    }

    fn authorize(&self, path: &AbsolutePathBuf) -> Result<(), JSONRPCErrorError> {
        if self.roots.is_empty() {
            return Err(denied_error());
        }
        let resolved = canonicalize_allowing_missing(path).map_err(|err| {
            internal_error(format!(
                "failed to resolve fs path for authorization: {err}"
            ))
        })?;
        if self
            .roots
            .iter()
            .any(|root| path_is_within(&resolved, root))
        {
            Ok(())
        } else {
            Err(denied_error())
        }
    }
}

fn denied_error() -> JSONRPCErrorError {
    invalid_request("path is outside the authorized workspace roots")
}

fn path_is_within(resolved: &Path, root: &Path) -> bool {
    resolved == root || resolved.starts_with(root)
}

/// Canonicalizes `path`, tolerating a non-existent tail. The deepest existing
/// ancestor is canonicalized (fully resolving symlinks) and the missing suffix
/// is re-attached. `AbsolutePathBuf` is already normalized, so the suffix
/// contains no `..`/`.` components that could climb back out of the resolved
/// ancestor.
fn canonicalize_allowing_missing(path: &AbsolutePathBuf) -> std::io::Result<PathBuf> {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.clone();
    loop {
        match current.canonicalize() {
            Ok(resolved) => {
                let mut resolved = resolved.into_path_buf();
                for component in suffix.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let Some(file_name) = current.as_path().file_name() else {
                    return Err(err);
                };
                suffix.push(file_name.to_os_string());
                let Some(parent) = current.parent() else {
                    return Err(err);
                };
                current = parent;
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn abs(path: impl AsRef<Path>) -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path(path.as_ref()).expect("absolute path")
    }

    #[test]
    fn allows_paths_inside_root() {
        let root = TempDir::new().expect("temp dir");
        let scope = FsScope::new([abs(root.path())]);
        let nested = root.path().join("sub").join("file.txt");
        std::fs::create_dir_all(nested.parent().expect("parent")).expect("mkdir");
        std::fs::write(&nested, "hi").expect("write");

        scope
            .authorize_existing(&abs(&nested))
            .expect("existing path inside root should be allowed");
    }

    #[test]
    fn allows_missing_target_inside_root() {
        let root = TempDir::new().expect("temp dir");
        let scope = FsScope::new([abs(root.path())]);
        let target = root.path().join("does").join("not").join("exist.txt");

        scope
            .authorize_target(&abs(&target))
            .expect("missing target inside root should be allowed");
    }

    #[test]
    fn rejects_paths_outside_root() {
        let root = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("temp dir");
        let scope = FsScope::new([abs(root.path())]);
        let target = outside.path().join("secret.txt");
        std::fs::write(&target, "secret").expect("write");

        let err = scope
            .authorize_existing(&abs(&target))
            .expect_err("path outside root should be denied");
        assert_eq!(
            err.message,
            "path is outside the authorized workspace roots"
        );
    }

    #[test]
    fn rejects_sibling_prefix_paths() {
        // `/root/../root-evil` must not be considered inside `/root`.
        let base = TempDir::new().expect("temp dir");
        let root = base.path().join("root");
        let sibling = base.path().join("root-evil");
        std::fs::create_dir_all(&root).expect("mkdir root");
        std::fs::create_dir_all(&sibling).expect("mkdir sibling");
        let scope = FsScope::new([abs(&root)]);

        let err = scope
            .authorize_existing(&abs(sibling.join("file.txt")))
            .expect_err("sibling with shared prefix should be denied");
        assert_eq!(
            err.message,
            "path is outside the authorized workspace roots"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("temp dir");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "secret").expect("write secret");
        let link = root.path().join("escape");
        symlink(outside.path(), &link).expect("symlink");
        let scope = FsScope::new([abs(root.path())]);

        // The link lives inside the root but resolves outside of it.
        let err = scope
            .authorize_existing(&abs(link.join("secret.txt")))
            .expect_err("symlink escape should be denied");
        assert_eq!(
            err.message,
            "path is outside the authorized workspace roots"
        );
    }

    #[test]
    fn empty_scope_denies_everything() {
        let scope = FsScope::new(std::iter::empty());
        let root = TempDir::new().expect("temp dir");
        let err = scope
            .authorize_existing(&abs(root.path().join("file.txt")))
            .expect_err("empty scope should default-deny");
        assert_eq!(
            err.message,
            "path is outside the authorized workspace roots"
        );
    }
}
