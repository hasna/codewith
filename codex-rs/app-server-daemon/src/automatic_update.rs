use std::future::Future;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use tokio::time::sleep;

use crate::Daemon;
use crate::backend;
use crate::client;
use crate::client::ProbeInfo;
use crate::managed_install::managed_codex_version;
use crate::settings::DaemonSettings;
use crate::try_lock_file;

const LOCAL_PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const LOCAL_PROBE_ATTEMPTS: usize = 5;

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
    if !backend_running {
        return AutomaticRestartAction::LeaveNotRunning;
    }
    if !remote_control_enabled {
        return match (mode, info, managed_version) {
            (RestartMode::IfVersionChanged, Some(info), Some(managed_version))
                if info.app_server_version == managed_version =>
            {
                AutomaticRestartAction::AlreadyCurrent
            }
            (_, Some(_), _) => AutomaticRestartAction::Defer,
            (_, None, _) => AutomaticRestartAction::Restart,
        };
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

fn deferred_restart_action(
    remote_control_enabled: bool,
    backend_running: bool,
) -> AutomaticRestartAction {
    if remote_control_enabled {
        AutomaticRestartAction::LeaveNotRunning
    } else if backend_running {
        AutomaticRestartAction::Defer
    } else {
        AutomaticRestartAction::Start
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
        && matches!(
            outcome,
            RestartIfRunningOutcome::Started | RestartIfRunningOutcome::Restarted
        )
}

/// Effects applied after the updater has classified the managed backend.
///
/// Implementations keep stop, start, and readiness validation ordered.
trait AutomaticUpdateEffects {
    fn stop_backend(&self) -> impl Future<Output = Result<()>> + Send;
    fn start_backend(&self) -> impl Future<Output = Result<()>> + Send;
    fn wait_until_ready(&self) -> impl Future<Output = Result<()>> + Send;
}

struct DaemonAutomaticUpdateEffects<'a> {
    daemon: &'a Daemon,
    settings: &'a DaemonSettings,
    managed_codex_bin: &'a Path,
    backend: Option<&'a backend::PidBackend>,
}

impl AutomaticUpdateEffects for DaemonAutomaticUpdateEffects<'_> {
    async fn stop_backend(&self) -> Result<()> {
        let Some(backend) = self.backend else {
            return Err(anyhow!("automatic update restart has no running backend"));
        };
        backend.stop().await
    }

    async fn start_backend(&self) -> Result<()> {
        let _ = self
            .daemon
            .start_backend_with_bin(self.settings, self.managed_codex_bin)
            .await?;
        Ok(())
    }

    async fn wait_until_ready(&self) -> Result<()> {
        self.daemon.wait_until_ready().await?;
        Ok(())
    }
}

async fn apply_automatic_restart_action<E>(
    effects: &E,
    action: AutomaticRestartAction,
) -> Result<RestartIfRunningOutcome>
where
    E: AutomaticUpdateEffects,
{
    match action {
        AutomaticRestartAction::Defer => Ok(RestartIfRunningOutcome::Deferred),
        AutomaticRestartAction::LeaveNotRunning => Ok(RestartIfRunningOutcome::NotRunning),
        AutomaticRestartAction::NotReady => Ok(RestartIfRunningOutcome::NotReady),
        AutomaticRestartAction::AlreadyCurrent => Ok(RestartIfRunningOutcome::AlreadyCurrent),
        AutomaticRestartAction::Start => {
            effects.start_backend().await?;
            effects.wait_until_ready().await?;
            Ok(RestartIfRunningOutcome::Started)
        }
        AutomaticRestartAction::Restart => {
            effects.stop_backend().await?;
            effects.start_backend().await?;
            effects.wait_until_ready().await?;
            Ok(RestartIfRunningOutcome::Restarted)
        }
    }
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
        let backend = self.running_backend_instance(&settings).await?;
        let (info, managed_version) = if backend.is_some() {
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
            (info, managed_version)
        } else {
            if client::probe(&self.socket_path).await.is_ok() {
                return Err(anyhow!(
                    "app server is running but is not managed by codewith app-server daemon"
                ));
            }
            (None, None)
        };
        let action = automatic_restart_action(
            settings.remote_control_enabled,
            backend.is_some(),
            mode,
            info.as_ref(),
            managed_version.as_deref(),
        );
        let effects = DaemonAutomaticUpdateEffects {
            daemon: self,
            settings: &settings,
            managed_codex_bin,
            backend: backend.as_ref(),
        };
        let outcome = apply_automatic_restart_action(&effects, action).await?;

        if should_reexec_updater(updater_refresh_mode, outcome) {
            crate::update_loop::reexec_managed_updater(managed_codex_bin)?;
        }

        Ok(outcome)
    }

    pub(crate) async fn try_complete_deferred_restart(
        &self,
        updater_refresh_mode: UpdaterRefreshMode,
        managed_codex_bin: &Path,
    ) -> Result<RestartIfRunningOutcome> {
        let operation_lock = self.open_operation_lock_file().await?;
        if !try_lock_file(&operation_lock)? {
            return Ok(RestartIfRunningOutcome::Busy);
        }
        let settings = self.load_settings().await?;
        let backend = self.running_backend_instance(&settings).await?;
        let action = deferred_restart_action(settings.remote_control_enabled, backend.is_some());
        if action == AutomaticRestartAction::Start && client::probe(&self.socket_path).await.is_ok()
        {
            return Err(anyhow!(
                "app server is running but is not managed by codewith app-server daemon"
            ));
        }
        let effects = DaemonAutomaticUpdateEffects {
            daemon: self,
            settings: &settings,
            managed_codex_bin,
            backend: backend.as_ref(),
        };
        let outcome = apply_automatic_restart_action(&effects, action).await?;

        if should_reexec_updater(updater_refresh_mode, outcome) {
            crate::update_loop::reexec_managed_updater(managed_codex_bin)?;
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::AutomaticRestartAction;
    use super::AutomaticUpdateEffects;
    use super::LOCAL_PROBE_ATTEMPTS;
    use super::RestartIfRunningOutcome;
    use super::RestartMode;
    use super::UpdaterRefreshMode;
    use super::apply_automatic_restart_action;
    use super::automatic_restart_action;
    use super::deferred_restart_action;
    use super::probe_local_endpoint;
    use super::should_reexec_updater;
    use crate::client::ProbeInfo;

    fn probe_info(version: &str) -> ProbeInfo {
        ProbeInfo {
            app_server_version: version.to_string(),
        }
    }

    #[derive(Default)]
    struct RecordingEffects {
        mutations: Mutex<Vec<&'static str>>,
    }

    impl RecordingEffects {
        fn mutations(&self) -> Vec<&'static str> {
            self.mutations.lock().expect("mutations lock").clone()
        }
    }

    impl AutomaticUpdateEffects for RecordingEffects {
        async fn stop_backend(&self) -> anyhow::Result<()> {
            self.mutations.lock().expect("mutations lock").push("stop");
            Ok(())
        }

        async fn start_backend(&self) -> anyhow::Result<()> {
            self.mutations.lock().expect("mutations lock").push("start");
            Ok(())
        }

        async fn wait_until_ready(&self) -> anyhow::Result<()> {
            self.mutations.lock().expect("mutations lock").push("wait");
            Ok(())
        }
    }

    async fn apply_action(
        action: AutomaticRestartAction,
    ) -> (RestartIfRunningOutcome, Vec<&'static str>) {
        let effects = RecordingEffects::default();
        let outcome = apply_automatic_restart_action(&effects, action)
            .await
            .expect("apply action");
        (outcome, effects.mutations())
    }

    #[tokio::test]
    async fn local_running_update_defers_without_backend_mutation() {
        let action = automatic_restart_action(
            /*remote_control_enabled*/ false,
            /*backend_running*/ true,
            RestartMode::Always,
            Some(&probe_info("0.1.77")),
            Some("0.1.78"),
        );
        assert_eq!(
            apply_action(action).await,
            (RestartIfRunningOutcome::Deferred, vec![])
        );
    }

    #[tokio::test]
    async fn local_update_starts_latest_after_deferred_daemon_exits() {
        let action = deferred_restart_action(
            /*remote_control_enabled*/ false, /*backend_running*/ false,
        );
        assert_eq!(
            apply_action(action).await,
            (RestartIfRunningOutcome::Started, vec!["start", "wait"])
        );
    }

    #[tokio::test]
    async fn local_update_preserves_stopped_state_without_prior_deferral() {
        let action = automatic_restart_action(
            /*remote_control_enabled*/ false,
            /*backend_running*/ false,
            RestartMode::Always,
            /*info*/ None,
            /*managed_version*/ None,
        );
        assert_eq!(
            apply_action(action).await,
            (RestartIfRunningOutcome::NotRunning, vec![])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_unresponsive_endpoint_recovers_after_bounded_probe_confirmation() {
        let probe_count = Arc::new(AtomicUsize::new(0));
        let probe_count_for_closure = Arc::clone(&probe_count);
        let info = probe_local_endpoint(move || {
            probe_count_for_closure.fetch_add(1, Ordering::SeqCst);
            async { Err(anyhow::anyhow!("not listening")) }
        })
        .await;
        let action = automatic_restart_action(
            /*remote_control_enabled*/ false,
            /*backend_running*/ true,
            RestartMode::IfVersionChanged,
            info.as_ref(),
            /*managed_version*/ None,
        );
        assert_eq!(probe_count.load(Ordering::SeqCst), LOCAL_PROBE_ATTEMPTS);
        assert_eq!(
            apply_action(action).await,
            (
                RestartIfRunningOutcome::Restarted,
                vec!["stop", "start", "wait"],
            )
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

    #[tokio::test]
    async fn remote_control_update_still_restarts_immediately() {
        let action = automatic_restart_action(
            /*remote_control_enabled*/ true,
            /*backend_running*/ true,
            RestartMode::Always,
            Some(&probe_info("0.1.77")),
            Some("0.1.78"),
        );
        assert_eq!(
            apply_action(action).await,
            (
                RestartIfRunningOutcome::Restarted,
                vec!["stop", "start", "wait"],
            )
        );
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
