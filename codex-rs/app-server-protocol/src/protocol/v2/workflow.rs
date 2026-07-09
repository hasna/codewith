use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ThreadGoalPlan;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadWorkflowStatus {
    Draft,
    NeedsClarification,
    Blocked,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflow {
    pub thread_id: String,
    pub workflow_record_id: String,
    pub spec_workflow_id: String,
    pub schema_version: String,
    pub display_name: String,
    pub status: ThreadWorkflowStatus,
    pub source_yaml_sha256: String,
    #[ts(type = "number")]
    pub agent_count: i64,
    #[ts(type = "number")]
    pub step_count: i64,
    #[ts(type = "number")]
    pub parallel_group_count: i64,
    #[ts(type = "number")]
    pub verifier_count: i64,
    #[ts(type = "number")]
    pub run_command_verifier_count: i64,
    #[ts(type = "number")]
    pub model_routed_step_count: i64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowCreateParams {
    pub thread_id: String,
    pub yaml: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowCreateResponse {
    pub workflow: ThreadWorkflow,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowGetParams {
    pub thread_id: String,
    pub workflow_record_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowGetResponse {
    pub workflow: Option<ThreadWorkflow>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowListResponse {
    pub data: Vec<ThreadWorkflow>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadWorkflowRunStatus {
    Pending,
    Running,
    Waiting,
    Blocked,
    Paused,
    CancelRequested,
    Cancelled,
    Failed,
    Completed,
    Other,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadWorkflowRunStepStatus {
    Pending,
    Ready,
    Active,
    WaitingVerifier,
    Blocked,
    Skipped,
    Cancelled,
    Failed,
    Succeeded,
    Other,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadWorkflowRunStepVerifierStatus {
    Pending,
    Running,
    Blocked,
    Passed,
    Failed,
    Skipped,
    Other,
}

/// A workflow step currently gated behind a pending approval review.
///
/// Only carries the author-defined step id and approval-gate label so a
/// manager can display or link to the approval-review flow. Never contains a
/// raw approvals payload, prompt, or secret.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowApprovalGate {
    pub step_id: String,
    pub gate: String,
}

/// Workflow-scoped approval-review context for a run.
///
/// Lets `/workflows` decide whether to enable its approval affordance and, if
/// so, surface the pending gates. The manager should keep approval rows
/// disabled unless `available` is true and keep the approval action disabled
/// unless `actionable` is true.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowApprovalReview {
    /// The run declares run-level approval gates (`approvals.required_before`).
    pub has_approval_config: bool,
    /// Any workflow-scoped approval data is available (config or a pending gate).
    pub available: bool,
    /// There is a pending approval to act on.
    pub actionable: bool,
    #[ts(type = "number")]
    pub pending_count: i64,
    /// Compact, sanitized list of steps awaiting an approval review.
    pub pending_gates: Vec<ThreadWorkflowApprovalGate>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRun {
    pub thread_id: Option<String>,
    pub run_id: String,
    pub workflow_record_id: String,
    pub spec_workflow_id: String,
    pub schema_version: String,
    pub source_yaml_sha256: String,
    pub status: ThreadWorkflowRunStatus,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    #[ts(type = "number")]
    pub generation: i64,
    #[ts(type = "number")]
    pub pending_step_count: i64,
    #[ts(type = "number")]
    pub ready_step_count: i64,
    #[ts(type = "number")]
    pub active_step_count: i64,
    #[ts(type = "number")]
    pub waiting_verifier_step_count: i64,
    #[ts(type = "number")]
    pub blocked_step_count: i64,
    #[ts(type = "number")]
    pub failed_step_count: i64,
    #[ts(type = "number")]
    pub succeeded_step_count: i64,
    #[ts(type = "number")]
    pub skipped_step_count: i64,
    #[ts(type = "number")]
    pub verifier_count: i64,
    #[ts(type = "number")]
    pub event_count: i64,
    /// Workflow-scoped approval-review context for this run.
    pub approval_review: ThreadWorkflowApprovalReview,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
    #[ts(type = "number | null")]
    pub completed_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunStep {
    pub step_run_id: String,
    pub step_id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub title: String,
    pub agent_id: String,
    pub status: ThreadWorkflowRunStepStatus,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    pub depends_on: Vec<String>,
    pub background_agent_run_id: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
    #[ts(type = "number | null")]
    pub completed_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunStepVerifier {
    pub verifier_run_id: String,
    pub step_id: String,
    pub verifier_id: String,
    pub verifier_type: String,
    pub status: ThreadWorkflowRunStepVerifierStatus,
    pub status_reason: Option<String>,
    pub reason_code: Option<String>,
    #[ts(type = "number")]
    pub attempt_count: i64,
    #[ts(type = "number | null")]
    pub max_attempts: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number | null")]
    pub completed_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunEvent {
    #[ts(type = "number")]
    pub seq: i64,
    pub event_type: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub step_run_id: Option<String>,
    pub verifier_run_id: Option<String>,
    pub visibility: String,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunSnapshot {
    pub run: ThreadWorkflowRun,
    pub steps: Vec<ThreadWorkflowRunStep>,
    pub verifiers: Vec<ThreadWorkflowRunStepVerifier>,
    pub events: Vec<ThreadWorkflowRunEvent>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunListResponse {
    pub data: Vec<ThreadWorkflowRun>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunGetParams {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunGetResponse {
    pub run: Option<ThreadWorkflowRunSnapshot>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunStartParams {
    pub thread_id: String,
    pub workflow_record_id: String,
    #[ts(optional = nullable)]
    pub idempotency_key: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunStartResponse {
    pub run: ThreadWorkflowRunSnapshot,
    pub goal_plan: Option<ThreadGoalPlan>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunPauseParams {
    pub thread_id: String,
    pub run_id: String,
    #[ts(optional = nullable)]
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunPauseResponse {
    pub run: Option<ThreadWorkflowRunSnapshot>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunResumeParams {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunResumeResponse {
    pub run: Option<ThreadWorkflowRunSnapshot>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunCancelParams {
    pub thread_id: String,
    pub run_id: String,
    #[ts(optional = nullable)]
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadWorkflowRunCancelResponse {
    pub run: Option<ThreadWorkflowRunSnapshot>,
}
