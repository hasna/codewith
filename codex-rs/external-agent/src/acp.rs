use std::collections::BTreeMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::ExternalAgentActionRequest;
use crate::ExternalAgentActionResult;
use crate::ExternalAgentError;
use crate::ExternalAgentEvent;
use crate::ExternalAgentHarness;
use crate::ExternalAgentHarnessKind;
use crate::ExternalAgentHost;
use crate::ExternalAgentLaunchIsolation;
use crate::ExternalAgentLaunchSpec;
use crate::ExternalAgentMode;
use crate::ExternalAgentPermissionDecision;
use crate::ExternalAgentPermissionOption;
use crate::ExternalAgentPermissionRequest;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentReadinessStatus;
use crate::ExternalAgentRequest;
use crate::ExternalAgentResult;
use crate::ExternalAgentRunStatus;
use crate::ExternalAgentRuntime;
use crate::ExternalAgentRuntimeDescriptor;
use crate::ExternalAgentRuntimeId;
use crate::ExternalAgentSandboxConfig;
use crate::ExternalAgentSandboxedLaunchSpec;
use crate::ExternalAgentSessionRequest;
use crate::ExternalAgentSessionState;
use crate::FileSystemCapability;
use crate::TerminalCapability;
use crate::find_external_agent_runtime;
use crate::platform_sandbox_external_agent_launch_with_writable_roots;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::Lines;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::task::JoinHandle;

const SAFE_ENV_VARS: &[&str] = &["LANG", "LC_ALL", "LC_CTYPE", "PATH", "TERM"];
const ACP_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const ACP_CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(1);
const ACP_STDERR_MAX_BYTES: usize = 64 * 1024;
static ACP_ISOLATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Environment policy for ACP subprocess launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpEnvironmentPolicy {
    inherited_vars: Vec<String>,
}

impl AcpEnvironmentPolicy {
    pub fn sanitized() -> Self {
        Self {
            inherited_vars: SAFE_ENV_VARS
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        }
    }

    pub fn sanitize(
        &self,
        source: &BTreeMap<String, String>,
        extra: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        for name in &self.inherited_vars {
            if let Some(value) = source.get(name) {
                env.insert(name.clone(), value.clone());
            }
        }
        for (name, value) in extra {
            env.insert(name.clone(), value.clone());
        }
        env
    }
}

impl Default for AcpEnvironmentPolicy {
    fn default() -> Self {
        Self::sanitized()
    }
}

/// Per-run filesystem roots exposed to an ACP subprocess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpProcessIsolation {
    pub root: PathBuf,
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub cache_home: PathBuf,
    pub data_home: PathBuf,
    pub temp_dir: PathBuf,
}

impl AcpProcessIsolation {
    pub fn create(runtime: &ExternalAgentRuntimeId) -> Result<Self, ExternalAgentError> {
        let now_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let sequence = ACP_ISOLATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir()
            .join("codewith-external-agent")
            .join(format!(
                "{}-{}-{sequence}",
                safe_path_segment(runtime.as_str()),
                now_nanos
            ));
        let isolation = Self {
            home: root.join("home"),
            config_home: root.join("config"),
            cache_home: root.join("cache"),
            data_home: root.join("data"),
            temp_dir: root.join("tmp"),
            root,
        };
        for path in [
            &isolation.home,
            &isolation.config_home,
            &isolation.cache_home,
            &isolation.data_home,
            &isolation.temp_dir,
        ] {
            std::fs::create_dir_all(path).map_err(|err| ExternalAgentError::NotReady {
                runtime: runtime.as_str().to_string(),
                reason: format!(
                    "failed to create external-agent isolation directory `{}`: {err}",
                    path.display()
                ),
            })?;
        }
        Ok(isolation)
    }

    pub fn env(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("CODEWITH_HOME".to_string(), self.home.display().to_string()),
            ("CODEX_HOME".to_string(), self.home.display().to_string()),
            (
                "CODEWITH_EXTERNAL_AGENT_HOME".to_string(),
                self.root.display().to_string(),
            ),
            ("HOME".to_string(), self.home.display().to_string()),
            ("USERPROFILE".to_string(), self.home.display().to_string()),
            (
                "APPDATA".to_string(),
                self.config_home.display().to_string(),
            ),
            (
                "LOCALAPPDATA".to_string(),
                self.data_home.display().to_string(),
            ),
            ("TEMP".to_string(), self.temp_dir.display().to_string()),
            ("TMP".to_string(), self.temp_dir.display().to_string()),
            ("TMPDIR".to_string(), self.temp_dir.display().to_string()),
            (
                "XDG_CACHE_HOME".to_string(),
                self.cache_home.display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                self.config_home.display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_string(),
                self.data_home.display().to_string(),
            ),
        ])
    }
}

/// Common ACP stdio harness for external-agent adapters.
pub struct AcpStdioHarness {
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: AcpEnvironmentPolicy,
}

impl AcpStdioHarness {
    pub fn new(descriptor: &'static ExternalAgentRuntimeDescriptor) -> Self {
        Self {
            descriptor,
            env_policy: AcpEnvironmentPolicy::default(),
        }
    }

    pub fn descriptor(&self) -> &'static ExternalAgentRuntimeDescriptor {
        self.descriptor
    }

    pub fn launch_spec(
        &self,
        cwd: impl Into<PathBuf>,
        resolved_program: impl Into<PathBuf>,
        source_env: &BTreeMap<String, String>,
        extra_env: &BTreeMap<String, String>,
    ) -> ExternalAgentLaunchSpec {
        ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from(self.descriptor.id),
            program: resolved_program.into(),
            args: self
                .descriptor
                .command
                .args
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            arg0: None,
            cwd: cwd.into(),
            env: self.env_policy.sanitize(source_env, extra_env),
            isolation: ExternalAgentLaunchIsolation::unenforced(
                "external-agent ACP launch has not been wrapped in a Codewith platform sandbox",
            ),
        }
    }

    fn runtime_missing_readiness(&self, detail: String) -> ExternalAgentReadiness {
        ExternalAgentReadiness {
            runtime: self.id(),
            status: ExternalAgentReadinessStatus::MissingRuntime,
            display_name: self.descriptor.display_name.to_string(),
            version: None,
            supported_modes: self.descriptor.supported_modes.to_vec(),
            detail: Some(detail),
        }
    }

    fn runtime_ready_readiness(&self, program: &Path) -> ExternalAgentReadiness {
        ExternalAgentReadiness {
            runtime: self.id(),
            status: ExternalAgentReadinessStatus::Ready,
            display_name: self.descriptor.display_name.to_string(),
            version: None,
            supported_modes: self.descriptor.supported_modes.to_vec(),
            detail: Some(program.display().to_string()),
        }
    }

    /// Run this ACP runtime after wrapping its child process in Codewith's
    /// platform sandbox.
    pub async fn run_sandboxed(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        let source_env = std::env::vars().collect::<BTreeMap<_, _>>();
        self.run_sandboxed_with_env(request, host, sandbox_config, source_env)
            .await
    }

    pub async fn run_sandboxed_with_env(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
        source_env: BTreeMap<String, String>,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        self.validate_request(&request)?;
        let program = self.resolve_program(&request, &source_env)?;
        let isolation = AcpProcessIsolation::create(&request.runtime)?;
        let isolation_root = isolation.root.clone();
        let mut extra_env = isolation.env();
        extra_env.insert(
            "CODEWITH_EXTERNAL_AGENT_RUNTIME".to_string(),
            request.runtime.as_str().to_string(),
        );
        let launch = self.launch_spec(request.cwd.clone(), program, &source_env, &extra_env);
        let launch = platform_sandbox_external_agent_launch_with_writable_roots(
            launch,
            sandbox_config,
            vec![isolation.root.clone()],
        )?;

        let result = self.run_sandboxed_launch(request, host, launch).await;
        let _ = std::fs::remove_dir_all(isolation_root);
        result
    }

    fn resolve_program(
        &self,
        request: &ExternalAgentRequest,
        source_env: &BTreeMap<String, String>,
    ) -> Result<PathBuf, ExternalAgentError> {
        let path = source_env.get("PATH").map(String::as_str);
        which::which_in(self.descriptor.command.program, path, &request.cwd).map_err(|err| {
            ExternalAgentError::NotReady {
                runtime: request.runtime.as_str().to_string(),
                reason: err.to_string(),
            }
        })
    }

    fn validate_request(&self, request: &ExternalAgentRequest) -> Result<(), ExternalAgentError> {
        if request.runtime.as_str() != self.descriptor.id {
            return Err(invalid_request(
                &request.runtime,
                format!(
                    "request runtime does not match harness runtime `{}`",
                    self.descriptor.id
                ),
            ));
        }
        if !self.descriptor.supported_modes.contains(&request.mode) {
            return Err(invalid_request(
                &request.runtime,
                format!("mode `{:?}` is not supported by this runtime", request.mode),
            ));
        }
        let expected_capabilities = crate::ExternalAgentCapabilities::for_mode(request.mode);
        if request.capabilities != expected_capabilities {
            return Err(invalid_request(
                &request.runtime,
                "request capabilities must match the selected external-agent mode",
            ));
        }
        Ok(())
    }

    async fn run_sandboxed_launch(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        launch: ExternalAgentSandboxedLaunchSpec,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        let mut process = AcpStdioProcess::spawn(launch)?;
        let result = self.run_protocol(&mut process, &request, &host).await;
        if let Err(err) = &result {
            let event = match err {
                ExternalAgentError::Cancelled => ExternalAgentEvent::Cancelled {
                    reason: Some("cancelled by host".to_string()),
                },
                _ => ExternalAgentEvent::Failed {
                    message: err.to_string(),
                },
            };
            let _ = host.emit(event).await;
        }
        process.shutdown().await;
        result
    }

    async fn run_protocol<H>(
        &self,
        process: &mut AcpStdioProcess,
        request: &ExternalAgentRequest,
        host: &H,
    ) -> Result<ExternalAgentResult, ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        process
            .request(
                "initialize",
                initialize_params(request),
                host,
                AcpRequestContext {
                    request,
                    session: None,
                },
            )
            .await?;

        let session_result = process
            .request(
                session_method(&request.session),
                session_params(request),
                host,
                AcpRequestContext {
                    request,
                    session: None,
                },
            )
            .await?;
        let session = session_state_from_result(request, &session_result)?;
        host.emit(ExternalAgentEvent::RunStarted {
            session: session.clone(),
        })
        .await?;

        if let Some(mode_id) = acp_mode_id(request.mode) {
            process
                .request(
                    "session/set_mode",
                    json!({
                        "sessionId": session
                            .external_session_id
                            .as_deref()
                            .unwrap_or_default(),
                        "modeId": mode_id,
                    }),
                    host,
                    AcpRequestContext {
                        request,
                        session: Some(&session),
                    },
                )
                .await?;
        }

        process
            .request(
                "session/prompt",
                json!({
                    "sessionId": session
                        .external_session_id
                        .as_deref()
                        .unwrap_or_default(),
                    "prompt": [{
                        "type": "text",
                        "text": request.task.clone(),
                    }],
                }),
                host,
                AcpRequestContext {
                    request,
                    session: Some(&session),
                },
            )
            .await?;

        let result = ExternalAgentResult {
            status: ExternalAgentRunStatus::Completed,
            session,
            summary: process.summary(),
            artifacts: Vec::new(),
        };
        host.emit(ExternalAgentEvent::Completed {
            result: result.clone(),
        })
        .await?;
        Ok(result)
    }
}

pub fn cursor_acp_harness() -> Option<AcpStdioHarness> {
    find_external_agent_runtime(ExternalAgentRuntimeId::CURSOR).map(AcpStdioHarness::new)
}

pub fn grok_build_acp_harness() -> Option<AcpStdioHarness> {
    find_external_agent_runtime(ExternalAgentRuntimeId::GROK_BUILD).map(AcpStdioHarness::new)
}

impl ExternalAgentRuntime for AcpStdioHarness {
    fn id(&self) -> ExternalAgentRuntimeId {
        ExternalAgentRuntimeId::from(self.descriptor.id)
    }

    async fn readiness(&self) -> ExternalAgentReadiness {
        match which::which(self.descriptor.command.program) {
            Ok(program) => self.runtime_ready_readiness(&program),
            Err(err) => self.runtime_missing_readiness(err.to_string()),
        }
    }

    async fn run(
        &self,
        request: ExternalAgentRequest,
        _host: impl ExternalAgentHost + Send + Sync,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        self.validate_request(&request)?;
        Err(ExternalAgentError::NotReady {
            runtime: request.runtime.as_str().to_string(),
            reason: "ACP runtimes must be executed with AcpStdioHarness::run_sandboxed".to_string(),
        })
    }
}

impl ExternalAgentHarness for AcpStdioHarness {
    fn harness_kind(&self) -> ExternalAgentHarnessKind {
        ExternalAgentHarnessKind::AcpStdio
    }
}

struct AcpRequestContext<'a> {
    request: &'a ExternalAgentRequest,
    session: Option<&'a ExternalAgentSessionState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalExecution {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

pub struct AcpStdioProcess {
    runtime: ExternalAgentRuntimeId,
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: Option<JoinHandle<String>>,
    next_id: u64,
    next_terminal_id: u64,
    terminals: BTreeMap<String, TerminalExecution>,
    output_text: String,
    idle_timeout: Duration,
}

impl AcpStdioProcess {
    pub fn spawn(launch: ExternalAgentSandboxedLaunchSpec) -> Result<Self, ExternalAgentError> {
        let launch = launch.into_launch_spec();
        let runtime = launch.runtime.clone();
        if let Some(reason) = launch.isolation.unenforced_reason() {
            return Err(ExternalAgentError::NotReady {
                runtime: runtime.as_str().to_string(),
                reason: reason.to_string(),
            });
        }
        let mut command = Command::new(&launch.program);
        #[cfg(unix)]
        if let Some(arg0) = launch.arg0 {
            command.arg0(arg0);
        }
        #[cfg(not(unix))]
        let _ = launch.arg0;
        #[cfg(unix)]
        unsafe {
            command.pre_exec(codex_utils_pty::process_group::set_process_group);
        }
        command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(&launch.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| protocol_error(&runtime, format!("spawn failed: {err}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stderr"))?;
        let stderr = tokio::spawn(read_bounded_stderr(stderr, ACP_STDERR_MAX_BYTES));

        Ok(Self {
            runtime,
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stderr: Some(stderr),
            next_id: 1,
            next_terminal_id: 1,
            terminals: BTreeMap::new(),
            output_text: String::new(),
            idle_timeout: ACP_IDLE_TIMEOUT,
        })
    }

    async fn request<H>(
        &mut self,
        method: &str,
        params: JsonValue,
        host: &H,
        context: AcpRequestContext<'_>,
    ) -> Result<JsonValue, ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        self.await_response(id, host, context).await
    }

    async fn await_response<H>(
        &mut self,
        id: u64,
        host: &H,
        context: AcpRequestContext<'_>,
    ) -> Result<JsonValue, ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let started_at = Instant::now();
        loop {
            if host.is_cancelled().await {
                self.shutdown().await;
                return Err(ExternalAgentError::Cancelled);
            }

            let line = match tokio::time::timeout(ACP_CANCEL_POLL_INTERVAL, self.stdout.next_line())
                .await
            {
                Ok(line) => line
                    .map_err(|err| protocol_error(&self.runtime, format!("read failed: {err}")))?,
                Err(_) => {
                    if started_at.elapsed() >= self.idle_timeout {
                        self.shutdown().await;
                        return Err(self
                            .protocol_error_with_stderr("timed out waiting for ACP output")
                            .await);
                    }
                    continue;
                }
            };
            let Some(line) = line else {
                self.shutdown().await;
                return Err(self.protocol_error_with_stderr("ACP process exited").await);
            };
            if line.trim().is_empty() {
                continue;
            }
            let message = serde_json::from_str::<JsonValue>(&line).map_err(|err| {
                protocol_error(&self.runtime, format!("invalid JSON-RPC line: {err}"))
            })?;

            if is_response_for(&message, id) {
                return response_result(&self.runtime, message);
            }
            if message.get("method").and_then(JsonValue::as_str) == Some("session/update") {
                if session_update_matches_context(context.session, &message) {
                    self.handle_session_update(&message, host, context.session)
                        .await?;
                }
                continue;
            }
            if message.get("method").and_then(JsonValue::as_str).is_some()
                && message.get("id").is_some()
            {
                if !server_request_matches_context(context.session, &message) {
                    let id = message
                        .get("id")
                        .cloned()
                        .unwrap_or(JsonValue::String("unknown".to_string()));
                    self.write_error(id, -32050, "ACP server request session mismatch")
                        .await?;
                    continue;
                }
                self.handle_server_request(&message, host, &context).await?;
            }
        }
    }

    async fn handle_session_update<H>(
        &mut self,
        message: &JsonValue,
        host: &H,
        _session: Option<&ExternalAgentSessionState>,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let Some(update) = message
            .get("params")
            .and_then(|params| params.get("update"))
        else {
            return Ok(());
        };
        let Some(kind) = update.get("sessionUpdate").and_then(JsonValue::as_str) else {
            return Ok(());
        };
        match kind {
            "agent_message_chunk" | "assistant_message_chunk" => {
                if let Some(text) = update
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(JsonValue::as_str)
                {
                    self.output_text.push_str(text);
                    host.emit(ExternalAgentEvent::OutputTextDelta {
                        text: text.to_string(),
                    })
                    .await?;
                }
            }
            "reasoning_chunk" | "thinking_chunk" => {
                if let Some(text) = update
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(JsonValue::as_str)
                    .or_else(|| update.get("text").and_then(JsonValue::as_str))
                {
                    host.emit(ExternalAgentEvent::ReasoningDelta {
                        text: text.to_string(),
                    })
                    .await?;
                }
            }
            "tool_call" => {
                let label = update
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("ACP tool call")
                    .to_string();
                host.emit(ExternalAgentEvent::ProposedAction {
                    proposal: ExternalAgentActionRequest::Other {
                        label,
                        payload: update.clone(),
                    },
                })
                .await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_server_request<H>(
        &mut self,
        message: &JsonValue,
        host: &H,
        context: &AcpRequestContext<'_>,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let id = message
            .get("id")
            .cloned()
            .ok_or_else(|| protocol_error(&self.runtime, "server request missing id"))?;
        let method = message
            .get("method")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| protocol_error(&self.runtime, "server request missing method"))?;
        let params = message
            .get("params")
            .cloned()
            .unwrap_or(JsonValue::Object(Default::default()));

        match method {
            "session/request_permission" => {
                self.respond_to_permission_request(id, params, host).await
            }
            "fs/read_text_file" => {
                self.respond_to_read_request(id, params, host, context)
                    .await
            }
            "fs/write_text_file" => {
                self.respond_to_write_request(id, params, host, context)
                    .await
            }
            "terminal/create" => {
                self.respond_to_terminal_create(id, params, host, context)
                    .await
            }
            "terminal/output" => self.respond_to_terminal_output(id, params).await,
            "terminal/wait_for_exit" => self.respond_to_terminal_wait(id, params).await,
            "terminal/kill" | "terminal/release" => self.write_result(id, json!({})).await,
            _ => {
                self.write_error(
                    id,
                    -32601,
                    format!("unsupported ACP server request `{method}`"),
                )
                .await
            }
        }
    }

    async fn respond_to_permission_request<H>(
        &mut self,
        id: JsonValue,
        params: JsonValue,
        host: &H,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let acp_options = acp_permission_options(&params);
        let request = ExternalAgentPermissionRequest {
            id: json_id_to_string(&id),
            action: permission_action(&params),
            options: acp_options
                .iter()
                .map(|option| match option.decision {
                    ExternalAgentPermissionDecision::AllowOnce => {
                        ExternalAgentPermissionOption::AllowOnce
                    }
                    ExternalAgentPermissionDecision::RejectOnce => {
                        ExternalAgentPermissionOption::RejectOnce
                    }
                })
                .collect::<Vec<_>>(),
        };
        host.emit(ExternalAgentEvent::PermissionRequested {
            request: request.clone(),
        })
        .await?;
        let decision = host.request_permission(request).await?;
        let option_id = acp_options
            .iter()
            .find(|option| option.decision == decision)
            .map(|option| option.option_id.clone());
        match option_id {
            Some(option_id) => {
                self.write_result(
                    id,
                    json!({
                        "outcome": {
                            "outcome": "selected",
                            "optionId": option_id,
                        },
                    }),
                )
                .await
            }
            None => {
                self.write_result(
                    id,
                    json!({
                        "outcome": "cancelled",
                    }),
                )
                .await
            }
        }
    }

    async fn respond_to_read_request<H>(
        &mut self,
        id: JsonValue,
        params: JsonValue,
        host: &H,
        context: &AcpRequestContext<'_>,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        if matches!(
            context.request.capabilities.filesystem,
            FileSystemCapability::None
        ) {
            return self
                .write_error(id, -32010, "filesystem reads are not allowed for this run")
                .await;
        }
        let path = match confined_path_from_params(context.request, &params, "path") {
            Ok(path) => path,
            Err(message) => return self.write_error(id, -32011, message).await,
        };
        match host
            .perform_action(ExternalAgentActionRequest::ReadFile { path })
            .await?
        {
            ExternalAgentActionResult::FileContent { content } => {
                self.write_result(id, json!({ "content": content })).await
            }
            ExternalAgentActionResult::Rejected { reason } => {
                self.write_error(id, -32012, reason).await
            }
            _ => {
                self.write_error(id, -32013, "host returned an invalid read result")
                    .await
            }
        }
    }

    async fn respond_to_write_request<H>(
        &mut self,
        id: JsonValue,
        params: JsonValue,
        host: &H,
        context: &AcpRequestContext<'_>,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        if context.request.capabilities.filesystem != FileSystemCapability::ManagedReadWrite {
            return self
                .write_error(id, -32020, "filesystem writes require managed mode")
                .await;
        }
        let path = match confined_path_from_params(context.request, &params, "path") {
            Ok(path) => path,
            Err(message) => return self.write_error(id, -32021, message).await,
        };
        let content = params
            .get("content")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string();
        let action = ExternalAgentActionRequest::WriteFile { path, content };
        let permission = ExternalAgentPermissionRequest {
            id: json_id_to_string(&id),
            action: action.clone(),
            options: vec![
                ExternalAgentPermissionOption::AllowOnce,
                ExternalAgentPermissionOption::RejectOnce,
            ],
        };
        host.emit(ExternalAgentEvent::PermissionRequested {
            request: permission.clone(),
        })
        .await?;
        if host.request_permission(permission).await? != ExternalAgentPermissionDecision::AllowOnce
        {
            return self.write_error(id, -32022, "write rejected").await;
        }
        match host.perform_action(action).await? {
            ExternalAgentActionResult::WriteAccepted => self.write_result(id, json!({})).await,
            ExternalAgentActionResult::Rejected { reason } => {
                self.write_error(id, -32023, reason).await
            }
            _ => {
                self.write_error(id, -32024, "host returned an invalid write result")
                    .await
            }
        }
    }

    async fn respond_to_terminal_create<H>(
        &mut self,
        id: JsonValue,
        params: JsonValue,
        host: &H,
        context: &AcpRequestContext<'_>,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        if context.request.capabilities.terminal != TerminalCapability::Managed {
            return self
                .write_error(id, -32030, "terminal access requires managed mode")
                .await;
        }
        let action = match terminal_action(context.request, &params) {
            Ok(action) => action,
            Err(message) => return self.write_error(id, -32034, message).await,
        };
        let permission = ExternalAgentPermissionRequest {
            id: json_id_to_string(&id),
            action: action.clone(),
            options: vec![
                ExternalAgentPermissionOption::AllowOnce,
                ExternalAgentPermissionOption::RejectOnce,
            ],
        };
        host.emit(ExternalAgentEvent::PermissionRequested {
            request: permission.clone(),
        })
        .await?;
        if host.request_permission(permission).await? != ExternalAgentPermissionDecision::AllowOnce
        {
            return self
                .write_error(id, -32031, "terminal command rejected")
                .await;
        }
        match host.perform_action(action).await? {
            ExternalAgentActionResult::CommandOutput {
                exit_code,
                stdout,
                stderr,
            } => {
                let terminal_id = format!("codewith-{}", self.next_terminal_id);
                self.next_terminal_id += 1;
                self.terminals.insert(
                    terminal_id.clone(),
                    TerminalExecution {
                        exit_code,
                        stdout,
                        stderr,
                    },
                );
                self.write_result(id, json!({ "terminalId": terminal_id }))
                    .await
            }
            ExternalAgentActionResult::Rejected { reason } => {
                self.write_error(id, -32032, reason).await
            }
            _ => {
                self.write_error(id, -32033, "host returned an invalid terminal result")
                    .await
            }
        }
    }

    async fn respond_to_terminal_output(
        &mut self,
        id: JsonValue,
        params: JsonValue,
    ) -> Result<(), ExternalAgentError> {
        let terminal_id = params
            .get("terminalId")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let Some(output) = self.terminals.get(terminal_id) else {
            return self.write_error(id, -32040, "unknown terminal").await;
        };
        self.write_result(
            id,
            json!({
                "output": format!("{}{}", output.stdout, output.stderr),
                "exitStatus": {
                    "exitCode": output.exit_code,
                },
                "truncated": false,
            }),
        )
        .await
    }

    async fn respond_to_terminal_wait(
        &mut self,
        id: JsonValue,
        params: JsonValue,
    ) -> Result<(), ExternalAgentError> {
        let terminal_id = params
            .get("terminalId")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let Some(output) = self.terminals.get(terminal_id) else {
            return self.write_error(id, -32041, "unknown terminal").await;
        };
        self.write_result(id, json!({ "exitCode": output.exit_code }))
            .await
    }

    async fn write_result(
        &mut self,
        id: JsonValue,
        result: JsonValue,
    ) -> Result<(), ExternalAgentError> {
        self.write_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
        .await
    }

    async fn write_error(
        &mut self,
        id: JsonValue,
        code: i64,
        message: impl Into<String>,
    ) -> Result<(), ExternalAgentError> {
        self.write_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message.into(),
            },
        }))
        .await
    }

    async fn write_json(&mut self, value: JsonValue) -> Result<(), ExternalAgentError> {
        let mut line = serde_json::to_vec(&value)
            .map_err(|err| protocol_error(&self.runtime, format!("encode failed: {err}")))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|err| protocol_error(&self.runtime, format!("write failed: {err}")))
    }

    fn summary(&self) -> Option<String> {
        let summary = self.output_text.trim();
        if summary.is_empty() {
            None
        } else {
            Some(summary.to_string())
        }
    }

    async fn shutdown(&mut self) {
        let _ = codex_utils_pty::process_group::kill_child_process_group(&mut self.child);
        let _ = self.child.kill().await;
    }

    async fn protocol_error_with_stderr(&mut self, message: &str) -> ExternalAgentError {
        let runtime = self.runtime.clone();
        let stderr = self.take_stderr().await;
        protocol_error(&runtime, append_stderr(message, stderr.as_deref()))
    }

    async fn take_stderr(&mut self) -> Option<String> {
        let stderr = self.stderr.take()?;
        match stderr.await {
            Ok(stderr) if !stderr.trim().is_empty() => Some(stderr.trim().to_string()),
            Ok(_) => None,
            Err(err) => Some(format!("stderr reader failed: {err}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpPermissionOption {
    option_id: String,
    decision: ExternalAgentPermissionDecision,
}

fn initialize_params(request: &ExternalAgentRequest) -> JsonValue {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "fs": {
                "readTextFile": request.capabilities.filesystem != FileSystemCapability::None,
                "writeTextFile": request.capabilities.filesystem == FileSystemCapability::ManagedReadWrite,
            },
            "terminal": request.capabilities.terminal == TerminalCapability::Managed,
        },
    })
}

fn session_method(session: &ExternalAgentSessionRequest) -> &'static str {
    match session {
        ExternalAgentSessionRequest::New => "session/new",
        ExternalAgentSessionRequest::Resume { .. } => "session/load",
    }
}

fn session_params(request: &ExternalAgentRequest) -> JsonValue {
    let mut params = json!({
        "cwd": request.cwd.to_string_lossy(),
        "mcpServers": [],
    });
    if let ExternalAgentSessionRequest::Resume {
        external_session_id,
    } = &request.session
    {
        params["sessionId"] = JsonValue::String(external_session_id.clone());
    }
    params
}

fn session_state_from_result(
    request: &ExternalAgentRequest,
    result: &JsonValue,
) -> Result<ExternalAgentSessionState, ExternalAgentError> {
    let session_id = result
        .get("sessionId")
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    if session_id.is_none() {
        return Err(protocol_error(
            &request.runtime,
            "ACP session response did not include sessionId",
        ));
    }
    Ok(ExternalAgentSessionState {
        runtime: request.runtime.clone(),
        external_session_id: session_id,
        mode: request.mode,
        cwd: request.cwd.clone(),
    })
}

fn acp_mode_id(mode: ExternalAgentMode) -> Option<&'static str> {
    match mode {
        ExternalAgentMode::Plan => Some("plan"),
        ExternalAgentMode::Consult | ExternalAgentMode::Propose | ExternalAgentMode::Managed => {
            None
        }
    }
}

fn is_response_for(message: &JsonValue, id: u64) -> bool {
    message.get("id").and_then(JsonValue::as_u64) == Some(id) && message.get("method").is_none()
}

fn response_result(
    runtime: &ExternalAgentRuntimeId,
    message: JsonValue,
) -> Result<JsonValue, ExternalAgentError> {
    if let Some(error) = message.get("error") {
        return Err(protocol_error(runtime, error.to_string()));
    }
    Ok(message.get("result").cloned().unwrap_or(JsonValue::Null))
}

fn acp_permission_options(params: &JsonValue) -> Vec<AcpPermissionOption> {
    params
        .get("options")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let option_id = option.get("optionId").and_then(JsonValue::as_str)?;
            let kind = option.get("kind").and_then(JsonValue::as_str)?;
            let decision = if kind.contains("allow") {
                ExternalAgentPermissionDecision::AllowOnce
            } else if kind.contains("reject") {
                ExternalAgentPermissionDecision::RejectOnce
            } else {
                return None;
            };
            Some(AcpPermissionOption {
                option_id: option_id.to_string(),
                decision,
            })
        })
        .collect()
}

fn permission_action(params: &JsonValue) -> ExternalAgentActionRequest {
    let label = params
        .get("toolCall")
        .and_then(|tool_call| tool_call.get("title"))
        .and_then(JsonValue::as_str)
        .unwrap_or("ACP permission request")
        .to_string();
    ExternalAgentActionRequest::Other {
        label,
        payload: params.clone(),
    }
}

fn confined_path_from_params(
    request: &ExternalAgentRequest,
    params: &JsonValue,
    key: &str,
) -> Result<PathBuf, String> {
    let path = params
        .get(key)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("missing `{key}`"))?;
    confine_path(&request.cwd, Path::new(path))
}

fn confine_path(cwd: &Path, path: &Path) -> Result<PathBuf, String> {
    let cwd = normalize_lexical(cwd);
    let path = if path.is_absolute() {
        normalize_lexical(path)
    } else {
        normalize_lexical(&cwd.join(path))
    };
    if path.starts_with(&cwd) {
        Ok(path)
    } else {
        Err(format!(
            "path `{}` is outside cwd `{}`",
            path.display(),
            cwd.display()
        ))
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

fn terminal_action(
    request: &ExternalAgentRequest,
    params: &JsonValue,
) -> Result<ExternalAgentActionRequest, String> {
    let command = params
        .get("command")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    let cwd = params
        .get("cwd")
        .and_then(JsonValue::as_str)
        .map(PathBuf::from)
        .map(|path| confine_path(&request.cwd, &path))
        .transpose()?;
    Ok(ExternalAgentActionRequest::RunCommand {
        command: vec![command],
        cwd,
    })
}

fn json_id_to_string(id: &JsonValue) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

fn safe_path_segment(value: &str) -> String {
    let segment = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if segment.is_empty() {
        "agent".to_string()
    } else {
        segment
    }
}

async fn read_bounded_stderr(mut reader: impl AsyncRead + Unpin, max_bytes: usize) -> String {
    let mut output = Vec::new();
    let mut buf = [0_u8; 4096];
    let mut truncated = false;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = max_bytes.saturating_sub(output.len());
                if remaining > 0 {
                    output.extend_from_slice(&buf[..n.min(remaining)]);
                }
                truncated |= n > remaining;
            }
            Err(err) => return format!("failed to read stderr: {err}"),
        }
    }

    let mut text = String::from_utf8_lossy(&output).into_owned();
    if truncated {
        text.push_str("\n[stderr truncated]");
    }
    text
}

fn append_stderr(message: &str, stderr: Option<&str>) -> String {
    let Some(stderr) = stderr.map(str::trim).filter(|stderr| !stderr.is_empty()) else {
        return message.to_string();
    };
    format!("{message}; stderr: {stderr}")
}

fn server_request_matches_context(
    session: Option<&ExternalAgentSessionState>,
    message: &JsonValue,
) -> bool {
    session_message_matches_context(session, message)
}

fn session_update_matches_context(
    session: Option<&ExternalAgentSessionState>,
    message: &JsonValue,
) -> bool {
    session_message_matches_context(session, message)
}

fn session_message_matches_context(
    session: Option<&ExternalAgentSessionState>,
    message: &JsonValue,
) -> bool {
    let Some(expected_session_id) =
        session.and_then(|session| session.external_session_id.as_ref())
    else {
        return true;
    };
    message
        .get("params")
        .and_then(|params| params.get("sessionId"))
        .and_then(JsonValue::as_str)
        .is_some_and(|actual_session_id| actual_session_id == expected_session_id)
}

fn protocol_error(
    runtime: &ExternalAgentRuntimeId,
    message: impl Into<String>,
) -> ExternalAgentError {
    ExternalAgentError::Protocol {
        runtime: runtime.as_str().to_string(),
        message: message.into(),
    }
}

fn invalid_request(
    runtime: &ExternalAgentRuntimeId,
    message: impl Into<String>,
) -> ExternalAgentError {
    ExternalAgentError::Runtime {
        runtime: runtime.as_str().to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_external_agent_runtime;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[test]
    fn sanitized_environment_only_keeps_safe_names_and_explicit_extras() {
        let source = BTreeMap::from([
            ("HOME".to_string(), "/home/me".to_string()),
            ("PATH".to_string(), "/bin".to_string()),
            ("OPENAI_API_KEY".to_string(), "secret".to_string()),
            ("XAI_API_KEY".to_string(), "secret".to_string()),
        ]);
        let extra = BTreeMap::from([(
            "CODEWITH_EXTERNAL_AGENT_RUN_ID".to_string(),
            "run-1".to_string(),
        )]);

        let env = AcpEnvironmentPolicy::sanitized().sanitize(&source, &extra);

        assert_eq!(
            env,
            BTreeMap::from([
                (
                    "CODEWITH_EXTERNAL_AGENT_RUN_ID".to_string(),
                    "run-1".to_string()
                ),
                ("PATH".to_string(), "/bin".to_string()),
            ])
        );
    }

    #[test]
    fn launch_spec_uses_canonical_runtime_command_and_sanitized_env() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let source = BTreeMap::from([("PATH".to_string(), "/bin".to_string())]);
        let extra = BTreeMap::new();

        let spec = harness.launch_spec("/repo", "/usr/bin/grok", &source, &extra);

        assert_eq!(
            spec,
            ExternalAgentLaunchSpec {
                runtime: ExternalAgentRuntimeId::from("grok-build"),
                program: PathBuf::from("/usr/bin/grok"),
                args: vec!["agent".to_string(), "stdio".to_string()],
                arg0: None,
                cwd: PathBuf::from("/repo"),
                env: BTreeMap::from([("PATH".to_string(), "/bin".to_string())]),
                isolation: ExternalAgentLaunchIsolation::unenforced(
                    "external-agent ACP launch has not been wrapped in a Codewith platform sandbox",
                ),
            }
        );
    }

    #[test]
    fn process_isolation_env_sets_private_home_and_config_roots() {
        let isolation = AcpProcessIsolation::create(&ExternalAgentRuntimeId::from("fake/runtime"))
            .unwrap_or_else(|err| panic!("create isolation: {err}"));
        let env = isolation.env();

        assert!(isolation.root.exists());
        assert!(isolation.home.exists());
        assert!(isolation.config_home.exists());
        assert!(isolation.cache_home.exists());
        assert!(isolation.data_home.exists());
        assert!(isolation.temp_dir.exists());
        assert_eq!(
            env,
            BTreeMap::from([
                (
                    "APPDATA".to_string(),
                    isolation.config_home.display().to_string(),
                ),
                (
                    "CODEWITH_EXTERNAL_AGENT_HOME".to_string(),
                    isolation.root.display().to_string(),
                ),
                (
                    "CODEWITH_HOME".to_string(),
                    isolation.home.display().to_string(),
                ),
                (
                    "CODEX_HOME".to_string(),
                    isolation.home.display().to_string()
                ),
                ("HOME".to_string(), isolation.home.display().to_string()),
                (
                    "LOCALAPPDATA".to_string(),
                    isolation.data_home.display().to_string(),
                ),
                ("TEMP".to_string(), isolation.temp_dir.display().to_string()),
                ("TMP".to_string(), isolation.temp_dir.display().to_string()),
                (
                    "TMPDIR".to_string(),
                    isolation.temp_dir.display().to_string()
                ),
                (
                    "USERPROFILE".to_string(),
                    isolation.home.display().to_string(),
                ),
                (
                    "XDG_CACHE_HOME".to_string(),
                    isolation.cache_home.display().to_string(),
                ),
                (
                    "XDG_CONFIG_HOME".to_string(),
                    isolation.config_home.display().to_string(),
                ),
                (
                    "XDG_DATA_HOME".to_string(),
                    isolation.data_home.display().to_string(),
                ),
            ])
        );
        assert!(
            isolation
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("fake_runtime-"))
        );

        std::fs::remove_dir_all(&isolation.root)
            .unwrap_or_else(|err| panic!("cleanup isolation: {err}"));
    }

    #[tokio::test]
    async fn runtime_run_requires_sandboxed_entry_point() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let request = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            "/tmp",
            ExternalAgentMode::Plan,
        );

        let err = harness
            .run(request, NoopHost)
            .await
            .expect_err("generic run should fail closed");

        assert_eq!(
            err.to_string(),
            "external agent runtime `grok-build` is not ready: ACP runtimes must be executed with AcpStdioHarness::run_sandboxed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_program_uses_supplied_source_env_path() {
        use std::os::unix::fs::PermissionsExt;

        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|err| panic!("create bin dir: {err}"));
        let grok = bin_dir.join("grok");
        std::fs::write(&grok, "#!/bin/sh\nexit 0\n")
            .unwrap_or_else(|err| panic!("write fake grok: {err}"));
        let mut permissions = std::fs::metadata(&grok)
            .unwrap_or_else(|err| panic!("metadata: {err}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&grok, permissions)
            .unwrap_or_else(|err| panic!("set executable: {err}"));
        let request = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let source_env =
            BTreeMap::from([("PATH".to_string(), bin_dir.to_string_lossy().into_owned())]);

        assert_eq!(
            harness
                .resolve_program(&request, &source_env)
                .expect("fake grok should resolve from source env PATH"),
            grok
        );
    }

    #[tokio::test]
    async fn acp_process_includes_bounded_stderr_in_exit_errors() {
        let Some(python) = which::which("python3").ok() else {
            return;
        };
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_crash.py");
        std::fs::write(
            &script,
            r#"
import sys

sys.stdin.readline()
sys.stderr.write("auth missing\n")
sys.stderr.flush()
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from("fake"),
            program: python,
            args: vec![script.display().to_string()],
            arg0: None,
            cwd: temp_dir.path().to_path_buf(),
            env: BTreeMap::from([("PATH".to_string(), path)]),
            isolation: ExternalAgentLaunchIsolation::test_only_unenforced(),
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        let err = process
            .request(
                "initialize",
                initialize_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .expect_err("crash should be surfaced");

        assert_eq!(
            err.to_string(),
            "external agent runtime `fake` protocol error: ACP process exited; stderr: auth missing"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_shutdown_kills_process_group_children() {
        let Some(python) = which::which("python3").ok() else {
            return;
        };
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_with_child.py");
        let marker = temp_dir.path().join("grandchild-marker");
        let marker_literal = serde_json::to_string(&marker.display().to_string())
            .unwrap_or_else(|err| panic!("encode marker path: {err}"));
        let child_code = format!(
            "import pathlib, time; time.sleep(0.5); pathlib.Path({marker_literal}).write_text('leaked')"
        );
        let child_code_literal = serde_json::to_string(&child_code)
            .unwrap_or_else(|err| panic!("encode child code: {err}"));
        std::fs::write(
            &script,
            format!(
                r#"
import pathlib
import subprocess
import sys
import time

marker = pathlib.Path({marker_literal})
subprocess.Popen([sys.executable, "-c", {child_code_literal}])
print("ready", flush=True)
time.sleep(30)
"#
            ),
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from("fake"),
            program: python,
            args: vec![script.display().to_string()],
            arg0: None,
            cwd: temp_dir.path().to_path_buf(),
            env: BTreeMap::from([("PATH".to_string(), path)]),
            isolation: ExternalAgentLaunchIsolation::test_only_unenforced(),
        };
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));
        let ready = tokio::time::timeout(Duration::from_secs(5), process.stdout.next_line())
            .await
            .unwrap_or_else(|err| panic!("wait ready: {err}"))
            .unwrap_or_else(|err| panic!("read ready: {err}"));

        assert_eq!(ready.as_deref(), Some("ready"));

        process.shutdown().await;
        tokio::time::sleep(Duration::from_secs(1)).await;

        assert!(
            !marker.exists(),
            "shutdown should kill the ACP grandchild before it writes the marker"
        );
    }

    #[derive(Clone, Copy)]
    struct NoopHost;

    impl ExternalAgentHost for NoopHost {
        async fn emit(&self, _event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
            Ok(())
        }

        async fn request_permission(
            &self,
            _request: ExternalAgentPermissionRequest,
        ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
            Ok(ExternalAgentPermissionDecision::RejectOnce)
        }

        async fn perform_action(
            &self,
            _action: ExternalAgentActionRequest,
        ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
            Ok(ExternalAgentActionResult::Rejected {
                reason: "not available".to_string(),
            })
        }

        async fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[derive(Clone, Copy)]
    struct CancelledHost;

    impl ExternalAgentHost for CancelledHost {
        async fn emit(&self, _event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
            Ok(())
        }

        async fn request_permission(
            &self,
            _request: ExternalAgentPermissionRequest,
        ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
            Ok(ExternalAgentPermissionDecision::RejectOnce)
        }

        async fn perform_action(
            &self,
            _action: ExternalAgentActionRequest,
        ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
            Ok(ExternalAgentActionResult::Rejected {
                reason: "cancelled".to_string(),
            })
        }

        async fn is_cancelled(&self) -> bool {
            true
        }
    }

    #[derive(Clone, Default)]
    struct EventRecordingHost {
        events: Arc<Mutex<Vec<ExternalAgentEvent>>>,
    }

    impl EventRecordingHost {
        fn output_text(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap_or_else(|err| panic!("events lock: {err}"))
                .iter()
                .filter_map(|event| match event {
                    ExternalAgentEvent::OutputTextDelta { text } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    impl ExternalAgentHost for EventRecordingHost {
        async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
            self.events
                .lock()
                .unwrap_or_else(|err| panic!("events lock: {err}"))
                .push(event);
            Ok(())
        }

        async fn request_permission(
            &self,
            _request: ExternalAgentPermissionRequest,
        ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
            Ok(ExternalAgentPermissionDecision::RejectOnce)
        }

        async fn perform_action(
            &self,
            _action: ExternalAgentActionRequest,
        ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
            Ok(ExternalAgentActionResult::Rejected {
                reason: "not available".to_string(),
            })
        }

        async fn is_cancelled(&self) -> bool {
            false
        }
    }

    fn python_launch(script: &Path, cwd: &Path) -> Option<ExternalAgentLaunchSpec> {
        let python = which::which("python3").ok()?;
        let path = std::env::var("PATH").unwrap_or_default();
        Some(ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from("fake"),
            program: python,
            args: vec![script.display().to_string()],
            arg0: None,
            cwd: cwd.to_path_buf(),
            env: BTreeMap::from([("PATH".to_string(), path)]),
            isolation: ExternalAgentLaunchIsolation::test_only_unenforced(),
        })
    }

    #[tokio::test]
    async fn acp_process_errors_unsupported_server_requests() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_unsupported.py");
        std::fs::write(
            &script,
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "fake-session"}})
    elif method == "session/prompt":
        send({
            "jsonrpc": "2.0",
            "id": "srv-unsupported",
            "method": "workspace/apply_edit",
            "params": {"sessionId": "fake-session", "path": "README.md"}
        })
        response = json.loads(sys.stdin.readline())
        error = response.get("error", {})
        if error.get("code") == -32601 and "unsupported ACP server request" in error.get("message", ""):
            send({"jsonrpc": "2.0", "id": request_id, "result": {"unsupportedFailedClosed": True}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "error": {"message": json.dumps(response)}})
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let Some(launch) = python_launch(&script, temp_dir.path()) else {
            return;
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        process
            .request(
                "initialize",
                initialize_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("initialize: {err}"));
        let session_result = process
            .request(
                "session/new",
                session_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/new: {err}"));
        let session = session_state_from_result(&request, &session_result)
            .unwrap_or_else(|err| panic!("session state: {err}"));
        let result = process
            .request(
                "session/prompt",
                json!({
                    "sessionId": "fake-session",
                    "prompt": [{"type": "text", "text": "inspect README"}],
                }),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: Some(&session),
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/prompt: {err}"));

        assert_eq!(result, json!({ "unsupportedFailedClosed": true }));
        process.shutdown().await;
    }

    #[tokio::test]
    async fn acp_process_filters_replayed_session_updates() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_replay.py");
        std::fs::write(
            &script,
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def update(session_id, text):
    send({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text}
            }
        }
    })

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "fake-session"}})
    elif method == "session/prompt":
        update("old-session", "stale")
        update("fake-session", "current")
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let Some(launch) = python_launch(&script, temp_dir.path()) else {
            return;
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let host = EventRecordingHost::default();
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        process
            .request(
                "initialize",
                initialize_params(&request),
                &host,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("initialize: {err}"));
        let session_result = process
            .request(
                "session/new",
                session_params(&request),
                &host,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/new: {err}"));
        let session = session_state_from_result(&request, &session_result)
            .unwrap_or_else(|err| panic!("session state: {err}"));
        process
            .request(
                "session/prompt",
                json!({
                    "sessionId": "fake-session",
                    "prompt": [{"type": "text", "text": "inspect README"}],
                }),
                &host,
                AcpRequestContext {
                    request: &request,
                    session: Some(&session),
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/prompt: {err}"));

        assert_eq!(host.output_text(), vec!["current".to_string()]);
        process.shutdown().await;
    }

    #[tokio::test]
    async fn acp_process_rejects_server_requests_without_active_session_id() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_missing_session_request.py");
        std::fs::write(
            &script,
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "fake-session"}})
    elif method == "session/prompt":
        send({
            "jsonrpc": "2.0",
            "id": "srv-missing-session",
            "method": "fs/read_text_file",
            "params": {"path": "README.md"}
        })
        response = json.loads(sys.stdin.readline())
        error = response.get("error", {})
        if error.get("code") == -32050:
            send({"jsonrpc": "2.0", "id": request_id, "result": {"missingSessionRejected": True}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "error": {"message": json.dumps(response)}})
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let Some(launch) = python_launch(&script, temp_dir.path()) else {
            return;
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Propose,
        );
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        process
            .request(
                "initialize",
                initialize_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("initialize: {err}"));
        let session_result = process
            .request(
                "session/new",
                session_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/new: {err}"));
        let session = session_state_from_result(&request, &session_result)
            .unwrap_or_else(|err| panic!("session state: {err}"));
        let result = process
            .request(
                "session/prompt",
                json!({
                    "sessionId": "fake-session",
                    "prompt": [{"type": "text", "text": "inspect README"}],
                }),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: Some(&session),
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/prompt: {err}"));

        assert_eq!(result, json!({ "missingSessionRejected": true }));
        process.shutdown().await;
    }

    #[tokio::test]
    async fn acp_process_errors_malformed_json_rpc() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_malformed.py");
        std::fs::write(
            &script,
            r#"
import sys

sys.stdin.readline()
sys.stdout.write("{not-json}\n")
sys.stdout.flush()
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let Some(launch) = python_launch(&script, temp_dir.path()) else {
            return;
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        let err = process
            .request(
                "initialize",
                initialize_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .expect_err("malformed JSON should fail");

        assert!(
            err.to_string().contains("invalid JSON-RPC line"),
            "unexpected error: {err}"
        );
        process.shutdown().await;
    }

    #[tokio::test]
    async fn acp_process_cancels_before_response() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_hangs.py");
        std::fs::write(
            &script,
            r#"
import sys
import time

sys.stdin.readline()
time.sleep(30)
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let Some(launch) = python_launch(&script, temp_dir.path()) else {
            return;
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        let err = process
            .request(
                "initialize",
                initialize_params(&request),
                &CancelledHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .expect_err("cancelled host should stop request");

        assert!(matches!(err, ExternalAgentError::Cancelled));
    }

    #[tokio::test]
    async fn acp_process_surfaces_session_load_failures() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp_load_failure.py");
        std::fs::write(
            &script,
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/load":
        send({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32000, "message": "missing session"}
        })
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let Some(launch) = python_launch(&script, temp_dir.path()) else {
            return;
        };
        let mut request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        request.session = ExternalAgentSessionRequest::Resume {
            external_session_id: "missing".to_string(),
        };
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        process
            .request(
                "initialize",
                initialize_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("initialize: {err}"));
        let err = process
            .request(
                session_method(&request.session),
                session_params(&request),
                &NoopHost,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .expect_err("session/load should fail");

        assert!(
            err.to_string().contains("missing session"),
            "unexpected error: {err}"
        );
        process.shutdown().await;
    }

    #[test]
    fn adapter_constructors_use_acp_stdio_harnesses() {
        let Some(cursor) = cursor_acp_harness() else {
            panic!("cursor harness");
        };
        let Some(grok_build) = grok_build_acp_harness() else {
            panic!("grok-build harness");
        };

        assert_eq!(cursor.harness_kind(), ExternalAgentHarnessKind::AcpStdio);
        assert_eq!(
            grok_build.harness_kind(),
            ExternalAgentHarnessKind::AcpStdio
        );
        assert_eq!(cursor.descriptor().command.program, "cursor-agent");
        assert_eq!(grok_build.descriptor().command.args, ["agent", "stdio"]);
    }

    #[test]
    fn acp_harness_rejects_unsupported_modes_and_capability_overrides() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let managed = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            "/tmp",
            ExternalAgentMode::Managed,
        );

        let err = harness
            .validate_request(&managed)
            .expect_err("managed mode is not supported for Grok Build MVP");
        assert_eq!(
            err.to_string(),
            "external agent runtime `grok-build` failed: mode `Managed` is not supported by this runtime"
        );

        let mut inconsistent = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            "/tmp",
            ExternalAgentMode::Plan,
        );
        inconsistent.capabilities =
            crate::ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose);

        let err = harness
            .validate_request(&inconsistent)
            .expect_err("capabilities must not be caller-widened");
        assert_eq!(
            err.to_string(),
            "external agent runtime `grok-build` failed: request capabilities must match the selected external-agent mode"
        );
    }

    #[test]
    fn confined_path_rejects_parent_traversal() {
        let cwd = Path::new("/repo/project");

        assert_eq!(
            confine_path(cwd, Path::new("src/lib.rs")),
            Ok(PathBuf::from("/repo/project/src/lib.rs"))
        );
        assert_eq!(
            confine_path(cwd, Path::new("../secret.txt")),
            Err("path `/repo/secret.txt` is outside cwd `/repo/project`".to_string())
        );
    }

    #[test]
    fn terminal_action_rejects_cwd_outside_request_cwd() {
        let request = ExternalAgentRequest::new(
            "fake",
            "run tests",
            "/repo/project",
            ExternalAgentMode::Managed,
        );

        assert_eq!(
            terminal_action(
                &request,
                &json!({
                    "command": "cargo test",
                    "cwd": "/repo/other",
                }),
            ),
            Err("path `/repo/other` is outside cwd `/repo/project`".to_string())
        );
    }

    #[tokio::test]
    async fn acp_process_handles_session_updates_and_host_file_reads() {
        #[derive(Clone, Default)]
        struct RecordingHost {
            events: Arc<Mutex<Vec<ExternalAgentEvent>>>,
            actions: Arc<Mutex<Vec<ExternalAgentActionRequest>>>,
        }

        impl RecordingHost {
            fn events(&self) -> Vec<ExternalAgentEvent> {
                self.events
                    .lock()
                    .unwrap_or_else(|err| panic!("events lock: {err}"))
                    .clone()
            }

            fn actions(&self) -> Vec<ExternalAgentActionRequest> {
                self.actions
                    .lock()
                    .unwrap_or_else(|err| panic!("actions lock: {err}"))
                    .clone()
            }
        }

        impl ExternalAgentHost for RecordingHost {
            async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
                self.events
                    .lock()
                    .unwrap_or_else(|err| panic!("events lock: {err}"))
                    .push(event);
                Ok(())
            }

            async fn request_permission(
                &self,
                _request: ExternalAgentPermissionRequest,
            ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
                Ok(ExternalAgentPermissionDecision::AllowOnce)
            }

            async fn perform_action(
                &self,
                action: ExternalAgentActionRequest,
            ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
                self.actions
                    .lock()
                    .unwrap_or_else(|err| panic!("actions lock: {err}"))
                    .push(action);
                Ok(ExternalAgentActionResult::FileContent {
                    content: "host file contents".to_string(),
                })
            }

            async fn is_cancelled(&self) -> bool {
                false
            }
        }

        let Some(python) = which::which("python3").ok() else {
            return;
        };
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let script = temp_dir.path().join("fake_acp.py");
        std::fs::write(
            &script,
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "hello "}
                }
            }
        })
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "fake-session"}})
    elif method == "session/prompt":
        send({
            "jsonrpc": "2.0",
            "id": "srv-1",
            "method": "fs/read_text_file",
            "params": {"sessionId": "fake-session", "path": "README.md"}
        })
        json.loads(sys.stdin.readline())
        send({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "fake-session",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "done"}
                }
            }
        })
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
    else:
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
"#,
        )
        .unwrap_or_else(|err| panic!("write fake acp: {err}"));
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from("fake"),
            program: python,
            args: vec![script.display().to_string()],
            arg0: None,
            cwd: temp_dir.path().to_path_buf(),
            env: BTreeMap::from([("PATH".to_string(), path)]),
            isolation: ExternalAgentLaunchIsolation::test_only_unenforced(),
        };
        let request = ExternalAgentRequest::new(
            "fake",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Propose,
        );
        let host = RecordingHost::default();
        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .unwrap_or_else(|err| panic!("spawn fake acp: {err}"));

        process
            .request(
                "initialize",
                initialize_params(&request),
                &host,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("initialize: {err}"));
        let session_result = process
            .request(
                "session/new",
                session_params(&request),
                &host,
                AcpRequestContext {
                    request: &request,
                    session: None,
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/new: {err}"));
        let session = session_state_from_result(&request, &session_result)
            .unwrap_or_else(|err| panic!("session state: {err}"));
        process
            .request(
                "session/prompt",
                json!({
                    "sessionId": "fake-session",
                    "prompt": [{"type": "text", "text": "inspect README"}],
                }),
                &host,
                AcpRequestContext {
                    request: &request,
                    session: Some(&session),
                },
            )
            .await
            .unwrap_or_else(|err| panic!("session/prompt: {err}"));

        assert_eq!(process.summary(), Some("hello done".to_string()));
        assert_eq!(
            host.actions(),
            vec![ExternalAgentActionRequest::ReadFile {
                path: temp_dir.path().join("README.md"),
            }]
        );
        assert_eq!(
            host.events()
                .into_iter()
                .filter_map(|event| match event {
                    ExternalAgentEvent::OutputTextDelta { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec!["hello ".to_string(), "done".to_string()]
        );
        process.shutdown().await;
    }
}
