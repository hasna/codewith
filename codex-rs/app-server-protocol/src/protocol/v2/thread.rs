use super::ActivePermissionProfile;
use super::ApprovalsReviewer;
use super::AskForApproval;
use super::SandboxMode;
use super::SandboxPolicy;
use super::Thread;
use super::ThreadItem;
use super::ThreadSource;
use super::Turn;
use super::TurnEnvironmentParams;
use super::TurnItemsView;
use super::shared::v2_enum_from_core;
use codex_experimental_api_macros::ExperimentalApi;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::ThreadGoalStatus as CoreThreadGoalStatus;
use codex_protocol::protocol::TokenUsage as CoreTokenUsage;
use codex_protocol::protocol::TokenUsageInfo as CoreTokenUsageInfo;
use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadStartSource {
    Startup,
    Clear,
}

#[derive(Serialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct DynamicToolSpec {
    #[ts(optional)]
    pub namespace: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub defer_loading: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DynamicToolSpecDe {
    namespace: Option<String>,
    name: String,
    description: String,
    input_schema: JsonValue,
    defer_loading: Option<bool>,
    expose_to_context: Option<bool>,
}

impl<'de> Deserialize<'de> for DynamicToolSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let DynamicToolSpecDe {
            namespace,
            name,
            description,
            input_schema,
            defer_loading,
            expose_to_context,
        } = DynamicToolSpecDe::deserialize(deserializer)?;

        Ok(Self {
            namespace,
            name,
            description,
            input_schema,
            defer_loading: defer_loading
                .unwrap_or_else(|| expose_to_context.map(|visible| !visible).unwrap_or(false)),
        })
    }
}

// === Threads, Turns, and Items ===
// Thread APIs
#[derive(
    Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS, ExperimentalApi,
)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadStartParams {
    #[ts(optional = nullable)]
    pub model: Option<String>,
    #[ts(optional = nullable)]
    pub model_provider: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub service_tier: Option<Option<String>>,
    /// Auth profile selected for model requests on this thread. `null` selects
    /// the default root auth; omission leaves config/env selection unchanged.
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub auth_profile: Option<Option<String>>,
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    /// Replace the thread's runtime workspace roots. Paths must be absolute.
    #[experimental("thread/start.runtimeWorkspaceRoots")]
    #[ts(optional = nullable)]
    pub runtime_workspace_roots: Option<Vec<AbsolutePathBuf>>,
    #[experimental(nested)]
    #[ts(optional = nullable)]
    pub approval_policy: Option<AskForApproval>,
    /// Override where approval requests are routed for review on this thread
    /// and subsequent turns.
    #[ts(optional = nullable)]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[ts(optional = nullable)]
    pub sandbox: Option<SandboxMode>,
    /// Named profile id for this thread. Cannot be combined with `sandbox`.
    #[experimental("thread/start.permissions")]
    #[ts(optional = nullable)]
    pub permissions: Option<String>,
    #[ts(optional = nullable)]
    pub config: Option<HashMap<String, JsonValue>>,
    #[ts(optional = nullable)]
    pub service_name: Option<String>,
    #[ts(optional = nullable)]
    pub base_instructions: Option<String>,
    #[ts(optional = nullable)]
    pub developer_instructions: Option<String>,
    #[ts(optional = nullable)]
    pub personality: Option<Personality>,
    #[ts(optional = nullable)]
    pub ephemeral: Option<bool>,
    #[ts(optional = nullable)]
    pub session_start_source: Option<ThreadStartSource>,
    /// Optional client-supplied analytics source classification for this thread.
    #[ts(optional = nullable)]
    pub thread_source: Option<ThreadSource>,
    /// Optional parent thread id when starting a fresh subagent child thread.
    #[ts(optional = nullable)]
    pub parent_thread_id: Option<String>,
    /// Optional sticky environments for this thread.
    ///
    /// Omitted selects the default environment when environment access is
    /// enabled. Empty disables environment access for turns that do not
    /// provide a turn override. Non-empty selects the first environment as the
    /// current turn environment.
    #[experimental("thread/start.environments")]
    #[ts(optional = nullable)]
    pub environments: Option<Vec<TurnEnvironmentParams>>,
    #[experimental("thread/start.dynamicTools")]
    #[ts(optional = nullable)]
    pub dynamic_tools: Option<Vec<DynamicToolSpec>>,
    /// Test-only experimental field used to validate experimental gating and
    /// schema filtering behavior in a stable way.
    #[experimental("thread/start.mockExperimentalField")]
    #[ts(optional = nullable)]
    pub mock_experimental_field: Option<String>,
    /// If true, opt into emitting raw Responses API items on the event stream.
    /// This is for internal use only (e.g. Codewith Cloud).
    #[experimental("thread/start.experimentalRawEvents")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub experimental_raw_events: bool,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MockExperimentalMethodParams {
    /// Test-only payload field.
    #[ts(optional = nullable)]
    pub value: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MockExperimentalMethodResponse {
    /// Echoes the input `value`.
    pub echoed: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS, ExperimentalApi)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadStartResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    pub service_tier: Option<String>,
    pub cwd: AbsolutePathBuf,
    /// Thread-scoped runtime workspace roots used to materialize
    /// `:workspace_roots`.
    #[experimental("thread/start.runtimeWorkspaceRoots")]
    #[serde(default)]
    pub runtime_workspace_roots: Vec<AbsolutePathBuf>,
    /// Profile-defined workspace roots that are active for this thread in
    /// addition to `runtimeWorkspaceRoots`.
    #[experimental("thread/start.profileWorkspaceRoots")]
    #[serde(default)]
    pub profile_workspace_roots: Vec<AbsolutePathBuf>,
    /// Instruction source files currently loaded for this thread.
    #[serde(default)]
    pub instruction_sources: Vec<AbsolutePathBuf>,
    #[experimental(nested)]
    pub approval_policy: AskForApproval,
    /// Reviewer currently used for approval requests on this thread.
    pub approvals_reviewer: ApprovalsReviewer,
    /// Legacy sandbox policy retained for compatibility. Experimental clients
    /// should prefer `activePermissionProfile` for profile provenance.
    pub sandbox: SandboxPolicy,
    /// Named or implicit built-in profile that produced the active
    /// permissions, when known.
    #[experimental("thread/start.activePermissionProfile")]
    #[serde(default)]
    pub active_permission_profile: Option<ActivePermissionProfile>,
    /// Auth profile selected for model requests on this thread, if any.
    #[serde(default)]
    pub auth_profile: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(
    Serialize, Deserialize, Debug, Default, Clone, PartialEq, JsonSchema, TS, ExperimentalApi,
)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSettingsUpdateParams {
    pub thread_id: String,
    /// Override the working directory for subsequent turns.
    #[ts(optional = nullable)]
    pub cwd: Option<PathBuf>,
    /// Override the approval policy for subsequent turns.
    #[experimental(nested)]
    #[ts(optional = nullable)]
    pub approval_policy: Option<AskForApproval>,
    /// Override where approval requests are routed for subsequent turns.
    #[ts(optional = nullable)]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    /// Override the sandbox policy for subsequent turns.
    #[ts(optional = nullable)]
    pub sandbox_policy: Option<SandboxPolicy>,
    /// Select a named permissions profile id for subsequent turns. Cannot be
    /// combined with `sandboxPolicy`.
    #[experimental("thread/settings/update.permissions")]
    #[ts(optional = nullable)]
    pub permissions: Option<String>,
    /// Override the auth profile for subsequent turns. `null` selects the
    /// default root auth; omission leaves it unchanged.
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub auth_profile: Option<Option<String>>,
    /// Override the model for subsequent turns.
    #[ts(optional = nullable)]
    pub model: Option<String>,
    /// Override the model provider for subsequent turns.
    #[ts(optional = nullable)]
    pub model_provider: Option<String>,
    /// Override the service tier for subsequent turns. `null` clears the
    /// current service tier; omission leaves it unchanged.
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub service_tier: Option<Option<String>>,
    /// Override the reasoning effort for subsequent turns.
    #[ts(optional = nullable)]
    pub effort: Option<ReasoningEffort>,
    /// Override the reasoning summary for subsequent turns.
    #[ts(optional = nullable)]
    pub summary: Option<ReasoningSummary>,
    /// EXPERIMENTAL - Set a pre-set collaboration mode for subsequent turns.
    ///
    /// For `collaboration_mode.settings.developer_instructions`, `null` means
    /// "use the built-in instructions for the selected mode".
    #[experimental("thread/settings/update.collaborationMode")]
    #[ts(optional = nullable)]
    pub collaboration_mode: Option<CollaborationMode>,
    /// Override the personality for subsequent turns.
    #[ts(optional = nullable)]
    pub personality: Option<Personality>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSettingsUpdateResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSettings {
    pub cwd: AbsolutePathBuf,
    pub approval_policy: AskForApproval,
    pub approvals_reviewer: ApprovalsReviewer,
    pub sandbox_policy: SandboxPolicy,
    pub active_permission_profile: Option<ActivePermissionProfile>,
    pub auth_profile: Option<String>,
    pub model: String,
    pub model_provider: String,
    pub service_tier: Option<String>,
    pub effort: Option<ReasoningEffort>,
    pub summary: Option<ReasoningSummary>,
    pub collaboration_mode: CollaborationMode,
    pub personality: Option<Personality>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSettingsUpdatedNotification {
    pub thread_id: String,
    pub thread_settings: ThreadSettings,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageListParams {
    pub thread_id: String,
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to no limit.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageListResponse {
    pub data: Vec<ThreadQueuedMessage>,
    pub next_cursor: Option<String>,
    pub stats: ThreadQueuedMessageStats,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessage {
    pub message_id: String,
    pub thread_id: String,
    /// 1-based position in the pending agent-message mailbox.
    pub position: u32,
    pub author: String,
    pub recipient: String,
    pub text: String,
    pub trigger_turn: bool,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageStats {
    pub total: u32,
    pub trigger_turn: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageUpdateParams {
    pub thread_id: String,
    pub message_id: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageUpdateResponse {
    pub message: Option<ThreadQueuedMessage>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadQueuedMessageMoveDirection {
    Up,
    Down,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageMoveParams {
    pub thread_id: String,
    pub message_id: String,
    pub direction: ThreadQueuedMessageMoveDirection,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadQueuedMessageMoveResponse {
    pub moved: bool,
    pub message: Option<ThreadQueuedMessage>,
}

#[derive(
    Serialize, Deserialize, Debug, Default, Clone, PartialEq, JsonSchema, TS, ExperimentalApi,
)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
/// There are three ways to resume a thread:
/// 1. By thread_id: load the thread from disk by thread_id and resume it.
/// 2. By history: instantiate the thread from memory and resume it.
/// 3. By path: load the thread from disk by path and resume it.
///
/// For non-running threads, the precedence is: history > non-empty path > thread_id.
/// If using history or a non-empty path for a non-running thread, the thread_id
/// param will be ignored.
///
/// If thread_id identifies a running thread, app-server rejoins that thread and
/// treats a non-empty path as a consistency check against the active rollout path.
/// Empty string path values are treated as absent.
///
/// Prefer using thread_id whenever possible.
pub struct ThreadResumeParams {
    pub thread_id: String,

    /// [UNSTABLE] FOR CODEX CLOUD - DO NOT USE.
    /// If specified, the thread will be resumed with the provided history
    /// instead of loaded from disk.
    #[experimental("thread/resume.history")]
    #[ts(optional = nullable)]
    pub history: Option<Vec<ResponseItem>>,

    /// [UNSTABLE] Specify the rollout path to resume from.
    /// If specified for a non-running thread, the thread_id param will be ignored.
    /// If thread_id identifies a running thread, the path must match the active
    /// rollout path.
    #[experimental("thread/resume.path")]
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_empty_path_as_none"
    )]
    #[ts(optional = nullable)]
    pub path: Option<PathBuf>,

    /// Configuration overrides for the resumed thread, if any.
    #[ts(optional = nullable)]
    pub model: Option<String>,
    #[ts(optional = nullable)]
    pub model_provider: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub service_tier: Option<Option<String>>,
    /// Auth profile selected for model requests on this thread. `null` selects
    /// the default root auth; omission leaves config/env selection unchanged.
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub auth_profile: Option<Option<String>>,
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    /// Replace the thread's runtime workspace roots. Paths must be absolute.
    #[experimental("thread/resume.runtimeWorkspaceRoots")]
    #[ts(optional = nullable)]
    pub runtime_workspace_roots: Option<Vec<AbsolutePathBuf>>,
    #[experimental(nested)]
    #[ts(optional = nullable)]
    pub approval_policy: Option<AskForApproval>,
    /// Override where approval requests are routed for review on this thread
    /// and subsequent turns.
    #[ts(optional = nullable)]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[ts(optional = nullable)]
    pub sandbox: Option<SandboxMode>,
    /// Named profile id for the resumed thread. Cannot be combined with
    /// `sandbox`.
    #[experimental("thread/resume.permissions")]
    #[ts(optional = nullable)]
    pub permissions: Option<String>,
    #[ts(optional = nullable)]
    pub config: Option<HashMap<String, serde_json::Value>>,
    #[ts(optional = nullable)]
    pub base_instructions: Option<String>,
    #[ts(optional = nullable)]
    pub developer_instructions: Option<String>,
    #[ts(optional = nullable)]
    pub personality: Option<Personality>,
    /// When true, return only thread metadata and live-resume state without
    /// populating `thread.turns`. This is useful when the client plans to call
    /// `thread/turns/list` immediately after resuming.
    #[experimental("thread/resume.excludeTurns")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_turns: bool,
    /// When present, include a `thread/turns/list` page in the resume response
    /// so clients can bootstrap recent turns without a second request.
    #[experimental("thread/resume.initialTurnsPage")]
    #[ts(optional = nullable)]
    pub initial_turns_page: Option<ThreadResumeInitialTurnsPageParams>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS, ExperimentalApi)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadResumeResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    pub service_tier: Option<String>,
    pub cwd: AbsolutePathBuf,
    /// Thread-scoped runtime workspace roots used to materialize
    /// `:workspace_roots`.
    #[experimental("thread/resume.runtimeWorkspaceRoots")]
    #[serde(default)]
    pub runtime_workspace_roots: Vec<AbsolutePathBuf>,
    /// Profile-defined workspace roots that are active for this thread in
    /// addition to `runtimeWorkspaceRoots`.
    #[experimental("thread/resume.profileWorkspaceRoots")]
    #[serde(default)]
    pub profile_workspace_roots: Vec<AbsolutePathBuf>,
    /// Instruction source files currently loaded for this thread.
    #[serde(default)]
    pub instruction_sources: Vec<AbsolutePathBuf>,
    #[experimental(nested)]
    pub approval_policy: AskForApproval,
    /// Reviewer currently used for approval requests on this thread.
    pub approvals_reviewer: ApprovalsReviewer,
    /// Legacy sandbox policy retained for compatibility. Experimental clients
    /// should prefer `activePermissionProfile` for profile provenance.
    pub sandbox: SandboxPolicy,
    /// Named or implicit built-in profile that produced the active
    /// permissions, when known.
    #[experimental("thread/resume.activePermissionProfile")]
    #[serde(default)]
    pub active_permission_profile: Option<ActivePermissionProfile>,
    /// Auth profile selected for model requests on this thread, if any.
    #[serde(default)]
    pub auth_profile: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// `thread/turns/list` page returned when requested by `initialTurnsPage`.
    #[experimental("thread/resume.initialTurnsPage")]
    #[serde(default)]
    pub initial_turns_page: Option<TurnsPage>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadResumeInitialTurnsPageParams {
    /// Optional turn page size.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Optional turn pagination direction; defaults to descending.
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
    /// How much item detail to include for each returned turn; defaults to summary.
    #[ts(optional = nullable)]
    pub items_view: Option<TurnItemsView>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnsPage {
    pub data: Vec<Turn>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

impl From<ThreadTurnsListResponse> for TurnsPage {
    fn from(response: ThreadTurnsListResponse) -> Self {
        Self {
            data: response.data,
            next_cursor: response.next_cursor,
            backwards_cursor: response.backwards_cursor,
        }
    }
}

#[derive(
    Serialize, Deserialize, Debug, Default, Clone, PartialEq, JsonSchema, TS, ExperimentalApi,
)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
/// There are two ways to fork a thread:
/// 1. By thread_id: load the thread from disk by thread_id and fork it into a new thread.
/// 2. By path: load the thread from disk by path and fork it into a new thread.
///
/// If using a non-empty path, the thread_id param will be ignored.
/// Empty string path values are treated as absent.
///
/// Prefer using thread_id whenever possible.
pub struct ThreadForkParams {
    pub thread_id: String,

    /// [UNSTABLE] Specify the rollout path to fork from.
    /// If specified, the thread_id param will be ignored.
    #[experimental("thread/fork.path")]
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_empty_path_as_none"
    )]
    #[ts(optional = nullable)]
    pub path: Option<PathBuf>,

    /// Configuration overrides for the forked thread, if any.
    #[ts(optional = nullable)]
    pub model: Option<String>,
    #[ts(optional = nullable)]
    pub model_provider: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub service_tier: Option<Option<String>>,
    /// Auth profile selected for model requests on this thread. `null` selects
    /// the default root auth; omission leaves config/env selection unchanged.
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable)]
    pub auth_profile: Option<Option<String>>,
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    /// Replace the thread's runtime workspace roots. Paths must be absolute.
    #[experimental("thread/fork.runtimeWorkspaceRoots")]
    #[ts(optional = nullable)]
    pub runtime_workspace_roots: Option<Vec<AbsolutePathBuf>>,
    #[experimental(nested)]
    #[ts(optional = nullable)]
    pub approval_policy: Option<AskForApproval>,
    /// Override where approval requests are routed for review on this thread
    /// and subsequent turns.
    #[ts(optional = nullable)]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[ts(optional = nullable)]
    pub sandbox: Option<SandboxMode>,
    /// Named profile id for the forked thread. Cannot be combined with
    /// `sandbox`.
    #[experimental("thread/fork.permissions")]
    #[ts(optional = nullable)]
    pub permissions: Option<String>,
    #[ts(optional = nullable)]
    pub config: Option<HashMap<String, serde_json::Value>>,
    #[ts(optional = nullable)]
    pub base_instructions: Option<String>,
    #[ts(optional = nullable)]
    pub developer_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    /// Optional client-supplied analytics source classification for this forked thread.
    #[ts(optional = nullable)]
    pub thread_source: Option<ThreadSource>,
    /// When true, return only thread metadata and live fork state without
    /// populating `thread.turns`. This is useful when the client plans to call
    /// `thread/turns/list` immediately after forking.
    #[experimental("thread/fork.excludeTurns")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_turns: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS, ExperimentalApi)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadForkResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    pub service_tier: Option<String>,
    pub cwd: AbsolutePathBuf,
    /// Thread-scoped runtime workspace roots used to materialize
    /// `:workspace_roots`.
    #[experimental("thread/fork.runtimeWorkspaceRoots")]
    #[serde(default)]
    pub runtime_workspace_roots: Vec<AbsolutePathBuf>,
    /// Profile-defined workspace roots that are active for this thread in
    /// addition to `runtimeWorkspaceRoots`.
    #[experimental("thread/fork.profileWorkspaceRoots")]
    #[serde(default)]
    pub profile_workspace_roots: Vec<AbsolutePathBuf>,
    /// Instruction source files currently loaded for this thread.
    #[serde(default)]
    pub instruction_sources: Vec<AbsolutePathBuf>,
    #[experimental(nested)]
    pub approval_policy: AskForApproval,
    /// Reviewer currently used for approval requests on this thread.
    pub approvals_reviewer: ApprovalsReviewer,
    /// Legacy sandbox policy retained for compatibility. Experimental clients
    /// should prefer `activePermissionProfile` for profile provenance.
    pub sandbox: SandboxPolicy,
    /// Named or implicit built-in profile that produced the active
    /// permissions, when known.
    #[experimental("thread/fork.activePermissionProfile")]
    #[serde(default)]
    pub active_permission_profile: Option<ActivePermissionProfile>,
    /// Auth profile selected for model requests on this thread, if any.
    #[serde(default)]
    pub auth_profile: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadArchiveParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadArchiveResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUnsubscribeParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUnsubscribeResponse {
    pub status: ThreadUnsubscribeStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum ThreadUnsubscribeStatus {
    NotLoaded,
    NotSubscribed,
    Unsubscribed,
}

/// Parameters for `thread/increment_elicitation`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadIncrementElicitationParams {
    /// Thread whose out-of-band elicitation counter should be incremented.
    pub thread_id: String,
}

/// Response for `thread/increment_elicitation`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadIncrementElicitationResponse {
    /// Current out-of-band elicitation count after the increment.
    pub count: u64,
    /// Whether timeout accounting is paused after applying the increment.
    pub paused: bool,
}

/// Parameters for `thread/decrement_elicitation`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadDecrementElicitationParams {
    /// Thread whose out-of-band elicitation counter should be decremented.
    pub thread_id: String,
}

/// Response for `thread/decrement_elicitation`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadDecrementElicitationResponse {
    /// Current out-of-band elicitation count after the decrement.
    pub count: u64,
    /// Whether timeout accounting remains paused after applying the decrement.
    pub paused: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSetNameParams {
    pub thread_id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUnarchiveParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSetNameResponse {}

v2_enum_from_core! {
    pub enum ThreadGoalStatus from CoreThreadGoalStatus {
        Active,
        Paused,
        Blocked,
        UsageLimited,
        BudgetLimited,
        Complete,
        Cancelled,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoal {
    pub thread_id: String,
    pub goal_id: String,
    pub objective: String,
    #[ts(type = "string | null")]
    pub title: Option<String>,
    pub status: ThreadGoalStatus,
    #[ts(type = "number | null")]
    pub token_budget: Option<i64>,
    #[ts(type = "number")]
    pub tokens_used: i64,
    #[ts(type = "number")]
    pub time_used_seconds: i64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

impl From<codex_protocol::protocol::ThreadGoal> for ThreadGoal {
    fn from(value: codex_protocol::protocol::ThreadGoal) -> Self {
        Self {
            thread_id: value.thread_id.to_string(),
            goal_id: value.goal_id,
            objective: value.objective,
            title: value.title,
            status: value.status.into(),
            token_budget: value.token_budget,
            tokens_used: value.tokens_used,
            time_used_seconds: value.time_used_seconds,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalSetParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub objective: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable, type = "string | null")]
    pub title: Option<Option<String>>,
    #[ts(optional = nullable)]
    pub status: Option<ThreadGoalStatus>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable, type = "number | null")]
    pub token_budget: Option<Option<i64>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalSetResponse {
    pub goal: ThreadGoal,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalGetParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalGetResponse {
    pub goal: Option<ThreadGoal>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadGoalPlanStatus {
    Active,
    Paused,
    Blocked,
    BudgetLimited,
    Complete,
    Cancelled,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadGoalPlanAutoExecute {
    Off,
    ReadyOnly,
    AiDirected,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadGoalPlanNodeStatus {
    Pending,
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
    Cancelled,
}

#[derive(Serialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalPlanNode {
    pub node_id: String,
    pub plan_id: String,
    pub thread_id: String,
    pub assigned_thread_id: String,
    pub key: String,
    #[ts(type = "number")]
    pub sequence: i64,
    #[ts(type = "number")]
    pub priority: i64,
    pub objective: String,
    #[ts(type = "string | null")]
    pub title: Option<String>,
    pub status: ThreadGoalPlanNodeStatus,
    pub ready: bool,
    #[ts(type = "number | null")]
    pub token_budget: Option<i64>,
    #[ts(type = "number")]
    pub tokens_used: i64,
    #[ts(type = "number")]
    pub time_used_seconds: i64,
    #[ts(type = "string | null")]
    pub projected_goal_id: Option<String>,
    pub depends_on: Vec<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

impl<'de> Deserialize<'de> for ThreadGoalPlanNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct WireThreadGoalPlanNode {
            node_id: String,
            plan_id: String,
            thread_id: String,
            #[serde(default)]
            assigned_thread_id: Option<String>,
            key: String,
            sequence: i64,
            priority: i64,
            objective: String,
            #[serde(default)]
            title: Option<String>,
            status: ThreadGoalPlanNodeStatus,
            ready: bool,
            token_budget: Option<i64>,
            tokens_used: i64,
            time_used_seconds: i64,
            projected_goal_id: Option<String>,
            depends_on: Vec<String>,
            created_at: i64,
            updated_at: i64,
        }

        let value = WireThreadGoalPlanNode::deserialize(deserializer)?;
        Ok(Self {
            node_id: value.node_id,
            plan_id: value.plan_id,
            assigned_thread_id: value
                .assigned_thread_id
                .unwrap_or_else(|| value.thread_id.clone()),
            thread_id: value.thread_id,
            key: value.key,
            sequence: value.sequence,
            priority: value.priority,
            objective: value.objective,
            title: value.title,
            status: value.status,
            ready: value.ready,
            token_budget: value.token_budget,
            tokens_used: value.tokens_used,
            time_used_seconds: value.time_used_seconds,
            projected_goal_id: value.projected_goal_id,
            depends_on: value.depends_on,
            created_at: value.created_at,
            updated_at: value.updated_at,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalPlan {
    pub plan_id: String,
    pub thread_id: String,
    pub status: ThreadGoalPlanStatus,
    pub auto_execute: ThreadGoalPlanAutoExecute,
    #[ts(type = "number | null")]
    pub max_tokens: Option<i64>,
    #[ts(type = "number")]
    pub total_tokens_used: i64,
    #[ts(type = "number")]
    pub total_time_used_seconds: i64,
    #[ts(type = "number | null")]
    pub remaining_tokens: Option<i64>,
    #[ts(type = "number")]
    pub node_count: i64,
    #[ts(type = "number")]
    pub completed_node_count: i64,
    #[ts(type = "number")]
    pub ready_node_count: i64,
    #[ts(type = "number")]
    pub active_node_count: i64,
    #[ts(type = "number")]
    pub pending_node_count: i64,
    #[ts(type = "number")]
    pub paused_node_count: i64,
    #[ts(type = "number")]
    pub blocked_node_count: i64,
    #[ts(type = "number")]
    pub usage_limited_node_count: i64,
    #[ts(type = "number")]
    pub budget_limited_node_count: i64,
    #[ts(type = "number")]
    pub cancelled_node_count: i64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub nodes: Vec<ThreadGoalPlanNode>,
}

impl From<codex_protocol::protocol::ThreadGoalPlan> for ThreadGoalPlan {
    fn from(value: codex_protocol::protocol::ThreadGoalPlan) -> Self {
        Self {
            plan_id: value.plan_id,
            thread_id: value.thread_id.to_string(),
            status: value.status.into(),
            auto_execute: value.auto_execute.into(),
            max_tokens: value.max_tokens,
            total_tokens_used: value.total_tokens_used,
            total_time_used_seconds: value.total_time_used_seconds,
            remaining_tokens: value.remaining_tokens,
            node_count: value.node_count,
            completed_node_count: value.completed_node_count,
            ready_node_count: value.ready_node_count,
            active_node_count: value.active_node_count,
            pending_node_count: value.pending_node_count,
            paused_node_count: value.paused_node_count,
            blocked_node_count: value.blocked_node_count,
            usage_limited_node_count: value.usage_limited_node_count,
            budget_limited_node_count: value.budget_limited_node_count,
            cancelled_node_count: value.cancelled_node_count,
            created_at: value.created_at,
            updated_at: value.updated_at,
            nodes: value
                .nodes
                .into_iter()
                .map(ThreadGoalPlanNode::from)
                .collect(),
        }
    }
}

impl From<codex_protocol::protocol::ThreadGoalPlanNode> for ThreadGoalPlanNode {
    fn from(value: codex_protocol::protocol::ThreadGoalPlanNode) -> Self {
        Self {
            node_id: value.node_id,
            plan_id: value.plan_id,
            thread_id: value.thread_id.to_string(),
            assigned_thread_id: value
                .assigned_thread_id
                .unwrap_or(value.thread_id)
                .to_string(),
            key: value.key,
            sequence: value.sequence,
            priority: value.priority,
            objective: value.objective,
            title: value.title,
            status: value.status.into(),
            ready: value.ready,
            token_budget: value.token_budget,
            tokens_used: value.tokens_used,
            time_used_seconds: value.time_used_seconds,
            projected_goal_id: value.projected_goal_id,
            depends_on: value.depends_on,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<codex_protocol::protocol::ThreadGoalPlanStatus> for ThreadGoalPlanStatus {
    fn from(value: codex_protocol::protocol::ThreadGoalPlanStatus) -> Self {
        match value {
            codex_protocol::protocol::ThreadGoalPlanStatus::Active => Self::Active,
            codex_protocol::protocol::ThreadGoalPlanStatus::Paused => Self::Paused,
            codex_protocol::protocol::ThreadGoalPlanStatus::Blocked => Self::Blocked,
            codex_protocol::protocol::ThreadGoalPlanStatus::BudgetLimited => Self::BudgetLimited,
            codex_protocol::protocol::ThreadGoalPlanStatus::Complete => Self::Complete,
            codex_protocol::protocol::ThreadGoalPlanStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<codex_protocol::protocol::ThreadGoalPlanAutoExecute> for ThreadGoalPlanAutoExecute {
    fn from(value: codex_protocol::protocol::ThreadGoalPlanAutoExecute) -> Self {
        match value {
            codex_protocol::protocol::ThreadGoalPlanAutoExecute::Off => Self::Off,
            codex_protocol::protocol::ThreadGoalPlanAutoExecute::ReadyOnly => Self::ReadyOnly,
            codex_protocol::protocol::ThreadGoalPlanAutoExecute::AiDirected => Self::AiDirected,
        }
    }
}

impl From<codex_protocol::protocol::ThreadGoalPlanNodeStatus> for ThreadGoalPlanNodeStatus {
    fn from(value: codex_protocol::protocol::ThreadGoalPlanNodeStatus) -> Self {
        match value {
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::Pending => Self::Pending,
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::Active => Self::Active,
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::Paused => Self::Paused,
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::Blocked => Self::Blocked,
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::UsageLimited => Self::UsageLimited,
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::BudgetLimited => {
                Self::BudgetLimited
            }
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::Complete => Self::Complete,
            codex_protocol::protocol::ThreadGoalPlanNodeStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalListResponse {
    pub goal: Option<ThreadGoal>,
    pub goal_plans: Vec<ThreadGoalPlan>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalPlanActivateNodeParams {
    pub thread_id: String,
    pub node_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalPlanActivateNodeResponse {
    pub goal: ThreadGoal,
    pub plan: ThreadGoalPlan,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalClearParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalClearResponse {
    pub cleared: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadScheduleStatus {
    Active,
    Paused,
    Expired,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadScheduleIntervalUnit {
    Minutes,
    Hours,
    Days,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadScheduleSpec {
    Once,
    Dynamic,
    Interval {
        #[ts(type = "number")]
        amount: i64,
        unit: ThreadScheduleIntervalUnit,
    },
    Cron {
        expression: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadSchedulePromptSource {
    Inline,
    Default,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSchedule {
    pub thread_id: String,
    pub schedule_id: String,
    pub prompt: String,
    pub prompt_source: ThreadSchedulePromptSource,
    pub schedule: ThreadScheduleSpec,
    pub timezone: String,
    pub status: ThreadScheduleStatus,
    #[ts(type = "number | null")]
    pub next_run_at: Option<i64>,
    #[ts(type = "number | null")]
    pub last_run_at: Option<i64>,
    #[ts(type = "number | null")]
    pub expires_at: Option<i64>,
    #[ts(type = "number")]
    pub failure_count: i64,
    #[ts(type = "number | null")]
    pub lease_expires_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadScheduleRunStatus {
    Leased,
    Running,
    Deferred,
    Completed,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleRun {
    pub thread_id: String,
    pub schedule_id: String,
    pub run_id: String,
    pub status: ThreadScheduleRunStatus,
    pub lease_id: String,
    #[ts(type = "string | null")]
    pub turn_id: Option<String>,
    #[ts(type = "string | null")]
    pub error: Option<String>,
    #[ts(type = "number | null")]
    pub scheduled_for_at: Option<i64>,
    #[ts(type = "number")]
    pub started_at: i64,
    #[ts(type = "number | null")]
    pub completed_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleStats {
    #[ts(type = "number")]
    pub total_runs: i64,
    #[ts(type = "number")]
    pub leased_runs: i64,
    #[ts(type = "number")]
    pub running_runs: i64,
    #[ts(type = "number")]
    pub deferred_runs: i64,
    #[ts(type = "number")]
    pub completed_runs: i64,
    #[ts(type = "number")]
    pub failed_runs: i64,
    #[ts(type = "number | null")]
    pub last_started_at: Option<i64>,
    #[ts(type = "number | null")]
    pub last_completed_at: Option<i64>,
    #[ts(type = "string | null")]
    pub last_error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleCreateParams {
    pub thread_id: String,
    pub prompt: String,
    #[ts(optional = nullable)]
    pub prompt_source: Option<ThreadSchedulePromptSource>,
    pub schedule: ThreadScheduleSpec,
    #[ts(optional = nullable)]
    pub timezone: Option<String>,
    #[ts(optional = nullable)]
    pub next_run_at: Option<i64>,
    #[ts(optional = nullable)]
    pub expires_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleCreateResponse {
    pub schedule: ThreadSchedule,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleListResponse {
    pub data: Vec<ThreadSchedule>,
    #[ts(type = "string | null")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleGetParams {
    pub thread_id: String,
    pub schedule_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleGetResponse {
    pub schedule: Option<ThreadSchedule>,
    pub stats: ThreadScheduleStats,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleUpdateParams {
    pub thread_id: String,
    pub schedule_id: String,
    #[ts(optional = nullable)]
    pub prompt: Option<String>,
    #[ts(optional = nullable)]
    pub schedule: Option<ThreadScheduleSpec>,
    #[ts(optional = nullable)]
    pub timezone: Option<String>,
    #[ts(optional = nullable)]
    pub status: Option<ThreadScheduleStatus>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable, type = "number | null")]
    pub next_run_at: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional = nullable, type = "number | null")]
    pub expires_at: Option<Option<i64>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleUpdateResponse {
    pub schedule: ThreadSchedule,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSchedulePauseParams {
    pub thread_id: String,
    pub schedule_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSchedulePauseResponse {
    pub schedule: ThreadSchedule,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleResumeParams {
    pub thread_id: String,
    pub schedule_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleResumeResponse {
    pub schedule: ThreadSchedule,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleDeleteParams {
    pub thread_id: String,
    pub schedule_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleDeleteResponse {
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleRunNowParams {
    pub thread_id: String,
    pub schedule_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleRunNowResponse {
    pub run: ThreadScheduleRun,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMonitorStatus {
    Running,
    Stopped,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMonitorRouting {
    Stream,
    File,
    Both,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadMonitorEventStream {
    Stdout,
    Stderr,
    System,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitor {
    pub thread_id: String,
    pub monitor_id: String,
    pub name: String,
    pub prompt: String,
    pub command: String,
    #[ts(type = "string | null")]
    pub cwd: Option<String>,
    pub routing: ThreadMonitorRouting,
    #[ts(type = "string | null")]
    pub output_file: Option<String>,
    pub status: ThreadMonitorStatus,
    #[ts(type = "number")]
    pub generation: i64,
    #[ts(type = "number | null")]
    pub process_id: Option<i64>,
    #[ts(type = "number | null")]
    pub last_event_at: Option<i64>,
    #[ts(type = "string | null")]
    pub last_error: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorEvent {
    pub thread_id: String,
    pub monitor_id: String,
    pub event_id: String,
    pub stream: ThreadMonitorEventStream,
    pub text: String,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorCreateParams {
    pub thread_id: String,
    pub name: String,
    pub prompt: String,
    pub command: String,
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    #[ts(optional = nullable)]
    pub routing: Option<ThreadMonitorRouting>,
    #[ts(optional = nullable)]
    pub output_file: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorCreateResponse {
    pub monitor: ThreadMonitor,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorListResponse {
    pub data: Vec<ThreadMonitor>,
    #[ts(type = "string | null")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorReadParams {
    pub thread_id: String,
    pub monitor_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorReadResponse {
    pub monitor: Option<ThreadMonitor>,
    pub events: Vec<ThreadMonitorEvent>,
    #[ts(type = "string | null")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorStopParams {
    pub thread_id: String,
    pub monitor_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorStopResponse {
    pub monitor: ThreadMonitor,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorRestartParams {
    pub thread_id: String,
    pub monitor_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorRestartResponse {
    pub monitor: ThreadMonitor,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorDeleteParams {
    pub thread_id: String,
    pub monitor_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorDeleteResponse {
    pub monitor_id: String,
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMetadataUpdateParams {
    pub thread_id: String,
    /// Patch the stored Git metadata for this thread.
    /// Omit a field to leave it unchanged, set it to `null` to clear it, or
    /// provide a string to replace the stored value.
    #[ts(optional = nullable)]
    pub git_info: Option<ThreadMetadataGitInfoUpdateParams>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMetadataGitInfoUpdateParams {
    /// Omit to leave the stored commit unchanged, set to `null` to clear it,
    /// or provide a non-empty string to replace it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option"
    )]
    #[ts(optional = nullable, type = "string | null")]
    pub sha: Option<Option<String>>,
    /// Omit to leave the stored branch unchanged, set to `null` to clear it,
    /// or provide a non-empty string to replace it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option"
    )]
    #[ts(optional = nullable, type = "string | null")]
    pub branch: Option<Option<String>>,
    /// Omit to leave the stored origin URL unchanged, set to `null` to clear it,
    /// or provide a non-empty string to replace it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::protocol::serde_helpers::serialize_double_option",
        deserialize_with = "crate::protocol::serde_helpers::deserialize_double_option"
    )]
    #[ts(optional = nullable, type = "string | null")]
    pub origin_url: Option<Option<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMetadataUpdateResponse {
    pub thread: Thread,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum ThreadMemoryMode {
    Enabled,
    Disabled,
}

impl ThreadMemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub fn to_core(self) -> codex_protocol::protocol::ThreadMemoryMode {
        match self {
            Self::Enabled => codex_protocol::protocol::ThreadMemoryMode::Enabled,
            Self::Disabled => codex_protocol::protocol::ThreadMemoryMode::Disabled,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMemoryModeSetParams {
    pub thread_id: String,
    pub mode: ThreadMemoryMode,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMemoryModeSetResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryResetResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUnarchiveResponse {
    pub thread: Thread,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadCompactStartParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadCompactStartResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadRecapParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub prompt: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadRecapResponse {
    pub summary: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadShellCommandParams {
    pub thread_id: String,
    /// Shell command string evaluated by the thread's configured shell.
    /// Unlike `command/exec`, this intentionally preserves shell syntax
    /// such as pipes, redirects, and quoting. This runs unsandboxed with full
    /// access rather than inheriting the thread sandbox policy.
    pub command: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadShellCommandResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename_all = "kebab-case", export_to = "v2/")]
pub enum ThreadExternalAgentMode {
    Plan,
    Propose,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadExternalAgentStartParams {
    pub thread_id: String,
    pub runtime_id: String,
    pub task: String,
    pub mode: ThreadExternalAgentMode,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadExternalAgentCancelParams {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadExternalAgentCancelResponse {
    pub cancelled: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadExternalAgentStartResponse {
    pub status: ThreadExternalAgentStartStatus,
    pub run_id: Option<String>,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename_all = "kebab-case", export_to = "v2/")]
pub enum ThreadExternalAgentStartStatus {
    Started,
    Gated,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadExternalAgentEventNotification {
    pub thread_id: String,
    pub run_id: String,
    pub event: ThreadExternalAgentEvent,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadExternalAgentEvent {
    RunStarted {
        runtime_id: String,
        mode: ThreadExternalAgentMode,
        task: String,
    },
    SessionResolved {
        external_session_id: Option<String>,
    },
    OutputTextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    PlanUpdated {
        plan: String,
    },
    PermissionRequested {
        request: ThreadExternalAgentPermissionRequest,
    },
    ProposedAction {
        proposal: JsonValue,
    },
    Status {
        message: String,
    },
    Completed {
        summary: Option<String>,
    },
    Failed {
        message: String,
    },
    Cancelled {
        reason: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadExternalAgentPermissionRequest {
    pub id: String,
    pub action: JsonValue,
    pub options: Vec<ThreadExternalAgentPermissionOption>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadExternalAgentPermissionOption {
    AllowOnce,
    RejectOnce,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadApproveGuardianDeniedActionParams {
    pub thread_id: String,
    /// Serialized `codex_protocol::protocol::GuardianAssessmentEvent`.
    pub event: JsonValue,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadApproveGuardianDeniedActionResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadBackgroundTerminalsCleanParams {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadBackgroundTerminalsCleanResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadRollbackParams {
    pub thread_id: String,
    /// The number of turns to drop from the end of the thread. Must be >= 1.
    ///
    /// This only modifies the thread's history and does not revert local file changes
    /// that have been made by the agent. Clients are responsible for reverting these changes.
    pub num_turns: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadRollbackResponse {
    /// The updated thread after applying the rollback, with `turns` populated.
    ///
    /// The ThreadItems stored in each Turn are lossy since we explicitly do not
    /// persist all agent interactions, such as command executions. This is the same
    /// behavior as `thread/resume`.
    pub thread: Thread,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadListParams {
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to a reasonable server-side value.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Optional sort key; defaults to created_at.
    #[ts(optional = nullable)]
    pub sort_key: Option<ThreadSortKey>,
    /// Optional sort direction; defaults to descending (newest first).
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
    /// Optional provider filter; when set, only sessions recorded under these
    /// providers are returned. When present but empty, includes all providers.
    #[ts(optional = nullable)]
    pub model_providers: Option<Vec<String>>,
    /// Optional source filter; when set, only sessions from these source kinds
    /// are returned. When omitted or empty, defaults to interactive sources.
    #[ts(optional = nullable)]
    pub source_kinds: Option<Vec<ThreadSourceKind>>,
    /// Optional archived filter; when set to true, only archived threads are returned.
    /// If false or null, only non-archived threads are returned.
    #[ts(optional = nullable)]
    pub archived: Option<bool>,
    /// Optional cwd filter or filters; when set, only threads whose session cwd
    /// exactly matches one of these paths are returned.
    #[ts(optional = nullable, type = "string | Array<string> | null")]
    pub cwd: Option<ThreadListCwdFilter>,
    /// If true, return from the state DB without scanning JSONL rollouts to
    /// repair thread metadata. Omitted or false preserves scan-and-repair
    /// behavior.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_state_db_only: bool,
    /// Optional substring filter for the extracted thread title.
    #[ts(optional = nullable)]
    pub search_term: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSearchParams {
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to a reasonable server-side value.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Optional sort key; defaults to created_at.
    #[ts(optional = nullable)]
    pub sort_key: Option<ThreadSortKey>,
    /// Optional sort direction; defaults to descending (newest first).
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
    /// Optional source filter; when set, only sessions from these source kinds
    /// are returned. When omitted or empty, defaults to interactive sources.
    #[ts(optional = nullable)]
    pub source_kinds: Option<Vec<ThreadSourceKind>>,
    /// Optional archived filter; when set to true, only archived threads are returned.
    /// If false or null, only non-archived threads are returned.
    #[ts(optional = nullable)]
    pub archived: Option<bool>,
    /// Required substring/full-text query for thread search.
    pub search_term: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum ThreadListCwdFilter {
    One(String),
    Many(Vec<String>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ThreadSourceKind {
    Cli,
    #[serde(rename = "vscode")]
    #[ts(rename = "vscode")]
    VsCode,
    Exec,
    AppServer,
    SubAgent,
    SubAgentReview,
    SubAgentCompact,
    SubAgentThreadSpawn,
    SubAgentOther,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum ThreadSortKey {
    CreatedAt,
    UpdatedAt,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadListResponse {
    pub data: Vec<Thread>,
    /// Opaque cursor to pass to the next call to continue after the last item.
    /// if None, there are no more items to return.
    pub next_cursor: Option<String>,
    /// Opaque cursor to pass as `cursor` when reversing `sortDirection`.
    /// This is only populated when the page contains at least one thread.
    /// Use it with the opposite `sortDirection`; for timestamp sorts it anchors
    /// at the start of the page timestamp so same-second updates are not skipped.
    pub backwards_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSearchResult {
    pub thread: Thread,
    pub snippet: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSearchResponse {
    pub data: Vec<ThreadSearchResult>,
    /// Opaque cursor to pass to the next call to continue after the last item.
    /// if None, there are no more items to return.
    pub next_cursor: Option<String>,
    /// Opaque cursor to pass as `cursor` when reversing `sortDirection`.
    /// This is only populated when the page contains at least one thread.
    /// Use it with the opposite `sortDirection`; for timestamp sorts it anchors
    /// at the start of the page timestamp so same-second updates are not skipped.
    pub backwards_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadLoadedListParams {
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to no limit.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadLoadedListResponse {
    /// Thread ids for sessions currently loaded in memory.
    pub data: Vec<String>,
    /// Opaque cursor to pass to the next call to continue after the last item.
    /// if None, there are no more items to return.
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
#[ts(export_to = "v2/")]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Active {
        active_flags: Vec<ThreadActiveFlag>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum ThreadActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadReadParams {
    pub thread_id: String,
    /// When true, include turns and their items from rollout history.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_turns: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadReadResponse {
    pub thread: Thread,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadInjectItemsParams {
    pub thread_id: String,
    /// Raw Responses API items to append to the thread's model-visible history.
    pub items: Vec<JsonValue>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadInjectItemsResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTurnsListParams {
    pub thread_id: String,
    /// Opaque cursor to pass to the next call to continue after the last turn.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional turn page size.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Optional turn pagination direction; defaults to descending.
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
    /// How much item detail to include for each returned turn; defaults to summary.
    #[ts(optional = nullable)]
    pub items_view: Option<TurnItemsView>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTurnsListResponse {
    pub data: Vec<Turn>,
    /// Opaque cursor to pass to the next call to continue after the last turn.
    /// if None, there are no more turns to return.
    pub next_cursor: Option<String>,
    /// Opaque cursor to pass as `cursor` when reversing `sortDirection`.
    /// This is only populated when the page contains at least one turn.
    /// Use it with the opposite `sortDirection` to include the anchor turn again
    /// and catch updates to that turn.
    pub backwards_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTurnsItemsListParams {
    pub thread_id: String,
    pub turn_id: String,
    /// Opaque cursor to pass to the next call to continue after the last item.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional item page size.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Optional item pagination direction; defaults to ascending.
    #[ts(optional = nullable)]
    pub sort_direction: Option<SortDirection>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTurnsItemsListResponse {
    pub data: Vec<ThreadItem>,
    /// Opaque cursor to pass to the next call to continue after the last item.
    /// if None, there are no more items to return.
    pub next_cursor: Option<String>,
    /// Opaque cursor to pass as `cursor` when reversing `sortDirection`.
    /// This is only populated when the page contains at least one item.
    pub backwards_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTokenUsageUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub token_usage: ThreadTokenUsage,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTokenUsage {
    pub total: TokenUsageBreakdown,
    pub last: TokenUsageBreakdown,
    // TODO(aibrahim): make this not optional
    #[ts(type = "number | null")]
    pub model_context_window: Option<i64>,
}

impl From<CoreTokenUsageInfo> for ThreadTokenUsage {
    fn from(value: CoreTokenUsageInfo) -> Self {
        Self {
            total: value.total_token_usage.into(),
            last: value.last_token_usage.into(),
            model_context_window: value.model_context_window,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TokenUsageBreakdown {
    #[ts(type = "number")]
    pub total_tokens: i64,
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub cached_input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub reasoning_output_tokens: i64,
}

impl From<CoreTokenUsage> for TokenUsageBreakdown {
    fn from(value: CoreTokenUsage) -> Self {
        Self {
            total_tokens: value.total_tokens,
            input_tokens: value.input_tokens,
            cached_input_tokens: value.cached_input_tokens,
            output_tokens: value.output_tokens,
            reasoning_output_tokens: value.reasoning_output_tokens,
        }
    }
}

// Thread/Turn lifecycle notifications and item progress events
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadStartedNotification {
    pub thread: Thread,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadStatusChangedNotification {
    pub thread_id: String,
    pub status: ThreadStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadArchivedNotification {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUnarchivedNotification {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadClosedNotification {
    pub thread_id: String,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadNameUpdatedNotification {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub thread_name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalUpdatedNotification {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub goal: ThreadGoal,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalPlanUpdatedNotification {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub plan: ThreadGoalPlan,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadGoalClearedNotification {
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleUpdatedNotification {
    pub thread_id: String,
    pub schedule: ThreadSchedule,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleDeletedNotification {
    pub thread_id: String,
    pub schedule_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadScheduleRunUpdatedNotification {
    pub thread_id: String,
    pub run: ThreadScheduleRun,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorUpdatedNotification {
    pub thread_id: String,
    pub monitor: ThreadMonitor,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorDeletedNotification {
    pub thread_id: String,
    pub monitor_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMonitorEventNotification {
    pub thread_id: String,
    pub monitor: ThreadMonitor,
    pub event: ThreadMonitorEvent,
}

/// Deprecated: Use `ContextCompaction` item type instead.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ContextCompactedNotification {
    pub thread_id: String,
    pub turn_id: String,
}
