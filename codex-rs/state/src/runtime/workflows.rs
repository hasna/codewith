use super::*;
use crate::model::WorkflowRunEventRow;
use crate::model::WorkflowRunRow;
use crate::model::WorkflowRunStepRow;
use crate::model::WorkflowRunStepVerifierRow;
use crate::model::WorkflowSpecRow;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeSet;
use std::collections::HashMap;
use uuid::Uuid;

pub const DEFAULT_THREAD_WORKFLOW_LIST_LIMIT: u32 = 20;
pub const MAX_THREAD_WORKFLOW_LIST_LIMIT: u32 = 50;
pub const DEFAULT_THREAD_WORKFLOW_RUN_LIST_LIMIT: u32 = 20;
pub const MAX_THREAD_WORKFLOW_RUN_LIST_LIMIT: u32 = 50;

/// A gated step is awaiting an explicit user approval decision.
pub const WORKFLOW_STEP_APPROVAL_PENDING: &str = "pending";
/// A gated step has been approved by the user and may be admitted for execution.
pub const WORKFLOW_STEP_APPROVAL_APPROVED: &str = "approved";
/// A gated step has been rejected by the user and will be skipped.
pub const WORKFLOW_STEP_APPROVAL_REJECTED: &str = "rejected";

#[derive(Clone)]
pub struct WorkflowStore {
    pool: Arc<SqlitePool>,
}

impl WorkflowStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSpecCreateParams {
    pub source_thread_id: Option<ThreadId>,
    pub source_yaml: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSpecListPage {
    pub data: Vec<crate::WorkflowSpecRecord>,
    pub next_cursor: Option<String>,
}

/// Result of attempting to delete a saved workflow spec for a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSpecDeleteOutcome {
    /// The spec (and any terminal runs cascading from it) was removed.
    Deleted,
    /// No spec matched the thread + workflow record id.
    NotFound,
    /// The spec still has a non-terminal run and was left untouched.
    BlockedByActiveRun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunListPage {
    pub data: Vec<crate::WorkflowRunSnapshot>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunStatusMutationOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunCreateParams {
    pub workflow_record_id: String,
    pub source_thread_id: Option<ThreadId>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunCancelParams {
    pub run_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunPauseParams {
    pub run_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunResumeParams {
    pub run_id: String,
}

/// Explicit user decision on a workflow run step guarded by an approval gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowRunStepApprovalDecision {
    Approve,
    Reject,
}

impl WorkflowRunStepApprovalDecision {
    fn approval_state(self) -> &'static str {
        match self {
            Self::Approve => WORKFLOW_STEP_APPROVAL_APPROVED,
            Self::Reject => WORKFLOW_STEP_APPROVAL_REJECTED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunStepApprovalParams {
    pub run_id: String,
    pub step_id: String,
    pub decision: WorkflowRunStepApprovalDecision,
    /// Optional user-supplied justification. The raw value is never persisted; only
    /// its presence is recorded so approval provenance stays free of injected data.
    pub reason: Option<String>,
    /// Optional actor id (for example the requesting agent or user handle).
    pub actor_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunStepApprovalOutcome {
    pub snapshot: crate::WorkflowRunSnapshot,
    /// True when the decision changed persisted state (idempotent re-decisions are false).
    pub changed: bool,
    /// True when the targeted step actually declares an approval gate.
    pub gate_present: bool,
    pub decision: WorkflowRunStepApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowSpecMetadata {
    schema_version: String,
    spec_workflow_id: String,
    display_name: String,
    status: crate::WorkflowSpecStatus,
    source_yaml_sha256: String,
    agent_count: i64,
    step_count: i64,
    parallel_group_count: i64,
    verifier_count: i64,
    run_command_verifier_count: i64,
    model_routed_step_count: i64,
}

impl WorkflowStore {
    pub async fn save_workflow_spec_yaml(
        &self,
        params: WorkflowSpecCreateParams,
    ) -> anyhow::Result<crate::WorkflowSpecRecord> {
        let WorkflowSpecCreateParams {
            source_thread_id,
            source_yaml,
        } = params;
        let spec = codex_workflows::parse_workflow_yaml(&source_yaml)?;
        let source_yaml_sha256 = workflow_source_sha256(&source_yaml);
        let metadata = metadata_from_spec(&spec, source_yaml_sha256)?;
        let workflow_record_id = Uuid::new_v4().to_string();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let row = sqlx::query(
            r#"
INSERT INTO workflow_specs (
    workflow_record_id,
    spec_workflow_id,
    source_thread_id,
    schema_version,
    display_name,
    status,
    source_yaml,
    source_yaml_sha256,
    agent_count,
    step_count,
    parallel_group_count,
    verifier_count,
    run_command_verifier_count,
    model_routed_step_count,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(source_thread_id, spec_workflow_id) DO UPDATE SET
    source_thread_id = excluded.source_thread_id,
    schema_version = excluded.schema_version,
    display_name = excluded.display_name,
    status = excluded.status,
    source_yaml = excluded.source_yaml,
    source_yaml_sha256 = excluded.source_yaml_sha256,
    agent_count = excluded.agent_count,
    step_count = excluded.step_count,
    parallel_group_count = excluded.parallel_group_count,
    verifier_count = excluded.verifier_count,
    run_command_verifier_count = excluded.run_command_verifier_count,
    model_routed_step_count = excluded.model_routed_step_count,
    updated_at_ms = excluded.updated_at_ms
RETURNING
    workflow_record_id,
    spec_workflow_id,
    source_thread_id,
    schema_version,
    display_name,
    status,
    source_yaml,
    source_yaml_sha256,
    agent_count,
    step_count,
    parallel_group_count,
    verifier_count,
    run_command_verifier_count,
    model_routed_step_count,
    created_at_ms,
    updated_at_ms
            "#,
        )
        .bind(workflow_record_id)
        .bind(metadata.spec_workflow_id)
        .bind(source_thread_id.map(|thread_id| thread_id.to_string()))
        .bind(metadata.schema_version)
        .bind(metadata.display_name)
        .bind(metadata.status.as_str())
        .bind(source_yaml)
        .bind(metadata.source_yaml_sha256)
        .bind(metadata.agent_count)
        .bind(metadata.step_count)
        .bind(metadata.parallel_group_count)
        .bind(metadata.verifier_count)
        .bind(metadata.run_command_verifier_count)
        .bind(metadata.model_routed_step_count)
        .bind(now_ms)
        .bind(now_ms)
        .fetch_one(self.pool.as_ref())
        .await?;

        workflow_spec_from_row(&row)
    }

    pub async fn get_workflow_spec(
        &self,
        workflow_record_id: &str,
    ) -> anyhow::Result<Option<crate::WorkflowSpecRecord>> {
        let sql = workflow_spec_select_by(
            r#"
SELECT
"#,
            "workflow_record_id = ?",
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(workflow_record_id)
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| workflow_spec_from_row(&row)).transpose()
    }

    pub async fn get_workflow_spec_by_spec_workflow_id(
        &self,
        spec_workflow_id: &str,
    ) -> anyhow::Result<Option<crate::WorkflowSpecRecord>> {
        let sql = workflow_spec_select_by(
            r#"
SELECT
"#,
            "spec_workflow_id = ?",
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(spec_workflow_id)
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| workflow_spec_from_row(&row)).transpose()
    }

    pub async fn get_thread_workflow_spec(
        &self,
        thread_id: ThreadId,
        workflow_record_id: &str,
    ) -> anyhow::Result<Option<crate::WorkflowSpecRecord>> {
        let sql = workflow_spec_select_by(
            r#"
SELECT
"#,
            "source_thread_id = ? AND workflow_record_id = ?",
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(thread_id.to_string())
            .bind(workflow_record_id)
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| workflow_spec_from_row(&row)).transpose()
    }

    /// Delete a saved workflow spec scoped to `thread_id`.
    ///
    /// Deletion is refused when the spec still owns a non-terminal run so an
    /// in-flight execution is never yanked out from under the runtime. Terminal
    /// runs (and their steps/verifiers/events) are cleaned up via the schema's
    /// `ON DELETE CASCADE`, which requires foreign-key enforcement to be enabled
    /// on the connection performing the delete (it is off by default in SQLite).
    pub async fn delete_thread_workflow_spec(
        &self,
        thread_id: ThreadId,
        workflow_record_id: &str,
    ) -> anyhow::Result<WorkflowSpecDeleteOutcome> {
        let thread_id = thread_id.to_string();
        let mut conn = self.pool.acquire().await?;
        // Foreign-key enforcement is per-connection and cannot be toggled inside
        // a transaction, so enable it before opening one.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await?;

        let result = delete_thread_workflow_spec_with_cascade(
            &mut conn,
            thread_id.as_str(),
            workflow_record_id,
        )
        .await;

        // Restore the connection default before it returns to the pool so this
        // one-off enablement never leaks foreign-key enforcement onto unrelated
        // callers that reuse the same pooled connection.
        let _ = sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await;

        result
    }

    pub async fn list_thread_workflow_specs_page(
        &self,
        thread_id: ThreadId,
        cursor: Option<u32>,
        limit: u32,
    ) -> anyhow::Result<WorkflowSpecListPage> {
        let offset = cursor.unwrap_or(0);
        let limit = limit.clamp(1, MAX_THREAD_WORKFLOW_LIST_LIMIT);
        let rows = sqlx::query(
            r#"
SELECT
    workflow_record_id,
    spec_workflow_id,
    source_thread_id,
    schema_version,
    display_name,
    status,
    source_yaml,
    source_yaml_sha256,
    agent_count,
    step_count,
    parallel_group_count,
    verifier_count,
    run_command_verifier_count,
    model_routed_step_count,
    created_at_ms,
    updated_at_ms
FROM workflow_specs
WHERE source_thread_id = ?
ORDER BY updated_at_ms DESC, workflow_record_id
LIMIT ? OFFSET ?
            "#,
        )
        .bind(thread_id.to_string())
        .bind(i64::from(limit) + 1)
        .bind(i64::from(offset))
        .fetch_all(self.pool.as_ref())
        .await?;
        let has_more = rows.len() > limit as usize;
        let data = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| workflow_spec_from_row(&row))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let next_cursor = has_more.then(|| offset.saturating_add(limit).to_string());
        Ok(WorkflowSpecListPage { data, next_cursor })
    }

    pub async fn create_workflow_run(
        &self,
        params: WorkflowRunCreateParams,
    ) -> anyhow::Result<crate::WorkflowRunSnapshot> {
        let spec_record = self
            .get_workflow_spec(params.workflow_record_id.as_str())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("workflow spec {} does not exist", params.workflow_record_id)
            })?;
        let source_thread_id = resolve_run_source_thread_id(&params, &spec_record)?;
        let spec = codex_workflows::parse_workflow_yaml(&spec_record.source_yaml)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;

        if let Some(idempotency_key) = params.idempotency_key.as_deref()
            && let Some(snapshot) = workflow_run_snapshot_by_idempotency_in_tx(
                &mut tx,
                spec_record.workflow_record_id.as_str(),
                idempotency_key,
            )
            .await?
        {
            tx.commit().await?;
            return Ok(snapshot);
        }

        let run_id = Uuid::new_v4().to_string();
        let inserted = insert_workflow_run_in_tx(
            &mut tx,
            InsertWorkflowRunParams {
                run_id: run_id.clone(),
                spec_record: &spec_record,
                source_thread_id,
                idempotency_key: params.idempotency_key.as_deref(),
                spec: &spec,
                now_ms,
            },
        )
        .await?;

        if !inserted {
            let idempotency_key = params
                .idempotency_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("workflow run insert was skipped unexpectedly"))?;
            let snapshot = workflow_run_snapshot_by_idempotency_in_tx(
                &mut tx,
                spec_record.workflow_record_id.as_str(),
                idempotency_key,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("idempotent workflow run was not found"))?;
            tx.commit().await?;
            return Ok(snapshot);
        }

        insert_workflow_run_steps_in_tx(&mut tx, run_id.as_str(), &spec, now_ms).await?;
        insert_workflow_run_automation_in_tx(&mut tx, run_id.as_str(), &spec, now_ms).await?;
        append_workflow_run_event_in_tx(
            &mut tx,
            run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "created",
                actor_kind: "system",
                actor_id: None,
                step_run_id: None,
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "workflowRecordId": spec_record.workflow_record_id,
                    "specWorkflowId": spec_record.spec_workflow_id,
                    "sourceYamlSha256": spec_record.source_yaml_sha256,
                    "stepCount": spec.steps.len(),
                    "agentCount": spec.agents.len(),
                }),
                now_ms,
            },
        )
        .await?;

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, run_id.as_str()).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    pub async fn get_workflow_run_snapshot(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<crate::WorkflowRunSnapshot>> {
        let mut tx = self.pool.begin().await?;
        let snapshot = maybe_snapshot_workflow_run_in_tx(&mut tx, run_id).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    pub async fn list_thread_workflow_runs_page(
        &self,
        thread_id: ThreadId,
        cursor: Option<u32>,
        limit: u32,
    ) -> anyhow::Result<WorkflowRunListPage> {
        let offset = cursor.unwrap_or(0);
        let limit = limit.clamp(1, MAX_THREAD_WORKFLOW_RUN_LIST_LIMIT);
        let rows = sqlx::query(
            r#"
SELECT run_id
FROM workflow_runs
WHERE source_thread_id = ?
ORDER BY updated_at_ms DESC, run_id
LIMIT ? OFFSET ?
            "#,
        )
        .bind(thread_id.to_string())
        .bind(i64::from(limit) + 1)
        .bind(i64::from(offset))
        .fetch_all(self.pool.as_ref())
        .await?;
        let has_more = rows.len() > limit as usize;
        let run_ids = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| row.try_get("run_id").map_err(anyhow::Error::from))
            .collect::<anyhow::Result<Vec<String>>>()?;
        let mut tx = self.pool.begin().await?;
        let mut data = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            data.push(snapshot_workflow_run_in_tx(&mut tx, run_id.as_str()).await?);
        }
        tx.commit().await?;
        let next_cursor = has_more.then(|| offset.saturating_add(limit).to_string());
        Ok(WorkflowRunListPage { data, next_cursor })
    }

    pub async fn request_workflow_run_cancel(
        &self,
        params: WorkflowRunCancelParams,
    ) -> anyhow::Result<Option<WorkflowRunStatusMutationOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some(run) = get_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let mut changed = false;

        if !run.status.is_terminal() && run.status != crate::WorkflowRunStatus::CancelRequested {
            changed = true;
            let status_reason = sanitized_workflow_cancel_reason();
            sqlx::query(
                r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    owner_id = NULL,
    lease_expires_at_ms = NULL,
    heartbeat_at_ms = NULL,
    generation = generation + 1,
    updated_at_ms = ?
WHERE run_id = ?
                "#,
            )
            .bind(crate::WorkflowRunStatus::CancelRequested.as_str())
            .bind(redact_state_string(status_reason))
            .bind("user_cancel_requested")
            .bind(now_ms)
            .bind(params.run_id.as_str())
            .execute(&mut *tx)
            .await?;
            append_workflow_run_event_in_tx(
                &mut tx,
                params.run_id.as_str(),
                WorkflowRunEventAppend {
                    event_type: "cancel_requested",
                    actor_kind: "system",
                    actor_id: None,
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "internal",
                    payload: json!({ "reasonCode": "user_cancel_requested" }),
                    now_ms,
                },
            )
            .await?;
        }

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunStatusMutationOutcome { snapshot, changed }))
    }

    pub async fn pause_workflow_run(
        &self,
        params: WorkflowRunPauseParams,
    ) -> anyhow::Result<Option<WorkflowRunStatusMutationOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some(run) = get_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let mut changed = false;

        if !run.status.is_terminal()
            && run.status != crate::WorkflowRunStatus::CancelRequested
            && run.status != crate::WorkflowRunStatus::Paused
        {
            changed = true;
            sqlx::query(
                r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = ?,
    reason_code = ?,
    owner_id = NULL,
    lease_expires_at_ms = NULL,
    heartbeat_at_ms = NULL,
    generation = generation + 1,
    updated_at_ms = ?
WHERE run_id = ?
                "#,
            )
            .bind(crate::WorkflowRunStatus::Paused.as_str())
            .bind(redact_state_string(sanitized_workflow_pause_reason()))
            .bind("user_pause_requested")
            .bind(now_ms)
            .bind(params.run_id.as_str())
            .execute(&mut *tx)
            .await?;
            append_workflow_run_event_in_tx(
                &mut tx,
                params.run_id.as_str(),
                WorkflowRunEventAppend {
                    event_type: "paused",
                    actor_kind: "system",
                    actor_id: None,
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "internal",
                    payload: json!({ "reasonCode": "user_pause_requested" }),
                    now_ms,
                },
            )
            .await?;
        }

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunStatusMutationOutcome { snapshot, changed }))
    }

    pub async fn resume_workflow_run(
        &self,
        params: WorkflowRunResumeParams,
    ) -> anyhow::Result<Option<WorkflowRunStatusMutationOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some(run) = get_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let mut changed = false;

        if run.status == crate::WorkflowRunStatus::Paused {
            changed = true;
            sqlx::query(
                r#"
UPDATE workflow_runs
SET
    status = ?,
    status_reason = NULL,
    reason_code = NULL,
    owner_id = NULL,
    lease_expires_at_ms = NULL,
    heartbeat_at_ms = NULL,
    generation = generation + 1,
    updated_at_ms = ?
WHERE run_id = ?
                "#,
            )
            .bind(crate::WorkflowRunStatus::Waiting.as_str())
            .bind(now_ms)
            .bind(params.run_id.as_str())
            .execute(&mut *tx)
            .await?;
            append_workflow_run_event_in_tx(
                &mut tx,
                params.run_id.as_str(),
                WorkflowRunEventAppend {
                    event_type: "resumed",
                    actor_kind: "system",
                    actor_id: None,
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "internal",
                    payload: json!({ "reasonCode": "user_resume_requested" }),
                    now_ms,
                },
            )
            .await?;
        }

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunStatusMutationOutcome { snapshot, changed }))
    }

    /// Record an explicit user approval decision for a gated workflow run step.
    ///
    /// Approving a gated step clears it for automatic branch admission on the next
    /// orchestrator tick; rejecting it marks the step `skipped` so downstream
    /// dependents stall instead of running without consent. Steps that never
    /// declared an approval gate, or that already left the pre-execution
    /// (`pending`/`ready`) states, are treated as no-ops. The raw approval reason
    /// is never persisted so approval provenance cannot smuggle injected content.
    pub async fn set_workflow_run_step_approval(
        &self,
        params: WorkflowRunStepApprovalParams,
    ) -> anyhow::Result<Option<WorkflowRunStepApprovalOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
SELECT step_run_id, status, approval_gate, approval_state
FROM workflow_run_steps
WHERE run_id = ? AND step_id = ?
            "#,
        )
        .bind(params.run_id.as_str())
        .bind(params.step_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let step_run_id: String = row.try_get("step_run_id")?;
        let status: String = row.try_get("status")?;
        let approval_gate: Option<String> = row.try_get("approval_gate")?;
        let approval_state: Option<String> = row.try_get("approval_state")?;
        let gate_present = approval_gate.is_some();
        let status = crate::WorkflowRunStepStatus::try_from(status.as_str())?;
        let admittable = matches!(
            status,
            crate::WorkflowRunStepStatus::Pending | crate::WorkflowRunStepStatus::Ready
        );
        let target_state = params.decision.approval_state();
        let mut changed = false;

        if gate_present && admittable && approval_state.as_deref() != Some(target_state) {
            changed = true;
            match params.decision {
                WorkflowRunStepApprovalDecision::Approve => {
                    sqlx::query(
                        r#"
UPDATE workflow_run_steps
SET approval_state = ?, updated_at_ms = ?
WHERE run_id = ? AND step_id = ?
                        "#,
                    )
                    .bind(WORKFLOW_STEP_APPROVAL_APPROVED)
                    .bind(now_ms)
                    .bind(params.run_id.as_str())
                    .bind(params.step_id.as_str())
                    .execute(&mut *tx)
                    .await?;
                }
                WorkflowRunStepApprovalDecision::Reject => {
                    sqlx::query(
                        r#"
UPDATE workflow_run_steps
SET
    approval_state = ?,
    status = ?,
    status_reason = ?,
    reason_code = ?,
    updated_at_ms = ?
WHERE run_id = ? AND step_id = ?
                        "#,
                    )
                    .bind(WORKFLOW_STEP_APPROVAL_REJECTED)
                    .bind(crate::WorkflowRunStepStatus::Skipped.as_str())
                    .bind(sanitized_workflow_step_rejection_reason())
                    .bind("user_rejected_approval")
                    .bind(now_ms)
                    .bind(params.run_id.as_str())
                    .bind(params.step_id.as_str())
                    .execute(&mut *tx)
                    .await?;
                }
            }
            let event_type = match params.decision {
                WorkflowRunStepApprovalDecision::Approve => "step_approval_granted",
                WorkflowRunStepApprovalDecision::Reject => "step_approval_rejected",
            };
            append_workflow_run_event_in_tx(
                &mut tx,
                params.run_id.as_str(),
                WorkflowRunEventAppend {
                    event_type,
                    actor_kind: "user",
                    actor_id: params.actor_id.clone(),
                    step_run_id: Some(step_run_id),
                    verifier_run_id: None,
                    visibility: "internal",
                    payload: json!({
                        "stepId": params.step_id,
                        "decision": target_state,
                        "reasonProvided": params.reason.is_some(),
                    }),
                    now_ms,
                },
            )
            .await?;
        }

        let snapshot = snapshot_workflow_run_in_tx(&mut tx, params.run_id.as_str()).await?;
        tx.commit().await?;
        Ok(Some(WorkflowRunStepApprovalOutcome {
            snapshot,
            changed,
            gate_present,
            decision: params.decision,
        }))
    }
}

struct InsertWorkflowRunParams<'a> {
    run_id: String,
    spec_record: &'a crate::WorkflowSpecRecord,
    source_thread_id: Option<ThreadId>,
    idempotency_key: Option<&'a str>,
    spec: &'a codex_workflows::WorkflowSpec,
    now_ms: i64,
}

pub(super) struct WorkflowRunEventAppend {
    pub(super) event_type: &'static str,
    pub(super) actor_kind: &'static str,
    pub(super) actor_id: Option<String>,
    pub(super) step_run_id: Option<String>,
    pub(super) verifier_run_id: Option<String>,
    pub(super) visibility: &'static str,
    pub(super) payload: Value,
    pub(super) now_ms: i64,
}

/// Runs the guarded spec delete inside a transaction on `conn`, which must
/// already have foreign-key enforcement enabled so `ON DELETE CASCADE` reaches
/// the spec's terminal runs.
async fn delete_thread_workflow_spec_with_cascade(
    conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    thread_id: &str,
    workflow_record_id: &str,
) -> anyhow::Result<WorkflowSpecDeleteOutcome> {
    let mut tx = sqlx::Connection::begin(&mut **conn).await?;

    let spec_exists = sqlx::query_scalar::<_, i64>(
        r#"
SELECT COUNT(*)
FROM workflow_specs
WHERE source_thread_id = ? AND workflow_record_id = ?
        "#,
    )
    .bind(thread_id)
    .bind(workflow_record_id)
    .fetch_one(&mut *tx)
    .await?;
    if spec_exists == 0 {
        tx.rollback().await?;
        return Ok(WorkflowSpecDeleteOutcome::NotFound);
    }

    let active_run_count = sqlx::query_scalar::<_, i64>(
        r#"
SELECT COUNT(*)
FROM workflow_runs
WHERE workflow_record_id = ?
    AND status NOT IN ('completed', 'complete', 'failed', 'cancelled')
        "#,
    )
    .bind(workflow_record_id)
    .fetch_one(&mut *tx)
    .await?;
    if active_run_count > 0 {
        tx.rollback().await?;
        return Ok(WorkflowSpecDeleteOutcome::BlockedByActiveRun);
    }

    let result = sqlx::query(
        r#"
DELETE FROM workflow_specs
WHERE source_thread_id = ? AND workflow_record_id = ?
        "#,
    )
    .bind(thread_id)
    .bind(workflow_record_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(if result.rows_affected() > 0 {
        WorkflowSpecDeleteOutcome::Deleted
    } else {
        WorkflowSpecDeleteOutcome::NotFound
    })
}

fn resolve_run_source_thread_id(
    params: &WorkflowRunCreateParams,
    spec_record: &crate::WorkflowSpecRecord,
) -> anyhow::Result<Option<ThreadId>> {
    if let (Some(requested), Some(spec_thread_id)) =
        (params.source_thread_id, spec_record.source_thread_id)
        && requested != spec_thread_id
    {
        anyhow::bail!(
            "workflow spec {} belongs to a different source thread",
            spec_record.workflow_record_id
        );
    }
    Ok(params.source_thread_id.or(spec_record.source_thread_id))
}

fn sanitized_workflow_pause_reason() -> &'static str {
    "workflow run paused"
}

fn sanitized_workflow_step_rejection_reason() -> &'static str {
    "workflow step rejected during user approval"
}

async fn insert_workflow_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: InsertWorkflowRunParams<'_>,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"
INSERT INTO workflow_runs (
    run_id,
    workflow_record_id,
    source_thread_id,
    idempotency_key,
    spec_workflow_id,
    schema_version,
    source_yaml_sha256,
    status,
    generation,
    last_event_seq,
    agents_json,
    execution_defaults_json,
    limits_json,
    approvals_json,
    loops_json,
    monitor_links_json,
    artifacts_json,
    cleanup_json,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(workflow_record_id, idempotency_key) DO NOTHING
RETURNING run_id
        "#,
    )
    .bind(params.run_id.as_str())
    .bind(params.spec_record.workflow_record_id.as_str())
    .bind(
        params
            .source_thread_id
            .map(|thread_id| thread_id.to_string()),
    )
    .bind(params.idempotency_key.map(redact_state_string))
    .bind(params.spec_record.spec_workflow_id.as_str())
    .bind(params.spec_record.schema_version.as_str())
    .bind(params.spec_record.source_yaml_sha256.as_str())
    .bind(crate::WorkflowRunStatus::Pending.as_str())
    .bind(0_i64)
    .bind(0_i64)
    .bind(workflow_state_json_string(
        "workflow_run_agents",
        json!(params.spec.agents),
    )?)
    .bind(workflow_state_json_string(
        "workflow_run_execution_defaults",
        json!(params.spec.execution_defaults),
    )?)
    .bind(workflow_state_json_string(
        "workflow_run_limits",
        json!(params.spec.limits),
    )?)
    .bind(workflow_state_json_string(
        "workflow_run_approvals",
        json!(params.spec.approvals),
    )?)
    .bind(optional_workflow_state_json_string(
        "workflow_run_loops",
        non_empty_slice(&params.spec.loops),
    )?)
    .bind(optional_workflow_state_json_string(
        "workflow_run_monitor_links",
        non_empty_slice(&params.spec.monitors),
    )?)
    .bind(workflow_state_json_string(
        "workflow_run_artifacts",
        json!(params.spec.artifacts),
    )?)
    .bind(workflow_state_json_string(
        "workflow_run_cleanup",
        json!(params.spec.cleanup),
    )?)
    .bind(params.now_ms)
    .bind(params.now_ms)
    .fetch_optional(&mut **tx)
    .await?;

    Ok(row.is_some())
}

fn non_empty_slice<T>(values: &[T]) -> Option<&[T]> {
    (!values.is_empty()).then_some(values)
}

async fn insert_workflow_run_steps_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    spec: &codex_workflows::WorkflowSpec,
    now_ms: i64,
) -> anyhow::Result<()> {
    for (sequence, step) in spec.steps.iter().enumerate() {
        sqlx::query(
            r#"
INSERT INTO workflow_run_steps (
    step_run_id,
    run_id,
    step_id,
    sequence,
    title,
    agent_id,
    status,
    parallel_group,
    approval_gate,
    approval_state,
    model_route_json,
    workspace_json,
    completion_model_marked_state,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(run_id)
        .bind(step.id.as_str())
        .bind(i64::try_from(sequence)?)
        .bind(redact_state_string(step.title.as_str()))
        .bind(step.agent.as_str())
        .bind(crate::WorkflowRunStepStatus::Pending.as_str())
        .bind(step.parallel_group.as_deref())
        .bind(step.approval_gate.as_deref())
        .bind(
            step.approval_gate
                .as_ref()
                .map(|_| WORKFLOW_STEP_APPROVAL_PENDING),
        )
        .bind(optional_workflow_state_json_string(
            "workflow_run_step_model_route",
            step.model.as_ref(),
        )?)
        .bind(optional_workflow_state_json_string(
            "workflow_run_step_workspace",
            step.workspace.as_ref(),
        )?)
        .bind(
            step.completion
                .as_ref()
                .map(|completion| completion.model_marked_state.as_str()),
        )
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut **tx)
        .await?;
    }

    for step in &spec.steps {
        for dependency in &step.depends_on {
            sqlx::query(
                r#"
INSERT INTO workflow_run_step_dependencies (run_id, step_id, depends_on_step_id)
VALUES (?, ?, ?)
                "#,
            )
            .bind(run_id)
            .bind(step.id.as_str())
            .bind(dependency.as_str())
            .execute(&mut **tx)
            .await?;
        }
        if let Some(completion) = &step.completion {
            for verifier in &completion.verifiers {
                sqlx::query(
                    r#"
INSERT INTO workflow_run_step_verifiers (
    verifier_run_id,
    run_id,
    step_id,
    verifier_id,
    verifier_type,
    status,
    definition_json,
    max_attempts,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(Uuid::new_v4().to_string())
                .bind(run_id)
                .bind(step.id.as_str())
                .bind(verifier.id.as_str())
                .bind(verifier.kind.as_str())
                .bind(crate::WorkflowRunStepVerifierStatus::Pending.as_str())
                .bind(workflow_state_json_string(
                    "workflow_run_step_verifier_definition",
                    json!(verifier),
                )?)
                .bind(
                    verifier
                        .retry_policy
                        .as_ref()
                        .map(|retry_policy| i64::from(retry_policy.max_attempts)),
                )
                .bind(now_ms)
                .bind(now_ms)
                .execute(&mut **tx)
                .await?;
            }
        }
    }

    Ok(())
}

async fn insert_workflow_run_automation_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    spec: &codex_workflows::WorkflowSpec,
    now_ms: i64,
) -> anyhow::Result<()> {
    for workflow_loop in &spec.loops {
        let next_fire_at_ms = workflow_loop.trigger_step.is_none().then_some(now_ms);
        let expires_at_ms = workflow_loop
            .expires_after_seconds
            .map(i64::try_from)
            .transpose()?
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000)));
        sqlx::query(
            r#"
INSERT INTO workflow_run_timers (
    timer_id,
    run_id,
    workflow_loop_id,
    title,
    schedule_json,
    timezone,
    status,
    stop_condition_json,
    trigger_step_id,
    max_iterations,
    next_fire_at_ms,
    expires_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(run_id)
        .bind(workflow_loop.id.as_str())
        .bind(redact_state_string(workflow_loop.title.as_str()))
        .bind(workflow_state_json_string(
            "workflow_run_loop_schedule",
            json!(workflow_loop.schedule),
        )?)
        .bind(workflow_loop.timezone.as_str())
        .bind(workflow_state_json_string(
            "workflow_run_loop_stop_condition",
            json!(workflow_loop.stop_condition),
        )?)
        .bind(workflow_loop.trigger_step.as_deref())
        .bind(i64::from(workflow_loop.max_iterations))
        .bind(next_fire_at_ms)
        .bind(expires_at_ms)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut **tx)
        .await?;
    }
    for monitor in &spec.monitors {
        sqlx::query(
            r#"
INSERT INTO workflow_run_monitor_links (
    link_id,
    run_id,
    workflow_monitor_id,
    title,
    source,
    monitor_ref,
    trigger_step_id,
    stop_condition_json,
    max_events_per_tick,
    status,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(run_id)
        .bind(monitor.id.as_str())
        .bind(redact_state_string(monitor.title.as_str()))
        .bind(monitor.source.as_str())
        .bind(monitor.monitor_ref.as_deref())
        .bind(monitor.trigger_step.as_deref())
        .bind(
            monitor
                .stop_condition
                .as_ref()
                .map(|stop_condition| {
                    workflow_state_json_string(
                        "workflow_run_monitor_stop_condition",
                        json!(stop_condition),
                    )
                })
                .transpose()?,
        )
        .bind(i64::from(monitor.max_events_per_tick))
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn workflow_run_snapshot_by_idempotency_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    workflow_record_id: &str,
    idempotency_key: &str,
) -> anyhow::Result<Option<crate::WorkflowRunSnapshot>> {
    let run_id = sqlx::query_scalar::<_, String>(
        r#"
SELECT run_id
FROM workflow_runs
WHERE workflow_record_id = ? AND idempotency_key = ?
        "#,
    )
    .bind(workflow_record_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some(run_id) = run_id {
        maybe_snapshot_workflow_run_in_tx(tx, run_id.as_str()).await
    } else {
        Ok(None)
    }
}

pub(super) async fn maybe_snapshot_workflow_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<crate::WorkflowRunSnapshot>> {
    if get_workflow_run_in_tx(tx, run_id).await?.is_none() {
        return Ok(None);
    }
    snapshot_workflow_run_in_tx(tx, run_id).await.map(Some)
}

pub(super) async fn snapshot_workflow_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<crate::WorkflowRunSnapshot> {
    let run = get_workflow_run_in_tx(tx, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workflow run {run_id} does not exist"))?;
    let mut steps = list_workflow_run_steps_in_tx(tx, run_id).await?;
    let dependencies = list_workflow_run_step_dependencies_in_tx(tx, run_id).await?;
    for step in &mut steps {
        step.depends_on = dependencies.get(&step.step_id).cloned().unwrap_or_default();
    }
    let verifiers = list_workflow_run_step_verifiers_in_tx(tx, run_id).await?;
    let events = list_workflow_run_events_in_tx(tx, run_id).await?;
    Ok(crate::WorkflowRunSnapshot {
        run,
        steps,
        verifiers,
        events,
    })
}

async fn get_workflow_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<crate::WorkflowRun>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(workflow_run_select_by(
        r#"
SELECT
"#,
        "run_id = ?",
    )))
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;

    row.map(|row| WorkflowRunRow::try_from_row(&row)?.try_into())
        .transpose()
}

async fn list_workflow_run_steps_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Vec<crate::WorkflowRunStep>> {
    let rows = sqlx::query(
        r#"
SELECT
    step_run_id,
    run_id,
    step_id,
    sequence,
    title,
    agent_id,
    status,
    status_reason,
    reason_code,
    parallel_group,
    approval_gate,
    approval_state,
    model_route_json,
    workspace_json,
    background_agent_run_id,
    branch_admission_json,
    completion_model_marked_state,
    attempt,
    created_at_ms,
    updated_at_ms,
    started_at_ms,
    completed_at_ms
FROM workflow_run_steps
WHERE run_id = ?
ORDER BY sequence, step_id
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .map(|row| WorkflowRunStepRow::try_from_row(&row)?.try_into())
        .collect()
}

async fn list_workflow_run_step_dependencies_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let rows = sqlx::query(
        r#"
SELECT step_id, depends_on_step_id
FROM workflow_run_step_dependencies
WHERE run_id = ?
ORDER BY step_id, depends_on_step_id
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        dependencies
            .entry(row.try_get("step_id")?)
            .or_default()
            .push(row.try_get("depends_on_step_id")?);
    }
    Ok(dependencies)
}

async fn list_workflow_run_step_verifiers_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Vec<crate::WorkflowRunStepVerifier>> {
    let rows = sqlx::query(
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
ORDER BY step_id, verifier_id
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .map(|row| WorkflowRunStepVerifierRow::try_from_row(&row)?.try_into())
        .collect()
}

async fn list_workflow_run_events_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Vec<crate::WorkflowRunEvent>> {
    let rows = sqlx::query(
        r#"
SELECT
    event_id,
    run_id,
    seq,
    event_type,
    actor_kind,
    actor_id,
    step_run_id,
    verifier_run_id,
    visibility,
    event_payload_json,
    created_at_ms
FROM workflow_run_events
WHERE run_id = ?
ORDER BY seq
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .map(|row| WorkflowRunEventRow::try_from_row(&row)?.try_into())
        .collect()
}

pub(super) async fn append_workflow_run_event_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    event: WorkflowRunEventAppend,
) -> anyhow::Result<i64> {
    let seq = sqlx::query_scalar::<_, i64>(
        r#"
UPDATE workflow_runs
SET last_event_seq = last_event_seq + 1, updated_at_ms = ?
WHERE run_id = ?
RETURNING last_event_seq
        "#,
    )
    .bind(event.now_ms)
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        r#"
INSERT INTO workflow_run_events (
    event_id,
    run_id,
    seq,
    event_type,
    actor_kind,
    actor_id,
    step_run_id,
    verifier_run_id,
    visibility,
    event_payload_json,
    created_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(run_id)
    .bind(seq)
    .bind(event.event_type)
    .bind(event.actor_kind)
    .bind(event.actor_id)
    .bind(event.step_run_id)
    .bind(event.verifier_run_id)
    .bind(event.visibility)
    .bind(workflow_state_json_string(
        "workflow_run_event",
        event.payload,
    )?)
    .bind(event.now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(seq)
}

fn workflow_run_select_columns() -> &'static str {
    r#"
    run_id,
    workflow_record_id,
    source_thread_id,
    idempotency_key,
    spec_workflow_id,
    schema_version,
    source_yaml_sha256,
    status,
    status_reason,
    reason_code,
    generation,
    owner_id,
    lease_expires_at_ms,
    heartbeat_at_ms,
    last_event_seq,
    agents_json,
    execution_defaults_json,
    limits_json,
    approvals_json,
    loops_json,
    monitor_links_json,
    artifacts_json,
    cleanup_json,
    created_at_ms,
    updated_at_ms,
    started_at_ms,
    completed_at_ms
"#
}

fn workflow_run_select_by(prefix: &'static str, predicate: &'static str) -> String {
    format!(
        "{}{}FROM workflow_runs WHERE {predicate}",
        prefix,
        workflow_run_select_columns()
    )
}

pub(super) fn workflow_state_json_string(kind: &str, data: Value) -> anyhow::Result<String> {
    redact_state_json_string(&json!({
        "schemaVersion": "workflow.run_state/v0",
        "redactionVersion": 1,
        "kind": kind,
        "data": data,
    }))
}

fn optional_workflow_state_json_string<T>(
    kind: &str,
    value: Option<&T>,
) -> anyhow::Result<Option<String>>
where
    T: serde::Serialize + ?Sized,
{
    value
        .map(|value| serde_json::to_value(value).map_err(anyhow::Error::from))
        .transpose()?
        .map(|data| workflow_state_json_string(kind, data))
        .transpose()
}

fn metadata_from_spec(
    spec: &codex_workflows::WorkflowSpec,
    source_yaml_sha256: String,
) -> anyhow::Result<WorkflowSpecMetadata> {
    let parallel_groups = spec
        .steps
        .iter()
        .filter_map(|step| step.parallel_group.as_deref())
        .collect::<BTreeSet<_>>();
    let verifiers = spec
        .steps
        .iter()
        .filter_map(|step| step.completion.as_ref())
        .flat_map(|completion| completion.verifiers.iter())
        .collect::<Vec<_>>();

    Ok(WorkflowSpecMetadata {
        schema_version: spec.schema_version.clone(),
        spec_workflow_id: spec.workflow_id.clone(),
        display_name: spec.display_name.clone(),
        status: spec.status.into(),
        source_yaml_sha256,
        agent_count: i64::try_from(spec.agents.len())?,
        step_count: i64::try_from(spec.steps.len())?,
        parallel_group_count: i64::try_from(parallel_groups.len())?,
        verifier_count: i64::try_from(verifiers.len())?,
        run_command_verifier_count: i64::try_from(
            verifiers
                .iter()
                .filter(|verifier| verifier.kind == "run_commands")
                .count(),
        )?,
        model_routed_step_count: i64::try_from(
            spec.steps
                .iter()
                .filter(|step| step.model.is_some())
                .count(),
        )?,
    })
}

fn workflow_source_sha256(source_yaml: &str) -> String {
    format!("{:x}", Sha256::digest(source_yaml.as_bytes()))
}

fn sanitized_workflow_cancel_reason() -> &'static str {
    "user requested workflow cancellation"
}

fn workflow_spec_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::WorkflowSpecRecord> {
    WorkflowSpecRow::try_from_row(row)?.try_into()
}

fn workflow_spec_select_columns() -> &'static str {
    r#"
    workflow_record_id,
    spec_workflow_id,
    source_thread_id,
    schema_version,
    display_name,
    status,
    source_yaml,
    source_yaml_sha256,
    agent_count,
    step_count,
    parallel_group_count,
    verifier_count,
    run_command_verifier_count,
    model_routed_step_count,
    created_at_ms,
    updated_at_ms
"#
}

fn workflow_spec_select_by(prefix: &'static str, predicate: &'static str) -> String {
    format!(
        "{}{}FROM workflow_specs WHERE {predicate}",
        prefix,
        workflow_spec_select_columns()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use codex_prompts::DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML;
    use pretty_assertions::assert_eq;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id(id: u32) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{id:012}"))
            .expect("valid thread id")
    }

    fn yaml_single_quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
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

    #[tokio::test]
    async fn save_workflow_spec_yaml_persists_compiled_metadata_only() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;

        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");

        assert_eq!("wf_dental_lead_saas_launch", saved.spec_workflow_id);
        assert_eq!(Some(thread_id), saved.source_thread_id);
        assert_eq!(crate::WorkflowSpecStatus::Draft, saved.status);
        assert_eq!(6, saved.agent_count);
        assert!(saved.step_count >= 12);
        assert!(saved.verifier_count >= 6);
        assert!(saved.run_command_verifier_count >= 4);
        assert!(saved.model_routed_step_count >= 12);
        assert_eq!(
            DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML,
            saved.source_yaml.as_str()
        );
        assert_eq!(
            workflow_source_sha256(DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML),
            saved.source_yaml_sha256
        );

        let loaded = runtime
            .workflows()
            .get_workflow_spec_by_spec_workflow_id("wf_dental_lead_saas_launch")
            .await
            .expect("workflow spec lookup should succeed")
            .expect("workflow spec should exist");
        assert_eq!(saved, loaded);

        let loaded_for_thread = runtime
            .workflows()
            .get_thread_workflow_spec(thread_id, saved.workflow_record_id.as_str())
            .await
            .expect("thread workflow spec lookup should succeed")
            .expect("thread workflow spec should exist");
        assert_eq!(saved, loaded_for_thread);

        let page = runtime
            .workflows()
            .list_thread_workflow_specs_page(
                thread_id,
                /*cursor*/ None,
                DEFAULT_THREAD_WORKFLOW_LIST_LIMIT,
            )
            .await
            .expect("thread workflow spec list should succeed");
        assert_eq!(
            WorkflowSpecListPage {
                data: vec![saved.clone()],
                next_cursor: None,
            },
            page
        );

        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .unwrap()
        );
        assert_eq!(
            Vec::<crate::ThreadSchedule>::new(),
            runtime
                .thread_schedules()
                .list_thread_schedules(thread_id)
                .await
                .unwrap()
        );
        assert_eq!(
            Vec::<crate::ThreadMonitor>::new(),
            runtime
                .thread_monitors()
                .list_thread_monitors(thread_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn delete_thread_workflow_spec_removes_spec_and_reports_missing() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;

        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");

        let outcome = runtime
            .workflows()
            .delete_thread_workflow_spec(thread_id, saved.workflow_record_id.as_str())
            .await
            .expect("delete should succeed");
        assert_eq!(WorkflowSpecDeleteOutcome::Deleted, outcome);

        let loaded = runtime
            .workflows()
            .get_thread_workflow_spec(thread_id, saved.workflow_record_id.as_str())
            .await
            .expect("thread workflow spec lookup should succeed");
        assert_eq!(None, loaded);

        // Deleting again (or an unknown record) reports NotFound rather than erroring.
        let repeat = runtime
            .workflows()
            .delete_thread_workflow_spec(thread_id, saved.workflow_record_id.as_str())
            .await
            .expect("delete should succeed");
        assert_eq!(WorkflowSpecDeleteOutcome::NotFound, repeat);

        let unknown = runtime
            .workflows()
            .delete_thread_workflow_spec(thread_id, "workflow_does_not_exist")
            .await
            .expect("delete should succeed");
        assert_eq!(WorkflowSpecDeleteOutcome::NotFound, unknown);
    }

    #[tokio::test]
    async fn delete_thread_workflow_spec_is_thread_scoped() {
        let runtime = test_runtime().await;
        let owner_thread_id = test_thread_id(/*id*/ 1);
        let other_thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, owner_thread_id).await;
        upsert_test_thread(&runtime, other_thread_id).await;

        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(owner_thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");

        // A different thread cannot delete another thread's spec.
        let outcome = runtime
            .workflows()
            .delete_thread_workflow_spec(other_thread_id, saved.workflow_record_id.as_str())
            .await
            .expect("delete should succeed");
        assert_eq!(WorkflowSpecDeleteOutcome::NotFound, outcome);
        assert!(
            runtime
                .workflows()
                .get_thread_workflow_spec(owner_thread_id, saved.workflow_record_id.as_str())
                .await
                .expect("lookup should succeed")
                .is_some()
        );
    }

    #[tokio::test]
    async fn delete_thread_workflow_spec_blocked_by_active_run() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;

        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");

        let snapshot = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id.clone(),
                source_thread_id: Some(thread_id),
                idempotency_key: Some("delete-guard".to_string()),
            })
            .await
            .expect("workflow run should be created");
        assert_eq!(crate::WorkflowRunStatus::Pending, snapshot.run.status);

        let outcome = runtime
            .workflows()
            .delete_thread_workflow_spec(thread_id, saved.workflow_record_id.as_str())
            .await
            .expect("delete should succeed");
        assert_eq!(WorkflowSpecDeleteOutcome::BlockedByActiveRun, outcome);

        // The spec (and its run) are left intact.
        assert!(
            runtime
                .workflows()
                .get_thread_workflow_spec(thread_id, saved.workflow_record_id.as_str())
                .await
                .expect("lookup should succeed")
                .is_some()
        );
        assert!(
            runtime
                .workflows()
                .get_workflow_run_snapshot(snapshot.run.run_id.as_str())
                .await
                .expect("run lookup should succeed")
                .is_some()
        );
    }

    #[tokio::test]
    async fn invalid_workflow_yaml_does_not_persist() {
        let runtime = test_runtime().await;
        let err = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: None,
                source_yaml: "```yaml\nnot: raw\n```".to_string(),
            })
            .await
            .expect_err("invalid workflow spec should fail");

        assert!(err.to_string().contains("Markdown fences"));
        assert_eq!(
            None,
            runtime
                .workflows()
                .get_workflow_spec_by_spec_workflow_id("not")
                .await
                .expect("lookup should succeed")
        );
    }

    #[tokio::test]
    async fn verifier_commands_are_persisted_as_inert_data() {
        let runtime = test_runtime().await;
        let marker = unique_temp_dir().join("workflow-verifier-command-ran");
        let marker_command = format!("touch {}", marker.display());
        let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
            "\"npm test -- --runInBand\"",
            &yaml_single_quoted(&marker_command),
        );

        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: None,
                source_yaml: yaml,
            })
            .await
            .expect("workflow spec should save");

        assert!(saved.source_yaml.contains(marker_command.as_str()));
        assert!(
            !marker.exists(),
            "saving workflow specs must not execute verifier commands"
        );
    }

    #[tokio::test]
    async fn list_thread_workflow_specs_paginates_and_is_thread_scoped() {
        let runtime = test_runtime().await;
        let first_thread_id = test_thread_id(/*id*/ 1);
        let second_thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, first_thread_id).await;
        upsert_test_thread(&runtime, second_thread_id).await;

        let first_yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML
            .replace("wf_dental_lead_saas_launch", "wf_first_thread");
        let second_yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML
            .replace("wf_dental_lead_saas_launch", "wf_second_thread");
        let first = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(first_thread_id),
                source_yaml: first_yaml,
            })
            .await
            .expect("first workflow spec should save");
        let _second = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(second_thread_id),
                source_yaml: second_yaml,
            })
            .await
            .expect("second workflow spec should save");

        let page = runtime
            .workflows()
            .list_thread_workflow_specs_page(
                first_thread_id,
                /*cursor*/ None,
                /*limit*/ 1,
            )
            .await
            .expect("workflow spec page should load");

        assert_eq!(
            WorkflowSpecListPage {
                data: vec![first],
                next_cursor: None,
            },
            page
        );
    }

    #[tokio::test]
    async fn same_spec_workflow_id_is_scoped_by_source_thread() {
        let runtime = test_runtime().await;
        let first_thread_id = test_thread_id(/*id*/ 1);
        let second_thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, first_thread_id).await;
        upsert_test_thread(&runtime, second_thread_id).await;

        let first = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(first_thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("first workflow spec should save");
        let second = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(second_thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("second workflow spec should save");

        assert_ne!(first.workflow_record_id, second.workflow_record_id);
        assert_eq!(first.spec_workflow_id, second.spec_workflow_id);
        assert_eq!(Some(first_thread_id), first.source_thread_id);
        assert_eq!(Some(second_thread_id), second.source_thread_id);
        assert!(
            runtime
                .workflows()
                .get_thread_workflow_spec(second_thread_id, first.workflow_record_id.as_str())
                .await
                .expect("cross-thread lookup should succeed")
                .is_none()
        );
    }

    #[tokio::test]
    async fn create_workflow_run_persists_inert_snapshot_state() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;
        let marker = unique_temp_dir().join("workflow-run-verifier-command-ran");
        let marker_command = format!("touch {}", marker.display());
        let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
            "\"npm test -- --runInBand\"",
            &yaml_single_quoted(&marker_command),
        );
        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: yaml,
            })
            .await
            .expect("workflow spec should save");

        let snapshot = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id.clone(),
                source_thread_id: Some(thread_id),
                idempotency_key: Some("run-key-1".to_string()),
            })
            .await
            .expect("workflow run should be created");

        assert_eq!(saved.workflow_record_id, snapshot.run.workflow_record_id);
        assert_eq!(saved.spec_workflow_id, snapshot.run.spec_workflow_id);
        assert_eq!(saved.schema_version, snapshot.run.schema_version);
        assert_eq!(saved.source_yaml_sha256, snapshot.run.source_yaml_sha256);
        assert_eq!(Some(thread_id), snapshot.run.source_thread_id);
        assert_eq!(Some("run-key-1".to_string()), snapshot.run.idempotency_key);
        assert_eq!(crate::WorkflowRunStatus::Pending, snapshot.run.status);
        assert_eq!(0, snapshot.run.generation);
        assert_eq!(None, snapshot.run.owner_id);
        assert_eq!(None, snapshot.run.lease_expires_at);
        assert_eq!(None, snapshot.run.heartbeat_at);
        assert_eq!(1, snapshot.run.last_event_seq);
        assert_eq!(
            "workflow.run_state/v0",
            snapshot.run.agents_json["schemaVersion"]
        );
        assert_eq!(1, snapshot.run.agents_json["redactionVersion"]);
        assert_eq!("workflow_run_agents", snapshot.run.agents_json["kind"]);
        assert_eq!(
            6,
            snapshot.run.agents_json["data"].as_array().unwrap().len()
        );
        assert_eq!(saved.step_count as usize, snapshot.steps.len());
        assert!(
            snapshot
                .steps
                .iter()
                .all(|step| step.status == crate::WorkflowRunStepStatus::Pending)
        );
        assert!(
            snapshot
                .steps
                .iter()
                .any(|step| !step.depends_on.is_empty()),
            "workflow run should persist step dependencies"
        );
        assert_eq!(saved.verifier_count as usize, snapshot.verifiers.len());
        assert!(
            snapshot
                .verifiers
                .iter()
                .all(|verifier| verifier.status == crate::WorkflowRunStepVerifierStatus::Pending)
        );
        assert_eq!(1, snapshot.events.len());
        assert_eq!("created", snapshot.events[0].event_type);
        assert_eq!("system", snapshot.events[0].actor_kind);
        assert_eq!(
            "workflow_run_event",
            snapshot.events[0].event_payload_json["kind"]
        );
        assert!(
            !marker.exists(),
            "creating workflow runs must not execute verifier commands"
        );

        let loaded = runtime
            .workflows()
            .get_workflow_run_snapshot(snapshot.run.run_id.as_str())
            .await
            .expect("workflow run should load")
            .expect("workflow run should exist");
        assert_eq!(snapshot, loaded);
        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .unwrap()
        );
        assert_eq!(
            Vec::<crate::ThreadSchedule>::new(),
            runtime
                .thread_schedules()
                .list_thread_schedules(thread_id)
                .await
                .unwrap()
        );
        assert_eq!(
            Vec::<crate::ThreadMonitor>::new(),
            runtime
                .thread_monitors()
                .list_thread_monitors(thread_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn keyed_workflow_run_create_is_idempotent_but_unkeyed_runs_are_distinct() {
        let runtime = test_runtime().await;
        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: None,
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");

        let first = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id.clone(),
                source_thread_id: None,
                idempotency_key: Some("same-key".to_string()),
            })
            .await
            .expect("first keyed workflow run should save");
        let second = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id.clone(),
                source_thread_id: None,
                idempotency_key: Some("same-key".to_string()),
            })
            .await
            .expect("second keyed workflow run should reuse first run");
        assert_eq!(first.run.run_id, second.run.run_id);
        assert_eq!(1, second.events.len());

        let unkeyed_first = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id.clone(),
                source_thread_id: None,
                idempotency_key: None,
            })
            .await
            .expect("first unkeyed workflow run should save");
        let unkeyed_second = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id,
                source_thread_id: None,
                idempotency_key: None,
            })
            .await
            .expect("second unkeyed workflow run should save");
        assert_ne!(unkeyed_first.run.run_id, unkeyed_second.run.run_id);
    }

    #[tokio::test]
    async fn workflow_run_cancel_request_appends_one_transactional_event() {
        let runtime = test_runtime().await;
        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: None,
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");
        let run = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id,
                source_thread_id: None,
                idempotency_key: Some("cancel-key".to_string()),
            })
            .await
            .expect("workflow run should save");

        let cancelled = runtime
            .workflows()
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: run.run.run_id.clone(),
                reason: "user stopped the workflow".to_string(),
            })
            .await
            .expect("cancel request should succeed")
            .expect("workflow run should exist");
        assert!(cancelled.changed);
        assert_eq!(
            crate::WorkflowRunStatus::CancelRequested,
            cancelled.snapshot.run.status
        );
        assert_eq!(
            Some("user requested workflow cancellation"),
            cancelled.snapshot.run.status_reason.as_deref()
        );
        assert_eq!(
            Some("user_cancel_requested"),
            cancelled.snapshot.run.reason_code.as_deref()
        );
        assert_eq!(2, cancelled.snapshot.run.last_event_seq);
        assert_eq!(
            vec!["created", "cancel_requested"],
            cancelled
                .snapshot
                .events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            !cancelled
                .snapshot
                .events
                .iter()
                .map(|event| event.event_payload_json.to_string())
                .collect::<String>()
                .contains("user stopped the workflow")
        );

        let cancelled_again = runtime
            .workflows()
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: run.run.run_id,
                reason: "second cancel request".to_string(),
            })
            .await
            .expect("second cancel request should succeed")
            .expect("workflow run should exist");
        assert!(!cancelled_again.changed);
        assert_eq!(2, cancelled_again.snapshot.run.last_event_seq);
        assert_eq!(2, cancelled_again.snapshot.events.len());
    }

    #[tokio::test]
    async fn workflow_run_keeps_spec_snapshot_when_spec_is_updated() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;
        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");
        let run = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id.clone(),
                source_thread_id: Some(thread_id),
                idempotency_key: Some("snapshot-key".to_string()),
            })
            .await
            .expect("workflow run should save");

        let updated_yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
            "build me a saas that collects leads to dentists and sells them to these",
            "updated workflow prompt after run creation",
        );
        let updated = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: updated_yaml,
            })
            .await
            .expect("workflow spec update should save");
        assert_eq!(saved.workflow_record_id, updated.workflow_record_id);
        assert_ne!(saved.source_yaml_sha256, updated.source_yaml_sha256);

        let loaded = runtime
            .workflows()
            .get_workflow_run_snapshot(run.run.run_id.as_str())
            .await
            .expect("workflow run should load")
            .expect("workflow run should exist");
        assert_eq!(saved.source_yaml_sha256, loaded.run.source_yaml_sha256);
        assert_ne!(updated.source_yaml_sha256, loaded.run.source_yaml_sha256);
    }

    #[tokio::test]
    async fn workflow_run_rejects_mismatched_source_thread() {
        let runtime = test_runtime().await;
        let first_thread_id = test_thread_id(/*id*/ 1);
        let second_thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, first_thread_id).await;
        upsert_test_thread(&runtime, second_thread_id).await;
        let saved = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(first_thread_id),
                source_yaml: DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_string(),
            })
            .await
            .expect("workflow spec should save");

        let err = runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: saved.workflow_record_id,
                source_thread_id: Some(second_thread_id),
                idempotency_key: Some("wrong-thread".to_string()),
            })
            .await
            .expect_err("wrong source thread should reject");
        assert!(
            err.to_string()
                .contains("belongs to a different source thread")
        );
    }
}
