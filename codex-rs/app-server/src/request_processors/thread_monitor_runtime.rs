use super::thread_monitor_api::api_thread_monitor_event_from_state;
use super::thread_monitor_api::api_thread_monitor_from_state;
use super::*;
use codex_protocol::AgentPath;
use codex_protocol::protocol::InterAgentCommunication;
#[cfg(not(target_os = "windows"))]
use codex_shell_command::shell_detect::ShellType;
#[cfg(not(target_os = "windows"))]
use codex_shell_command::shell_detect::get_shell;
#[cfg(not(target_os = "windows"))]
use codex_shell_command::shell_detect::ultimate_fallback_shell;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;

const MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_MONITOR_EVENT_CHARS: usize = 8_000;
const MAX_MONITOR_ERROR_CHARS: usize = 1_000;

#[derive(Clone)]
pub(crate) struct ThreadMonitorRuntime {
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    state_db: Option<StateDbHandle>,
    active: Arc<Mutex<HashMap<String, ActiveMonitor>>>,
    cancel_token: CancellationToken,
    tasks: TaskTracker,
}

#[derive(Clone)]
struct ActiveMonitor {
    generation: i64,
    cancel_token: CancellationToken,
}

impl ThreadMonitorRuntime {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        state_db: Option<StateDbHandle>,
    ) -> Self {
        Self {
            thread_manager,
            outgoing,
            config,
            state_db,
            active: Arc::new(Mutex::new(HashMap::new())),
            cancel_token: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }

    pub(crate) fn start(&self) {
        if self.state_db.is_none() {
            return;
        }
        let runtime = self.clone();
        self.tasks.spawn(async move {
            runtime.run().await;
        });
    }

    pub(crate) fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    pub(crate) async fn drain_background_tasks(&self) {
        self.shutdown();
        self.tasks.close();
        if tokio::time::timeout(Duration::from_secs(10), self.tasks.wait())
            .await
            .is_err()
        {
            warn!("timed out waiting for thread monitor runtime to shut down; proceeding");
        }
    }

    pub(crate) async fn start_monitor_now(&self, monitor: ThreadMonitor) {
        let Some(state_db) = self.state_db.as_ref() else {
            return;
        };
        let state_monitor = match state_db
            .thread_monitors()
            .get_thread_monitor(monitor.monitor_id.as_str())
            .await
        {
            Ok(Some(monitor)) => monitor,
            Ok(None) => return,
            Err(err) => {
                warn!(
                    monitor_id = monitor.monitor_id,
                    "failed to read monitor before starting: {err}"
                );
                return;
            }
        };
        self.start_monitor_if_needed(state_monitor).await;
    }

    pub(crate) async fn stop_monitor(&self, monitor_id: &str) {
        let active = self.active.lock().await.remove(monitor_id);
        if let Some(active) = active {
            active.cancel_token.cancel();
        }
    }

    async fn run(self) {
        let mut interval = tokio::time::interval(MONITOR_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => break,
                _ = interval.tick() => self.tick().await,
            }
        }
    }

    async fn tick(&self) {
        if !self.config.features.enabled(Feature::ScheduledTasks) {
            return;
        }
        let Some(state_db) = self.state_db.as_ref() else {
            return;
        };
        let monitors = match state_db
            .thread_monitors()
            .list_running_thread_monitors()
            .await
        {
            Ok(monitors) => monitors,
            Err(err) => {
                warn!("failed to list running thread monitors: {err}");
                return;
            }
        };
        let running_generations = monitors
            .iter()
            .map(|monitor| (monitor.monitor_id.clone(), monitor.generation))
            .collect::<HashMap<_, _>>();
        for monitor in monitors {
            self.start_monitor_if_needed(monitor).await;
        }

        let active_ids = self
            .active
            .lock()
            .await
            .iter()
            .filter_map(|(monitor_id, active)| {
                let running_generation = running_generations.get(monitor_id)?;
                (active.generation == *running_generation).then_some(monitor_id.clone())
            })
            .collect::<HashSet<_>>();
        let all_active_ids = self.active.lock().await.keys().cloned().collect::<Vec<_>>();
        for monitor_id in all_active_ids {
            if !active_ids.contains(&monitor_id) && !running_generations.contains_key(&monitor_id) {
                self.stop_monitor(monitor_id.as_str()).await;
            }
        }
    }

    async fn start_monitor_if_needed(&self, monitor: codex_state::ThreadMonitor) {
        if monitor.status != codex_state::ThreadMonitorStatus::Running {
            return;
        }

        // Only run the monitor in the process that currently hosts its thread.
        //
        // `list_running_thread_monitors` is a global `status = 'running'` query
        // with no owner/lease scoping, so every app-server sharing CODEX_HOME
        // (daemon + `codewith exec` + embedded TUI app-server) would otherwise
        // spawn its own copy of every running monitor: N duplicate child
        // processes, N duplicate `thread_monitor_events` rows, and — for a
        // consuming monitor (e.g. one that fetches-and-marks-read new messages)
        // — a non-hosting copy could drain the watched source so the hosting
        // thread never observes the event. Injection also requires the loaded
        // thread (see `inject_monitor_output`), so a monitor that cannot be
        // co-located with its thread here can never steer output into it. This
        // gate does mean a monitor whose thread is not loaded in any process is
        // paused until the thread is reloaded, which is correct for
        // stream-routing monitors and an acceptable change for file-routing
        // background loggers (previously run, buggily, from every process).
        if self
            .thread_manager
            .get_thread(monitor.thread_id)
            .await
            .is_err()
        {
            return;
        }

        let mut active = self.active.lock().await;
        if let Some(existing) = active.get(monitor.monitor_id.as_str()) {
            if existing.generation == monitor.generation {
                return;
            }
            existing.cancel_token.cancel();
            active.remove(monitor.monitor_id.as_str());
        }

        let cancel_token = self.cancel_token.child_token();
        active.insert(
            monitor.monitor_id.clone(),
            ActiveMonitor {
                generation: monitor.generation,
                cancel_token: cancel_token.clone(),
            },
        );
        drop(active);

        let runtime = self.clone();
        self.tasks.spawn(async move {
            runtime.execute_monitor(monitor, cancel_token).await;
        });
    }

    async fn execute_monitor(
        &self,
        monitor: codex_state::ThreadMonitor,
        cancel_token: CancellationToken,
    ) {
        let Some(state_db) = self.state_db.as_ref().cloned() else {
            self.remove_active_monitor(&monitor.monitor_id, monitor.generation)
                .await;
            return;
        };
        self.record_monitor_event(
            &state_db,
            &monitor,
            codex_state::ThreadMonitorEventStream::System,
            "monitor process starting",
        )
        .await;

        let thread_cwd = match monitor_thread_cwd(&state_db, &monitor).await {
            Ok(cwd) => cwd,
            Err(err) => {
                let error = monitor_error(format!("invalid monitor thread cwd: {err}"));
                self.mark_monitor_failed(&state_db, &monitor, error).await;
                self.remove_active_monitor(&monitor.monitor_id, monitor.generation)
                    .await;
                return;
            }
        };
        let cwd = match resolve_monitor_cwd(&monitor, thread_cwd.as_path()).await {
            Ok(cwd) => cwd,
            Err(err) => {
                let error = monitor_error(format!("invalid monitor cwd: {err}"));
                self.mark_monitor_failed(&state_db, &monitor, error).await;
                self.remove_active_monitor(&monitor.monitor_id, monitor.generation)
                    .await;
                return;
            }
        };
        let mut command = monitor_command(&monitor.command);
        command
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let error = monitor_error(format!("failed to start monitor command: {err}"));
                self.mark_monitor_failed(&state_db, &monitor, error).await;
                self.remove_active_monitor(&monitor.monitor_id, monitor.generation)
                    .await;
                return;
            }
        };
        let process_id = child.id().map(i64::from);
        match state_db
            .thread_monitors()
            .mark_thread_monitor_started(
                monitor.monitor_id.as_str(),
                monitor.generation,
                process_id,
            )
            .await
        {
            Ok(Some(updated)) => self.emit_monitor_updated(updated).await,
            Ok(None) => {}
            Err(err) => warn!(
                monitor_id = %monitor.monitor_id,
                "failed to mark monitor as started: {err}"
            ),
        }

        if let Some(stdout) = child.stdout.take() {
            let runtime = self.clone();
            let state_db = state_db.clone();
            let monitor = monitor.clone();
            let cwd = cwd.clone();
            let cancel_token = cancel_token.clone();
            self.tasks.spawn(async move {
                runtime
                    .read_monitor_stream(
                        state_db,
                        monitor,
                        codex_state::ThreadMonitorEventStream::Stdout,
                        cwd,
                        stdout,
                        cancel_token,
                    )
                    .await;
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let runtime = self.clone();
            let state_db = state_db.clone();
            let monitor = monitor.clone();
            let cwd = cwd.clone();
            let cancel_token = cancel_token.clone();
            self.tasks.spawn(async move {
                runtime
                    .read_monitor_stream(
                        state_db,
                        monitor,
                        codex_state::ThreadMonitorEventStream::Stderr,
                        cwd,
                        stderr,
                        cancel_token,
                    )
                    .await;
            });
        }

        let wait_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                if let Err(err) = child.kill().await {
                    warn!(monitor_id = %monitor.monitor_id, "failed to kill monitor process: {err}");
                }
                child.wait().await
            }
            status = child.wait() => status,
        };

        if cancel_token.is_cancelled() {
            self.remove_active_monitor(&monitor.monitor_id, monitor.generation)
                .await;
            return;
        }

        match wait_result {
            Ok(status) if status.success() => {
                self.mark_monitor_stopped(&state_db, &monitor, "monitor process exited")
                    .await;
            }
            Ok(status) => {
                let error = monitor_error(format!("monitor process exited with status {status}"));
                self.mark_monitor_failed(&state_db, &monitor, error).await;
            }
            Err(err) => {
                let error = monitor_error(format!("failed to wait for monitor process: {err}"));
                self.mark_monitor_failed(&state_db, &monitor, error).await;
            }
        }

        self.remove_active_monitor(&monitor.monitor_id, monitor.generation)
            .await;
    }

    async fn read_monitor_stream<T>(
        &self,
        state_db: StateDbHandle,
        monitor: codex_state::ThreadMonitor,
        stream: codex_state::ThreadMonitorEventStream,
        cwd: PathBuf,
        pipe: T,
        cancel_token: CancellationToken,
    ) where
        T: tokio::io::AsyncRead + Unpin,
    {
        let mut lines = BufReader::new(pipe).lines();
        loop {
            let line = tokio::select! {
                _ = cancel_token.cancelled() => break,
                line = lines.next_line() => line,
            };
            let line = match line {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(err) => {
                    warn!(monitor_id = %monitor.monitor_id, "failed to read monitor output: {err}");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            self.handle_monitor_output_line(&state_db, &monitor, stream, cwd.as_path(), line)
                .await;
        }
    }

    async fn handle_monitor_output_line(
        &self,
        state_db: &StateDbHandle,
        monitor: &codex_state::ThreadMonitor,
        stream: codex_state::ThreadMonitorEventStream,
        cwd: &Path,
        line: String,
    ) {
        let line = truncate_chars(line, MAX_MONITOR_EVENT_CHARS);
        self.record_monitor_event(state_db, monitor, stream, &line)
            .await;
        if monitor.routing.writes_to_file()
            && let Err(err) = self
                .append_monitor_output_file(monitor, stream, cwd, &line)
                .await
        {
            warn!(
                monitor_id = %monitor.monitor_id,
                "failed to append monitor output file: {err}"
            );
        }
        if stream == codex_state::ThreadMonitorEventStream::Stdout
            && monitor.routing.streams_to_thread()
        {
            self.inject_monitor_output(monitor, line).await;
        }
    }

    async fn append_monitor_output_file(
        &self,
        monitor: &codex_state::ThreadMonitor,
        stream: codex_state::ThreadMonitorEventStream,
        cwd: &Path,
        line: &str,
    ) -> anyhow::Result<()> {
        let Some(output_file) = monitor.output_file.as_deref() else {
            return Ok(());
        };
        let output_path = resolve_monitor_relative_path("monitor outputFile", cwd, output_file)?;
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
            let canonical_cwd = tokio::fs::canonicalize(cwd).await?;
            let canonical_parent = tokio::fs::canonicalize(parent).await?;
            if !canonical_parent.starts_with(canonical_cwd.as_path()) {
                anyhow::bail!("monitor outputFile parent must stay within monitor cwd");
            }
        }
        if let Ok(metadata) = tokio::fs::symlink_metadata(&output_path).await
            && metadata.file_type().is_symlink()
        {
            anyhow::bail!("monitor outputFile must not be a symlink");
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(output_path)
            .await?;
        let timestamp = Utc::now().to_rfc3339();
        file.write_all(format!("{timestamp} [{}] {line}\n", stream.as_str()).as_bytes())
            .await?;
        Ok(())
    }

    /// Injects a monitor stdout line into the thread using the same durable
    /// mailbox path that inter-agent messages use.
    ///
    /// The previous implementation pushed the line via `thread.steer_input`,
    /// which places items only into the turn-scoped `TurnState.pending_input`.
    /// That dropped the line whenever the thread was idle (no active turn) and
    /// wiped it when a turn was interrupted before its next model call
    /// (`clear_pending`), so monitor events did not reach the model reliably.
    ///
    /// Routing through `deliver_inter_agent_communication` instead lands the
    /// line in the session-level mailbox queue, which (a) survives turn abort,
    /// (b) is drained into the next model call, and (c) wakes an idle thread
    /// (`trigger_turn`), matching inter-agent mailbox reliability.
    async fn inject_monitor_output(&self, monitor: &codex_state::ThreadMonitor, line: String) {
        let Ok(thread) = self.thread_manager.get_thread(monitor.thread_id).await else {
            return;
        };
        let communication = monitor_output_communication(monitor, line);
        match thread
            .deliver_inter_agent_communication(communication)
            .await
        {
            Ok(()) => {}
            // The thread stopped running between the host check and delivery;
            // there is no live session to inject into, so drop quietly.
            Err(CodexErr::InternalAgentDied) => {}
            Err(err) => warn!(
                monitor_id = %monitor.monitor_id,
                thread_id = %monitor.thread_id,
                "failed to inject monitor output into thread mailbox: {err:?}"
            ),
        }
    }

    async fn mark_monitor_stopped(
        &self,
        state_db: &StateDbHandle,
        monitor: &codex_state::ThreadMonitor,
        message: &str,
    ) {
        self.record_monitor_event(
            state_db,
            monitor,
            codex_state::ThreadMonitorEventStream::System,
            message,
        )
        .await;
        match state_db
            .thread_monitors()
            .set_thread_monitor_status(
                monitor.monitor_id.as_str(),
                codex_state::ThreadMonitorStatus::Stopped,
                /*last_error*/ None,
            )
            .await
        {
            Ok(Some(updated)) => self.emit_monitor_updated(updated).await,
            Ok(None) => {}
            Err(err) => warn!(
                monitor_id = %monitor.monitor_id,
                "failed to mark monitor stopped: {err}"
            ),
        }
    }

    async fn mark_monitor_failed(
        &self,
        state_db: &StateDbHandle,
        monitor: &codex_state::ThreadMonitor,
        error: String,
    ) {
        self.record_monitor_event(
            state_db,
            monitor,
            codex_state::ThreadMonitorEventStream::System,
            &error,
        )
        .await;
        match state_db
            .thread_monitors()
            .set_thread_monitor_status(
                monitor.monitor_id.as_str(),
                codex_state::ThreadMonitorStatus::Failed,
                Some(error),
            )
            .await
        {
            Ok(Some(updated)) => self.emit_monitor_updated(updated).await,
            Ok(None) => {}
            Err(err) => warn!(
                monitor_id = %monitor.monitor_id,
                "failed to mark monitor failed: {err}"
            ),
        }
    }

    async fn record_monitor_event(
        &self,
        state_db: &StateDbHandle,
        monitor: &codex_state::ThreadMonitor,
        stream: codex_state::ThreadMonitorEventStream,
        text: &str,
    ) {
        let event = match state_db
            .thread_monitors()
            .create_thread_monitor_event(codex_state::ThreadMonitorEventCreateParams {
                thread_id: monitor.thread_id,
                monitor_id: monitor.monitor_id.clone(),
                stream,
                text: text.to_string(),
            })
            .await
        {
            Ok(event) => event,
            Err(err) => {
                warn!(
                    monitor_id = %monitor.monitor_id,
                    "failed to record monitor event: {err}"
                );
                return;
            }
        };
        let monitor = match state_db
            .thread_monitors()
            .get_thread_monitor(monitor.monitor_id.as_str())
            .await
        {
            Ok(Some(monitor)) => monitor,
            Ok(None) => return,
            Err(err) => {
                warn!(
                    monitor_id = %monitor.monitor_id,
                    "failed to read monitor after event: {err}"
                );
                return;
            }
        };
        self.outgoing
            .send_server_notification(ServerNotification::ThreadMonitorEvent(
                ThreadMonitorEventNotification {
                    thread_id: monitor.thread_id.to_string(),
                    monitor: api_thread_monitor_from_state(monitor),
                    event: api_thread_monitor_event_from_state(event),
                },
            ))
            .await;
    }

    async fn emit_monitor_updated(&self, monitor: codex_state::ThreadMonitor) {
        self.outgoing
            .send_server_notification(ServerNotification::ThreadMonitorUpdated(
                ThreadMonitorUpdatedNotification {
                    thread_id: monitor.thread_id.to_string(),
                    monitor: api_thread_monitor_from_state(monitor),
                },
            ))
            .await;
    }

    async fn remove_active_monitor(&self, monitor_id: &str, generation: i64) {
        let mut active = self.active.lock().await;
        if active
            .get(monitor_id)
            .is_some_and(|active| active.generation == generation)
        {
            active.remove(monitor_id);
        }
    }
}

async fn monitor_thread_cwd(
    state_db: &StateDbHandle,
    monitor: &codex_state::ThreadMonitor,
) -> anyhow::Result<PathBuf> {
    let metadata = state_db
        .get_thread(monitor.thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("monitor thread metadata not found"))?;
    if metadata.cwd.as_os_str().is_empty() {
        anyhow::bail!("monitor thread cwd is empty");
    }
    Ok(metadata.cwd)
}

/// Builds the mailbox communication injected into a thread for a monitor line.
///
/// `trigger_turn` is `true` so an idle thread is woken to observe the event
/// (parity with inter-agent mailbox `UserInstruction` delivery); while a turn is
/// already running the message is queued and delivered on the next turn instead
/// of being dropped.
fn monitor_output_communication(
    monitor: &codex_state::ThreadMonitor,
    line: String,
) -> InterAgentCommunication {
    let content = format!(
        "\
Codewith monitor `{}` ({}) emitted this stdout update.

Monitor purpose:
{}

Output:
{}",
        monitor.name, monitor.monitor_id, monitor.prompt, line
    );
    InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::root(),
        Vec::new(),
        content,
        /*trigger_turn*/ true,
    )
}

fn monitor_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell = get_shell(ShellType::Bash, /*path*/ None)
            .or_else(|| get_shell(ShellType::Zsh, /*path*/ None))
            .or_else(|| get_shell(ShellType::Sh, /*path*/ None))
            .unwrap_or_else(ultimate_fallback_shell);
        let mut cmd = Command::new(shell.shell_path);
        cmd.arg("-lc").arg(command);
        cmd
    }
}

async fn resolve_monitor_cwd(
    monitor: &codex_state::ThreadMonitor,
    fallback: &Path,
) -> anyhow::Result<PathBuf> {
    let cwd = match monitor.cwd.as_deref() {
        Some(cwd) => resolve_monitor_relative_path("monitor cwd", fallback, cwd)?,
        None => fallback.to_path_buf(),
    };
    let canonical_fallback = tokio::fs::canonicalize(fallback).await?;
    let canonical_cwd = tokio::fs::canonicalize(&cwd).await?;
    if !canonical_cwd.starts_with(canonical_fallback.as_path()) {
        anyhow::bail!("monitor cwd must stay within the thread cwd");
    }
    Ok(canonical_cwd)
}

fn resolve_monitor_relative_path(
    field_name: &str,
    base: &Path,
    value: &str,
) -> anyhow::Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("{field_name} must be a relative path within the thread cwd");
    }
    if !path
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("{field_name} must include a path component");
    }
    Ok(base.join(path))
}

fn monitor_error(error: String) -> String {
    truncate_chars(error, MAX_MONITOR_ERROR_CHARS)
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    value.chars().take(max_chars).collect()
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn monitor_relative_path_resolver_stays_within_base() {
        let base = Path::new("/workspace");
        assert_eq!(
            resolve_monitor_relative_path("monitor outputFile", base, "logs/out.log")
                .expect("relative path should resolve"),
            PathBuf::from("/workspace/logs/out.log")
        );

        assert!(resolve_monitor_relative_path("monitor outputFile", base, "/tmp/out.log").is_err());
        assert!(resolve_monitor_relative_path("monitor outputFile", base, "../out.log").is_err());
        assert!(resolve_monitor_relative_path("monitor outputFile", base, ".").is_err());
    }

    #[tokio::test]
    async fn monitor_cwd_resolves_against_thread_cwd() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let app_server_cwd = tempdir.path().join("server");
        let thread_cwd = tempdir.path().join("thread");
        let thread_logs = thread_cwd.join("logs");
        let server_logs = app_server_cwd.join("logs");
        tokio::fs::create_dir_all(&thread_logs)
            .await
            .expect("thread logs dir");
        tokio::fs::create_dir_all(&server_logs)
            .await
            .expect("server logs dir");
        let monitor = test_monitor(Some("logs"));
        let resolved = resolve_monitor_cwd(&monitor, thread_cwd.as_path())
            .await
            .expect("thread-relative cwd should resolve");

        assert_eq!(
            resolved,
            tokio::fs::canonicalize(&thread_logs)
                .await
                .expect("canonical thread logs")
        );
        assert_ne!(
            resolved,
            tokio::fs::canonicalize(&server_logs)
                .await
                .expect("canonical server logs")
        );
    }

    #[tokio::test]
    async fn monitor_thread_cwd_reads_persisted_thread_metadata() -> anyhow::Result<()> {
        let tempdir = tempfile::TempDir::new()?;
        let app_server_cwd = tempdir.path().join("server");
        let thread_cwd = tempdir.path().join("thread");
        tokio::fs::create_dir_all(&app_server_cwd).await?;
        tokio::fs::create_dir_all(&thread_cwd).await?;
        let state_db =
            codex_state::StateRuntime::init(tempdir.path().join("state"), "test-provider".into())
                .await?;
        let thread_id = codex_protocol::ThreadId::new();
        let mut builder = codex_state::ThreadMetadataBuilder::new(
            thread_id,
            tempdir.path().join("rollout.jsonl"),
            chrono::Utc::now(),
            codex_protocol::protocol::SessionSource::default(),
        );
        builder.cwd = thread_cwd.clone();
        state_db
            .upsert_thread(&builder.build("test-provider"))
            .await?;
        let monitor = test_monitor_for_thread(thread_id, None);

        assert_eq!(monitor_thread_cwd(&state_db, &monitor).await?, thread_cwd);
        assert_ne!(
            monitor_thread_cwd(&state_db, &monitor).await?,
            app_server_cwd
        );
        Ok(())
    }

    #[tokio::test]
    async fn monitor_command_supports_bash_source_when_bash_is_available() {
        if get_shell(ShellType::Bash, /*path*/ None).is_none() {
            return;
        }

        let output = monitor_command("source /dev/null && printf ok")
            .output()
            .await
            .expect("monitor command should run");

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "ok");
    }

    #[test]
    fn monitor_output_communication_uses_wake_if_idle_mailbox_shape() {
        let monitor = test_monitor(None);
        let communication =
            monitor_output_communication(&monitor, "new conversations message".to_string());

        // Routed as a root-addressed mailbox message so it uses the durable
        // queue + wake-if-idle path rather than the turn-scoped steer buffer.
        assert_eq!(AgentPath::root(), communication.author);
        assert_eq!(AgentPath::root(), communication.recipient);
        assert!(
            communication.trigger_turn,
            "monitor output must wake an idle thread"
        );
        // The human-readable monitor context is preserved in the payload.
        assert!(communication.content.contains(monitor.name.as_str()));
        assert!(communication.content.contains(monitor.monitor_id.as_str()));
        assert!(communication.content.contains(monitor.prompt.as_str()));
        assert!(communication.content.contains("new conversations message"));
    }

    fn test_monitor(cwd: Option<&str>) -> codex_state::ThreadMonitor {
        test_monitor_for_thread(codex_protocol::ThreadId::new(), cwd)
    }

    fn test_monitor_for_thread(
        thread_id: codex_protocol::ThreadId,
        cwd: Option<&str>,
    ) -> codex_state::ThreadMonitor {
        let now = chrono::Utc::now();
        codex_state::ThreadMonitor {
            thread_id,
            monitor_id: "monitor-id".to_string(),
            name: "monitor".to_string(),
            prompt: "watch".to_string(),
            command: "printf ok".to_string(),
            cwd: cwd.map(str::to_string),
            routing: codex_state::ThreadMonitorRouting::File,
            output_file: Some("monitor.log".to_string()),
            status: codex_state::ThreadMonitorStatus::Running,
            generation: 1,
            process_id: None,
            last_event_at: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        }
    }
}
