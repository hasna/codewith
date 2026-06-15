use super::thread_monitor_api::api_thread_monitor_event_from_state;
use super::thread_monitor_api::api_thread_monitor_from_state;
use super::*;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

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

    /// Spawn the monitor's command as a streaming child confined by the
    /// session's sandbox policy — the same confinement the `shell` tool uses —
    /// instead of running model-designed commands with no sandbox or approval.
    ///
    /// Fails closed: when the session policy requires an enforcing sandbox that
    /// cannot be applied to a streaming child on this platform, the command is
    /// not run and the monitor is marked failed.
    async fn spawn_monitor_child(
        &self,
        monitor: &codex_state::ThreadMonitor,
    ) -> codex_protocol::error::Result<tokio::process::Child> {
        let argv = monitor_command_argv(&monitor.command);
        let cwd = AbsolutePathBuf::try_from(monitor_cwd(monitor, self.config.cwd.as_path()))
            .unwrap_or_else(|_| self.config.cwd.clone());
        let permission_profile = self.config.permissions.effective_permission_profile();
        let env: HashMap<String, String> = std::env::vars().collect();
        codex_core::exec::spawn_streaming_command_under_sandbox(
            argv,
            cwd,
            env,
            &permission_profile,
            &self.config.cwd,
            &self.config.codex_linux_sandbox_exe,
            self.config.features.use_legacy_landlock(),
        )
        .await
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

        let mut child = match self.spawn_monitor_child(&monitor).await {
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
            let cancel_token = cancel_token.clone();
            self.tasks.spawn(async move {
                runtime
                    .read_monitor_stream(
                        state_db,
                        monitor,
                        codex_state::ThreadMonitorEventStream::Stdout,
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
            let cancel_token = cancel_token.clone();
            self.tasks.spawn(async move {
                runtime
                    .read_monitor_stream(
                        state_db,
                        monitor,
                        codex_state::ThreadMonitorEventStream::Stderr,
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
            self.handle_monitor_output_line(&state_db, &monitor, stream, line)
                .await;
        }
    }

    async fn handle_monitor_output_line(
        &self,
        state_db: &StateDbHandle,
        monitor: &codex_state::ThreadMonitor,
        stream: codex_state::ThreadMonitorEventStream,
        line: String,
    ) {
        let line = truncate_chars(line, MAX_MONITOR_EVENT_CHARS);
        self.record_monitor_event(state_db, monitor, stream, &line)
            .await;
        if monitor.routing.writes_to_file()
            && let Err(err) = self
                .append_monitor_output_file(monitor, stream, &line)
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
            self.steer_monitor_output(monitor, line).await;
        }
    }

    async fn append_monitor_output_file(
        &self,
        monitor: &codex_state::ThreadMonitor,
        stream: codex_state::ThreadMonitorEventStream,
        line: &str,
    ) -> anyhow::Result<()> {
        let Some(output_file) = monitor.output_file.as_deref() else {
            return Ok(());
        };
        let output_path = {
            let path = Path::new(output_file);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                monitor_cwd(monitor, self.config.cwd.as_path()).join(path)
            }
        };
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
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

    async fn steer_monitor_output(&self, monitor: &codex_state::ThreadMonitor, line: String) {
        let Ok(thread) = self.thread_manager.get_thread(monitor.thread_id).await else {
            return;
        };
        let text = format!(
            "\
Codewith monitor `{}` ({}) emitted this stdout update.

Monitor purpose:
{}

Output:
{}",
            monitor.name, monitor.monitor_id, monitor.prompt, line
        );
        let result = thread
            .steer_input(
                vec![CoreInputItem::Text {
                    text,
                    text_elements: Vec::new(),
                }],
                BTreeMap::new(),
                /*expected_turn_id*/ None,
                /*client_user_message_id*/ None,
                /*responsesapi_client_metadata*/ None,
            )
            .await;
        match result {
            Ok(_) | Err(SteerInputError::NoActiveTurn(_)) => {}
            Err(err) => warn!(
                monitor_id = %monitor.monitor_id,
                thread_id = %monitor.thread_id,
                "failed to steer monitor output into thread: {err:?}"
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
                None,
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

/// Build the argv for a monitor command. The command itself is model-designed,
/// so it is always handed to a shell; callers spawn this argv under the
/// session sandbox via [`ThreadMonitorRuntime::spawn_monitor_child`] rather than
/// executing it directly.
fn monitor_command_argv(command: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec!["cmd".to_string(), "/C".to_string(), command.to_string()]
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Monitor commands are model-designed shell scripts and may use Bash
        // builtins such as `source`; `/bin/sh` is often dash on Linux.
        vec!["bash".to_string(), "-lc".to_string(), command.to_string()]
    }
}

fn monitor_cwd(monitor: &codex_state::ThreadMonitor, fallback: &Path) -> PathBuf {
    monitor
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_path_buf())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn monitor_command_supports_source_builtin() {
        let temp_dir = tempfile::tempdir().expect("tempdir should be created");
        let env_path = temp_dir.path().join("monitor.env");
        std::fs::write(&env_path, "MONITOR_VALUE=ok\n").expect("env file should be written");
        let env_path = env_path.to_string_lossy();
        let quoted_env_path = shlex::try_quote(env_path.as_ref()).expect("path should quote");
        let argv = monitor_command_argv(&format!(
            "source {quoted_env_path} && printf '__monitor_source_test__%s' \"$MONITOR_VALUE\""
        ));
        let output = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .await
            .expect("monitor command should start");

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("__monitor_source_test__ok"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn monitor_command_argv_uses_a_shell() {
        let argv = monitor_command_argv("echo hi");
        // The model-designed command must be passed to a shell as a single
        // argument (so it is later wrapped by the sandbox), never split.
        assert_eq!(argv.len(), 3);
        assert_eq!(argv[2], "echo hi");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(&argv[0..2], &["bash".to_string(), "-lc".to_string()]);
        #[cfg(target_os = "windows")]
        assert_eq!(&argv[0..2], &["cmd".to_string(), "/C".to_string()]);
    }
}
