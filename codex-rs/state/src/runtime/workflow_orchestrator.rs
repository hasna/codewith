use super::*;
use crate::runtime::background_agents::append_background_agent_event_in_tx;
use crate::runtime::workflow_automation::arm_workflow_timers_for_succeeded_step_in_tx;
use crate::runtime::workflows::WorkflowRunEventAppend;
use crate::runtime::workflows::append_workflow_run_event_in_tx;
use crate::runtime::workflows::maybe_snapshot_workflow_run_in_tx;
use crate::runtime::workflows::snapshot_workflow_run_in_tx;
use crate::runtime::workflows::workflow_state_json_string;
use codex_workflows::WorkflowBranchPrompt;
use codex_workflows::render_workflow_branch_prompt;
use serde_json::Value;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

const DEFAULT_WORKFLOW_LEASE_DURATION_MS: i64 = 60_000;
const OPENROUTER_API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
const OPENROUTER_PROVIDER_ID: &str = "openrouter";
const VERIFIER_EXECUTOR_PENDING_REASON: &str = "deterministic verifier executor is not enabled";
const VERIFIER_EXECUTOR_PENDING_REASON_CODE: &str = "verifier_executor_pending";
const WORKFLOW_BRANCH_ADMITTED_REASON: &str = "workflow branch admitted";
const WORKFLOW_BRANCH_ADMITTED_REASON_CODE: &str = "workflow_branch_admitted";
const WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON: &str =
    "OpenRouter workflow branch requires OPENROUTER_API_KEY before admission";
const WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON_CODE: &str =
    "workflow_branch_provider_env_missing";
const WORKFLOW_BRANCH_SOURCE: &str = "workflow";
const WORKFLOW_BRANCH_THREAD_STORE_KIND: &str = "background-agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunClaimParams {
    pub run_id: String,
    pub owner_id: String,
    pub lease_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunClaimOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    pub generation: i64,
    pub lease_expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunAdvanceParams {
    pub run_id: String,
    pub owner_id: String,
    pub generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunAdvanceOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunBranchAdmissionParams {
    pub run_id: String,
    pub owner_id: String,
    pub generation: i64,
    pub auth_profile_ref: Option<String>,
    pub config_fingerprint: Option<String>,
    pub version_fingerprint: Option<String>,
    pub parent_agent_run_id: Option<String>,
    pub max_active_background_agent_runs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunBranchAdmission {
    pub step_id: String,
    pub step_run_id: String,
    pub agent_id: String,
    pub background_agent_run_id: String,
    pub idempotency_key: String,
    pub model_route_json: Value,
    pub workspace_json: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunBranchAdmissionOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    pub admitted: Vec<WorkflowRunBranchAdmission>,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunBranchReconcileParams {
    pub run_id: String,
    pub owner_id: String,
    pub generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunBranchReconcileOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    pub changed: bool,
}

impl StateRuntime {
    pub async fn claim_workflow_run(
        &self,
        params: WorkflowRunClaimParams,
    ) -> anyhow::Result<Option<WorkflowRunClaimOutcome>> {
        validate_owner_id(&params.owner_id)?;
        let lease_duration_ms = params
            .lease_duration_ms
            .unwrap_or(DEFAULT_WORKFLOW_LEASE_DURATION_MS);
        if lease_duration_ms <= 0 {
            anyhow::bail!("workflow run lease_duration_ms must be positive");
        }
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let lease_expires_at_ms = now_ms.saturating_add(lease_duration_ms);
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
UPDATE workflow_runs
SET
    owner_id = ?,
    lease_expires_at_ms = ?,
    heartbeat_at_ms = ?,
    generation = generation + 1,
    status = CASE
        WHEN status IN ('pending', 'waiting') THEN ?
        ELSE status
    END,
    started_at_ms = COALESCE(started_at_ms, ?),
    updated_at_ms = ?
WHERE run_id = ?
  AND status NOT IN ('completed', 'failed', 'cancelled', 'paused')
  AND (
      owner_id IS NULL
      OR owner_id = ?
      OR lease_expires_at_ms IS NULL
      OR lease_expires_at_ms <= ?
  )
RETURNING generation
            "#,
        )
        .bind(params.owner_id.as_str())
        .bind(lease_expires_at_ms)
        .bind(now_ms)
        .bind(crate::WorkflowRunStatus::Running.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .bind(params.run_id.as_str())
        .bind(params.owner_id.as_str())
        .bind(now_ms)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let generation: i64 = row.try_get("generation")?;
        append_workflow_run_event_in_tx(
            &mut tx,
            params.run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "claimed",
                actor_kind: "orchestrator",
                actor_id: Some(params.owner_id),
                step_run_id: None,
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "generation": generation,
                    "leaseExpiresAtMs": lease_expires_at_ms,
                }),
                now_ms,
            },
        )
        .await?;
        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunClaimOutcome {
            snapshot,
            generation,
            lease_expires_at_ms,
        }))
    }

    pub async fn advance_workflow_run(
        &self,
        params: WorkflowRunAdvanceParams,
    ) -> anyhow::Result<Option<WorkflowRunAdvanceOutcome>> {
        validate_owner_id(&params.owner_id)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let completed_projected_steps = self
            .thread_goals
            .completed_workflow_projection_steps(params.run_id.as_str())
            .await?;
        let mut tx = self.pool.begin().await?;
        let Some(run) = claim_checked_workflow_run_in_tx(
            &mut tx,
            params.run_id.as_str(),
            &params.owner_id,
            params.generation,
            now_ms,
        )
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };

        let mut changed = false;
        if run.status == crate::WorkflowRunStatus::CancelRequested {
            changed |= cancel_workflow_run_in_tx(
                &mut tx,
                params.run_id.as_str(),
                params.owner_id.as_str(),
                now_ms,
            )
            .await?;
        } else if !run.status.is_terminal() {
            changed |= mark_ready_steps_in_tx(
                &mut tx,
                params.run_id.as_str(),
                params.owner_id.as_str(),
                now_ms,
            )
            .await?;
            changed |= observe_goal_plan_completion_in_tx(
                &mut tx,
                params.run_id.as_str(),
                params.owner_id.as_str(),
                &completed_projected_steps,
                now_ms,
            )
            .await?;
            changed |= promote_verified_steps_in_tx(
                &mut tx,
                params.run_id.as_str(),
                params.owner_id.as_str(),
                now_ms,
            )
            .await?;
            changed |= recompute_workflow_run_status_in_tx(
                &mut tx,
                params.run_id.as_str(),
                params.owner_id.as_str(),
                now_ms,
            )
            .await?;
        }

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        if snapshot.run.status == crate::WorkflowRunStatus::Cancelled {
            self.thread_goals
                .block_workflow_goal_plan_projection(params.run_id.as_str())
                .await?;
        }
        Ok(Some(WorkflowRunAdvanceOutcome { snapshot, changed }))
    }

    pub async fn admit_workflow_run_branches(
        &self,
        params: WorkflowRunBranchAdmissionParams,
    ) -> anyhow::Result<Option<WorkflowRunBranchAdmissionOutcome>> {
        self.admit_workflow_run_branches_with_provider_env_check(params, provider_env_key_present)
            .await
    }

    async fn admit_workflow_run_branches_with_provider_env_check(
        &self,
        params: WorkflowRunBranchAdmissionParams,
        provider_env_key_present: impl Fn(&str) -> bool,
    ) -> anyhow::Result<Option<WorkflowRunBranchAdmissionOutcome>> {
        validate_owner_id(&params.owner_id)?;
        if params
            .max_active_background_agent_runs
            .is_some_and(|limit| limit <= 0)
        {
            anyhow::bail!("max_active_background_agent_runs must be positive when set");
        }
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some(run) = claim_checked_workflow_run_in_tx(
            &mut tx,
            params.run_id.as_str(),
            &params.owner_id,
            params.generation,
            now_ms,
        )
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        if run.status.is_terminal()
            || run.status == crate::WorkflowRunStatus::CancelRequested
            || run.status == crate::WorkflowRunStatus::Blocked
        {
            let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
            tx.commit().await?;
            return Ok(Some(WorkflowRunBranchAdmissionOutcome {
                snapshot,
                admitted: Vec::new(),
                changed: false,
            }));
        }

        let admission = admit_ready_workflow_branches_in_tx(
            &mut tx,
            &run,
            &params,
            &provider_env_key_present,
            now_ms,
        )
        .await?;
        let changed = admission.changed;
        let blocked_by_provider_preflight = admission.blocked_by_provider_preflight;
        if changed {
            recompute_workflow_run_status_in_tx(
                &mut tx,
                params.run_id.as_str(),
                params.owner_id.as_str(),
                now_ms,
            )
            .await?;
        }
        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        if blocked_by_provider_preflight {
            self.thread_goals
                .block_workflow_goal_plan_projection(params.run_id.as_str())
                .await?;
        }
        Ok(Some(WorkflowRunBranchAdmissionOutcome {
            snapshot,
            admitted: admission.admitted,
            changed,
        }))
    }

    pub async fn reconcile_workflow_run_branches(
        &self,
        params: WorkflowRunBranchReconcileParams,
    ) -> anyhow::Result<Option<WorkflowRunBranchReconcileOutcome>> {
        validate_owner_id(&params.owner_id)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some(run) = claim_checked_workflow_run_in_tx(
            &mut tx,
            params.run_id.as_str(),
            &params.owner_id,
            params.generation,
            now_ms,
        )
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        if run.status.is_terminal() || run.status == crate::WorkflowRunStatus::CancelRequested {
            let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
            tx.commit().await?;
            return Ok(Some(WorkflowRunBranchReconcileOutcome {
                snapshot,
                changed: false,
            }));
        }

        let mut changed = reconcile_terminal_workflow_branches_in_tx(
            &mut tx,
            params.run_id.as_str(),
            params.owner_id.as_str(),
            now_ms,
        )
        .await?;
        changed |= recompute_workflow_run_status_in_tx(
            &mut tx,
            params.run_id.as_str(),
            params.owner_id.as_str(),
            now_ms,
        )
        .await?;
        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunBranchReconcileOutcome {
            snapshot,
            changed,
        }))
    }

    pub async fn request_workflow_run_cancel(
        &self,
        params: WorkflowRunCancelParams,
    ) -> anyhow::Result<Option<crate::WorkflowRunSnapshot>> {
        let run_id = params.run_id.clone();
        let outcome = self.workflows.request_workflow_run_cancel(params).await?;
        if outcome.as_ref().is_some_and(|outcome| outcome.changed) {
            self.thread_goals
                .block_workflow_goal_plan_projection(run_id.as_str())
                .await?;
        }
        Ok(outcome.map(|outcome| outcome.snapshot))
    }

    pub async fn pause_workflow_run(
        &self,
        params: crate::WorkflowRunPauseParams,
    ) -> anyhow::Result<Option<crate::WorkflowRunSnapshot>> {
        let run_id = params.run_id.clone();
        let outcome = self.workflows.pause_workflow_run(params).await?;
        if outcome.as_ref().is_some_and(|outcome| outcome.changed) {
            self.thread_goals
                .pause_workflow_goal_plan_projection(run_id.as_str())
                .await?;
        }
        Ok(outcome.map(|outcome| outcome.snapshot))
    }

    pub async fn resume_workflow_run(
        &self,
        params: crate::WorkflowRunResumeParams,
    ) -> anyhow::Result<Option<crate::WorkflowRunSnapshot>> {
        let run_id = params.run_id.clone();
        let outcome = self.workflows.resume_workflow_run(params).await?;
        if outcome.as_ref().is_some_and(|outcome| outcome.changed) {
            self.thread_goals
                .resume_workflow_goal_plan_projection(run_id.as_str())
                .await?;
        }
        Ok(outcome.map(|outcome| outcome.snapshot))
    }
}

pub(super) async fn claim_checked_workflow_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    generation: i64,
    now_ms: i64,
) -> anyhow::Result<Option<crate::WorkflowRun>> {
    let Some(snapshot) = maybe_snapshot_workflow_run_in_tx(tx, run_id).await? else {
        return Ok(None);
    };
    let run = snapshot.run;
    if run.status == crate::WorkflowRunStatus::Paused {
        return Ok(None);
    }
    if run.owner_id.as_deref() != Some(owner_id) {
        return Ok(None);
    }
    if run.generation != generation {
        return Ok(None);
    }
    if run
        .lease_expires_at
        .is_none_or(|lease_expires_at| datetime_to_epoch_millis(lease_expires_at) <= now_ms)
    {
        return Ok(None);
    }
    Ok(Some(run))
}

async fn admit_ready_workflow_branches_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run: &crate::WorkflowRun,
    params: &WorkflowRunBranchAdmissionParams,
    provider_env_key_present: &impl Fn(&str) -> bool,
    now_ms: i64,
) -> anyhow::Result<WorkflowRunBranchAdmissionTxOutcome> {
    let limits = WorkflowBranchLimits::from_run(run)?;
    let active_counts = active_workflow_branch_counts_in_tx(tx, run.run_id.as_str()).await?;
    let mut capacity = limits
        .max_parallel_steps
        .saturating_sub(active_counts.branch_count)
        .min(limits.max_agents.saturating_sub(active_counts.branch_count));
    if let Some(max_active_background_agent_runs) = params.max_active_background_agent_runs {
        let active_background_runs = active_background_agent_run_count_in_tx(tx).await?;
        capacity =
            capacity.min(max_active_background_agent_runs.saturating_sub(active_background_runs));
    }
    if capacity <= 0 {
        return Ok(WorkflowRunBranchAdmissionTxOutcome {
            admitted: Vec::new(),
            changed: false,
            blocked_by_provider_preflight: false,
        });
    }

    let candidates = ready_branch_candidates_in_tx(tx, run.run_id.as_str()).await?;
    let mut changed = false;
    for candidate in &candidates {
        let model_route_json = branch_model_route_json(run, candidate.model_route_json.as_ref())?;
        let Some(preflight_block) =
            workflow_branch_provider_preflight_block(&model_route_json, provider_env_key_present)
        else {
            continue;
        };
        changed |= block_workflow_branch_provider_preflight_in_tx(
            tx,
            run,
            params,
            candidate,
            &model_route_json,
            &preflight_block,
            now_ms,
        )
        .await?;
    }
    if changed {
        return Ok(WorkflowRunBranchAdmissionTxOutcome {
            admitted: Vec::new(),
            changed: true,
            blocked_by_provider_preflight: true,
        });
    }

    let mut admitted = Vec::new();
    let mut isolated_worktree_count = active_counts.isolated_worktree_count;
    for candidate in candidates {
        if i64::try_from(admitted.len())? >= capacity {
            break;
        }
        let model_route_json = branch_model_route_json(run, candidate.model_route_json.as_ref())?;
        let workspace_json = optional_workflow_state_data(candidate.workspace_json.as_ref())?;
        let workspace_mode = workflow_workspace_mode(workspace_json.as_ref());
        if workspace_mode == Some("isolated_worktree") {
            if isolated_worktree_count >= limits.max_worktrees {
                continue;
            }
            isolated_worktree_count += 1;
        }

        let branch_attempt = candidate.attempt.saturating_add(1);
        let idempotency_key = workflow_branch_idempotency_key(
            run.run_id.as_str(),
            candidate.step_id.as_str(),
            branch_attempt,
        );
        let background_agent_run_id =
            existing_background_agent_run_id_by_idempotency_key_in_tx(tx, idempotency_key.as_str())
                .await?
                .unwrap_or_else(|| Uuid::new_v4().to_string());
        let admission_json = workflow_state_json_string(
            "workflow_branch_admission",
            json!({
                "agentRunId": background_agent_run_id,
                "idempotencyKey": idempotency_key,
                "ownerId": params.owner_id.as_str(),
                "generation": params.generation,
                "attempt": branch_attempt,
                "admittedAtMs": now_ms,
                "route": workflow_branch_route_summary(&model_route_json),
                "workspace": workspace_json,
            }),
        )?;
        let updated = sqlx::query(
            r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    background_agent_run_id = ?,
    branch_admission_json = ?,
    attempt = ?,
    started_at_ms = COALESCE(started_at_ms, ?),
    updated_at_ms = ?
WHERE step_run_id = ?
  AND status = 'ready'
  AND background_agent_run_id IS NULL
            "#,
        )
        .bind(crate::WorkflowRunStepStatus::Active.as_str())
        .bind(WORKFLOW_BRANCH_ADMITTED_REASON)
        .bind(WORKFLOW_BRANCH_ADMITTED_REASON_CODE)
        .bind(background_agent_run_id.as_str())
        .bind(admission_json.as_str())
        .bind(branch_attempt)
        .bind(now_ms)
        .bind(now_ms)
        .bind(candidate.step_run_id.as_str())
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if updated == 0 {
            continue;
        }

        let created = create_background_branch_run_if_missing_in_tx(
            tx,
            BackgroundBranchRunCreate {
                run,
                candidate: &candidate,
                model_route_json: &model_route_json,
                workspace_json: workspace_json.as_ref(),
                background_agent_run_id: background_agent_run_id.as_str(),
                idempotency_key: idempotency_key.as_str(),
                params,
                now_ms,
            },
        )
        .await?;
        append_workflow_run_event_in_tx(
            tx,
            run.run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "branch_admitted",
                actor_kind: "orchestrator",
                actor_id: Some(params.owner_id.clone()),
                step_run_id: Some(candidate.step_run_id.clone()),
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "stepId": candidate.step_id.as_str(),
                    "agentId": candidate.agent_id.as_str(),
                    "backgroundAgentRunId": background_agent_run_id,
                    "createdBackgroundAgentRun": created,
                    "route": workflow_branch_route_summary(&model_route_json),
                    "workspaceMode": workspace_mode,
                }),
                now_ms,
            },
        )
        .await?;
        admitted.push(WorkflowRunBranchAdmission {
            step_id: candidate.step_id,
            step_run_id: candidate.step_run_id,
            agent_id: candidate.agent_id,
            background_agent_run_id,
            idempotency_key,
            model_route_json,
            workspace_json,
        });
    }
    Ok(WorkflowRunBranchAdmissionTxOutcome {
        changed: !admitted.is_empty(),
        admitted,
        blocked_by_provider_preflight: false,
    })
}

async fn reconcile_terminal_workflow_branches_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r#"
SELECT
    step.step_run_id,
    step.step_id,
    step.background_agent_run_id,
    agent.status
FROM workflow_run_steps step
JOIN background_agent_runs agent
  ON agent.id = step.background_agent_run_id
WHERE step.run_id = ?
  AND step.status = 'active'
  AND step.background_agent_run_id IS NOT NULL
  AND agent.status IN ('completed', 'failed', 'cancelled')
ORDER BY step.sequence, step.step_id
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut changed = false;
    for row in rows {
        let branch = TerminalWorkflowBranch {
            step_run_id: row.try_get("step_run_id")?,
            step_id: row.try_get("step_id")?,
            background_agent_run_id: row.try_get("background_agent_run_id")?,
            status: row.try_get("status")?,
        };
        if branch.status == BackgroundAgentRunStatus::Completed.as_str() {
            changed |= mark_branch_completed_in_tx(tx, run_id, owner_id, &branch, now_ms).await?;
        } else {
            changed |= mark_branch_failed_in_tx(tx, run_id, owner_id, &branch, now_ms).await?;
        }
    }
    Ok(changed)
}

async fn mark_branch_completed_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    branch: &TerminalWorkflowBranch,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let updated = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    updated_at_ms = ?
WHERE step_run_id = ?
  AND status = 'active'
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::WaitingVerifier.as_str())
    .bind(VERIFIER_EXECUTOR_PENDING_REASON)
    .bind(VERIFIER_EXECUTOR_PENDING_REASON_CODE)
    .bind(now_ms)
    .bind(branch.step_run_id.as_str())
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    append_workflow_run_event_in_tx(
        tx,
        run_id,
        WorkflowRunEventAppend {
            event_type: "branch_completed",
            actor_kind: "orchestrator",
            actor_id: Some(owner_id.to_string()),
            step_run_id: Some(branch.step_run_id.clone()),
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({
                "stepId": branch.step_id.as_str(),
                "backgroundAgentRunId": branch.background_agent_run_id.as_str(),
            }),
            now_ms,
        },
    )
    .await?;
    block_pending_step_verifiers_in_tx(tx, run_id, branch.step_id.as_str(), owner_id, now_ms)
        .await?;
    Ok(true)
}

async fn mark_branch_failed_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    branch: &TerminalWorkflowBranch,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let reason_code = if branch.status == BackgroundAgentRunStatus::Cancelled.as_str() {
        "workflow_branch_cancelled"
    } else {
        "workflow_branch_failed"
    };
    let updated = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE step_run_id = ?
  AND status = 'active'
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Failed.as_str())
    .bind("workflow branch agent did not complete successfully")
    .bind(reason_code)
    .bind(now_ms)
    .bind(now_ms)
    .bind(branch.step_run_id.as_str())
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    sqlx::query(
        r#"
UPDATE workflow_run_step_verifiers
SET status = ?, reason_code = ?, completed_at_ms = ?, updated_at_ms = ?
WHERE run_id = ?
  AND step_id = ?
  AND status NOT IN ('passed', 'failed', 'skipped')
        "#,
    )
    .bind(crate::WorkflowRunStepVerifierStatus::Skipped.as_str())
    .bind(reason_code)
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .bind(branch.step_id.as_str())
    .execute(&mut **tx)
    .await?;
    append_workflow_run_event_in_tx(
        tx,
        run_id,
        WorkflowRunEventAppend {
            event_type: "branch_failed",
            actor_kind: "orchestrator",
            actor_id: Some(owner_id.to_string()),
            step_run_id: Some(branch.step_run_id.clone()),
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({
                "stepId": branch.step_id.as_str(),
                "backgroundAgentRunId": branch.background_agent_run_id.as_str(),
                "branchStatus": branch.status.as_str(),
                "reasonCode": reason_code,
            }),
            now_ms,
        },
    )
    .await?;
    Ok(true)
}

#[derive(Debug)]
struct WorkflowBranchLimits {
    max_parallel_steps: i64,
    max_agents: i64,
    max_worktrees: i64,
}

impl WorkflowBranchLimits {
    fn from_run(run: &crate::WorkflowRun) -> anyhow::Result<Self> {
        let data = workflow_state_data(&run.limits_json);
        Ok(Self {
            max_parallel_steps: workflow_limit_i64(data, "max_parallel_steps")?,
            max_agents: workflow_limit_i64(data, "max_agents")?,
            max_worktrees: workflow_limit_i64(data, "max_worktrees")?,
        })
    }
}

struct ActiveWorkflowBranchCounts {
    branch_count: i64,
    isolated_worktree_count: i64,
}

struct TerminalWorkflowBranch {
    step_run_id: String,
    step_id: String,
    background_agent_run_id: String,
    status: String,
}

struct WorkflowRunBranchAdmissionTxOutcome {
    admitted: Vec<WorkflowRunBranchAdmission>,
    changed: bool,
    blocked_by_provider_preflight: bool,
}

struct ReadyBranchCandidate {
    step_run_id: String,
    step_id: String,
    title: String,
    agent_id: String,
    parallel_group: Option<String>,
    model_route_json: Option<Value>,
    workspace_json: Option<Value>,
    attempt: i64,
}

struct WorkflowBranchProviderPreflightBlock {
    env_key: &'static str,
    reason: &'static str,
    reason_code: &'static str,
}

struct BackgroundBranchRunCreate<'a> {
    run: &'a crate::WorkflowRun,
    candidate: &'a ReadyBranchCandidate,
    model_route_json: &'a Value,
    workspace_json: Option<&'a Value>,
    background_agent_run_id: &'a str,
    idempotency_key: &'a str,
    params: &'a WorkflowRunBranchAdmissionParams,
    now_ms: i64,
}

struct BackgroundAgentStatusSnapshotUpsert<'a> {
    run_id: &'a str,
    seq: i64,
    status: BackgroundAgentRunStatus,
    desired_state: BackgroundAgentDesiredState,
    summary: &'a str,
    payload_json: &'a Value,
    now: i64,
}

async fn active_workflow_branch_counts_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<ActiveWorkflowBranchCounts> {
    let rows = sqlx::query(
        r#"
SELECT workspace_json
FROM workflow_run_steps
WHERE run_id = ?
  AND status = 'active'
  AND background_agent_run_id IS NOT NULL
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut isolated_worktree_count = 0_i64;
    for row in &rows {
        let workspace_json: Option<String> = row.try_get("workspace_json")?;
        let workspace_json = workspace_json
            .map(|value| serde_json::from_str::<Value>(value.as_str()))
            .transpose()?;
        if workflow_workspace_mode(workspace_json.as_ref().and_then(|value| value.get("data")))
            == Some("isolated_worktree")
        {
            isolated_worktree_count += 1;
        }
    }
    Ok(ActiveWorkflowBranchCounts {
        branch_count: i64::try_from(rows.len())?,
        isolated_worktree_count,
    })
}

async fn active_background_agent_run_count_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
) -> anyhow::Result<i64> {
    sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM background_agent_runs
WHERE retention_state = 'active'
  AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(anyhow::Error::from)
}

async fn ready_branch_candidates_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Vec<ReadyBranchCandidate>> {
    let rows = sqlx::query(
        r#"
SELECT
    step_run_id,
    step_id,
    title,
    agent_id,
    parallel_group,
    model_route_json,
    workspace_json,
    attempt
FROM workflow_run_steps
WHERE run_id = ?
  AND status = 'ready'
  AND background_agent_run_id IS NULL
  AND (approval_gate IS NULL OR approval_state = 'approved')
ORDER BY sequence, step_id
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let model_route_json: Option<String> = row.try_get("model_route_json")?;
            let workspace_json: Option<String> = row.try_get("workspace_json")?;
            Ok(ReadyBranchCandidate {
                step_run_id: row.try_get("step_run_id")?,
                step_id: row.try_get("step_id")?,
                title: row.try_get("title")?,
                agent_id: row.try_get("agent_id")?,
                parallel_group: row.try_get("parallel_group")?,
                model_route_json: model_route_json
                    .map(|value| serde_json::from_str::<Value>(value.as_str()))
                    .transpose()?,
                workspace_json: workspace_json
                    .map(|value| serde_json::from_str::<Value>(value.as_str()))
                    .transpose()?,
                attempt: row.try_get("attempt")?,
            })
        })
        .collect()
}

async fn block_workflow_branch_provider_preflight_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run: &crate::WorkflowRun,
    params: &WorkflowRunBranchAdmissionParams,
    candidate: &ReadyBranchCandidate,
    model_route_json: &Value,
    preflight_block: &WorkflowBranchProviderPreflightBlock,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let updated = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    updated_at_ms = ?
WHERE step_run_id = ?
  AND status = 'ready'
  AND background_agent_run_id IS NULL
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Blocked.as_str())
    .bind(preflight_block.reason)
    .bind(preflight_block.reason_code)
    .bind(now_ms)
    .bind(candidate.step_run_id.as_str())
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }

    sqlx::query(
        r#"
UPDATE workflow_run_step_verifiers
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND step_id = ?
  AND status NOT IN ('passed', 'failed', 'skipped')
        "#,
    )
    .bind(crate::WorkflowRunStepVerifierStatus::Blocked.as_str())
    .bind(preflight_block.reason)
    .bind(preflight_block.reason_code)
    .bind(now_ms)
    .bind(now_ms)
    .bind(run.run_id.as_str())
    .bind(candidate.step_id.as_str())
    .execute(&mut **tx)
    .await?;
    append_workflow_run_event_in_tx(
        tx,
        run.run_id.as_str(),
        WorkflowRunEventAppend {
            event_type: "branch_preflight_blocked",
            actor_kind: "orchestrator",
            actor_id: Some(params.owner_id.clone()),
            step_run_id: Some(candidate.step_run_id.clone()),
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({
                "stepId": candidate.step_id.as_str(),
                "agentId": candidate.agent_id.as_str(),
                "missingEnvKey": preflight_block.env_key,
                "reasonCode": preflight_block.reason_code,
                "route": workflow_branch_route_summary(model_route_json),
            }),
            now_ms,
        },
    )
    .await?;
    Ok(true)
}

fn workflow_branch_provider_preflight_block(
    model_route_json: &Value,
    provider_env_key_present: &impl Fn(&str) -> bool,
) -> Option<WorkflowBranchProviderPreflightBlock> {
    let uses_openrouter = ["model_gateway", "provider"].iter().any(|field| {
        model_route_json
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case(OPENROUTER_PROVIDER_ID))
    });
    if !uses_openrouter || provider_env_key_present(OPENROUTER_API_KEY_ENV_VAR) {
        return None;
    }

    Some(WorkflowBranchProviderPreflightBlock {
        env_key: OPENROUTER_API_KEY_ENV_VAR,
        reason: WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON,
        reason_code: WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON_CODE,
    })
}

fn provider_env_key_present(env_key: &str) -> bool {
    std::env::var(env_key).is_ok_and(|value| !value.trim().is_empty())
}

async fn existing_background_agent_run_id_by_idempotency_key_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    idempotency_key: &str,
) -> anyhow::Result<Option<String>> {
    sqlx::query_scalar(
        r#"
SELECT id
FROM background_agent_runs
WHERE idempotency_key = ?
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(anyhow::Error::from)
}

async fn create_background_branch_run_if_missing_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    branch: BackgroundBranchRunCreate<'_>,
) -> anyhow::Result<bool> {
    let BackgroundBranchRunCreate {
        run,
        candidate,
        model_route_json,
        workspace_json,
        background_agent_run_id,
        idempotency_key,
        params,
        now_ms,
    } = branch;
    let now = now_ms.div_euclid(1000);
    let existing: Option<String> = sqlx::query_scalar(
        r#"
SELECT id
FROM background_agent_runs
WHERE id = ?
        "#,
    )
    .bind(background_agent_run_id)
    .fetch_optional(&mut **tx)
    .await?;
    if existing.is_some() {
        return Ok(false);
    }

    let prompt = render_workflow_branch_prompt(WorkflowBranchPrompt {
        run_id: run.run_id.as_str(),
        step_id: candidate.step_id.as_str(),
        title: candidate.title.as_str(),
        agent_id: candidate.agent_id.as_str(),
        parallel_group: candidate.parallel_group.as_deref(),
    });
    let prompt_snapshot_ref = format!("workflow:{}:step:{}:prompt", run.run_id, candidate.step_id);
    let source = WORKFLOW_BRANCH_SOURCE.to_string();
    let spawn_linkage_json = redact_state_json_string(&json!({
        "schemaVersion": "workflow.branch_spawn/v0",
        "redactionVersion": 1,
        "workflowRunId": run.run_id.as_str(),
        "workflowRecordId": run.workflow_record_id.as_str(),
        "specWorkflowId": run.spec_workflow_id.as_str(),
        "stepId": candidate.step_id.as_str(),
        "stepRunId": candidate.step_run_id.as_str(),
        "agentId": candidate.agent_id.as_str(),
        "parallelGroup": candidate.parallel_group.as_deref(),
    }))?;
    sqlx::query(
        r#"
INSERT INTO background_agent_runs (
    id,
    idempotency_key,
    request_id,
    source,
    prompt_snapshot_ref,
    input_snapshot_ref,
    thread_id,
    thread_store_kind,
    thread_store_id,
    rollout_path,
    parent_thread_id,
    parent_agent_run_id,
    spawn_linkage_json,
    auth_profile_ref,
    desired_state,
    status,
    status_reason,
    config_fingerprint,
    version_fingerprint,
    retention_state,
    created_at,
    updated_at
) VALUES (?, ?, NULL, ?, ?, NULL, NULL, ?, NULL, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(background_agent_run_id)
    .bind(redact_state_string(idempotency_key))
    .bind(source)
    .bind(redact_state_string(prompt_snapshot_ref.as_str()))
    .bind(WORKFLOW_BRANCH_THREAD_STORE_KIND)
    .bind(
        run.source_thread_id
            .as_ref()
            .map(std::string::ToString::to_string),
    )
    .bind(params.parent_agent_run_id.as_deref())
    .bind(spawn_linkage_json.as_str())
    .bind(params.auth_profile_ref.as_deref().map(redact_state_string))
    .bind(BackgroundAgentDesiredState::Running.as_str())
    .bind(BackgroundAgentRunStatus::Queued.as_str())
    .bind(redact_state_string("queued by workflow branch admission"))
    .bind(params.config_fingerprint.as_deref())
    .bind(params.version_fingerprint.as_deref())
    .bind(crate::BackgroundAgentRetentionState::Active.as_str())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let event = append_background_agent_event_in_tx(
        tx,
        background_agent_run_id,
        "agent.started",
        &json!({
            "cwd": Value::Null,
            "prompt": prompt,
            "promptSnapshotRef": prompt_snapshot_ref,
        }),
        now,
    )
    .await?;
    let status_payload = json!({
        "phase": "queued",
        "workflowRunId": run.run_id.as_str(),
        "workflowStepId": candidate.step_id.as_str(),
    });
    upsert_background_agent_status_snapshot_in_tx(
        tx,
        BackgroundAgentStatusSnapshotUpsert {
            run_id: background_agent_run_id,
            seq: event.seq,
            status: BackgroundAgentRunStatus::Queued,
            desired_state: BackgroundAgentDesiredState::Running,
            summary: "Queued",
            payload_json: &status_payload,
            now,
        },
    )
    .await?;
    insert_background_agent_execution_snapshot_in_tx(
        tx,
        background_agent_run_id,
        "initial_execution_context",
        branch_execution_payload(run, candidate, model_route_json, workspace_json, params),
        "abort_mid_turn_resume_at_safe_boundary",
        params.config_fingerprint.as_deref(),
        now,
    )
    .await?;
    Ok(true)
}

async fn upsert_background_agent_status_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    snapshot: BackgroundAgentStatusSnapshotUpsert<'_>,
) -> anyhow::Result<()> {
    let BackgroundAgentStatusSnapshotUpsert {
        run_id,
        seq,
        status,
        desired_state,
        summary,
        payload_json,
        now,
    } = snapshot;
    sqlx::query(
        r#"
INSERT INTO background_agent_status_snapshots (
    run_id,
    seq,
    status,
    desired_state,
    summary,
    pending_interaction_count,
    last_event_seq,
    payload_json,
    updated_at
) VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)
ON CONFLICT(run_id) DO UPDATE SET
    seq = excluded.seq,
    status = excluded.status,
    desired_state = excluded.desired_state,
    summary = excluded.summary,
    pending_interaction_count = excluded.pending_interaction_count,
    last_event_seq = excluded.last_event_seq,
    payload_json = excluded.payload_json,
    updated_at = excluded.updated_at
        "#,
    )
    .bind(run_id)
    .bind(seq)
    .bind(status.as_str())
    .bind(desired_state.as_str())
    .bind(redact_state_string(summary))
    .bind(seq)
    .bind(redact_state_json_string(payload_json)?)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_background_agent_execution_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    snapshot_kind: &str,
    payload_json: Value,
    recovery_policy: &str,
    config_fingerprint: Option<&str>,
    now: i64,
) -> anyhow::Result<()> {
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM background_agent_execution_snapshots WHERE run_id = ?",
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        r#"
INSERT INTO background_agent_execution_snapshots (
    run_id,
    seq,
    snapshot_kind,
    payload_json,
    recovery_policy,
    config_fingerprint,
    created_at
) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(run_id)
    .bind(seq)
    .bind(snapshot_kind)
    .bind(redact_state_json_string(&payload_json)?)
    .bind(recovery_policy)
    .bind(config_fingerprint)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
UPDATE background_agent_runs
SET last_snapshot_seq = ?, updated_at = ?
WHERE id = ?
        "#,
    )
    .bind(seq)
    .bind(now)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn branch_execution_payload(
    run: &crate::WorkflowRun,
    candidate: &ReadyBranchCandidate,
    model_route_json: &Value,
    workspace_json: Option<&Value>,
    params: &WorkflowRunBranchAdmissionParams,
) -> Value {
    json!({
        "snapshotSource": "workflow/branch_admission",
        "workflowRunId": run.run_id.as_str(),
        "workflowStepId": candidate.step_id.as_str(),
        "workflowStepRunId": candidate.step_run_id.as_str(),
        "agentId": candidate.agent_id.as_str(),
        "cwd": Value::Null,
        "workspaceRoots": Value::Null,
        "modelGateway": model_route_json.get("model_gateway"),
        "model": model_route_json.get("model"),
        "provider": model_route_json.get("provider"),
        "reasoning": model_route_json.get("reasoning"),
        "serviceTier": model_route_json.get("service_tier"),
        "approvalPolicy": model_route_json.get("approval_policy"),
        "permissionProfile": model_route_json.get("permission_profile"),
        "authProfileRef": params.auth_profile_ref.as_deref(),
        "workspace": workspace_json,
        "envSnapshotPolicy": "inherit-minimal",
        "maxRuntimeSeconds": workflow_state_data(&run.limits_json).get("max_step_runtime_seconds"),
    })
}

fn workflow_branch_route_summary(model_route_json: &Value) -> Value {
    json!({
        "modelGateway": model_route_json.get("model_gateway"),
        "provider": model_route_json.get("provider"),
        "model": model_route_json.get("model"),
        "reasoning": model_route_json.get("reasoning"),
        "serviceTier": model_route_json.get("service_tier"),
        "permissionProfile": model_route_json.get("permission_profile"),
    })
}

fn branch_model_route_json(
    run: &crate::WorkflowRun,
    step_route_json: Option<&Value>,
) -> anyhow::Result<Value> {
    let route = step_route_json.unwrap_or(&run.execution_defaults_json);
    Ok(workflow_state_data(route).clone())
}

fn optional_workflow_state_data(value: Option<&Value>) -> anyhow::Result<Option<Value>> {
    Ok(value.map(workflow_state_data).cloned())
}

fn workflow_workspace_mode(workspace_json: Option<&Value>) -> Option<&str> {
    workspace_json
        .and_then(|workspace_json| workspace_json.get("mode"))
        .and_then(Value::as_str)
}

fn workflow_branch_idempotency_key(run_id: &str, step_id: &str, attempt: i64) -> String {
    format!("workflow:{run_id}:step:{step_id}:attempt:{attempt}")
}

fn workflow_state_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn workflow_limit_i64(data: &Value, field: &str) -> anyhow::Result<i64> {
    let Some(value) = data.get(field).and_then(Value::as_u64) else {
        anyhow::bail!("workflow limits missing `{field}`");
    };
    i64::try_from(value).map_err(anyhow::Error::from)
}

async fn mark_ready_steps_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET status = ?, updated_at_ms = ?
WHERE run_id = ?
  AND status = 'pending'
  AND NOT EXISTS (
      SELECT 1
      FROM workflow_run_step_dependencies dependency
      JOIN workflow_run_steps dependency_step
        ON dependency_step.run_id = dependency.run_id
       AND dependency_step.step_id = dependency.depends_on_step_id
      WHERE dependency.run_id = workflow_run_steps.run_id
        AND dependency.step_id = workflow_run_steps.step_id
        AND dependency_step.status != 'succeeded'
  )
RETURNING step_run_id, step_id
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Ready.as_str())
    .bind(now_ms)
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;

    for row in &rows {
        let step_run_id: String = row.try_get("step_run_id")?;
        let step_id: String = row.try_get("step_id")?;
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "step_ready",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: Some(step_run_id),
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({ "stepId": step_id }),
                now_ms,
            },
        )
        .await?;
    }
    Ok(!rows.is_empty())
}

async fn observe_goal_plan_completion_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    completed_projected_steps: &[String],
    now_ms: i64,
) -> anyhow::Result<bool> {
    let mut changed = false;
    for step_id in completed_projected_steps {
        let row = sqlx::query(
            r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND step_id = ?
  AND status IN ('pending', 'ready', 'active')
RETURNING step_run_id
            "#,
        )
        .bind(crate::WorkflowRunStepStatus::WaitingVerifier.as_str())
        .bind(VERIFIER_EXECUTOR_PENDING_REASON)
        .bind(VERIFIER_EXECUTOR_PENDING_REASON_CODE)
        .bind(now_ms)
        .bind(run_id)
        .bind(step_id.as_str())
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            continue;
        };
        let step_run_id: String = row.try_get("step_run_id")?;
        changed = true;
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "step_waiting_verifier",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: Some(step_run_id),
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "stepId": step_id,
                    "reasonCode": VERIFIER_EXECUTOR_PENDING_REASON_CODE,
                }),
                now_ms,
            },
        )
        .await?;
        changed |=
            block_pending_step_verifiers_in_tx(tx, run_id, step_id, owner_id, now_ms).await?;
    }
    Ok(changed)
}

async fn block_pending_step_verifiers_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    step_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r#"
UPDATE workflow_run_step_verifiers
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND step_id = ?
  AND status = 'pending'
RETURNING verifier_run_id, verifier_id, verifier_type
        "#,
    )
    .bind(crate::WorkflowRunStepVerifierStatus::Blocked.as_str())
    .bind(VERIFIER_EXECUTOR_PENDING_REASON)
    .bind(VERIFIER_EXECUTOR_PENDING_REASON_CODE)
    .bind(now_ms)
    .bind(run_id)
    .bind(step_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in &rows {
        let verifier_run_id: String = row.try_get("verifier_run_id")?;
        let verifier_id: String = row.try_get("verifier_id")?;
        let verifier_type: String = row.try_get("verifier_type")?;
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "verifier_blocked",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: None,
                verifier_run_id: Some(verifier_run_id),
                visibility: "internal",
                payload: json!({
                    "stepId": step_id,
                    "verifierId": verifier_id,
                    "verifierType": verifier_type,
                    "reasonCode": VERIFIER_EXECUTOR_PENDING_REASON_CODE,
                }),
                now_ms,
            },
        )
        .await?;
    }
    Ok(!rows.is_empty())
}

async fn promote_verified_steps_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = NULL,
    reason_code = NULL,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND status = 'waiting_verifier'
  AND NOT EXISTS (
      SELECT 1
      FROM workflow_run_step_verifiers verifier
      WHERE verifier.run_id = workflow_run_steps.run_id
        AND verifier.step_id = workflow_run_steps.step_id
        AND verifier.status != 'passed'
  )
RETURNING step_run_id, step_id
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Succeeded.as_str())
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in &rows {
        let step_run_id: String = row.try_get("step_run_id")?;
        let step_id: String = row.try_get("step_id")?;
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "step_succeeded",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: Some(step_run_id),
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({ "stepId": step_id }),
                now_ms,
            },
        )
        .await?;
        arm_workflow_timers_for_succeeded_step_in_tx(
            tx,
            run_id,
            step_id.as_str(),
            owner_id,
            now_ms,
        )
        .await?;
    }
    Ok(!rows.is_empty())
}

async fn recompute_workflow_run_status_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let step_statuses = sqlx::query_scalar::<_, String>(
        r#"
SELECT status
FROM workflow_run_steps
WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    let next_status = derive_run_status_from_step_statuses(&step_statuses)?;
    let current_status: String = sqlx::query_scalar(
        r#"
SELECT status
FROM workflow_runs
WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    if current_status == next_status.as_str() {
        return Ok(false);
    }
    let (status_reason, reason_code) = workflow_run_reason_for_status(&next_status);
    sqlx::query(
        r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    completed_at_ms = CASE WHEN ? THEN ? ELSE completed_at_ms END,
    updated_at_ms = ?
WHERE run_id = ?
        "#,
    )
    .bind(next_status.as_str())
    .bind(status_reason)
    .bind(reason_code)
    .bind(next_status.is_terminal())
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    append_workflow_run_event_in_tx(
        tx,
        run_id,
        WorkflowRunEventAppend {
            event_type: "run_status_changed",
            actor_kind: "orchestrator",
            actor_id: Some(owner_id.to_string()),
            step_run_id: None,
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({
                "status": next_status.as_str(),
                "reasonCode": reason_code,
            }),
            now_ms,
        },
    )
    .await?;
    Ok(true)
}

async fn cancel_workflow_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let updated = sqlx::query(
        r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = COALESCE(status_reason, ?),
    reason_code = ?,
    owner_id = NULL,
    lease_expires_at_ms = NULL,
    heartbeat_at_ms = ?,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND status = 'cancel_requested'
        "#,
    )
    .bind(crate::WorkflowRunStatus::Cancelled.as_str())
    .bind("workflow run cancelled")
    .bind("workflow_cancelled")
    .bind(now_ms)
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    stop_workflow_branch_agents_for_cancel_in_tx(tx, run_id, owner_id, now_ms).await?;
    cancel_workflow_automation_in_tx(tx, run_id, owner_id, now_ms).await?;
    sqlx::query(
        r#"
UPDATE workflow_run_steps
SET status = ?, reason_code = ?, updated_at_ms = ?, completed_at_ms = ?
WHERE run_id = ?
  AND status NOT IN ('succeeded', 'cancelled', 'failed', 'skipped')
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Cancelled.as_str())
    .bind("workflow_cancelled")
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
UPDATE workflow_run_step_verifiers
SET status = ?, reason_code = ?, updated_at_ms = ?, completed_at_ms = ?
WHERE run_id = ?
  AND status NOT IN ('passed', 'failed', 'skipped')
        "#,
    )
    .bind(crate::WorkflowRunStepVerifierStatus::Skipped.as_str())
    .bind("workflow_cancelled")
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    append_workflow_run_event_in_tx(
        tx,
        run_id,
        WorkflowRunEventAppend {
            event_type: "cancelled",
            actor_kind: "orchestrator",
            actor_id: Some(owner_id.to_string()),
            step_run_id: None,
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({ "reasonCode": "workflow_cancelled" }),
            now_ms,
        },
    )
    .await?;
    Ok(true)
}

async fn stop_workflow_branch_agents_for_cancel_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r#"
SELECT
    step.step_run_id,
    step.step_id,
    step.background_agent_run_id,
    agent.status,
    agent.supervisor_id
FROM workflow_run_steps step
JOIN background_agent_runs agent
  ON agent.id = step.background_agent_run_id
WHERE step.run_id = ?
  AND step.background_agent_run_id IS NOT NULL
  AND agent.status NOT IN ('completed', 'failed', 'cancelled')
ORDER BY step.sequence, step.step_id
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    let now = now_ms.div_euclid(1000);
    let mut changed = false;
    for row in rows {
        let step_run_id: String = row.try_get("step_run_id")?;
        let step_id: String = row.try_get("step_id")?;
        let background_agent_run_id: String = row.try_get("background_agent_run_id")?;
        let status: String = row.try_get("status")?;
        let supervisor_id: Option<String> = row.try_get("supervisor_id")?;
        let terminalize_immediately =
            supervisor_id.is_none() || matches!(status.as_str(), "queued" | "orphaned");
        let next_status = if terminalize_immediately {
            BackgroundAgentRunStatus::Cancelled
        } else {
            BackgroundAgentRunStatus::Stopping
        };
        let status_reason = if terminalize_immediately {
            "workflow cancellation requested before worker claim"
        } else {
            "workflow cancellation requested"
        };
        let updated = sqlx::query(
            r#"
UPDATE background_agent_runs
SET
    desired_state = ?,
    status = ?,
    status_reason = ?,
    completed_at = CASE WHEN ? THEN COALESCE(completed_at, ?) ELSE completed_at END,
    updated_at = ?
WHERE id = ?
  AND status NOT IN ('completed', 'failed', 'cancelled')
            "#,
        )
        .bind(BackgroundAgentDesiredState::Stopped.as_str())
        .bind(next_status.as_str())
        .bind(status_reason)
        .bind(terminalize_immediately)
        .bind(now)
        .bind(now)
        .bind(background_agent_run_id.as_str())
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if updated == 0 {
            continue;
        }
        changed = true;
        cancel_background_agent_pending_interactions_in_tx(
            tx,
            background_agent_run_id.as_str(),
            now,
        )
        .await?;
        release_background_agent_worktree_leases_for_cancel_in_tx(
            tx,
            background_agent_run_id.as_str(),
            now,
        )
        .await?;
        let event = append_background_agent_event_in_tx(
            tx,
            background_agent_run_id.as_str(),
            "agent.stopRequested",
            &json!({
                "reason": "workflow_cancelled",
                "workflowRunId": run_id,
                "workflowStepId": step_id,
            }),
            now,
        )
        .await?;
        if terminalize_immediately {
            let status_payload = json!({
                "phase": "cancelled",
                "reason": "workflow_cancelled",
                "workflowRunId": run_id,
                "workflowStepId": step_id,
            });
            upsert_background_agent_status_snapshot_in_tx(
                tx,
                BackgroundAgentStatusSnapshotUpsert {
                    run_id: background_agent_run_id.as_str(),
                    seq: event.seq,
                    status: BackgroundAgentRunStatus::Cancelled,
                    desired_state: BackgroundAgentDesiredState::Stopped,
                    summary: "Cancelled",
                    payload_json: &status_payload,
                    now,
                },
            )
            .await?;
        }
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "branch_stop_requested",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: Some(step_run_id),
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "stepId": step_id,
                    "backgroundAgentRunId": background_agent_run_id,
                    "terminalizedImmediately": terminalize_immediately,
                    "reasonCode": "workflow_cancelled",
                }),
                now_ms,
            },
        )
        .await?;
    }
    Ok(changed)
}

async fn cancel_workflow_automation_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    let cancelled_timers = sqlx::query(
        r#"
UPDATE workflow_run_timers
SET
    status = 'cancelled',
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    next_fire_at_ms = NULL,
    updated_at_ms = ?
WHERE run_id = ?
  AND status = 'active'
        "#,
    )
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let skipped_fires = sqlx::query(
        r#"
UPDATE workflow_run_timer_fires
SET
    status = 'skipped',
    completed_at_ms = ?,
    result_json = ?
WHERE run_id = ?
  AND status = 'claimed'
        "#,
    )
    .bind(now_ms)
    .bind(workflow_state_json_string(
        "workflow_timer_fire_result",
        json!({ "reasonCode": "workflow_cancelled" }),
    )?)
    .bind(run_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let cancelled_monitor_links = sqlx::query(
        r#"
UPDATE workflow_run_monitor_links
SET status = 'cancelled', updated_at_ms = ?
WHERE run_id = ?
  AND status = 'active'
        "#,
    )
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if cancelled_timers > 0 || skipped_fires > 0 || cancelled_monitor_links > 0 {
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "automation_cancelled",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: None,
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "timerCount": cancelled_timers,
                    "timerFireCount": skipped_fires,
                    "monitorLinkCount": cancelled_monitor_links,
                    "reasonCode": "workflow_cancelled",
                }),
                now_ms,
            },
        )
        .await?;
    }
    Ok(())
}

async fn cancel_background_agent_pending_interactions_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    background_agent_run_id: &str,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
UPDATE background_agent_pending_interactions
SET
    status = ?,
    response_payload_json = ?,
    responded_at = COALESCE(responded_at, ?),
    updated_at = ?
WHERE run_id = ?
  AND status IN (?, ?)
        "#,
    )
    .bind(BackgroundAgentPendingInteractionStatus::Cancelled.as_str())
    .bind(redact_state_json_string(
        &json!({"reason": "workflow_cancelled"}),
    )?)
    .bind(now)
    .bind(now)
    .bind(background_agent_run_id)
    .bind(BackgroundAgentPendingInteractionStatus::Pending.as_str())
    .bind(BackgroundAgentPendingInteractionStatus::Delivered.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn release_background_agent_worktree_leases_for_cancel_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    background_agent_run_id: &str,
    now: i64,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
        r#"
SELECT
    id,
    mode,
    worktree_path,
    dirty,
    status_snapshot_json,
    cleanup_after
FROM background_agent_worktree_leases
WHERE run_id = ?
  AND released_at IS NULL
  AND deleted_at IS NULL
        "#,
    )
    .bind(background_agent_run_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let lease_id: String = row.try_get("id")?;
        let mode: String = row.try_get("mode")?;
        let worktree_path: String = row.try_get("worktree_path")?;
        let dirty: i64 = row.try_get("dirty")?;
        let status_snapshot_json: String = row.try_get("status_snapshot_json")?;
        let cleanup_after: Option<i64> = row.try_get("cleanup_after")?;
        sqlx::query(
            r#"
UPDATE background_agent_worktree_leases
SET released_at = COALESCE(released_at, ?), updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(lease_id.as_str())
        .execute(&mut **tx)
        .await?;
        let lifecycle_status = if mode == BackgroundAgentWorkspaceMode::IsolatedWorktree.as_str() {
            ManagedWorktreeLifecycleStatus::CleanupPending
        } else {
            ManagedWorktreeLifecycleStatus::Released
        };
        sqlx::query(
            r#"
UPDATE managed_worktrees
SET
    lifecycle_status = ?,
    released_at_ms = COALESCE(released_at_ms, ?),
    cleanup_policy = ?,
    updated_at_ms = ?
WHERE worktree_id = ?
  AND deleted_at_ms IS NULL
            "#,
        )
        .bind(lifecycle_status.as_str())
        .bind(now * 1000)
        .bind(ManagedWorktreeCleanupPolicy::DeleteIfClean.as_str())
        .bind(now * 1000)
        .bind(lease_id.as_str())
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind(now * 1000)
        .bind(lease_id.as_str())
        .execute(&mut **tx)
        .await?;
        if lifecycle_status == ManagedWorktreeLifecycleStatus::CleanupPending || dirty != 0 {
            let payload_json = json!({
                "cleanup": ManagedWorktreeCleanupPolicy::DeleteIfClean.as_str(),
                "forceDeleteRequired": dirty != 0,
                "statusSnapshot": serde_json::from_str::<Value>(&status_snapshot_json)
                    .unwrap_or_else(|_| json!({})),
            });
            sqlx::query(
                r#"
INSERT INTO background_agent_cleanup_tombstones (
    run_id,
    reason,
    worktree_path,
    dirty_worktree,
    retained_until,
    payload_json,
    created_at
) VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(run_id) DO UPDATE SET
    reason = excluded.reason,
    worktree_path = excluded.worktree_path,
    dirty_worktree = excluded.dirty_worktree,
    retained_until = excluded.retained_until,
    payload_json = excluded.payload_json,
    created_at = excluded.created_at,
    deleted_at = NULL
                "#,
            )
            .bind(background_agent_run_id)
            .bind("worktree cleanup pending")
            .bind(worktree_path)
            .bind(dirty)
            .bind(cleanup_after)
            .bind(redact_state_json_string(&payload_json)?)
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

fn derive_run_status_from_step_statuses(
    statuses: &[String],
) -> anyhow::Result<crate::WorkflowRunStatus> {
    let statuses = statuses
        .iter()
        .map(|status| crate::WorkflowRunStepStatus::try_from(status.as_str()))
        .collect::<anyhow::Result<Vec<_>>>()?;
    if statuses.contains(&crate::WorkflowRunStepStatus::Failed) {
        return Ok(crate::WorkflowRunStatus::Failed);
    }
    if statuses.contains(&crate::WorkflowRunStepStatus::Blocked) {
        return Ok(crate::WorkflowRunStatus::Blocked);
    }
    if statuses.contains(&crate::WorkflowRunStepStatus::WaitingVerifier) {
        return Ok(crate::WorkflowRunStatus::Waiting);
    }
    if !statuses.is_empty()
        && statuses.iter().all(|status| {
            matches!(
                status,
                crate::WorkflowRunStepStatus::Succeeded | crate::WorkflowRunStepStatus::Skipped
            )
        })
    {
        return Ok(crate::WorkflowRunStatus::Completed);
    }
    Ok(crate::WorkflowRunStatus::Running)
}

fn workflow_run_reason_for_status(
    status: &crate::WorkflowRunStatus,
) -> (Option<&'static str>, Option<&'static str>) {
    match status {
        crate::WorkflowRunStatus::Waiting => (
            Some(VERIFIER_EXECUTOR_PENDING_REASON),
            Some(VERIFIER_EXECUTOR_PENDING_REASON_CODE),
        ),
        crate::WorkflowRunStatus::Blocked => {
            (Some("workflow run is blocked"), Some("workflow_blocked"))
        }
        crate::WorkflowRunStatus::Failed => (Some("workflow run failed"), Some("workflow_failed")),
        crate::WorkflowRunStatus::Completed => (None, None),
        crate::WorkflowRunStatus::Pending
        | crate::WorkflowRunStatus::Running
        | crate::WorkflowRunStatus::CancelRequested
        | crate::WorkflowRunStatus::Paused
        | crate::WorkflowRunStatus::Cancelled
        | crate::WorkflowRunStatus::Other(_) => (None, None),
    }
}

fn validate_owner_id(owner_id: &str) -> anyhow::Result<()> {
    if owner_id.trim().is_empty() {
        anyhow::bail!("workflow run owner_id must not be empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::sync::Arc;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000654").expect("valid thread id")
    }

    async fn upsert_test_thread(runtime: &StateRuntime, thread_id: ThreadId) {
        let metadata = test_thread_metadata(
            runtime.codex_home(),
            thread_id,
            runtime.codex_home().join("workspace"),
        );
        runtime
            .upsert_thread(&metadata)
            .await
            .expect("test thread should be upserted");
    }

    fn orchestrator_workflow_yaml(
        marker: &Path,
        workflow_id: &str,
        include_second: bool,
    ) -> String {
        let marker_command = format!("touch {}", marker.display());
        let marker_command = format!("'{}'", marker_command.replace('\'', "''"));
        let second_step = if include_second {
            r#"
  - id: "adversarial_review"
    title: "Review the inert orchestrator output"
    agent: "adversarial_review"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    parallel_group: "review"
    depends_on:
      - "adversarial_scope"
    outputs:
      - "review.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "review_artifact"
          type: "artifact_contains"
          artifact: "review.md"
          must_contain:
            - "review"
"#
        } else {
            ""
        };
        format!(
            r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "{workflow_id}"
display_name: "Orchestrator Test"
source_prompt: "source_prompt and command strings must stay out of events"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 2
  max_agents: 2
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 120
  max_tokens: 6000
  max_tool_calls: 50
approvals:
  required_before: []
agents:
  - id: "adversarial_scope"
    display_name: "Adversary-Hypatia"
    role: "Attack accidental execution."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "adversarial_review"
    display_name: "Adversary-Cicero"
    role: "Attack stale state."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "adversarial_scope"
    title: "Scope without running verifier commands"
    agent: "adversarial_scope"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on: []
    outputs:
      - "scope.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "scope_commands"
          type: "run_commands"
          cwd: "."
          sandbox: "read-only"
          network: "disabled"
          timeout_seconds: 30
          output_limit_bytes: 2048
          commands:
            - {marker_command}
          expected_exit_code: 0
{second_step}artifacts:
  retention: "until_workflow_complete"
  required:
    - "scope.md"
cleanup:
  on_cancel: []
  on_complete: []
"#,
        )
    }

    fn parallel_branch_workflow_yaml(
        workflow_id: &str,
        step_count: usize,
        max_parallel_steps: u32,
        max_agents: u32,
        max_worktrees: u32,
        title_suffix: &str,
    ) -> String {
        let ancient_names = [
            "Hypatia",
            "Cicero",
            "Euclid",
            "Archimedes",
            "Ptolemy",
            "Aquinas",
        ];
        let agents = (0..step_count)
            .map(|index| {
                let display_name = format!(
                    "Adversary-{}",
                    ancient_names.get(index).copied().unwrap_or("Aristotle")
                );
                format!(
                    r#"  - id: "agent_{index}"
    display_name: "{display_name}"
    role: "Review branch {index}."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
"#
                )
            })
            .collect::<String>();
        let steps = (0..step_count)
            .map(|index| {
                let route = if index == 0 {
                    r#"    model:
      model_gateway: "openrouter"
      provider: "openrouter"
      model: "openai/gpt-oss-120b"
      reasoning: "xhigh"
      service_tier: "priority"
      permission_profile: "workspace-write"
"#
                    .to_string()
                } else {
                    r#"    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
"#
                    .to_string()
                };
                format!(
                    r#"  - id: "branch_{index}"
    title: "Parallel branch {index} {title_suffix}"
    agent: "agent_{index}"
{route}    workspace:
      mode: "isolated_worktree"
    parallel_group: "parallel"
    depends_on: []
    outputs:
      - "branch-{index}.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "artifact_{index}"
          type: "artifact_contains"
          artifact: "branch-{index}.md"
          must_contain:
            - "done"
"#
                )
            })
            .collect::<String>();
        format!(
            r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "{workflow_id}"
display_name: "Parallel Branch Test"
source_prompt: "branch source prompt should not enter workflow events"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: {max_parallel_steps}
  max_agents: {max_agents}
  max_worktrees: {max_worktrees}
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 120
  max_tokens: 6000
  max_tool_calls: 50
approvals:
  required_before: []
agents:
{agents}steps:
{steps}artifacts:
  retention: "until_workflow_complete"
  required: []
cleanup:
  on_cancel: []
  on_complete: []
"#
        )
    }

    async fn create_unprojected_run(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        workflow_id: &str,
        yaml: String,
    ) -> crate::WorkflowRunSnapshot {
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: yaml,
            })
            .await
            .expect("workflow spec should save");
        runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: Some(format!("{workflow_id}-run")),
            })
            .await
            .expect("workflow run should create")
    }

    async fn create_projected_run(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        workflow_id: &str,
        marker: &Path,
        include_second: bool,
    ) -> (
        crate::WorkflowRunSnapshot,
        WorkflowGoalPlanProjectionOutcome,
    ) {
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: orchestrator_workflow_yaml(marker, workflow_id, include_second),
            })
            .await
            .expect("workflow spec should save");
        let run = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: Some(format!("{workflow_id}-run")),
            })
            .await
            .expect("workflow run should create");
        let projection = runtime
            .project_workflow_run_to_goal_plan(WorkflowGoalPlanProjectionParams {
                workflow_run_id: run.run.run_id.clone(),
                thread_id,
                idempotency_key: Some(format!("{workflow_id}-projection")),
            })
            .await
            .expect("workflow projection should succeed")
            .expect("workflow run should project");
        (run, projection)
    }

    async fn admit_test_workflow_run_branches(
        runtime: &StateRuntime,
        params: WorkflowRunBranchAdmissionParams,
    ) -> anyhow::Result<Option<WorkflowRunBranchAdmissionOutcome>> {
        runtime
            .admit_workflow_run_branches_with_provider_env_check(params, |_| true)
            .await
    }

    async fn mark_projected_node_complete(
        runtime: &StateRuntime,
        projection: &WorkflowGoalPlanProjectionOutcome,
        key: &str,
    ) {
        sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET status = 'complete'
WHERE plan_id = ? AND key = ?
            "#,
        )
        .bind(projection.plan_id.as_str())
        .bind(key)
        .execute(runtime.thread_goals().pool.as_ref())
        .await
        .expect("projected node should update");
    }

    #[tokio::test]
    async fn workflow_claim_fences_stale_owner_generation() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let marker = runtime.codex_home().join("claim-marker");
        let (run, _) = create_projected_run(
            &runtime,
            thread_id,
            "wf_claim_fence",
            &marker,
            /*include_second*/ false,
        )
        .await;

        let claimed_a = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-a".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        assert_eq!(1, claimed_a.generation);

        let claimed_b = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-b".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("second claim should not error");
        assert_eq!(None, claimed_b);

        sqlx::query("UPDATE workflow_runs SET lease_expires_at_ms = 0 WHERE run_id = ?")
            .bind(run.run.run_id.as_str())
            .execute(runtime.pool.as_ref())
            .await
            .expect("lease should expire");
        let claimed_b = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-b".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("stale lease claim should succeed")
            .expect("run should reclaim");
        assert_eq!(2, claimed_b.generation);

        let stale_advance = runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id,
                owner_id: "owner-a".to_string(),
                generation: claimed_a.generation,
            })
            .await
            .expect("stale advance should not error");
        assert_eq!(None, stale_advance);
    }

    #[tokio::test]
    async fn workflow_advance_blocks_verifiers_without_executing_commands() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let marker = runtime.codex_home().join("verifier-command-ran");
        let (run, projection) = create_projected_run(
            &runtime,
            thread_id,
            "wf_verifier_block",
            &marker,
            /*include_second*/ true,
        )
        .await;
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");

        let first_advance = runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");
        assert!(first_advance.changed);
        assert_eq!(
            crate::WorkflowRunStatus::Running,
            first_advance.snapshot.run.status
        );
        assert_eq!(
            crate::WorkflowRunStepStatus::Ready,
            first_advance.snapshot.steps[0].status
        );

        mark_projected_node_complete(&runtime, &projection, "adversarial_scope").await;
        let advanced = runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");

        assert!(advanced.changed);
        assert_eq!(
            crate::WorkflowRunStatus::Waiting,
            advanced.snapshot.run.status
        );
        assert_eq!(
            crate::WorkflowRunStepStatus::WaitingVerifier,
            advanced.snapshot.steps[0].status
        );
        let scope_verifier = advanced
            .snapshot
            .verifiers
            .iter()
            .find(|verifier| verifier.step_id == "adversarial_scope")
            .expect("scope verifier should exist");
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Blocked,
            scope_verifier.status
        );
        assert!(
            !marker.exists(),
            "orchestrator skeleton must not execute verifier commands"
        );
        let event_payloads = advanced
            .snapshot
            .events
            .iter()
            .map(|event| event.event_payload_json.to_string())
            .collect::<String>();
        assert!(!event_payloads.contains("touch "));
        assert!(!event_payloads.contains("source_prompt"));
    }

    #[tokio::test]
    async fn workflow_branch_admission_honors_parallel_limits_and_model_route() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_branch_parallel",
            parallel_branch_workflow_yaml(
                "wf_branch_parallel",
                /*step_count*/ 3,
                /*max_parallel_steps*/ 2,
                /*max_agents*/ 3,
                /*max_worktrees*/ 2,
                "no-secret",
            ),
        )
        .await;
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "branch-owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "branch-owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");

        let admitted = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "branch-owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: Some("profile:workflow".to_string()),
                config_fingerprint: Some("cfg-workflow".to_string()),
                version_fingerprint: Some("version-workflow".to_string()),
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("branch admission should succeed")
        .expect("run should still be owned");

        assert!(admitted.changed);
        assert_eq!(2, admitted.admitted.len());
        assert_eq!(
            vec!["branch_0".to_string(), "branch_1".to_string()],
            admitted
                .admitted
                .iter()
                .map(|branch| branch.step_id.clone())
                .collect::<Vec<_>>()
        );
        let active_steps = admitted
            .snapshot
            .steps
            .iter()
            .filter(|step| step.status == crate::WorkflowRunStepStatus::Active)
            .count();
        assert_eq!(2, active_steps);
        let ready_steps = admitted
            .snapshot
            .steps
            .iter()
            .filter(|step| step.status == crate::WorkflowRunStepStatus::Ready)
            .count();
        assert_eq!(1, ready_steps);
        let first_branch = &admitted.admitted[0];
        assert_eq!(
            Some("openrouter"),
            first_branch
                .model_route_json
                .get("model_gateway")
                .and_then(Value::as_str)
        );
        assert_eq!(
            Some("xhigh"),
            first_branch
                .model_route_json
                .get("reasoning")
                .and_then(Value::as_str)
        );
        let first_run = runtime
            .get_background_agent_run(first_branch.background_agent_run_id.as_str())
            .await
            .expect("background run should load")
            .expect("background run should exist");
        assert_eq!(BackgroundAgentRunStatus::Queued, first_run.status);
        assert_eq!(
            Some("profile:workflow"),
            first_run.auth_profile_ref.as_deref()
        );
        let execution_snapshot = runtime
            .get_latest_background_agent_execution_snapshot(
                first_branch.background_agent_run_id.as_str(),
            )
            .await
            .expect("execution snapshot should load")
            .expect("execution snapshot should exist");
        assert_eq!(
            Some("openrouter"),
            execution_snapshot
                .payload_json
                .get("modelGateway")
                .and_then(Value::as_str)
        );
        assert_eq!(
            Some("xhigh"),
            execution_snapshot
                .payload_json
                .get("reasoning")
                .and_then(Value::as_str)
        );
    }

    #[tokio::test]
    async fn workflow_branch_admission_blocks_openrouter_route_without_env_before_agent_run() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_branch_openrouter_missing_env",
            parallel_branch_workflow_yaml(
                "wf_branch_openrouter_missing_env",
                /*step_count*/ 2,
                /*max_parallel_steps*/ 2,
                /*max_agents*/ 2,
                /*max_worktrees*/ 2,
                "missing-openrouter-env",
            ),
        )
        .await;
        let projection = runtime
            .project_workflow_run_to_goal_plan(WorkflowGoalPlanProjectionParams {
                workflow_run_id: run.run.run_id.clone(),
                thread_id,
                idempotency_key: Some("wf_branch_openrouter_missing_env-projection".to_string()),
            })
            .await
            .expect("workflow projection should succeed")
            .expect("workflow run should project");
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "missing-env-owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "missing-env-owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");

        let admitted = runtime
            .admit_workflow_run_branches_with_provider_env_check(
                WorkflowRunBranchAdmissionParams {
                    run_id: run.run.run_id.clone(),
                    owner_id: "missing-env-owner".to_string(),
                    generation: claim.generation,
                    auth_profile_ref: None,
                    config_fingerprint: None,
                    version_fingerprint: None,
                    parent_agent_run_id: None,
                    max_active_background_agent_runs: Some(10),
                },
                |env_key| {
                    assert_eq!(OPENROUTER_API_KEY_ENV_VAR, env_key);
                    false
                },
            )
            .await
            .expect("branch admission should succeed")
            .expect("run should still be owned");

        assert!(admitted.changed);
        assert!(admitted.admitted.is_empty());
        assert_eq!(
            crate::WorkflowRunStatus::Blocked,
            admitted.snapshot.run.status
        );
        let step_states = admitted
            .snapshot
            .steps
            .iter()
            .map(|step| {
                (
                    step.step_id.clone(),
                    step.status.clone(),
                    step.reason_code.clone(),
                    step.background_agent_run_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            vec![
                (
                    "branch_0".to_string(),
                    crate::WorkflowRunStepStatus::Blocked,
                    Some(WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON_CODE.to_string()),
                    None,
                ),
                (
                    "branch_1".to_string(),
                    crate::WorkflowRunStepStatus::Ready,
                    None,
                    None,
                ),
            ],
            step_states
        );
        let branch_verifier = admitted
            .snapshot
            .verifiers
            .iter()
            .find(|verifier| verifier.step_id == "branch_0")
            .expect("blocked branch verifier should exist");
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Blocked,
            branch_verifier.status
        );
        assert_eq!(
            Some(WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON_CODE),
            branch_verifier.reason_code.as_deref()
        );
        let preflight_event = admitted
            .snapshot
            .events
            .iter()
            .find(|event| event.event_type == "branch_preflight_blocked")
            .expect("preflight block event should be recorded");
        let preflight_data = preflight_event
            .event_payload_json
            .get("data")
            .expect("preflight event should store wrapped workflow data");
        assert_eq!(
            Some(OPENROUTER_API_KEY_ENV_VAR),
            preflight_data.get("missingEnvKey").and_then(Value::as_str)
        );
        assert_eq!(
            Some(WORKFLOW_BRANCH_PROVIDER_ENV_MISSING_REASON_CODE),
            preflight_data.get("reasonCode").and_then(Value::as_str)
        );
        assert_eq!(
            Some("openrouter"),
            preflight_data
                .get("route")
                .and_then(|route| route.get("modelGateway"))
                .and_then(Value::as_str)
        );
        assert!(
            !admitted
                .snapshot
                .events
                .iter()
                .any(|event| event.event_type == "branch_admitted")
        );
        let background_agent_runs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_runs")
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("background agent run count should load");
        assert_eq!(0, background_agent_runs);
        let execution_snapshots: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_execution_snapshots")
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("execution snapshot count should load");
        assert_eq!(0, execution_snapshots);
        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(projection.plan_id, plan.plan.plan_id);
        assert_eq!(crate::ThreadGoalPlanStatus::Blocked, plan.plan.status);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Blocked,
                crate::ThreadGoalPlanNodeStatus::Blocked,
            ],
            plan.nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let retry = runtime
            .admit_workflow_run_branches_with_provider_env_check(
                WorkflowRunBranchAdmissionParams {
                    run_id: run.run.run_id,
                    owner_id: "missing-env-owner".to_string(),
                    generation: claim.generation,
                    auth_profile_ref: None,
                    config_fingerprint: None,
                    version_fingerprint: None,
                    parent_agent_run_id: None,
                    max_active_background_agent_runs: Some(10),
                },
                |_| true,
            )
            .await
            .expect("retry admission should succeed")
            .expect("blocked run should still be readable");
        assert!(!retry.changed);
        assert!(retry.admitted.is_empty());
    }

    #[tokio::test]
    async fn workflow_branch_admission_honors_max_worktrees_limit() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_branch_worktree_cap",
            parallel_branch_workflow_yaml(
                "wf_branch_worktree_cap",
                /*step_count*/ 3,
                /*max_parallel_steps*/ 3,
                /*max_agents*/ 3,
                /*max_worktrees*/ 1,
                "worktree-cap",
            ),
        )
        .await;
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "worktree-owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "worktree-owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");

        let first_admission = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "worktree-owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("branch admission should succeed")
        .expect("run should still be owned");

        assert!(first_admission.changed);
        assert_eq!(
            vec!["branch_0".to_string()],
            first_admission
                .admitted
                .iter()
                .map(|branch| branch.step_id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            1,
            first_admission
                .snapshot
                .steps
                .iter()
                .filter(|step| step.status == crate::WorkflowRunStepStatus::Active)
                .count()
        );
        assert_eq!(
            2,
            first_admission
                .snapshot
                .steps
                .iter()
                .filter(|step| step.status == crate::WorkflowRunStepStatus::Ready)
                .count()
        );

        let retry = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "worktree-owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("retry admission should succeed")
        .expect("run should still be owned");
        assert_eq!(0, retry.admitted.len());

        runtime
            .update_background_agent_run_status(
                first_admission.admitted[0].background_agent_run_id.as_str(),
                BackgroundAgentRunStatus::Completed,
                Some("branch completed"),
            )
            .await
            .expect("branch status should update");
        runtime
            .reconcile_workflow_run_branches(WorkflowRunBranchReconcileParams {
                run_id: run.run.run_id.clone(),
                owner_id: "worktree-owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("reconcile should succeed")
            .expect("run should still be owned");

        let second_admission = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id,
                owner_id: "worktree-owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("second branch admission should succeed")
        .expect("run should still be owned");

        assert!(second_admission.changed);
        assert_eq!(
            vec!["branch_1".to_string()],
            second_admission
                .admitted
                .iter()
                .map(|branch| branch.step_id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            1,
            second_admission
                .snapshot
                .steps
                .iter()
                .filter(|step| step.status == crate::WorkflowRunStepStatus::Active)
                .count()
        );
        assert_eq!(
            1,
            second_admission
                .snapshot
                .steps
                .iter()
                .filter(|step| step.status == crate::WorkflowRunStepStatus::Ready)
                .count()
        );
    }

    #[tokio::test]
    async fn workflow_branch_admission_rejects_stale_generation_and_is_idempotent() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_branch_stale",
            parallel_branch_workflow_yaml(
                "wf_branch_stale",
                /*step_count*/ 2,
                /*max_parallel_steps*/ 1,
                /*max_agents*/ 2,
                /*max_worktrees*/ 1,
                "no-secret",
            ),
        )
        .await;
        let first_claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-a".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-a".to_string(),
                generation: first_claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");
        sqlx::query("UPDATE workflow_runs SET lease_expires_at_ms = 0 WHERE run_id = ?")
            .bind(run.run.run_id.as_str())
            .execute(runtime.pool.as_ref())
            .await
            .expect("lease should expire");
        let second_claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-b".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("reclaim should succeed")
            .expect("run should reclaim");

        let stale = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-a".to_string(),
                generation: first_claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("stale admission should not error");
        assert_eq!(None, stale);

        let admitted = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner-b".to_string(),
                generation: second_claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("admission should succeed")
        .expect("run should be owned");
        assert_eq!(1, admitted.admitted.len());

        let retry = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id,
                owner_id: "owner-b".to_string(),
                generation: second_claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("retry should succeed")
        .expect("run should be owned");
        assert_eq!(0, retry.admitted.len());
        let counts = runtime
            .count_background_agent_runs_by_status()
            .await
            .expect("status counts should load");
        assert_eq!(
            Some(1),
            counts
                .iter()
                .find(|(status, _)| *status == BackgroundAgentRunStatus::Queued)
                .map(|(_, count)| *count)
        );
    }

    #[tokio::test]
    async fn workflow_cancel_terminalizes_queued_branch_and_sanitizes_events() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let sentinel = "branch-secret-sentinel";
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_branch_cancel",
            parallel_branch_workflow_yaml(
                "wf_branch_cancel",
                /*step_count*/ 2,
                /*max_parallel_steps*/ 1,
                /*max_agents*/ 2,
                /*max_worktrees*/ 1,
                sentinel,
            ),
        )
        .await;
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");
        let admitted = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("admission should succeed")
        .expect("run should be owned");
        let branch_run_id = admitted.admitted[0].background_agent_run_id.clone();
        let repo = unique_temp_dir().join("repo");
        let worktree = repo.join(".git").join("worktrees").join("branch-lease");
        runtime
            .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
                id: "branch-lease".to_string(),
                run_id: branch_run_id.clone(),
                identity: "branch-lease".to_string(),
                mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
                base_repo_path: repo,
                worktree_path: worktree,
                branch: Some("codewith/branch-lease".to_string()),
                head_sha: Some("abc123".to_string()),
                status_snapshot_json: json!({"dirty": true, "paths": ["src/user-work.rs"]}),
                dirty: true,
                cleanup_after: None,
            })
            .await
            .expect("branch worktree lease should create");
        runtime
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: run.run.run_id.clone(),
                reason: "stop".to_string(),
            })
            .await
            .expect("cancel should succeed")
            .expect("workflow should exist");
        let cancel_claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("cancel claim should succeed")
            .expect("cancel requested run should claim");
        let cancelled = runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id,
                owner_id: "owner".to_string(),
                generation: cancel_claim.generation,
            })
            .await
            .expect("cancel advance should succeed")
            .expect("run should advance");

        let branch = runtime
            .get_background_agent_run(branch_run_id.as_str())
            .await
            .expect("branch should load")
            .expect("branch should exist");
        assert_eq!(BackgroundAgentDesiredState::Stopped, branch.desired_state);
        assert_eq!(BackgroundAgentRunStatus::Cancelled, branch.status);
        let active_assignment_after_cancel: (i64,) = sqlx::query_as(
            r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ? AND detached_at_ms IS NULL
            "#,
        )
        .bind("branch-lease")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("assignment count should load");
        assert_eq!(active_assignment_after_cancel, (0,));
        let cleanup_candidates = runtime
            .managed_worktrees()
            .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
            .await
            .expect("cleanup candidates should load");
        assert!(
            cleanup_candidates
                .iter()
                .any(|worktree| worktree.worktree_id == "branch-lease")
        );
        let cleanup_candidate = cleanup_candidates
            .iter()
            .find(|worktree| worktree.worktree_id == "branch-lease")
            .expect("dirty branch lease should be a cleanup candidate");
        assert!(cleanup_candidate.dirty);
        let tombstone: (i64, String) = sqlx::query_as(
            r#"
SELECT dirty_worktree, payload_json
FROM background_agent_cleanup_tombstones
WHERE run_id = ?
            "#,
        )
        .bind(branch_run_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("cleanup tombstone should load");
        assert_eq!(tombstone.0, 1);
        let payload: Value =
            serde_json::from_str(tombstone.1.as_str()).expect("payload should be JSON");
        assert_eq!(payload["forceDeleteRequired"], true);
        assert_eq!(
            crate::WorkflowRunStatus::Cancelled,
            cancelled.snapshot.run.status
        );
        let workflow_events = cancelled
            .snapshot
            .events
            .iter()
            .map(|event| event.event_payload_json.to_string())
            .collect::<String>();
        assert!(!workflow_events.contains(sentinel));
    }

    #[tokio::test]
    async fn workflow_branch_completion_moves_step_to_verifier_gate() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_branch_complete",
            parallel_branch_workflow_yaml(
                "wf_branch_complete",
                /*step_count*/ 2,
                /*max_parallel_steps*/ 1,
                /*max_agents*/ 2,
                /*max_worktrees*/ 1,
                "no-secret",
            ),
        )
        .await;
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");
        let admitted = admit_test_workflow_run_branches(
            &runtime,
            WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            },
        )
        .await
        .expect("admission should succeed")
        .expect("run should be owned");
        runtime
            .update_background_agent_run_status(
                admitted.admitted[0].background_agent_run_id.as_str(),
                BackgroundAgentRunStatus::Completed,
                Some("test completed"),
            )
            .await
            .expect("branch status should update");

        let reconciled = runtime
            .reconcile_workflow_run_branches(WorkflowRunBranchReconcileParams {
                run_id: run.run.run_id,
                owner_id: "owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("reconcile should succeed")
            .expect("run should be owned");

        assert!(reconciled.changed);
        assert_eq!(
            crate::WorkflowRunStepStatus::WaitingVerifier,
            reconciled.snapshot.steps[0].status
        );
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Blocked,
            reconciled.snapshot.verifiers[0].status
        );
    }

    #[tokio::test]
    async fn workflow_cancel_blocks_projected_goal_plan_and_sanitizes_reason() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let marker = runtime.codex_home().join("cancel-marker");
        let (run, projection) = create_projected_run(
            &runtime,
            thread_id,
            "wf_cancel_projection",
            &marker,
            /*include_second*/ false,
        )
        .await;
        let raw_reason = "source_prompt: secret\ncommands:\n  - touch should-not-leak";

        let cancelled = runtime
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: run.run.run_id.clone(),
                reason: raw_reason.to_string(),
            })
            .await
            .expect("cancel should succeed")
            .expect("workflow run should exist");

        assert_eq!(
            crate::WorkflowRunStatus::CancelRequested,
            cancelled.run.status
        );
        assert_eq!(
            Some("user requested workflow cancellation"),
            cancelled.run.status_reason.as_deref()
        );
        let event_payloads = cancelled
            .events
            .iter()
            .map(|event| event.event_payload_json.to_string())
            .collect::<String>();
        assert!(!event_payloads.contains("should-not-leak"));
        assert!(!event_payloads.contains("source_prompt"));

        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(projection.plan_id, plan.plan.plan_id);
        assert_eq!(crate::ThreadGoalPlanStatus::Blocked, plan.plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Blocked],
            plan.nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let activation = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(thread_id, plan.nodes[0].node_id.as_str())
            .await
            .expect("activation should not error");
        assert_eq!(None, activation);
        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("thread goal should read")
        );
    }

    #[tokio::test]
    async fn workflow_pause_and_resume_sync_projected_goal_plan() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let marker = runtime.codex_home().join("pause-marker");
        let (run, projection) = create_projected_run(
            &runtime,
            thread_id,
            "wf_pause_projection",
            &marker,
            /*include_second*/ false,
        )
        .await;
        let node_id = projection.snapshot.nodes[0].node_id.clone();
        let activation = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(thread_id, node_id.as_str())
            .await
            .expect("activation should succeed")
            .expect("node should activate");
        let active_goal = activation
            .activated_goal
            .expect("projection activation should create a goal");
        assert_eq!(crate::ThreadGoalStatus::Active, active_goal.status);

        let paused = runtime
            .pause_workflow_run(WorkflowRunPauseParams {
                run_id: run.run.run_id.clone(),
                reason: "pause".to_string(),
            })
            .await
            .expect("pause should succeed")
            .expect("workflow run should exist");

        assert_eq!(crate::WorkflowRunStatus::Paused, paused.run.status);
        let goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("thread goal should read")
            .expect("active goal should still exist");
        assert_eq!(crate::ThreadGoalStatus::Paused, goal.status);
        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(crate::ThreadGoalPlanStatus::Paused, plan.plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Paused],
            plan.nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let resumed = runtime
            .resume_workflow_run(WorkflowRunResumeParams {
                run_id: run.run.run_id.clone(),
            })
            .await
            .expect("resume should succeed")
            .expect("workflow run should exist");

        assert_eq!(crate::WorkflowRunStatus::Waiting, resumed.run.status);
        let goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("thread goal should read")
            .expect("active goal should still exist");
        assert_eq!(crate::ThreadGoalStatus::Active, goal.status);
        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(projection.plan_id, plan.plan.plan_id);
        assert_eq!(crate::ThreadGoalPlanStatus::Active, plan.plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Active],
            plan.nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        runtime
            .thread_goals()
            .pause_workflow_goal_plan_projection(run.run.run_id.as_str())
            .await
            .expect("projection pause should succeed");
        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(crate::ThreadGoalPlanStatus::Paused, plan.plan.status);

        let resumed_again = runtime
            .resume_workflow_run(WorkflowRunResumeParams {
                run_id: run.run.run_id.clone(),
            })
            .await
            .expect("second resume should succeed")
            .expect("workflow run should exist");

        assert_eq!(crate::WorkflowRunStatus::Waiting, resumed_again.run.status);
        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(projection.plan_id, plan.plan.plan_id);
        assert_eq!(crate::ThreadGoalPlanStatus::Paused, plan.plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Paused],
            plan.nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn workflow_terminal_cancel_does_not_block_projected_goal_plan() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let marker = runtime.codex_home().join("terminal-cancel-marker");
        let (run, projection) = create_projected_run(
            &runtime,
            thread_id,
            "wf_terminal_cancel",
            &marker,
            /*include_second*/ false,
        )
        .await;
        sqlx::query(
            r#"
UPDATE workflow_runs
SET status = ?, updated_at_ms = updated_at_ms + 1
WHERE run_id = ?
            "#,
        )
        .bind(crate::WorkflowRunStatus::Completed.as_str())
        .bind(run.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("workflow run should update");

        let cancelled = runtime
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: run.run.run_id,
                reason: "cancel completed run".to_string(),
            })
            .await
            .expect("terminal cancel should succeed")
            .expect("workflow run should exist");

        assert_eq!(crate::WorkflowRunStatus::Completed, cancelled.run.status);
        let plan = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list")
            .pop()
            .expect("projection plan should exist");
        assert_eq!(projection.plan_id, plan.plan.plan_id);
        assert_eq!(crate::ThreadGoalPlanStatus::Active, plan.plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Pending],
            plan.nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn workflow_cancel_requested_can_be_claimed_and_finalized() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let marker = runtime.codex_home().join("final-cancel-marker");
        let (run, _) = create_projected_run(
            &runtime,
            thread_id,
            "wf_cancel_finalize",
            &marker,
            /*include_second*/ false,
        )
        .await;
        runtime
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: run.run.run_id.clone(),
                reason: "stop".to_string(),
            })
            .await
            .expect("cancel should succeed");
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "canceller".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("cancelled run should claim")
            .expect("cancel requested run should be claimable");

        let advanced = runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id,
                owner_id: "canceller".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("cancel advance should succeed")
            .expect("run should advance");

        assert!(advanced.changed);
        assert_eq!(
            crate::WorkflowRunStatus::Cancelled,
            advanced.snapshot.run.status
        );
        assert!(
            advanced
                .snapshot
                .steps
                .iter()
                .all(|step| step.status == crate::WorkflowRunStepStatus::Cancelled)
        );
        assert!(
            advanced
                .snapshot
                .verifiers
                .iter()
                .all(|verifier| verifier.status == crate::WorkflowRunStepVerifierStatus::Skipped)
        );
    }

    fn gated_approval_workflow_yaml() -> String {
        r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "wf_gated_approval"
display_name: "Gated Approval Test"
source_prompt: "gated source prompt should not enter workflow events"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 2
  max_agents: 2
  max_worktrees: 2
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 120
  max_tokens: 6000
  max_tool_calls: 50
approvals:
  required_before:
    - "human-review"
agents:
  - id: "agent_open"
    display_name: "Adversary-Hypatia"
    role: "Attack the open branch."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "agent_gated"
    display_name: "Adversary-Cicero"
    role: "Attack the gated branch."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "open_step"
    title: "Open branch adversarial step"
    agent: "agent_open"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    workspace:
      mode: "isolated_worktree"
    parallel_group: "parallel"
    depends_on: []
    outputs:
      - "open.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "open_artifact"
          type: "artifact_contains"
          artifact: "open.md"
          must_contain:
            - "done"
  - id: "gated_step"
    title: "Gated branch adversarial step"
    agent: "agent_gated"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    workspace:
      mode: "isolated_worktree"
    parallel_group: "parallel"
    approval_gate: "human-review"
    depends_on: []
    outputs:
      - "gated.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "gated_artifact"
          type: "artifact_contains"
          artifact: "gated.md"
          must_contain:
            - "done"
artifacts:
  retention: "until_workflow_complete"
  required: []
cleanup:
  on_cancel: []
  on_complete: []
"#
        .to_string()
    }

    fn find_step<'a>(
        snapshot: &'a crate::WorkflowRunSnapshot,
        step_id: &str,
    ) -> &'a crate::WorkflowRunStep {
        snapshot
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .unwrap_or_else(|| panic!("snapshot should contain step `{step_id}`"))
    }

    async fn claim_and_advance(runtime: &StateRuntime, run_id: &str, owner_id: &str) -> i64 {
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run_id.to_string(),
                owner_id: owner_id.to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("claim should succeed")
            .expect("run should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run_id.to_string(),
                owner_id: owner_id.to_string(),
                generation: claim.generation,
            })
            .await
            .expect("advance should succeed")
            .expect("run should advance");
        claim.generation
    }

    async fn admit_branches(
        runtime: &StateRuntime,
        run_id: &str,
        owner_id: &str,
        generation: i64,
    ) -> WorkflowRunBranchAdmissionOutcome {
        runtime
            .admit_workflow_run_branches(WorkflowRunBranchAdmissionParams {
                run_id: run_id.to_string(),
                owner_id: owner_id.to_string(),
                generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            })
            .await
            .expect("branch admission should succeed")
            .expect("run should still be owned")
    }

    #[tokio::test]
    async fn gated_step_requires_explicit_approval_before_branch_admission() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_gated_approval",
            gated_approval_workflow_yaml(),
        )
        .await;
        let run_id = run.run.run_id.clone();

        // The gated step is persisted awaiting a decision; the open step has no gate.
        assert_eq!(
            Some("pending"),
            find_step(&run, "gated_step").approval_state.as_deref()
        );
        assert_eq!(
            Some("human-review"),
            find_step(&run, "gated_step").approval_gate.as_deref()
        );
        assert_eq!(None, find_step(&run, "open_step").approval_state);

        let owner = "approval-owner";
        let generation = claim_and_advance(&runtime, run_id.as_str(), owner).await;

        // Only the ungated step is admitted while the gate is unresolved.
        let first = admit_branches(&runtime, run_id.as_str(), owner, generation).await;
        assert_eq!(
            vec!["open_step".to_string()],
            first
                .admitted
                .iter()
                .map(|branch| branch.step_id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            crate::WorkflowRunStepStatus::Ready,
            find_step(&first.snapshot, "gated_step").status,
            "gated step must stay ready until approved"
        );

        // A repeat admission changes nothing while the gate is still pending.
        let stalled = admit_branches(&runtime, run_id.as_str(), owner, generation).await;
        assert!(stalled.admitted.is_empty());

        // Explicit user approval clears the gate and records provenance.
        let approval = runtime
            .workflows()
            .set_workflow_run_step_approval(crate::WorkflowRunStepApprovalParams {
                run_id: run_id.clone(),
                step_id: "gated_step".to_string(),
                decision: crate::WorkflowRunStepApprovalDecision::Approve,
                reason: Some("reviewed and safe".to_string()),
                actor_id: Some("user-1".to_string()),
            })
            .await
            .expect("approval should succeed")
            .expect("run should exist");
        assert!(approval.changed);
        assert!(approval.gate_present);
        assert_eq!(
            Some("approved"),
            find_step(&approval.snapshot, "gated_step")
                .approval_state
                .as_deref()
        );
        let approval_event = approval
            .snapshot
            .events
            .iter()
            .find(|event| event.event_type == "step_approval_granted")
            .expect("approval event should be recorded");
        assert_eq!("user", approval_event.actor_kind);
        assert_eq!(Some("user-1".to_string()), approval_event.actor_id);
        assert_eq!(
            "gated_step",
            approval_event.event_payload_json["data"]["stepId"]
                .as_str()
                .expect("stepId")
        );
        assert_eq!(
            true,
            approval_event.event_payload_json["data"]["reasonProvided"]
        );

        // Re-approving is idempotent.
        let repeat = runtime
            .workflows()
            .set_workflow_run_step_approval(crate::WorkflowRunStepApprovalParams {
                run_id: run_id.clone(),
                step_id: "gated_step".to_string(),
                decision: crate::WorkflowRunStepApprovalDecision::Approve,
                reason: None,
                actor_id: None,
            })
            .await
            .expect("repeat approval should succeed")
            .expect("run should exist");
        assert!(!repeat.changed);

        // The approved step is now admitted on the next tick.
        let second = admit_branches(&runtime, run_id.as_str(), owner, generation).await;
        assert_eq!(
            vec!["gated_step".to_string()],
            second
                .admitted
                .iter()
                .map(|branch| branch.step_id.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn rejecting_a_gated_step_skips_it_and_ignores_ungated_steps() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run = create_unprojected_run(
            &runtime,
            thread_id,
            "wf_gated_approval",
            gated_approval_workflow_yaml(),
        )
        .await;
        let run_id = run.run.run_id.clone();

        // Rejecting a gated step skips it and records a sanitized reason only.
        let rejection = runtime
            .workflows()
            .set_workflow_run_step_approval(crate::WorkflowRunStepApprovalParams {
                run_id: run_id.clone(),
                step_id: "gated_step".to_string(),
                decision: crate::WorkflowRunStepApprovalDecision::Reject,
                reason: Some("commands:\n  - should-not-leak".to_string()),
                actor_id: Some("user-1".to_string()),
            })
            .await
            .expect("rejection should succeed")
            .expect("run should exist");
        assert!(rejection.changed);
        let rejected = find_step(&rejection.snapshot, "gated_step");
        assert_eq!(crate::WorkflowRunStepStatus::Skipped, rejected.status);
        assert_eq!(Some("rejected"), rejected.approval_state.as_deref());
        assert_eq!(
            Some("user_rejected_approval"),
            rejected.reason_code.as_deref()
        );
        assert!(!rejection.snapshot.events.iter().any(|event| {
            serde_json::to_string(&event.event_payload_json)
                .expect("payload should serialize")
                .contains("should-not-leak")
        }));

        // Ungated steps are treated as no-ops and never fabricate a gate.
        let ungated = runtime
            .workflows()
            .set_workflow_run_step_approval(crate::WorkflowRunStepApprovalParams {
                run_id: run_id.clone(),
                step_id: "open_step".to_string(),
                decision: crate::WorkflowRunStepApprovalDecision::Approve,
                reason: None,
                actor_id: None,
            })
            .await
            .expect("ungated approval should succeed")
            .expect("run should exist");
        assert!(!ungated.changed);
        assert!(!ungated.gate_present);
        assert_eq!(
            None,
            find_step(&ungated.snapshot, "open_step").approval_state
        );

        // Unknown steps and runs resolve to None instead of erroring.
        let unknown_step = runtime
            .workflows()
            .set_workflow_run_step_approval(crate::WorkflowRunStepApprovalParams {
                run_id: run_id.clone(),
                step_id: "missing_step".to_string(),
                decision: crate::WorkflowRunStepApprovalDecision::Approve,
                reason: None,
                actor_id: None,
            })
            .await
            .expect("unknown step should not error");
        assert!(unknown_step.is_none());

        // The rejected gated step is never admitted for execution.
        let owner = "reject-owner";
        let generation = claim_and_advance(&runtime, run_id.as_str(), owner).await;
        let admission = admit_branches(&runtime, run_id.as_str(), owner, generation).await;
        assert!(
            !admission
                .admitted
                .iter()
                .any(|branch| branch.step_id == "gated_step")
        );
    }
}
