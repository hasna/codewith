mod backend;
mod client;
mod managed_install;
mod remote_control_client;
mod settings;
mod update_loop;

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
pub use backend::BackendKind;
use backend::BackendPaths;
use codex_app_server_protocol::RemoteControlConnectionStatus;
use codex_app_server_transport::app_server_control_socket_path;
use codex_app_server_transport::selected_auth_profile_from_env;
use codex_utils_home_dir::find_codex_home;
use managed_install::managed_codex_bin;
#[cfg(unix)]
use managed_install::managed_codex_version;
use serde::Serialize;
use settings::DaemonSettings;
use tokio::time::sleep;

const START_POLL_INTERVAL: Duration = Duration::from_millis(50);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const CLIENT_LEASE_SETTLE_TIMEOUT: Duration = Duration::from_millis(500);
const OPERATION_LOCK_TIMEOUT: Duration = Duration::from_secs(75);
const PID_FILE_NAME: &str = "app-server.pid";
const UPDATE_PID_FILE_NAME: &str = "app-server-updater.pid";
const OPERATION_LOCK_FILE_NAME: &str = "daemon.lock";
pub(crate) const CLIENT_LEASE_FILE_NAME: &str = "client.lock";
const UPDATER_LEASE_CAPABILITY_FILE_NAME: &str = "lease-aware-updater.lock";
const SETTINGS_FILE_NAME: &str = "settings.json";
const STATE_DIR_NAME: &str = "app-server-daemon";
const AUTH_PROFILE_STATE_DIR_NAME: &str = "auth_profiles";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCommand {
    Start,
    Restart,
    Stop,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleStatus {
    AlreadyRunning,
    Started,
    Restarted,
    Stopped,
    NotRunning,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleOutput {
    pub status: LifecycleStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub managed_codex_path: PathBuf,
    pub managed_codex_version: Option<String>,
    pub socket_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_server_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapOptions {
    pub remote_control_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDaemonStartOptions {
    pub codex_bin: PathBuf,
}

/// Shared lease held by a local client for the lifetime of its daemon connection.
///
/// Automatic updater restarts are deferred while any lease is alive.
#[derive(Debug)]
pub struct LocalDaemonLease {
    _client_file: tokio::fs::File,
    _legacy_operation_file: Option<tokio::fs::File>,
}

/// Passively probes an existing app-server socket and returns its reported
/// app-server version.
pub async fn probe_app_server_version(socket_path: &Path) -> Result<String> {
    Ok(client::probe(socket_path).await?.app_server_version)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BootstrapStatus {
    Bootstrapped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapOutput {
    pub status: BootstrapStatus,
    pub backend: BackendKind,
    pub auto_update_enabled: bool,
    pub remote_control_enabled: bool,
    pub managed_codex_path: PathBuf,
    pub managed_codex_version: Option<String>,
    pub socket_path: PathBuf,
    pub cli_version: String,
    pub app_server_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum RemoteControlStartOutput {
    Bootstrap(BootstrapOutput),
    Start(LifecycleOutput),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlReadyStatus {
    pub status: RemoteControlConnectionStatus,
    pub server_name: String,
    pub environment_id: Option<String>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlReadyOutput {
    pub daemon: RemoteControlStartOutput,
    pub remote_control: RemoteControlReadyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlMode {
    Enabled,
    Disabled,
}

impl RemoteControlMode {
    fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteControlStatus {
    Enabled,
    Disabled,
    AlreadyEnabled,
    AlreadyDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlOutput {
    pub status: RemoteControlStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendKind>,
    pub remote_control_enabled: bool,
    pub socket_path: PathBuf,
    pub cli_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_server_version: Option<String>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartIfRunningOutcome {
    Busy,
    Deferred,
    NotRunning,
    NotReady,
    AlreadyCurrent,
    Restarted,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartMode {
    IfVersionChanged,
    Always,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdaterRefreshMode {
    None,
    ReexecIfManagedBinaryChanged,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartDecision {
    NotReady,
    AlreadyCurrent,
    Deferred,
    Restart,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartAvailability {
    Available,
    ActiveLocalClient,
    LegacyIdleRetiringDaemon,
}

pub async fn run(command: LifecycleCommand) -> Result<LifecycleOutput> {
    ensure_supported_platform()?;
    Daemon::from_environment()?.run(command).await
}

pub async fn ensure_local_daemon_started(options: LocalDaemonStartOptions) -> Result<PathBuf> {
    ensure_supported_platform()?;
    let daemon = Daemon::from_environment_with_app_server_codex_bin(options.codex_bin)?;
    let _operation_lock = daemon.acquire_operation_lock().await?;
    daemon.start_local().await.map(|output| output.socket_path)
}

/// Acquires a shared local-client lease for the default app-server daemon.
///
/// Callers should retain the returned lease until their durable connection has
/// closed so automatic updates cannot reset that connection.
pub async fn acquire_local_daemon_lease(codex_home: &Path) -> Result<LocalDaemonLease> {
    ensure_supported_platform()?;
    let daemon = Daemon::from_codex_home_and_app_server_codex_bin(
        codex_home,
        managed_codex_bin(codex_home),
    )?;
    let operation_lock = daemon.acquire_shared_operation_lock().await?;
    daemon
        .acquire_client_lease_with_migration_guard(operation_lock)
        .await
}

pub async fn bootstrap(options: BootstrapOptions) -> Result<BootstrapOutput> {
    ensure_supported_platform()?;
    Daemon::from_environment()?.bootstrap(options).await
}

pub async fn ensure_remote_control_started() -> Result<RemoteControlStartOutput> {
    ensure_supported_platform()?;
    Daemon::from_environment()?
        .ensure_remote_control_started()
        .await
}

pub async fn ensure_remote_control_ready() -> Result<RemoteControlReadyOutput> {
    ensure_supported_platform()?;
    Daemon::from_environment()?
        .ensure_remote_control_ready()
        .await
}

pub async fn enable_remote_control_on_socket(
    socket_path: &Path,
    connect_timeout: Duration,
    connect_retry_delay: Duration,
) -> Result<RemoteControlReadyStatus> {
    ensure_supported_platform()?;
    remote_control_client::enable_remote_control_with_connect_retry(
        socket_path,
        connect_timeout,
        connect_retry_delay,
    )
    .await
}

pub async fn set_remote_control(mode: RemoteControlMode) -> Result<RemoteControlOutput> {
    ensure_supported_platform()?;
    Daemon::from_environment()?.set_remote_control(mode).await
}

pub async fn run_pid_update_loop() -> Result<()> {
    ensure_supported_platform()?;
    update_loop::run().await
}

#[cfg(unix)]
fn ensure_supported_platform() -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn ensure_supported_platform() -> Result<()> {
    Err(anyhow!(
        "codewith app-server daemon lifecycle is only supported on Unix platforms"
    ))
}

struct Daemon {
    socket_path: PathBuf,
    pid_file: PathBuf,
    update_pid_file: PathBuf,
    operation_lock_file: PathBuf,
    client_lease_file: PathBuf,
    updater_lease_capability_file: PathBuf,
    settings_file: PathBuf,
    managed_codex_bin: PathBuf,
    app_server_codex_bin: PathBuf,
}

impl Daemon {
    fn from_environment() -> Result<Self> {
        let codex_home = find_codex_home().context("failed to resolve CODEWITH_HOME")?;
        let managed_codex_bin = managed_codex_bin(codex_home.as_path());
        Self::from_codex_home_and_app_server_codex_bin(codex_home.as_path(), managed_codex_bin)
    }

    fn from_environment_with_app_server_codex_bin(app_server_codex_bin: PathBuf) -> Result<Self> {
        let codex_home = find_codex_home().context("failed to resolve CODEWITH_HOME")?;
        Self::from_codex_home_and_app_server_codex_bin(codex_home.as_path(), app_server_codex_bin)
    }

    fn from_codex_home_and_app_server_codex_bin(
        codex_home: &Path,
        app_server_codex_bin: PathBuf,
    ) -> Result<Self> {
        let managed_codex_bin = managed_codex_bin(codex_home);
        let socket_path = app_server_control_socket_path(codex_home)?
            .as_path()
            .to_path_buf();
        let state_dir = app_server_daemon_state_dir(codex_home)?;
        Ok(Self {
            socket_path,
            pid_file: state_dir.join(PID_FILE_NAME),
            update_pid_file: state_dir.join(UPDATE_PID_FILE_NAME),
            operation_lock_file: state_dir.join(OPERATION_LOCK_FILE_NAME),
            client_lease_file: state_dir.join(CLIENT_LEASE_FILE_NAME),
            updater_lease_capability_file: state_dir.join(UPDATER_LEASE_CAPABILITY_FILE_NAME),
            settings_file: state_dir.join(SETTINGS_FILE_NAME),
            managed_codex_bin,
            app_server_codex_bin,
        })
    }

    async fn run(&self, command: LifecycleCommand) -> Result<LifecycleOutput> {
        match command {
            LifecycleCommand::Start => {
                let _operation_lock = self.acquire_operation_lock().await?;
                self.start().await
            }
            LifecycleCommand::Restart => {
                let _operation_lock = self.acquire_operation_lock().await?;
                self.restart().await
            }
            LifecycleCommand::Stop => {
                let _operation_lock = self.acquire_operation_lock().await?;
                self.stop().await
            }
            LifecycleCommand::Version => self.version().await,
        }
    }

    async fn start(&self) -> Result<LifecycleOutput> {
        let settings = self.load_settings().await?;
        self.start_with_settings(&settings).await
    }

    async fn start_local(&self) -> Result<LifecycleOutput> {
        self.start_with_settings(&Self::local_start_settings())
            .await
    }

    fn local_start_settings() -> DaemonSettings {
        DaemonSettings::default()
    }

    async fn start_with_settings(&self, settings: &DaemonSettings) -> Result<LifecycleOutput> {
        if let Ok(info) = client::probe(&self.socket_path).await {
            return Ok(self
                .output(
                    LifecycleStatus::AlreadyRunning,
                    self.running_backend(settings).await?,
                    /*pid*/ None,
                    Some(info.app_server_version),
                )
                .await);
        }

        if self.running_backend_instance(settings).await?.is_some() {
            let info = self.wait_until_ready().await?;
            return Ok(self
                .output(
                    LifecycleStatus::AlreadyRunning,
                    Some(BackendKind::Pid),
                    /*pid*/ None,
                    Some(info.app_server_version),
                )
                .await);
        }

        self.ensure_app_server_codex_bin()?;
        let pid = self.start_app_server_backend(settings).await?;
        let info = self.wait_until_ready().await?;
        Ok(self
            .output(
                LifecycleStatus::Started,
                Some(BackendKind::Pid),
                pid,
                Some(info.app_server_version),
            )
            .await)
    }

    async fn restart(&self) -> Result<LifecycleOutput> {
        let settings = self.load_settings().await?;
        if client::probe(&self.socket_path).await.is_ok()
            && self.running_backend(&settings).await?.is_none()
        {
            return Err(anyhow!(
                "app server is running but is not managed by codewith app-server daemon"
            ));
        }

        self.ensure_app_server_codex_bin()?;
        if let Some(backend) = self.running_backend_instance(&settings).await? {
            backend.stop().await?;
        }

        let pid = self.start_app_server_backend(&settings).await?;
        let info = self.wait_until_ready().await?;
        Ok(self
            .output(
                LifecycleStatus::Restarted,
                Some(BackendKind::Pid),
                pid,
                Some(info.app_server_version),
            )
            .await)
    }

    #[cfg(unix)]
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
            let info = client::probe(&self.socket_path).await.ok();
            let managed_version = if info.is_some() {
                Some(managed_codex_version(managed_codex_bin).await?)
            } else {
                None
            };
            let availability =
                if !settings.remote_control_enabled && backend.retires_when_idle().await? {
                    RestartAvailability::LegacyIdleRetiringDaemon
                } else {
                    let restart_guard = self.acquire_restart_guard_after_probe().await?;
                    if restart_guard.is_some() {
                        RestartAvailability::Available
                    } else {
                        RestartAvailability::ActiveLocalClient
                    }
                };
            match restart_decision(
                mode,
                info.as_ref(),
                managed_version.as_deref(),
                availability,
            ) {
                RestartDecision::NotReady => return Ok(RestartIfRunningOutcome::NotReady),
                RestartDecision::AlreadyCurrent => RestartIfRunningOutcome::AlreadyCurrent,
                RestartDecision::Deferred => return Ok(RestartIfRunningOutcome::Deferred),
                RestartDecision::Restart => {
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
            RestartIfRunningOutcome::NotRunning
        };

        if should_reexec_updater(updater_refresh_mode, outcome) {
            crate::update_loop::reexec_managed_updater(managed_codex_bin)?;
        }

        Ok(outcome)
    }

    async fn stop(&self) -> Result<LifecycleOutput> {
        let settings = match self.load_settings().await {
            Ok(settings) => settings,
            Err(err)
                if err
                    .chain()
                    .any(<dyn std::error::Error + 'static>::is::<serde_json::Error>) =>
            {
                DaemonSettings::default()
            }
            Err(err) => return Err(err),
        };
        if let Some(backend) = self.running_backend_instance(&settings).await? {
            backend.stop().await?;
            return Ok(self
                .output(
                    LifecycleStatus::Stopped,
                    Some(BackendKind::Pid),
                    /*pid*/ None,
                    /*app_server_version*/ None,
                )
                .await);
        }

        if client::probe(&self.socket_path).await.is_ok() {
            return Err(anyhow!(
                "app server is running but is not managed by codewith app-server daemon"
            ));
        }

        Ok(self
            .output(
                LifecycleStatus::NotRunning,
                /*backend*/ None,
                /*pid*/ None,
                /*app_server_version*/ None,
            )
            .await)
    }

    async fn version(&self) -> Result<LifecycleOutput> {
        let settings = self.load_settings().await?;
        let info = client::probe(&self.socket_path).await?;
        Ok(self
            .output(
                LifecycleStatus::Running,
                self.running_backend(&settings).await?,
                /*pid*/ None,
                Some(info.app_server_version),
            )
            .await)
    }

    async fn wait_until_ready(&self) -> Result<client::ProbeInfo> {
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            match client::probe(&self.socket_path).await {
                Ok(info) => return Ok(info),
                Err(err) if tokio::time::Instant::now() < deadline => {
                    let _ = err;
                    sleep(START_POLL_INTERVAL).await;
                }
                Err(err) => {
                    let context = self.app_server_not_ready_context().await;
                    return Err(err).context(context);
                }
            }
        }
    }

    async fn app_server_not_ready_context(&self) -> String {
        let mut context = format!(
            "app server did not become ready on {}",
            self.socket_path.display()
        );
        self.append_daemon_app_server_context(&mut context).await;
        backend::append_stderr_log_tail_context(&self.pid_file, &mut context).await;
        context
    }

    async fn append_daemon_app_server_context(&self, context: &mut String) {
        let app_server_codex_version = self
            .app_server_codex_version_best_effort()
            .await
            .unwrap_or_else(|| "unknown".to_string());
        context.push_str(&format!(
            "\n\nDaemon used app-server:\n  path: {}\n  version: {app_server_codex_version}",
            self.app_server_codex_bin.display()
        ));
    }

    async fn bootstrap(&self, options: BootstrapOptions) -> Result<BootstrapOutput> {
        let _operation_lock = self.acquire_operation_lock().await?;
        self.bootstrap_locked(options).await
    }

    async fn ensure_remote_control_started(&self) -> Result<RemoteControlStartOutput> {
        let _operation_lock = self.acquire_operation_lock().await?;
        let settings = self.load_settings().await?;
        if self.is_bootstrapped(&settings).await? {
            let _ = self
                .set_remote_control_locked(RemoteControlMode::Enabled)
                .await?;
            let output = self.start().await?;
            return Ok(RemoteControlStartOutput::Start(output));
        }

        let output = self
            .bootstrap_locked(BootstrapOptions {
                remote_control_enabled: true,
            })
            .await?;
        Ok(RemoteControlStartOutput::Bootstrap(output))
    }

    async fn ensure_remote_control_ready(&self) -> Result<RemoteControlReadyOutput> {
        let daemon = self.ensure_remote_control_started().await?;
        let remote_control =
            remote_control_client::enable_remote_control(&self.socket_path).await?;
        Ok(RemoteControlReadyOutput {
            daemon,
            remote_control,
        })
    }

    async fn set_remote_control(&self, mode: RemoteControlMode) -> Result<RemoteControlOutput> {
        let _operation_lock = self.acquire_operation_lock().await?;
        self.set_remote_control_locked(mode).await
    }

    async fn set_remote_control_locked(
        &self,
        mode: RemoteControlMode,
    ) -> Result<RemoteControlOutput> {
        let previous_settings = self.load_settings().await?;
        let mut settings = previous_settings.clone();
        let remote_control_enabled = mode.is_enabled();
        let backend = self.running_backend_instance(&previous_settings).await?;

        if backend.is_none() && client::probe(&self.socket_path).await.is_ok() {
            return Err(anyhow!(
                "app server is running but is not managed by codewith app-server daemon"
            ));
        }

        if settings.remote_control_enabled == remote_control_enabled {
            let info = if backend.is_some() {
                Some(self.wait_until_ready().await?)
            } else {
                None
            };
            return Ok(self.remote_control_output(
                already_remote_control_status(mode),
                backend.map(|_| BackendKind::Pid),
                remote_control_enabled,
                info.map(|info| info.app_server_version),
            ));
        }

        settings.remote_control_enabled = remote_control_enabled;
        settings.save(&self.settings_file).await?;

        let app_server_version = if let Some(backend) = backend {
            self.ensure_app_server_codex_bin()?;
            backend.stop().await?;
            let _ = self.start_app_server_backend(&settings).await?;
            Some(self.wait_until_ready().await?.app_server_version)
        } else {
            None
        };

        Ok(self.remote_control_output(
            remote_control_status(mode),
            app_server_version.as_ref().map(|_| BackendKind::Pid),
            remote_control_enabled,
            app_server_version,
        ))
    }

    async fn bootstrap_locked(&self, options: BootstrapOptions) -> Result<BootstrapOutput> {
        self.ensure_managed_codex_bin()?;

        let settings = DaemonSettings {
            remote_control_enabled: options.remote_control_enabled,
        };
        if client::probe(&self.socket_path).await.is_ok()
            && self.running_backend(&settings).await?.is_none()
        {
            return Err(anyhow!(
                "app server is running but is not managed by codewith app-server daemon"
            ));
        }
        settings.save(&self.settings_file).await?;

        if let Some(backend) = self.running_backend_instance(&settings).await? {
            backend.stop().await?;
        }

        let backend = backend::pid_backend(self.backend_paths(&settings));
        backend.start().await?;
        let updater = backend::pid_update_loop_backend(self.backend_paths(&settings));
        if updater.is_starting_or_running().await? {
            updater.stop().await?;
        }
        updater.start().await?;

        let info = self.wait_until_ready().await?;
        let managed_codex_version = self.managed_codex_version_best_effort().await;
        Ok(BootstrapOutput {
            status: BootstrapStatus::Bootstrapped,
            backend: BackendKind::Pid,
            auto_update_enabled: true,
            remote_control_enabled: settings.remote_control_enabled,
            managed_codex_path: self.managed_codex_bin.clone(),
            managed_codex_version,
            socket_path: self.socket_path.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            app_server_version: info.app_server_version,
        })
    }

    async fn running_backend(&self, settings: &DaemonSettings) -> Result<Option<BackendKind>> {
        Ok(self
            .running_backend_instance(settings)
            .await?
            .map(|_| BackendKind::Pid))
    }

    async fn running_backend_instance(
        &self,
        settings: &DaemonSettings,
    ) -> Result<Option<backend::PidBackend>> {
        let backend = backend::pid_backend(self.app_server_backend_paths(settings));
        if backend.is_starting_or_running().await? {
            return Ok(Some(backend));
        }
        Ok(None)
    }

    async fn start_app_server_backend(&self, settings: &DaemonSettings) -> Result<Option<u32>> {
        self.start_backend_with_bin(settings, &self.app_server_codex_bin)
            .await
    }

    async fn start_backend_with_bin(
        &self,
        settings: &DaemonSettings,
        codex_bin: &Path,
    ) -> Result<Option<u32>> {
        let backend = backend::pid_backend(self.backend_paths_with_bin(settings, codex_bin));
        backend.start().await
    }

    async fn is_bootstrapped(&self, settings: &DaemonSettings) -> Result<bool> {
        let updater = backend::pid_update_loop_backend(self.backend_paths(settings));
        updater.is_starting_or_running().await
    }

    fn ensure_managed_codex_bin(&self) -> Result<()> {
        if self.managed_codex_bin.is_file() {
            return Ok(());
        }

        let managed_codex_path = self.managed_codex_bin.display();
        Err(anyhow!(
            "managed standalone Codewith install not found at {managed_codex_path}\n\n\
             This command requires the standalone install managed by the Codewith installer, because \
             the daemon starts and updates app-server from that fixed path.\n\n\
             Install it with:\n  curl -fsSL https://raw.githubusercontent.com/hasna/codewith/main/scripts/install/install.sh | sh\n\n\
             Then rerun the command you just tried."
        ))
    }

    fn ensure_app_server_codex_bin(&self) -> Result<()> {
        if self.app_server_codex_bin.is_file() {
            return Ok(());
        }
        if self.app_server_codex_bin == self.managed_codex_bin {
            return self.ensure_managed_codex_bin();
        }

        let app_server_codex_path = self.app_server_codex_bin.display();
        Err(anyhow!(
            "Codewith binary not found at {app_server_codex_path}"
        ))
    }

    #[cfg(unix)]
    async fn managed_codex_version_best_effort(&self) -> Option<String> {
        managed_codex_version(&self.managed_codex_bin).await.ok()
    }

    #[cfg(not(unix))]
    async fn managed_codex_version_best_effort(&self) -> Option<String> {
        None
    }

    #[cfg(unix)]
    async fn app_server_codex_version_best_effort(&self) -> Option<String> {
        managed_codex_version(&self.app_server_codex_bin).await.ok()
    }

    #[cfg(not(unix))]
    async fn app_server_codex_version_best_effort(&self) -> Option<String> {
        None
    }

    fn app_server_backend_paths(&self, settings: &DaemonSettings) -> BackendPaths {
        self.backend_paths_with_bin(settings, &self.app_server_codex_bin)
    }

    fn backend_paths(&self, settings: &DaemonSettings) -> BackendPaths {
        self.backend_paths_with_bin(settings, &self.managed_codex_bin)
    }

    fn backend_paths_with_bin(&self, settings: &DaemonSettings, codex_bin: &Path) -> BackendPaths {
        BackendPaths {
            codex_bin: codex_bin.to_path_buf(),
            pid_file: self.pid_file.clone(),
            update_pid_file: self.update_pid_file.clone(),
            remote_control_enabled: settings.remote_control_enabled,
        }
    }

    async fn load_settings(&self) -> Result<DaemonSettings> {
        DaemonSettings::load(&self.settings_file).await
    }

    async fn acquire_operation_lock(&self) -> Result<tokio::fs::File> {
        let operation_lock = self.open_operation_lock_file().await?;
        let deadline = tokio::time::Instant::now() + OPERATION_LOCK_TIMEOUT;
        while !try_lock_file(&operation_lock)? {
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for daemon operation lock {}",
                    self.operation_lock_file.display()
                ));
            }
            sleep(START_POLL_INTERVAL).await;
        }
        Ok(operation_lock)
    }

    async fn acquire_shared_operation_lock(&self) -> Result<tokio::fs::File> {
        let operation_lock = self.open_operation_lock_file().await?;
        let deadline = tokio::time::Instant::now() + OPERATION_LOCK_TIMEOUT;
        while !try_lock_file_shared(&operation_lock)? {
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for daemon operation lock {}",
                    self.operation_lock_file.display()
                ));
            }
            sleep(START_POLL_INTERVAL).await;
        }
        Ok(operation_lock)
    }

    async fn open_operation_lock_file(&self) -> Result<tokio::fs::File> {
        self.open_lock_file(&self.operation_lock_file, "daemon operation")
            .await
    }

    async fn acquire_client_lease(&self) -> Result<LocalDaemonLease> {
        let file = self.open_client_lease_file().await?;
        if !try_lock_file_shared(&file)? {
            return Err(anyhow!(
                "app-server daemon client lease is temporarily unavailable"
            ));
        }
        Ok(LocalDaemonLease {
            _client_file: file,
            _legacy_operation_file: None,
        })
    }

    async fn acquire_client_lease_with_migration_guard(
        &self,
        operation_lock: tokio::fs::File,
    ) -> Result<LocalDaemonLease> {
        let mut lease = self.acquire_client_lease().await?;
        let capability_file = self.open_updater_lease_capability_file().await?;
        if try_lock_file(&capability_file)? {
            lease._legacy_operation_file = Some(operation_lock);
        }
        Ok(lease)
    }

    pub(crate) async fn acquire_updater_lease_capability(&self) -> Result<tokio::fs::File> {
        let file = self.open_updater_lease_capability_file().await?;
        let deadline = tokio::time::Instant::now() + OPERATION_LOCK_TIMEOUT;
        while !try_lock_file(&file)? {
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for updater lease capability {}",
                    self.updater_lease_capability_file.display()
                ));
            }
            sleep(START_POLL_INTERVAL).await;
        }
        Ok(file)
    }

    #[cfg(test)]
    async fn try_acquire_restart_guard(&self) -> Result<Option<tokio::fs::File>> {
        let file = self.open_client_lease_file().await?;
        if try_lock_file(&file)? {
            Ok(Some(file))
        } else {
            Ok(None)
        }
    }

    async fn acquire_restart_guard_after_probe(&self) -> Result<Option<tokio::fs::File>> {
        let file = self.open_client_lease_file().await?;
        let deadline = tokio::time::Instant::now() + CLIENT_LEASE_SETTLE_TIMEOUT;
        while !try_lock_file(&file)? {
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(START_POLL_INTERVAL).await;
        }
        Ok(Some(file))
    }

    async fn open_client_lease_file(&self) -> Result<tokio::fs::File> {
        self.open_lock_file(&self.client_lease_file, "daemon client lease")
            .await
    }

    async fn open_updater_lease_capability_file(&self) -> Result<tokio::fs::File> {
        self.open_lock_file(
            &self.updater_lease_capability_file,
            "updater lease capability",
        )
        .await
    }

    async fn open_lock_file(&self, path: &Path, label: &str) -> Result<tokio::fs::File> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "failed to create daemon state directory {}",
                    parent.display()
                )
            })?;
        }
        tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {label} {}", path.display()))
    }

    async fn output(
        &self,
        status: LifecycleStatus,
        backend: Option<BackendKind>,
        pid: Option<u32>,
        app_server_version: Option<String>,
    ) -> LifecycleOutput {
        let managed_codex_version = self.managed_codex_version_best_effort().await;
        LifecycleOutput {
            status,
            backend,
            pid,
            managed_codex_path: self.managed_codex_bin.clone(),
            managed_codex_version,
            socket_path: self.socket_path.clone(),
            cli_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            app_server_version,
        }
    }

    fn remote_control_output(
        &self,
        status: RemoteControlStatus,
        backend: Option<BackendKind>,
        remote_control_enabled: bool,
        app_server_version: Option<String>,
    ) -> RemoteControlOutput {
        RemoteControlOutput {
            status,
            backend,
            remote_control_enabled,
            socket_path: self.socket_path.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            app_server_version,
        }
    }
}

fn app_server_daemon_state_dir(codex_home: &Path) -> Result<PathBuf> {
    Ok(app_server_daemon_state_dir_for_auth_profile(
        codex_home,
        selected_auth_profile_from_env()?.as_deref(),
    ))
}

fn app_server_daemon_state_dir_for_auth_profile(
    codex_home: &Path,
    auth_profile: Option<&str>,
) -> PathBuf {
    let state_dir = codex_home.join(STATE_DIR_NAME);
    let Some(auth_profile) = auth_profile else {
        return state_dir;
    };
    state_dir
        .join(AUTH_PROFILE_STATE_DIR_NAME)
        .join(auth_profile)
}

fn remote_control_status(mode: RemoteControlMode) -> RemoteControlStatus {
    match mode {
        RemoteControlMode::Enabled => RemoteControlStatus::Enabled,
        RemoteControlMode::Disabled => RemoteControlStatus::Disabled,
    }
}

fn already_remote_control_status(mode: RemoteControlMode) -> RemoteControlStatus {
    match mode {
        RemoteControlMode::Enabled => RemoteControlStatus::AlreadyEnabled,
        RemoteControlMode::Disabled => RemoteControlStatus::AlreadyDisabled,
    }
}

#[cfg(unix)]
fn restart_decision(
    mode: RestartMode,
    info: Option<&client::ProbeInfo>,
    managed_version: Option<&str>,
    availability: RestartAvailability,
) -> RestartDecision {
    let decision = match (mode, info, managed_version) {
        (RestartMode::IfVersionChanged, None, _) => RestartDecision::NotReady,
        (RestartMode::IfVersionChanged, Some(info), Some(managed_version))
            if info.app_server_version == managed_version =>
        {
            RestartDecision::AlreadyCurrent
        }
        _ => RestartDecision::Restart,
    };
    match (decision, availability) {
        (
            RestartDecision::Restart,
            RestartAvailability::ActiveLocalClient | RestartAvailability::LegacyIdleRetiringDaemon,
        ) => RestartDecision::Deferred,
        (decision, _) => decision,
    }
}

#[cfg(unix)]
fn should_reexec_updater(
    updater_refresh_mode: UpdaterRefreshMode,
    outcome: RestartIfRunningOutcome,
) -> bool {
    updater_refresh_mode == UpdaterRefreshMode::ReexecIfManagedBinaryChanged
        && matches!(
            outcome,
            RestartIfRunningOutcome::NotRunning | RestartIfRunningOutcome::Restarted
        )
}

#[cfg(unix)]
fn try_lock_file(file: &tokio::fs::File) -> Result<bool> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(err).context("failed to lock daemon operation")
}

#[cfg(unix)]
fn try_lock_file_shared(file: &tokio::fs::File) -> Result<bool> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(err).context("failed to lock daemon client lease")
}

#[cfg(not(unix))]
fn try_lock_file_shared(_file: &tokio::fs::File) -> Result<bool> {
    Ok(true)
}

#[cfg(not(unix))]
fn try_lock_file(_file: &tokio::fs::File) -> Result<bool> {
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::BackendKind;
    use super::BootstrapOutput;
    use super::BootstrapStatus;
    use super::Daemon;
    use super::LifecycleOutput;
    use super::LifecycleStatus;
    use super::RemoteControlStartOutput;
    use super::RemoteControlStatus;
    use super::RestartAvailability;
    use super::RestartDecision;
    use super::RestartIfRunningOutcome;
    use super::RestartMode;
    use super::UpdaterRefreshMode;
    use super::app_server_daemon_state_dir_for_auth_profile;
    use super::restart_decision;
    use super::should_reexec_updater;
    use crate::client::ProbeInfo;
    use crate::settings::DaemonSettings;

    #[test]
    fn remote_control_status_uses_camel_case_json() {
        assert_eq!(
            serde_json::to_string(&RemoteControlStatus::AlreadyEnabled).expect("serialize"),
            "\"alreadyEnabled\""
        );
    }

    #[test]
    fn daemon_state_dir_can_be_scoped_to_auth_profile() {
        let temp_dir = TempDir::new().expect("temp dir");

        assert_eq!(
            app_server_daemon_state_dir_for_auth_profile(
                temp_dir.path(),
                /*auth_profile*/ None
            ),
            temp_dir.path().join("app-server-daemon")
        );
        assert_eq!(
            app_server_daemon_state_dir_for_auth_profile(temp_dir.path(), Some("work")),
            temp_dir
                .path()
                .join("app-server-daemon")
                .join("auth_profiles")
                .join("work")
        );
    }

    #[test]
    fn updater_reexecs_after_restart_or_when_daemon_is_idle() {
        assert_eq!(
            [
                RestartIfRunningOutcome::Busy,
                RestartIfRunningOutcome::Deferred,
                RestartIfRunningOutcome::NotReady,
                RestartIfRunningOutcome::AlreadyCurrent,
                RestartIfRunningOutcome::NotRunning,
                RestartIfRunningOutcome::Restarted,
            ]
            .map(|outcome| {
                should_reexec_updater(UpdaterRefreshMode::ReexecIfManagedBinaryChanged, outcome)
            }),
            [false, false, false, false, true, true]
        );
    }

    #[test]
    fn unchanged_updater_never_reexecs() {
        assert_eq!(
            [
                RestartIfRunningOutcome::Busy,
                RestartIfRunningOutcome::Deferred,
                RestartIfRunningOutcome::NotReady,
                RestartIfRunningOutcome::AlreadyCurrent,
                RestartIfRunningOutcome::NotRunning,
                RestartIfRunningOutcome::Restarted,
            ]
            .map(|outcome| should_reexec_updater(UpdaterRefreshMode::None, outcome)),
            [false, false, false, false, false, false]
        );
    }

    #[test]
    fn restart_decision_preserves_forced_refreshes() {
        let current_info = ProbeInfo {
            app_server_version: "0.1.0".to_string(),
        };

        assert_eq!(
            [
                restart_decision(
                    RestartMode::IfVersionChanged,
                    Some(&current_info),
                    Some("0.1.0"),
                    RestartAvailability::Available,
                ),
                restart_decision(
                    RestartMode::IfVersionChanged,
                    /*info*/ None,
                    /*managed_version*/ None,
                    RestartAvailability::Available,
                ),
                restart_decision(
                    RestartMode::Always,
                    Some(&current_info),
                    Some("0.1.0"),
                    RestartAvailability::Available,
                ),
                restart_decision(
                    RestartMode::Always,
                    /*info*/ None,
                    /*managed_version*/ None,
                    RestartAvailability::Available,
                ),
            ],
            [
                RestartDecision::AlreadyCurrent,
                RestartDecision::NotReady,
                RestartDecision::Restart,
                RestartDecision::Restart,
            ]
        );
    }

    #[test]
    fn updater_defers_requested_restart_until_local_clients_leave() {
        let current_info = ProbeInfo {
            app_server_version: "0.1.0".to_string(),
        };

        assert_eq!(
            [
                restart_decision(
                    RestartMode::IfVersionChanged,
                    Some(&current_info),
                    Some("0.2.0"),
                    RestartAvailability::ActiveLocalClient,
                ),
                restart_decision(
                    RestartMode::Always,
                    Some(&current_info),
                    Some("0.1.0"),
                    RestartAvailability::ActiveLocalClient,
                ),
                restart_decision(
                    RestartMode::Always,
                    Some(&current_info),
                    Some("0.1.0"),
                    RestartAvailability::LegacyIdleRetiringDaemon,
                ),
                restart_decision(
                    RestartMode::Always,
                    Some(&current_info),
                    Some("0.1.0"),
                    RestartAvailability::Available,
                ),
            ],
            [
                RestartDecision::Deferred,
                RestartDecision::Deferred,
                RestartDecision::Deferred,
                RestartDecision::Restart,
            ],
        );
    }

    #[tokio::test]
    async fn clients_hold_legacy_operation_guard_until_lease_aware_updater_runs() {
        let codex_home = TempDir::new().expect("temp dir");
        let daemon = Daemon::from_codex_home_and_app_server_codex_bin(
            codex_home.path(),
            codex_home.path().join("codewith"),
        )
        .expect("daemon paths");

        let operation_lock = daemon
            .acquire_shared_operation_lock()
            .await
            .expect("operation lock");
        let legacy_lease = daemon
            .acquire_client_lease_with_migration_guard(operation_lock)
            .await
            .expect("legacy lease");
        let competing_operation = daemon
            .open_operation_lock_file()
            .await
            .expect("competing operation");
        assert!(!super::try_lock_file(&competing_operation).expect("operation check"));
        let second_operation_lock = daemon
            .acquire_shared_operation_lock()
            .await
            .expect("second shared operation lock");
        let second_legacy_lease = daemon
            .acquire_client_lease_with_migration_guard(second_operation_lock)
            .await
            .expect("second legacy lease");
        drop(legacy_lease);
        assert!(!super::try_lock_file(&competing_operation).expect("operation check"));
        drop(second_legacy_lease);
        assert!(super::try_lock_file(&competing_operation).expect("operation check"));
        drop(competing_operation);

        let _lease_capability = daemon
            .acquire_updater_lease_capability()
            .await
            .expect("lease-aware updater");
        let operation_lock = daemon
            .acquire_shared_operation_lock()
            .await
            .expect("operation lock");
        let current_lease = daemon
            .acquire_client_lease_with_migration_guard(operation_lock)
            .await
            .expect("current lease");
        let manual_operation = daemon
            .open_operation_lock_file()
            .await
            .expect("manual operation");
        assert!(super::try_lock_file(&manual_operation).expect("manual operation check"));
        assert!(
            daemon
                .try_acquire_restart_guard()
                .await
                .expect("restart guard")
                .is_none()
        );
        drop(current_lease);
    }

    #[tokio::test]
    async fn local_client_leases_block_automatic_restart_until_all_are_released() {
        let codex_home = TempDir::new().expect("temp dir");
        let daemon = Daemon::from_codex_home_and_app_server_codex_bin(
            codex_home.path(),
            codex_home.path().join("codewith"),
        )
        .expect("daemon paths");

        let first_lease = daemon.acquire_client_lease().await.expect("first lease");
        let second_lease = daemon.acquire_client_lease().await.expect("second lease");
        assert!(
            daemon
                .try_acquire_restart_guard()
                .await
                .expect("restart guard")
                .is_none()
        );

        drop(first_lease);
        assert!(
            daemon
                .try_acquire_restart_guard()
                .await
                .expect("restart guard")
                .is_none()
        );

        drop(second_lease);
        assert!(
            daemon
                .try_acquire_restart_guard()
                .await
                .expect("restart guard")
                .is_some()
        );
    }

    #[tokio::test]
    async fn updater_absorbs_probe_lease_close_before_restart_decision() {
        let codex_home = TempDir::new().expect("temp dir");
        let daemon = Daemon::from_codex_home_and_app_server_codex_bin(
            codex_home.path(),
            codex_home.path().join("codewith"),
        )
        .expect("daemon paths");
        let probe_lease = daemon.acquire_client_lease().await.expect("probe lease");
        let release_probe = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(75)).await;
            drop(probe_lease);
        });

        assert!(
            daemon
                .acquire_restart_guard_after_probe()
                .await
                .expect("restart guard")
                .is_some()
        );
        release_probe.await.expect("release probe");
    }

    #[test]
    fn local_daemon_start_settings_disable_remote_control() {
        assert_eq!(
            Daemon::local_start_settings(),
            DaemonSettings {
                remote_control_enabled: false,
            }
        );
    }

    #[test]
    fn remote_control_start_output_serializes_inner_output_without_tag() {
        let lifecycle_output = LifecycleOutput {
            status: LifecycleStatus::AlreadyRunning,
            backend: Some(BackendKind::Pid),
            pid: None,
            managed_codex_path: "codewith".into(),
            managed_codex_version: Some("1.2.3".to_string()),
            socket_path: "codex.sock".into(),
            cli_version: Some("1.2.3".to_string()),
            app_server_version: Some("1.2.4".to_string()),
        };
        let output = RemoteControlStartOutput::Start(lifecycle_output.clone());

        assert_eq!(
            serde_json::to_value(&lifecycle_output).expect("serialize"),
            serde_json::json!({
                "status": "alreadyRunning",
                "backend": "pid",
                "managedCodexPath": "codewith",
                "managedCodexVersion": "1.2.3",
                "socketPath": "codex.sock",
                "cliVersion": "1.2.3",
                "appServerVersion": "1.2.4",
            })
        );
        assert_eq!(
            serde_json::to_value(output).expect("serialize"),
            serde_json::to_value(lifecycle_output).expect("serialize")
        );

        let bootstrap_output = BootstrapOutput {
            status: BootstrapStatus::Bootstrapped,
            backend: BackendKind::Pid,
            auto_update_enabled: true,
            remote_control_enabled: true,
            managed_codex_path: "codewith".into(),
            managed_codex_version: Some("1.2.3".to_string()),
            socket_path: "codex.sock".into(),
            cli_version: "1.2.3".to_string(),
            app_server_version: "1.2.4".to_string(),
        };
        let output = RemoteControlStartOutput::Bootstrap(bootstrap_output.clone());

        assert_eq!(
            serde_json::to_value(&bootstrap_output).expect("serialize"),
            serde_json::json!({
                "status": "bootstrapped",
                "backend": "pid",
                "autoUpdateEnabled": true,
                "remoteControlEnabled": true,
                "managedCodexPath": "codewith",
                "managedCodexVersion": "1.2.3",
                "socketPath": "codex.sock",
                "cliVersion": "1.2.3",
                "appServerVersion": "1.2.4",
            })
        );
        assert_eq!(
            serde_json::to_value(output).expect("serialize"),
            serde_json::to_value(bootstrap_output).expect("serialize")
        );
    }

    #[tokio::test]
    async fn not_ready_context_reports_daemon_app_server_before_stderr() {
        let temp_dir = TempDir::new().expect("temp dir");
        let daemon = Daemon {
            socket_path: temp_dir.path().join("app-server-control.sock"),
            pid_file: temp_dir.path().join("app-server.pid"),
            update_pid_file: temp_dir.path().join("app-server-updater.pid"),
            operation_lock_file: temp_dir.path().join("daemon.lock"),
            client_lease_file: temp_dir.path().join("client.lock"),
            updater_lease_capability_file: temp_dir.path().join("lease-aware-updater.lock"),
            settings_file: temp_dir.path().join("settings.json"),
            managed_codex_bin: temp_dir.path().join("missing-codewith"),
            app_server_codex_bin: temp_dir.path().join("runtime-codewith"),
        };
        let stderr_log = daemon.pid_file.with_extension("stderr.log");
        tokio::fs::write(&stderr_log, "unexpected argument")
            .await
            .expect("write stderr log");

        assert_eq!(
            daemon.app_server_not_ready_context().await,
            format!(
                "app server did not become ready on {}\n\n\
                 Daemon used app-server:\n  path: {}\n  version: unknown\n\n\
                 Managed app-server stderr ({}):\n  unexpected argument",
                daemon.socket_path.display(),
                daemon.app_server_codex_bin.display(),
                stderr_log.display()
            )
        );
    }

    #[tokio::test]
    async fn stop_recovers_from_corrupt_settings_when_not_running() {
        let temp_dir = TempDir::new().expect("temp dir");
        let daemon = Daemon {
            socket_path: temp_dir.path().join("app-server-control.sock"),
            pid_file: temp_dir.path().join("app-server.pid"),
            update_pid_file: temp_dir.path().join("app-server-updater.pid"),
            operation_lock_file: temp_dir.path().join("daemon.lock"),
            client_lease_file: temp_dir.path().join("client.lock"),
            updater_lease_capability_file: temp_dir.path().join("lease-aware-updater.lock"),
            settings_file: temp_dir.path().join("settings.json"),
            managed_codex_bin: temp_dir.path().join("missing-codewith"),
            app_server_codex_bin: temp_dir.path().join("runtime-codewith"),
        };
        tokio::fs::write(&daemon.settings_file, b"{")
            .await
            .expect("write corrupt settings");

        let output = daemon.stop().await.expect("stop");

        assert_eq!(output.status, LifecycleStatus::NotRunning);
        assert_eq!(output.backend, None);
    }

    #[test]
    fn non_managed_app_server_binary_uses_plain_missing_binary_error() {
        let temp_dir = TempDir::new().expect("temp dir");
        let daemon = Daemon {
            socket_path: temp_dir.path().join("app-server-control.sock"),
            pid_file: temp_dir.path().join("app-server.pid"),
            update_pid_file: temp_dir.path().join("app-server-updater.pid"),
            operation_lock_file: temp_dir.path().join("daemon.lock"),
            client_lease_file: temp_dir.path().join("client.lock"),
            updater_lease_capability_file: temp_dir.path().join("lease-aware-updater.lock"),
            settings_file: temp_dir.path().join("settings.json"),
            managed_codex_bin: temp_dir.path().join("managed-codewith"),
            app_server_codex_bin: temp_dir.path().join("runtime-codewith"),
        };

        let err = daemon
            .ensure_app_server_codex_bin()
            .expect_err("runtime binary should be missing");

        assert_eq!(
            err.to_string(),
            format!(
                "Codewith binary not found at {}",
                daemon.app_server_codex_bin.display()
            )
        );
    }
}
