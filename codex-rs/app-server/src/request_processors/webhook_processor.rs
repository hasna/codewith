use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::WebhookEventDetail;
use codex_app_server_protocol::WebhookEventIngestParams;
use codex_app_server_protocol::WebhookEventIngestResponse;
use codex_app_server_protocol::WebhookEventListParams;
use codex_app_server_protocol::WebhookEventListResponse;
use codex_app_server_protocol::WebhookEventMarkParams;
use codex_app_server_protocol::WebhookEventMarkResponse;
use codex_app_server_protocol::WebhookEventReadParams;
use codex_app_server_protocol::WebhookEventReadResponse;
use codex_app_server_protocol::WebhookEventStatus;
use codex_app_server_protocol::WebhookEventSummary;
use codex_app_server_protocol::WebhookPayloadRedaction;
use codex_protocol::ThreadId;
use codex_rollout::StateDbHandle;

const MAX_WEBHOOK_TEXT_CHARS: usize = 256;
const MAX_WEBHOOK_PAYLOAD_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct WebhookRequestProcessor {
    state_db: Option<StateDbHandle>,
}

impl WebhookRequestProcessor {
    pub(crate) fn new(state_db: Option<StateDbHandle>) -> Self {
        Self { state_db }
    }

    pub(crate) async fn list(
        &self,
        params: WebhookEventListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let limit = normalize_webhook_limit(params.limit)?;
        let list_params = codex_state::WebhookEventListParams {
            source_app_id: normalize_optional_text("sourceAppId", params.source_app_id)?,
            target_thread_id: params
                .target_thread_id
                .as_deref()
                .map(parse_thread_id)
                .transpose()?,
            statuses: params.statuses.map(|statuses| {
                statuses
                    .into_iter()
                    .map(api_webhook_status_to_state)
                    .collect()
            }),
            cursor: normalize_webhook_cursor(params.cursor)?,
            limit,
        };
        let page = state_db
            .webhook_events()
            .list_webhook_events(list_params)
            .await
            .map_err(|err| internal_error(format!("failed to list webhook events: {err}")))?;
        Ok(Some(
            WebhookEventListResponse {
                data: page.data.into_iter().map(api_webhook_summary).collect(),
                next_cursor: page.next_cursor,
            }
            .into(),
        ))
    }

    pub(crate) async fn read(
        &self,
        params: WebhookEventReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let event_id = normalize_required_text("eventId", params.event_id)?;
        let event = state_db
            .webhook_events()
            .get_webhook_event(event_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read webhook event: {err}")))?
            .map(api_webhook_detail);
        Ok(Some(WebhookEventReadResponse { event }.into()))
    }

    pub(crate) async fn mark(
        &self,
        params: WebhookEventMarkParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let event_id = normalize_required_text("eventId", params.event_id)?;
        let status = api_webhook_status_to_state(params.status);
        let event = state_db
            .webhook_events()
            .mark_webhook_event(event_id.as_str(), status)
            .await
            .map_err(|err| internal_error(format!("failed to update webhook event: {err}")))?
            .map(api_webhook_summary);
        Ok(Some(WebhookEventMarkResponse { event }.into()))
    }

    pub(crate) async fn ingest(
        &self,
        params: WebhookEventIngestParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let payload_size = serde_json::to_vec(&params.payload_json)
            .map_err(|err| invalid_request(format!("webhook payloadJson is invalid: {err}")))?
            .len();
        if payload_size > MAX_WEBHOOK_PAYLOAD_BYTES {
            return Err(invalid_request(format!(
                "webhook payloadJson must be at most {MAX_WEBHOOK_PAYLOAD_BYTES} bytes"
            )));
        }
        let ingest_params = codex_state::WebhookEventIngestParams {
            source_app_id: normalize_required_text("sourceAppId", params.source_app_id)?,
            source_app_name: normalize_optional_text("sourceAppName", params.source_app_name)?,
            subscription_id: normalize_optional_text("subscriptionId", params.subscription_id)?,
            event_type: normalize_required_text("eventType", params.event_type)?,
            external_delivery_id: normalize_optional_text(
                "externalDeliveryId",
                params.external_delivery_id,
            )?,
            idempotency_key: normalize_optional_text("idempotencyKey", params.idempotency_key)?,
            target_thread_id: params
                .target_thread_id
                .as_deref()
                .map(parse_thread_id)
                .transpose()?,
            payload_json: params.payload_json,
        };
        let outcome = state_db
            .webhook_events()
            .ingest_webhook_event(ingest_params)
            .await
            .map_err(map_webhook_ingest_error)?;
        Ok(Some(
            WebhookEventIngestResponse {
                event: api_webhook_detail(outcome.event),
                created: outcome.created,
            }
            .into(),
        ))
    }

    fn state_db(&self) -> Result<StateDbHandle, JSONRPCErrorError> {
        self.state_db
            .clone()
            .ok_or_else(|| internal_error("webhook event state store is unavailable"))
    }
}

fn normalize_webhook_limit(limit: Option<u32>) -> Result<u32, JSONRPCErrorError> {
    let limit = limit.unwrap_or(codex_state::DEFAULT_WEBHOOK_EVENT_LIST_LIMIT);
    if limit == 0 || limit > codex_state::MAX_WEBHOOK_EVENT_LIST_LIMIT {
        return Err(invalid_request(format!(
            "webhook event list limit must be between 1 and {}",
            codex_state::MAX_WEBHOOK_EVENT_LIST_LIMIT
        )));
    }
    Ok(limit)
}

fn normalize_webhook_cursor(cursor: Option<String>) -> Result<Option<String>, JSONRPCErrorError> {
    let Some(cursor) = normalize_optional_text("cursor", cursor)? else {
        return Ok(None);
    };
    cursor
        .parse::<u32>()
        .map_err(|_| invalid_request("webhook event list cursor is invalid"))?;
    Ok(Some(cursor))
}

fn map_webhook_ingest_error(err: anyhow::Error) -> JSONRPCErrorError {
    let message = err.to_string();
    if message == codex_state::WEBHOOK_EVENT_DEDUPE_CONFLICT_MESSAGE {
        invalid_request(message)
    } else {
        internal_error(format!("failed to ingest webhook event: {message}"))
    }
}

fn normalize_required_text(field_name: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request(format!("{field_name} must not be empty")));
    }
    if value.chars().count() > MAX_WEBHOOK_TEXT_CHARS {
        return Err(invalid_request(format!(
            "{field_name} must be at most {MAX_WEBHOOK_TEXT_CHARS} characters"
        )));
    }
    Ok(value.to_string())
}

fn normalize_optional_text(
    field_name: &str,
    value: Option<String>,
) -> Result<Option<String>, JSONRPCErrorError> {
    value
        .map(|value| normalize_required_text(field_name, value))
        .transpose()
}

fn parse_thread_id(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::try_from(thread_id.to_string())
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}

fn api_webhook_detail(event: codex_state::WebhookEvent) -> WebhookEventDetail {
    WebhookEventDetail {
        payload_json: event.payload_json.clone(),
        summary: api_webhook_summary(event),
    }
}

fn api_webhook_summary(event: codex_state::WebhookEvent) -> WebhookEventSummary {
    WebhookEventSummary {
        event_id: event.event_id,
        source_app_id: event.source_app_id,
        source_app_name: event.source_app_name,
        subscription_id: event.subscription_id,
        event_type: event.event_type,
        external_delivery_id: event.external_delivery_id,
        idempotency_key: event.idempotency_key.map(|_| "<redacted>".to_string()),
        target_thread_id: event
            .target_thread_id
            .map(|thread_id| thread_id.to_string()),
        status: api_webhook_status_from_state(event.status),
        payload_sha256: event.payload_sha256,
        payload_preview: event.payload_preview,
        redactions: event
            .redactions
            .into_iter()
            .map(api_webhook_redaction)
            .collect(),
        received_at: event.received_at.timestamp(),
        updated_at: event.updated_at.timestamp(),
    }
}

fn api_webhook_redaction(
    redaction: codex_state::WebhookPayloadRedaction,
) -> WebhookPayloadRedaction {
    WebhookPayloadRedaction {
        path: redaction.path,
        reason: redaction.reason,
    }
}

fn api_webhook_status_from_state(status: codex_state::WebhookEventStatus) -> WebhookEventStatus {
    match status {
        codex_state::WebhookEventStatus::Unread => WebhookEventStatus::Unread,
        codex_state::WebhookEventStatus::Processed => WebhookEventStatus::Processed,
        codex_state::WebhookEventStatus::Archived => WebhookEventStatus::Archived,
        codex_state::WebhookEventStatus::Injected => WebhookEventStatus::Injected,
        codex_state::WebhookEventStatus::Queued => WebhookEventStatus::Queued,
    }
}

fn api_webhook_status_to_state(status: WebhookEventStatus) -> codex_state::WebhookEventStatus {
    match status {
        WebhookEventStatus::Unread => codex_state::WebhookEventStatus::Unread,
        WebhookEventStatus::Processed => codex_state::WebhookEventStatus::Processed,
        WebhookEventStatus::Archived => codex_state::WebhookEventStatus::Archived,
        WebhookEventStatus::Injected => codex_state::WebhookEventStatus::Injected,
        WebhookEventStatus::Queued => codex_state::WebhookEventStatus::Queued,
    }
}
