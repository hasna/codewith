use super::thread_processor::ThreadRequestProcessor;
use super::*;
use codex_app_server_protocol::ThreadExternalAgentEvent;
use codex_app_server_protocol::ThreadExternalAgentEventNotification;
use codex_app_server_protocol::ThreadExternalAgentPermissionOption;
use codex_app_server_protocol::ThreadExternalAgentPermissionRequest;
use codex_external_agent::AcpStdioHarness;
use codex_external_agent::ClaudeCodeHarness;
use codex_external_agent::ExternalAgentActionRequest;
use codex_external_agent::ExternalAgentActionResult;
use codex_external_agent::ExternalAgentError;
use codex_external_agent::ExternalAgentEvent;
use codex_external_agent::ExternalAgentHost;
use codex_external_agent::ExternalAgentMode;
use codex_external_agent::ExternalAgentPermissionDecision;
use codex_external_agent::ExternalAgentPermissionOption;
use codex_external_agent::ExternalAgentPermissionRequest;
use codex_external_agent::ExternalAgentReadiness;
use codex_external_agent::ExternalAgentReadinessStatus;
use codex_external_agent::ExternalAgentRequest;
use codex_external_agent::ExternalAgentRuntimeId;
use codex_external_agent::ExternalAgentSandboxConfig;
use codex_external_agent::claude_code_harness;
use codex_external_agent::cursor_acp_harness;
use codex_external_agent::grok_build_acp_harness;
use codex_login::AuthProfileSubscriptionProvider;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;

impl ThreadRequestProcessor {
    pub(crate) async fn thread_external_agent_start(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadExternalAgentStartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        match self.thread_external_agent_start_inner(params).await? {
            ExternalAgentStartOutcome::Started(run) => {
                let run = *run;
                self.outgoing
                    .send_response(request_id, run.response.clone())
                    .await;
                self.background_tasks.spawn(async move {
                    run.execute().await;
                });
            }
            ExternalAgentStartOutcome::Gated(response) => {
                self.outgoing.send_response(request_id, response).await;
            }
        }
        Ok(None)
    }

    pub(crate) async fn thread_external_agent_cancel(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadExternalAgentCancelParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let response = self.thread_external_agent_cancel_inner(params).await?;
        self.outgoing.send_response(request_id, response).await;
        Ok(None)
    }

    async fn thread_external_agent_start_inner(
        &self,
        params: ThreadExternalAgentStartParams,
    ) -> Result<ExternalAgentStartOutcome, JSONRPCErrorError> {
        let ThreadExternalAgentStartParams {
            thread_id,
            runtime_id,
            task,
            mode,
        } = params;
        let runtime_id = runtime_id.trim();
        let task = task.trim();
        if runtime_id.is_empty() {
            return Ok(ExternalAgentStartOutcome::gated(
                "runtimeId must not be empty",
            ));
        }
        if runtime_id == "grok" {
            return Ok(ExternalAgentStartOutcome::gated(
                "use runtimeId `grok-build` for Grok Build external-agent runs",
            ));
        }
        if !matches!(runtime_id, "cursor" | "grok-build" | "claude") {
            return Ok(ExternalAgentStartOutcome::gated(format!(
                "unsupported external-agent runtime `{runtime_id}`"
            )));
        }
        if task.is_empty() {
            return Ok(ExternalAgentStartOutcome::gated("task must not be empty"));
        }
        if let Err(message) = validate_external_agent_subscription_profile(
            &self.config.codex_home,
            self.config.selected_auth_profile.as_deref(),
            runtime_id,
        ) {
            return Ok(ExternalAgentStartOutcome::gated(message));
        }
        match mode {
            ThreadExternalAgentMode::Plan | ThreadExternalAgentMode::Propose => {}
        }
        let (thread_id, _) = self.load_thread(&thread_id).await?;

        let run_id = format!("ext_{}", Uuid::new_v4());
        let runtime_mode = external_agent_mode(mode);
        let runtime_request = ExternalAgentRequest::new(
            runtime_id,
            task,
            self.config.cwd.to_path_buf(),
            runtime_mode,
        );
        let runner = runner_for_runtime(runtime_id).ok_or_else(|| {
            invalid_request(format!("unsupported external-agent runtime `{runtime_id}`"))
        })?;
        let source_env = external_agent_source_env(
            &self.config.permissions.shell_environment_policy,
            runtime_id,
        );
        let readiness = runner.readiness_with_env(&source_env).await;
        if readiness.status != ExternalAgentReadinessStatus::Ready {
            return Ok(ExternalAgentStartOutcome::gated(readiness_gate_message(
                readiness,
            )));
        }
        let permission_profile = external_agent_permission_profile(
            &self.config.cwd,
            &self.config.workspace_roots,
            runtime_id,
            &source_env,
        );
        let sandbox_config = ExternalAgentSandboxConfig {
            use_legacy_landlock: external_agent_use_legacy_landlock(
                &permission_profile,
                self.config.cwd.as_path(),
            ),
            permission_profile,
            codex_linux_sandbox_exe: self.arg0_paths.codex_linux_sandbox_exe.clone(),
            windows_sandbox_level: WindowsSandboxLevel::from_config(&self.config),
            windows_sandbox_private_desktop: self
                .config
                .permissions
                .windows_sandbox_private_desktop,
        };
        let response = ThreadExternalAgentStartResponse {
            status: ThreadExternalAgentStartStatus::Started,
            run_id: Some(run_id),
            message: "external-agent run started".to_string(),
        };
        let run_id = response.run_id.clone().unwrap_or_default();
        let thread_id_string = thread_id.to_string();
        let run_key = external_agent_run_key(&thread_id_string, &run_id);
        let cancellation_token = CancellationToken::new();
        self.external_agent_runs
            .lock()
            .await
            .insert(run_key.clone(), cancellation_token.clone());

        Ok(ExternalAgentStartOutcome::Started(Box::new(
            ExternalAgentRun {
                runtime_id: runtime_id.to_string(),
                mode,
                task: runtime_request.task.clone(),
                request: runtime_request,
                runner,
                sandbox_config,
                source_env,
                host: AppServerExternalAgentHost::new(
                    self.outgoing.clone(),
                    thread_id_string,
                    run_id,
                    cancellation_token,
                ),
                run_registry: self.external_agent_runs.clone(),
                run_key,
                response,
            },
        )))
    }

    async fn thread_external_agent_cancel_inner(
        &self,
        params: ThreadExternalAgentCancelParams,
    ) -> Result<ThreadExternalAgentCancelResponse, JSONRPCErrorError> {
        let ThreadExternalAgentCancelParams { thread_id, run_id } = params;
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Ok(ThreadExternalAgentCancelResponse {
                cancelled: false,
                message: "runId must not be empty".to_string(),
            });
        }
        let (thread_id, _) = self.load_thread(&thread_id).await?;
        let run_key = external_agent_run_key(&thread_id.to_string(), run_id);
        let token = self.external_agent_runs.lock().await.get(&run_key).cloned();
        match token {
            Some(token) => {
                token.cancel();
                Ok(ThreadExternalAgentCancelResponse {
                    cancelled: true,
                    message: "external-agent run cancellation requested".to_string(),
                })
            }
            None => Ok(ThreadExternalAgentCancelResponse {
                cancelled: false,
                message: format!("external-agent run `{run_id}` is not active"),
            }),
        }
    }
}

enum ExternalAgentStartOutcome {
    Started(Box<ExternalAgentRun>),
    Gated(ThreadExternalAgentStartResponse),
}

impl ExternalAgentStartOutcome {
    fn gated(message: impl Into<String>) -> Self {
        Self::Gated(ThreadExternalAgentStartResponse {
            status: ThreadExternalAgentStartStatus::Gated,
            run_id: None,
            message: message.into(),
        })
    }
}

fn external_agent_permission_profile(
    cwd: &AbsolutePathBuf,
    workspace_roots: &[AbsolutePathBuf],
    runtime_id: &str,
    source_env: &BTreeMap<String, String>,
) -> PermissionProfile {
    let readable_roots =
        external_agent_readable_roots(cwd, workspace_roots, runtime_id, source_env);
    let file_system = FileSystemSandboxPolicy::restricted(
        readable_roots
            .into_iter()
            .map(|path| FileSystemSandboxEntry {
                path: FileSystemPath::Path { path },
                access: FileSystemAccessMode::Read,
            })
            .collect(),
    );
    PermissionProfile::from_runtime_permissions(&file_system, NetworkSandboxPolicy::Enabled)
}

fn external_agent_readable_roots(
    cwd: &AbsolutePathBuf,
    workspace_roots: &[AbsolutePathBuf],
    runtime_id: &str,
    source_env: &BTreeMap<String, String>,
) -> Vec<AbsolutePathBuf> {
    let mut roots = Vec::new();
    if workspace_roots.is_empty() {
        push_readable_root(&mut roots, cwd.as_path());
    } else {
        for root in workspace_roots {
            push_readable_root(&mut roots, root.as_path());
        }
    }
    add_runtime_config_read_roots(&mut roots, runtime_id, source_env);
    roots
}

fn add_runtime_config_read_roots(
    roots: &mut Vec<AbsolutePathBuf>,
    runtime_id: &str,
    source_env: &BTreeMap<String, String>,
) {
    match runtime_id {
        ExternalAgentRuntimeId::CLAUDE => {
            if let Some(path) = source_env.get("CLAUDE_CONFIG_DIR") {
                push_readable_root(roots, Path::new(path));
            }
            push_home_child_read_root(roots, source_env, ".claude");
            push_xdg_child_read_root(roots, source_env, "XDG_CONFIG_HOME", "claude");
            push_xdg_child_read_root(roots, source_env, "XDG_STATE_HOME", "claude");
            push_xdg_child_read_root(roots, source_env, "XDG_DATA_HOME", "claude");
            push_xdg_child_read_root(roots, source_env, "XDG_CACHE_HOME", "claude");
            push_xdg_child_read_root(roots, source_env, "APPDATA", "Claude");
            push_xdg_child_read_root(roots, source_env, "LOCALAPPDATA", "Claude");
        }
        ExternalAgentRuntimeId::GROK_BUILD => {
            push_home_child_read_root(roots, source_env, ".grok");
            push_xdg_child_read_root(roots, source_env, "XDG_CONFIG_HOME", "grok");
            push_xdg_child_read_root(roots, source_env, "XDG_STATE_HOME", "grok");
            push_xdg_child_read_root(roots, source_env, "XDG_DATA_HOME", "grok");
            push_xdg_child_read_root(roots, source_env, "XDG_CACHE_HOME", "grok");
        }
        ExternalAgentRuntimeId::CURSOR => {
            push_home_child_read_root(roots, source_env, ".cursor");
            push_home_child_read_root(roots, source_env, ".cursor-agent");
            push_xdg_child_read_root(roots, source_env, "XDG_CONFIG_HOME", "cursor");
            push_xdg_child_read_root(roots, source_env, "XDG_STATE_HOME", "cursor");
            push_xdg_child_read_root(roots, source_env, "XDG_DATA_HOME", "cursor");
            push_xdg_child_read_root(roots, source_env, "XDG_CACHE_HOME", "cursor");
        }
        _ => {}
    }
}

fn push_home_child_read_root(
    roots: &mut Vec<AbsolutePathBuf>,
    source_env: &BTreeMap<String, String>,
    child: &str,
) {
    if let Some(home) = source_env
        .get("HOME")
        .or_else(|| source_env.get("USERPROFILE"))
    {
        push_readable_root(roots, PathBuf::from(home).join(child));
    }
}

fn push_xdg_child_read_root(
    roots: &mut Vec<AbsolutePathBuf>,
    source_env: &BTreeMap<String, String>,
    env_name: &str,
    child: &str,
) {
    if let Some(root) = source_env.get(env_name) {
        push_readable_root(roots, PathBuf::from(root).join(child));
    }
}

fn push_readable_root(roots: &mut Vec<AbsolutePathBuf>, path: impl AsRef<Path>) {
    let path = path.as_ref();
    if !path.is_absolute() {
        return;
    }
    let Ok(path) = AbsolutePathBuf::from_absolute_path(path) else {
        return;
    };
    if !roots.iter().any(|existing| existing == &path) {
        roots.push(path);
    }
}

fn external_agent_run_key(thread_id: &str, run_id: &str) -> String {
    format!("{thread_id}:{run_id}")
}

fn external_agent_use_legacy_landlock(permission_profile: &PermissionProfile, cwd: &Path) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let (file_system, network) = permission_profile.to_runtime_permissions();
    !file_system.needs_direct_runtime_enforcement(network, cwd)
}

const COMMON_STABLE_CONFIG_ENV_VARS: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
];

const CLAUDE_STABLE_CONFIG_ENV_VARS: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "CLAUDE_CONFIG_DIR",
];

fn external_agent_source_env(
    policy: &codex_protocol::config_types::ShellEnvironmentPolicy,
    runtime_id: &str,
) -> BTreeMap<String, String> {
    let mut source_env = create_env(policy, /*thread_id*/ None)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    add_provider_stable_config_env(runtime_id, &mut source_env, std::env::vars());
    source_env
}

fn add_provider_stable_config_env<I>(
    runtime_id: &str,
    source_env: &mut BTreeMap<String, String>,
    process_env: I,
) where
    I: IntoIterator<Item = (String, String)>,
{
    let names = match runtime_id {
        ExternalAgentRuntimeId::CLAUDE => CLAUDE_STABLE_CONFIG_ENV_VARS,
        ExternalAgentRuntimeId::CURSOR | ExternalAgentRuntimeId::GROK_BUILD => {
            COMMON_STABLE_CONFIG_ENV_VARS
        }
        _ => return,
    };
    let process_env = process_env.into_iter().collect::<BTreeMap<_, _>>();
    for name in names {
        if !source_env.contains_key(*name)
            && let Some(value) = process_env.get(*name)
        {
            source_env.insert((*name).to_string(), value.clone());
        }
    }
}

fn validate_external_agent_subscription_profile(
    codex_home: &Path,
    selected_auth_profile: Option<&str>,
    runtime_id: &str,
) -> Result<(), String> {
    let Some(required_provider) = subscription_provider_for_runtime(runtime_id) else {
        return Ok(());
    };
    let Some(profile_name) = selected_auth_profile else {
        if required_provider != AuthProfileSubscriptionProvider::ClaudeAi {
            return Ok(());
        }
        return Err(format!(
            "external-agent runtime `{runtime_id}` requires an active {} auth profile",
            required_provider.label()
        ));
    };
    let metadata = codex_login::load_auth_profile_metadata(codex_home, profile_name)
        .map_err(|err| format!("failed to load auth profile `{profile_name}`: {err}"))?;
    if metadata.subscription_provider == required_provider
        || (required_provider != AuthProfileSubscriptionProvider::ClaudeAi
            && metadata.subscription_provider == AuthProfileSubscriptionProvider::ChatGpt)
    {
        return Ok(());
    }
    if required_provider == AuthProfileSubscriptionProvider::ClaudeAi {
        return Err(format!(
            "external-agent runtime `{runtime_id}` requires an active {} auth profile, but `{profile_name}` is tied to {}",
            required_provider.label(),
            metadata.subscription_provider.label(),
        ));
    }
    Err(format!(
        "external-agent runtime `{runtime_id}` requires an active {} auth profile or a ChatGPT profile, but `{profile_name}` is tied to {}",
        required_provider.label(),
        metadata.subscription_provider.label(),
    ))
}

fn subscription_provider_for_runtime(runtime_id: &str) -> Option<AuthProfileSubscriptionProvider> {
    match runtime_id {
        ExternalAgentRuntimeId::CURSOR => Some(AuthProfileSubscriptionProvider::Cursor),
        ExternalAgentRuntimeId::GROK_BUILD => Some(AuthProfileSubscriptionProvider::Grok),
        ExternalAgentRuntimeId::CLAUDE => Some(AuthProfileSubscriptionProvider::ClaudeAi),
        _ => None,
    }
}

struct ExternalAgentRun {
    runtime_id: String,
    mode: ThreadExternalAgentMode,
    task: String,
    request: ExternalAgentRequest,
    runner: ExternalAgentRunner,
    sandbox_config: ExternalAgentSandboxConfig,
    source_env: BTreeMap<String, String>,
    host: AppServerExternalAgentHost,
    run_registry: Arc<Mutex<HashMap<String, CancellationToken>>>,
    run_key: String,
    response: ThreadExternalAgentStartResponse,
}

impl ExternalAgentRun {
    async fn execute(self) {
        self.host
            .emit(ThreadExternalAgentEvent::RunStarted {
                runtime_id: self.runtime_id,
                mode: self.mode,
                task: self.task,
            })
            .await;

        let result = self
            .runner
            .run_sandboxed_with_env(
                self.request,
                self.host.clone(),
                &self.sandbox_config,
                self.source_env,
            )
            .await;

        if let Err(err) = result
            && !self.host.terminal_sent()
        {
            self.host
                .emit(ThreadExternalAgentEvent::Failed {
                    message: err.to_string(),
                })
                .await;
        }
        self.run_registry.lock().await.remove(&self.run_key);
    }
}

enum ExternalAgentRunner {
    Acp(AcpStdioHarness),
    Claude(ClaudeCodeHarness),
}

impl ExternalAgentRunner {
    async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        match self {
            Self::Acp(harness) => harness.readiness_with_env(source_env).await,
            Self::Claude(harness) => harness.readiness_with_env(source_env).await,
        }
    }

    async fn run_sandboxed_with_env(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
        source_env: BTreeMap<String, String>,
    ) -> Result<codex_external_agent::ExternalAgentResult, ExternalAgentError> {
        match self {
            Self::Acp(harness) => {
                harness
                    .run_sandboxed_with_env(request, host, sandbox_config, source_env)
                    .await
            }
            Self::Claude(harness) => {
                harness
                    .run_sandboxed_with_env(request, host, sandbox_config, source_env)
                    .await
            }
        }
    }
}

fn readiness_gate_message(readiness: ExternalAgentReadiness) -> String {
    let status = match readiness.status {
        ExternalAgentReadinessStatus::Ready => "ready",
        ExternalAgentReadinessStatus::MissingRuntime => "missing runtime",
        ExternalAgentReadinessStatus::MissingAuth => "missing auth",
        ExternalAgentReadinessStatus::Unsupported => "unsupported",
        ExternalAgentReadinessStatus::Disabled => "disabled",
    };
    match readiness.detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!(
                "{} external-agent runtime is gated: {status}. {detail}",
                readiness.display_name
            )
        }
        _ => format!(
            "{} external-agent runtime is gated: {status}",
            readiness.display_name
        ),
    }
}

#[derive(Clone)]
struct AppServerExternalAgentHost {
    outgoing: Arc<OutgoingMessageSender>,
    thread_id: String,
    run_id: String,
    terminal_sent: Arc<AtomicBool>,
    cancellation_token: CancellationToken,
}

impl AppServerExternalAgentHost {
    fn new(
        outgoing: Arc<OutgoingMessageSender>,
        thread_id: String,
        run_id: String,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            outgoing,
            thread_id,
            run_id,
            terminal_sent: Arc::new(AtomicBool::new(false)),
            cancellation_token,
        }
    }

    async fn emit(&self, event: ThreadExternalAgentEvent) {
        if matches!(
            event,
            ThreadExternalAgentEvent::Completed { .. }
                | ThreadExternalAgentEvent::Failed { .. }
                | ThreadExternalAgentEvent::Cancelled { .. }
        ) {
            self.terminal_sent.store(true, Ordering::SeqCst);
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadExternalAgentEvent(
                ThreadExternalAgentEventNotification {
                    thread_id: self.thread_id.clone(),
                    run_id: self.run_id.clone(),
                    event,
                },
            ))
            .await;
    }

    fn terminal_sent(&self) -> bool {
        self.terminal_sent.load(Ordering::SeqCst)
    }
}

impl ExternalAgentHost for AppServerExternalAgentHost {
    async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
        self.emit(api_external_agent_event(event)).await;
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
        action: ExternalAgentActionRequest,
    ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
        self.emit(ThreadExternalAgentEvent::ProposedAction {
            proposal: action_json(&action),
        })
        .await;
        Ok(ExternalAgentActionResult::Rejected {
            reason: "external-agent managed action routing is not enabled yet".to_string(),
        })
    }

    async fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}

fn runner_for_runtime(runtime_id: &str) -> Option<ExternalAgentRunner> {
    match runtime_id {
        ExternalAgentRuntimeId::CURSOR => cursor_acp_harness().map(ExternalAgentRunner::Acp),
        ExternalAgentRuntimeId::GROK_BUILD => {
            grok_build_acp_harness().map(ExternalAgentRunner::Acp)
        }
        ExternalAgentRuntimeId::CLAUDE => claude_code_harness().map(ExternalAgentRunner::Claude),
        _ => None,
    }
}

fn external_agent_mode(mode: ThreadExternalAgentMode) -> ExternalAgentMode {
    match mode {
        ThreadExternalAgentMode::Plan => ExternalAgentMode::Plan,
        ThreadExternalAgentMode::Propose => ExternalAgentMode::Propose,
    }
}

fn api_external_agent_event(event: ExternalAgentEvent) -> ThreadExternalAgentEvent {
    match event {
        ExternalAgentEvent::RunStarted { session } => ThreadExternalAgentEvent::SessionResolved {
            external_session_id: session.external_session_id,
        },
        ExternalAgentEvent::SessionResolved { session } => {
            ThreadExternalAgentEvent::SessionResolved {
                external_session_id: session.external_session_id,
            }
        }
        ExternalAgentEvent::OutputTextDelta { text } => {
            ThreadExternalAgentEvent::OutputTextDelta { text }
        }
        ExternalAgentEvent::ReasoningDelta { text } => {
            ThreadExternalAgentEvent::ReasoningDelta { text }
        }
        ExternalAgentEvent::PlanUpdated { plan } => ThreadExternalAgentEvent::PlanUpdated { plan },
        ExternalAgentEvent::PermissionRequested { request } => {
            ThreadExternalAgentEvent::PermissionRequested {
                request: ThreadExternalAgentPermissionRequest {
                    id: request.id,
                    action: action_json(&request.action),
                    options: request
                        .options
                        .into_iter()
                        .map(api_permission_option)
                        .collect(),
                },
            }
        }
        ExternalAgentEvent::ProposedAction { proposal } => {
            ThreadExternalAgentEvent::ProposedAction {
                proposal: action_json(&proposal),
            }
        }
        ExternalAgentEvent::Artifact { artifact } => ThreadExternalAgentEvent::Status {
            message: format!("Artifact: {}", artifact.label),
        },
        ExternalAgentEvent::Status { message } => ThreadExternalAgentEvent::Status { message },
        ExternalAgentEvent::Completed { result } => ThreadExternalAgentEvent::Completed {
            summary: result.summary,
        },
        ExternalAgentEvent::Failed { message } => ThreadExternalAgentEvent::Failed { message },
        ExternalAgentEvent::Cancelled { reason } => ThreadExternalAgentEvent::Cancelled { reason },
    }
}

fn api_permission_option(
    option: ExternalAgentPermissionOption,
) -> ThreadExternalAgentPermissionOption {
    match option {
        ExternalAgentPermissionOption::AllowOnce => ThreadExternalAgentPermissionOption::AllowOnce,
        ExternalAgentPermissionOption::RejectOnce => {
            ThreadExternalAgentPermissionOption::RejectOnce
        }
    }
}

fn action_json(action: &ExternalAgentActionRequest) -> JsonValue {
    serde_json::to_value(action).unwrap_or_else(|err| {
        serde_json::json!({
            "type": "serialization-error",
            "message": err.to_string(),
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;

    #[test]
    fn external_agent_permission_profile_scopes_read_roots_with_network() {
        let cwd = tempfile::TempDir::new().expect("cwd tempdir");
        let workspace = tempfile::TempDir::new().expect("workspace tempdir");
        let home = tempfile::TempDir::new().expect("home tempdir");
        let cwd = AbsolutePathBuf::from_absolute_path(cwd.path()).expect("absolute cwd");
        let workspace =
            AbsolutePathBuf::from_absolute_path(workspace.path()).expect("absolute workspace");
        let claude_config = home.path().join(".claude");
        let source_env = BTreeMap::from([
            ("HOME".to_string(), home.path().display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                claude_config.display().to_string(),
            ),
        ]);

        let profile = external_agent_permission_profile(
            &cwd,
            std::slice::from_ref(&workspace),
            ExternalAgentRuntimeId::CLAUDE,
            &source_env,
        );
        let (file_system, network) = profile.to_runtime_permissions();
        let read_paths = file_system
            .entries
            .iter()
            .filter_map(|entry| match &entry.path {
                FileSystemPath::Path { path } if entry.access == FileSystemAccessMode::Read => {
                    Some(path.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(network, NetworkSandboxPolicy::Enabled);
        assert!(
            !file_system
                .entries
                .iter()
                .any(|entry| matches!(entry.path, FileSystemPath::Special { .. })),
            "external-agent profile must not grant special root reads"
        );
        assert!(read_paths.contains(&workspace));
        assert!(read_paths.contains(
            &AbsolutePathBuf::from_absolute_path(claude_config).expect("absolute claude config")
        ));
    }

    #[test]
    fn external_agent_disables_legacy_landlock_for_direct_enforcement_profiles() {
        let cwd = tempfile::TempDir::new().expect("tempdir");
        let read_only_child =
            AbsolutePathBuf::from_absolute_path(cwd.path().join("read-only-child"))
                .expect("absolute read-only child");
        let file_system = codex_protocol::protocol::FileSystemSandboxPolicy::restricted(vec![
            codex_protocol::protocol::FileSystemSandboxEntry {
                path: codex_protocol::protocol::FileSystemPath::Special {
                    value: codex_protocol::protocol::FileSystemSpecialPath::Root,
                },
                access: codex_protocol::protocol::FileSystemAccessMode::Write,
            },
            codex_protocol::protocol::FileSystemSandboxEntry {
                path: codex_protocol::protocol::FileSystemPath::Path {
                    path: read_only_child,
                },
                access: codex_protocol::protocol::FileSystemAccessMode::Read,
            },
        ]);
        let profile = PermissionProfile::from_runtime_permissions(
            &file_system,
            NetworkSandboxPolicy::Restricted,
        );
        let (file_system, network) = profile.to_runtime_permissions();

        assert!(file_system.needs_direct_runtime_enforcement(network, cwd.path()));
        assert!(!external_agent_use_legacy_landlock(&profile, cwd.path()));
    }

    #[test]
    fn claude_runtime_uses_claude_runner() {
        let Some(runner) = runner_for_runtime(ExternalAgentRuntimeId::CLAUDE) else {
            panic!("claude runner");
        };

        assert!(matches!(runner, ExternalAgentRunner::Claude(_)));
    }

    #[test]
    fn claude_source_env_preserves_stable_config_paths_without_provider_keys() {
        let mut source_env = BTreeMap::from([("PATH".to_string(), "/bin".to_string())]);

        add_provider_stable_config_env(
            ExternalAgentRuntimeId::CLAUDE,
            &mut source_env,
            [
                ("HOME".to_string(), "/home/alex".to_string()),
                (
                    "XDG_CONFIG_HOME".to_string(),
                    "/home/alex/.config".to_string(),
                ),
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    "/home/alex/.claude".to_string(),
                ),
                ("ANTHROPIC_API_KEY".to_string(), "secret".to_string()),
                ("XAI_API_KEY".to_string(), "secret".to_string()),
                ("CURSOR_API_KEY".to_string(), "secret".to_string()),
                ("OPENAI_API_KEY".to_string(), "secret".to_string()),
            ],
        );

        assert_eq!(
            source_env,
            BTreeMap::from([
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    "/home/alex/.claude".to_string()
                ),
                ("HOME".to_string(), "/home/alex".to_string()),
                ("PATH".to_string(), "/bin".to_string()),
                (
                    "XDG_CONFIG_HOME".to_string(),
                    "/home/alex/.config".to_string()
                ),
            ])
        );
    }

    #[test]
    fn provider_config_env_does_not_override_shell_policy_values() {
        let mut source_env = BTreeMap::from([("HOME".to_string(), "/custom-home".to_string())]);

        add_provider_stable_config_env(
            ExternalAgentRuntimeId::CLAUDE,
            &mut source_env,
            [("HOME".to_string(), "/real-home".to_string())],
        );

        assert_eq!(
            source_env,
            BTreeMap::from([("HOME".to_string(), "/custom-home".to_string())])
        );
    }

    #[test]
    fn external_agent_runtime_requires_matching_subscription_profile() {
        let codex_home = tempfile::TempDir::new().expect("tempdir");
        codex_login::save_auth_profile_metadata(
            codex_home.path(),
            "claude-work",
            codex_login::AuthProfileMetadata {
                subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
                last_permissions: None,
            },
        )
        .expect("save claude profile");
        codex_login::save_auth_profile_metadata(
            codex_home.path(),
            "cursor-work",
            codex_login::AuthProfileMetadata {
                subscription_provider: AuthProfileSubscriptionProvider::Cursor,
                last_permissions: None,
            },
        )
        .expect("save cursor profile");
        codex_login::save_auth_profile_metadata(
            codex_home.path(),
            "grok-work",
            codex_login::AuthProfileMetadata {
                subscription_provider: AuthProfileSubscriptionProvider::Grok,
                last_permissions: None,
            },
        )
        .expect("save grok profile");
        codex_login::save_auth_profile_metadata(
            codex_home.path(),
            "chatgpt-work",
            codex_login::AuthProfileMetadata {
                subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
                last_permissions: None,
            },
        )
        .expect("save chatgpt profile");

        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("claude-work"),
                ExternalAgentRuntimeId::CLAUDE,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("cursor-work"),
                ExternalAgentRuntimeId::CURSOR,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("grok-work"),
                ExternalAgentRuntimeId::GROK_BUILD,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("chatgpt-work"),
                ExternalAgentRuntimeId::CURSOR,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("chatgpt-work"),
                ExternalAgentRuntimeId::GROK_BUILD,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                /*selected_auth_profile*/ None,
                ExternalAgentRuntimeId::CURSOR,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                /*selected_auth_profile*/ None,
                ExternalAgentRuntimeId::GROK_BUILD,
            )
            .is_ok()
        );
        assert_eq!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("cursor-work"),
                ExternalAgentRuntimeId::CLAUDE,
            )
            .expect_err("mismatch should fail"),
            "external-agent runtime `claude` requires an active Claude.ai auth profile, but `cursor-work` is tied to Cursor"
        );
        assert_eq!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                /*selected_auth_profile*/ None,
                ExternalAgentRuntimeId::CLAUDE,
            )
            .expect_err("missing profile should fail"),
            "external-agent runtime `claude` requires an active Claude.ai auth profile"
        );
        assert_eq!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("grok-work"),
                ExternalAgentRuntimeId::CURSOR,
            )
            .expect_err("mismatch should fail"),
            "external-agent runtime `cursor` requires an active Cursor auth profile or a ChatGPT profile, but `grok-work` is tied to Grok"
        );
    }
}
