use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentRunStatus {
    Queued,
    Starting,
    Running,
    WaitingOnApproval,
    WaitingOnUser,
    Stopping,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentDesiredState {
    Running,
    Stopped,
    Deleted,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentRetentionState {
    Active,
    Archived,
    DeleteRequested,
    Deleted,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentPendingInteractionKind {
    Approval,
    UserInput,
    McpElicitation,
    PermissionGrant,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentPendingInteractionStatus {
    Pending,
    Delivered,
    Responded,
    Expired,
    Cancelled,
    Denied,
    WorkerNoLongerWaiting,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentPendingInteractionTerminalStatus {
    Responded,
    Expired,
    Cancelled,
    Denied,
    WorkerNoLongerWaiting,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AgentLifecycleEffect {
    ReplayState,
    RemoveSubscriberOnly,
    RequestWorkerStop,
    MarkDeleteRequested,
    KeepWorkerRunning,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentRun {
    pub agent_id: String,
    #[ts(type = "string | null")]
    pub idempotency_key: Option<String>,
    #[ts(type = "string | null")]
    pub request_id: Option<String>,
    pub source: String,
    pub prompt_snapshot_ref: String,
    #[ts(type = "string | null")]
    pub input_snapshot_ref: Option<String>,
    #[ts(type = "string | null")]
    pub thread_id: Option<String>,
    pub thread_store_kind: String,
    #[ts(type = "string | null")]
    pub thread_store_id: Option<String>,
    #[ts(type = "string | null")]
    pub rollout_path: Option<String>,
    #[ts(type = "string | null")]
    pub parent_thread_id: Option<String>,
    #[ts(type = "string | null")]
    pub parent_agent_run_id: Option<String>,
    pub spawn_linkage: Option<JsonValue>,
    #[ts(type = "string | null")]
    pub worktree_lease_id: Option<String>,
    #[ts(type = "string | null")]
    pub auth_profile_ref: Option<String>,
    pub desired_state: AgentDesiredState,
    pub status: AgentRunStatus,
    #[ts(type = "string | null")]
    pub status_reason: Option<String>,
    #[ts(type = "string | null")]
    pub config_fingerprint: Option<String>,
    #[ts(type = "string | null")]
    pub version_fingerprint: Option<String>,
    pub retention_state: AgentRetentionState,
    #[ts(type = "number | null")]
    pub archive_after: Option<i64>,
    #[ts(type = "number | null")]
    pub delete_after: Option<i64>,
    #[ts(type = "number | null")]
    pub archived_at: Option<i64>,
    #[ts(type = "number | null")]
    pub deleted_at: Option<i64>,
    #[ts(type = "string | null")]
    pub supervisor_id: Option<String>,
    #[ts(type = "number")]
    pub generation: i64,
    #[ts(type = "number | null")]
    pub pid: Option<i64>,
    #[ts(type = "number | null")]
    pub pgid: Option<i64>,
    #[ts(type = "string | null")]
    pub job_id: Option<String>,
    #[ts(type = "number | null")]
    pub heartbeat_at: Option<i64>,
    #[ts(type = "string | null")]
    pub crash_reason: Option<String>,
    #[ts(type = "number | null")]
    pub exit_code: Option<i64>,
    #[ts(type = "number | null")]
    pub exit_signal: Option<i64>,
    #[ts(type = "number")]
    pub last_event_seq: i64,
    #[ts(type = "number")]
    pub last_snapshot_seq: i64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
    #[ts(type = "number | null")]
    pub completed_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentEvent {
    pub event_id: String,
    pub agent_id: String,
    #[ts(type = "number")]
    pub seq: i64,
    pub event_type: String,
    pub payload: JsonValue,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentStatusSnapshot {
    pub agent_id: String,
    #[ts(type = "number")]
    pub seq: i64,
    pub status: AgentRunStatus,
    pub desired_state: AgentDesiredState,
    #[ts(type = "string | null")]
    pub summary: Option<String>,
    #[ts(type = "number")]
    pub pending_interaction_count: i64,
    #[ts(type = "number")]
    pub last_event_seq: i64,
    pub payload: JsonValue,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentExecutionSnapshot {
    pub snapshot_id: String,
    pub agent_id: String,
    #[ts(type = "number")]
    pub seq: i64,
    pub snapshot_kind: String,
    pub payload: JsonValue,
    pub recovery_policy: String,
    #[ts(type = "string | null")]
    pub config_fingerprint: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentExecutionContextParams {
    #[ts(optional = nullable)]
    pub workspace_roots: Option<Vec<String>>,
    #[ts(optional = nullable)]
    pub permission_profile: Option<JsonValue>,
    #[ts(optional = nullable)]
    pub sandbox_policy: Option<JsonValue>,
    #[ts(optional = nullable)]
    pub network_policy: Option<JsonValue>,
    #[ts(optional = nullable)]
    pub model: Option<String>,
    #[ts(optional = nullable)]
    pub provider: Option<String>,
    #[ts(optional = nullable)]
    pub service_tier: Option<String>,
    #[ts(optional = nullable)]
    pub mcp_tool_allowlist: Option<Vec<String>>,
    #[ts(optional = nullable)]
    pub env_snapshot_policy: Option<String>,
    #[ts(optional = nullable)]
    pub shell_snapshot: Option<JsonValue>,
    #[ts(optional = nullable)]
    pub config_source_hashes: Option<JsonValue>,
    #[ts(optional = nullable)]
    pub max_runtime_seconds: Option<u32>,
    #[ts(optional = nullable)]
    pub max_tokens: Option<u32>,
    #[ts(optional = nullable)]
    pub recovery_policy: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentPendingInteraction {
    pub interaction_id: String,
    pub agent_id: String,
    #[ts(type = "string | null")]
    pub worker_request_id: Option<String>,
    pub kind: AgentPendingInteractionKind,
    pub status: AgentPendingInteractionStatus,
    pub request_payload: JsonValue,
    pub response_payload: Option<JsonValue>,
    pub no_client_policy: String,
    #[ts(type = "number | null")]
    pub timeout_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub delivered_at: Option<i64>,
    #[ts(type = "number | null")]
    pub responded_at: Option<i64>,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentStartParams {
    pub prompt: String,
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    #[ts(optional = nullable)]
    pub idempotency_key: Option<String>,
    #[ts(optional = nullable)]
    pub request_id: Option<String>,
    #[ts(optional = nullable)]
    pub source: Option<String>,
    #[ts(optional = nullable)]
    pub prompt_snapshot_ref: Option<String>,
    #[ts(optional = nullable)]
    pub input_snapshot_ref: Option<String>,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub thread_store_kind: Option<String>,
    #[ts(optional = nullable)]
    pub thread_store_id: Option<String>,
    #[ts(optional = nullable)]
    pub rollout_path: Option<String>,
    #[ts(optional = nullable)]
    pub parent_thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub parent_agent_run_id: Option<String>,
    #[ts(optional = nullable)]
    pub spawn_linkage: Option<JsonValue>,
    #[ts(optional = nullable)]
    pub auth_profile_ref: Option<String>,
    #[ts(optional = nullable)]
    pub config_fingerprint: Option<String>,
    #[ts(optional = nullable)]
    pub version_fingerprint: Option<String>,
    #[ts(optional = nullable)]
    pub execution_context: Option<Box<AgentExecutionContextParams>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentStartResponse {
    pub agent: AgentRun,
    pub status_snapshot: AgentStatusSnapshot,
    pub execution_snapshot: AgentExecutionSnapshot,
    pub event: AgentEvent,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentListParams {
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentListResponse {
    pub data: Vec<AgentRun>,
    #[ts(type = "string | null")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentReadParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentReadResponse {
    pub agent: Option<AgentRun>,
    pub status_snapshot: Option<AgentStatusSnapshot>,
    pub execution_snapshot: Option<AgentExecutionSnapshot>,
    pub pending_interactions: Vec<AgentPendingInteraction>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentAttachParams {
    pub agent_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentAttachResponse {
    pub effect: AgentLifecycleEffect,
    pub agent: Option<AgentRun>,
    pub status_snapshot: Option<AgentStatusSnapshot>,
    pub execution_snapshot: Option<AgentExecutionSnapshot>,
    pub events: Vec<AgentEvent>,
    pub pending_interactions: Vec<AgentPendingInteraction>,
    #[ts(type = "string | null")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentDetachParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentDetachResponse {
    pub effect: AgentLifecycleEffect,
    pub agent: Option<AgentRun>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentStopParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentStopResponse {
    pub effect: AgentLifecycleEffect,
    pub agent: Option<AgentRun>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentDeleteParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentDeleteResponse {
    pub effect: AgentLifecycleEffect,
    pub agent: Option<AgentRun>,
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentEventsListParams {
    pub agent_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentEventsListResponse {
    pub data: Vec<AgentEvent>,
    #[ts(type = "string | null")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentPendingInteractionRespondParams {
    pub agent_id: String,
    pub interaction_id: String,
    pub response: JsonValue,
    pub terminal_status: AgentPendingInteractionTerminalStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentPendingInteractionRespondResponse {
    pub updated: bool,
    pub interaction: Option<AgentPendingInteraction>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentDaemonDiagnosticsParams {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentRunStatusCount {
    pub status: AgentRunStatus,
    #[ts(type = "number")]
    pub count: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentDaemonDiagnosticsResponse {
    pub state_store_available: bool,
    #[ts(type = "number")]
    pub active_run_count: i64,
    #[ts(type = "number")]
    pub queued_run_count: i64,
    #[ts(type = "number")]
    pub starting_run_count: i64,
    #[ts(type = "number")]
    pub running_run_count: i64,
    #[ts(type = "number")]
    pub waiting_run_count: i64,
    #[ts(type = "number")]
    pub stopping_run_count: i64,
    #[ts(type = "number")]
    pub pending_interaction_count: i64,
    pub runs_by_status: Vec<AgentRunStatusCount>,
    #[ts(type = "number")]
    pub max_active_runs_per_user: i64,
    #[ts(type = "number")]
    pub available_active_run_slots: i64,
    pub admission_allowed: bool,
    pub backpressure_reasons: Vec<String>,
    #[ts(type = "number")]
    pub max_list_limit: u32,
}
