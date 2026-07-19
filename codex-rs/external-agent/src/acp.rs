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
#[cfg(windows)]
use crate::windows_command::WindowsBatchLaunchError;
#[cfg(windows)]
use crate::windows_command::configure_windows_batch_launch;
#[cfg(windows)]
use crate::windows_command::merge_windows_environment;
#[cfg(windows)]
use crate::windows_command::prepare_windows_batch_launch_from_source_env;
#[cfg(windows)]
use crate::windows_command::resolve_windows_program_from_source_env;
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
#[cfg(windows)]
const WINDOWS_COMMAND_ENV_VARS: &[&str] = &["PATHEXT", "COMSPEC", "SYSTEMROOT"];
const CURSOR_AUTH_ENV_VARS: &[&str] = &["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"];
const GROK_BUILD_AUTH_ENV_VARS: &[&str] = &["XAI_API_KEY"];
const ACP_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const ACP_CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(1);
const ACP_READINESS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const ACP_STDERR_MAX_BYTES: usize = 64 * 1024;
static ACP_ISOLATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Environment policy for ACP subprocess launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpEnvironmentPolicy {
    inherited_vars: Vec<String>,
}

impl AcpEnvironmentPolicy {
    pub fn sanitized() -> Self {
        let inherited_vars = SAFE_ENV_VARS
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();
        #[cfg(windows)]
        let inherited_vars = {
            let mut inherited_vars = inherited_vars;
            inherited_vars.extend(
                WINDOWS_COMMAND_ENV_VARS
                    .iter()
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
            .rfind(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    }
    #[cfg(not(windows))]
    {
        source_env.get(name)
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
    ) -> Result<ExternalAgentLaunchSpec, ExternalAgentError> {
        let program = resolved_program.into();
        let env = self.env_policy.sanitize(source_env, extra_env);
        let args = self
            .descriptor
            .command
            .args
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        #[cfg(windows)]
        let (program, args) = prepare_windows_batch_launch_from_source_env(program, args, &env)
            .map_err(|err| invalid_batch_launch_request(self.descriptor.id, err))?;
        Ok(ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from(self.descriptor.id),
            program,
            args,
            arg0: None,
            cwd: cwd.into(),
            env,
            isolation: ExternalAgentLaunchIsolation::unenforced(
                "external-agent ACP launch has not been wrapped in a Codewith platform sandbox",
            ),
        })
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

    pub async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let program = match self.resolve_program_with_cwd(source_env, cwd.as_path()) {
            Ok(program) => program,
            Err(err) => return self.runtime_missing_readiness(err),
        };
        if self.descriptor.id == ExternalAgentRuntimeId::CURSOR
            && let Err(message) = self.probe_cursor_runtime(&program, source_env).await
        {
            return self.runtime_missing_readiness(message);
        }
        self.runtime_ready_readiness(&program)
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
        copy_acp_runtime_auth_env(&mut extra_env, &source_env, request.runtime.as_str());
        let launch = self.launch_spec(request.cwd.clone(), program, &source_env, &extra_env)?;
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
        let source_env = merge_windows_environment(source_env, &BTreeMap::new());
        #[cfg(not(windows))]
        let path = source_env_value(source_env, "PATH").map(String::as_str);
        let mut last_error = None;
        for program in acp_program_candidates(self.descriptor) {
            #[cfg(windows)]
            let resolved = resolve_windows_program_from_source_env(program, &source_env, cwd);
            #[cfg(not(windows))]
            let resolved = which::which_in(program, path, cwd).map_err(|err| err.to_string());
            match resolved {
                Ok(program) => return Ok(program),
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            format!(
                "could not resolve external-agent command `{}`",
                self.descriptor.command.program
            )
        }))
    }

    async fn probe_cursor_runtime(
        &self,
        program: &Path,
        source_env: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let env = self
            .env_policy
            .sanitize(source_env, &BTreeMap::<String, String>::new());
        let mut args = self
            .descriptor
            .command
            .args
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();
        args.push("--help".to_string());
        #[cfg(windows)]
        let (program, args) =
            prepare_windows_batch_launch_from_source_env(program.to_path_buf(), args, &env)
                .map_err(|err| err.to_string())?;
        #[cfg(not(windows))]
        let program = program.to_path_buf();
        let mut command = Command::new(program);
        #[cfg(windows)]
        configure_windows_batch_launch(&mut command, &args);
        #[cfg(not(windows))]
        command.args(args);
        command
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = match tokio::time::timeout(ACP_READINESS_PROBE_TIMEOUT, command.output()).await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(format!("Cursor ACP readiness probe failed: {err}")),
            Err(_) => return Ok(()),
        };
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            Err("Cursor ACP readiness probe exited unsuccessfully".to_string())
        } else {
            Err(detail.to_string())
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
        let initialize_result = process
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
        process
            .authenticate_if_needed(&initialize_result, request, host)
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
            reason: "ACP runtimes must be executed with AcpStdioHarness::run_sandboxed".to_string(),
        })
    }
}

impl ExternalAgentHarness for AcpStdioHarness {
    fn harness_kind(&self) -> ExternalAgentHarnessKind {
        ExternalAgentHarnessKind::AcpStdio
    }
}

fn acp_runtime_auth_env_vars(runtime_id: &str) -> &'static [&'static str] {
    match runtime_id {
        ExternalAgentRuntimeId::CURSOR => CURSOR_AUTH_ENV_VARS,
        ExternalAgentRuntimeId::GROK_BUILD => GROK_BUILD_AUTH_ENV_VARS,
        _ => &[],
    }
}

fn copy_acp_runtime_auth_env(
    destination: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
    runtime_id: &str,
) {
    #[cfg(windows)]
    let source_env = merge_windows_environment(source_env, &BTreeMap::new());
    #[cfg(windows)]
    let source_env = &source_env;
    for name in acp_runtime_auth_env_vars(runtime_id) {
        if let Some(value) = source_env_value(source_env, name) {
            destination.insert((*name).to_string(), value.clone());
        }
    }
}

fn acp_program_candidates(descriptor: &ExternalAgentRuntimeDescriptor) -> Vec<&'static str> {
    if descriptor.id == ExternalAgentRuntimeId::CURSOR {
        vec![descriptor.command.program, "cursor-agent"]
    } else {
        vec![descriptor.command.program]
    }
}

fn acp_auth_method(
    runtime: &ExternalAgentRuntimeId,
    initialize_result: &JsonValue,
) -> Result<Option<String>, ExternalAgentError> {
    let Some(auth_methods) = initialize_result
        .get("authMethods")
        .and_then(JsonValue::as_array)
    else {
        return Ok(None);
    };
    if auth_methods.is_empty() {
        return Ok(None);
    }
    let ids = auth_methods
        .iter()
        .filter_map(|method| method.get("id").and_then(JsonValue::as_str))
        .collect::<Vec<_>>();
    let preferred = match runtime.as_str() {
        ExternalAgentRuntimeId::CURSOR => &["cursor_login", "cached_token"][..],
        ExternalAgentRuntimeId::GROK_BUILD => &["cached_token", "xai.api_key"][..],
        _ => &["cached_token"][..],
    };
    for method_id in preferred {
        if ids.contains(method_id) {
            return Ok(Some((*method_id).to_string()));
        }
    }
    Err(protocol_error(
        runtime,
        format!("unsupported ACP auth methods: {}", ids.join(", ")),
    ))
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
        #[cfg(windows)]
        configure_windows_batch_launch(&mut command, &launch.args);
        #[cfg(not(windows))]
        command.args(&launch.args);
        command
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

    async fn authenticate_if_needed<H>(
        &mut self,
        initialize_result: &JsonValue,
        request: &ExternalAgentRequest,
        host: &H,
    ) -> Result<(), ExternalAgentError>
    where
        H: ExternalAgentHost + Send + Sync,
    {
        let Some(method_id) = acp_auth_method(&request.runtime, initialize_result)? else {
            return Ok(());
        };
        self.request(
            "authenticate",
            json!({
                "methodId": method_id,
                "_meta": {
                    "headless": true,
                },
            }),
            host,
            AcpRequestContext {
                request,
                session: None,
            },
        )
        .await
        .map(|_| ())
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
                    self.write_error(
                        id,
                        /*code*/ -32050,
                        "ACP server request session mismatch",
                    )
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
                    /*code*/ -32601,
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
                .write_error(
                    id,
                    /*code*/ -32010,
                    "filesystem reads are not allowed for this run",
                )
                .await;
        }
        let path = match confined_path_from_params(context.request, &params, "path") {
            Ok(path) => path,
            Err(message) => return self.write_error(id, /*code*/ -32011, message).await,
        };
        match host
            .perform_action(ExternalAgentActionRequest::ReadFile { path })
            .await?
        {
            ExternalAgentActionResult::FileContent { content } => {
                self.write_result(id, json!({ "content": content })).await
            }
            ExternalAgentActionResult::Rejected { reason } => {
                self.write_error(id, /*code*/ -32012, reason).await
            }
            _ => {
                self.write_error(
                    id,
                    /*code*/ -32013,
                    "host returned an invalid read result",
                )
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
                .write_error(
                    id,
                    /*code*/ -32020,
                    "filesystem writes require managed mode",
                )
                .await;
        }
        let path = match confined_path_from_params(context.request, &params, "path") {
            Ok(path) => path,
            Err(message) => return self.write_error(id, /*code*/ -32021, message).await,
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
            return self
                .write_error(id, /*code*/ -32022, "write rejected")
                .await;
        }
        match host.perform_action(action).await? {
            ExternalAgentActionResult::WriteAccepted => self.write_result(id, json!({})).await,
            ExternalAgentActionResult::Rejected { reason } => {
                self.write_error(id, /*code*/ -32023, reason).await
            }
            _ => {
                self.write_error(
                    id,
                    /*code*/ -32024,
                    "host returned an invalid write result",
                )
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
                .write_error(
                    id,
                    /*code*/ -32030,
                    "terminal access requires managed mode",
                )
                .await;
        }
        let action = match terminal_action(context.request, &params) {
            Ok(action) => action,
            Err(message) => return self.write_error(id, /*code*/ -32034, message).await,
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
                .write_error(id, /*code*/ -32031, "terminal command rejected")
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
                self.write_error(id, /*code*/ -32032, reason).await
            }
            _ => {
                self.write_error(
                    id,
                    /*code*/ -32033,
                    "host returned an invalid terminal result",
                )
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
            return self
                .write_error(id, /*code*/ -32040, "unknown terminal")
                .await;
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
            return self
                .write_error(id, /*code*/ -32041, "unknown terminal")
                .await;
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
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
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

        let env = AcpEnvironmentPolicy::sanitized().sanitize(&source, &BTreeMap::new());

        assert_eq!(env.get("PATH"), Some(&r"C:\bin".to_string()));
        assert_eq!(env.get("PATHEXT"), Some(&".CMD".to_string()));
        assert_eq!(
            env.get("COMSPEC"),
            Some(&r"C:\Windows\System32\cmd.exe".to_string())
        );
        assert_eq!(env.get("SYSTEMROOT"), Some(&r"C:\Windows".to_string()));
    }

    #[test]
    fn launch_spec_uses_canonical_runtime_command_and_sanitized_env() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let source = BTreeMap::from([("PATH".to_string(), "/bin".to_string())]);
        let extra = BTreeMap::new();

        let spec = harness
            .launch_spec("/repo", "/usr/bin/grok", &source, &extra)
            .expect("non-batch launch spec should build");

        assert_eq!(
            spec,
            ExternalAgentLaunchSpec {
                runtime: ExternalAgentRuntimeId::from("grok-build"),
                program: PathBuf::from("/usr/bin/grok"),
                args: vec![
                    "--no-auto-update".to_string(),
                    "agent".to_string(),
                    "stdio".to_string(),
                ],
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

    #[cfg(windows)]
    #[test]
    fn resolve_program_uses_case_insensitive_source_pathext_without_ambient_environment() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let extension = format!(
            ".CODEWITHACP{}",
            temp_dir
                .path()
                .file_name()
                .expect("temporary directory name")
                .to_string_lossy()
                .to_ascii_uppercase()
        );
        let ambient_pathext = std::env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(
            !ambient_pathext
                .split(';')
                .any(|ambient_extension| ambient_extension.eq_ignore_ascii_case(extension.as_str())),
            "the source-only test extension must not be present in ambient PATHEXT"
        );
        let grok = bin_dir.join(format!("grok{extension}"));
        std::fs::write(&grok, "not executed").expect("write fake grok");
        let request = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let source_env = BTreeMap::from([
            ("pAtH".to_string(), bin_dir.display().to_string()),
            ("pAtHeXt".to_string(), format!("{extension};.CMD")),
        ]);

        assert_eq!(
            harness
                .resolve_program(&request, &source_env)
                .expect("source PATHEXT should resolve the supplied command"),
            grok
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_program_uses_source_cmd_pathext_case_insensitively() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let grok = bin_dir.join("grok.CMD");
        std::fs::write(&grok, "not executed").expect("write fake grok");
        let request = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let source_env = BTreeMap::from([
            ("pAtH".to_string(), bin_dir.display().to_string()),
            ("pAtHeXt".to_string(), ".CMD".to_string()),
        ]);

        assert_eq!(
            harness
                .resolve_program(&request, &source_env)
                .expect("source .CMD PATHEXT should resolve the supplied command"),
            grok
        );
    }

    #[cfg(windows)]
    #[test]
    fn runtime_auth_env_lookup_is_case_insensitive() {
        let source_env = BTreeMap::from([
            ("CURSOR_API_KEY".to_string(), "ambient-key".to_string()),
            ("cUrSoR_aPi_KeY".to_string(), "policy-key".to_string()),
            ("CuRsOr_AuTh_ToKeN".to_string(), "cursor-token".to_string()),
        ]);
        let mut destination = BTreeMap::new();

        copy_acp_runtime_auth_env(
            &mut destination,
            &source_env,
            ExternalAgentRuntimeId::CURSOR,
        );

        assert_eq!(
            destination,
            BTreeMap::from([
                ("CURSOR_API_KEY".to_string(), "policy-key".to_string()),
                ("CURSOR_AUTH_TOKEN".to_string(), "cursor-token".to_string(),),
            ])
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_environment_overrides_deduplicate_case_insensitively() {
        let source = BTreeMap::from([
            ("PATH".to_string(), r"C:\ambient-bin".to_string()),
            ("PATHEXT".to_string(), ".EXE".to_string()),
            ("COMSPEC".to_string(), r"C:\ambient\cmd.exe".to_string()),
            ("SYSTEMROOT".to_string(), r"C:\ambient".to_string()),
        ]);
        let overrides = BTreeMap::from([
            ("Path".to_string(), r"C:\policy-bin".to_string()),
            ("PathExt".to_string(), ".CMD".to_string()),
            ("ComSpec".to_string(), r"C:\policy\cmd.exe".to_string()),
            ("SystemRoot".to_string(), r"C:\policy".to_string()),
        ]);

        let environment = AcpEnvironmentPolicy::sanitized().sanitize(&source, &overrides);

        assert_eq!(
            environment,
            BTreeMap::from([
                ("COMSPEC".to_string(), r"C:\policy\cmd.exe".to_string()),
                ("PATH".to_string(), r"C:\policy-bin".to_string()),
                ("PATHEXT".to_string(), ".CMD".to_string()),
                ("SYSTEMROOT".to_string(), r"C:\policy".to_string()),
            ])
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_program_uses_the_case_insensitive_pathext_override() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let grok = bin_dir.join("grok.cmd");
        std::fs::write(&grok, "not executed").expect("write fake grok");
        let request = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let source_env = BTreeMap::from([
            ("PATH".to_string(), bin_dir.display().to_string()),
            ("PATHEXT".to_string(), ".EXE".to_string()),
            ("PathExt".to_string(), ".CMD".to_string()),
        ]);

        assert_eq!(
            harness
                .resolve_program(&request, &source_env)
                .expect("case-insensitive PathExt override should resolve the .cmd shim")
                .to_string_lossy()
                .to_ascii_lowercase(),
            grok.to_string_lossy().to_ascii_lowercase()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn acp_cmd_launches_with_source_comspec_and_source_only_pathext() {
        let Some(descriptor) = find_external_agent_runtime("grok-build") else {
            panic!("grok-build runtime");
        };
        let harness = AcpStdioHarness::new(descriptor);
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).expect("create bin dir");
        let grok = bin_dir.join("grok.cmd");
        std::fs::write(&grok, "@echo off\r\necho batch-launch\r\nexit /b 0\r\n")
            .expect("write fake ACP batch shim");
        let comspec = std::env::var("COMSPEC").expect("Windows supplies COMSPEC");
        let source_env = BTreeMap::from([
            ("pAtH".to_string(), bin_dir.display().to_string()),
            ("pAtHeXt".to_string(), ".CMD".to_string()),
            ("cOmSpEc".to_string(), comspec.clone()),
        ]);
        let request = ExternalAgentRequest::new(
            "grok-build",
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Plan,
        );
        let resolved = harness
            .resolve_program(&request, &source_env)
            .expect("source-only PATH and PATHEXT should resolve the batch shim");
        assert_eq!(
            resolved.to_string_lossy().to_ascii_lowercase(),
            grok.to_string_lossy().to_ascii_lowercase()
        );
        let launch = harness
            .launch_spec(temp_dir.path(), resolved, &source_env, &BTreeMap::new())
            .expect("batch launch spec should build");
        assert_eq!(launch.program, PathBuf::from(comspec));
        assert_eq!(launch.args[3], "/c");
        assert!(
            launch.args[4]
                .to_ascii_lowercase()
                .contains(grok.to_string_lossy().to_ascii_lowercase().as_str())
        );

        let mut process = AcpStdioProcess::spawn(
            ExternalAgentSandboxedLaunchSpec::test_only_unenforced(launch),
        )
        .expect("cmd.exe should launch the ACP batch shim");
        assert_eq!(
            process
                .stdout
                .next_line()
                .await
                .expect("read ACP batch output"),
            Some("batch-launch".to_string())
        );
        assert!(
            process
                .child
                .wait()
                .await
                .expect("wait for ACP batch shim")
                .success()
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

    #[test]
    fn acp_auth_method_selects_known_runtime_methods() {
        let cursor_init = json!({
            "authMethods": [
                {"id": "cursor_login"},
                {"id": "unknown"},
            ],
        });
        let grok_init = json!({
            "authMethods": [
                {"id": "xai.api_key"},
            ],
        });

        assert_eq!(
            acp_auth_method(&ExternalAgentRuntimeId::from("cursor"), &cursor_init)
                .expect("cursor auth"),
            Some("cursor_login".to_string())
        );
        assert_eq!(
            acp_auth_method(&ExternalAgentRuntimeId::from("grok-build"), &grok_init)
                .expect("grok auth"),
            Some("xai.api_key".to_string())
        );
    }

    #[test]
    fn acp_auth_method_rejects_unknown_advertised_methods() {
        let init = json!({
            "authMethods": [
                {"id": "browser_popup"},
            ],
        });

        let err = acp_auth_method(&ExternalAgentRuntimeId::from("cursor"), &init)
            .expect_err("unknown auth method should be rejected");

        assert_eq!(
            err.to_string(),
            "external agent runtime `cursor` protocol error: unsupported ACP auth methods: browser_popup"
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
        assert_eq!(cursor.descriptor().command.program, "agent");
        assert_eq!(cursor.descriptor().command.args, ["acp"]);
        assert_eq!(
            grok_build.descriptor().command.args,
            ["--no-auto-update", "agent", "stdio"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn cursor_resolver_falls_back_to_cursor_agent_wrapper() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|err| panic!("create bin: {err}"));
        let cursor_agent = bin_dir.join("cursor-agent");
        std::fs::write(&cursor_agent, "#!/bin/sh\nexit 0\n")
            .unwrap_or_else(|err| panic!("write cursor-agent: {err}"));
        let mut permissions = std::fs::metadata(&cursor_agent)
            .unwrap_or_else(|err| panic!("metadata cursor-agent: {err}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&cursor_agent, permissions)
            .unwrap_or_else(|err| panic!("chmod cursor-agent: {err}"));
        let harness = cursor_acp_harness().expect("cursor harness");
        let source_env = BTreeMap::from([("PATH".to_string(), bin_dir.display().to_string())]);

        let resolved = harness
            .resolve_program_with_cwd(&source_env, temp_dir.path())
            .expect("resolve cursor fallback");

        assert_eq!(resolved, cursor_agent);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cursor_readiness_rejects_broken_agent_wrapper() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|err| panic!("create bin: {err}"));
        let agent = bin_dir.join("agent");
        std::fs::write(
            &agent,
            "#!/bin/sh\necho 'No Cursor IDE installation found' >&2\nexit 42\n",
        )
        .unwrap_or_else(|err| panic!("write agent: {err}"));
        let mut permissions = std::fs::metadata(&agent)
            .unwrap_or_else(|err| panic!("metadata agent: {err}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&agent, permissions)
            .unwrap_or_else(|err| panic!("chmod agent: {err}"));
        let harness = cursor_acp_harness().expect("cursor harness");
        let source_env = BTreeMap::from([("PATH".to_string(), bin_dir.display().to_string())]);

        let readiness = harness.readiness_with_env(&source_env).await;

        assert_eq!(
            readiness.status,
            ExternalAgentReadinessStatus::MissingRuntime
        );
        assert_eq!(
            readiness.detail,
            Some("No Cursor IDE installation found".to_string())
        );
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
        let cwd = normalize_lexical(Path::new("/repo/project"));
        let sibling_path = normalize_lexical(Path::new("/repo/secret.txt"));

        assert_eq!(
            confine_path(&cwd, Path::new("src/lib.rs")),
            Ok(cwd.join("src/lib.rs"))
        );
        assert_eq!(
            confine_path(&cwd, Path::new("../secret.txt")),
            Err(format!(
                "path `{}` is outside cwd `{}`",
                sibling_path.display(),
                cwd.display()
            ))
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
            Err(format!(
                "path `{}` is outside cwd `{}`",
                normalize_lexical(Path::new("/repo/other")).display(),
                normalize_lexical(Path::new("/repo/project")).display()
            ))
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
