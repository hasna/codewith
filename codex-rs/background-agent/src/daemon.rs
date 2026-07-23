use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs;
#[cfg(unix)]
use tokio::time::Instant;
#[cfg(unix)]
use tokio::time::sleep;

use crate::process_lifecycle::WorkerProcessCommand;
use crate::process_lifecycle::WorkerProcessController;
use crate::process_lifecycle::WorkerProcessHandle;
use crate::process_lifecycle::WorkerProcessLogTail;
use crate::process_lifecycle::WorkerProcessStatus;
use crate::process_lifecycle::WorkerProcessStopReport;
use crate::BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION;
use crate::BACKGROUND_AGENT_DAEMON_INCOMPATIBLE;
use crate::BACKGROUND_AGENT_DAEMON_PROTOCOL_VERSION;

const DAEMON_STATE_DIR_NAME: &str = "background-agent-daemon";
const DAEMON_PID_FILE_NAME: &str = "daemon.json";
const DAEMON_LOCK_FILE_NAME: &str = "daemon.lock";
const DAEMON_STDERR_FILE_NAME: &str = "daemon.stderr.log";
#[cfg(unix)]
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
#[cfg(unix)]
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const DAEMON_STOP_GRACE_PERIOD: Duration = Duration::from_secs(35);
const DAEMON_HARD_KILL_TIMEOUT: Duration = Duration::from_secs(15);
const DAEMON_CAPABILITIES: &[&str] = &[
    "durable-admission",
    "exact-auth-profile",
    "generation-fencing",
    "lifecycle-receipts",
];

pub fn background_agent_daemon_state_dir(codex_home: &Path) -> PathBuf {
    codex_home.join(DAEMON_STATE_DIR_NAME)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundAgentDaemonPaths {
    pub codex_bin: PathBuf,
    pub state_dir: PathBuf,
}

impl BackgroundAgentDaemonPaths {
    pub fn new(codex_bin: impl Into<PathBuf>, state_dir: impl Into<PathBuf>) -> Self {
        Self {
            codex_bin: codex_bin.into(),
            state_dir: state_dir.into(),
        }
    }

    fn pid_file(&self) -> PathBuf {
        self.state_dir.join(DAEMON_PID_FILE_NAME)
    }

    fn lock_file(&self) -> PathBuf {
        self.state_dir.join(DAEMON_LOCK_FILE_NAME)
    }

    fn stderr_log_path(&self) -> PathBuf {
        self.state_dir.join(DAEMON_STDERR_FILE_NAME)
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundAgentDaemon {
    paths: BackgroundAgentDaemonPaths,
    controller: WorkerProcessController,
}

impl BackgroundAgentDaemon {
    pub fn new(paths: BackgroundAgentDaemonPaths) -> Self {
        Self {
            paths,
            controller: WorkerProcessController::with_timeouts(
                DAEMON_STOP_GRACE_PERIOD,
                DAEMON_HARD_KILL_TIMEOUT,
            ),
        }
    }

    pub fn with_controller(
        paths: BackgroundAgentDaemonPaths,
        controller: WorkerProcessController,
    ) -> Self {
        Self { paths, controller }
    }

    pub async fn start(&self) -> Result<BackgroundAgentDaemonOutput> {
        ensure_supported_platform()?;
        fs::create_dir_all(&self.paths.state_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create background-agent daemon state directory {}",
                    self.paths.state_dir.display()
                )
            })?;
        let _lock = acquire_lock(&self.paths.lock_file()).await?;
        if let Some(record) = read_pid_record(&self.paths.pid_file()).await? {
            match self.controller.status(&record.handle).await? {
                WorkerProcessStatus::Running => {
                    ensure_daemon_record_compatible(&record)?;
                    return self
                        .output(
                            BackgroundAgentDaemonStatus::AlreadyRunning,
                            Some(record),
                            /*stop_report*/ None,
                        )
                        .await;
                }
                WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord => {
                    remove_pid_file(&self.paths.pid_file()).await?;
                }
            }
        }

        let command =
            WorkerProcessCommand::new(&self.paths.codex_bin, self.paths.stderr_log_path()).args([
                "app-server",
                "--listen",
                "off",
                "--background-agent-host",
            ]);
        let handle = self.controller.spawn(command).await?;
        let record = BackgroundAgentDaemonPidRecord {
            handle,
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: BACKGROUND_AGENT_DAEMON_PROTOCOL_VERSION,
            admission_schema_version: BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION.to_string(),
            capabilities: daemon_capabilities(),
        };
        if let Err(publication_error) = write_pid_record(&self.paths.pid_file(), &record).await {
            return match self.controller.stop(&record.handle).await {
                Ok(_) => Err(publication_error),
                Err(cleanup_error) => Err(publication_error.context(format!(
                    "failed to stop background-agent worker after pid record publication failed: {cleanup_error:#}"
                ))),
            };
        }
        self.output(
            BackgroundAgentDaemonStatus::Started,
            Some(record),
            /*stop_report*/ None,
        )
        .await
    }

    pub async fn status(&self) -> Result<BackgroundAgentDaemonOutput> {
        ensure_supported_platform()?;
        let Some(record) = read_pid_record(&self.paths.pid_file()).await? else {
            return self
                .output(
                    BackgroundAgentDaemonStatus::NotRunning,
                    /*record*/ None,
                    /*stop_report*/ None,
                )
                .await;
        };
        let status = match self.controller.status(&record.handle).await? {
            WorkerProcessStatus::Running => BackgroundAgentDaemonStatus::Running,
            WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord => {
                BackgroundAgentDaemonStatus::StalePidRecord
            }
        };
        self.output(status, Some(record), /*stop_report*/ None)
            .await
    }

    pub async fn stop(&self) -> Result<BackgroundAgentDaemonOutput> {
        ensure_supported_platform()?;
        fs::create_dir_all(&self.paths.state_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create background-agent daemon state directory {}",
                    self.paths.state_dir.display()
                )
            })?;
        let _lock = acquire_lock(&self.paths.lock_file()).await?;
        let Some(record) = read_pid_record(&self.paths.pid_file()).await? else {
            return self
                .output(
                    BackgroundAgentDaemonStatus::NotRunning,
                    /*record*/ None,
                    /*stop_report*/ None,
                )
                .await;
        };
        let status = self.controller.status(&record.handle).await?;
        let stop_report = match status {
            WorkerProcessStatus::Running => Some(self.controller.stop(&record.handle).await?),
            WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord => None,
        };
        remove_pid_file(&self.paths.pid_file()).await?;
        let output_status = match status {
            WorkerProcessStatus::Running => BackgroundAgentDaemonStatus::Stopped,
            WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord => {
                BackgroundAgentDaemonStatus::StalePidRemoved
            }
        };
        self.output(output_status, Some(record), stop_report).await
    }

    async fn output(
        &self,
        status: BackgroundAgentDaemonStatus,
        record: Option<BackgroundAgentDaemonPidRecord>,
        stop_report: Option<WorkerProcessStopReport>,
    ) -> Result<BackgroundAgentDaemonOutput> {
        let stderr_tail = match &record {
            Some(record) => self
                .controller
                .stderr_tail(&record.handle, /*byte_limit*/ None)
                .await
                .unwrap_or(None),
            None => None,
        };
        let handle = record.as_ref().map(|record| &record.handle);
        let version = record.as_ref().map(|record| record.version.clone());
        let protocol_version = record.as_ref().map(|record| record.protocol_version);
        let admission_schema_version = record
            .as_ref()
            .map(|record| record.admission_schema_version.clone());
        let capabilities = record
            .as_ref()
            .map(|record| record.capabilities.clone())
            .unwrap_or_default();
        Ok(BackgroundAgentDaemonOutput {
            status,
            pid: handle.map(|handle| handle.pid),
            pgid: handle.and_then(|handle| handle.pgid),
            version,
            protocol_version,
            admission_schema_version,
            capabilities,
            state_dir: self.paths.state_dir.clone(),
            pid_file: self.paths.pid_file(),
            stderr_log_path: self.paths.stderr_log_path(),
            stderr_tail,
            stop_report,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundAgentDaemonStatus {
    Started,
    AlreadyRunning,
    Running,
    NotRunning,
    Stopped,
    StalePidRecord,
    StalePidRemoved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundAgentDaemonOutput {
    pub status: BackgroundAgentDaemonStatus,
    pub pid: Option<u32>,
    pub pgid: Option<u32>,
    pub version: Option<String>,
    pub protocol_version: Option<u32>,
    pub admission_schema_version: Option<String>,
    pub capabilities: Vec<String>,
    pub state_dir: PathBuf,
    pub pid_file: PathBuf,
    pub stderr_log_path: PathBuf,
    pub stderr_tail: Option<WorkerProcessLogTail>,
    pub stop_report: Option<WorkerProcessStopReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundAgentDaemonPidRecord {
    handle: WorkerProcessHandle,
    version: String,
    #[serde(default)]
    protocol_version: u32,
    #[serde(default)]
    admission_schema_version: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

fn daemon_capabilities() -> Vec<String> {
    DAEMON_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect()
}

fn ensure_daemon_record_compatible(record: &BackgroundAgentDaemonPidRecord) -> Result<()> {
    let compatible = record.version == env!("CARGO_PKG_VERSION")
        && record.protocol_version == BACKGROUND_AGENT_DAEMON_PROTOCOL_VERSION
        && record.admission_schema_version == BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION
        && DAEMON_CAPABILITIES
            .iter()
            .all(|capability| record.capabilities.iter().any(|value| value == capability));
    if !compatible {
        bail!(
            "{BACKGROUND_AGENT_DAEMON_INCOMPATIBLE}: running daemon package/protocol/schema or capabilities do not match this client"
        );
    }
    Ok(())
}

async fn read_pid_record(path: &Path) -> Result<Option<BackgroundAgentDaemonPidRecord>> {
    let contents = match fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read daemon pid file {}", path.display()));
        }
    };
    if contents.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&contents)
        .with_context(|| format!("invalid daemon pid file {}", path.display()))
        .map(Some)
}

async fn write_pid_record(path: &Path, record: &BackgroundAgentDaemonPidRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!("failed to create daemon pid directory {}", parent.display())
        })?;
    }
    let temp_path = path.with_extension("json.tmp");
    let contents = serde_json::to_vec(record).context("failed to serialize daemon pid record")?;
    fs::write(&temp_path, contents).await.with_context(|| {
        format!(
            "failed to write daemon pid temp file {}",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, path)
        .await
        .with_context(|| format!("failed to publish daemon pid file {}", path.display()))
}

async fn remove_pid_file(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove daemon pid file {}", path.display()))
        }
    }
}

#[cfg(unix)]
async fn acquire_lock(path: &Path) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "failed to create daemon lock directory {}",
                parent.display()
            )
        })?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("failed to open daemon lock file {}", path.display()))?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    while !try_lock_file(&file)? {
        if Instant::now() >= deadline {
            bail!("timed out waiting for daemon lock {}", path.display());
        }
        sleep(LOCK_POLL_INTERVAL).await;
    }
    Ok(file)
}

#[cfg(unix)]
fn try_lock_file(file: &fs::File) -> Result<bool> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(err).context("failed to lock daemon pid file")
}

#[cfg(not(unix))]
async fn acquire_lock(_path: &Path) -> Result<fs::File> {
    bail!(
        "background-agent daemon lifecycle is unsupported on this platform until Job Object cleanup is implemented"
    )
}

#[cfg(unix)]
pub fn ensure_supported_platform() -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
pub fn ensure_supported_platform() -> Result<()> {
    bail!(
        "background-agent daemon lifecycle is unsupported on this platform until Job Object cleanup is implemented"
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn daemon_status_reports_not_running_without_pid_file() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let daemon = BackgroundAgentDaemon::new(BackgroundAgentDaemonPaths::new(
            "/bin/true",
            temp_dir.path(),
        ));

        let output = daemon.status().await?;

        assert_eq!(output.status, BackgroundAgentDaemonStatus::NotRunning);
        assert_eq!(output.pid, None);
        Ok(())
    }

    #[test]
    fn daemon_uses_extended_stop_grace_for_worker_cleanup() {
        let temp_dir = TempDir::new().expect("temp dir");
        let daemon = BackgroundAgentDaemon::new(BackgroundAgentDaemonPaths::new(
            "/bin/true",
            temp_dir.path(),
        ));

        assert_eq!(
            daemon.controller.stop_grace_period,
            DAEMON_STOP_GRACE_PERIOD
        );
        assert_eq!(
            daemon.controller.hard_kill_timeout,
            DAEMON_HARD_KILL_TIMEOUT
        );
        assert!(
            daemon.controller.stop_grace_period
                > WorkerProcessController::default().stop_grace_period
        );
    }

    #[tokio::test]
    async fn daemon_start_reuses_running_pid_record() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(50),
            Duration::from_secs(5),
        );
        let existing = controller
            .spawn(
                WorkerProcessCommand::new("/bin/sh", temp_dir.path().join("existing.stderr.log"))
                    .arg("-c")
                    .arg("sleep 60"),
            )
            .await?;
        let record = BackgroundAgentDaemonPidRecord {
            handle: existing.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: BACKGROUND_AGENT_DAEMON_PROTOCOL_VERSION,
            admission_schema_version: BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION.to_string(),
            capabilities: daemon_capabilities(),
        };
        let daemon = BackgroundAgentDaemon::new(BackgroundAgentDaemonPaths::new(
            "/bin/false",
            temp_dir.path(),
        ));
        write_pid_record(&daemon.paths.pid_file(), &record).await?;

        let output = daemon.start().await?;

        assert_eq!(output.status, BackgroundAgentDaemonStatus::AlreadyRunning);
        assert_eq!(output.pid, Some(existing.pid));
        let _ = controller.stop(&existing).await;
        Ok(())
    }

    #[tokio::test]
    async fn daemon_start_rejects_incompatible_running_pid_record() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(50),
            Duration::from_secs(5),
        );
        let existing = controller
            .spawn(
                WorkerProcessCommand::new("/bin/sh", temp_dir.path().join("existing.stderr.log"))
                    .arg("-c")
                    .arg("sleep 60"),
            )
            .await?;
        let record = BackgroundAgentDaemonPidRecord {
            handle: existing.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: BACKGROUND_AGENT_DAEMON_PROTOCOL_VERSION,
            admission_schema_version: "older-schema".to_string(),
            capabilities: daemon_capabilities(),
        };
        let daemon = BackgroundAgentDaemon::new(BackgroundAgentDaemonPaths::new(
            "/bin/false",
            temp_dir.path(),
        ));
        write_pid_record(&daemon.paths.pid_file(), &record).await?;

        let error = daemon
            .start()
            .await
            .expect_err("incompatible daemon must not be reused");

        assert!(error.to_string().contains(BACKGROUND_AGENT_DAEMON_INCOMPATIBLE));
        assert_eq!(
            controller.status(&existing).await?,
            WorkerProcessStatus::Running
        );
        let _ = controller.stop(&existing).await;
        Ok(())
    }

    #[tokio::test]
    async fn daemon_start_stops_worker_when_pid_record_write_fails() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let worker_path = temp_dir.path().join("worker.sh");
        fs::write(&worker_path, "#!/bin/sh\nsleep 60\n").await?;
        let mut permissions = std::fs::metadata(&worker_path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&worker_path, permissions)?;
        fs::create_dir(temp_dir.path().join("daemon.json.tmp")).await?;
        let daemon = BackgroundAgentDaemon::with_controller(
            BackgroundAgentDaemonPaths::new(&worker_path, temp_dir.path()),
            WorkerProcessController::with_timeouts(
                Duration::from_millis(50),
                Duration::from_secs(5),
            ),
        );

        let error = daemon
            .start()
            .await
            .expect_err("pid record write should fail");

        assert!(
            format!("{error:#}").contains("failed to write daemon pid temp file"),
            "unexpected error: {error:#}"
        );
        let process_list = tokio::process::Command::new("ps")
            .args(["-eo", "pid=,args="])
            .output()
            .await
            .context("failed to list processes")?;
        if !process_list.status.success() {
            bail!("ps failed while listing processes");
        }
        let stdout = String::from_utf8(process_list.stdout).context("ps output was not utf-8")?;
        let worker_arg = worker_path.to_string_lossy();
        let leaked_worker_pid = stdout
            .lines()
            .find(|line| line.contains(worker_arg.as_ref()))
            .map(|line| {
                line.split_whitespace()
                    .next()
                    .context("matching ps line had no pid")?
                    .parse::<u32>()
                    .context("matching ps line had an invalid pid")
            })
            .transpose()?;
        if let Some(worker_pid) = leaked_worker_pid {
            let raw_pid = libc::pid_t::try_from(worker_pid)?;
            unsafe {
                libc::kill(-raw_pid, libc::SIGKILL);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let result = unsafe {
                    libc::kill(raw_pid, /*sig*/ 0)
                };
                if result != 0
                    && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM)
                {
                    break;
                }
                if Instant::now() >= deadline {
                    bail!("timed out waiting for process {worker_pid} to exit");
                }
                sleep(Duration::from_millis(10)).await;
            }
        }
        assert_eq!(leaked_worker_pid, None);
        Ok(())
    }

    #[tokio::test]
    async fn daemon_start_preserves_publication_error_when_worker_cleanup_fails() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let worker_path = temp_dir.path().join("worker.sh");
        fs::write(&worker_path, "#!/bin/sh\ntrap '' TERM\nsleep 60\n").await?;
        let mut permissions = std::fs::metadata(&worker_path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&worker_path, permissions)?;
        let temp_pid_path = temp_dir.path().join("daemon.json.tmp");
        fs::create_dir(&temp_pid_path).await?;
        let daemon = BackgroundAgentDaemon::with_controller(
            BackgroundAgentDaemonPaths::new(&worker_path, temp_dir.path()),
            WorkerProcessController::with_timeouts(
                /*stop_grace_period*/ Duration::ZERO,
                /*hard_kill_timeout*/ Duration::ZERO,
            ),
        );

        let error = daemon
            .start()
            .await
            .expect_err("pid publication and worker cleanup should fail");
        let cleanup_context = error.to_string();
        let worker_pid = cleanup_context
            .strip_prefix(
                "failed to stop background-agent worker after pid record publication failed: \
                 timed out waiting for background agent worker process ",
            )
            .and_then(|message| message.strip_suffix(" to stop"))
            .context("cleanup error did not retain the worker pid")?
            .parse::<u32>()
            .context("cleanup error contained an invalid worker pid")?;
        let raw_pid = libc::pid_t::try_from(worker_pid)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let result = unsafe {
                libc::kill(raw_pid, /*sig*/ 0)
            };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
                break;
            }
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-raw_pid, libc::SIGKILL);
                }
                bail!("timed out waiting for process {worker_pid} to exit");
            }
            sleep(Duration::from_millis(10)).await;
        }
        let error_chain = error.chain().map(ToString::to_string).collect::<Vec<_>>();

        assert_eq!(
            error_chain.first(),
            Some(&format!(
                "failed to stop background-agent worker after pid record publication failed: \
                 timed out waiting for background agent worker process {worker_pid} to stop"
            ))
        );
        assert_eq!(
            error_chain.get(1),
            Some(&format!(
                "failed to write daemon pid temp file {}",
                temp_pid_path.display()
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn daemon_stop_removes_stale_pid_record_without_signalling() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let daemon = BackgroundAgentDaemon::new(BackgroundAgentDaemonPaths::new(
            "/bin/false",
            temp_dir.path(),
        ));
        let record = BackgroundAgentDaemonPidRecord {
            handle: WorkerProcessHandle {
                pid: std::process::id(),
                pgid: Some(std::process::id()),
                start_token: Some("not-the-current-start-token".to_string()),
                stderr_log_path: temp_dir.path().join("stale.stderr.log"),
            },
            version: "test".to_string(),
            protocol_version: 0,
            admission_schema_version: String::new(),
            capabilities: Vec::new(),
        };
        write_pid_record(&daemon.paths.pid_file(), &record).await?;

        let output = daemon.stop().await?;

        assert_eq!(output.status, BackgroundAgentDaemonStatus::StalePidRemoved);
        assert!(!daemon.paths.pid_file().exists());
        Ok(())
    }
}
