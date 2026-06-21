use std::ffi::OsString;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
#[cfg(unix)]
use tokio::process::Command;
#[cfg(unix)]
use tokio::time::Instant;
#[cfg(unix)]
use tokio::time::sleep;

const DEFAULT_STOP_GRACE_PERIOD: Duration = Duration::from_secs(10);
const DEFAULT_HARD_KILL_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const DEFAULT_STDERR_TAIL_BYTES: u64 = 4096;

/// Command specification for a supervised background-agent worker process.
///
/// The process lifecycle backend owns OS process creation and cleanup only. It
/// does not own durable run state, event replay, or pending interactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerProcessCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub stderr_log_path: PathBuf,
}

impl WorkerProcessCommand {
    pub fn new(program: impl Into<PathBuf>, stderr_log_path: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            stderr_log_path: stderr_log_path.into(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

/// Platform process handle recorded for durable worker stop/recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerProcessHandle {
    pub pid: u32,
    pub pgid: Option<u32>,
    pub start_token: Option<String>,
    pub stderr_log_path: PathBuf,
}

/// Tail of a worker stderr log suitable for diagnostics responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerProcessLogTail {
    pub path: PathBuf,
    pub contents: String,
}

/// Liveness check result for a recorded worker handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerProcessStatus {
    Missing,
    Running,
    StalePidRecord,
}

/// Result of an attempted worker shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerProcessStopReport {
    pub signal_sent: bool,
    pub killed_after_grace: bool,
    pub stale_pid_record: bool,
}

/// Reusable OS process lifecycle backend for background-agent workers.
///
/// Unix workers are spawned into their own process group so stop requests can
/// terminate child and grandchild processes. Non-Unix platforms intentionally
/// return an unsupported error until equivalent Job Object cleanup exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerProcessController {
    pub stop_grace_period: Duration,
    pub hard_kill_timeout: Duration,
}

impl Default for WorkerProcessController {
    fn default() -> Self {
        Self {
            stop_grace_period: DEFAULT_STOP_GRACE_PERIOD,
            hard_kill_timeout: DEFAULT_HARD_KILL_TIMEOUT,
        }
    }
}

impl WorkerProcessController {
    pub fn with_timeouts(stop_grace_period: Duration, hard_kill_timeout: Duration) -> Self {
        Self {
            stop_grace_period,
            hard_kill_timeout,
        }
    }

    #[cfg(unix)]
    pub async fn status(&self, handle: &WorkerProcessHandle) -> Result<WorkerProcessStatus> {
        process_match(handle).await.map(WorkerProcessStatus::from)
    }

    #[cfg(not(unix))]
    pub async fn status(&self, _handle: &WorkerProcessHandle) -> Result<WorkerProcessStatus> {
        bail!(
            "background-agent worker process lifecycle is unsupported on this platform until Job Object cleanup is implemented"
        )
    }

    #[cfg(unix)]
    pub async fn spawn(&self, request: WorkerProcessCommand) -> Result<WorkerProcessHandle> {
        if let Some(parent) = request.stderr_log_path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "failed to create worker stderr log directory {}",
                    parent.display()
                )
            })?;
        }
        let stderr_log = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&request.stderr_log_path)
            .await
            .with_context(|| {
                format!(
                    "failed to open background agent worker stderr log {}",
                    request.stderr_log_path.display()
                )
            })?;
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_log.into_std().await));
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        unsafe {
            command.pre_exec(codex_utils_pty::process_group::set_process_group);
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn background agent worker process {}",
                request.program.display()
            )
        })?;
        let pid = child
            .id()
            .context("spawned background agent worker has no pid")?;
        let start_token = match read_process_start_token(pid).await {
            Ok(start_token) => Some(start_token),
            Err(err) => {
                let _ = codex_utils_pty::process_group::kill_process_group_by_pid(pid);
                let _ = child.wait().await;
                return Err(err).with_context(|| {
                    format!("failed to record background agent worker pid {pid} start token")
                });
            }
        };
        let stderr_log_path = request.stderr_log_path;
        tokio::spawn(async move {
            let _ = child.wait().await;
            let _ = codex_utils_pty::process_group::kill_process_group(pid);
        });

        Ok(WorkerProcessHandle {
            pid,
            pgid: Some(pid),
            start_token,
            stderr_log_path,
        })
    }

    #[cfg(not(unix))]
    pub async fn spawn(&self, _request: WorkerProcessCommand) -> Result<WorkerProcessHandle> {
        bail!(
            "background-agent worker process lifecycle is unsupported on this platform until Job Object cleanup is implemented"
        )
    }

    #[cfg(unix)]
    pub async fn stop(&self, handle: &WorkerProcessHandle) -> Result<WorkerProcessStopReport> {
        match process_match(handle).await? {
            ProcessMatch::Missing => {
                return Ok(WorkerProcessStopReport {
                    signal_sent: false,
                    killed_after_grace: false,
                    stale_pid_record: false,
                });
            }
            ProcessMatch::Stale => {
                return Ok(WorkerProcessStopReport {
                    signal_sent: false,
                    killed_after_grace: false,
                    stale_pid_record: true,
                });
            }
            ProcessMatch::Matches => {}
        }

        let signal_sent = terminate_handle(handle)?;
        if !signal_sent {
            return Ok(WorkerProcessStopReport {
                signal_sent: false,
                killed_after_grace: false,
                stale_pid_record: false,
            });
        }

        let grace_deadline = Instant::now() + self.stop_grace_period;
        while Instant::now() < grace_deadline {
            match process_match(handle).await? {
                ProcessMatch::Missing => {
                    return Ok(WorkerProcessStopReport {
                        signal_sent: true,
                        killed_after_grace: false,
                        stale_pid_record: false,
                    });
                }
                ProcessMatch::Stale => {
                    return Ok(WorkerProcessStopReport {
                        signal_sent: true,
                        killed_after_grace: false,
                        stale_pid_record: true,
                    });
                }
                ProcessMatch::Matches => sleep(STOP_POLL_INTERVAL).await,
            }
        }

        kill_handle(handle)?;
        let kill_deadline = Instant::now() + self.hard_kill_timeout;
        while Instant::now() < kill_deadline {
            match process_match(handle).await? {
                ProcessMatch::Missing => {
                    return Ok(WorkerProcessStopReport {
                        signal_sent: true,
                        killed_after_grace: true,
                        stale_pid_record: false,
                    });
                }
                ProcessMatch::Stale => {
                    return Ok(WorkerProcessStopReport {
                        signal_sent: true,
                        killed_after_grace: true,
                        stale_pid_record: true,
                    });
                }
                ProcessMatch::Matches => sleep(STOP_POLL_INTERVAL).await,
            }
        }

        bail!(
            "timed out waiting for background agent worker process {} to stop",
            handle.pid
        )
    }

    #[cfg(not(unix))]
    pub async fn stop(&self, _handle: &WorkerProcessHandle) -> Result<WorkerProcessStopReport> {
        bail!(
            "background-agent worker process lifecycle is unsupported on this platform until Job Object cleanup is implemented"
        )
    }

    pub async fn stderr_tail(
        &self,
        handle: &WorkerProcessHandle,
        byte_limit: Option<u64>,
    ) -> Result<Option<WorkerProcessLogTail>> {
        read_worker_stderr_tail(
            &handle.stderr_log_path,
            byte_limit.unwrap_or(DEFAULT_STDERR_TAIL_BYTES),
        )
        .await
    }
}

pub async fn read_worker_stderr_tail(
    path: &Path,
    byte_limit: u64,
) -> Result<Option<WorkerProcessLogTail>> {
    let mut file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to open worker stderr log {}", path.display()));
        }
    };
    let len = file
        .metadata()
        .await
        .with_context(|| format!("failed to inspect worker stderr log {}", path.display()))?
        .len();
    if len == 0 {
        return Ok(None);
    }

    let start = len.saturating_sub(byte_limit);
    file.seek(SeekFrom::Start(start))
        .await
        .with_context(|| format!("failed to seek worker stderr log {}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .await
        .with_context(|| format!("failed to read worker stderr log {}", path.display()))?;
    if start > 0
        && let Some(newline_index) = bytes.iter().position(|byte| *byte == b'\n')
    {
        bytes.drain(..=newline_index);
    }
    let contents = String::from_utf8_lossy(&bytes).trim_end().to_string();
    if contents.is_empty() {
        return Ok(None);
    }
    Ok(Some(WorkerProcessLogTail {
        path: path.to_path_buf(),
        contents,
    }))
}

#[cfg(unix)]
pub async fn current_process_start_token() -> Result<String> {
    read_process_start_token(std::process::id()).await
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessMatch {
    Missing,
    Matches,
    Stale,
}

#[cfg(unix)]
impl From<ProcessMatch> for WorkerProcessStatus {
    fn from(value: ProcessMatch) -> Self {
        match value {
            ProcessMatch::Missing => Self::Missing,
            ProcessMatch::Matches => Self::Running,
            ProcessMatch::Stale => Self::StalePidRecord,
        }
    }
}

#[cfg(unix)]
async fn process_match(handle: &WorkerProcessHandle) -> Result<ProcessMatch> {
    if !process_exists(handle.pid) {
        return Ok(ProcessMatch::Missing);
    }
    let Some(expected_start_token) = handle.start_token.as_deref() else {
        return Ok(ProcessMatch::Stale);
    };
    match read_process_start_token(handle.pid).await {
        Ok(actual_start_token) if actual_start_token == expected_start_token => {
            Ok(ProcessMatch::Matches)
        }
        Ok(_) => Ok(ProcessMatch::Stale),
        Err(_err) if !process_exists(handle.pid) => Ok(ProcessMatch::Missing),
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn terminate_handle(handle: &WorkerProcessHandle) -> Result<bool> {
    if let Some(pgid) = handle.pgid {
        return codex_utils_pty::process_group::terminate_process_group(pgid)
            .with_context(|| format!("failed to terminate worker process group {pgid}"));
    }
    terminate_process(handle.pid)
}

#[cfg(unix)]
fn kill_handle(handle: &WorkerProcessHandle) -> Result<()> {
    if let Some(pgid) = handle.pgid {
        return codex_utils_pty::process_group::kill_process_group(pgid)
            .with_context(|| format!("failed to kill worker process group {pgid}"));
    }
    kill_process(handle.pid)
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> Result<bool> {
    let raw_pid = libc::pid_t::try_from(pid)
        .with_context(|| format!("background agent worker pid {pid} is out of range"))?;
    let result = unsafe { libc::kill(raw_pid, libc::SIGTERM) };
    if result == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(err).with_context(|| format!("failed to terminate background agent worker {pid}"))
}

#[cfg(unix)]
fn kill_process(pid: u32) -> Result<()> {
    let raw_pid = libc::pid_t::try_from(pid)
        .with_context(|| format!("background agent worker pid {pid} is out of range"))?;
    let result = unsafe { libc::kill(raw_pid, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(err).with_context(|| format!("failed to kill background agent worker {pid}"))
}

#[cfg(unix)]
async fn read_process_start_token(pid: u32) -> Result<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .await
        .context("failed to invoke ps for background agent worker pid")?;
    if !output.status.success() {
        bail!("failed to read start time for background agent worker {pid}");
    }

    let start_token = String::from_utf8(output.stdout)
        .context("background agent worker start time was not utf-8")?;
    let start_token = start_token.trim();
    if start_token.is_empty() {
        bail!("background agent worker {pid} has no recorded start time");
    }
    Ok(start_token.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn stop_kills_worker_process_group_children() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let ready_path = temp_dir.path().join("ready");
        let child_pid_path = temp_dir.path().join("child.pid");
        let stderr_path = temp_dir.path().join("worker.stderr.log");
        let script = r#"
trap '' TERM
(trap '' TERM; sleep 60) &
echo $! > "$1"
touch "$2"
wait
"#;
        let command = WorkerProcessCommand::new("/bin/sh", &stderr_path)
            .arg("-c")
            .arg(script)
            .arg("background-agent-worker-test")
            .arg(child_pid_path.as_os_str().to_os_string())
            .arg(ready_path.as_os_str().to_os_string());
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(50),
            Duration::from_secs(5),
        );
        let handle = controller.spawn(command).await?;
        wait_for_path(&ready_path).await?;
        let child_pid: u32 = fs::read_to_string(&child_pid_path)
            .await?
            .trim()
            .parse()
            .context("parse child pid")?;
        assert!(process_exists(handle.pid));
        assert!(process_exists(child_pid));

        let report = controller.stop(&handle).await?;

        assert_eq!(
            report,
            WorkerProcessStopReport {
                signal_sent: true,
                killed_after_grace: true,
                stale_pid_record: false,
            }
        );
        wait_for_process_exit(handle.pid).await?;
        wait_for_process_exit(child_pid).await?;
        Ok(())
    }

    #[tokio::test]
    async fn stop_refuses_to_signal_stale_pid_record() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let handle = WorkerProcessHandle {
            pid: std::process::id(),
            pgid: Some(std::process::id()),
            start_token: Some("not-the-current-process-start-token".to_string()),
            stderr_log_path: temp_dir.path().join("stderr.log"),
        };
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(1),
            Duration::from_millis(1),
        );

        let report = controller.stop(&handle).await?;

        assert_eq!(
            report,
            WorkerProcessStopReport {
                signal_sent: false,
                killed_after_grace: false,
                stale_pid_record: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_start_token_is_treated_as_stale() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let handle = WorkerProcessHandle {
            pid: std::process::id(),
            pgid: Some(std::process::id()),
            start_token: None,
            stderr_log_path: temp_dir.path().join("stderr.log"),
        };
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(1),
            Duration::from_millis(1),
        );

        assert_eq!(
            controller.status(&handle).await?,
            WorkerProcessStatus::StalePidRecord
        );
        assert_eq!(
            controller.stop(&handle).await?,
            WorkerProcessStopReport {
                signal_sent: false,
                killed_after_grace: false,
                stale_pid_record: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn stderr_tail_returns_recent_complete_lines() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let stderr_path = temp_dir.path().join("worker.stderr.log");
        let command = WorkerProcessCommand::new("/bin/sh", &stderr_path)
            .arg("-c")
            .arg(format!(
                "printf '{}\\nrecent error\\nusage\\n' >&2",
                "x".repeat(4100)
            ));
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(50),
            Duration::from_secs(5),
        );
        let handle = controller.spawn(command).await?;
        wait_for_process_exit(handle.pid).await?;

        let tail = controller.stderr_tail(&handle, Some(4096)).await?;

        assert_eq!(
            tail,
            Some(WorkerProcessLogTail {
                path: stderr_path,
                contents: "recent error\nusage".to_string(),
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn watcher_kills_descendants_when_worker_exits() -> Result<()> {
        let temp_dir = TempDir::new().expect("temp dir");
        let ready_path = temp_dir.path().join("ready");
        let child_pid_path = temp_dir.path().join("child.pid");
        let stderr_path = temp_dir.path().join("worker.stderr.log");
        let script = r#"
(trap '' TERM; sleep 60) &
echo $! > "$1"
touch "$2"
exit 0
"#;
        let command = WorkerProcessCommand::new("/bin/sh", &stderr_path)
            .arg("-c")
            .arg(script)
            .arg("background-agent-worker-test")
            .arg(child_pid_path.as_os_str().to_os_string())
            .arg(ready_path.as_os_str().to_os_string());
        let controller = WorkerProcessController::with_timeouts(
            Duration::from_millis(50),
            Duration::from_secs(5),
        );
        let handle = controller.spawn(command).await?;
        wait_for_path(&ready_path).await?;
        let child_pid: u32 = fs::read_to_string(&child_pid_path)
            .await?
            .trim()
            .parse()
            .context("parse child pid")?;

        wait_for_process_exit(handle.pid).await?;
        wait_for_process_exit(child_pid).await?;
        Ok(())
    }

    async fn wait_for_path(path: &Path) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            sleep(Duration::from_millis(10)).await;
        }
        bail!("timed out waiting for {}", path.display())
    }

    async fn wait_for_process_exit(pid: u32) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return Ok(());
            }
            sleep(Duration::from_millis(10)).await;
        }
        bail!("timed out waiting for process {pid} to exit")
    }
}
