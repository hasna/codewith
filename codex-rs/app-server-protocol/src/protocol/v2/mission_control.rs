use super::LocalSession;
use super::LocalSessionStatus;
use super::SortDirection;
use super::ThreadGoal;
use super::ThreadGoalPlan;
use super::ThreadListCwdFilter;
use super::ThreadMailboxMessageSummary;
use super::ThreadMailboxReceipt;
use super::ThreadPendingInteraction;
use super::ThreadPendingInteractionResponsePayload;
use super::ThreadPendingInteractionStatus;
use super::ThreadPendingInteractionTerminalStatus;
use super::ThreadSortKey;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlOverviewParams {
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    #[ts(optional = nullable)]
    pub sort_key: Option<ThreadSortKey>,
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
    #[ts(optional = nullable, type = "string | Array<string> | null")]
    pub cwd: Option<ThreadListCwdFilter>,
    #[ts(optional = nullable)]
    pub session_statuses: Option<Vec<LocalSessionStatus>>,
    #[ts(optional = nullable)]
    pub search_term: Option<String>,
    #[ts(optional = nullable)]
    pub pending_interaction_cursor: Option<String>,
    #[ts(optional = nullable)]
    pub pending_interaction_limit: Option<u32>,
    #[ts(optional = nullable)]
    pub pending_interaction_statuses: Option<Vec<ThreadPendingInteractionStatus>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_goal_plans: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_state_db_only: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlOverviewResponse {
    pub sessions: Vec<MissionControlSession>,
    pub pending_interactions: Vec<ThreadPendingInteraction>,
    pub next_session_cursor: Option<String>,
    pub next_pending_interaction_cursor: Option<String>,
    pub capabilities: MissionControlCapabilities,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlSession {
    pub session: LocalSession,
    pub goal: Option<ThreadGoal>,
    pub goal_plans: Vec<ThreadGoalPlan>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlCapabilities {
    pub local_sessions: bool,
    pub durable_mailbox: bool,
    pub pending_interactions: bool,
    pub goals: bool,
    pub remote_dispatch: bool,
    pub workflow_mutation: bool,
    pub shell_execution: bool,
    pub filesystem_mutation: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlEnqueueInstructionParams {
    pub target_thread_id: String,
    pub message: String,
    #[ts(optional = nullable)]
    pub sender_thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub sender_label: Option<String>,
    #[ts(optional = nullable)]
    pub idempotency_key: Option<String>,
    #[ts(optional = nullable)]
    pub priority: Option<i64>,
    #[ts(optional = nullable)]
    pub max_attempts: Option<u32>,
    #[ts(type = "number | null", optional = nullable)]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlEnqueueInstructionResponse {
    pub dry_run: bool,
    pub delivery_policy: MissionControlDeliveryPolicy,
    pub preview: String,
    pub message: Option<ThreadMailboxMessageSummary>,
    pub created: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MissionControlDeliveryPolicy {
    LiveOnly,
    ResumeAndTrigger,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlMailboxReceiptsParams {
    pub target_thread_id: String,
    pub message_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlMailboxReceiptsResponse {
    pub data: Vec<ThreadMailboxReceipt>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlRespondInteractionParams {
    pub interaction_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    pub terminal_status: ThreadPendingInteractionTerminalStatus,
    pub response: ThreadPendingInteractionResponsePayload,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MissionControlRespondInteractionResponse {
    pub dry_run: bool,
    pub updated: bool,
    pub interaction: Option<ThreadPendingInteraction>,
}
