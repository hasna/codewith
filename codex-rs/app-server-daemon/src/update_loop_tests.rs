use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use pretty_assertions::assert_eq;

use super::UpdateCycleOutcome;
use super::complete_update_cycle;
use super::perform_update_cycle;
use super::update_failure_diagnostic;
use crate::managed_install::executable_identity_from_bytes;

async fn write_managed_binary(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let managed_codex_bin = temp_dir.path().join("codewith");
    tokio::fs::write(&managed_codex_bin, bytes)
        .await
        .expect("write managed binary");
    (temp_dir, managed_codex_bin)
}

#[tokio::test]
async fn perform_update_cycle_reexecs_after_installed_identity_changes() {
    let running_identity = executable_identity_from_bytes(b"old");
    let (_temp_dir, managed_codex_bin) = write_managed_binary(b"old").await;
    let install_path = managed_codex_bin.clone();
    let requested_paths = Arc::new(Mutex::new(Vec::new()));
    let requested_paths_for_reexec = Arc::clone(&requested_paths);

    let outcome = perform_update_cycle(&running_identity, &managed_codex_bin, move || async move {
        tokio::fs::write(install_path, b"new").await?;
        Ok(())
    })
    .await
    .expect("perform update cycle");
    complete_update_cycle(outcome, move |path| {
        requested_paths_for_reexec
            .lock()
            .expect("lock requested paths")
            .push(path.to_path_buf());
        Ok(())
    })
    .expect("complete update cycle");

    assert_eq!(
        *requested_paths.lock().expect("lock requested paths"),
        vec![std::fs::canonicalize(&managed_codex_bin).expect("resolve managed binary")]
    );
}

#[tokio::test]
async fn perform_update_cycle_does_not_reexec_for_unchanged_identity() {
    let running_identity = executable_identity_from_bytes(b"same");
    let (_temp_dir, managed_codex_bin) = write_managed_binary(b"same").await;
    let reexec_count = Arc::new(AtomicUsize::new(0));
    let reexec_count_for_cycle = Arc::clone(&reexec_count);

    let outcome = perform_update_cycle(&running_identity, &managed_codex_bin, || async { Ok(()) })
        .await
        .expect("perform update cycle");
    assert_eq!(outcome, UpdateCycleOutcome::Unchanged);
    complete_update_cycle(outcome, move |_path| {
        reexec_count_for_cycle.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect("complete update cycle");

    assert_eq!(reexec_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn perform_update_cycle_stops_after_install_failure() {
    let running_identity = executable_identity_from_bytes(b"old");
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let missing_managed_codex_bin = temp_dir.path().join("missing-codewith");

    let err = perform_update_cycle(&running_identity, &missing_managed_codex_bin, || async {
        Err(anyhow::anyhow!("install failed"))
    })
    .await
    .expect_err("install failure should fail the update cycle");

    assert_eq!(format!("{err:#}"), "install failed");
}

#[test]
fn updater_loop_does_not_consult_app_server_lifecycle() {
    let source = include_str!("update_loop.rs");
    let crate_references = source
        .lines()
        .filter(|line| line.contains("crate::"))
        .collect::<Vec<_>>();
    assert_eq!(
        crate_references,
        vec![
            "use crate::managed_install::ExecutableIdentity;",
            "use crate::managed_install::executable_identity;",
            "use crate::managed_install::managed_codex_bin;",
            "use crate::managed_install::resolved_managed_codex_bin;",
        ]
    );
    assert!(!source.contains("super::"));
    for forbidden in [
        "Daemon::from_environment",
        "RestartIfRunningOutcome",
        "RestartMode",
        "UpdaterRefreshMode",
        "load_settings",
        "try_complete_deferred_restart",
        "try_restart_if_running",
    ] {
        assert!(
            !source.contains(forbidden),
            "update loop must not depend on app-server lifecycle symbol {forbidden}"
        );
    }

    let termination_check = source
        .find("terminate.recv().now_or_never()")
        .expect("termination check");
    let updater_handoff = source
        .find("complete_update_cycle(outcome")
        .expect("updater handoff");
    let normal_sleep = source
        .find("sleep_or_terminate(UPDATE_INTERVAL")
        .expect("normal update sleep");
    assert!(termination_check < updater_handoff);
    assert!(updater_handoff < normal_sleep);
    assert!(source.contains(".env_remove(\"CODEWITH_RELEASE\")"));
    assert!(source.contains(".env_remove(\"CODEX_RELEASE\")"));
}

#[test]
fn updater_failure_diagnostic_includes_error_chain() {
    let err = anyhow::anyhow!("fetch failed").context("update pass failed");

    assert_eq!(
        update_failure_diagnostic(&err),
        "Codewith app-server daemon updater failed: update pass failed: fetch failed"
    );
}
