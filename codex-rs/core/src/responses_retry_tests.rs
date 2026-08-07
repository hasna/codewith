use super::ResponsesStreamRequest;
use super::log_retry;
use crate::session::tests::make_session_and_context;
use codex_api::ApiError;
use codex_api::TransportError;
use codex_api::map_api_error;
use codex_protocol::error::CodexErr;
use http::HeaderMap;
use std::time::Duration;
use tracing_test::internal::MockWriter;

#[tokio::test]
async fn sampling_retry_logs_stream_error_context() {
    let (_session, turn_context) = make_session_and_context().await;
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    log_retry(
        ResponsesStreamRequest::Sampling,
        &turn_context,
        &CodexErr::Stream(
            "websocket closed by server before response.completed".to_string(),
            None,
        ),
        /*retries*/ 2,
        /*max_retries*/ 5,
        Duration::from_secs(1),
    );

    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("retry log should be valid utf-8");
    assert!(logs.contains("stream disconnected - retrying sampling request"));
    assert!(logs.contains(&format!("turn_id={}", turn_context.sub_id)));
    assert!(logs.contains("retries=2"));
    assert!(logs.contains("max_retries=5"));
    assert!(logs.contains(
        "sampling_error=stream disconnected before completion: websocket closed by server before response.completed"
    ));
}

#[tokio::test]
async fn sampling_retry_redacts_reflected_auth_error_context() {
    let (_session, turn_context) = make_session_and_context().await;
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    let credential_fragment = "q7";
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-request-id",
        http::HeaderValue::from_static("req-retry-401"),
    );
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::UNAUTHORIZED,
        url: Some("https://example.com/v1/responses".to_string()),
        headers: Some(headers),
        body: Some(
            serde_json::json!({
                "error": {
                    "message": format!(
                        "Incorrect API key provided: {credential_fragment}."
                    ),
                    "type": "invalid_request_error",
                    "code": "invalid_api_key"
                }
            })
            .to_string(),
        ),
    }));

    log_retry(
        ResponsesStreamRequest::Sampling,
        &turn_context,
        &err,
        /*retries*/ 2,
        /*max_retries*/ 5,
        Duration::from_secs(1),
    );

    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("retry log should be valid utf-8");
    assert!(
        !logs.contains(credential_fragment),
        "retry log retained synthetic credential fragment: {logs}"
    );
    assert!(logs.contains("401 Unauthorized"));
    assert!(logs.contains("provider error code: invalid_api_key"));
    assert!(logs.contains("request id: req-retry-401"));

    let fallback_summary =
        format!("Falling back from WebSockets to HTTPS transport. {err:#}");
    assert!(!fallback_summary.contains(credential_fragment));
    assert!(fallback_summary.contains("provider error code: invalid_api_key"));
    assert!(fallback_summary.contains("request id: req-retry-401"));
}
