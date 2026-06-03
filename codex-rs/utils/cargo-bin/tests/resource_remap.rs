use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_nextest_workspace_root<T>(value: &str, f: impl FnOnce() -> T) -> T {
    let _guard = match ENV_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let _env_guard = NextestWorkspaceRootGuard {
        previous: std::env::var_os("NEXTEST_WORKSPACE_ROOT"),
    };
    unsafe {
        std::env::set_var("NEXTEST_WORKSPACE_ROOT", value);
    }
    f()
}

struct NextestWorkspaceRootGuard {
    previous: Option<OsString>,
}

impl Drop for NextestWorkspaceRootGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("NEXTEST_WORKSPACE_ROOT", previous);
            } else {
                std::env::remove_var("NEXTEST_WORKSPACE_ROOT");
            }
        }
    }
}

#[test]
fn remaps_archived_windows_manifest_dir_to_nextest_workspace_root() {
    with_nextest_workspace_root(r"C:\a\codewith\codewith\codex-rs", || {
        let resolved = codex_utils_cargo_bin::resolve_cargo_manifest_resource(
            Path::new(r"D:\a\codewith\codewith\codex-rs\tui"),
            Path::new("src/chatwidget.rs"),
        )
        .expect("resolve resource");

        assert_eq!(
            resolved,
            PathBuf::from(r"C:\a\codewith\codewith\codex-rs")
                .join("tui")
                .join("src/chatwidget.rs")
        );
    });
}

#[test]
fn remaps_unix_manifest_dir_to_nextest_workspace_root() {
    with_nextest_workspace_root("/tmp/remapped/codewith/codex-rs", || {
        let resolved = codex_utils_cargo_bin::resolve_cargo_manifest_resource(
            Path::new("/home/runner/work/codewith/codewith/codex-rs/tools"),
            Path::new("tests/fixtures/data.json"),
        )
        .expect("resolve resource");

        assert_eq!(
            resolved,
            PathBuf::from("/tmp/remapped/codewith/codex-rs")
                .join("tools")
                .join("tests/fixtures/data.json")
        );
    });
}
