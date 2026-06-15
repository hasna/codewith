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
use tokio::time::Instant;
use tokio::time::sleep;

use crate::process_lifecycle::WorkerProcessCommand;
use crate::process_lifecycle::WorkerProcessController;
use crate::process_lifecycle::WorkerProcessHandle;
use crate::process_lifecycle::WorkerProcessLogTail;
use crate::process_lifecycle::WorkerProcessStatus;
use crate::process_lifecycle::WorkerProcessStopReport;

const DAEMON_STATE_DIR_NAME: &str = "background-agent-daemon";
const DAEMON_PID_FILE_NAME: &str = "daemon.json";
const DAEMON_LOCK_FILE_NAME: &str = "daemon.lock";
const DAEMON_STDERR_FILE_NAME: &str = "daemon.stderr.log";
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const DAEMON_STOP_GRACE_PERIOD: Duration = Duration::from_secs(35);
const DAEMON_HARD_KILL_TIMEOUT: Duration = Duration::from_secs(15);

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
                    return self
                        .output(
                            BackgroundAgentDaemonStatus::AlreadyRunning,
                            Some(record),
                            None,
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
        };
        write_pid_record(&self.paths.pid_file(), &record).await?;
        self.output(BackgroundAgentDaemonStatus::Started, Some(record), None)
            .await
    }

    pub async fn status(&self) -> Result<BackgroundAgentDaemonOutput> {
        ensure_supported_platform()?;
        let Some(record) = read_pid_record(&self.paths.pid_file()).await? else {
            return self
                .output(BackgroundAgentDaemonStatus::NotRunning, None, None)
                .await;
        };
        let status = match self.controller.status(&record.handle).await? {
            WorkerProcessStatus::Running => BackgroundAgentDaemonStatus::Running,
            WorkerProcessStatus::Missing | WorkerProcessStatus::StalePidRecord => {
                BackgroundAgentDaemonStatus::StalePidRecord
            }
        };
        self.output(status, Some(record), None).await
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
                .output(BackgroundAgentDaemonStatus::NotRunning, None, None)
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
                .stderr_tail(&record.handle, None)
                .await
                .unwrap_or(None),
            None => None,
        };
        let handle = record.as_ref().map(|record| &record.handle);
        Ok(BackgroundAgentDaemonOutput {
            status,
            pid: handle.map(|handle| handle.pid),
            pgid: handle.and_then(|handle| handle.pgid),
            version: record.map(|record| record.version),
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
            version: "test".to_string(),
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
        };
        write_pid_record(&daemon.paths.pid_file(), &record).await?;

        let output = daemon.stop().await?;

        assert_eq!(output.status, BackgroundAgentDaemonStatus::StalePidRemoved);
        assert!(!daemon.paths.pid_file().exists());
        Ok(())
    }
}
