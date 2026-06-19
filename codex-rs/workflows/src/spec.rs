use serde::Deserialize;
use serde::Serialize;
use serde_yaml::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpec {
    pub schema_version: String,
    pub workflow_id: String,
    pub display_name: String,
    pub source_prompt: String,
    pub status: WorkflowStatus,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub blocking_reasons: Vec<String>,
    #[serde(default)]
    pub fixture: Option<WorkflowFixture>,
    pub execution_defaults: WorkflowModelRoute,
    pub limits: WorkflowLimits,
    pub approvals: WorkflowApprovals,
    #[serde(default)]
    pub domain_invariants: Option<Value>,
    pub agents: Vec<WorkflowAgent>,
    pub steps: Vec<WorkflowStep>,
    #[serde(default)]
    pub loops: Vec<WorkflowLoop>,
    #[serde(default)]
    pub monitors: Vec<WorkflowMonitorLink>,
    pub artifacts: WorkflowArtifacts,
    pub cleanup: WorkflowCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Draft,
    NeedsClarification,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFixture {
    pub purpose: String,
    pub executable: bool,
    pub data: String,
    pub generated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowModelRoute {
    pub model_gateway: String,
    pub provider: String,
    pub model: String,
    pub reasoning: String,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<String>,
    #[serde(default)]
    pub permission_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLimits {
    pub max_parallel_steps: u32,
    pub max_agents: u32,
    pub max_worktrees: u32,
    pub max_runtime_seconds: u64,
    pub max_step_runtime_seconds: u64,
    pub max_tokens: u64,
    pub max_tool_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowApprovals {
    #[serde(default)]
    pub required_before: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgent {
    pub id: String,
    pub display_name: String,
    pub role: String,
    pub model: WorkflowModelRoute,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStep {
    pub id: String,
    pub title: String,
    pub agent: String,
    #[serde(default)]
    pub model: Option<WorkflowModelRoute>,
    #[serde(default)]
    pub workspace: Option<WorkflowWorkspace>,
    #[serde(default)]
    pub parallel_group: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub approval_gate: Option<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub completion: Option<WorkflowCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowWorkspace {
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLoop {
    pub id: String,
    pub title: String,
    pub schedule: WorkflowLoopSchedule,
    pub timezone: String,
    pub stop_condition: WorkflowStopCondition,
    pub max_iterations: u32,
    #[serde(default)]
    pub trigger_step: Option<String>,
    #[serde(default)]
    pub expires_after_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowLoopSchedule {
    Dynamic,
    Interval {
        amount: u32,
        unit: WorkflowLoopIntervalUnit,
    },
    Cron {
        expression: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowLoopIntervalUnit {
    Minutes,
    Hours,
    Days,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowStopCondition {
    WorkflowComplete,
    StepSucceeded { step: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowMonitorLink {
    pub id: String,
    pub title: String,
    pub source: String,
    pub max_events_per_tick: u32,
    #[serde(default)]
    pub monitor_ref: Option<String>,
    #[serde(default)]
    pub trigger_step: Option<String>,
    #[serde(default)]
    pub stop_condition: Option<WorkflowStopCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCompletion {
    pub model_marked_state: String,
    #[serde(default)]
    pub verifiers: Vec<WorkflowVerifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVerifier {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub artifact: Option<String>,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub sandbox: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub output_limit_bytes: Option<u64>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub expected_stdout: Option<String>,
    #[serde(default)]
    pub expected_exit_code: Option<i32>,
    #[serde(default)]
    pub retry_policy: Option<WorkflowVerifierRetryPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVerifierRetryPolicy {
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowArtifacts {
    pub retention: String,
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCleanup {
    #[serde(default)]
    pub on_cancel: Vec<String>,
    #[serde(default)]
    pub on_complete: Vec<String>,
}
