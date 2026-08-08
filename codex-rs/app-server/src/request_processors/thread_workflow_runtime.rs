use super::thread_processor::ThreadRequestProcessor;
use codex_protocol::protocol::SandboxPolicy;
use codex_state::StateRuntime;
use codex_state::WorkflowRunAdvanceParams;
use codex_state::WorkflowRunBranchAdmissionParams;
use codex_state::WorkflowRunBranchReconcileParams;
use codex_state::WorkflowRunClaimParams;
use codex_state::WorkflowRunStep;
use codex_state::WorkflowRunStepStatus;
use codex_state::WorkflowRunStepVerifier;
use codex_state::WorkflowRunStepVerifierStatus;
use codex_state::WorkflowRunVerifierClaimParams;
use codex_state::WorkflowRunVerifierClaimSelection;
use codex_state::WorkflowRunVerifierOutcomeStatus;
use codex_state::WorkflowRunVerifierRecordResultParams;
use codex_state::WorkflowRunVerifierResultSummary;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tracing::warn;

const WORKFLOW_RUNTIME_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const WORKFLOW_RUNTIME_RUN_LIMIT: u32 = 200;

#[derive(Clone)]
struct WorkflowRuntimeContext {
    state_db: Arc<StateRuntime>,
    codex_linux_sandbox_exe: Option<PathBuf>,
    use_legacy_landlock: bool,
}

struct VerifierExecution {
    outcome: WorkflowRunVerifierOutcomeStatus,
    summary: WorkflowRunVerifierResultSummary,
}

impl ThreadRequestProcessor {
    pub(crate) fn start_workflow_run_supervisor(&self) {
        let Some(state_db) = self.state_db.clone() else {
            return;
        };
        let context = WorkflowRuntimeContext {
            state_db,
            codex_linux_sandbox_exe: self.arg0_paths.codex_linux_sandbox_exe.clone(),
            use_legacy_landlock: self.config.features.use_legacy_landlock(),
        };
        let cancel_token = self.background_agent_supervisor_token.clone();
        self.background_tasks.spawn(async move {
            let mut interval = tokio::time::interval(WORKFLOW_RUNTIME_RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if let Err(err) = reconcile_workflow_runs(&context).await {
                    warn!("workflow runtime reconcile failed: {err}");
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {}
                }
            }
        });
    }
}

async fn reconcile_workflow_runs(context: &WorkflowRuntimeContext) -> anyhow::Result<()> {
    let run_ids = context
        .state_db
        .list_active_workflow_run_ids(WORKFLOW_RUNTIME_RUN_LIMIT)
        .await?;
    for run_id in run_ids {
        if let Err(err) = reconcile_workflow_run(context, run_id.as_str()).await {
            warn!(run_id, "workflow run reconcile failed: {err}");
        }
    }
    Ok(())
}

async fn reconcile_workflow_run(
    context: &WorkflowRuntimeContext,
    run_id: &str,
) -> anyhow::Result<()> {
    let Some(initial_snapshot) = context
        .state_db
        .workflows()
        .get_workflow_run_snapshot(run_id)
        .await?
    else {
        return Ok(());
    };
    let Some(source_thread_id) = initial_snapshot.run.source_thread_id else {
        return Ok(());
    };
    let owner_id = format!("workflow-manager:{source_thread_id}");
    let Some(claim) = context
        .state_db
        .claim_workflow_run(WorkflowRunClaimParams {
            run_id: run_id.to_string(),
            owner_id: owner_id.clone(),
            lease_duration_ms: None,
        })
        .await?
    else {
        return Ok(());
    };
    let generation = claim.generation;
    let Some(reconciled) = context
        .state_db
        .reconcile_workflow_run_branches(WorkflowRunBranchReconcileParams {
            run_id: run_id.to_string(),
            owner_id: owner_id.clone(),
            generation,
        })
        .await?
    else {
        return Ok(());
    };
    let mut snapshot = reconciled.snapshot;

    loop {
        let Some(verifier) = next_executable_verifier(&snapshot) else {
            break;
        };
        let Some(claimed) = context
            .state_db
            .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                run_id: run_id.to_string(),
                owner_id: owner_id.clone(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::VerifierRunId(
                    verifier.verifier_run_id.clone(),
                ),
            })
            .await?
        else {
            break;
        };
        let execution = execute_verifier(context, &claimed.step, &claimed.verifier).await;
        let Some(recorded) = context
            .state_db
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run_id.to_string(),
                owner_id: owner_id.clone(),
                generation,
                verifier_run_id: claimed.verifier.verifier_run_id,
                outcome: execution.outcome,
                summary: execution.summary,
            })
            .await?
        else {
            break;
        };
        snapshot = recorded.snapshot;
        if execution.outcome == WorkflowRunVerifierOutcomeStatus::Failed {
            break;
        }
    }

    let Some(advanced) = context
        .state_db
        .advance_workflow_run(WorkflowRunAdvanceParams {
            run_id: run_id.to_string(),
            owner_id: owner_id.clone(),
            generation,
        })
        .await?
    else {
        return Ok(());
    };
    if advanced.snapshot.run.status.is_terminal() {
        return Ok(());
    }
    context
        .state_db
        .admit_workflow_run_branches(WorkflowRunBranchAdmissionParams {
            run_id: run_id.to_string(),
            owner_id,
            generation,
            auth_profile_ref: None,
            config_fingerprint: None,
            version_fingerprint: None,
            parent_agent_run_id: None,
            max_active_background_agent_runs: None,
        })
        .await?;
    Ok(())
}

fn next_executable_verifier(
    snapshot: &codex_state::WorkflowRunSnapshot,
) -> Option<&WorkflowRunStepVerifier> {
    snapshot.verifiers.iter().find(|verifier| {
        matches!(
            verifier.status,
            WorkflowRunStepVerifierStatus::Pending | WorkflowRunStepVerifierStatus::Blocked
        ) && snapshot.steps.iter().any(|step| {
            step.step_id == verifier.step_id
                && step.status == WorkflowRunStepStatus::WaitingVerifier
        })
    })
}

async fn execute_verifier(
    context: &WorkflowRuntimeContext,
    step: &WorkflowRunStep,
    verifier: &WorkflowRunStepVerifier,
) -> VerifierExecution {
    let started_at = Instant::now();
    let result = execute_verifier_inner(context, step, verifier).await;
    match result {
        Ok(execution) => execution,
        Err(_) => VerifierExecution {
            outcome: WorkflowRunVerifierOutcomeStatus::Failed,
            summary: WorkflowRunVerifierResultSummary {
                command_count: 0,
                expected_exit_code: verifier_definition(verifier)
                    .get("expected_exit_code")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok()),
                observed_exit_code: None,
                timed_out: false,
                duration_ms: duration_millis(started_at.elapsed()),
                output_bytes: 0,
                output_truncated: false,
            },
        },
    }
}

async fn execute_verifier_inner(
    context: &WorkflowRuntimeContext,
    step: &WorkflowRunStep,
    verifier: &WorkflowRunStepVerifier,
) -> anyhow::Result<VerifierExecution> {
    let background_agent_run_id = step
        .background_agent_run_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("workflow step has no background agent run"))?;
    let status_snapshot = context
        .state_db
        .get_background_agent_status_snapshot(background_agent_run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workflow branch status snapshot is missing"))?;
    let workspace_cwd = status_snapshot
        .payload_json
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("workflow branch status snapshot has no cwd"))?;
    let workspace_cwd = canonical_directory(Path::new(workspace_cwd))?;
    match verifier.verifier_type.as_str() {
        "artifact_contains" => execute_artifact_verifier(&workspace_cwd, verifier).await,
        "run_commands" => execute_command_verifier(context, &workspace_cwd, verifier).await,
        verifier_type => anyhow::bail!("unsupported workflow verifier type `{verifier_type}`"),
    }
}

async fn execute_artifact_verifier(
    workspace_cwd: &Path,
    verifier: &WorkflowRunStepVerifier,
) -> anyhow::Result<VerifierExecution> {
    let started_at = Instant::now();
    let definition = verifier_definition(verifier);
    let artifact = definition
        .get("artifact")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("artifact verifier is missing artifact"))?;
    let artifact_path = resolve_beneath(workspace_cwd, artifact)?;
    let bytes = tokio::fs::read(&artifact_path).await?;
    let content = String::from_utf8_lossy(&bytes);
    let passed = definition
        .get("must_contain")
        .and_then(Value::as_array)
        .is_some_and(|needles| {
            !needles.is_empty()
                && needles
                    .iter()
                    .filter_map(Value::as_str)
                    .all(|needle| content.contains(needle))
        });
    Ok(VerifierExecution {
        outcome: if passed {
            WorkflowRunVerifierOutcomeStatus::Passed
        } else {
            WorkflowRunVerifierOutcomeStatus::Failed
        },
        summary: WorkflowRunVerifierResultSummary {
            command_count: 0,
            expected_exit_code: None,
            observed_exit_code: None,
            timed_out: false,
            duration_ms: duration_millis(started_at.elapsed()),
            output_bytes: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            output_truncated: false,
        },
    })
}

async fn execute_command_verifier(
    context: &WorkflowRuntimeContext,
    workspace_cwd: &Path,
    verifier: &WorkflowRunStepVerifier,
) -> anyhow::Result<VerifierExecution> {
    let started_at = Instant::now();
    let definition = verifier_definition(verifier);
    let verifier_cwd = definition
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("command verifier is missing cwd"))?;
    let verifier_cwd = resolve_beneath(workspace_cwd, verifier_cwd)?;
    let timeout_seconds = definition
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("command verifier is missing timeout_seconds"))?;
    let output_limit_bytes = definition
        .get("output_limit_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("command verifier is missing output_limit_bytes"))?;
    let expected_exit_code = definition
        .get("expected_exit_code")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(0);
    let commands = definition
        .get("commands")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("command verifier is missing commands"))?;
    let permission_profile = verifier_permission_profile(definition, &verifier_cwd)?;
    let absolute_cwd = AbsolutePathBuf::try_from(verifier_cwd.clone())?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let mut command_count = 0_i64;
    let mut observed_exit_code = None;
    let mut output_bytes = 0_i64;
    let mut output_truncated = false;
    let mut timed_out = false;

    for command in commands.iter().filter_map(Value::as_str) {
        command_count += 1;
        let mut child = codex_core::exec::spawn_streaming_command_under_sandbox(
            vec![
                "/bin/bash".to_string(),
                "-lc".to_string(),
                command.to_string(),
            ],
            absolute_cwd.clone(),
            verifier_environment(),
            &permission_profile,
            &absolute_cwd,
            &context.codex_linux_sandbox_exe,
            context.use_legacy_landlock,
        )
        .await?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("verifier stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("verifier stderr was not piped"))?;
        let stdout_task = tokio::spawn(drain_stream(stdout, output_limit_bytes));
        let stderr_task = tokio::spawn(drain_stream(stderr, output_limit_bytes));
        match tokio::time::timeout_at(deadline, child.wait()).await {
            Ok(status) => {
                observed_exit_code = status?.code();
            }
            Err(_) => {
                timed_out = true;
                child.kill().await?;
                let _ = child.wait().await;
            }
        }
        let stdout_result = stdout_task.await??;
        let stderr_result = stderr_task.await??;
        output_bytes = output_bytes
            .saturating_add(stdout_result.0)
            .saturating_add(stderr_result.0);
        output_truncated |= stdout_result.1 || stderr_result.1;
        if timed_out || observed_exit_code != Some(expected_exit_code) {
            break;
        }
    }

    Ok(VerifierExecution {
        outcome: if !timed_out
            && command_count == i64::try_from(commands.len())?
            && observed_exit_code == Some(expected_exit_code)
        {
            WorkflowRunVerifierOutcomeStatus::Passed
        } else {
            WorkflowRunVerifierOutcomeStatus::Failed
        },
        summary: WorkflowRunVerifierResultSummary {
            command_count,
            expected_exit_code: Some(expected_exit_code),
            observed_exit_code,
            timed_out,
            duration_ms: duration_millis(started_at.elapsed()),
            output_bytes,
            output_truncated,
        },
    })
}

fn verifier_definition(verifier: &WorkflowRunStepVerifier) -> &Value {
    verifier
        .definition_json
        .get("data")
        .unwrap_or(&verifier.definition_json)
}

fn verifier_permission_profile(
    definition: &Value,
    cwd: &Path,
) -> anyhow::Result<codex_protocol::models::PermissionProfile> {
    let network_access = match definition
        .get("network")
        .and_then(Value::as_str)
        .unwrap_or("disabled")
    {
        "disabled" | "default" => false,
        "enabled" => true,
        network => anyhow::bail!("unsupported verifier network policy `{network}`"),
    };
    let sandbox_policy = match definition
        .get("sandbox")
        .and_then(Value::as_str)
        .unwrap_or("read-only")
    {
        "default" | "read-only" => SandboxPolicy::ReadOnly { network_access },
        "workspace-write" => SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
        sandbox => anyhow::bail!("unsupported verifier sandbox policy `{sandbox}`"),
    };
    let cwd = AbsolutePathBuf::try_from(cwd.to_path_buf())?;
    Ok(
        codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy_for_cwd(
            &sandbox_policy,
            &cwd,
        ),
    )
}

fn verifier_environment() -> HashMap<String, String> {
    ["HOME", "PATH", "LANG", "LC_ALL", "TERM"]
        .into_iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| (key.to_string(), value))
        })
        .collect()
}

fn canonical_directory(path: &Path) -> anyhow::Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    if !path.is_dir() {
        anyhow::bail!("workflow verifier cwd is not a directory");
    }
    Ok(path)
}

fn resolve_beneath(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("workflow verifier path must stay beneath the branch workspace");
    }
    let candidate = std::fs::canonicalize(root.join(relative_path))?;
    if !candidate.starts_with(root) {
        anyhow::bail!("workflow verifier path escaped the branch workspace");
    }
    Ok(candidate)
}

async fn drain_stream(
    mut stream: impl AsyncRead + Unpin,
    output_limit_bytes: u64,
) -> io::Result<(i64, bool)> {
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    Ok((
        i64::try_from(total).unwrap_or(i64::MAX),
        total > output_limit_bytes,
    ))
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_paths_cannot_escape_workspace() {
        let root = tempfile::tempdir().expect("tempdir");
        assert!(resolve_beneath(root.path(), "../escape").is_err());
        assert!(resolve_beneath(root.path(), "/tmp").is_err());
    }

    #[test]
    fn verifier_permission_profile_rejects_unknown_policy() {
        let cwd = tempfile::tempdir().expect("tempdir");
        let definition = serde_json::json!({
            "sandbox": "unsafe",
            "network": "disabled",
        });
        assert!(verifier_permission_profile(&definition, cwd.path()).is_err());
    }
}
