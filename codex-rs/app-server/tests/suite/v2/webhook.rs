use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::WebhookEventIngestResponse;
use codex_app_server_protocol::WebhookEventListResponse;
use codex_app_server_protocol::WebhookEventMarkResponse;
use codex_app_server_protocol::WebhookEventReadResponse;
use codex_app_server_protocol::WebhookEventStatus;
use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

#[tokio::test]
async fn webhook_event_lifecycle_redacts_dedupes_and_marks_events() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;
    let target_thread_id = ThreadId::new().to_string();

    let first_response = send_raw(
        &mut mcp,
        "webhook/event/ingest",
        json!({
            "sourceAppId": "github",
            "sourceAppName": "GitHub",
            "subscriptionId": "pull-requests",
            "eventType": "pull_request.opened",
            "externalDeliveryId": "delivery-1",
            "idempotencyKey": "sk-live-idempotency-secret",
            "targetThreadId": target_thread_id.clone(),
            "payloadJson": {
                "action": "opened",
                "authorization": "Bearer raw-secret-should-not-leak",
                "pull_request": {
                    "title": "Add a feature",
                    "token": "sk-live-secret"
                }
            }
        }),
    )
    .await?;
    let serialized_first_response = serde_json::to_string(&first_response)?;
    assert!(!serialized_first_response.contains("raw-secret-should-not-leak"));
    assert!(!serialized_first_response.contains("sk-live-secret"));
    assert!(!serialized_first_response.contains("sk-live-idempotency-secret"));

    let first: WebhookEventIngestResponse = to_response(first_response)?;
    assert!(first.created);
    assert_eq!(first.event.summary.status, WebhookEventStatus::Unread);
    assert_eq!(
        first.event.summary.idempotency_key.as_deref(),
        Some("<redacted>")
    );
    assert_eq!(
        first.event.payload_json,
        json!({
            "action": "opened",
            "authorization": "<redacted>",
            "pull_request": {
                "title": "Add a feature",
                "token": "<redacted>"
            }
        })
    );
    assert_eq!(first.event.summary.redactions.len(), 2);

    let duplicate: WebhookEventIngestResponse = send_and_decode(
        &mut mcp,
        "webhook/event/ingest",
        json!({
            "sourceAppId": "github",
            "sourceAppName": "GitHub",
            "subscriptionId": "pull-requests",
            "eventType": "pull_request.opened",
            "externalDeliveryId": "delivery-1",
            "idempotencyKey": "sk-live-idempotency-secret",
            "targetThreadId": target_thread_id.clone(),
            "payloadJson": {
                "action": "opened",
                "authorization": "Bearer another-secret"
            }
        }),
    )
    .await?;
    assert!(!duplicate.created);
    assert_eq!(
        duplicate.event.summary.event_id,
        first.event.summary.event_id
    );

    let listed: WebhookEventListResponse = send_and_decode(
        &mut mcp,
        "webhook/event/list",
        json!({
            "sourceAppId": "github",
            "targetThreadId": target_thread_id.clone(),
            "statuses": ["unread"],
            "cursor": null,
            "limit": 10
        }),
    )
    .await?;
    assert_eq!(listed.data, vec![first.event.summary.clone()]);

    let read: WebhookEventReadResponse = send_and_decode(
        &mut mcp,
        "webhook/event/read",
        json!({
            "eventId": first.event.summary.event_id.clone(),
        }),
    )
    .await?;
    assert_eq!(read.event, Some(first.event.clone()));

    let marked: WebhookEventMarkResponse = send_and_decode(
        &mut mcp,
        "webhook/event/mark",
        json!({
            "eventId": first.event.summary.event_id.clone(),
            "status": "processed",
        }),
    )
    .await?;
    let marked_event = marked.event.expect("event should still exist");
    assert_eq!(marked_event.status, WebhookEventStatus::Processed);
    assert_eq!(marked_event.event_id, first.event.summary.event_id);

    let listed_unread: WebhookEventListResponse = send_and_decode(
        &mut mcp,
        "webhook/event/list",
        json!({
            "statuses": ["unread"],
            "cursor": null,
            "limit": 10
        }),
    )
    .await?;
    assert_eq!(listed_unread.data, Vec::new());

    Ok(())
}

#[tokio::test]
async fn webhook_event_list_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "webhook/event/list",
            Some(json!({
                "cursor": "not-a-cursor",
            })),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, "webhook event list cursor is invalid");

    Ok(())
}

#[tokio::test]
async fn webhook_event_ingest_rejects_conflicting_dedupe_keys() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    send_and_decode::<WebhookEventIngestResponse>(
        &mut mcp,
        "webhook/event/ingest",
        json!({
            "sourceAppId": "github",
            "eventType": "pull_request.opened",
            "externalDeliveryId": "delivery-1",
            "idempotencyKey": "idempotency-1",
            "payloadJson": { "delivery": 1 }
        }),
    )
    .await?;
    send_and_decode::<WebhookEventIngestResponse>(
        &mut mcp,
        "webhook/event/ingest",
        json!({
            "sourceAppId": "github",
            "eventType": "pull_request.opened",
            "externalDeliveryId": "delivery-2",
            "idempotencyKey": "idempotency-2",
            "payloadJson": { "delivery": 2 }
        }),
    )
    .await?;

    let request_id = mcp
        .send_raw_request(
            "webhook/event/ingest",
            Some(json!({
                "sourceAppId": "github",
                "eventType": "pull_request.opened",
                "externalDeliveryId": "delivery-2",
                "idempotencyKey": "idempotency-1",
                "payloadJson": { "delivery": "conflict" }
            })),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        codex_state::WEBHOOK_EVENT_DEDUPE_CONFLICT_MESSAGE
    );

    Ok(())
}

async fn send_and_decode<T>(
    mcp: &mut TestAppServer,
    method: &str,
    params: serde_json::Value,
) -> Result<T>
where
    T: DeserializeOwned,
{
    to_response(send_raw(mcp, method, params).await?)
}

async fn send_raw(
    mcp: &mut TestAppServer,
    method: &str,
    params: serde_json::Value,
) -> Result<JSONRPCResponse> {
    let request_id = mcp.send_raw_request(method, Some(params)).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(response)
}
