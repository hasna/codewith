use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;

#[test]
fn map_api_error_maps_server_overloaded() {
    let err = map_api_error(ApiError::ServerOverloaded);
    assert!(matches!(err, CodexErr::ServerOverloaded));
}

#[test]
fn map_api_error_maps_server_overloaded_from_503_body() {
    let body = serde_json::json!({
        "error": {
            "code": "server_is_overloaded"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::SERVICE_UNAVAILABLE,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: None,
        body: Some(body),
    }));

    assert!(matches!(err, CodexErr::ServerOverloaded));
}

#[test]
fn map_api_error_maps_cyber_policy_from_400_body() {
    let body = serde_json::json!({
        "error": {
            "message": "This request has been flagged for potentially high-risk cyber activity.",
            "type": "invalid_request",
            "param": null,
            "code": "cyber_policy"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: None,
        body: Some(body),
    }));

    let CodexErr::CyberPolicy { message } = err else {
        panic!("expected CodexErr::CyberPolicy, got {err:?}");
    };
    assert_eq!(
        message,
        "This request has been flagged for potentially high-risk cyber activity."
    );
}

#[test]
fn map_api_error_maps_context_window_exceeded_from_400_body() {
    let body = serde_json::json!({
        "error": {
            "message": "Your input exceeds the context window of this model.",
            "type": "invalid_request_error",
            "code": "context_length_exceeded"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: Some("http://example.com/v1/responses/compact".to_string()),
        headers: None,
        body: Some(body),
    }));

    assert!(matches!(err, CodexErr::ContextWindowExceeded));
}

#[test]
fn map_api_error_maps_invalid_image_from_json_400_body() {
    let body = serde_json::json!({
        "error": {
            "message": "The image data you provided does not represent a valid image. Please check your input and try again.",
            "type": "invalid_request_error",
            "code": "invalid_value"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: None,
        body: Some(body),
    }));

    assert!(matches!(err, CodexErr::InvalidImageRequest()));
}

#[test]
fn map_api_error_maps_wrapped_websocket_cyber_policy_from_400_body() {
    let body = serde_json::json!({
        "type": "error",
        "status": 400,
        "error": {
            "message": "This websocket request was flagged.",
            "type": "invalid_request",
            "code": "cyber_policy"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: Some("ws://example.com/v1/responses".to_string()),
        headers: None,
        body: Some(body),
    }));

    let CodexErr::CyberPolicy { message } = err else {
        panic!("expected CodexErr::CyberPolicy, got {err:?}");
    };
    assert_eq!(message, "This websocket request was flagged.");
}

#[test]
fn map_api_error_uses_cyber_policy_fallback_for_missing_message() {
    let body = serde_json::json!({
        "error": {
            "code": "cyber_policy"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: None,
        body: Some(body),
    }));

    let CodexErr::CyberPolicy { message } = err else {
        panic!("expected CodexErr::CyberPolicy, got {err:?}");
    };
    assert_eq!(
        message,
        "This request has been flagged for possible cybersecurity risk."
    );
}

#[test]
fn map_api_error_keeps_unknown_400_errors_generic() {
    let body = serde_json::json!({
        "error": {
            "message": "Some other bad request.",
            "code": "some_other_policy"
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::BAD_REQUEST,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: None,
        body: Some(body.clone()),
    }));

    let CodexErr::InvalidRequest(message) = err else {
        panic!("expected CodexErr::InvalidRequest, got {err:?}");
    };
    assert_eq!(message, body);
}

#[test]
fn map_api_error_maps_usage_limit_limit_name_header() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACTIVE_LIMIT_HEADER,
        http::HeaderValue::from_static("codex_other"),
    );
    headers.insert(
        "x-codex-other-limit-name",
        http::HeaderValue::from_static("codex_other"),
    );
    let body = serde_json::json!({
        "error": {
            "type": "usage_limit_reached",
            "plan_type": "pro",
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::TOO_MANY_REQUESTS,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: Some(headers),
        body: Some(body),
    }));

    let CodexErr::UsageLimitReached(usage_limit) = err else {
        panic!("expected CodexErr::UsageLimitReached, got {err:?}");
    };
    assert_eq!(
        usage_limit
            .rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.limit_name.as_deref()),
        Some("codex_other")
    );
}

#[test]
fn map_api_error_does_not_fallback_limit_name_to_limit_id() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACTIVE_LIMIT_HEADER,
        http::HeaderValue::from_static("codex_other"),
    );
    let body = serde_json::json!({
        "error": {
            "type": "usage_limit_reached",
            "plan_type": "pro",
        }
    })
    .to_string();
    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::TOO_MANY_REQUESTS,
        url: Some("http://example.com/v1/responses".to_string()),
        headers: Some(headers),
        body: Some(body),
    }));

    let CodexErr::UsageLimitReached(usage_limit) = err else {
        panic!("expected CodexErr::UsageLimitReached, got {err:?}");
    };
    assert_eq!(
        usage_limit
            .rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.limit_name.as_deref()),
        None
    );
}

#[test]
fn map_api_error_ignores_unparseable_rate_limit_reached_type_headers() {
    let values = [
        http::HeaderValue::from_static("future_rate_limit_reached_type"),
        http::HeaderValue::from_bytes(&[0xff]).expect("valid opaque header value"),
    ];

    for value in values {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-rate-limit-reached-type", value);
        let body = serde_json::json!({
            "error": {
                "type": "usage_limit_reached",
                "plan_type": "pro",
            }
        })
        .to_string();
        let err = map_api_error(ApiError::Transport(TransportError::Http {
            status: http::StatusCode::TOO_MANY_REQUESTS,
            url: Some("http://example.com/v1/responses".to_string()),
            headers: Some(headers),
            body: Some(body),
        }));

        let CodexErr::UsageLimitReached(usage_limit) = err else {
            panic!("expected CodexErr::UsageLimitReached, got {err:?}");
        };
        assert_eq!(usage_limit.rate_limit_reached_type, None);
    }
}

#[test]
fn map_api_error_extracts_identity_auth_details_from_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(REQUEST_ID_HEADER, http::HeaderValue::from_static("req-401"));
    headers.insert(CF_RAY_HEADER, http::HeaderValue::from_static("ray-401"));
    headers.insert(
        X_OPENAI_AUTHORIZATION_ERROR_HEADER,
        http::HeaderValue::from_static("missing_authorization_header"),
    );
    let x_error_json =
        base64::engine::general_purpose::STANDARD.encode(r#"{"error":{"code":"token_expired"}}"#);
    headers.insert(
        X_ERROR_JSON_HEADER,
        http::HeaderValue::from_str(&x_error_json).expect("valid x-error-json header"),
    );

    let err = map_api_error(ApiError::Transport(TransportError::Http {
        status: http::StatusCode::UNAUTHORIZED,
        url: Some("https://chatgpt.com/backend-api/codex/models".to_string()),
        headers: Some(headers),
        body: Some(r#"{"detail":"Unauthorized"}"#.to_string()),
    }));

    let CodexErr::UnexpectedStatus(err) = err else {
        panic!("expected CodexErr::UnexpectedStatus, got {err:?}");
    };
    assert_eq!(err.request_id.as_deref(), Some("req-401"));
    assert_eq!(err.cf_ray.as_deref(), Some("ray-401"));
    assert_eq!(
        err.identity_authorization_error.as_deref(),
        Some("missing_authorization_header")
    );
    assert_eq!(err.identity_error_code.as_deref(), Some("token_expired"));
}

#[test]
fn map_api_error_redacts_reflected_credentials_and_preserves_diagnostics() {
    for credential_fragment in ["xy", "synthetic-credential-piece"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            REQUEST_ID_HEADER,
            http::HeaderValue::from_static("req-provider-auth-401"),
        );
        let body = serde_json::json!({
            "error": {
                "message": format!(
                    "Incorrect API key provided: {credential_fragment}."
                ),
                "type": "invalid_request_error",
                "code": "invalid_api_key"
            }
        })
        .to_string();

        let err = map_api_error(ApiError::Transport(TransportError::Http {
            status: http::StatusCode::UNAUTHORIZED,
            url: Some("https://example.com/v1/responses".to_string()),
            headers: Some(headers),
            body: Some(body),
        }));

        let CodexErr::UnexpectedStatus(err) = err else {
            panic!("expected CodexErr::UnexpectedStatus, got {err:?}");
        };
        assert!(
            !err.body.contains(credential_fragment),
            "mapped error body retained synthetic credential fragment {credential_fragment:?}: {}",
            err.body
        );

        let rendered = err.to_string();
        assert!(
            !rendered.contains(credential_fragment),
            "mapped error display retained synthetic credential fragment {credential_fragment:?}: {rendered}"
        );
        assert!(rendered.contains("401 Unauthorized"));
        assert!(rendered.contains("provider error code: invalid_api_key"));
        assert!(rendered.contains("request id: req-provider-auth-401"));
    }
}

#[test]
fn map_api_error_preserves_sanitized_auth_classification_on_nonstandard_status() {
    let credential_fragment = "xy";
    let mut headers = HeaderMap::new();
    headers.insert(
        REQUEST_ID_HEADER,
        http::HeaderValue::from_static("req-provider-auth-400"),
    );
    headers.insert(
        CF_RAY_HEADER,
        http::HeaderValue::from_static("ray-provider-auth-400"),
    );
    headers.insert(
        X_OPENAI_AUTHORIZATION_ERROR_HEADER,
        http::HeaderValue::from_static("invalid_api_key"),
    );
    let x_error_json =
        base64::engine::general_purpose::STANDARD.encode(r#"{"error":{"code":"token_expired"}}"#);
    headers.insert(
        X_ERROR_JSON_HEADER,
        http::HeaderValue::from_str(&x_error_json).expect("valid x-error-json header"),
    );
    let body = serde_json::json!({
        "error": {
            "message": format!(
                "Incorrect API key provided: {credential_fragment}."
            ),
            "type": "invalid_request_error",
            "code": format!("invalid_api_key_{credential_fragment}")
        }
    })
    .to_string();

    let transport = TransportError::http(
        http::StatusCode::BAD_REQUEST,
        Some("https://example.com/v1/responses".to_string()),
        Some(headers),
        Some(body),
    );
    let TransportError::Http { body, .. } = &transport else {
        panic!("expected HTTP transport error");
    };
    let stored_body = body.as_deref().expect("expected sanitized body");
    assert!(!stored_body.contains(credential_fragment));
    assert!(!stored_body.contains("invalid_api_key_xy"));
    assert!(stored_body.contains("authentication_error"));

    let codex_error = map_api_error(ApiError::Transport(transport));

    let CodexErr::UnexpectedStatus(err) = &codex_error else {
        panic!("expected CodexErr::UnexpectedStatus, got {codex_error:?}");
    };
    assert!(!err.body.contains(credential_fragment));
    let rendered = err.to_string();
    assert!(!rendered.contains(credential_fragment));
    assert!(rendered.contains("400 Bad Request"));
    assert!(rendered.contains("provider error code: authentication_error"));
    assert!(rendered.contains("request id: req-provider-auth-400"));
    assert!(rendered.contains("cf-ray: ray-provider-auth-400"));
    assert!(rendered.contains("auth error: invalid_api_key"));
    assert!(rendered.contains("auth error code: token_expired"));
    assert_eq!(err.request_id.as_deref(), Some("req-provider-auth-400"));
    assert_eq!(err.cf_ray.as_deref(), Some("ray-provider-auth-400"));
    assert_eq!(
        err.identity_authorization_error.as_deref(),
        Some("invalid_api_key")
    );
    assert_eq!(err.identity_error_code.as_deref(), Some("token_expired"));

    let protocol_event = codex_error.to_error_event(/*message_prefix*/ None);
    assert!(!protocol_event.message.contains(credential_fragment));
    assert!(!protocol_event.message.contains("invalid_api_key_xy"));
    assert!(protocol_event.message.contains("400 Bad Request"));
    assert!(
        protocol_event
            .message
            .contains("provider error code: authentication_error")
    );
    assert!(
        protocol_event
            .message
            .contains("request id: req-provider-auth-400")
    );
    assert!(
        protocol_event
            .message
            .contains("cf-ray: ray-provider-auth-400")
    );
    assert!(
        protocol_event
            .message
            .contains("auth error: invalid_api_key")
    );
    assert!(
        protocol_event
            .message
            .contains("auth error code: token_expired")
    );
}
