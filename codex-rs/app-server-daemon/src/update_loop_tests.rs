use pretty_assertions::assert_eq;

use super::RESTART_RETRY_INTERVAL;
use super::UpdateAttempt;
use super::next_update_attempt;
use super::retry_delay;
use super::update_failure_diagnostic;
use super::update_modes_for_identities;
use crate::RestartIfRunningOutcome;
use crate::RestartMode;
use crate::UpdaterRefreshMode;
use crate::managed_install::executable_identity_from_bytes;

#[test]
fn unchanged_updater_uses_version_based_restart() {
    assert_eq!(
        update_modes_for_identities(
            &executable_identity_from_bytes(b"same"),
            &executable_identity_from_bytes(b"same"),
        ),
        (RestartMode::IfVersionChanged, UpdaterRefreshMode::None)
    );
}

#[test]
fn changed_updater_forces_refresh_even_when_version_may_match() {
    assert_eq!(
        update_modes_for_identities(
            &executable_identity_from_bytes(b"old"),
            &executable_identity_from_bytes(b"new"),
        ),
        (
            RestartMode::Always,
            UpdaterRefreshMode::ReexecIfManagedBinaryChanged,
        )
    );
}

#[test]
fn updater_failure_diagnostic_includes_error_chain() {
    let err = anyhow::anyhow!("fetch failed").context("update pass failed");

    assert_eq!(
        update_failure_diagnostic(&err),
        "Codewith app-server daemon updater failed: update pass failed: fetch failed"
    );
}

#[test]
fn local_update_deferral_uses_short_bounded_retry_cadence() {
    assert_eq!(RESTART_RETRY_INTERVAL, std::time::Duration::from_secs(5));
    assert_eq!(
        [
            RestartIfRunningOutcome::Busy,
            RestartIfRunningOutcome::Deferred,
            RestartIfRunningOutcome::AlreadyCurrent,
            RestartIfRunningOutcome::Started,
        ]
        .map(retry_delay),
        [
            Some(std::time::Duration::from_secs(5)),
            Some(std::time::Duration::from_secs(5)),
            None,
            None,
        ]
    );
    assert_eq!(
        next_update_attempt(UpdateAttempt::Initial, RestartIfRunningOutcome::Deferred),
        UpdateAttempt::AwaitingLocalIdle
    );
    assert_eq!(
        next_update_attempt(
            UpdateAttempt::AwaitingLocalIdle,
            RestartIfRunningOutcome::Busy
        ),
        UpdateAttempt::AwaitingLocalIdle
    );
}
