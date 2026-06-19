use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

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
