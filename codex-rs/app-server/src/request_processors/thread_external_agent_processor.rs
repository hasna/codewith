use super::thread_processor::ThreadRequestProcessor;
use super::*;
use codex_app_server_protocol::ThreadExternalAgentEvent;
use codex_app_server_protocol::ThreadExternalAgentEventNotification;
use codex_app_server_protocol::ThreadExternalAgentPermissionOption;
use codex_app_server_protocol::ThreadExternalAgentPermissionRequest;
use codex_external_agent::AcpStdioHarness;
use codex_external_agent::ExternalAgentActionRequest;
use codex_external_agent::ExternalAgentActionResult;
use codex_external_agent::ExternalAgentError;
use codex_external_agent::ExternalAgentEvent;
use codex_external_agent::ExternalAgentHost;
use codex_external_agent::ExternalAgentMode;
use codex_external_agent::ExternalAgentPermissionDecision;
use codex_external_agent::ExternalAgentPermissionOption;
use codex_external_agent::ExternalAgentPermissionRequest;
use codex_external_agent::ExternalAgentRequest;
use codex_external_agent::ExternalAgentRuntimeId;
use codex_external_agent::ExternalAgentSandboxConfig;
use codex_external_agent::cursor_acp_harness;
use codex_external_agent::grok_build_acp_harness;
use serde_json::Value as JsonValue;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

impl ThreadRequestProcessor {
    pub(crate) async fn thread_external_agent_start(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadExternalAgentStartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let run = self.thread_external_agent_start_inner(params).await?;
        self.outgoing
            .send_response(request_id, run.response.clone())
            .await;
        self.background_tasks.spawn(async move {
            run.execute().await;
        });
        Ok(None)
    }

    async fn thread_external_agent_start_inner(
        &self,
        params: ThreadExternalAgentStartParams,
    ) -> Result<ExternalAgentRun, JSONRPCErrorError> {
        let ThreadExternalAgentStartParams {
            thread_id,
            runtime_id,
            task,
            mode,
        } = params;
        let runtime_id = runtime_id.trim();
        let task = task.trim();
        if runtime_id.is_empty() {
            return Err(invalid_request("runtimeId must not be empty"));
        }
        if runtime_id == "grok" {
            return Err(invalid_request(
                "use runtimeId `grok-build` for Grok Build external-agent runs",
            ));
        }
        if !matches!(runtime_id, "cursor" | "grok-build") {
            return Err(invalid_request(format!(
                "unsupported external-agent runtime `{runtime_id}`"
            )));
        }
        if task.is_empty() {
            return Err(invalid_request("task must not be empty"));
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
        let harness = harness_for_runtime(runtime_id).ok_or_else(|| {
            invalid_request(format!("unsupported external-agent runtime `{runtime_id}`"))
        })?;
        let sandbox_config = ExternalAgentSandboxConfig {
            permission_profile: codex_protocol::models::PermissionProfile::read_only(),
            codex_linux_sandbox_exe: self.arg0_paths.codex_linux_sandbox_exe.clone(),
            use_legacy_landlock: self.config.features.use_legacy_landlock(),
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

        Ok(ExternalAgentRun {
            runtime_id: runtime_id.to_string(),
            mode,
            task: runtime_request.task.clone(),
            request: runtime_request,
            harness,
            sandbox_config,
            host: AppServerExternalAgentHost::new(
                self.outgoing.clone(),
                thread_id.to_string(),
                response.run_id.clone().unwrap_or_default(),
            ),
            response,
        })
    }
}

struct ExternalAgentRun {
    runtime_id: String,
    mode: ThreadExternalAgentMode,
    task: String,
    request: ExternalAgentRequest,
    harness: AcpStdioHarness,
    sandbox_config: ExternalAgentSandboxConfig,
    host: AppServerExternalAgentHost,
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
            .harness
            .run_sandboxed(self.request, self.host.clone(), &self.sandbox_config)
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
    }
}

#[derive(Clone)]
struct AppServerExternalAgentHost {
    outgoing: Arc<OutgoingMessageSender>,
    thread_id: String,
    run_id: String,
    terminal_sent: Arc<AtomicBool>,
}

impl AppServerExternalAgentHost {
    fn new(outgoing: Arc<OutgoingMessageSender>, thread_id: String, run_id: String) -> Self {
        Self {
            outgoing,
            thread_id,
            run_id,
            terminal_sent: Arc::new(AtomicBool::new(false)),
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
        false
    }
}

fn harness_for_runtime(runtime_id: &str) -> Option<AcpStdioHarness> {
    match runtime_id {
        ExternalAgentRuntimeId::CURSOR => cursor_acp_harness(),
        ExternalAgentRuntimeId::GROK_BUILD => grok_build_acp_harness(),
        ExternalAgentRuntimeId::CLAUDE => None,
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
