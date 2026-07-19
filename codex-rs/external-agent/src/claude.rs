use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;
use std::time::Duration;

use crate::EnvironmentCapability;
use crate::ExternalAgentError;
use crate::ExternalAgentEvent;
use crate::ExternalAgentHarness;
use crate::ExternalAgentHarnessKind;
use crate::ExternalAgentHost;
use crate::ExternalAgentLaunchIsolation;
use crate::ExternalAgentLaunchSpec;
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
use crate::ExternalAgentSessionState;
use crate::find_external_agent_runtime;
use crate::platform_sandbox_external_agent_launch;
#[cfg(windows)]
use crate::windows_command::WindowsBatchLaunchError;
#[cfg(windows)]
use crate::windows_command::merge_windows_environment;
#[cfg(windows)]
use crate::windows_command::prepare_windows_batch_launch_from_source_env;
#[cfg(windows)]
use crate::windows_command::resolve_windows_program_from_source_env;
use serde_json::Value as JsonValue;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;
use tokio::task::JoinHandle;

const CLAUDE_SAFE_ENV_VARS: &[&str] = &[
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "PATH",
    "TERM",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "CLAUDE_CONFIG_DIR",
];
const CLAUDE_AGENT_SDK_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "CLAUDE_CODE_SKIP_MANTLE_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
];
const CLAUDE_AWS_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_AWS_BASE_URL",
    "ANTHROPIC_AWS_WORKSPACE_ID",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "ANTHROPIC_BEDROCK_SERVICE_TIER",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_STS_REGIONAL_ENDPOINTS",
];
const CLAUDE_VERTEX_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GOOGLE_PROJECT",
    "GCLOUD_PROJECT",
];
const CLAUDE_FOUNDRY_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_RESOURCE",
];
const CLAUDE_READ_ONLY_TOOLS: &[&str] = &["Read", "Glob", "Grep"];
const CLAUDE_CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CLAUDE_STDERR_MAX_BYTES: usize = 64 * 1024;

/// Environment policy for Claude Code subprocess launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeEnvironmentPolicy {
    inherited_vars: Vec<String>,
}

impl ClaudeEnvironmentPolicy {
    pub fn sanitized() -> Self {
        let inherited_vars = CLAUDE_SAFE_ENV_VARS
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();
        #[cfg(windows)]
        let inherited_vars = {
            let mut inherited_vars = inherited_vars;
            inherited_vars.extend(
                ["PATHEXT", "COMSPEC", "SYSTEMROOT"]
                    .into_iter()
                    .map(std::string::ToString::to_string),
            );
            inherited_vars
        };
        Self { inherited_vars }
    }

    pub fn sanitize(
        &self,
        source: &BTreeMap<String, String>,
        extra: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        #[cfg(windows)]
        let source = merge_windows_environment(source, extra);
        #[cfg(windows)]
        let source = &source;
        let mut env = BTreeMap::new();
        for name in &self.inherited_vars {
            if let Some(value) = source_env_value(source, name) {
                env.insert(name.clone(), value.clone());
            }
        }
        #[cfg(not(windows))]
        for (name, value) in extra {
            env.insert(name.clone(), value.clone());
        }
        #[cfg(windows)]
        for (name, value) in extra {
            env.insert(name.to_ascii_uppercase(), value.clone());
        }
        env
    }
}

fn source_env_value<'a>(
    source_env: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    #[cfg(windows)]
    {
        source_env
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .next_back()
            .map(|(_, value)| value)
    }
    #[cfg(not(windows))]
    {
        source_env.get(name)
    }
}

impl Default for ClaudeEnvironmentPolicy {
    fn default() -> Self {
        Self::sanitized()
    }
}

/// Claude Code CLI harness using Claude's documented print-mode stream.
pub struct ClaudeCodeHarness {
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: ClaudeEnvironmentPolicy,
}

impl ClaudeCodeHarness {
    pub fn new(descriptor: &'static ExternalAgentRuntimeDescriptor) -> Self {
        Self {
            descriptor,
            env_policy: ClaudeEnvironmentPolicy::default(),
        }
    }

    pub fn descriptor(&self) -> &'static ExternalAgentRuntimeDescriptor {
        self.descriptor
    }

    pub async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        let program = match self.resolve_program_with_cwd(source_env, Path::new(".")) {
            Ok(program) => program,
            Err(err) => return self.runtime_missing_readiness(err),
        };

        if has_agent_sdk_auth_env(source_env) {
            return self.runtime_ready_readiness(&program);
        }

        let launch = match self.launch_spec_with_args(
            PathBuf::from("."),
            program.clone(),
            source_env,
            vec!["auth".to_string(), "status".to_string()],
        ) {
            Ok(launch) => launch,
            Err(error) => return self.runtime_missing_readiness(error.to_string()),
        };
        match Command::new(&launch.program)
            .args(&launch.args)
            .env_clear()
            .envs(launch.env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(status) if status.success() => self.runtime_ready_readiness(&program),
            Ok(_) => self.runtime_missing_auth_readiness(&program),
            Err(err) => self.runtime_missing_readiness(err.to_string()),
        }
    }

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
        let launch = self.launch_spec_with_args(
            request.cwd.clone(),
            program,
            &source_env,
            claude_code_args(request.task.as_str()),
        )?;
        let launch = platform_sandbox_external_agent_launch(launch, sandbox_config)?;
        self.run_sandboxed_launch(request, host, launch).await
    }

    #[cfg(test)]
    fn launch_spec(
        &self,
        cwd: impl Into<PathBuf>,
        resolved_program: impl Into<PathBuf>,
        source_env: &BTreeMap<String, String>,
    ) -> Result<ExternalAgentLaunchSpec, ExternalAgentError> {
        self.launch_spec_with_args(cwd, resolved_program, source_env, Vec::new())
    }

    fn launch_spec_with_args(
        &self,
        cwd: impl Into<PathBuf>,
        resolved_program: impl Into<PathBuf>,
        source_env: &BTreeMap<String, String>,
        args: Vec<String>,
    ) -> Result<ExternalAgentLaunchSpec, ExternalAgentError> {
        let program = resolved_program.into();
        let env = self.sanitized_env(source_env);
        #[cfg(windows)]
        let (program, args) = prepare_windows_batch_launch_from_source_env(program, args, &env)
            .map_err(|error| invalid_batch_launch_request(self.descriptor.id, error))?;
        Ok(ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from(self.descriptor.id),
            program,
            args,
            arg0: None,
            cwd: cwd.into(),
            env,
            isolation: ExternalAgentLaunchIsolation::unenforced(
                "Claude Code launch has not been wrapped in a Codewith platform sandbox",
            ),
        })
    }

    fn sanitized_env(&self, source_env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let extra_env = BTreeMap::from([(
            "CODEWITH_EXTERNAL_AGENT_RUNTIME".to_string(),
            self.descriptor.id.to_string(),
        )]);
        let mut env = self.env_policy.sanitize(source_env, &extra_env);
        add_agent_sdk_auth_env(&mut env, source_env);
        env
    }

    fn resolve_program(
        &self,
        request: &ExternalAgentRequest,
        source_env: &BTreeMap<String, String>,
    ) -> Result<PathBuf, ExternalAgentError> {
        self.resolve_program_with_cwd(source_env, &request.cwd)
            .map_err(|err| ExternalAgentError::NotReady {
                runtime: request.runtime.as_str().to_string(),
                reason: err,
            })
    }

    fn resolve_program_with_cwd(
        &self,
        source_env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<PathBuf, String> {
        #[cfg(windows)]
        {
            let source_env = merge_windows_environment(source_env, &BTreeMap::new());
            resolve_windows_program_from_source_env(
                self.descriptor.command.program,
                &source_env,
                cwd,
            )
        }
        #[cfg(not(windows))]
        {
            let path = source_env.get("PATH").map(String::as_str);
            which::which_in(self.descriptor.command.program, path, cwd)
                .map_err(|err| err.to_string())
        }
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
        if !matches!(
            request.capabilities.environment,
            EnvironmentCapability::Sanitized
        ) {
            return Err(invalid_request(
                &request.runtime,
                "Claude Code runs require sanitized environment capabilities",
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
        let mut process = ClaudeCodeProcess::spawn(launch, &request)?;
        let result = process.run(request, &host).await;
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

    fn runtime_missing_auth_readiness(&self, program: &Path) -> ExternalAgentReadiness {
        ExternalAgentReadiness {
            runtime: self.id(),
            status: ExternalAgentReadinessStatus::MissingAuth,
            display_name: self.descriptor.display_name.to_string(),
            version: None,
            supported_modes: self.descriptor.supported_modes.to_vec(),
            detail: Some(format!(
                "`{} auth status` reported no active local Claude login and no Claude Agent SDK auth environment was configured",
                program.display()
            )),
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
}

#[cfg(windows)]
fn invalid_batch_launch_request(
    runtime: &str,
    error: WindowsBatchLaunchError,
) -> ExternalAgentError {
    ExternalAgentError::InvalidRequest {
        runtime: runtime.to_string(),
        message: error.to_string(),
    }
}

pub fn claude_code_harness() -> Option<ClaudeCodeHarness> {
    find_external_agent_runtime(ExternalAgentRuntimeId::CLAUDE).map(ClaudeCodeHarness::new)
}

impl ExternalAgentRuntime for ClaudeCodeHarness {
    fn id(&self) -> ExternalAgentRuntimeId {
        ExternalAgentRuntimeId::from(self.descriptor.id)
    }

    async fn readiness(&self) -> ExternalAgentReadiness {
        let source_env = std::env::vars().collect::<BTreeMap<_, _>>();
        self.readiness_with_env(&source_env).await
    }

    async fn run(
        &self,
        request: ExternalAgentRequest,
        _host: impl ExternalAgentHost + Send + Sync,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        self.validate_request(&request)?;
        Err(ExternalAgentError::NotReady {
            runtime: request.runtime.as_str().to_string(),
            reason: "Claude Code must be executed with ClaudeCodeHarness::run_sandboxed"
                .to_string(),
        })
    }
}

impl ExternalAgentHarness for ClaudeCodeHarness {
    fn harness_kind(&self) -> ExternalAgentHarnessKind {
        ExternalAgentHarnessKind::Sdk
    }
}

struct ClaudeCodeProcess {
    runtime: ExternalAgentRuntimeId,
    child: Child,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: Option<JoinHandle<String>>,
    output_text: String,
    summary: Option<String>,
    external_session_id: Option<String>,
}

impl ClaudeCodeProcess {
    fn spawn(
        launch: ExternalAgentSandboxedLaunchSpec,
        _request: &ExternalAgentRequest,
    ) -> Result<Self, ExternalAgentError> {
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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| protocol_error(&runtime, format!("spawn failed: {err}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| protocol_error(&runtime, "missing child stderr"))?;
        let stderr = tokio::spawn(read_bounded_stderr(stderr, CLAUDE_STDERR_MAX_BYTES));

        Ok(Self {
            runtime,
            child,
            stdout: BufReader::new(stdout).lines(),
            stderr: Some(stderr),
            output_text: String::new(),
            summary: None,
            external_session_id: None,
        })
    }

    async fn run<H>(
        &mut self,
        request: ExternalAgentRequest,
        host: &H,
    ) -> Result<ExternalAgentResult, ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let mut session = ExternalAgentSessionState {
            runtime: request.runtime.clone(),
            external_session_id: None,
            mode: request.mode,
            cwd: request.cwd.clone(),
        };
        host.emit(ExternalAgentEvent::RunStarted {
            session: session.clone(),
        })
        .await?;

        loop {
            if host.is_cancelled().await {
                self.shutdown().await;
                return Err(ExternalAgentError::Cancelled);
            }

            let line =
                match tokio::time::timeout(CLAUDE_CANCEL_POLL_INTERVAL, self.stdout.next_line())
                    .await
                {
                    Ok(line) => line.map_err(|err| {
                        protocol_error(&self.runtime, format!("read failed: {err}"))
                    })?,
                    Err(_) => {
                        if self.is_finished()? {
                            break;
                        }
                        continue;
                    }
                };

            let Some(line) = line else {
                break;
            };
            self.handle_stdout_line(&line, &mut session, host).await?;
        }

        let status = self.wait_for_exit().await?;
        if !status.success() {
            return Err(ExternalAgentError::Runtime {
                runtime: self.runtime.as_str().to_string(),
                message: self.take_stderr().await,
            });
        }

        let result = ExternalAgentResult {
            status: ExternalAgentRunStatus::Completed,
            session,
            summary: self.summary.clone().or_else(|| {
                (!self.output_text.trim().is_empty()).then(|| self.output_text.trim().to_string())
            }),
            artifacts: Vec::new(),
        };
        host.emit(ExternalAgentEvent::Completed {
            result: result.clone(),
        })
        .await?;
        Ok(result)
    }

    async fn handle_stdout_line<H>(
        &mut self,
        line: &str,
        session: &mut ExternalAgentSessionState,
        host: &H,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        if let Ok(value) = serde_json::from_str::<JsonValue>(line) {
            validate_claude_system_event(&self.runtime, &value)?;
        }
        for event in claude_stream_events(line) {
            match event {
                ClaudeStreamEvent::Output(text) => {
                    self.output_text.push_str(&text);
                    host.emit(ExternalAgentEvent::OutputTextDelta { text })
                        .await?;
                }
                ClaudeStreamEvent::Reasoning(text) => {
                    host.emit(ExternalAgentEvent::ReasoningDelta { text })
                        .await?;
                }
                ClaudeStreamEvent::SessionId(session_id) => {
                    if self.external_session_id.as_deref() != Some(session_id.as_str()) {
                        self.external_session_id = Some(session_id.clone());
                        session.external_session_id = Some(session_id);
                        host.emit(ExternalAgentEvent::SessionResolved {
                            session: session.clone(),
                        })
                        .await?;
                    }
                }
                ClaudeStreamEvent::Summary(summary) => {
                    self.summary = Some(summary);
                }
            }
        }
        Ok(())
    }

    fn is_finished(&mut self) -> Result<bool, ExternalAgentError> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|err| protocol_error(&self.runtime, format!("wait failed: {err}")))
    }

    async fn wait_for_exit(&mut self) -> Result<ExitStatus, ExternalAgentError> {
        self.child
            .wait()
            .await
            .map_err(|err| protocol_error(&self.runtime, format!("wait failed: {err}")))
    }

    async fn take_stderr(&mut self) -> String {
        let stderr = self
            .stderr
            .take()
            .map(|handle| async move { handle.await.unwrap_or_default() });
        match stderr {
            Some(stderr) => {
                let stderr = stderr.await;
                if stderr.trim().is_empty() {
                    "Claude Code exited unsuccessfully".to_string()
                } else {
                    stderr
                }
            }
            None => "Claude Code exited unsuccessfully".to_string(),
        }
    }

    async fn shutdown(&mut self) {
        let _ = codex_utils_pty::process_group::kill_child_process_group(&mut self.child);
        let _ = self.child.kill().await;
    }
}

fn claude_code_args(task: &str) -> Vec<String> {
    [
        "--safe-mode",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--tools",
        "Read,Glob,Grep",
        "-p",
        task,
        "--permission-mode",
        "plan",
        "--allowedTools",
        "Read,Glob,Grep",
        "--output-format",
        "stream-json",
        "--verbose",
    ]
    .into_iter()
    .map(std::string::ToString::to_string)
    .collect()
}

fn add_agent_sdk_auth_env(
    env: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
) {
    copy_env_vars(env, source_env, CLAUDE_AGENT_SDK_AUTH_ENV_VARS);
    if env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_BEDROCK")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_ANTHROPIC_AWS")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_MANTLE")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_BEDROCK_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_MANTLE_AUTH")
    {
        copy_env_vars(env, source_env, CLAUDE_AWS_AUTH_ENV_VARS);
    }
    if env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_VERTEX")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_VERTEX_AUTH")
    {
        copy_env_vars(env, source_env, CLAUDE_VERTEX_AUTH_ENV_VARS);
    }
    if env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_FOUNDRY")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_FOUNDRY_AUTH")
    {
        copy_env_vars(env, source_env, CLAUDE_FOUNDRY_AUTH_ENV_VARS);
    }
}

fn has_agent_sdk_auth_env(source_env: &BTreeMap<String, String>) -> bool {
    env_value_is_set(source_env, "ANTHROPIC_API_KEY")
        || env_value_is_set(source_env, "ANTHROPIC_AUTH_TOKEN")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_BEDROCK")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_ANTHROPIC_AWS")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_VERTEX")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_FOUNDRY")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_MANTLE")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_BEDROCK_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_FOUNDRY_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_MANTLE_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_VERTEX_AUTH")
}

fn copy_env_vars(
    env: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
    names: &[&str],
) {
    for name in names {
        if let Some(value) = source_env_value(source_env, name)
            && !value.trim().is_empty()
        {
            env.insert((*name).to_string(), value.clone());
        }
    }
}

fn env_value_is_set(source_env: &BTreeMap<String, String>, name: &str) -> bool {
    source_env_value(source_env, name).is_some_and(|value| !value.trim().is_empty())
}

fn env_flag_is_enabled(source_env: &BTreeMap<String, String>, name: &str) -> bool {
    source_env_value(source_env, name).is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn validate_claude_system_event(
    runtime: &ExternalAgentRuntimeId,
    value: &JsonValue,
) -> Result<(), ExternalAgentError> {
    if value.get("type").and_then(JsonValue::as_str) != Some("system")
        || value.get("subtype").and_then(JsonValue::as_str) != Some("init")
    {
        return Ok(());
    }
    validate_claude_init(runtime, value)
}

fn validate_claude_init(
    runtime: &ExternalAgentRuntimeId,
    value: &JsonValue,
) -> Result<(), ExternalAgentError> {
    let tools = json_string_set(runtime, value, "tools")?;
    let allowed_tools = CLAUDE_READ_ONLY_TOOLS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let unsafe_tools = tools
        .iter()
        .filter(|tool| !allowed_tools.contains(tool.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unsafe_tools.is_empty() {
        return Err(protocol_error(
            runtime,
            format!(
                "Claude Code exposed unsafe tools for an external-agent run: {}",
                unsafe_tools.join(", ")
            ),
        ));
    }

    let mcp_servers = value
        .get("mcp_servers")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| protocol_error(runtime, "Claude Code init did not report MCP servers"))?;
    if !mcp_servers.is_empty() {
        return Err(protocol_error(
            runtime,
            "Claude Code exposed MCP servers for an external-agent run",
        ));
    }

    if let Some(permission_mode) = value.get("permissionMode").and_then(JsonValue::as_str)
        && permission_mode != "plan"
        && permission_mode != "bypassPermissions"
    {
        return Err(protocol_error(
            runtime,
            format!("Claude Code reported unsupported permission mode `{permission_mode}`"),
        ));
    }

    for field in ["slash_commands", "skills"] {
        let values = value
            .get(field)
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                protocol_error(runtime, format!("Claude Code init did not report {field}"))
            })?;
        if !values.is_empty() {
            return Err(protocol_error(
                runtime,
                format!("Claude Code exposed {field} for an external-agent run"),
            ));
        }
    }

    Ok(())
}

fn json_string_set(
    runtime: &ExternalAgentRuntimeId,
    value: &JsonValue,
    field: &str,
) -> Result<BTreeSet<String>, ExternalAgentError> {
    let values = value
        .get(field)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            protocol_error(runtime, format!("Claude Code init did not report {field}"))
        })?;
    let mut strings = BTreeSet::new();
    for value in values {
        let Some(value) = value.as_str() else {
            return Err(protocol_error(
                runtime,
                format!("Claude Code init field `{field}` included a non-string value"),
            ));
        };
        strings.insert(value.to_string());
    }
    Ok(strings)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeStreamEvent {
    Output(String),
    Reasoning(String),
    SessionId(String),
    Summary(String),
}

fn claude_stream_events(line: &str) -> Vec<ClaudeStreamEvent> {
    let Ok(value) = serde_json::from_str::<JsonValue>(line) else {
        return vec![ClaudeStreamEvent::Output(format!("{line}\n"))];
    };

    let mut events = Vec::new();
    if let Some(session_id) = value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(JsonValue::as_str)
    {
        events.push(ClaudeStreamEvent::SessionId(session_id.to_string()));
    }

    match value.get("type").and_then(JsonValue::as_str) {
        Some("assistant") => collect_assistant_content(&value, &mut events),
        Some("result") => {
            if let Some(summary) = value.get("result").and_then(JsonValue::as_str)
                && !summary.trim().is_empty()
            {
                events.push(ClaudeStreamEvent::Summary(summary.to_string()));
            }
        }
        Some("system") => {}
        Some(_) | None => {
            if let Some(text) = value.get("text").and_then(JsonValue::as_str) {
                events.push(ClaudeStreamEvent::Output(text.to_string()));
            }
        }
    }
    events
}

fn collect_assistant_content(value: &JsonValue, events: &mut Vec<ClaudeStreamEvent>) {
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(JsonValue::as_array)
    else {
        return;
    };

    for block in content {
        let text = block
            .get("text")
            .or_else(|| block.get("thinking"))
            .and_then(JsonValue::as_str);
        let Some(text) = text else {
            continue;
        };
        match block.get("type").and_then(JsonValue::as_str) {
            Some("thinking") | Some("redacted_thinking") => {
                events.push(ClaudeStreamEvent::Reasoning(text.to_string()));
            }
            _ => events.push(ClaudeStreamEvent::Output(text.to_string())),
        }
    }
}

async fn read_bounded_stderr<R>(reader: R, max_bytes: usize) -> String
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let _ = reader.take(max_bytes as u64).read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).to_string()
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

fn protocol_error(
    runtime: &ExternalAgentRuntimeId,
    message: impl Into<String>,
) -> ExternalAgentError {
    ExternalAgentError::Protocol {
        runtime: runtime.as_str().to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_claude_stream_json_events() {
        let events = claude_stream_events(
            r#"{"type":"assistant","session_id":"sess-1","message":{"content":[{"type":"thinking","thinking":"checking"},{"type":"text","text":"done"}]}}"#,
        );

        assert_eq!(
            events,
            vec![
                ClaudeStreamEvent::SessionId("sess-1".to_string()),
                ClaudeStreamEvent::Reasoning("checking".to_string()),
                ClaudeStreamEvent::Output("done".to_string()),
            ]
        );
    }

    #[test]
    fn parses_claude_result_summary() {
        let events = claude_stream_events(r#"{"type":"result","result":"finished"}"#);

        assert_eq!(
            events,
            vec![ClaudeStreamEvent::Summary("finished".to_string())]
        );
    }

    #[test]
    fn claude_code_args_force_safe_plan_streaming_mode() {
        assert_eq!(
            claude_code_args("inspect this repo"),
            vec![
                "--safe-mode",
                "--disable-slash-commands",
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
                "--tools",
                "Read,Glob,Grep",
                "-p",
                "inspect this repo",
                "--permission-mode",
                "plan",
                "--allowedTools",
                "Read,Glob,Grep",
                "--output-format",
                "stream-json",
                "--verbose",
            ]
        );
    }

    #[test]
    fn validates_claude_init_read_only_surface() {
        validate_claude_init(
            &ExternalAgentRuntimeId::from("claude"),
            &serde_json::json!({
                "type": "system",
                "subtype": "init",
                "tools": ["Glob", "Grep", "Read"],
                "mcp_servers": [],
                "permissionMode": "bypassPermissions",
                "slash_commands": [],
                "skills": [],
            }),
        )
        .expect("read-only init surface is acceptable");
    }

    #[test]
    fn rejects_claude_init_with_write_or_shell_tools() {
        let err = validate_claude_init(
            &ExternalAgentRuntimeId::from("claude"),
            &serde_json::json!({
                "type": "system",
                "subtype": "init",
                "tools": ["Read", "Bash", "Edit"],
                "mcp_servers": [],
                "permissionMode": "bypassPermissions",
                "slash_commands": [],
                "skills": [],
            }),
        )
        .expect_err("unsafe init surface should be rejected");

        assert_eq!(
            err.to_string(),
            "external agent runtime `claude` protocol error: Claude Code exposed unsafe tools for an external-agent run: Bash, Edit"
        );
    }

    #[test]
    fn rejects_claude_init_with_mcp_servers() {
        let err = validate_claude_init(
            &ExternalAgentRuntimeId::from("claude"),
            &serde_json::json!({
                "type": "system",
                "subtype": "init",
                "tools": ["Read"],
                "mcp_servers": [{"name": "github", "status": "ready"}],
                "permissionMode": "plan",
                "slash_commands": [],
                "skills": [],
            }),
        )
        .expect_err("MCP exposure should be rejected");

        assert_eq!(
            err.to_string(),
            "external agent runtime `claude` protocol error: Claude Code exposed MCP servers for an external-agent run"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_accepts_agent_sdk_auth_env_without_local_login() {
        let (_temp_dir, mut env) = fake_claude_env(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  exit 1
fi
exit 2
"#,
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-value".to_string());
        let harness = claude_code_harness().expect("claude harness");

        let readiness = harness.readiness_with_env(&env).await;

        assert_eq!(readiness.status, ExternalAgentReadinessStatus::Ready);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_reports_ready_when_claude_auth_status_succeeds() {
        let (_temp_dir, env) = fake_claude_env(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  exit 0
fi
exit 2
"#,
        );
        let harness = claude_code_harness().expect("claude harness");

        let readiness = harness.readiness_with_env(&env).await;

        assert_eq!(readiness.status, ExternalAgentReadinessStatus::Ready);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_reports_missing_auth_when_claude_auth_status_fails() {
        let (_temp_dir, env) = fake_claude_env(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  exit 1
fi
exit 2
"#,
        );
        let harness = claude_code_harness().expect("claude harness");

        let readiness = harness.readiness_with_env(&env).await;

        assert_eq!(readiness.status, ExternalAgentReadinessStatus::MissingAuth);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn readiness_uses_source_pathext_for_claude_discovery() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let claude_path = bin_dir.join("claude.CLAUDEEXT");
        std::fs::write(&claude_path, "not executed").expect("write fake claude");
        let source_env = BTreeMap::from([
            ("Path".to_string(), bin_dir.display().to_string()),
            ("PathExt".to_string(), ".CLAUDEEXT".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
        ]);
        let harness = claude_code_harness().expect("claude harness");

        let readiness = harness.readiness_with_env(&source_env).await;

        assert_eq!(readiness.status, ExternalAgentReadinessStatus::Ready);
        assert_eq!(readiness.detail, Some(claude_path.display().to_string()));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn claude_cmd_receives_source_pathext_in_sanitized_environment() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let claude_path = bin_dir.join("claude.cmd");
        std::fs::write(&claude_path, "@echo off\r\necho %PATHEXT%\r\nexit /b 0\r\n")
            .expect("write fake claude");
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let source_env = BTreeMap::from([
            ("Path".to_string(), bin_dir.display().to_string()),
            ("PathExt".to_string(), ".CMD".to_string()),
            ("cOmSpEc".to_string(), comspec.clone()),
            ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
        ]);
        let harness = claude_code_harness().expect("claude harness");
        let request = ExternalAgentRequest::new(
            "claude",
            "inspect the environment",
            temp_dir.path(),
            crate::ExternalAgentMode::Plan,
        );
        let launch = harness
            .launch_spec(temp_dir.path(), &claude_path, &source_env)
            .expect("batch launch spec should build");
        assert_eq!(launch.env.get("PATHEXT"), Some(&".CMD".to_string()));
        assert_eq!(launch.env.get("COMSPEC"), Some(&comspec));
        assert_eq!(launch.env.get("SYSTEMROOT"), None);
        assert_eq!(launch.program, PathBuf::from(comspec));
        assert_eq!(launch.args[3], "/c");
        assert!(launch.args[4].contains(claude_path.to_string_lossy().as_ref()));
        let mut process = ClaudeCodeProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
            &request,
        )
        .expect("spawn fake claude");

        assert_eq!(
            process
                .stdout
                .next_line()
                .await
                .expect("read fake claude stdout"),
            Some(".CMD".to_string())
        );
        assert!(
            process
                .wait_for_exit()
                .await
                .expect("wait for fake claude")
                .success()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn claude_cmd_forwards_a_hostile_task_without_executing_it() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let claude_path = bin_dir.join("claude.cmd");
        let capture_path = bin_dir.join("captured-task.txt");
        let marker_path = bin_dir.join("injected.txt");
        std::fs::write(
            &claude_path,
            r#"@echo off
setlocal DisableDelayedExpansion
set "TASK=%~9"
set TASK > "%~dp0captured-task.txt"
exit /b 0
"#,
        )
        .expect("write fake Claude batch shim");
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let source_env = BTreeMap::from([
            ("Path".to_string(), bin_dir.display().to_string()),
            ("PathExt".to_string(), ".CMD".to_string()),
            ("cOmSpEc".to_string(), comspec),
            (
                "ANTHROPIC_API_KEY".to_string(),
                "expanded-in-test".to_string(),
            ),
        ]);
        let task = format!(
            "inspect \" & type nul > \"{}\" & rem | < > ( ) ^ %ANTHROPIC_API_KEY% !",
            marker_path.display()
        );
        let harness = claude_code_harness().expect("claude harness");
        let launch = harness
            .launch_spec_with_args(
                temp_dir.path(),
                &claude_path,
                &source_env,
                claude_code_args(task.as_str()),
            )
            .expect("hostile single-line task should prepare");

        let status = Command::new(&launch.program)
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(&launch.env)
            .status()
            .await
            .expect("launch fake Claude batch shim");
        assert!(status.success());
        assert!(
            !marker_path.exists(),
            "hostile task escaped the Claude batch command line"
        );
        let captured = std::fs::read_to_string(capture_path).expect("read captured task");
        assert_eq!(captured.trim_end(), format!("TASK={task}"));
    }

    #[cfg(windows)]
    #[test]
    fn claude_cmd_rejects_line_break_tasks_before_spawning() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let claude_path = temp_dir.path().join("claude.cmd");
        let marker_path = temp_dir.path().join("injected.txt");
        std::fs::write(&claude_path, "@echo off\r\nexit /b 0\r\n")
            .expect("write fake Claude batch shim");
        let source_env = BTreeMap::from([(
            "COMSPEC".to_string(),
            std::env::var("COMSPEC").expect("Windows supplies COMSPEC"),
        )]);
        let harness = claude_code_harness().expect("claude harness");

        for line_break in ["\r", "\n", "\r\n"] {
            let task = format!(
                "review{line_break}& type nul > \"{}\" & rem",
                marker_path.display()
            );
            let error = harness
                .launch_spec_with_args(
                    temp_dir.path(),
                    &claude_path,
                    &source_env,
                    claude_code_args(task.as_str()),
                )
                .expect_err("line-bearing Claude tasks must be rejected before cmd.exe launches");
            assert!(
                matches!(
                    error,
                    ExternalAgentError::InvalidRequest { ref message, .. }
                        if message.contains("CR or LF")
                ),
                "unexpected launch error: {error}"
            );
            assert!(
                !marker_path.exists(),
                "rejected Claude task must not execute an injected command"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn sanitized_environment_keeps_windows_command_bootstrap_case_insensitively() {
        let source = BTreeMap::from([
            ("Path".to_string(), r"C:\bin".to_string()),
            ("PathExt".to_string(), ".CMD".to_string()),
            (
                "ComSpec".to_string(),
                r"C:\Windows\System32\cmd.exe".to_string(),
            ),
            ("SystemRoot".to_string(), r"C:\Windows".to_string()),
        ]);

        let env = ClaudeEnvironmentPolicy::sanitized().sanitize(&source, &BTreeMap::new());

        assert_eq!(env.get("PATH"), Some(&r"C:\bin".to_string()));
        assert_eq!(env.get("PATHEXT"), Some(&".CMD".to_string()));
        assert_eq!(
            env.get("COMSPEC"),
            Some(&r"C:\Windows\System32\cmd.exe".to_string())
        );
        assert_eq!(env.get("SYSTEMROOT"), Some(&r"C:\Windows".to_string()));
    }

    #[test]
    fn launch_env_preserves_stable_config_and_claude_auth_only() {
        let harness = claude_code_harness().expect("claude harness");
        let source = BTreeMap::from([
            ("HOME".to_string(), "/home/alex".to_string()),
            ("PATH".to_string(), "/bin".to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                "/home/alex/.config".to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                "/home/alex/.local/state".to_string(),
            ),
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                "/home/alex/.claude".to_string(),
            ),
            ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), "test-value".to_string()),
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://api.example.invalid".to_string(),
            ),
            ("CLAUDE_CODE_USE_BEDROCK".to_string(), "1".to_string()),
            (
                "ANTHROPIC_BEDROCK_BASE_URL".to_string(),
                "https://bedrock.example.invalid".to_string(),
            ),
            (
                "ANTHROPIC_BEDROCK_MANTLE_BASE_URL".to_string(),
                "https://mantle.example.invalid".to_string(),
            ),
            (
                "AWS_BEARER_TOKEN_BEDROCK".to_string(),
                "test-value".to_string(),
            ),
            (
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "test-value".to_string(),
            ),
            ("CLAUDE_CODE_USE_ANTHROPIC_AWS".to_string(), "1".to_string()),
            (
                "ANTHROPIC_AWS_API_KEY".to_string(),
                "test-value".to_string(),
            ),
            (
                "ANTHROPIC_AWS_WORKSPACE_ID".to_string(),
                "workspace".to_string(),
            ),
            ("CLAUDE_CODE_USE_VERTEX".to_string(), "1".to_string()),
            (
                "ANTHROPIC_VERTEX_PROJECT_ID".to_string(),
                "project".to_string(),
            ),
            ("CLOUD_ML_REGION".to_string(), "us-east1".to_string()),
            ("CLAUDE_CODE_USE_FOUNDRY".to_string(), "1".to_string()),
            (
                "ANTHROPIC_FOUNDRY_API_KEY".to_string(),
                "test-value".to_string(),
            ),
            (
                "ANTHROPIC_FOUNDRY_RESOURCE".to_string(),
                "resource".to_string(),
            ),
            ("CLAUDE_CODE_USE_MANTLE".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH".to_string(),
                "1".to_string(),
            ),
            ("CLAUDE_CODE_SKIP_BEDROCK_AUTH".to_string(), "1".to_string()),
            ("CLAUDE_CODE_SKIP_FOUNDRY_AUTH".to_string(), "1".to_string()),
            ("CLAUDE_CODE_SKIP_MANTLE_AUTH".to_string(), "1".to_string()),
            ("CLAUDE_CODE_SKIP_VERTEX_AUTH".to_string(), "1".to_string()),
            ("AWS_PROFILE".to_string(), "dev".to_string()),
            (
                "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                "/home/alex/gcp.json".to_string(),
            ),
            ("AZURE_CLIENT_SECRET".to_string(), "test-value".to_string()),
            ("CURSOR_API_KEY".to_string(), "test-value".to_string()),
            ("GROK_API_KEY".to_string(), "test-value".to_string()),
            ("OPENAI_API_KEY".to_string(), "test-value".to_string()),
            ("XAI_API_KEY".to_string(), "test-value".to_string()),
        ]);

        let spec = harness
            .launch_spec("/repo", "/usr/bin/claude", &source)
            .expect("non-batch launch spec should build");

        assert_eq!(
            spec.env,
            BTreeMap::from([
                ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
                ("ANTHROPIC_AUTH_TOKEN".to_string(), "test-value".to_string()),
                (
                    "ANTHROPIC_AWS_API_KEY".to_string(),
                    "test-value".to_string()
                ),
                (
                    "ANTHROPIC_AWS_WORKSPACE_ID".to_string(),
                    "workspace".to_string()
                ),
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "https://api.example.invalid".to_string()
                ),
                (
                    "ANTHROPIC_BEDROCK_BASE_URL".to_string(),
                    "https://bedrock.example.invalid".to_string()
                ),
                (
                    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL".to_string(),
                    "https://mantle.example.invalid".to_string()
                ),
                (
                    "ANTHROPIC_FOUNDRY_API_KEY".to_string(),
                    "test-value".to_string()
                ),
                (
                    "ANTHROPIC_FOUNDRY_RESOURCE".to_string(),
                    "resource".to_string()
                ),
                (
                    "ANTHROPIC_VERTEX_PROJECT_ID".to_string(),
                    "project".to_string()
                ),
                (
                    "AWS_BEARER_TOKEN_BEDROCK".to_string(),
                    "test-value".to_string()
                ),
                (
                    "AWS_SECRET_ACCESS_KEY".to_string(),
                    "test-value".to_string()
                ),
                ("AWS_PROFILE".to_string(), "dev".to_string()),
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    "/home/alex/.claude".to_string()
                ),
                ("CLAUDE_CODE_USE_ANTHROPIC_AWS".to_string(), "1".to_string()),
                ("CLAUDE_CODE_USE_BEDROCK".to_string(), "1".to_string()),
                ("CLAUDE_CODE_USE_FOUNDRY".to_string(), "1".to_string()),
                ("CLAUDE_CODE_USE_MANTLE".to_string(), "1".to_string()),
                ("CLAUDE_CODE_USE_VERTEX".to_string(), "1".to_string()),
                (
                    "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH".to_string(),
                    "1".to_string()
                ),
                ("CLAUDE_CODE_SKIP_BEDROCK_AUTH".to_string(), "1".to_string()),
                ("CLAUDE_CODE_SKIP_FOUNDRY_AUTH".to_string(), "1".to_string()),
                ("CLAUDE_CODE_SKIP_MANTLE_AUTH".to_string(), "1".to_string()),
                ("CLAUDE_CODE_SKIP_VERTEX_AUTH".to_string(), "1".to_string()),
                ("CLOUD_ML_REGION".to_string(), "us-east1".to_string()),
                (
                    "CODEWITH_EXTERNAL_AGENT_RUNTIME".to_string(),
                    "claude".to_string()
                ),
                ("HOME".to_string(), "/home/alex".to_string()),
                ("PATH".to_string(), "/bin".to_string()),
                (
                    "XDG_CONFIG_HOME".to_string(),
                    "/home/alex/.config".to_string()
                ),
                (
                    "XDG_STATE_HOME".to_string(),
                    "/home/alex/.local/state".to_string()
                ),
            ])
        );
    }

    #[cfg(unix)]
    fn fake_claude_env(script: &str) -> (tempfile::TempDir, BTreeMap<String, String>) {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let claude_path = bin_dir.join("claude");
        std::fs::write(&claude_path, script).expect("write fake claude");
        let mut permissions = std::fs::metadata(&claude_path)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&claude_path, permissions).expect("chmod fake claude");
        let path = bin_dir.display().to_string();
        (temp_dir, BTreeMap::from([("PATH".to_string(), path)]))
    }
}
