use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventListParams {
    #[ts(optional = nullable)]
    pub source_app_id: Option<String>,
    #[ts(optional = nullable)]
    pub target_thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub statuses: Option<Vec<WebhookEventStatus>>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventListResponse {
    pub data: Vec<WebhookEventSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventReadParams {
    pub event_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventReadResponse {
    pub event: Option<WebhookEventDetail>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventMarkParams {
    pub event_id: String,
    pub status: WebhookEventStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventMarkResponse {
    pub event: Option<WebhookEventSummary>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventIngestParams {
    pub source_app_id: String,
    #[ts(optional = nullable)]
    pub source_app_name: Option<String>,
    #[ts(optional = nullable)]
    pub subscription_id: Option<String>,
    pub event_type: String,
    #[ts(optional = nullable)]
    pub external_delivery_id: Option<String>,
    #[ts(optional = nullable)]
    pub idempotency_key: Option<String>,
    #[ts(optional = nullable)]
    pub target_thread_id: Option<String>,
    pub payload_json: JsonValue,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventIngestResponse {
    pub event: WebhookEventDetail,
    pub created: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventSummary {
    pub event_id: String,
    pub source_app_id: String,
    pub source_app_name: Option<String>,
    pub subscription_id: Option<String>,
    pub event_type: String,
    pub external_delivery_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub target_thread_id: Option<String>,
    pub status: WebhookEventStatus,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions: Vec<WebhookPayloadRedaction>,
    #[ts(type = "number")]
    pub received_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookEventDetail {
    pub summary: WebhookEventSummary,
    pub payload_json: JsonValue,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WebhookPayloadRedaction {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum WebhookEventStatus {
    Unread,
    Processed,
    Archived,
    Injected,
    Queued,
}
