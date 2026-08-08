use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_background_agent::BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION;
use codex_background_agent::BACKGROUND_AGENT_RUNTIME_COMPATIBILITY_FINGERPRINT;
use codex_core::exec::ExecCapturePolicy;
use codex_core::exec::ExecExpiration;
use codex_core::exec::ExecParams;
use codex_core::exec::process_exec_tool_call;
use codex_core::sandboxing::SandboxPermissions;
use codex_protocol::ThreadId;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_shell_command::shell_detect::ShellType;
use codex_state::StateRuntime;
use codex_state::WorkflowGoalPlanProjectionOutcome;
use codex_state::WorkflowGoalPlanProjectionParams;
use codex_state::WorkflowRunAdvanceParams;
use codex_state::WorkflowRunBranchAdmissionParams;
use codex_state::WorkflowRunBranchReconcileParams;
use codex_state::WorkflowRunClaimParams;
use codex_state::WorkflowRunCreateParams;
use codex_state::WorkflowRunFenceParams;
use codex_state::WorkflowRunHeartbeatParams;
use codex_state::WorkflowRunSnapshot;
use codex_state::WorkflowRunStatus;
use codex_state::WorkflowRunVerifierClaimOutcome;
use codex_state::WorkflowRunVerifierClaimParams;
use codex_state::WorkflowRunVerifierClaimSelection;
use codex_state::WorkflowRunVerifierOutcomeStatus;
use codex_state::WorkflowRunVerifierRecordResultParams;
use codex_state::WorkflowRunVerifierResultSummary;
use codex_state::busy_retry::retry_on_busy;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_workflows::WorkflowVerifier;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const WORKFLOW_SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(250);
const WORKFLOW_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct WorkflowActivationConfig {
    pub auth_profile_ref: Option<String>,
    pub permission_profile: PermissionProfile,
    pub codex_linux_sandbox_exe: Option<PathBuf>,
    pub use_legacy_landlock: bool,
    pub windows_sandbox_level: WindowsSandboxLevel,
    pub windows_sandbox_private_desktop: bool,
    pub max_active_background_agent_runs: Option<i64>,
}

impl Default for WorkflowActivationConfig {
    fn default() -> Self {
        Self {
            auth_profile_ref: None,
            permission_profile: PermissionProfile::read_only(),
            codex_linux_sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
            max_active_background_agent_runs: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkflowStartRequest {
    pub workflow_record_id: String,
    pub source_thread_id: ThreadId,
    pub idempotency_key: Option<String>,
    pub activation_config: WorkflowActivationConfig,
}

#[derive(Debug, Clone)]
pub struct WorkflowStartOutcome {
    pub snapshot: WorkflowRunSnapshot,
    pub goal_plan: Option<WorkflowGoalPlanProjectionOutcome>,
}

#[derive(Clone)]
pub struct WorkflowActivationService {
    state_db: Arc<StateRuntime>,
    owner_instance_id: Arc<str>,
    active_supervisors: Arc<Mutex<HashSet<String>>>,
}

impl WorkflowActivationService {
    pub fn new(state_db: Arc<StateRuntime>) -> Self {
        Self {
            state_db,
            owner_instance_id: Arc::from(format!("workflow-activation:{}", ThreadId::new())),
            active_supervisors: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn start_workflow_run(
        &self,
        request: WorkflowStartRequest,
    ) -> anyhow::Result<WorkflowStartOutcome> {
        let create_params = WorkflowRunCreateParams {
            workflow_record_id: request.workflow_record_id,
            source_thread_id: Some(request.source_thread_id),
            idempotency_key: request.idempotency_key.clone(),
        };
        let snapshot = if create_params.idempotency_key.is_some() {
            retry_on_busy("create workflow run", || {
                self.state_db
                    .workflows()
                    .create_workflow_run(create_params.clone())
            })
            .await?
        } else {
            self.state_db
                .workflows()
                .create_workflow_run(create_params)
                .await?
        };
        let projection_params = WorkflowGoalPlanProjectionParams {
            workflow_run_id: snapshot.run.run_id.clone(),
            thread_id: request.source_thread_id,
            idempotency_key: request.idempotency_key,
        };
        let goal_plan = retry_on_busy("project workflow run to goal plan", || {
            self.state_db
                .project_workflow_run_to_goal_plan(projection_params.clone())
        })
        .await?;
        self.activate(snapshot.run.run_id.clone(), request.activation_config)
            .await;
        Ok(WorkflowStartOutcome {
            snapshot,
            goal_plan,
        })
    }

    pub async fn activate_thread_runs(
        &self,
        thread_id: ThreadId,
        config: WorkflowActivationConfig,
    ) -> anyhow::Result<()> {
        let mut cursor = None;
        loop {
            let page = self
                .state_db
                .workflows()
                .list_thread_workflow_runs_page(
                    thread_id,
                    cursor,
                    codex_state::DEFAULT_THREAD_WORKFLOW_RUN_LIST_LIMIT,
                )
                .await?;
            for snapshot in page.data {
                if !snapshot.run.status.is_terminal()
                    && snapshot.run.status != WorkflowRunStatus::Paused
                {
                    self.activate(snapshot.run.run_id, config.clone()).await;
                }
            }
            let Some(next_cursor) = page.next_cursor else {
                return Ok(());
            };
            cursor = Some(next_cursor.parse::<u32>()?);
        }
    }

    pub async fn activate(&self, run_id: String, config: WorkflowActivationConfig) {
        let mut active_supervisors = self.active_supervisors.lock().await;
        if !active_supervisors.insert(run_id.clone()) {
            return;
        }
        drop(active_supervisors);

        let service = self.clone();
        tokio::spawn(async move {
            loop {
                match service
                    .run_supervisor(run_id.as_str(), config.clone())
                    .await
                {
                    Ok(()) => break,
                    Err(err) => {
                        tracing::warn!(
                            workflow_run_id = %run_id,
                            "workflow activation supervisor retrying after error: {err}"
                        );
                        tokio::time::sleep(WORKFLOW_SUPERVISOR_POLL_INTERVAL).await;
                    }
                }
            }
            service.active_supervisors.lock().await.remove(&run_id);
        });
    }

    async fn run_supervisor(
        &self,
        run_id: &str,
        config: WorkflowActivationConfig,
    ) -> anyhow::Result<()> {
        loop {
            let Some(snapshot) = self
                .state_db
                .workflows()
                .get_workflow_run_snapshot(run_id)
                .await?
            else {
                return Ok(());
            };
            if snapshot.run.status.is_terminal() {
                return Ok(());
            }
            if snapshot.run.status == WorkflowRunStatus::Paused {
                tokio::time::sleep(WORKFLOW_SUPERVISOR_POLL_INTERVAL).await;
                continue;
            }

            let Some(claim) = self
                .state_db
                .claim_workflow_run(WorkflowRunClaimParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    lease_duration_ms: None,
                })
                .await?
            else {
                tokio::time::sleep(WORKFLOW_SUPERVISOR_POLL_INTERVAL).await;
                continue;
            };
            self.drive_generation(run_id, claim.generation, &config)
                .await?;
        }
    }

    async fn drive_generation(
        &self,
        run_id: &str,
        generation: i64,
        config: &WorkflowActivationConfig,
    ) -> anyhow::Result<()> {
        loop {
            if self
                .state_db
                .heartbeat_workflow_run(WorkflowRunHeartbeatParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    generation,
                    lease_duration_ms: None,
                })
                .await?
                .is_none()
            {
                return Ok(());
            }

            let Some(advanced) = self
                .state_db
                .advance_workflow_run(WorkflowRunAdvanceParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    generation,
                })
                .await?
            else {
                return Ok(());
            };
            if advanced.snapshot.run.status.is_terminal()
                || advanced.snapshot.run.status == WorkflowRunStatus::Paused
            {
                return Ok(());
            }

            let Some(reconciled) = self
                .state_db
                .reconcile_workflow_run_branches(WorkflowRunBranchReconcileParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    generation,
                })
                .await?
            else {
                return Ok(());
            };
            if reconciled.snapshot.run.status.is_terminal()
                || reconciled.snapshot.run.status == WorkflowRunStatus::Paused
            {
                return Ok(());
            }

            let fingerprint = activation_config_fingerprint(config)?;
            let Some(admitted) = self
                .state_db
                .admit_workflow_run_branches(WorkflowRunBranchAdmissionParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    generation,
                    auth_profile_ref: config.auth_profile_ref.clone(),
                    config_fingerprint: Some(fingerprint),
                    version_fingerprint: Some(
                        BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION.to_string(),
                    ),
                    runtime_package_fingerprint: Some(
                        BACKGROUND_AGENT_RUNTIME_COMPATIBILITY_FINGERPRINT.to_string(),
                    ),
                    parent_agent_run_id: None,
                    max_active_background_agent_runs: config.max_active_background_agent_runs,
                })
                .await?
            else {
                return Ok(());
            };
            if admitted.snapshot.run.status.is_terminal()
                || admitted.snapshot.run.status == WorkflowRunStatus::Paused
            {
                return Ok(());
            }

            if let Some(claimed_verifier) = self
                .state_db
                .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    generation,
                    selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
                })
                .await?
            {
                if !self
                    .execute_verifier(run_id, generation, claimed_verifier, config)
                    .await?
                {
                    return Ok(());
                }
                continue;
            }

            if !(advanced.changed || reconciled.changed || admitted.changed) {
                tokio::time::sleep(WORKFLOW_SUPERVISOR_POLL_INTERVAL).await;
            }
        }
    }

    async fn execute_verifier(
        &self,
        run_id: &str,
        generation: i64,
        claimed: WorkflowRunVerifierClaimOutcome,
        config: &WorkflowActivationConfig,
    ) -> anyhow::Result<bool> {
        let started = std::time::Instant::now();
        let definition: WorkflowVerifier = match serde_json::from_value(
            workflow_state_data(&claimed.verifier.definition_json).clone(),
        ) {
            Ok(definition) => definition,
            Err(err) => {
                tracing::warn!(
                    workflow_run_id = %run_id,
                    verifier_run_id = %claimed.verifier.verifier_run_id,
                    "workflow verifier definition is invalid: {err}"
                );
                return self
                    .record_failed_verifier_setup(
                        run_id, generation, claimed, started, /*expected_exit_code*/ None,
                    )
                    .await;
            }
        };
        let expected_exit_code = definition.expected_exit_code.unwrap_or(0);
        let execution_root = match verifier_execution_root(&claimed) {
            Ok(execution_root) => execution_root,
            Err(err) => {
                tracing::warn!(
                    workflow_run_id = %run_id,
                    verifier_run_id = %claimed.verifier.verifier_run_id,
                    "workflow verifier execution root is unavailable: {err}"
                );
                return self
                    .record_failed_verifier_setup(
                        run_id,
                        generation,
                        claimed,
                        started,
                        Some(expected_exit_code),
                    )
                    .await;
            }
        };
        let cwd = match verifier_cwd(&execution_root, definition.cwd.as_deref()) {
            Ok(cwd) => cwd,
            Err(err) => {
                tracing::warn!(
                    workflow_run_id = %run_id,
                    verifier_run_id = %claimed.verifier.verifier_run_id,
                    "workflow verifier cwd is invalid: {err}"
                );
                return self
                    .record_failed_verifier_setup(
                        run_id,
                        generation,
                        claimed,
                        started,
                        Some(expected_exit_code),
                    )
                    .await;
            }
        };
        let permission_profile = match verifier_permission_profile(
            &config.permission_profile,
            definition.sandbox.as_deref(),
            definition.network.as_deref(),
            &execution_root,
        ) {
            Ok(permission_profile) => permission_profile,
            Err(err) => {
                tracing::warn!(
                    workflow_run_id = %run_id,
                    verifier_run_id = %claimed.verifier.verifier_run_id,
                    "workflow verifier sandbox request is invalid: {err}"
                );
                return self
                    .record_failed_verifier_setup(
                        run_id,
                        generation,
                        claimed,
                        started,
                        Some(expected_exit_code),
                    )
                    .await;
            }
        };
        let timeout = Duration::from_secs(definition.timeout_seconds.unwrap_or(1));
        let output_limit =
            usize::try_from(definition.output_limit_bytes.unwrap_or(1)).unwrap_or(usize::MAX);
        let cancellation = CancellationToken::new();
        let fence_lost = Arc::new(AtomicBool::new(false));
        let heartbeat = self.spawn_verifier_heartbeat(
            run_id.to_string(),
            generation,
            cancellation.clone(),
            Arc::clone(&fence_lost),
        );

        let mut command_count = 0_i64;
        let mut observed_exit_code = None;
        let mut timed_out = false;
        let mut output_bytes = 0_usize;
        let mut output_truncated = false;
        let mut combined_stdout = String::new();
        let mut passed = true;

        for command in &definition.commands {
            if fence_lost.load(Ordering::Relaxed) {
                cancellation.cancel();
                let _ = heartbeat.await;
                return Ok(false);
            }
            command_count = command_count.saturating_add(1);
            let output = process_exec_tool_call(
                ExecParams {
                    command: verifier_shell_command(command),
                    cwd: cwd.clone(),
                    expiration: ExecExpiration::TimeoutOrCancellation {
                        timeout,
                        cancellation: cancellation.clone(),
                    },
                    capture_policy: ExecCapturePolicy::ShellTool,
                    env: HashMap::new(),
                    network: None,
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    windows_sandbox_level: config.windows_sandbox_level,
                    windows_sandbox_private_desktop: config.windows_sandbox_private_desktop,
                    justification: None,
                    arg0: None,
                },
                &permission_profile,
                &execution_root,
                std::slice::from_ref(&execution_root),
                &config.codex_linux_sandbox_exe,
                config.use_legacy_landlock,
                /*stdout_stream*/ None,
            )
            .await;
            let output = match output {
                Ok(output) => output,
                Err(err) => {
                    tracing::warn!(
                        workflow_run_id = %run_id,
                        verifier_run_id = %claimed.verifier.verifier_run_id,
                        "workflow verifier command failed to execute: {err}"
                    );
                    passed = false;
                    break;
                }
            };
            observed_exit_code = Some(output.exit_code);
            timed_out |= output.timed_out;
            combined_stdout.push_str(output.stdout.text.as_str());
            output_bytes = output_bytes
                .saturating_add(output.stdout.text.len())
                .saturating_add(output.stderr.text.len());
            output_truncated |= output.stdout.truncated_after_lines.is_some()
                || output.stderr.truncated_after_lines.is_some()
                || output.aggregated_output.truncated_after_lines.is_some()
                || output_bytes > output_limit;
            if output.exit_code != expected_exit_code || output.timed_out || output_truncated {
                passed = false;
                break;
            }
        }
        if let Some(expected_stdout) = definition.expected_stdout.as_deref()
            && combined_stdout != expected_stdout
        {
            passed = false;
        }

        cancellation.cancel();
        let _ = heartbeat.await;
        if fence_lost.load(Ordering::Relaxed)
            || !self
                .state_db
                .workflow_run_fence_is_current(WorkflowRunFenceParams {
                    run_id: run_id.to_string(),
                    owner_id: self.owner_instance_id.to_string(),
                    generation,
                })
                .await?
        {
            return Ok(false);
        }

        let recorded = self
            .state_db
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
                verifier_run_id: claimed.verifier.verifier_run_id,
                outcome: if passed {
                    WorkflowRunVerifierOutcomeStatus::Passed
                } else {
                    WorkflowRunVerifierOutcomeStatus::Failed
                },
                summary: WorkflowRunVerifierResultSummary {
                    command_count,
                    expected_exit_code: Some(expected_exit_code),
                    observed_exit_code,
                    timed_out,
                    duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
                    output_bytes: i64::try_from(output_bytes).unwrap_or(i64::MAX),
                    output_truncated,
                },
            })
            .await?;
        Ok(recorded.is_some())
    }

    async fn record_failed_verifier_setup(
        &self,
        run_id: &str,
        generation: i64,
        claimed: WorkflowRunVerifierClaimOutcome,
        started: std::time::Instant,
        expected_exit_code: Option<i32>,
    ) -> anyhow::Result<bool> {
        let recorded = self
            .state_db
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
                verifier_run_id: claimed.verifier.verifier_run_id,
                outcome: WorkflowRunVerifierOutcomeStatus::Failed,
                summary: WorkflowRunVerifierResultSummary {
                    command_count: 0,
                    expected_exit_code,
                    observed_exit_code: None,
                    timed_out: false,
                    duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
                    output_bytes: 0,
                    output_truncated: false,
                },
            })
            .await?;
        Ok(recorded.is_some())
    }

    fn spawn_verifier_heartbeat(
        &self,
        run_id: String,
        generation: i64,
        cancellation: CancellationToken,
        fence_lost: Arc<AtomicBool>,
    ) -> tokio::task::JoinHandle<()> {
        let state_db = Arc::clone(&self.state_db);
        let owner_instance_id = Arc::clone(&self.owner_instance_id);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => return,
                    _ = tokio::time::sleep(WORKFLOW_HEARTBEAT_INTERVAL) => {}
                }
                match state_db
                    .heartbeat_workflow_run(WorkflowRunHeartbeatParams {
                        run_id: run_id.clone(),
                        owner_id: owner_instance_id.to_string(),
                        generation,
                        lease_duration_ms: None,
                    })
                    .await
                {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => {
                        fence_lost.store(true, Ordering::Relaxed);
                        cancellation.cancel();
                        return;
                    }
                }
            }
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowActivationFingerprint<'a> {
    auth_profile_ref: Option<&'a str>,
    admission_schema: &'static str,
}

fn activation_config_fingerprint(config: &WorkflowActivationConfig) -> anyhow::Result<String> {
    let value = WorkflowActivationFingerprint {
        auth_profile_ref: config.auth_profile_ref.as_deref(),
        admission_schema: BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION,
    };
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

fn workflow_state_data(value: &serde_json::Value) -> &serde_json::Value {
    value.get("data").unwrap_or(value)
}

fn verifier_execution_root(
    claimed: &WorkflowRunVerifierClaimOutcome,
) -> anyhow::Result<AbsolutePathBuf> {
    let admission = claimed
        .step
        .branch_admission_json
        .as_ref()
        .map(workflow_state_data);
    let cwd = admission
        .and_then(|value| value.get("cwd"))
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .or_else(|| claimed.run.source_cwd.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "workflow verifier {} has no persisted execution cwd",
                claimed.verifier.verifier_run_id
            )
        })?;
    AbsolutePathBuf::try_from(cwd)
        .map_err(|err| anyhow::anyhow!("workflow verifier execution cwd is invalid: {err}"))
}

fn verifier_cwd(
    execution_root: &AbsolutePathBuf,
    requested_cwd: Option<&str>,
) -> anyhow::Result<AbsolutePathBuf> {
    let requested_cwd = Path::new(requested_cwd.unwrap_or("."));
    if requested_cwd.is_absolute() {
        anyhow::bail!("workflow verifier cwd must be relative to the admitted workspace");
    }
    let root = std::fs::canonicalize(execution_root.as_path())?;
    let resolved = std::fs::canonicalize(root.join(requested_cwd))?;
    if !resolved.starts_with(root.as_path()) {
        anyhow::bail!("workflow verifier cwd escapes the admitted workspace");
    }
    AbsolutePathBuf::try_from(resolved)
        .map_err(|err| anyhow::anyhow!("workflow verifier cwd is invalid: {err}"))
}

fn verifier_permission_profile(
    configured: &PermissionProfile,
    sandbox: Option<&str>,
    network: Option<&str>,
    execution_root: &AbsolutePathBuf,
) -> anyhow::Result<PermissionProfile> {
    let profile = match sandbox.unwrap_or("default") {
        "read-only" | "read_only" => PermissionProfile::read_only(),
        "workspace-write" | "workspace_write" => PermissionProfile::workspace_write(),
        "default" => configured.clone(),
        other => anyhow::bail!("unsupported workflow verifier sandbox `{other}`"),
    };
    let configured_network = configured.network_sandbox_policy();
    let network = match network.unwrap_or("default") {
        "disabled" | "restricted" => NetworkSandboxPolicy::Restricted,
        "enabled" if configured_network.is_enabled() => NetworkSandboxPolicy::Enabled,
        "enabled" => {
            anyhow::bail!(
                "workflow verifier requested network access that the runtime configuration does not allow"
            )
        }
        "default" => configured_network,
        other => anyhow::bail!("unsupported workflow verifier network policy `{other}`"),
    };
    let (file_system, _) = profile.to_runtime_permissions();
    Ok(
        PermissionProfile::from_runtime_permissions(&file_system, network)
            .materialize_project_roots_with_workspace_roots(std::slice::from_ref(execution_root)),
    )
}

fn verifier_shell_command(command: &str) -> Vec<String> {
    let shell = codex_shell_command::shell_detect::default_user_shell();
    let shell_path = shell.shell_path.to_string_lossy().into_owned();
    match shell.shell_type {
        ShellType::PowerShell => vec![
            shell_path,
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            command.to_string(),
        ],
        ShellType::Cmd => vec![shell_path, "/C".to_string(), command.to_string()],
        ShellType::Zsh | ShellType::Bash | ShellType::Sh => {
            vec![shell_path, "-lc".to_string(), command.to_string()]
        }
    }
}
