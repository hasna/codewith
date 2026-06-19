use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryListParams {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_disabled: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_forgotten: bool,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryListResponse {
    pub data: Vec<MachineRegistryMachine>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryReadParams {
    pub machine_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryReadResponse {
    pub machine: Option<MachineRegistryMachine>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryUpsertParams {
    #[ts(optional = nullable)]
    pub machine_id: Option<String>,
    #[ts(optional = nullable)]
    pub installation_id: Option<String>,
    #[ts(optional = nullable)]
    pub display_name: Option<String>,
    pub capabilities: JsonValue,
    pub endpoints: Vec<MachineRegistryEndpointUpsert>,
    #[ts(type = "number | null", optional = nullable)]
    pub last_seen_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryUpsertResponse {
    pub machine: MachineRegistryMachine,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryEndpointUpsert {
    #[ts(optional = nullable)]
    pub endpoint_id: Option<String>,
    pub transport: MachineRegistryEndpointTransport,
    pub address: String,
    #[ts(optional = nullable)]
    pub display_address: Option<String>,
    #[ts(optional = nullable)]
    pub priority: Option<i64>,
    pub capabilities: JsonValue,
    #[ts(type = "number | null", optional = nullable)]
    pub last_success_at: Option<i64>,
    #[ts(optional = nullable)]
    pub last_error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryDisableParams {
    pub machine_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryDisableResponse {
    pub machine: Option<MachineRegistryMachine>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryUpdateTrustParams {
    pub machine_id: String,
    pub trust_state: MachineRegistryTrustState,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryUpdateTrustResponse {
    pub machine: Option<MachineRegistryMachine>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryForgetParams {
    pub machine_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryForgetResponse {
    pub found: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryMachine {
    pub machine_id: String,
    pub installation_id: Option<String>,
    pub display_name: Option<String>,
    pub trust_state: MachineRegistryTrustState,
    pub enrollment_state: MachineRegistryEnrollmentState,
    pub health_state: MachineRegistryHealthState,
    pub source_kind: MachineRegistrySourceKind,
    pub adapter_name: Option<String>,
    pub capabilities: JsonValue,
    #[ts(type = "number | null")]
    pub last_seen_at: Option<i64>,
    #[ts(type = "number | null")]
    pub disabled_at: Option<i64>,
    #[ts(type = "number | null")]
    pub forgotten_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub endpoints: Vec<MachineRegistryEndpoint>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MachineRegistryEndpoint {
    pub endpoint_id: String,
    pub machine_id: String,
    pub transport: MachineRegistryEndpointTransport,
    pub display_address: String,
    pub redactions: Vec<MachineRegistryRedaction>,
    pub priority: i64,
    pub capabilities: JsonValue,
    #[ts(type = "number | null")]
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MachineRegistryTrustState {
    Local,
    Trusted,
    Untrusted,
    Disabled,
    Revoked,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MachineRegistryEnrollmentState {
    Local,
    Manual,
    Discovered,
    Enrolled,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MachineRegistryHealthState {
    Unknown,
    Online,
    Offline,
    Degraded,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MachineRegistrySourceKind {
    Local,
    Manual,
    Adapter,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MachineRegistryEndpointTransport {
    Lan,
    Tailscale,
    Manual,
    RemoteControl,
    Adapter,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MachineRegistryRedaction {
    EndpointAddress,
}
