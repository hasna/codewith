use super::thread_processor::ThreadRequestProcessor;
use super::*;
use codex_app_server_protocol::ThreadExternalAgentEvent;
use codex_app_server_protocol::ThreadExternalAgentEventNotification;
use codex_app_server_protocol::ThreadExternalAgentExecutionSurface;
use codex_app_server_protocol::ThreadExternalAgentModel;
use codex_app_server_protocol::ThreadExternalAgentModelsListParams;
use codex_app_server_protocol::ThreadExternalAgentModelsListResponse;
use codex_app_server_protocol::ThreadExternalAgentPermissionOption;
use codex_app_server_protocol::ThreadExternalAgentPermissionRequest;
use codex_external_agent::AcpStdioHarness;
use codex_external_agent::ClaudeCodeHarness;
use codex_external_agent::ExternalAgentActionRequest;
use codex_external_agent::ExternalAgentActionResult;
use codex_external_agent::ExternalAgentCapabilities;
use codex_external_agent::ExternalAgentError;
use codex_external_agent::ExternalAgentEvent;
use codex_external_agent::ExternalAgentExecutionSurface;
use codex_external_agent::ExternalAgentHost;
use codex_external_agent::ExternalAgentMode;
use codex_external_agent::ExternalAgentModelDescriptor;
use codex_external_agent::ExternalAgentPermissionDecision;
use codex_external_agent::ExternalAgentPermissionOption;
use codex_external_agent::ExternalAgentPermissionRequest;
use codex_external_agent::ExternalAgentReadiness;
use codex_external_agent::ExternalAgentReadinessStatus;
use codex_external_agent::ExternalAgentRequest;
use codex_external_agent::ExternalAgentRuntimeDescriptor;
use codex_external_agent::ExternalAgentRuntimeId;
use codex_external_agent::ExternalAgentSandboxConfig;
use codex_external_agent::FileSystemCapability;
use codex_external_agent::McpCapability;
use codex_external_agent::NetworkCapability;
use codex_external_agent::TerminalCapability;
use codex_external_agent::claude_code_harness;
use codex_external_agent::cursor_acp_harness;
use codex_external_agent::find_external_agent_runtime;
use codex_external_agent::grok_build_acp_harness;
use codex_login::AuthProfileSubscriptionProvider;
use codex_protocol::ThreadId;
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

    pub(crate) async fn thread_external_agent_models_list(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadExternalAgentModelsListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let response = self.thread_external_agent_models_list_inner(params).await?;
        self.outgoing.send_response(request_id, response).await;
        Ok(None)
    }

    async fn thread_external_agent_models_list_inner(
        &self,
        params: ThreadExternalAgentModelsListParams,
    ) -> Result<ThreadExternalAgentModelsListResponse, JSONRPCErrorError> {
        let ThreadExternalAgentModelsListParams {
            thread_id,
            runtime_id,
            execution_surface,
        } = params;
        let runtime_id = runtime_id.trim();
        // Resolve the thread so unknown ids surface as an error, matching the
        // other external-agent RPCs.
        let (_thread_id, _thread) = self.load_thread(&thread_id).await?;
        let descriptor = find_external_agent_runtime(runtime_id).ok_or_else(|| {
            invalid_request(format!("unsupported external-agent runtime `{runtime_id}`"))
        })?;
        let surface = execution_surface.map(external_agent_execution_surface);
        Ok(external_agent_models_response(runtime_id, descriptor, surface))
    }

    pub(crate) async fn thread_external_agent_permission_respond(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadExternalAgentPermissionRespondParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let response = self
            .thread_external_agent_permission_respond_inner(params)
            .await?;
        self.outgoing.send_response(request_id, response).await;
        Ok(None)
    }

    async fn thread_external_agent_permission_respond_inner(
        &self,
        params: ThreadExternalAgentPermissionRespondParams,
    ) -> Result<ThreadExternalAgentPermissionRespondResponse, JSONRPCErrorError> {
        let ThreadExternalAgentPermissionRespondParams {
            thread_id,
            run_id,
            request_id,
            decision,
        } = params;
        // Resolve the thread so unknown ids surface as an error, matching the
        // other external-agent RPCs, while unknown *request* ids stay a benign
        // `accepted: false`.
        let (thread_id, _) = self.load_thread(&thread_id).await?;
        let key = external_agent_permission_key(&run_id, &request_id);
        let thread_state = self.thread_state_manager.thread_state(thread_id).await;
        let accepted = {
            let mut state = thread_state.lock().await;
            state.respond_external_agent_permission(&key, decision)
        };
        // Unknown, already-resolved, replayed, or abandoned (waiter dropped)
        // request ids all resolve to `accepted: false` without erroring.
        Ok(ThreadExternalAgentPermissionRespondResponse { accepted })
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
            model,
            execution_surface,
            managed,
        } = params;
        let runtime_id = runtime_id.trim();
        let task = task.trim();
        let requested_model = model.as_deref().map(str::trim).filter(|m| !m.is_empty());
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
        let Some(descriptor) = find_external_agent_runtime(runtime_id) else {
            return Ok(ExternalAgentStartOutcome::gated(format!(
                "unsupported external-agent runtime `{runtime_id}`"
            )));
        };
        if task.is_empty() {
            return Ok(ExternalAgentStartOutcome::gated("task must not be empty"));
        }

        // Resolve the execution surface (requested or the runtime default) and
        // gate anything the runtime does not advertise.
        let surface = match execution_surface {
            Some(surface) => external_agent_execution_surface(surface),
            None => descriptor.default_execution_surface,
        };
        if !descriptor.supports_execution_surface(surface) {
            return Ok(ExternalAgentStartOutcome::gated(format!(
                "external-agent runtime `{runtime_id}` does not support the {} execution surface",
                surface.as_str()
            )));
        }

        // Resolve + validate the requested model against the runtime's advertised
        // models on this surface, falling back to the runtime default.
        let resolved_model = match requested_model {
            Some(model) => match descriptor.find_model(model) {
                Some(descriptor_model)
                    if descriptor_model.execution_surfaces.contains(&surface) =>
                {
                    Some(descriptor_model.id.to_string())
                }
                Some(_) => {
                    return Ok(ExternalAgentStartOutcome::gated(format!(
                        "model `{model}` is not available on the {} execution surface for `{runtime_id}`",
                        surface.as_str()
                    )));
                }
                None => {
                    return Ok(ExternalAgentStartOutcome::gated(format!(
                        "unknown model `{model}` for external-agent runtime `{runtime_id}`"
                    )));
                }
            },
            None => descriptor
                .models_for_surface(surface)
                .next()
                .map(|descriptor_model| descriptor_model.id.to_string()),
        };

        // Resolve the runtime mode, honoring managed requests only for runtimes
        // whose Codewith-mediated action executor has landed.
        let runtime_mode = if managed {
            if !descriptor.supports_mode(ExternalAgentMode::Managed) {
                return Ok(ExternalAgentStartOutcome::gated(format!(
                    "external-agent runtime `{runtime_id}` does not support managed mode"
                )));
            }
            ExternalAgentMode::Managed
        } else {
            external_agent_mode(mode)
        };

        // The hosted (cloud) surface needs the cloud harness, which is not part
        // of this build yet; keep it discoverable but gate runs cleanly.
        if surface == ExternalAgentExecutionSurface::Cloud {
            return Ok(ExternalAgentStartOutcome::gated(format!(
                "external-agent runtime `{runtime_id}` cloud execution surface is not available in this build yet; use the acp or sdk-local surface"
            )));
        }

        let (thread_id, thread) = self.load_thread(&thread_id).await?;
        let thread_config = thread.config().await;
        if let Err(message) = validate_external_agent_subscription_profile(
            &thread_config.codex_home,
            thread_config.selected_auth_profile.as_deref(),
            runtime_id,
        ) {
            return Ok(ExternalAgentStartOutcome::gated(message));
        }

        let run_id = format!("ext_{}", Uuid::new_v4());
        let mut runtime_request = ExternalAgentRequest::new(
            runtime_id,
            task,
            thread_config.cwd.to_path_buf(),
            runtime_mode,
        );
        runtime_request
            .metadata
            .insert("executionSurface".to_string(), surface.as_str().to_string());
        if let Some(model) = resolved_model.as_ref() {
            runtime_request
                .metadata
                .insert("model".to_string(), model.clone());
        }
        let runner = runner_for_runtime(runtime_id, surface).ok_or_else(|| {
            invalid_request(format!("unsupported external-agent runtime `{runtime_id}`"))
        })?;
        let source_env = external_agent_source_env(
            &thread_config.permissions.shell_environment_policy,
            runtime_id,
        );
        let readiness = runner.readiness_with_env(&source_env).await;
        if readiness.status != ExternalAgentReadinessStatus::Ready {
            return Ok(ExternalAgentStartOutcome::gated(readiness_gate_message(
                readiness,
            )));
        }
        let permission_profile = external_agent_permission_profile(
            &thread_config.cwd,
            &thread_config.workspace_roots,
            runtime_id,
            &source_env,
        );
        // The Codewith-mediated action executor (Comp2) enforces the run's mode
        // capabilities against the same permission profile that scopes the
        // sandboxed subprocess: reads within granted roots are auto-served, while
        // writes/commands/network egress require explicit per-action consent.
        let action_executor = external_agent_action_executor(
            &permission_profile,
            runtime_mode,
            &thread_config.cwd,
            &thread_config.workspace_roots,
        );
        let sandbox_config = ExternalAgentSandboxConfig {
            use_legacy_landlock: external_agent_use_legacy_landlock(
                &permission_profile,
                thread_config.cwd.as_path(),
            ),
            permission_profile,
            codex_linux_sandbox_exe: self.arg0_paths.codex_linux_sandbox_exe.clone(),
            windows_sandbox_level: WindowsSandboxLevel::from_config(thread_config.as_ref()),
            windows_sandbox_private_desktop: thread_config
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
                    self.thread_state_manager.clone(),
                    thread_id,
                    run_id,
                    cancellation_token,
                )
                .with_action_executor(action_executor),
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

/// Replay-stable key for a parked permission responder. Scoping by run keeps
/// runtime-chosen request ids from colliding across concurrent runs on one thread.
fn external_agent_permission_key(run_id: &str, request_id: &str) -> String {
    format!("{run_id}:{request_id}")
}

fn permission_decision_from_option(
    option: ThreadExternalAgentPermissionOption,
) -> ExternalAgentPermissionDecision {
    match option {
        ThreadExternalAgentPermissionOption::AllowOnce => {
            ExternalAgentPermissionDecision::AllowOnce
        }
        ThreadExternalAgentPermissionOption::RejectOnce => {
            ExternalAgentPermissionDecision::RejectOnce
        }
    }
}

/// Bound on how long the server parks a permission request before denying by
/// default, ensuring a run can never block forever waiting on a client.
const EXTERNAL_AGENT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);

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
    "PATHEXT",
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
        ExternalAgentRuntimeId::CLAUDE => None,
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
    thread_state_manager: ThreadStateManager,
    thread_id: ThreadId,
    run_id: String,
    terminal_sent: Arc<AtomicBool>,
    cancellation_token: CancellationToken,
    permission_timeout: Duration,
    action_executor: Arc<ExternalAgentActionExecutor>,
}

impl AppServerExternalAgentHost {
    fn new(
        outgoing: Arc<OutgoingMessageSender>,
        thread_state_manager: ThreadStateManager,
        thread_id: ThreadId,
        run_id: String,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            outgoing,
            thread_state_manager,
            thread_id,
            run_id,
            terminal_sent: Arc::new(AtomicBool::new(false)),
            cancellation_token,
            permission_timeout: EXTERNAL_AGENT_PERMISSION_TIMEOUT,
            // Deny-all by default; a real run attaches the executor built from
            // its permission profile via `with_action_executor`.
            action_executor: Arc::new(ExternalAgentActionExecutor::deny_all()),
        }
    }

    fn with_action_executor(mut self, action_executor: ExternalAgentActionExecutor) -> Self {
        self.action_executor = Arc::new(action_executor);
        self
    }

    #[cfg(test)]
    fn with_permission_timeout(mut self, permission_timeout: Duration) -> Self {
        self.permission_timeout = permission_timeout;
        self
    }

    /// Emit the audit notification for a resolved permission request.
    async fn emit_permission_resolved(
        &self,
        request_id: String,
        decision: ThreadExternalAgentPermissionOption,
        resolution: ThreadExternalAgentPermissionResolution,
    ) {
        self.emit(ThreadExternalAgentEvent::PermissionResolved {
            request_id,
            decision,
            resolution,
        })
        .await;
    }

    /// Park a responder for `request` and wait for a client decision, bounded by
    /// [`Self::permission_timeout`] and run cancellation. Always denies by
    /// default (`RejectOnce`) when no explicit allow is received, and emits a
    /// [`ThreadExternalAgentEvent::PermissionResolved`] audit notification.
    async fn await_permission_decision(
        &self,
        request: ExternalAgentPermissionRequest,
    ) -> ExternalAgentPermissionDecision {
        let ExternalAgentPermissionRequest {
            id: request_id,
            action,
            options,
        } = request;
        let key = external_agent_permission_key(&self.run_id, &request_id);
        let api_options = options
            .into_iter()
            .map(api_permission_option)
            .collect::<Vec<_>>();

        // Deny immediately if the run is already cancelled; do not park a waiter.
        if self.cancellation_token.is_cancelled() {
            self.emit_permission_resolved(
                request_id,
                ThreadExternalAgentPermissionOption::RejectOnce,
                ThreadExternalAgentPermissionResolution::Superseded,
            )
            .await;
            return ExternalAgentPermissionDecision::RejectOnce;
        }

        let (tx, rx) = oneshot::channel();
        let thread_state = self.thread_state_manager.thread_state(self.thread_id).await;
        let registered = {
            let mut state = thread_state.lock().await;
            state.register_external_agent_permission(key.clone(), api_options.clone(), tx)
        };
        if !registered {
            // A request with this id is already pending; deny this duplicate
            // without disturbing the original waiter (replay-safe).
            self.emit_permission_resolved(
                request_id,
                ThreadExternalAgentPermissionOption::RejectOnce,
                ThreadExternalAgentPermissionResolution::Superseded,
            )
            .await;
            return ExternalAgentPermissionDecision::RejectOnce;
        }

        // External-agent runtimes may emit a permission request before calling
        // into the host. App-server suppresses that early notification and emits
        // here, after the responder is registered, so a fast client cannot race
        // the parked request.
        self.emit(ThreadExternalAgentEvent::PermissionRequested {
            request: ThreadExternalAgentPermissionRequest {
                id: request_id.clone(),
                action: action_json(&action),
                options: api_options,
            },
        })
        .await;

        // Never hold the state mutex across this await.
        let (decision, resolution) = tokio::select! {
            biased;
            () = self.cancellation_token.cancelled() => (
                ThreadExternalAgentPermissionOption::RejectOnce,
                ThreadExternalAgentPermissionResolution::Superseded,
            ),
            result = tokio::time::timeout(self.permission_timeout, rx) => match result {
                Ok(Ok(option)) => (option, ThreadExternalAgentPermissionResolution::Client),
                Ok(Err(_)) => (
                    ThreadExternalAgentPermissionOption::RejectOnce,
                    ThreadExternalAgentPermissionResolution::DefaultDenied,
                ),
                Err(_) => (
                    ThreadExternalAgentPermissionOption::RejectOnce,
                    ThreadExternalAgentPermissionResolution::TimedOut,
                ),
            },
        };

        // Drop any responder still parked (timeout/cancellation path) so it
        // cannot leak and a replayed response can never match it.
        {
            let mut state = thread_state.lock().await;
            let _ = state.take_external_agent_permission(&key);
        }

        self.emit_permission_resolved(request_id, decision, resolution)
            .await;
        permission_decision_from_option(decision)
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
        let connection_ids = self
            .thread_state_manager
            .subscribed_connection_ids(self.thread_id)
            .await;
        if connection_ids.is_empty() {
            return;
        }
        self.outgoing
            .send_server_notification_to_connections(
                &connection_ids,
                ServerNotification::ThreadExternalAgentEvent(
                    ThreadExternalAgentEventNotification {
                        thread_id: self.thread_id.to_string(),
                        run_id: self.run_id.clone(),
                        event,
                    },
                ),
            )
            .await;
    }

    fn terminal_sent(&self) -> bool {
        self.terminal_sent.load(Ordering::SeqCst)
    }
}

impl ExternalAgentHost for AppServerExternalAgentHost {
    async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
        if matches!(event, ExternalAgentEvent::PermissionRequested { .. }) {
            return Ok(());
        }
        self.emit(api_external_agent_event(event)).await;
        Ok(())
    }

    async fn request_permission(
        &self,
        request: ExternalAgentPermissionRequest,
    ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
        Ok(self.await_permission_decision(request).await)
    }

    async fn perform_action(
        &self,
        action: ExternalAgentActionRequest,
    ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
        // Audit the proposed action before executing so replay stays
        // deterministic even if execution or consent later fails.
        self.emit(ThreadExternalAgentEvent::ProposedAction {
            proposal: action_json(&action),
        })
        .await;
        let executor = self.action_executor.clone();
        Ok(executor.execute(action, self).await)
    }

    async fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}

fn runner_for_runtime(
    runtime_id: &str,
    surface: ExternalAgentExecutionSurface,
) -> Option<ExternalAgentRunner> {
    match surface {
        // The hosted cloud harness (Comp1) is not part of this build yet; cloud
        // runs are gated before this point. Guard here defensively.
        ExternalAgentExecutionSurface::Cloud => None,
        // The acp and sdk-local surfaces both resolve to the runtime's local
        // harness: ACP stdio for Cursor/Grok, the Agent SDK stream for Claude.
        ExternalAgentExecutionSurface::Acp | ExternalAgentExecutionSurface::SdkLocal => {
            match runtime_id {
                ExternalAgentRuntimeId::CURSOR => {
                    cursor_acp_harness().map(ExternalAgentRunner::Acp)
                }
                ExternalAgentRuntimeId::GROK_BUILD => {
                    grok_build_acp_harness().map(ExternalAgentRunner::Acp)
                }
                ExternalAgentRuntimeId::CLAUDE => {
                    claude_code_harness().map(ExternalAgentRunner::Claude)
                }
                _ => None,
            }
        }
    }
}

fn external_agent_execution_surface(
    surface: ThreadExternalAgentExecutionSurface,
) -> ExternalAgentExecutionSurface {
    match surface {
        ThreadExternalAgentExecutionSurface::Acp => ExternalAgentExecutionSurface::Acp,
        ThreadExternalAgentExecutionSurface::SdkLocal => ExternalAgentExecutionSurface::SdkLocal,
        ThreadExternalAgentExecutionSurface::Cloud => ExternalAgentExecutionSurface::Cloud,
    }
}

fn api_execution_surface(
    surface: ExternalAgentExecutionSurface,
) -> ThreadExternalAgentExecutionSurface {
    match surface {
        ExternalAgentExecutionSurface::Acp => ThreadExternalAgentExecutionSurface::Acp,
        ExternalAgentExecutionSurface::SdkLocal => ThreadExternalAgentExecutionSurface::SdkLocal,
        ExternalAgentExecutionSurface::Cloud => ThreadExternalAgentExecutionSurface::Cloud,
    }
}

fn external_agent_model_api(model: &ExternalAgentModelDescriptor) -> ThreadExternalAgentModel {
    ThreadExternalAgentModel {
        id: model.id.to_string(),
        display_name: model.display_name.to_string(),
        description: Some(model.description.to_string()),
        execution_surfaces: model
            .execution_surfaces
            .iter()
            .map(|surface| api_execution_surface(*surface))
            .collect(),
    }
}

/// Build the models-list response for a runtime, optionally filtered to a single
/// execution surface. This is the Comp3 discovery source: the runtime's built-in
/// advertised models, which a live per-account discovery path can later refine.
fn external_agent_models_response(
    runtime_id: &str,
    descriptor: &ExternalAgentRuntimeDescriptor,
    surface: Option<ExternalAgentExecutionSurface>,
) -> ThreadExternalAgentModelsListResponse {
    let models = descriptor
        .models
        .iter()
        .filter(|model| match surface {
            Some(surface) => model.execution_surfaces.contains(&surface),
            None => true,
        })
        .map(external_agent_model_api)
        .collect::<Vec<_>>();
    let default_model = match surface {
        Some(surface) => descriptor
            .models_for_surface(surface)
            .next()
            .map(|model| model.id.to_string()),
        None => descriptor.default_model().map(|model| model.id.to_string()),
    };
    ThreadExternalAgentModelsListResponse {
        runtime_id: runtime_id.to_string(),
        models,
        default_model,
    }
}

fn external_agent_mode(mode: ThreadExternalAgentMode) -> ExternalAgentMode {
    match mode {
        ThreadExternalAgentMode::Plan => ExternalAgentMode::Plan,
        ThreadExternalAgentMode::Propose => ExternalAgentMode::Propose,
    }
}

/// Default upper bound on how long a delegated command may run before the
/// executor abandons it.
const EXTERNAL_AGENT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Codewith-mediated executor for external-agent managed actions (Comp2).
///
/// Built from the run's [`PermissionProfile`] plus the mode capabilities. File
/// reads inside the granted roots are auto-served; writes ("write-promote"),
/// terminal commands ("exec-delegate"), and network egress ("egress-consent")
/// require explicit per-action client consent routed through the host. This is
/// the single decision + execution point that replaced the old inline
/// reject-everything path, so managed runtimes never mutate the workspace
/// without Codewith approval + scope enforcement.
struct ExternalAgentActionExecutor {
    capabilities: ExternalAgentCapabilities,
    readable_roots: Vec<AbsolutePathBuf>,
    writable_roots: Vec<AbsolutePathBuf>,
    network: NetworkSandboxPolicy,
    cwd: PathBuf,
    command_timeout: Duration,
}

/// How the executor should handle a classified action before it is performed.
enum ActionDisposition {
    /// Perform immediately (reads within granted roots).
    Allow,
    /// Ask the client for per-action consent before performing.
    NeedsConsent,
    /// Reject outright with the given reason.
    Deny(String),
}

impl ExternalAgentActionExecutor {
    fn new(
        capabilities: ExternalAgentCapabilities,
        readable_roots: Vec<AbsolutePathBuf>,
        writable_roots: Vec<AbsolutePathBuf>,
        network: NetworkSandboxPolicy,
        cwd: PathBuf,
    ) -> Self {
        Self {
            capabilities,
            readable_roots,
            writable_roots,
            network,
            cwd,
            command_timeout: EXTERNAL_AGENT_COMMAND_TIMEOUT,
        }
    }

    /// An executor that rejects every action; the default until a run attaches
    /// the executor built from its permission profile.
    fn deny_all() -> Self {
        Self::new(
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Plan),
            Vec::new(),
            Vec::new(),
            NetworkSandboxPolicy::Restricted,
            PathBuf::new(),
        )
    }

    #[cfg(test)]
    fn with_command_timeout(mut self, command_timeout: Duration) -> Self {
        self.command_timeout = command_timeout;
        self
    }

    async fn execute(
        &self,
        action: ExternalAgentActionRequest,
        host: &AppServerExternalAgentHost,
    ) -> ExternalAgentActionResult {
        match self.classify(&action) {
            ActionDisposition::Deny(reason) => ExternalAgentActionResult::Rejected { reason },
            ActionDisposition::Allow => self.perform(action).await,
            ActionDisposition::NeedsConsent => {
                if self.request_consent(host, action.clone()).await {
                    self.perform(action).await
                } else {
                    ExternalAgentActionResult::Rejected {
                        reason: consent_denied_reason(&action),
                    }
                }
            }
        }
    }

    fn classify(&self, action: &ExternalAgentActionRequest) -> ActionDisposition {
        match action {
            ExternalAgentActionRequest::ReadFile { path } => {
                if matches!(self.capabilities.filesystem, FileSystemCapability::None) {
                    return ActionDisposition::Deny(
                        "this run mode does not permit file reads".to_string(),
                    );
                }
                if !self.path_is_readable(path) {
                    return ActionDisposition::Deny(format!(
                        "read path `{}` is outside the run's granted roots",
                        path.display()
                    ));
                }
                ActionDisposition::Allow
            }
            ExternalAgentActionRequest::WriteFile { path, .. } => {
                if !matches!(self.capabilities.filesystem, FileSystemCapability::ManagedReadWrite) {
                    return ActionDisposition::Deny(
                        "this run mode does not permit file writes".to_string(),
                    );
                }
                if !self.path_is_writable(path) {
                    return ActionDisposition::Deny(format!(
                        "write path `{}` is outside the run's writable workspace roots",
                        path.display()
                    ));
                }
                ActionDisposition::NeedsConsent
            }
            ExternalAgentActionRequest::RunCommand { command, .. } => {
                if !matches!(self.capabilities.terminal, TerminalCapability::Managed) {
                    return ActionDisposition::Deny(
                        "this run mode does not permit terminal commands".to_string(),
                    );
                }
                if command.is_empty() {
                    return ActionDisposition::Deny("cannot run an empty command".to_string());
                }
                ActionDisposition::NeedsConsent
            }
            ExternalAgentActionRequest::NetworkAccess { .. } => {
                if !matches!(self.capabilities.network, NetworkCapability::Managed)
                    || self.network != NetworkSandboxPolicy::Enabled
                {
                    return ActionDisposition::Deny(
                        "this run mode does not permit network egress".to_string(),
                    );
                }
                ActionDisposition::NeedsConsent
            }
            ExternalAgentActionRequest::McpToolCall { server, tool, .. } => {
                if matches!(self.capabilities.mcp, McpCapability::None) {
                    return ActionDisposition::Deny(
                        "this run mode does not permit MCP tool calls".to_string(),
                    );
                }
                ActionDisposition::Deny(format!(
                    "no MCP facade is configured for this run (server `{server}`, tool `{tool}`)"
                ))
            }
            ExternalAgentActionRequest::Other { label, .. } => {
                ActionDisposition::Deny(format!("unsupported managed action `{label}`"))
            }
        }
    }

    async fn request_consent(
        &self,
        host: &AppServerExternalAgentHost,
        action: ExternalAgentActionRequest,
    ) -> bool {
        let request = ExternalAgentPermissionRequest {
            id: format!("act_{}", Uuid::new_v4()),
            action,
            options: vec![
                ExternalAgentPermissionOption::AllowOnce,
                ExternalAgentPermissionOption::RejectOnce,
            ],
        };
        matches!(
            host.await_permission_decision(request).await,
            ExternalAgentPermissionDecision::AllowOnce
        )
    }

    async fn perform(&self, action: ExternalAgentActionRequest) -> ExternalAgentActionResult {
        match action {
            ExternalAgentActionRequest::ReadFile { path } => match std::fs::read_to_string(&path) {
                Ok(content) => ExternalAgentActionResult::FileContent { content },
                Err(err) => {
                    rejected(format!("failed to read `{}`: {err}", path.display()))
                }
            },
            ExternalAgentActionRequest::WriteFile { path, content } => {
                if let Some(parent) = path.parent()
                    && let Err(err) = std::fs::create_dir_all(parent)
                {
                    return rejected(format!("failed to prepare `{}`: {err}", path.display()));
                }
                match std::fs::write(&path, content) {
                    Ok(()) => ExternalAgentActionResult::WriteAccepted,
                    Err(err) => rejected(format!("failed to write `{}`: {err}", path.display())),
                }
            }
            ExternalAgentActionRequest::RunCommand { command, cwd } => {
                self.run_command(command, cwd).await
            }
            ExternalAgentActionRequest::NetworkAccess { .. } => {
                ExternalAgentActionResult::NetworkAccessReady
            }
            ExternalAgentActionRequest::McpToolCall { .. }
            | ExternalAgentActionRequest::Other { .. } => {
                rejected("unsupported managed action".to_string())
            }
        }
    }

    async fn run_command(
        &self,
        command: Vec<String>,
        cwd: Option<PathBuf>,
    ) -> ExternalAgentActionResult {
        let Some((program, args)) = command.split_first() else {
            return rejected("cannot run an empty command".to_string());
        };
        let working_dir = cwd.unwrap_or_else(|| self.cwd.clone());
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .current_dir(&working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let output = match tokio::time::timeout(self.command_timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return rejected(format!("failed to run `{program}`: {err}")),
            Err(_) => return rejected(format!("command `{program}` timed out")),
        };
        ExternalAgentActionResult::CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn path_is_readable(&self, path: &Path) -> bool {
        self.path_within(path, &self.readable_roots) || self.path_within(path, &self.writable_roots)
    }

    fn path_is_writable(&self, path: &Path) -> bool {
        self.path_within(path, &self.writable_roots)
    }

    fn path_within(&self, path: &Path, roots: &[AbsolutePathBuf]) -> bool {
        if roots.is_empty() {
            return false;
        }
        let candidate = self.absolutize(path);
        roots
            .iter()
            .any(|root| candidate.starts_with(normalize_lexically(root.as_path())))
    }

    fn absolutize(&self, path: &Path) -> PathBuf {
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        normalize_lexically(&joined)
    }
}

fn rejected(reason: String) -> ExternalAgentActionResult {
    ExternalAgentActionResult::Rejected { reason }
}

fn consent_denied_reason(action: &ExternalAgentActionRequest) -> String {
    match action {
        ExternalAgentActionRequest::WriteFile { path, .. } => {
            format!("write to `{}` was not approved", path.display())
        }
        ExternalAgentActionRequest::RunCommand { command, .. } => format!(
            "command `{}` was not approved",
            command.first().map(String::as_str).unwrap_or_default()
        ),
        ExternalAgentActionRequest::NetworkAccess { target, .. } => {
            format!("network egress to `{target}` was not approved")
        }
        _ => "the requested action was not approved".to_string(),
    }
}

/// Lexically normalize a path (resolve `.`/`..` without touching the
/// filesystem) so root-containment checks cannot be bypassed with `..`.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Build the Comp2 action executor from the run's permission profile and mode.
fn external_agent_action_executor(
    permission_profile: &PermissionProfile,
    mode: ExternalAgentMode,
    cwd: &AbsolutePathBuf,
    workspace_roots: &[AbsolutePathBuf],
) -> ExternalAgentActionExecutor {
    let (file_system, network) = permission_profile.to_runtime_permissions();
    let readable_roots = file_system
        .entries
        .iter()
        .filter_map(|entry| match &entry.path {
            FileSystemPath::Path { path } if entry.access == FileSystemAccessMode::Read => {
                Some(path.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    // Managed runs may promote writes within the workspace roots (each write is
    // still consented per-action); non-managed runs get no writable scope.
    let writable_roots = if matches!(mode, ExternalAgentMode::Managed) {
        if workspace_roots.is_empty() {
            vec![cwd.clone()]
        } else {
            workspace_roots.to_vec()
        }
    } else {
        Vec::new()
    };
    ExternalAgentActionExecutor::new(
        ExternalAgentCapabilities::for_mode(mode),
        readable_roots,
        writable_roots,
        network,
        cwd.to_path_buf(),
    )
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
    use crate::outgoing_message::OutgoingEnvelope;
    use crate::outgoing_message::OutgoingMessage;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn external_agent_host_targets_subscribed_thread_connections() {
        let (tx, mut rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            tx,
            codex_analytics::AnalyticsEventsClient::disabled(),
        ));
        let thread_state_manager = ThreadStateManager::new();
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000301")
            .expect("thread id should parse");
        thread_state_manager
            .connection_initialized(ConnectionId(1), ConnectionCapabilities::default())
            .await;
        thread_state_manager
            .connection_initialized(ConnectionId(2), ConnectionCapabilities::default())
            .await;
        assert!(
            thread_state_manager
                .try_add_connection_to_thread(thread_id, ConnectionId(1))
                .await
        );
        let host = AppServerExternalAgentHost::new(
            outgoing,
            thread_state_manager,
            thread_id,
            "run-1".to_string(),
            CancellationToken::new(),
        );

        host.emit(ThreadExternalAgentEvent::Status {
            message: "working".to_string(),
        })
        .await;

        let envelope = rx.recv().await.expect("targeted notification");
        let OutgoingEnvelope::ToConnection {
            connection_id,
            message,
            ..
        } = envelope
        else {
            panic!("expected targeted envelope, got {envelope:?}");
        };
        assert_eq!(connection_id, ConnectionId(1));
        let OutgoingMessage::AppServerNotification(ServerNotification::ThreadExternalAgentEvent(
            notification,
        )) = message
        else {
            panic!("expected external-agent notification, got {message:?}");
        };
        assert_eq!(notification.thread_id, thread_id.to_string());
        assert_eq!(notification.run_id, "run-1");
        assert_eq!(
            notification.event,
            ThreadExternalAgentEvent::Status {
                message: "working".to_string(),
            }
        );
        assert!(rx.try_recv().is_err());
    }

    async fn subscribed_host(
        thread_id: ThreadId,
        run_id: &str,
        permission_timeout: Duration,
    ) -> (AppServerExternalAgentHost, mpsc::Receiver<OutgoingEnvelope>) {
        let (tx, rx) = mpsc::channel(8);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            tx,
            codex_analytics::AnalyticsEventsClient::disabled(),
        ));
        let thread_state_manager = ThreadStateManager::new();
        thread_state_manager
            .connection_initialized(ConnectionId(1), ConnectionCapabilities::default())
            .await;
        assert!(
            thread_state_manager
                .try_add_connection_to_thread(thread_id, ConnectionId(1))
                .await
        );
        let host = AppServerExternalAgentHost::new(
            outgoing,
            thread_state_manager,
            thread_id,
            run_id.to_string(),
            CancellationToken::new(),
        )
        .with_permission_timeout(permission_timeout);
        (host, rx)
    }

    async fn recv_external_agent_event(
        rx: &mut mpsc::Receiver<OutgoingEnvelope>,
    ) -> ThreadExternalAgentEvent {
        let envelope = rx.recv().await.expect("external-agent notification");
        let OutgoingEnvelope::ToConnection { message, .. } = envelope else {
            panic!("expected targeted envelope, got {envelope:?}");
        };
        let OutgoingMessage::AppServerNotification(ServerNotification::ThreadExternalAgentEvent(
            notification,
        )) = message
        else {
            panic!("expected external-agent notification, got {message:?}");
        };
        notification.event
    }

    fn permission_request(id: &str) -> ExternalAgentPermissionRequest {
        ExternalAgentPermissionRequest {
            id: id.to_string(),
            action: ExternalAgentActionRequest::ReadFile {
                path: PathBuf::from("/tmp/example"),
            },
            options: vec![
                ExternalAgentPermissionOption::AllowOnce,
                ExternalAgentPermissionOption::RejectOnce,
            ],
        }
    }

    #[tokio::test]
    async fn external_agent_permission_allow_once_round_trip_emits_resolved() {
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000401")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-1", Duration::from_secs(30)).await;
        let key = external_agent_permission_key("run-1", "perm-1");
        let thread_state = host.thread_state_manager.thread_state(thread_id).await;

        let await_host = host.clone();
        let handle = tokio::spawn(async move {
            await_host
                .request_permission(permission_request("perm-1"))
                .await
        });

        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionRequested {
                request: ThreadExternalAgentPermissionRequest {
                    id: "perm-1".to_string(),
                    action: action_json(&ExternalAgentActionRequest::ReadFile {
                        path: PathBuf::from("/tmp/example"),
                    }),
                    options: vec![
                        ThreadExternalAgentPermissionOption::AllowOnce,
                        ThreadExternalAgentPermissionOption::RejectOnce,
                    ],
                },
            }
        );
        assert!(thread_state.lock().await.respond_external_agent_permission(
            &key,
            ThreadExternalAgentPermissionOption::AllowOnce
        ));

        let decision = handle.await.expect("join").expect("permission decision");
        assert_eq!(decision, ExternalAgentPermissionDecision::AllowOnce);
        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionResolved {
                request_id: "perm-1".to_string(),
                decision: ThreadExternalAgentPermissionOption::AllowOnce,
                resolution: ThreadExternalAgentPermissionResolution::Client,
            }
        );
    }

    #[tokio::test]
    async fn external_agent_permission_times_out_and_default_denies() {
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000402")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-1", Duration::from_millis(20)).await;

        let decision = host.request_permission(permission_request("perm-1")).await;
        assert_eq!(
            decision.expect("permission decision"),
            ExternalAgentPermissionDecision::RejectOnce
        );
        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionRequested {
                request: ThreadExternalAgentPermissionRequest {
                    id: "perm-1".to_string(),
                    action: action_json(&ExternalAgentActionRequest::ReadFile {
                        path: PathBuf::from("/tmp/example"),
                    }),
                    options: vec![
                        ThreadExternalAgentPermissionOption::AllowOnce,
                        ThreadExternalAgentPermissionOption::RejectOnce,
                    ],
                },
            }
        );
        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionResolved {
                request_id: "perm-1".to_string(),
                decision: ThreadExternalAgentPermissionOption::RejectOnce,
                resolution: ThreadExternalAgentPermissionResolution::TimedOut,
            }
        );

        // The timed-out responder must not leak; a late respond finds nothing.
        let thread_state = host.thread_state_manager.thread_state(thread_id).await;
        let key = external_agent_permission_key("run-1", "perm-1");
        assert!(
            thread_state
                .lock()
                .await
                .take_external_agent_permission(&key)
                .is_none()
        );
    }

    #[tokio::test]
    async fn external_agent_permission_cancellation_default_denies() {
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000403")
            .expect("thread id should parse");
        let (mut host, mut rx) = subscribed_host(thread_id, "run-1", Duration::from_secs(30)).await;
        let token = CancellationToken::new();
        host.cancellation_token = token.clone();
        token.cancel();

        let decision = host.request_permission(permission_request("perm-1")).await;
        assert_eq!(
            decision.expect("permission decision"),
            ExternalAgentPermissionDecision::RejectOnce
        );
        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionResolved {
                request_id: "perm-1".to_string(),
                decision: ThreadExternalAgentPermissionOption::RejectOnce,
                resolution: ThreadExternalAgentPermissionResolution::Superseded,
            }
        );
    }

    #[tokio::test]
    async fn external_agent_permission_take_is_idempotent_and_replay_safe() {
        let mut state = crate::thread_state::ThreadState::default();
        let key = external_agent_permission_key("run-1", "perm-1");
        let (tx, _rx) = tokio::sync::oneshot::channel();
        assert!(state.register_external_agent_permission(
            key.clone(),
            vec![
                ThreadExternalAgentPermissionOption::AllowOnce,
                ThreadExternalAgentPermissionOption::RejectOnce,
            ],
            tx
        ));

        // A duplicate registration for the same key is rejected (original kept).
        let (dup_tx, _dup_rx) = tokio::sync::oneshot::channel();
        assert!(!state.register_external_agent_permission(
            key.clone(),
            vec![ThreadExternalAgentPermissionOption::RejectOnce],
            dup_tx
        ));

        // First take returns the responder; a replayed take is a no-op.
        assert!(state.take_external_agent_permission(&key).is_some());
        assert!(state.take_external_agent_permission(&key).is_none());
        // An unknown request id is also a no-op.
        assert!(
            state
                .take_external_agent_permission(&external_agent_permission_key("run-1", "other"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn external_agent_permission_invalid_decision_default_denies() {
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000404")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-1", Duration::from_secs(30)).await;
        let key = external_agent_permission_key("run-1", "perm-1");
        let thread_state = host.thread_state_manager.thread_state(thread_id).await;

        let await_host = host.clone();
        let handle = tokio::spawn(async move {
            await_host
                .request_permission(ExternalAgentPermissionRequest {
                    id: "perm-1".to_string(),
                    action: ExternalAgentActionRequest::ReadFile {
                        path: PathBuf::from("/tmp/example"),
                    },
                    options: vec![ExternalAgentPermissionOption::RejectOnce],
                })
                .await
        });

        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionRequested {
                request: ThreadExternalAgentPermissionRequest {
                    id: "perm-1".to_string(),
                    action: action_json(&ExternalAgentActionRequest::ReadFile {
                        path: PathBuf::from("/tmp/example"),
                    }),
                    options: vec![ThreadExternalAgentPermissionOption::RejectOnce],
                },
            }
        );
        assert!(
            !thread_state.lock().await.respond_external_agent_permission(
                &key,
                ThreadExternalAgentPermissionOption::AllowOnce
            )
        );

        let decision = handle.await.expect("join").expect("permission decision");
        assert_eq!(decision, ExternalAgentPermissionDecision::RejectOnce);
        assert_eq!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::PermissionResolved {
                request_id: "perm-1".to_string(),
                decision: ThreadExternalAgentPermissionOption::RejectOnce,
                resolution: ThreadExternalAgentPermissionResolution::DefaultDenied,
            }
        );
    }

    #[tokio::test]
    async fn external_agent_host_drops_events_without_subscribers() {
        let (tx, mut rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            tx,
            codex_analytics::AnalyticsEventsClient::disabled(),
        ));
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000302")
            .expect("thread id should parse");
        let host = AppServerExternalAgentHost::new(
            outgoing,
            ThreadStateManager::new(),
            thread_id,
            "run-1".to_string(),
            CancellationToken::new(),
        );

        host.emit(ThreadExternalAgentEvent::Status {
            message: "working".to_string(),
        })
        .await;

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn external_agent_permission_profile_scopes_read_roots_with_network() {
        let cwd = tempfile::TempDir::new().expect("cwd tempdir");
        let workspace = tempfile::TempDir::new().expect("workspace tempdir");
        let home = tempfile::TempDir::new().expect("home tempdir");
        let cwd = AbsolutePathBuf::from_absolute_path(cwd.path()).expect("absolute cwd");
        let workspace =
            AbsolutePathBuf::from_absolute_path(workspace.path()).expect("absolute workspace");
        let claude_config = home.path().join(".claude");
        let aws_credentials = home.path().join(".aws").join("credentials");
        let google_credentials = home.path().join("gcp.json");
        let azure_dir = home.path().join(".azure");
        let source_env = BTreeMap::from([
            ("HOME".to_string(), home.path().display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                claude_config.display().to_string(),
            ),
            ("CLAUDE_CODE_USE_BEDROCK".to_string(), "1".to_string()),
            (
                "AWS_SHARED_CREDENTIALS_FILE".to_string(),
                aws_credentials.display().to_string(),
            ),
            ("CLAUDE_CODE_USE_VERTEX".to_string(), "1".to_string()),
            (
                "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                google_credentials.display().to_string(),
            ),
            ("CLAUDE_CODE_USE_FOUNDRY".to_string(), "1".to_string()),
            (
                "AZURE_CONFIG_DIR".to_string(),
                azure_dir.display().to_string(),
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
        assert!(
            !read_paths.contains(
                &AbsolutePathBuf::from_absolute_path(aws_credentials).expect("aws credentials")
            ),
            "external-agent readable roots must not expose cloud credential files"
        );
        assert!(
            !read_paths.contains(
                &AbsolutePathBuf::from_absolute_path(google_credentials)
                    .expect("google credentials")
            ),
            "external-agent readable roots must not expose cloud credential files"
        );
        assert!(
            !read_paths
                .contains(&AbsolutePathBuf::from_absolute_path(azure_dir).expect("azure dir")),
            "external-agent readable roots must not expose cloud credential directories"
        );
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
        let Some(runner) =
            runner_for_runtime(ExternalAgentRuntimeId::CLAUDE, ExternalAgentExecutionSurface::SdkLocal)
        else {
            panic!("claude runner");
        };

        assert!(matches!(runner, ExternalAgentRunner::Claude(_)));
    }

    #[test]
    fn runner_selection_follows_execution_surface() {
        let Some(cursor_acp) =
            runner_for_runtime(ExternalAgentRuntimeId::CURSOR, ExternalAgentExecutionSurface::Acp)
        else {
            panic!("cursor acp runner");
        };
        assert!(matches!(cursor_acp, ExternalAgentRunner::Acp(_)));

        let Some(cursor_sdk) = runner_for_runtime(
            ExternalAgentRuntimeId::CURSOR,
            ExternalAgentExecutionSurface::SdkLocal,
        ) else {
            panic!("cursor sdk-local runner");
        };
        assert!(matches!(cursor_sdk, ExternalAgentRunner::Acp(_)));

        // The hosted cloud harness is not part of this build yet.
        assert!(
            runner_for_runtime(
                ExternalAgentRuntimeId::CURSOR,
                ExternalAgentExecutionSurface::Cloud
            )
            .is_none()
        );
    }

    #[test]
    fn execution_surface_round_trips_between_protocol_and_runtime() {
        for (api, runtime) in [
            (
                ThreadExternalAgentExecutionSurface::Acp,
                ExternalAgentExecutionSurface::Acp,
            ),
            (
                ThreadExternalAgentExecutionSurface::SdkLocal,
                ExternalAgentExecutionSurface::SdkLocal,
            ),
            (
                ThreadExternalAgentExecutionSurface::Cloud,
                ExternalAgentExecutionSurface::Cloud,
            ),
        ] {
            assert_eq!(external_agent_execution_surface(api), runtime);
            assert_eq!(api_execution_surface(runtime), api);
        }
    }

    #[test]
    fn cursor_models_response_advertises_auto_default_and_filters_by_surface() {
        let cursor = find_external_agent_runtime(ExternalAgentRuntimeId::CURSOR)
            .expect("cursor descriptor");
        let all = external_agent_models_response(ExternalAgentRuntimeId::CURSOR, cursor, None);
        assert_eq!(all.runtime_id, "cursor");
        assert_eq!(all.default_model.as_deref(), Some("auto"));
        assert!(all.models.iter().any(|model| model.id == "auto"));
        assert!(all.models.iter().any(|model| model.id == "gpt-5-codex"));
        for model in &all.models {
            assert!(
                model
                    .execution_surfaces
                    .contains(&ThreadExternalAgentExecutionSurface::Acp),
                "cursor models should advertise the acp surface"
            );
        }

        let cloud = external_agent_models_response(
            ExternalAgentRuntimeId::CURSOR,
            cursor,
            Some(ExternalAgentExecutionSurface::Cloud),
        );
        assert!(!cloud.models.is_empty());
        for model in &cloud.models {
            assert!(
                model
                    .execution_surfaces
                    .contains(&ThreadExternalAgentExecutionSurface::Cloud)
            );
        }
    }

    #[test]
    fn claude_source_env_preserves_stable_config_paths_without_process_auth_env() {
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
    fn claude_source_env_keeps_agent_sdk_auth_policy_values() {
        let policy = codex_protocol::config_types::ShellEnvironmentPolicy {
            inherit: codex_protocol::config_types::ShellEnvironmentPolicyInherit::None,
            r#set: HashMap::from([
                ("ANTHROPIC_API_KEY".to_string(), "test-value".to_string()),
                ("CLAUDE_CODE_USE_BEDROCK".to_string(), "1".to_string()),
            ]),
            ..codex_protocol::config_types::ShellEnvironmentPolicy::default()
        };

        let source_env = external_agent_source_env(&policy, ExternalAgentRuntimeId::CLAUDE);

        // Policy-set agent SDK auth values survive with their policy values.
        assert_eq!(
            source_env.get("ANTHROPIC_API_KEY"),
            Some(&"test-value".to_string())
        );
        assert_eq!(
            source_env.get("CLAUDE_CODE_USE_BEDROCK"),
            Some(&"1".to_string())
        );
        // Beyond the policy values, only the stable config path variables may
        // be inherited from the ambient process environment (whose contents
        // this test cannot pin); auth env vars never leak in from the
        // process environment.
        for key in source_env.keys() {
            assert!(
                key == "ANTHROPIC_API_KEY"
                    || key == "CLAUDE_CODE_USE_BEDROCK"
                    || CLAUDE_STABLE_CONFIG_ENV_VARS.contains(&key.as_str()),
                "unexpected env var {key} in claude source env"
            );
        }
    }

    #[test]
    fn external_agent_runtime_requires_matching_subscription_profile_except_claude() {
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
                /*selected_auth_profile*/ None,
                ExternalAgentRuntimeId::CLAUDE,
            )
            .is_ok()
        );
        assert!(
            validate_external_agent_subscription_profile(
                codex_home.path(),
                Some("cursor-work"),
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
                Some("grok-work"),
                ExternalAgentRuntimeId::CURSOR,
            )
            .expect_err("mismatch should fail"),
            "external-agent runtime `cursor` requires an active Cursor auth profile or a ChatGPT profile, but `grok-work` is tied to Grok"
        );
    }

    fn managed_executor(
        readable_roots: Vec<AbsolutePathBuf>,
        writable_roots: Vec<AbsolutePathBuf>,
        cwd: PathBuf,
    ) -> ExternalAgentActionExecutor {
        ExternalAgentActionExecutor::new(
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Managed),
            readable_roots,
            writable_roots,
            NetworkSandboxPolicy::Enabled,
            cwd,
        )
    }

    /// Drive `perform_action` for an action that needs consent, answer with
    /// `decision`, and return the executor result.
    async fn perform_with_consent(
        host: &AppServerExternalAgentHost,
        rx: &mut mpsc::Receiver<OutgoingEnvelope>,
        thread_id: ThreadId,
        run_id: &str,
        action: ExternalAgentActionRequest,
        decision: ThreadExternalAgentPermissionOption,
    ) -> ExternalAgentActionResult {
        let action_host = host.clone();
        let handle = tokio::spawn(async move { action_host.perform_action(action).await });

        // Every managed action is audited before it runs.
        assert!(matches!(
            recv_external_agent_event(rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));

        // Then the executor asks for explicit per-action consent.
        let ThreadExternalAgentEvent::PermissionRequested { request } =
            recv_external_agent_event(rx).await
        else {
            panic!("expected a permission request for the mediated action");
        };
        let key = external_agent_permission_key(run_id, &request.id);
        let thread_state = host.thread_state_manager.thread_state(thread_id).await;
        assert!(
            thread_state
                .lock()
                .await
                .respond_external_agent_permission(&key, decision)
        );

        let result = handle.await.expect("join").expect("action result");
        assert!(matches!(
            recv_external_agent_event(rx).await,
            ThreadExternalAgentEvent::PermissionResolved { .. }
        ));
        result
    }

    #[tokio::test]
    async fn managed_read_auto_allows_within_granted_roots() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "hello world").expect("seed file");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000501")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let result = host
            .perform_action(ExternalAgentActionRequest::ReadFile { path: file })
            .await
            .expect("read result");
        assert_eq!(
            result,
            ExternalAgentActionResult::FileContent {
                content: "hello world".to_string()
            }
        );
        // Reads within scope only surface the proposed-action audit; no consent.
        assert!(matches!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn managed_read_outside_roots_is_rejected() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000502")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let escape = dir.path().join("..").join("outside.txt");
        let result = host
            .perform_action(ExternalAgentActionRequest::ReadFile { path: escape })
            .await
            .expect("read result");
        assert!(
            matches!(result, ExternalAgentActionResult::Rejected { .. }),
            "reads outside the granted roots must be rejected: {result:?}"
        );
        assert!(matches!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn managed_write_promote_writes_after_consent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());
        let target = dir.path().join("nested").join("generated.txt");

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000503")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let result = perform_with_consent(
            &host,
            &mut rx,
            thread_id,
            "run-exec",
            ExternalAgentActionRequest::WriteFile {
                path: target.clone(),
                content: "generated by codewith".to_string(),
            },
            ThreadExternalAgentPermissionOption::AllowOnce,
        )
        .await;

        assert_eq!(result, ExternalAgentActionResult::WriteAccepted);
        assert_eq!(
            std::fs::read_to_string(&target).expect("written file"),
            "generated by codewith"
        );
    }

    #[tokio::test]
    async fn managed_write_promote_denied_leaves_file_untouched() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());
        let target = dir.path().join("denied.txt");

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000504")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let result = perform_with_consent(
            &host,
            &mut rx,
            thread_id,
            "run-exec",
            ExternalAgentActionRequest::WriteFile {
                path: target.clone(),
                content: "should not land".to_string(),
            },
            ThreadExternalAgentPermissionOption::RejectOnce,
        )
        .await;

        assert!(matches!(result, ExternalAgentActionResult::Rejected { .. }));
        assert!(!target.exists(), "a denied write must not touch the file");
    }

    #[tokio::test]
    async fn managed_exec_delegate_runs_command_after_consent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000505")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo codewith".to_string(),
            ]
        } else {
            vec!["/bin/echo".to_string(), "codewith".to_string()]
        };
        let result = perform_with_consent(
            &host,
            &mut rx,
            thread_id,
            "run-exec",
            ExternalAgentActionRequest::RunCommand {
                command,
                cwd: None,
            },
            ThreadExternalAgentPermissionOption::AllowOnce,
        )
        .await;

        let ExternalAgentActionResult::CommandOutput {
            exit_code, stdout, ..
        } = result
        else {
            panic!("expected command output, got {result:?}");
        };
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("codewith"),
            "delegated command stdout should carry output: {stdout:?}"
        );
    }

    #[tokio::test]
    async fn managed_network_egress_ready_after_consent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000506")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let result = perform_with_consent(
            &host,
            &mut rx,
            thread_id,
            "run-exec",
            ExternalAgentActionRequest::NetworkAccess {
                target: "https://api.cursor.com".to_string(),
                purpose: Some("model discovery".to_string()),
            },
            ThreadExternalAgentPermissionOption::AllowOnce,
        )
        .await;

        assert_eq!(result, ExternalAgentActionResult::NetworkAccessReady);
    }

    #[tokio::test]
    async fn propose_mode_rejects_writes_and_commands_without_consent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = ExternalAgentActionExecutor::new(
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
            vec![root.clone()],
            Vec::new(),
            NetworkSandboxPolicy::Enabled,
            dir.path().to_path_buf(),
        );

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000507")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(executor);

        let write = host
            .perform_action(ExternalAgentActionRequest::WriteFile {
                path: dir.path().join("x.txt"),
                content: "nope".to_string(),
            })
            .await
            .expect("write result");
        assert!(matches!(write, ExternalAgentActionResult::Rejected { .. }));
        assert!(matches!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));

        let command = host
            .perform_action(ExternalAgentActionRequest::RunCommand {
                command: vec!["/bin/echo".to_string(), "nope".to_string()],
                cwd: None,
            })
            .await
            .expect("command result");
        assert!(matches!(command, ExternalAgentActionResult::Rejected { .. }));
        assert!(matches!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));
        // Neither rejected action asked the client for consent.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn plan_mode_rejects_reads() {
        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000508")
            .expect("thread id should parse");
        let (host, mut rx) = subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let host = host.with_action_executor(ExternalAgentActionExecutor::deny_all());

        let result = host
            .perform_action(ExternalAgentActionRequest::ReadFile {
                path: PathBuf::from("/etc/hosts"),
            })
            .await
            .expect("read result");
        assert!(matches!(result, ExternalAgentActionResult::Rejected { .. }));
        assert!(matches!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));
    }

    #[tokio::test]
    async fn managed_action_rejected_when_run_is_cancelled() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = AbsolutePathBuf::from_absolute_path(dir.path()).expect("abs root");
        let executor = managed_executor(vec![root.clone()], vec![root], dir.path().to_path_buf());

        let thread_id = ThreadId::from_string("00000000-0000-4000-8000-000000000509")
            .expect("thread id should parse");
        let (mut host, mut rx) =
            subscribed_host(thread_id, "run-exec", Duration::from_secs(30)).await;
        let token = CancellationToken::new();
        host.cancellation_token = token.clone();
        token.cancel();
        let host = host.with_action_executor(executor);

        let result = host
            .perform_action(ExternalAgentActionRequest::WriteFile {
                path: dir.path().join("late.txt"),
                content: "too late".to_string(),
            })
            .await
            .expect("write result");
        assert!(matches!(result, ExternalAgentActionResult::Rejected { .. }));
        // Proposed audit, then a superseded resolution because the run is gone.
        assert!(matches!(
            recv_external_agent_event(&mut rx).await,
            ThreadExternalAgentEvent::ProposedAction { .. }
        ));
        let resolved = recv_external_agent_event(&mut rx).await;
        let ThreadExternalAgentEvent::PermissionResolved {
            decision,
            resolution,
            ..
        } = resolved
        else {
            panic!("expected a permission-resolved audit, got {resolved:?}");
        };
        assert_eq!(decision, ThreadExternalAgentPermissionOption::RejectOnce);
        assert_eq!(
            resolution,
            ThreadExternalAgentPermissionResolution::Superseded
        );
    }
}
