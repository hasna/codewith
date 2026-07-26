use std::future::Future;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use tokio::time::sleep;

use crate::Daemon;
use crate::client;
use crate::client::ProbeInfo;
use crate::managed_install::managed_codex_version;
use crate::try_lock_file;

const LOCAL_PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const LOCAL_PROBE_ATTEMPTS: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartIfRunningOutcome {
    Busy,
    Deferred,
    NotRunning,
    NotReady,
    AlreadyCurrent,
    Started,
    Restarted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartMode {
    IfVersionChanged,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdaterRefreshMode {
    None,
    ReexecIfManagedBinaryChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomaticRestartAction {
    Defer,
    LeaveNotRunning,
    NotReady,
    AlreadyCurrent,
    Start,
    Restart,
}

fn automatic_restart_action(
    remote_control_enabled: bool,
    backend_running: bool,
    mode: RestartMode,
    info: Option<&ProbeInfo>,
    managed_version: Option<&str>,
) -> AutomaticRestartAction {
    let _ = remote_control_enabled;
    if !backend_running {
        return AutomaticRestartAction::LeaveNotRunning;
    }
    match (mode, info, managed_version) {
        (RestartMode::IfVersionChanged, None, _) => AutomaticRestartAction::NotReady,
        (RestartMode::IfVersionChanged, Some(info), Some(managed_version))
            if info.app_server_version == managed_version =>
        {
            AutomaticRestartAction::AlreadyCurrent
        }
        _ => AutomaticRestartAction::Restart,
    }
}

async fn probe_local_endpoint<Probe, ProbeFuture>(mut probe: Probe) -> Option<ProbeInfo>
where
    Probe: FnMut() -> ProbeFuture,
    ProbeFuture: Future<Output = Result<ProbeInfo>>,
{
    for attempt in 0..LOCAL_PROBE_ATTEMPTS {
        if let Ok(info) = probe().await {
            return Some(info);
        }
        if attempt + 1 < LOCAL_PROBE_ATTEMPTS {
            sleep(LOCAL_PROBE_RETRY_INTERVAL).await;
        }
    }
    None
}

fn should_reexec_updater(
    updater_refresh_mode: UpdaterRefreshMode,
    outcome: RestartIfRunningOutcome,
) -> bool {
    updater_refresh_mode == UpdaterRefreshMode::ReexecIfManagedBinaryChanged
        && outcome == RestartIfRunningOutcome::Restarted
}

impl Daemon {
    pub(crate) async fn try_restart_if_running(
        &self,
        mode: RestartMode,
        updater_refresh_mode: UpdaterRefreshMode,
        managed_codex_bin: &Path,
    ) -> Result<RestartIfRunningOutcome> {
        let operation_lock = self.open_operation_lock_file().await?;
        if !try_lock_file(&operation_lock)? {
            return Ok(RestartIfRunningOutcome::Busy);
        }
        let settings = self.load_settings().await?;
        let outcome = if let Some(backend) = self.running_backend_instance(&settings).await? {
            let info = if settings.remote_control_enabled {
                client::probe(&self.socket_path).await.ok()
            } else {
                probe_local_endpoint(|| client::probe(&self.socket_path)).await
            };
            let managed_version = if info.is_some() {
                Some(managed_codex_version(managed_codex_bin).await?)
            } else {
                None
            };
            match automatic_restart_action(
                settings.remote_control_enabled,
                /*backend_running*/ true,
                mode,
                info.as_ref(),
                managed_version.as_deref(),
            ) {
                AutomaticRestartAction::Defer => RestartIfRunningOutcome::Deferred,
                AutomaticRestartAction::LeaveNotRunning => RestartIfRunningOutcome::NotRunning,
                AutomaticRestartAction::NotReady => RestartIfRunningOutcome::NotReady,
                AutomaticRestartAction::AlreadyCurrent => RestartIfRunningOutcome::AlreadyCurrent,
                AutomaticRestartAction::Start => {
                    let _ = self
                        .start_backend_with_bin(&settings, managed_codex_bin)
                        .await?;
                    self.wait_until_ready().await?;
                    RestartIfRunningOutcome::Started
                }
                AutomaticRestartAction::Restart => {
                    backend.stop().await?;
                    let _ = self
                        .start_backend_with_bin(&settings, managed_codex_bin)
                        .await?;
                    self.wait_until_ready().await?;
                    RestartIfRunningOutcome::Restarted
                }
            }
        } else if client::probe(&self.socket_path).await.is_ok() {
            return Err(anyhow!(
                "app server is running but is not managed by codewith app-server daemon"
            ));
        } else {
            match automatic_restart_action(
                settings.remote_control_enabled,
                /*backend_running*/ false,
                mode,
                /*info*/ None,
                /*managed_version*/ None,
            ) {
                AutomaticRestartAction::Start => {
                    let _ = self
                        .start_backend_with_bin(&settings, managed_codex_bin)
                        .await?;
                    self.wait_until_ready().await?;
                    RestartIfRunningOutcome::Started
                }
                AutomaticRestartAction::LeaveNotRunning => RestartIfRunningOutcome::NotRunning,
                AutomaticRestartAction::Defer
                | AutomaticRestartAction::NotReady
                | AutomaticRestartAction::AlreadyCurrent
                | AutomaticRestartAction::Restart => {
                    unreachable!("missing backend produced a running-backend action")
                }
            }
        };

        if should_reexec_updater(updater_refresh_mode, outcome) {
            crate::update_loop::reexec_managed_updater(managed_codex_bin)?;
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::AutomaticRestartAction;
    use super::LOCAL_PROBE_ATTEMPTS;
    use super::RestartIfRunningOutcome;
    use super::RestartMode;
    use super::UpdaterRefreshMode;
    use super::automatic_restart_action;
    use super::probe_local_endpoint;
    use super::should_reexec_updater;
    use crate::client::ProbeInfo;

    fn probe_info(version: &str) -> ProbeInfo {
        ProbeInfo {
            app_server_version: version.to_string(),
        }
    }

    #[test]
    fn local_running_update_defers_without_restart() {
        assert_eq!(
            automatic_restart_action(
                /*remote_control_enabled*/ false,
                /*backend_running*/ true,
                RestartMode::Always,
                Some(&probe_info("0.1.77")),
                Some("0.1.78"),
            ),
            AutomaticRestartAction::Defer
        );
    }

    #[test]
    fn local_update_defers_then_starts_latest_after_natural_exit() {
        assert_eq!(
            [
                automatic_restart_action(
                    /*remote_control_enabled*/ false,
                    /*backend_running*/ true,
                    RestartMode::Always,
                    Some(&probe_info("0.1.77")),
                    Some("0.1.78"),
                ),
                automatic_restart_action(
                    /*remote_control_enabled*/ false,
                    /*backend_running*/ false,
                    RestartMode::Always,
                    /*info*/ None,
                    /*managed_version*/ None,
                ),
            ],
            [AutomaticRestartAction::Defer, AutomaticRestartAction::Start,]
        );
    }

    #[test]
    fn local_unresponsive_endpoint_recovers_only_after_probe_confirmation() {
        assert_eq!(
            automatic_restart_action(
                /*remote_control_enabled*/ false,
                /*backend_running*/ true,
                RestartMode::IfVersionChanged,
                /*info*/ None,
                /*managed_version*/ None,
            ),
            AutomaticRestartAction::Restart
        );
    }

    #[test]
    fn remote_control_keeps_immediate_restart_policy() {
        assert_eq!(
            [
                automatic_restart_action(
                    /*remote_control_enabled*/ true,
                    /*backend_running*/ true,
                    RestartMode::IfVersionChanged,
                    Some(&probe_info("0.1.77")),
                    Some("0.1.77"),
                ),
                automatic_restart_action(
                    /*remote_control_enabled*/ true,
                    /*backend_running*/ true,
                    RestartMode::IfVersionChanged,
                    /*info*/ None,
                    /*managed_version*/ None,
                ),
                automatic_restart_action(
                    /*remote_control_enabled*/ true,
                    /*backend_running*/ true,
                    RestartMode::Always,
                    Some(&probe_info("0.1.77")),
                    Some("0.1.78"),
                ),
                automatic_restart_action(
                    /*remote_control_enabled*/ true,
                    /*backend_running*/ false,
                    RestartMode::Always,
                    /*info*/ None,
                    /*managed_version*/ None,
                ),
            ],
            [
                AutomaticRestartAction::AlreadyCurrent,
                AutomaticRestartAction::NotReady,
                AutomaticRestartAction::Restart,
                AutomaticRestartAction::LeaveNotRunning,
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_unresponsive_probe_is_retried_to_a_bounded_limit() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_probe = Arc::clone(&attempts);

        assert_eq!(
            probe_local_endpoint(move || {
                attempts_for_probe.fetch_add(1, Ordering::SeqCst);
                async { Err(anyhow::anyhow!("not listening")) }
            })
            .await,
            None
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 5);
        assert_eq!(LOCAL_PROBE_ATTEMPTS, 5);
    }

    #[test]
    fn updater_reexecs_after_latest_local_daemon_starts() {
        assert!(should_reexec_updater(
            UpdaterRefreshMode::ReexecIfManagedBinaryChanged,
            RestartIfRunningOutcome::Started,
        ));
    }

    #[test]
    fn unchanged_updater_never_reexecs() {
        assert_eq!(
            [
                RestartIfRunningOutcome::Busy,
                RestartIfRunningOutcome::Deferred,
                RestartIfRunningOutcome::NotRunning,
                RestartIfRunningOutcome::NotReady,
                RestartIfRunningOutcome::AlreadyCurrent,
                RestartIfRunningOutcome::Started,
                RestartIfRunningOutcome::Restarted,
            ]
            .map(|outcome| should_reexec_updater(UpdaterRefreshMode::None, outcome)),
            [false, false, false, false, false, false, false]
        );
    }
}
