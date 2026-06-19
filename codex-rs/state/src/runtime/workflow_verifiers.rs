use super::*;
use crate::runtime::workflow_automation::arm_workflow_timers_for_succeeded_step_in_tx;
use crate::runtime::workflow_orchestrator::claim_checked_workflow_run_in_tx;
use crate::runtime::workflows::WorkflowRunEventAppend;
use crate::runtime::workflows::append_workflow_run_event_in_tx;
use crate::runtime::workflows::snapshot_workflow_run_in_tx;
use crate::runtime::workflows::workflow_state_json_string;
use serde_json::json;
use sqlx::AssertSqlSafe;
use sqlx::Row;

const VERIFIER_RUNNING_REASON: &str = "deterministic verifier is running";
const VERIFIER_RUNNING_REASON_CODE: &str = "verifier_running";
const VERIFIER_FAILED_REASON: &str = "deterministic verifier failed";
const VERIFIER_FAILED_REASON_CODE: &str = "verifier_failed";
const VERIFIER_RETRY_PENDING_REASON: &str = "deterministic verifier retry is pending";
const VERIFIER_RETRY_PENDING_REASON_CODE: &str = "verifier_retry_pending";
const VERIFIER_RUNNING_LEASE_EXTENSION_MS: i64 = 31 * 60 * 1000;
const MAX_VERIFIER_RETRY_ATTEMPTS: i64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunVerifierClaimSelection {
    NextRunCommands,
    VerifierRunId(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunVerifierClaimParams {
    pub run_id: String,
    pub owner_id: String,
    pub generation: i64,
    pub selection: WorkflowRunVerifierClaimSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunVerifierClaimOutcome {
    pub run: crate::WorkflowRun,
    pub step: crate::WorkflowRunStep,
    pub verifier: crate::WorkflowRunStepVerifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowRunVerifierOutcomeStatus {
    Passed,
    Failed,
}

impl WorkflowRunVerifierOutcomeStatus {
    fn verifier_status(self) -> crate::WorkflowRunStepVerifierStatus {
        match self {
            Self::Passed => crate::WorkflowRunStepVerifierStatus::Passed,
            Self::Failed => crate::WorkflowRunStepVerifierStatus::Failed,
        }
    }

    fn event_type(self) -> &'static str {
        match self {
            Self::Passed => "verifier_passed",
            Self::Failed => "verifier_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunVerifierResultSummary {
    pub command_count: i64,
    pub expected_exit_code: Option<i32>,
    pub observed_exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: i64,
    pub output_bytes: i64,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunVerifierRecordResultParams {
    pub run_id: String,
    pub owner_id: String,
    pub generation: i64,
    pub verifier_run_id: String,
    pub outcome: WorkflowRunVerifierOutcomeStatus,
    pub summary: WorkflowRunVerifierResultSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunVerifierRecordResultOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    pub retry_pending: bool,
}

impl StateRuntime {
    pub async fn claim_workflow_run_verifier(
        &self,
        params: WorkflowRunVerifierClaimParams,
    ) -> anyhow::Result<Option<WorkflowRunVerifierClaimOutcome>> {
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
            tx.commit().await?;
            return Ok(None);
        }

        let row = match &params.selection {
            WorkflowRunVerifierClaimSelection::NextRunCommands => {
                claim_next_run_commands_verifier_in_tx(&mut tx, params.run_id.as_str(), now_ms)
                    .await?
            }
            WorkflowRunVerifierClaimSelection::VerifierRunId(verifier_run_id) => {
                claim_verifier_by_id_in_tx(
                    &mut tx,
                    params.run_id.as_str(),
                    verifier_run_id.as_str(),
                    now_ms,
                )
                .await?
            }
        };
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };

        let step_run_id: String = row.try_get("step_run_id")?;
        let step_id: String = row.try_get("step_id")?;
        let verifier_run_id: String = row.try_get("verifier_run_id")?;
        let verifier_id: String = row.try_get("verifier_id")?;
        let verifier_type: String = row.try_get("verifier_type")?;
        let attempt_count: i64 = row.try_get("attempt_count")?;
        let lease_expires_at_ms = now_ms.saturating_add(VERIFIER_RUNNING_LEASE_EXTENSION_MS);
        let lease_extended = sqlx::query(
            r#"
UPDATE workflow_runs
SET
    lease_expires_at_ms = CASE
        WHEN lease_expires_at_ms IS NULL OR lease_expires_at_ms < ?
        THEN ?
        ELSE lease_expires_at_ms
    END,
    heartbeat_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND owner_id = ?
  AND generation = ?
            "#,
        )
        .bind(lease_expires_at_ms)
        .bind(lease_expires_at_ms)
        .bind(now_ms)
        .bind(now_ms)
        .bind(params.run_id.as_str())
        .bind(params.owner_id.as_str())
        .bind(params.generation)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if lease_extended == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        append_workflow_run_event_in_tx(
            &mut tx,
            params.run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "verifier_started",
                actor_kind: "workflow_verifier",
                actor_id: Some(params.owner_id),
                step_run_id: Some(step_run_id),
                verifier_run_id: Some(verifier_run_id.clone()),
                visibility: "internal",
                payload: json!({
                    "stepId": step_id,
                    "verifierId": verifier_id,
                    "verifierType": verifier_type,
                    "attempt": attempt_count,
                }),
                now_ms,
            },
        )
        .await?;

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;

        let step = snapshot
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("claimed workflow step {step_id} was not found"))?;
        let verifier = snapshot
            .verifiers
            .iter()
            .find(|verifier| verifier.verifier_run_id == verifier_run_id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("claimed workflow verifier {verifier_run_id} was not found")
            })?;

        Ok(Some(WorkflowRunVerifierClaimOutcome {
            run: snapshot.run,
            step,
            verifier,
        }))
    }

    pub async fn record_workflow_run_verifier_result(
        &self,
        params: WorkflowRunVerifierRecordResultParams,
    ) -> anyhow::Result<Option<WorkflowRunVerifierRecordResultOutcome>> {
        validate_verifier_result_summary(&params)?;
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
            tx.commit().await?;
            return Ok(None);
        }

        let Some(verifier) =
            running_verifier_for_update_in_tx(&mut tx, params.run_id.as_str(), &params).await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        let retry_pending = params.outcome == WorkflowRunVerifierOutcomeStatus::Failed
            && verifier.max_attempts.is_some_and(|max_attempts| {
                verifier.attempt_count < max_attempts.clamp(1, MAX_VERIFIER_RETRY_ATTEMPTS)
            });

        if retry_pending {
            mark_verifier_retry_pending_in_tx(&mut tx, &verifier, &params, now_ms).await?;
        } else {
            mark_verifier_terminal_in_tx(&mut tx, &verifier, &params, now_ms).await?;
            match params.outcome {
                WorkflowRunVerifierOutcomeStatus::Passed => {
                    maybe_promote_step_after_verifier_pass_in_tx(
                        &mut tx,
                        &verifier,
                        params.owner_id.as_str(),
                        now_ms,
                    )
                    .await?;
                }
                WorkflowRunVerifierOutcomeStatus::Failed => {
                    mark_step_and_run_failed_in_tx(
                        &mut tx,
                        &verifier,
                        params.owner_id.as_str(),
                        now_ms,
                    )
                    .await?;
                }
            }
        }

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunVerifierRecordResultOutcome {
            snapshot,
            retry_pending,
        }))
    }
}

async fn claim_next_run_commands_verifier_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    now_ms: i64,
) -> anyhow::Result<Option<sqlx::sqlite::SqliteRow>> {
    sqlx::query(AssertSqlSafe(claim_verifier_sql(
        r#"
      AND verifier.verifier_type = 'run_commands'
ORDER BY step.sequence, verifier.verifier_id
LIMIT 1
        "#,
    )))
    .bind(crate::WorkflowRunStepVerifierStatus::Running.as_str())
    .bind(VERIFIER_RUNNING_REASON)
    .bind(VERIFIER_RUNNING_REASON_CODE)
    .bind(now_ms)
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(anyhow::Error::from)
}

async fn claim_verifier_by_id_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    verifier_run_id: &str,
    now_ms: i64,
) -> anyhow::Result<Option<sqlx::sqlite::SqliteRow>> {
    sqlx::query(AssertSqlSafe(claim_verifier_sql(
        r#"
      AND verifier.verifier_run_id = ?
LIMIT 1
        "#,
    )))
    .bind(crate::WorkflowRunStepVerifierStatus::Running.as_str())
    .bind(VERIFIER_RUNNING_REASON)
    .bind(VERIFIER_RUNNING_REASON_CODE)
    .bind(now_ms)
    .bind(run_id)
    .bind(verifier_run_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(anyhow::Error::from)
}

fn claim_verifier_sql(selection_predicate: &'static str) -> String {
    format!(
        r#"
UPDATE workflow_run_step_verifiers
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    attempt_count = attempt_count + 1,
    updated_at_ms = ?
WHERE verifier_run_id = (
    SELECT verifier.verifier_run_id
    FROM workflow_run_step_verifiers verifier
    JOIN workflow_run_steps step
      ON step.run_id = verifier.run_id
     AND step.step_id = verifier.step_id
    WHERE verifier.run_id = ?
      AND verifier.status IN ('pending', 'blocked')
      AND (
          verifier.reason_code IS NULL
          OR verifier.reason_code IN ('verifier_executor_pending', 'verifier_retry_pending')
      )
      AND step.status = 'waiting_verifier'
{selection_predicate}
)
RETURNING
    verifier_run_id,
    run_id,
    step_id,
    verifier_id,
    verifier_type,
    status,
    status_reason,
    reason_code,
    definition_json,
    last_result_json,
    attempt_count,
    max_attempts,
    created_at_ms,
    updated_at_ms,
    completed_at_ms,
    (
        SELECT step_run_id
        FROM workflow_run_steps step
        WHERE step.run_id = workflow_run_step_verifiers.run_id
          AND step.step_id = workflow_run_step_verifiers.step_id
    ) AS step_run_id
        "#
    )
}

async fn running_verifier_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    params: &WorkflowRunVerifierRecordResultParams,
) -> anyhow::Result<Option<crate::WorkflowRunStepVerifier>> {
    let row = sqlx::query(
        r#"
SELECT
    verifier_run_id,
    run_id,
    step_id,
    verifier_id,
    verifier_type,
    status,
    status_reason,
    reason_code,
    definition_json,
    last_result_json,
    attempt_count,
    max_attempts,
    created_at_ms,
    updated_at_ms,
    completed_at_ms
FROM workflow_run_step_verifiers
WHERE run_id = ?
  AND verifier_run_id = ?
  AND status = 'running'
        "#,
    )
    .bind(run_id)
    .bind(params.verifier_run_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;

    row.map(|row| crate::model::WorkflowRunStepVerifierRow::try_from_row(&row)?.try_into())
        .transpose()
}

async fn mark_verifier_retry_pending_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
    params: &WorkflowRunVerifierRecordResultParams,
    now_ms: i64,
) -> anyhow::Result<()> {
    update_verifier_result_in_tx(
        tx,
        verifier,
        params,
        VerifierResultUpdate {
            status: crate::WorkflowRunStepVerifierStatus::Blocked,
            status_reason: Some(VERIFIER_RETRY_PENDING_REASON),
            reason_code: Some(VERIFIER_RETRY_PENDING_REASON_CODE),
            completed_at_ms: None,
            now_ms,
        },
    )
    .await?;
    append_verifier_result_event_in_tx(
        tx,
        verifier,
        params,
        "verifier_retry_pending",
        Some(VERIFIER_RETRY_PENDING_REASON_CODE),
        now_ms,
    )
    .await
}

async fn mark_verifier_terminal_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
    params: &WorkflowRunVerifierRecordResultParams,
    now_ms: i64,
) -> anyhow::Result<()> {
    let (reason, reason_code) = match params.outcome {
        WorkflowRunVerifierOutcomeStatus::Passed => (None, None),
        WorkflowRunVerifierOutcomeStatus::Failed => (
            Some(VERIFIER_FAILED_REASON),
            Some(VERIFIER_FAILED_REASON_CODE),
        ),
    };
    update_verifier_result_in_tx(
        tx,
        verifier,
        params,
        VerifierResultUpdate {
            status: params.outcome.verifier_status(),
            status_reason: reason,
            reason_code,
            completed_at_ms: Some(now_ms),
            now_ms,
        },
    )
    .await?;
    append_verifier_result_event_in_tx(
        tx,
        verifier,
        params,
        params.outcome.event_type(),
        reason_code,
        now_ms,
    )
    .await
}

async fn update_verifier_result_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
    params: &WorkflowRunVerifierRecordResultParams,
    update: VerifierResultUpdate<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
UPDATE workflow_run_step_verifiers
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    last_result_json = ?,
    updated_at_ms = ?,
    completed_at_ms = ?
WHERE verifier_run_id = ?
  AND status = 'running'
        "#,
    )
    .bind(update.status.as_str())
    .bind(update.status_reason)
    .bind(update.reason_code)
    .bind(sanitized_verifier_result_json(
        verifier,
        params.outcome,
        &params.summary,
        update.reason_code,
    )?)
    .bind(update.now_ms)
    .bind(update.completed_at_ms)
    .bind(verifier.verifier_run_id.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

struct VerifierResultUpdate<'a> {
    status: crate::WorkflowRunStepVerifierStatus,
    status_reason: Option<&'a str>,
    reason_code: Option<&'a str>,
    completed_at_ms: Option<i64>,
    now_ms: i64,
}

async fn append_verifier_result_event_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
    params: &WorkflowRunVerifierRecordResultParams,
    event_type: &'static str,
    reason_code: Option<&str>,
    now_ms: i64,
) -> anyhow::Result<()> {
    let step_run_id = step_run_id_for_verifier_in_tx(tx, verifier).await?;
    append_workflow_run_event_in_tx(
        tx,
        params.run_id.as_str(),
        WorkflowRunEventAppend {
            event_type,
            actor_kind: "workflow_verifier",
            actor_id: Some(params.owner_id.clone()),
            step_run_id,
            verifier_run_id: Some(verifier.verifier_run_id.clone()),
            visibility: "internal",
            payload: json!({
                "stepId": verifier.step_id,
                "verifierId": verifier.verifier_id,
                "verifierType": verifier.verifier_type,
                "attempt": verifier.attempt_count,
                "outcome": match params.outcome {
                    WorkflowRunVerifierOutcomeStatus::Passed => "passed",
                    WorkflowRunVerifierOutcomeStatus::Failed => "failed",
                },
                "reasonCode": reason_code,
                "commandCount": params.summary.command_count,
                "observedExitCode": params.summary.observed_exit_code,
                "timedOut": params.summary.timed_out,
                "durationMs": params.summary.duration_ms,
                "outputBytes": params.summary.output_bytes,
                "outputTruncated": params.summary.output_truncated,
            }),
            now_ms,
        },
    )
    .await?;
    Ok(())
}

async fn step_run_id_for_verifier_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
) -> anyhow::Result<Option<String>> {
    sqlx::query_scalar(
        r#"
SELECT step_run_id
FROM workflow_run_steps
WHERE run_id = ? AND step_id = ?
        "#,
    )
    .bind(verifier.run_id.as_str())
    .bind(verifier.step_id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(anyhow::Error::from)
}

async fn maybe_promote_step_after_verifier_pass_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    let incomplete_count: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM workflow_run_step_verifiers
WHERE run_id = ?
  AND step_id = ?
  AND status != 'passed'
        "#,
    )
    .bind(verifier.run_id.as_str())
    .bind(verifier.step_id.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if incomplete_count != 0 {
        return Ok(());
    }

    let row = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = NULL,
    reason_code = NULL,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND step_id = ?
  AND status = 'waiting_verifier'
RETURNING step_run_id
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Succeeded.as_str())
    .bind(now_ms)
    .bind(now_ms)
    .bind(verifier.run_id.as_str())
    .bind(verifier.step_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = row {
        append_workflow_run_event_in_tx(
            tx,
            verifier.run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "step_succeeded",
                actor_kind: "workflow_verifier",
                actor_id: Some(owner_id.to_string()),
                step_run_id: Some(row.try_get("step_run_id")?),
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({ "stepId": verifier.step_id }),
                now_ms,
            },
        )
        .await?;
        arm_workflow_timers_for_succeeded_step_in_tx(
            tx,
            verifier.run_id.as_str(),
            verifier.step_id.as_str(),
            owner_id,
            now_ms,
        )
        .await?;
    }
    maybe_complete_run_after_step_success_in_tx(tx, verifier.run_id.as_str(), owner_id, now_ms)
        .await
}

async fn maybe_complete_run_after_step_success_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    let unfinished_count: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM workflow_run_steps
WHERE run_id = ?
  AND status NOT IN ('succeeded', 'skipped')
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    if unfinished_count != 0 {
        return Ok(());
    }
    let updated = sqlx::query(
        r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = NULL,
    reason_code = NULL,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(crate::WorkflowRunStatus::Completed.as_str())
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(());
    }
    append_workflow_run_event_in_tx(
        tx,
        run_id,
        WorkflowRunEventAppend {
            event_type: "run_status_changed",
            actor_kind: "workflow_verifier",
            actor_id: Some(owner_id.to_string()),
            step_run_id: None,
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({
                "status": crate::WorkflowRunStatus::Completed.as_str(),
                "reasonCode": null,
            }),
            now_ms,
        },
    )
    .await?;
    Ok(())
}

async fn mark_step_and_run_failed_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    verifier: &crate::WorkflowRunStepVerifier,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    let row = sqlx::query(
        r#"
UPDATE workflow_run_steps
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND step_id = ?
  AND status NOT IN ('succeeded', 'failed', 'cancelled', 'skipped')
RETURNING step_run_id
        "#,
    )
    .bind(crate::WorkflowRunStepStatus::Failed.as_str())
    .bind(VERIFIER_FAILED_REASON)
    .bind(VERIFIER_FAILED_REASON_CODE)
    .bind(now_ms)
    .bind(now_ms)
    .bind(verifier.run_id.as_str())
    .bind(verifier.step_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = row {
        append_workflow_run_event_in_tx(
            tx,
            verifier.run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "step_failed",
                actor_kind: "workflow_verifier",
                actor_id: Some(owner_id.to_string()),
                step_run_id: Some(row.try_get("step_run_id")?),
                verifier_run_id: Some(verifier.verifier_run_id.clone()),
                visibility: "internal",
                payload: json!({
                    "stepId": verifier.step_id,
                    "verifierId": verifier.verifier_id,
                    "reasonCode": VERIFIER_FAILED_REASON_CODE,
                }),
                now_ms,
            },
        )
        .await?;
    }

    let updated = sqlx::query(
        r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    completed_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(crate::WorkflowRunStatus::Failed.as_str())
    .bind(VERIFIER_FAILED_REASON)
    .bind(VERIFIER_FAILED_REASON_CODE)
    .bind(now_ms)
    .bind(now_ms)
    .bind(verifier.run_id.as_str())
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(());
    }
    append_workflow_run_event_in_tx(
        tx,
        verifier.run_id.as_str(),
        WorkflowRunEventAppend {
            event_type: "run_status_changed",
            actor_kind: "workflow_verifier",
            actor_id: Some(owner_id.to_string()),
            step_run_id: None,
            verifier_run_id: Some(verifier.verifier_run_id.clone()),
            visibility: "internal",
            payload: json!({
                "status": crate::WorkflowRunStatus::Failed.as_str(),
                "reasonCode": VERIFIER_FAILED_REASON_CODE,
            }),
            now_ms,
        },
    )
    .await?;
    Ok(())
}

fn sanitized_verifier_result_json(
    verifier: &crate::WorkflowRunStepVerifier,
    outcome: WorkflowRunVerifierOutcomeStatus,
    summary: &WorkflowRunVerifierResultSummary,
    reason_code: Option<&str>,
) -> anyhow::Result<String> {
    workflow_state_json_string(
        "workflow_run_step_verifier_result",
        json!({
            "verifierId": verifier.verifier_id,
            "verifierType": verifier.verifier_type,
            "status": match outcome {
                WorkflowRunVerifierOutcomeStatus::Passed => "passed",
                WorkflowRunVerifierOutcomeStatus::Failed => "failed",
            },
            "reasonCode": reason_code,
            "commandCount": summary.command_count,
            "expectedExitCode": summary.expected_exit_code,
            "observedExitCode": summary.observed_exit_code,
            "timedOut": summary.timed_out,
            "durationMs": summary.duration_ms,
            "outputBytes": summary.output_bytes,
            "outputTruncated": summary.output_truncated,
        }),
    )
}

fn validate_verifier_result_summary(
    params: &WorkflowRunVerifierRecordResultParams,
) -> anyhow::Result<()> {
    let summary = &params.summary;
    if summary.command_count < 0 {
        anyhow::bail!("workflow verifier command_count must be non-negative");
    }
    if summary.duration_ms < 0 {
        anyhow::bail!("workflow verifier duration_ms must be non-negative");
    }
    if summary.output_bytes < 0 {
        anyhow::bail!("workflow verifier output_bytes must be non-negative");
    }
    if params.outcome == WorkflowRunVerifierOutcomeStatus::Passed {
        if summary.timed_out {
            anyhow::bail!("workflow verifier passed outcome cannot have timed_out=true");
        }
        if summary.output_truncated {
            anyhow::bail!("workflow verifier passed outcome cannot have truncated output");
        }
        if summary.command_count > 0 && summary.observed_exit_code.is_none() {
            anyhow::bail!("workflow verifier passed outcome must include an observed exit code");
        }
        if let (Some(expected_exit_code), Some(observed_exit_code)) =
            (summary.expected_exit_code, summary.observed_exit_code)
            && expected_exit_code != observed_exit_code
        {
            anyhow::bail!("workflow verifier passed outcome cannot have mismatched exit codes");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000777").expect("valid thread id")
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

    fn verifier_workflow_yaml(workflow_id: &str, command: &str) -> String {
        format!(
            r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "{workflow_id}"
display_name: "Verifier State Test"
source_prompt: "deterministic verifier state test"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 1
  max_agents: 2
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 120
  max_tokens: 6000
  max_tool_calls: 50
approvals:
  required_before: []
agents:
  - id: "scope"
    display_name: "Adversary-Hypatia"
    role: "Attack verifier state."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "review"
    display_name: "Adversary-Euclid"
    role: "Attack verifier result leakage."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "scope"
    title: "Run deterministic verifier"
    agent: "scope"
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
        - id: "commands"
          type: "run_commands"
          cwd: "."
          sandbox: "default"
          network: "default"
          timeout_seconds: 30
          output_limit_bytes: 2048
          commands:
            - "{command}"
          expected_exit_code: 0
artifacts:
  retention: "until_workflow_complete"
  required:
    - "scope.md"
cleanup:
  on_cancel: []
  on_complete: []
"#
        )
    }

    async fn create_claimed_waiting_run(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        workflow_id: &str,
        command: &str,
    ) -> (crate::WorkflowRunSnapshot, i64) {
        create_claimed_waiting_run_from_yaml(
            runtime,
            thread_id,
            workflow_id,
            verifier_workflow_yaml(workflow_id, command),
        )
        .await
    }

    async fn create_claimed_waiting_run_from_yaml(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        workflow_id: &str,
        source_yaml: String,
    ) -> (crate::WorkflowRunSnapshot, i64) {
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml,
            })
            .await
            .expect("workflow spec should save");
        let snapshot = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: Some(format!("{workflow_id}-run")),
            })
            .await
            .expect("workflow run should create");
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("workflow run should claim")
            .expect("workflow run should be available");
        sqlx::query(
            r#"
UPDATE workflow_run_steps
SET status = 'waiting_verifier', reason_code = 'verifier_executor_pending'
WHERE run_id = ?
            "#,
        )
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("step should wait for verifier");
        sqlx::query(
            r#"
UPDATE workflow_run_step_verifiers
SET status = 'blocked',
    status_reason = 'deterministic verifier executor is not enabled',
    reason_code = 'verifier_executor_pending'
WHERE run_id = ?
            "#,
        )
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("verifier should be blocked pending executor");
        let snapshot = runtime
            .workflows()
            .get_workflow_run_snapshot(snapshot.run.run_id.as_str())
            .await
            .expect("workflow run should reload")
            .expect("workflow run should exist");
        (snapshot, claim.generation)
    }

    fn verifier_workflow_yaml_with_triggered_loop(workflow_id: &str, command: &str) -> String {
        verifier_workflow_yaml(workflow_id, command).replace(
            "artifacts:",
            r#"loops:
  - id: "after_scope_loop"
    title: "Recheck after scope passes"
    schedule:
      type: interval
      amount: 5
      unit: minutes
    timezone: "UTC"
    stop_condition:
      type: workflow_complete
    max_iterations: 2
    trigger_step: "scope"
artifacts:"#,
        )
    }

    fn passing_summary() -> WorkflowRunVerifierResultSummary {
        WorkflowRunVerifierResultSummary {
            command_count: 1,
            expected_exit_code: Some(0),
            observed_exit_code: Some(0),
            timed_out: false,
            duration_ms: 7,
            output_bytes: 32,
            output_truncated: false,
        }
    }

    fn persisted_event_payloads(snapshot: &crate::WorkflowRunSnapshot) -> String {
        snapshot
            .events
            .iter()
            .map(|event| event.event_payload_json.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn verifier_pass_promotes_step_and_completes_run_without_leaking_output() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let (run, generation) = create_claimed_waiting_run(
            &runtime,
            thread_id,
            "wf_verifier_pass",
            "true RAW_COMMAND_SECRET",
        )
        .await;

        let claim = runtime
            .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
            })
            .await
            .expect("verifier claim should succeed")
            .expect("verifier should claim");
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Running,
            claim.verifier.status
        );
        assert_eq!(1, claim.verifier.attempt_count);
        assert!(
            claim.run.lease_expires_at > run.run.lease_expires_at,
            "verifier claim should extend the workflow lease for the bounded command window"
        );

        let recorded = runtime
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run.run.run_id,
                owner_id: "verifier-owner".to_string(),
                generation,
                verifier_run_id: claim.verifier.verifier_run_id,
                outcome: WorkflowRunVerifierOutcomeStatus::Passed,
                summary: passing_summary(),
            })
            .await
            .expect("verifier result should record")
            .expect("verifier result should update");

        assert_eq!(
            crate::WorkflowRunStatus::Completed,
            recorded.snapshot.run.status
        );
        assert_eq!(
            crate::WorkflowRunStepStatus::Succeeded,
            recorded.snapshot.steps[0].status
        );
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Passed,
            recorded.snapshot.verifiers[0].status
        );
        let result_json = recorded.snapshot.verifiers[0]
            .last_result_json
            .as_ref()
            .expect("verifier result should persist")
            .to_string();
        let event_payloads = persisted_event_payloads(&recorded.snapshot);
        assert!(!result_json.contains("RAW_COMMAND_SECRET"));
        assert!(!result_json.contains("true "));
        assert!(!event_payloads.contains("RAW_COMMAND_SECRET"));
        assert!(!event_payloads.contains("true "));
    }

    #[tokio::test]
    async fn verifier_pass_arms_triggered_timers_for_succeeded_step() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let (run, generation) = create_claimed_waiting_run_from_yaml(
            &runtime,
            thread_id,
            "wf_verifier_arms_timer",
            verifier_workflow_yaml_with_triggered_loop("wf_verifier_arms_timer", "true"),
        )
        .await;
        let timer_before: Option<i64> = sqlx::query_scalar(
            "SELECT next_fire_at_ms FROM workflow_run_timers WHERE run_id = ? AND workflow_loop_id = 'after_scope_loop'",
        )
        .bind(run.run.run_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("timer should query before verifier pass");
        assert_eq!(None, timer_before);

        let claim = runtime
            .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
            })
            .await
            .expect("verifier claim should succeed")
            .expect("verifier should claim");
        let recorded = runtime
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                verifier_run_id: claim.verifier.verifier_run_id,
                outcome: WorkflowRunVerifierOutcomeStatus::Passed,
                summary: passing_summary(),
            })
            .await
            .expect("verifier result should record")
            .expect("verifier result should update");

        let timer_after: Option<i64> = sqlx::query_scalar(
            "SELECT next_fire_at_ms FROM workflow_run_timers WHERE run_id = ? AND workflow_loop_id = 'after_scope_loop'",
        )
        .bind(run.run.run_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("timer should query after verifier pass");
        assert!(timer_after.is_some());
        let event_types = recorded
            .snapshot
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>();
        assert!(event_types.contains(&"step_succeeded"));
        assert!(event_types.contains(&"timer_armed"));
    }

    #[tokio::test]
    async fn verifier_failure_fails_step_and_run_without_leaking_output() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let (run, generation) = create_claimed_waiting_run(
            &runtime,
            thread_id,
            "wf_verifier_fail",
            "false SECRET_OUTPUT",
        )
        .await;

        let claim = runtime
            .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
            })
            .await
            .expect("verifier claim should succeed")
            .expect("verifier should claim");
        let summary = WorkflowRunVerifierResultSummary {
            observed_exit_code: Some(1),
            output_bytes: 128,
            ..passing_summary()
        };
        let recorded = runtime
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run.run.run_id,
                owner_id: "verifier-owner".to_string(),
                generation,
                verifier_run_id: claim.verifier.verifier_run_id,
                outcome: WorkflowRunVerifierOutcomeStatus::Failed,
                summary,
            })
            .await
            .expect("verifier result should record")
            .expect("verifier result should update");

        assert_eq!(
            crate::WorkflowRunStatus::Failed,
            recorded.snapshot.run.status
        );
        assert_eq!(
            crate::WorkflowRunStepStatus::Failed,
            recorded.snapshot.steps[0].status
        );
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Failed,
            recorded.snapshot.verifiers[0].status
        );
        let result_json = recorded.snapshot.verifiers[0]
            .last_result_json
            .as_ref()
            .expect("verifier result should persist")
            .to_string();
        let event_payloads = persisted_event_payloads(&recorded.snapshot);
        assert!(!result_json.contains("SECRET_OUTPUT"));
        assert!(!result_json.contains("false "));
        assert!(!event_payloads.contains("SECRET_OUTPUT"));
        assert!(!event_payloads.contains("false "));
    }

    #[tokio::test]
    async fn passed_verifier_result_rejects_failed_summary() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let (run, generation) =
            create_claimed_waiting_run(&runtime, thread_id, "wf_verifier_bad_pass", "true").await;
        let claim = runtime
            .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
            })
            .await
            .expect("verifier claim should succeed")
            .expect("verifier should claim");
        let result = runtime
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                verifier_run_id: claim.verifier.verifier_run_id,
                outcome: WorkflowRunVerifierOutcomeStatus::Passed,
                summary: WorkflowRunVerifierResultSummary {
                    observed_exit_code: Some(1),
                    ..passing_summary()
                },
            })
            .await;

        let err = result.expect_err("inconsistent passed verifier summary should be rejected");
        assert!(
            err.to_string().contains("mismatched exit codes"),
            "unexpected error: {err}"
        );
        let snapshot = runtime
            .workflows()
            .get_workflow_run_snapshot(run.run.run_id.as_str())
            .await
            .expect("workflow run should reload")
            .expect("workflow run should exist");
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Running,
            snapshot.verifiers[0].status
        );
    }

    #[tokio::test]
    async fn stale_generation_cannot_record_verifier_result() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let (run, generation) =
            create_claimed_waiting_run(&runtime, thread_id, "wf_verifier_stale", "true").await;
        let claim = runtime
            .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
            })
            .await
            .expect("verifier claim should succeed")
            .expect("verifier should claim");
        sqlx::query("UPDATE workflow_runs SET generation = generation + 1 WHERE run_id = ?")
            .bind(run.run.run_id.as_str())
            .execute(runtime.pool.as_ref())
            .await
            .expect("generation should advance");

        let stale_record = runtime
            .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation,
                verifier_run_id: claim.verifier.verifier_run_id,
                outcome: WorkflowRunVerifierOutcomeStatus::Passed,
                summary: passing_summary(),
            })
            .await
            .expect("stale record should not error");
        assert_eq!(None, stale_record);

        let snapshot = runtime
            .workflows()
            .get_workflow_run_snapshot(run.run.run_id.as_str())
            .await
            .expect("workflow run should reload")
            .expect("workflow run should exist");
        assert_eq!(
            crate::WorkflowRunStepVerifierStatus::Running,
            snapshot.verifiers[0].status
        );
    }
}
