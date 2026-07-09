use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSpecStatus {
    Draft,
    NeedsClarification,
    Blocked,
}

impl WorkflowSpecStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::NeedsClarification => "needs_clarification",
            Self::Blocked => "blocked",
        }
    }
}

impl TryFrom<&str> for WorkflowSpecStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "draft" => Ok(Self::Draft),
            "needs_clarification" => Ok(Self::NeedsClarification),
            "blocked" => Ok(Self::Blocked),
            other => Err(anyhow!("unknown workflow spec status `{other}`")),
        }
    }
}

impl From<codex_workflows::WorkflowStatus> for WorkflowSpecStatus {
    fn from(value: codex_workflows::WorkflowStatus) -> Self {
        match value {
            codex_workflows::WorkflowStatus::Draft => Self::Draft,
            codex_workflows::WorkflowStatus::NeedsClarification => Self::NeedsClarification,
            codex_workflows::WorkflowStatus::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSpecRecord {
    pub workflow_record_id: String,
    pub spec_workflow_id: String,
    pub source_thread_id: Option<ThreadId>,
    pub schema_version: String,
    pub display_name: String,
    pub status: WorkflowSpecStatus,
    pub source_yaml: String,
    pub source_yaml_sha256: String,
    pub agent_count: i64,
    pub step_count: i64,
    pub parallel_group_count: i64,
    pub verifier_count: i64,
    pub run_command_verifier_count: i64,
    pub model_routed_step_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) struct WorkflowSpecRow {
    pub workflow_record_id: String,
    pub spec_workflow_id: String,
    pub source_thread_id: Option<String>,
    pub schema_version: String,
    pub display_name: String,
    pub status: String,
    pub source_yaml: String,
    pub source_yaml_sha256: String,
    pub agent_count: i64,
    pub step_count: i64,
    pub parallel_group_count: i64,
    pub verifier_count: i64,
    pub run_command_verifier_count: i64,
    pub model_routed_step_count: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl WorkflowSpecRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            workflow_record_id: row.try_get("workflow_record_id")?,
            spec_workflow_id: row.try_get("spec_workflow_id")?,
            source_thread_id: row.try_get("source_thread_id")?,
            schema_version: row.try_get("schema_version")?,
            display_name: row.try_get("display_name")?,
            status: row.try_get("status")?,
            source_yaml: row.try_get("source_yaml")?,
            source_yaml_sha256: row.try_get("source_yaml_sha256")?,
            agent_count: row.try_get("agent_count")?,
            step_count: row.try_get("step_count")?,
            parallel_group_count: row.try_get("parallel_group_count")?,
            verifier_count: row.try_get("verifier_count")?,
            run_command_verifier_count: row.try_get("run_command_verifier_count")?,
            model_routed_step_count: row.try_get("model_routed_step_count")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<WorkflowSpecRow> for WorkflowSpecRecord {
    type Error = anyhow::Error;

    fn try_from(row: WorkflowSpecRow) -> Result<Self> {
        Ok(Self {
            workflow_record_id: row.workflow_record_id,
            spec_workflow_id: row.spec_workflow_id,
            source_thread_id: row
                .source_thread_id
                .map(|thread_id| ThreadId::from_string(&thread_id))
                .transpose()?,
            schema_version: row.schema_version,
            display_name: row.display_name,
            status: WorkflowSpecStatus::try_from(row.status.as_str())?,
            source_yaml: row.source_yaml,
            source_yaml_sha256: row.source_yaml_sha256,
            agent_count: row.agent_count,
            step_count: row.step_count,
            parallel_group_count: row.parallel_group_count,
            verifier_count: row.verifier_count,
            run_command_verifier_count: row.run_command_verifier_count,
            model_routed_step_count: row.model_routed_step_count,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunStatus {
    Pending,
    Running,
    Waiting,
    Blocked,
    Paused,
    CancelRequested,
    Cancelled,
    Failed,
    Completed,
    Other(String),
}

impl WorkflowRunStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Paused => "paused",
            Self::CancelRequested => "cancel_requested",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::Other(status) => status.as_str(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::Completed)
    }
}

impl TryFrom<&str> for WorkflowRunStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "blocked" => Ok(Self::Blocked),
            "paused" => Ok(Self::Paused),
            "cancel_requested" => Ok(Self::CancelRequested),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "completed" | "complete" => Ok(Self::Completed),
            other if !other.trim().is_empty() => Ok(Self::Other(other.to_string())),
            other => Err(anyhow!("invalid workflow run status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunStepStatus {
    Pending,
    Ready,
    Active,
    WaitingVerifier,
    Blocked,
    Skipped,
    Cancelled,
    Failed,
    Succeeded,
    Other(String),
}

impl WorkflowRunStepStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::WaitingVerifier => "waiting_verifier",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Succeeded => "succeeded",
            Self::Other(status) => status.as_str(),
        }
    }
}

impl TryFrom<&str> for WorkflowRunStepStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "active" => Ok(Self::Active),
            "waiting_verifier" => Ok(Self::WaitingVerifier),
            "blocked" => Ok(Self::Blocked),
            "skipped" => Ok(Self::Skipped),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "succeeded" | "complete" => Ok(Self::Succeeded),
            other if !other.trim().is_empty() => Ok(Self::Other(other.to_string())),
            other => Err(anyhow!("invalid workflow run step status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunStepVerifierStatus {
    Pending,
    Running,
    Blocked,
    Passed,
    Failed,
    Skipped,
    Other(String),
}

impl WorkflowRunStepVerifierStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Other(status) => status.as_str(),
        }
    }
}

impl TryFrom<&str> for WorkflowRunStepVerifierStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "blocked" => Ok(Self::Blocked),
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            other if !other.trim().is_empty() => Ok(Self::Other(other.to_string())),
            other => Err(anyhow!(
                "invalid workflow run step verifier status `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    pub run_id: String,
    pub workflow_record_id: String,
    pub source_thread_id: Option<ThreadId>,
    pub idempotency_key: Option<String>,
    pub spec_workflow_id: String,
    pub schema_version: String,
    pub source_yaml_sha256: String,
    pub status: WorkflowRunStatus,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub generation: i64,
    pub owner_id: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub last_event_seq: i64,
    pub agents_json: Value,
    pub execution_defaults_json: Value,
    pub limits_json: Value,
    pub approvals_json: Value,
    pub loops_json: Option<Value>,
    pub monitor_links_json: Option<Value>,
    pub artifacts_json: Value,
    pub cleanup_json: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunStep {
    pub step_run_id: String,
    pub run_id: String,
    pub step_id: String,
    pub sequence: i64,
    pub title: String,
    pub agent_id: String,
    pub status: WorkflowRunStepStatus,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub parallel_group: Option<String>,
    pub approval_gate: Option<String>,
    pub model_route_json: Option<Value>,
    pub workspace_json: Option<Value>,
    pub background_agent_run_id: Option<String>,
    pub branch_admission_json: Option<Value>,
    pub completion_model_marked_state: Option<String>,
    pub attempt: i64,
    pub depends_on: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunStepVerifier {
    pub verifier_run_id: String,
    pub run_id: String,
    pub step_id: String,
    pub verifier_id: String,
    pub verifier_type: String,
    pub status: WorkflowRunStepVerifierStatus,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub definition_json: Value,
    pub last_result_json: Option<Value>,
    pub attempt_count: i64,
    pub max_attempts: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunEvent {
    pub event_id: String,
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub step_run_id: Option<String>,
    pub verifier_run_id: Option<String>,
    pub visibility: String,
    pub event_payload_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunSnapshot {
    pub run: WorkflowRun,
    pub steps: Vec<WorkflowRunStep>,
    pub verifiers: Vec<WorkflowRunStepVerifier>,
    pub events: Vec<WorkflowRunEvent>,
}

/// A step currently gated behind a pending approval review.
///
/// Carries only the author-defined step id and approval-gate label; never a
/// raw approvals payload, prompt, or secret. Consumers that render these to a
/// UI are still expected to sanitize the labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowApprovalGate {
    pub step_id: String,
    pub gate: String,
}

/// A compact, non-secret summary of workflow-scoped approval-review state,
/// derived from the run-level approvals config and per-step approval gates.
///
/// This is the run context that lets a manager surface (or link to) the
/// existing approval-review flow and decide whether its approval affordance
/// should be enabled. It intentionally excludes the raw `approvals_json`
/// payload, step titles, prompts, and any other free-form content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowApprovalReview {
    /// The run declares run-level approval gates (`approvals.required_before`
    /// is present and non-empty).
    pub has_approval_config: bool,
    /// Steps currently awaiting an approval review before they can be admitted.
    pub pending: Vec<WorkflowApprovalGate>,
}

impl WorkflowApprovalReview {
    /// Number of steps currently awaiting an approval review.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Whether there is a pending approval to act on. Manager approval
    /// actions should stay disabled unless this is true.
    pub fn is_actionable(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether any workflow-scoped approval data is available for this run,
    /// either because approval gates are configured or a gated step is
    /// currently pending. Approval rows should stay disabled otherwise.
    pub fn is_available(&self) -> bool {
        self.has_approval_config || !self.pending.is_empty()
    }
}

impl WorkflowRunSnapshot {
    /// Compute the workflow-scoped approval-review summary for this run.
    pub fn approval_review(&self) -> WorkflowApprovalReview {
        let has_approval_config = approvals_config_present(&self.run.approvals_json);
        let pending = self
            .steps
            .iter()
            .filter_map(|step| {
                let gate = step.approval_gate.as_deref()?;
                step_awaiting_approval(&step.status).then(|| WorkflowApprovalGate {
                    step_id: step.step_id.clone(),
                    gate: gate.to_string(),
                })
            })
            .collect();
        WorkflowApprovalReview {
            has_approval_config,
            pending,
        }
    }
}

/// True when the run-level approvals config declares at least one required gate.
///
/// The stored `approvals_json` is a redaction envelope
/// (`{"kind": ..., "data": {"required_before": [...]}}`), so the config lives
/// under `data`. Fall back to the bare value for robustness.
fn approvals_config_present(approvals_json: &Value) -> bool {
    let payload = approvals_json.get("data").unwrap_or(approvals_json);
    payload
        .get("required_before")
        .and_then(Value::as_array)
        .is_some_and(|entries| !entries.is_empty())
}

/// A gated step is awaiting approval while it has not yet been admitted for
/// execution (the orchestrator never admits steps with an approval gate set).
fn step_awaiting_approval(status: &WorkflowRunStepStatus) -> bool {
    matches!(
        status,
        WorkflowRunStepStatus::Pending
            | WorkflowRunStepStatus::Ready
            | WorkflowRunStepStatus::Blocked
    )
}

pub(crate) struct WorkflowRunRow {
    pub run_id: String,
    pub workflow_record_id: String,
    pub source_thread_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub spec_workflow_id: String,
    pub schema_version: String,
    pub source_yaml_sha256: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub generation: i64,
    pub owner_id: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub heartbeat_at_ms: Option<i64>,
    pub last_event_seq: i64,
    pub agents_json: String,
    pub execution_defaults_json: String,
    pub limits_json: String,
    pub approvals_json: String,
    pub loops_json: Option<String>,
    pub monitor_links_json: Option<String>,
    pub artifacts_json: String,
    pub cleanup_json: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
}

impl WorkflowRunRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            run_id: row.try_get("run_id")?,
            workflow_record_id: row.try_get("workflow_record_id")?,
            source_thread_id: row.try_get("source_thread_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            spec_workflow_id: row.try_get("spec_workflow_id")?,
            schema_version: row.try_get("schema_version")?,
            source_yaml_sha256: row.try_get("source_yaml_sha256")?,
            status: row.try_get("status")?,
            status_reason: row.try_get("status_reason")?,
            reason_code: row.try_get("reason_code")?,
            generation: row.try_get("generation")?,
            owner_id: row.try_get("owner_id")?,
            lease_expires_at_ms: row.try_get("lease_expires_at_ms")?,
            heartbeat_at_ms: row.try_get("heartbeat_at_ms")?,
            last_event_seq: row.try_get("last_event_seq")?,
            agents_json: row.try_get("agents_json")?,
            execution_defaults_json: row.try_get("execution_defaults_json")?,
            limits_json: row.try_get("limits_json")?,
            approvals_json: row.try_get("approvals_json")?,
            loops_json: row.try_get("loops_json")?,
            monitor_links_json: row.try_get("monitor_links_json")?,
            artifacts_json: row.try_get("artifacts_json")?,
            cleanup_json: row.try_get("cleanup_json")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            started_at_ms: row.try_get("started_at_ms")?,
            completed_at_ms: row.try_get("completed_at_ms")?,
        })
    }
}

impl TryFrom<WorkflowRunRow> for WorkflowRun {
    type Error = anyhow::Error;

    fn try_from(row: WorkflowRunRow) -> Result<Self> {
        Ok(Self {
            run_id: row.run_id,
            workflow_record_id: row.workflow_record_id,
            source_thread_id: row
                .source_thread_id
                .map(|thread_id| ThreadId::from_string(&thread_id))
                .transpose()?,
            idempotency_key: row.idempotency_key,
            spec_workflow_id: row.spec_workflow_id,
            schema_version: row.schema_version,
            source_yaml_sha256: row.source_yaml_sha256,
            status: WorkflowRunStatus::try_from(row.status.as_str())?,
            status_reason: row.status_reason,
            reason_code: row.reason_code,
            generation: row.generation,
            owner_id: row.owner_id,
            lease_expires_at: row
                .lease_expires_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            heartbeat_at: row
                .heartbeat_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            last_event_seq: row.last_event_seq,
            agents_json: parse_workflow_json(row.agents_json, "agents_json")?,
            execution_defaults_json: parse_workflow_json(
                row.execution_defaults_json,
                "execution_defaults_json",
            )?,
            limits_json: parse_workflow_json(row.limits_json, "limits_json")?,
            approvals_json: parse_workflow_json(row.approvals_json, "approvals_json")?,
            loops_json: row
                .loops_json
                .map(|value| parse_workflow_json(value, "loops_json"))
                .transpose()?,
            monitor_links_json: row
                .monitor_links_json
                .map(|value| parse_workflow_json(value, "monitor_links_json"))
                .transpose()?,
            artifacts_json: parse_workflow_json(row.artifacts_json, "artifacts_json")?,
            cleanup_json: parse_workflow_json(row.cleanup_json, "cleanup_json")?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            started_at: row
                .started_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            completed_at: row
                .completed_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
        })
    }
}

pub(crate) struct WorkflowRunStepRow {
    pub step_run_id: String,
    pub run_id: String,
    pub step_id: String,
    pub sequence: i64,
    pub title: String,
    pub agent_id: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub parallel_group: Option<String>,
    pub approval_gate: Option<String>,
    pub model_route_json: Option<String>,
    pub workspace_json: Option<String>,
    pub background_agent_run_id: Option<String>,
    pub branch_admission_json: Option<String>,
    pub completion_model_marked_state: Option<String>,
    pub attempt: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
}

impl WorkflowRunStepRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            step_run_id: row.try_get("step_run_id")?,
            run_id: row.try_get("run_id")?,
            step_id: row.try_get("step_id")?,
            sequence: row.try_get("sequence")?,
            title: row.try_get("title")?,
            agent_id: row.try_get("agent_id")?,
            status: row.try_get("status")?,
            status_reason: row.try_get("status_reason")?,
            reason_code: row.try_get("reason_code")?,
            parallel_group: row.try_get("parallel_group")?,
            approval_gate: row.try_get("approval_gate")?,
            model_route_json: row.try_get("model_route_json")?,
            workspace_json: row.try_get("workspace_json")?,
            background_agent_run_id: row.try_get("background_agent_run_id")?,
            branch_admission_json: row.try_get("branch_admission_json")?,
            completion_model_marked_state: row.try_get("completion_model_marked_state")?,
            attempt: row.try_get("attempt")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            started_at_ms: row.try_get("started_at_ms")?,
            completed_at_ms: row.try_get("completed_at_ms")?,
        })
    }
}

impl TryFrom<WorkflowRunStepRow> for WorkflowRunStep {
    type Error = anyhow::Error;

    fn try_from(row: WorkflowRunStepRow) -> Result<Self> {
        Ok(Self {
            step_run_id: row.step_run_id,
            run_id: row.run_id,
            step_id: row.step_id,
            sequence: row.sequence,
            title: row.title,
            agent_id: row.agent_id,
            status: WorkflowRunStepStatus::try_from(row.status.as_str())?,
            status_reason: row.status_reason,
            reason_code: row.reason_code,
            parallel_group: row.parallel_group,
            approval_gate: row.approval_gate,
            model_route_json: row
                .model_route_json
                .map(|value| parse_workflow_json(value, "model_route_json"))
                .transpose()?,
            workspace_json: row
                .workspace_json
                .map(|value| parse_workflow_json(value, "workspace_json"))
                .transpose()?,
            background_agent_run_id: row.background_agent_run_id,
            branch_admission_json: row
                .branch_admission_json
                .map(|value| parse_workflow_json(value, "branch_admission_json"))
                .transpose()?,
            completion_model_marked_state: row.completion_model_marked_state,
            attempt: row.attempt,
            depends_on: Vec::new(),
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            started_at: row
                .started_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            completed_at: row
                .completed_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
        })
    }
}

pub(crate) struct WorkflowRunStepVerifierRow {
    pub verifier_run_id: String,
    pub run_id: String,
    pub step_id: String,
    pub verifier_id: String,
    pub verifier_type: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub definition_json: String,
    pub last_result_json: Option<String>,
    pub attempt_count: i64,
    pub max_attempts: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

impl WorkflowRunStepVerifierRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            verifier_run_id: row.try_get("verifier_run_id")?,
            run_id: row.try_get("run_id")?,
            step_id: row.try_get("step_id")?,
            verifier_id: row.try_get("verifier_id")?,
            verifier_type: row.try_get("verifier_type")?,
            status: row.try_get("status")?,
            status_reason: row.try_get("status_reason")?,
            reason_code: row.try_get("reason_code")?,
            definition_json: row.try_get("definition_json")?,
            last_result_json: row.try_get("last_result_json")?,
            attempt_count: row.try_get("attempt_count")?,
            max_attempts: row.try_get("max_attempts")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            completed_at_ms: row.try_get("completed_at_ms")?,
        })
    }
}

impl TryFrom<WorkflowRunStepVerifierRow> for WorkflowRunStepVerifier {
    type Error = anyhow::Error;

    fn try_from(row: WorkflowRunStepVerifierRow) -> Result<Self> {
        Ok(Self {
            verifier_run_id: row.verifier_run_id,
            run_id: row.run_id,
            step_id: row.step_id,
            verifier_id: row.verifier_id,
            verifier_type: row.verifier_type,
            status: WorkflowRunStepVerifierStatus::try_from(row.status.as_str())?,
            status_reason: row.status_reason,
            reason_code: row.reason_code,
            definition_json: parse_workflow_json(row.definition_json, "definition_json")?,
            last_result_json: row
                .last_result_json
                .map(|value| parse_workflow_json(value, "last_result_json"))
                .transpose()?,
            attempt_count: row.attempt_count,
            max_attempts: row.max_attempts,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            completed_at: row
                .completed_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
        })
    }
}

pub(crate) struct WorkflowRunEventRow {
    pub event_id: String,
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub step_run_id: Option<String>,
    pub verifier_run_id: Option<String>,
    pub visibility: String,
    pub event_payload_json: String,
    pub created_at_ms: i64,
}

impl WorkflowRunEventRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            event_id: row.try_get("event_id")?,
            run_id: row.try_get("run_id")?,
            seq: row.try_get("seq")?,
            event_type: row.try_get("event_type")?,
            actor_kind: row.try_get("actor_kind")?,
            actor_id: row.try_get("actor_id")?,
            step_run_id: row.try_get("step_run_id")?,
            verifier_run_id: row.try_get("verifier_run_id")?,
            visibility: row.try_get("visibility")?,
            event_payload_json: row.try_get("event_payload_json")?,
            created_at_ms: row.try_get("created_at_ms")?,
        })
    }
}

impl TryFrom<WorkflowRunEventRow> for WorkflowRunEvent {
    type Error = anyhow::Error;

    fn try_from(row: WorkflowRunEventRow) -> Result<Self> {
        Ok(Self {
            event_id: row.event_id,
            run_id: row.run_id,
            seq: row.seq,
            event_type: row.event_type,
            actor_kind: row.actor_kind,
            actor_id: row.actor_id,
            step_run_id: row.step_run_id,
            verifier_run_id: row.verifier_run_id,
            visibility: row.visibility,
            event_payload_json: parse_workflow_json(row.event_payload_json, "event_payload_json")?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
        })
    }
}

fn parse_workflow_json(raw: String, field: &str) -> Result<Value> {
    serde_json::from_str(&raw).map_err(|err| anyhow!("invalid workflow JSON in {field}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn timestamp() -> DateTime<Utc> {
        chrono::Utc
            .timestamp_opt(1_800_000_000, 0)
            .single()
            .expect("timestamp should be valid")
    }

    /// Wrap an approvals payload in the redaction envelope the runtime stores,
    /// so tests exercise the real `approvals_json` shape.
    fn approvals_envelope(data: Value) -> Value {
        json!({
            "schemaVersion": "workflow.run_state/v0",
            "redactionVersion": 1,
            "kind": "workflow_run_approvals",
            "data": data,
        })
    }

    fn run_with_approvals(approvals_json: Value) -> WorkflowRun {
        WorkflowRun {
            run_id: "run-1".to_string(),
            workflow_record_id: "workflow-1".to_string(),
            source_thread_id: None,
            idempotency_key: None,
            spec_workflow_id: "wf_test".to_string(),
            schema_version: "1".to_string(),
            source_yaml_sha256: "sha".to_string(),
            status: WorkflowRunStatus::Running,
            status_reason: None,
            reason_code: None,
            generation: 1,
            owner_id: None,
            lease_expires_at: None,
            heartbeat_at: None,
            last_event_seq: 0,
            agents_json: json!([]),
            execution_defaults_json: json!({}),
            limits_json: json!({}),
            approvals_json,
            loops_json: None,
            monitor_links_json: None,
            artifacts_json: json!({}),
            cleanup_json: json!({}),
            created_at: timestamp(),
            updated_at: timestamp(),
            started_at: Some(timestamp()),
            completed_at: None,
        }
    }

    fn step(step_id: &str, status: WorkflowRunStepStatus, gate: Option<&str>) -> WorkflowRunStep {
        WorkflowRunStep {
            step_run_id: format!("{step_id}-run"),
            run_id: "run-1".to_string(),
            step_id: step_id.to_string(),
            sequence: 1,
            title: step_id.to_string(),
            agent_id: "agent".to_string(),
            status,
            status_reason: None,
            reason_code: None,
            parallel_group: None,
            approval_gate: gate.map(str::to_string),
            model_route_json: None,
            workspace_json: None,
            background_agent_run_id: None,
            branch_admission_json: None,
            completion_model_marked_state: None,
            attempt: 1,
            depends_on: Vec::new(),
            created_at: timestamp(),
            updated_at: timestamp(),
            started_at: None,
            completed_at: None,
        }
    }

    fn snapshot(run: WorkflowRun, steps: Vec<WorkflowRunStep>) -> WorkflowRunSnapshot {
        WorkflowRunSnapshot {
            run,
            steps,
            verifiers: Vec::new(),
            events: Vec::new(),
        }
    }

    #[test]
    fn approval_review_reports_pending_gates_and_config() {
        let snapshot = snapshot(
            run_with_approvals(approvals_envelope(json!({ "required_before": ["deploy"] }))),
            vec![
                step("collect", WorkflowRunStepStatus::Succeeded, None),
                step("deploy", WorkflowRunStepStatus::Ready, Some("deploy")),
                step("rollout", WorkflowRunStepStatus::Blocked, Some("prod")),
                // Already admitted/completed gated step is not pending.
                step("hotfix", WorkflowRunStepStatus::Succeeded, Some("deploy")),
            ],
        );

        let review = snapshot.approval_review();

        assert!(review.has_approval_config);
        assert!(review.is_available());
        assert!(review.is_actionable());
        assert_eq!(2, review.pending_count());
        assert_eq!(
            vec![
                WorkflowApprovalGate {
                    step_id: "deploy".to_string(),
                    gate: "deploy".to_string(),
                },
                WorkflowApprovalGate {
                    step_id: "rollout".to_string(),
                    gate: "prod".to_string(),
                },
            ],
            review.pending
        );
    }

    #[test]
    fn approval_review_without_config_or_gates_is_unavailable() {
        let snapshot = snapshot(
            run_with_approvals(approvals_envelope(json!({ "required_before": [] }))),
            vec![step("collect", WorkflowRunStepStatus::Ready, None)],
        );

        let review = snapshot.approval_review();

        assert!(!review.has_approval_config);
        assert!(!review.is_available());
        assert!(!review.is_actionable());
        assert_eq!(0, review.pending_count());
        assert!(review.pending.is_empty());
    }

    #[test]
    fn approval_review_surfaces_pending_gate_even_without_run_config() {
        // A gated step is actionable even if the run-level config is absent.
        let snapshot = snapshot(
            run_with_approvals(json!({})),
            vec![step(
                "deploy",
                WorkflowRunStepStatus::Pending,
                Some("deploy"),
            )],
        );

        let review = snapshot.approval_review();

        assert!(!review.has_approval_config);
        assert!(review.is_available());
        assert!(review.is_actionable());
        assert_eq!(1, review.pending_count());
    }
}
