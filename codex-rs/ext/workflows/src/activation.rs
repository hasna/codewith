use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
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
use codex_workflows::WorkflowProviderCreditControl;
use codex_workflows::WorkflowModelRoute;
use codex_workflows::WorkflowRouteReceipt;
use codex_workflows::WorkflowRouteRuntime;
use codex_workflows::WorkflowWorkspaceMode;
use codex_workflows::admit_workflow_model_route_for_runtime;
use codex_workflows::parse_workflow_yaml;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const WORKFLOW_SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(250);
const WORKFLOW_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

#[cfg(target_os = "linux")]
const VERIFIER_SANDBOX_SUPPORT_ENV_VARS: [&str; 5] = [
    "CARGO_BIN_EXE_bwrap",
    "RUNFILES_DIR",
    "TEST_SRCDIR",
    "RUNFILES_MANIFEST_FILE",
    "TEST_WORKSPACE",
];

#[cfg(target_os = "linux")]
fn verifier_sandbox_support_env() -> HashMap<String, String> {
    verifier_sandbox_support_env_from(|key| std::env::var(key).ok())
}

#[cfg(target_os = "linux")]
fn verifier_sandbox_support_env_from(
    mut read_var: impl FnMut(&str) -> Option<String>,
) -> HashMap<String, String> {
    VERIFIER_SANDBOX_SUPPORT_ENV_VARS
        .into_iter()
        .filter_map(|key| read_var(key).map(|value| (key.to_string(), value)))
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn verifier_sandbox_support_env() -> HashMap<String, String> {
    HashMap::new()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowStateOperation {
    CreateRun,
    ProjectGoalPlan,
    ListThreadRuns,
    LoadSnapshot,
    ClaimRun,
    HeartbeatRun,
    AdvanceRun,
    ReconcileBranches,
    AdmitBranches,
    ClaimVerifier,
    CheckVerifierFence,
    RecordVerifierResult,
}

impl WorkflowStateOperation {
    #[cfg(test)]
    const ALL: [Self; 12] = [
        Self::CreateRun,
        Self::ProjectGoalPlan,
        Self::ListThreadRuns,
        Self::LoadSnapshot,
        Self::ClaimRun,
        Self::HeartbeatRun,
        Self::AdvanceRun,
        Self::ReconcileBranches,
        Self::AdmitBranches,
        Self::ClaimVerifier,
        Self::CheckVerifierFence,
        Self::RecordVerifierResult,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::CreateRun => "create workflow run",
            Self::ProjectGoalPlan => "project workflow run to goal plan",
            Self::ListThreadRuns => "list active thread workflow runs",
            Self::LoadSnapshot => "load workflow run snapshot",
            Self::ClaimRun => "claim workflow run",
            Self::HeartbeatRun => "heartbeat workflow run",
            Self::AdvanceRun => "advance workflow run",
            Self::ReconcileBranches => "reconcile workflow run branches",
            Self::AdmitBranches => "admit workflow run branches",
            Self::ClaimVerifier => "claim workflow run verifier",
            Self::CheckVerifierFence => "check workflow verifier fence",
            Self::RecordVerifierResult => "record workflow verifier result",
        }
    }
}

async fn retry_workflow_state<T, F, Fut>(
    operation: WorkflowStateOperation,
    f: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    retry_on_busy(operation.label(), f).await
}

#[derive(Debug, Clone)]
pub struct WorkflowActivationConfig {
    pub auth_profile_ref: Option<String>,
    pub permission_profile: PermissionProfile,
    pub route_runtime: WorkflowRouteRuntime,
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
            route_runtime: WorkflowRouteRuntime {
                model_gateway: None,
                provider: None,
                model: None,
                reasoning: None,
                service_tier: None,
                auth_profile: None,
                approval_policy: None,
                permission_profile: None,
                context_window_tokens: None,
                credit_control: WorkflowProviderCreditControl::Unavailable,
            },
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
        let spec_record = self
            .state_db
            .workflows()
            .get_workflow_spec(request.workflow_record_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("workflow spec record not found"))?;
        let spec = parse_workflow_yaml(spec_record.source_yaml.as_str())?;
        validate_workflow_routes_before_effects(&spec, &request.activation_config.route_runtime)?;
        let create_params = WorkflowRunCreateParams {
            workflow_record_id: request.workflow_record_id,
            source_thread_id: Some(request.source_thread_id),
            idempotency_key: request.idempotency_key.clone(),
        };
        let snapshot = if create_params.idempotency_key.is_some() {
            retry_workflow_state(WorkflowStateOperation::CreateRun, || {
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
        let goal_plan = retry_workflow_state(WorkflowStateOperation::ProjectGoalPlan, || {
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
            let page = retry_workflow_state(WorkflowStateOperation::ListThreadRuns, || {
                self.state_db.workflows().list_thread_workflow_runs_page(
                    thread_id,
                    cursor,
                    codex_state::DEFAULT_THREAD_WORKFLOW_RUN_LIST_LIMIT,
                )
            })
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
            let Some(snapshot) = retry_workflow_state(WorkflowStateOperation::LoadSnapshot, || {
                self.state_db.workflows().get_workflow_run_snapshot(run_id)
            })
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

            let claim_params = WorkflowRunClaimParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                lease_duration_ms: None,
            };
            let Some(claim) = retry_workflow_state(WorkflowStateOperation::ClaimRun, || {
                self.state_db.claim_workflow_run(claim_params.clone())
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
        let permission_profile_json =
            serde_json::to_value(&config.permission_profile).map_err(|err| {
                anyhow::anyhow!("failed to serialize workflow permission profile: {err}")
            })?;
        loop {
            let heartbeat_params = WorkflowRunHeartbeatParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
                lease_duration_ms: None,
            };
            if retry_workflow_state(WorkflowStateOperation::HeartbeatRun, || {
                self.state_db
                    .heartbeat_workflow_run(heartbeat_params.clone())
            })
            .await?
            .is_none()
            {
                return Ok(());
            }

            let advance_params = WorkflowRunAdvanceParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
            };
            let Some(advanced) = retry_workflow_state(WorkflowStateOperation::AdvanceRun, || {
                self.state_db.advance_workflow_run(advance_params.clone())
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

            let reconcile_params = WorkflowRunBranchReconcileParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
            };
            let Some(reconciled) =
                retry_workflow_state(WorkflowStateOperation::ReconcileBranches, || {
                    self.state_db
                        .reconcile_workflow_run_branches(reconcile_params.clone())
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
            let admission_params = WorkflowRunBranchAdmissionParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
                auth_profile_ref: config.auth_profile_ref.clone(),
                config_fingerprint: Some(fingerprint),
                version_fingerprint: Some(BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION.to_string()),
                runtime_package_fingerprint: Some(
                    BACKGROUND_AGENT_RUNTIME_COMPATIBILITY_FINGERPRINT.to_string(),
                ),
                permission_profile_json: permission_profile_json.clone(),
                route_runtime: config.route_runtime.clone(),
                parent_agent_run_id: None,
                max_active_background_agent_runs: config.max_active_background_agent_runs,
            };
            let Some(admitted) =
                retry_workflow_state(WorkflowStateOperation::AdmitBranches, || {
                    self.state_db
                        .admit_workflow_run_branches(admission_params.clone())
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

            let verifier_claim_params = WorkflowRunVerifierClaimParams {
                run_id: run_id.to_string(),
                owner_id: self.owner_instance_id.to_string(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
            };
            if let Some(claimed_verifier) =
                retry_workflow_state(WorkflowStateOperation::ClaimVerifier, || {
                    self.state_db
                        .claim_workflow_run_verifier(verifier_claim_params.clone())
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
        if let Err(err) = validate_verifier_route(&claimed, &config.route_runtime) {
            tracing::warn!(
                workflow_run_id = %run_id,
                verifier_run_id = %claimed.verifier.verifier_run_id,
                "workflow verifier route is not admitted: {err}"
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
            if let Err(err) = validate_verifier_route(&claimed, &config.route_runtime) {
                tracing::warn!(
                    workflow_run_id = %run_id,
                    verifier_run_id = %claimed.verifier.verifier_run_id,
                    "workflow verifier route changed before command execution: {err}"
                );
                passed = false;
                break;
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
                    // Verifier commands otherwise run with an empty environment.
                    // Preserve that credential-zero boundary while forwarding
                    // only Bazel's non-secret runfile paths so the Linux sandbox
                    // helper can locate the bundled bwrap binary in hosted tests.
                    env: verifier_sandbox_support_env(),
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
        let fence_params = WorkflowRunFenceParams {
            run_id: run_id.to_string(),
            owner_id: self.owner_instance_id.to_string(),
            generation,
        };
        if fence_lost.load(Ordering::Relaxed)
            || !retry_workflow_state(WorkflowStateOperation::CheckVerifierFence, || {
                self.state_db
                    .workflow_run_fence_is_current(fence_params.clone())
            })
            .await?
        {
            return Ok(false);
        }

        let record_params = WorkflowRunVerifierRecordResultParams {
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
        };
        let recorded = retry_workflow_state(WorkflowStateOperation::RecordVerifierResult, || {
            self.state_db
                .record_workflow_run_verifier_result(record_params.clone())
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
        let record_params = WorkflowRunVerifierRecordResultParams {
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
        };
        let recorded = retry_workflow_state(WorkflowStateOperation::RecordVerifierResult, || {
            self.state_db
                .record_workflow_run_verifier_result(record_params.clone())
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
                let heartbeat_params = WorkflowRunHeartbeatParams {
                    run_id: run_id.clone(),
                    owner_id: owner_instance_id.to_string(),
                    generation,
                    lease_duration_ms: None,
                };
                match retry_workflow_state(WorkflowStateOperation::HeartbeatRun, || {
                    state_db.heartbeat_workflow_run(heartbeat_params.clone())
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

fn validate_verifier_route(
    claimed: &WorkflowRunVerifierClaimOutcome,
    runtime: &WorkflowRouteRuntime,
) -> anyhow::Result<WorkflowRouteReceipt> {
    let requested_value = claimed.step.model_route_json.as_ref().ok_or_else(|| {
        anyhow::anyhow!("workflow_route_receipt_missing: verifier step has no model route")
    })?;
    let requested = serde_json::from_value::<WorkflowModelRoute>(
        workflow_state_data(requested_value).clone(),
    )?;
    let admitted_value = claimed
        .step
        .branch_admission_json
        .as_ref()
        .and_then(|admission| workflow_state_data(admission).get("routeReceipt"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "workflow_route_receipt_missing: verifier step has no admitted route receipt"
            )
        })?;
    let admitted = serde_json::from_value::<WorkflowRouteReceipt>(admitted_value.clone())?;
    let worktree_mode = match claimed
        .step
        .workspace_json
        .as_ref()
        .map(workflow_state_data)
        .and_then(|workspace| workspace.get("mode"))
        .and_then(serde_json::Value::as_str)
    {
        Some("isolated_worktree") => "isolated_worktree",
        Some("shared_repository") => "shared_repository",
        Some(other) => anyhow::bail!(
            "workflow_route_worktree_mode_invalid: unsupported verifier worktree mode `{other}`"
        ),
        None => "shared_repository",
    };
    let current = admit_workflow_model_route_for_runtime(&requested, runtime, worktree_mode)?;
    if current != admitted {
        anyhow::bail!(
            "workflow_route_receipt_mismatch: verifier route differs from branch admission"
        );
    }
    admitted.enforce_provider_attempt(&current.effective)?;
    Ok(admitted)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowActivationFingerprint<'a> {
    auth_profile_ref: Option<&'a str>,
    route_runtime: &'a WorkflowRouteRuntime,
    admission_schema: &'static str,
}

fn activation_config_fingerprint(config: &WorkflowActivationConfig) -> anyhow::Result<String> {
    let value = WorkflowActivationFingerprint {
        auth_profile_ref: config.auth_profile_ref.as_deref(),
        route_runtime: &config.route_runtime,
        admission_schema: BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION,
    };
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

fn validate_workflow_routes_before_effects(
    spec: &codex_workflows::WorkflowSpec,
    runtime: &WorkflowRouteRuntime,
) -> anyhow::Result<()> {
    for step in &spec.steps {
        let path = format!("steps.{}.model", step.id);
        let route = step.model.as_ref().unwrap_or(&spec.execution_defaults);
        let workspace_mode = step
            .workspace
            .as_ref()
            .map_or(WorkflowWorkspaceMode::SharedRepository, |workspace| workspace.mode);
        let worktree_mode = match workspace_mode {
            WorkflowWorkspaceMode::IsolatedWorktree => "isolated_worktree",
            WorkflowWorkspaceMode::SharedRepository => "shared_repository",
        };
        admit_workflow_model_route_for_runtime(route, runtime, worktree_mode)
            .map_err(|err| anyhow::anyhow!("{path}: {err}"))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn workflow_state_operation_labels_cover_every_activation_boundary() {
        assert_eq!(
            [
                "create workflow run",
                "project workflow run to goal plan",
                "list active thread workflow runs",
                "load workflow run snapshot",
                "claim workflow run",
                "heartbeat workflow run",
                "advance workflow run",
                "reconcile workflow run branches",
                "admit workflow run branches",
                "claim workflow run verifier",
                "check workflow verifier fence",
                "record workflow verifier result",
            ],
            WorkflowStateOperation::ALL.map(WorkflowStateOperation::label)
        );
    }

    #[tokio::test]
    async fn workflow_state_retry_survives_busy_and_busy_snapshot_contention() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&attempts);
        let result = retry_workflow_state(WorkflowStateOperation::AdmitBranches, move || {
            let observed_attempts = Arc::clone(&observed_attempts);
            async move {
                match observed_attempts.fetch_add(1, Ordering::SeqCst) {
                    0 => Err(anyhow::anyhow!(
                        "error returned from database: (code: 5) database is locked"
                    )),
                    1 => Err(anyhow::anyhow!(
                        "error returned from database: (code: 517) database is locked"
                    )),
                    _ => Ok("admitted"),
                }
            }
        })
        .await
        .expect("activation state operation should retry transient contention");

        assert_eq!("admitted", result);
        assert_eq!(3, attempts.load(Ordering::SeqCst));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn verifier_sandbox_support_env_forwards_only_bazel_bwrap_runtime_paths() {
        let ambient = HashMap::from([
            (
                "CARGO_BIN_EXE_bwrap".to_string(),
                "/tmp/bazel-bin/codex-rs/bwrap/bwrap".to_string(),
            ),
            (
                "RUNFILES_DIR".to_string(),
                "/tmp/app-server.runfiles".to_string(),
            ),
            (
                "TEST_SRCDIR".to_string(),
                "/tmp/app-server.runfiles".to_string(),
            ),
            (
                "RUNFILES_MANIFEST_FILE".to_string(),
                "/tmp/app-server.runfiles/MANIFEST".to_string(),
            ),
            ("TEST_WORKSPACE".to_string(), "_main".to_string()),
            (
                "UNRELATED_ENV".to_string(),
                "must-not-reach-verifier".to_string(),
            ),
        ]);

        let env = verifier_sandbox_support_env_from(|key| ambient.get(key).cloned());

        assert_eq!(
            HashMap::from([
                (
                    "CARGO_BIN_EXE_bwrap".to_string(),
                    "/tmp/bazel-bin/codex-rs/bwrap/bwrap".to_string(),
                ),
                (
                    "RUNFILES_DIR".to_string(),
                    "/tmp/app-server.runfiles".to_string(),
                ),
                (
                    "TEST_SRCDIR".to_string(),
                    "/tmp/app-server.runfiles".to_string(),
                ),
                (
                    "RUNFILES_MANIFEST_FILE".to_string(),
                    "/tmp/app-server.runfiles/MANIFEST".to_string(),
                ),
                ("TEST_WORKSPACE".to_string(), "_main".to_string()),
            ]),
            env
        );
        assert!(!env.contains_key("UNRELATED_ENV"));
    }
}
