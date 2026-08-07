use http::HeaderMap;
use http::StatusCode;
use serde_json::Map;
use serde_json::Value;
use thiserror::Error;

const PROVIDER_AUTH_ERROR_MESSAGE: &str = "Authentication failed.";
const PROVIDER_ERROR_TOKEN_MAX_BYTES: usize = 128;

#[derive(Debug)]
pub enum TransportError {
    Http {
        status: StatusCode,
        url: Option<String>,
        headers: Option<HeaderMap>,
        body: Option<String>,
    },
    RetryLimit,
    Timeout,
    Network(String),
    Build(String),
}

impl TransportError {
    pub fn http(
        status: StatusCode,
        url: Option<String>,
        headers: Option<HeaderMap>,
        body: Option<String>,
    ) -> Self {
        Self::Http {
            status,
            url,
            headers,
            body: sanitize_http_error_body(status, body),
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http { status, body, .. } => {
                let body = sanitize_http_error_body(*status, body.clone());
                write!(f, "http {status}: {body:?}")
            }
            Self::RetryLimit => write!(f, "retry limit reached"),
            Self::Timeout => write!(f, "timeout"),
            Self::Network(message) => write!(f, "network error: {message}"),
            Self::Build(message) => write!(f, "request build error: {message}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Removes provider-reflected credentials from authentication error bodies.
///
/// Authentication failures are normalized to a fixed message. Recognized,
/// enum-like provider error codes remain available for diagnostics, while
/// arbitrary provider text is discarded before it can reach logs or traces.
pub fn sanitize_http_error_body(status: StatusCode, body: Option<String>) -> Option<String> {
    body.map(|body| {
        if is_http_auth_error(status, Some(&body)) {
            sanitized_provider_auth_error_body(&body)
        } else {
            body
        }
    })
}

pub fn is_http_auth_error(status: StatusCode, body: Option<&str>) -> bool {
    if status == StatusCode::UNAUTHORIZED {
        return true;
    }

    let Some(body) = body else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    provider_error_field(&value, "code").is_some_and(is_authentication_error_token)
        || provider_error_field(&value, "type").is_some_and(is_authentication_error_token)
}

fn sanitized_provider_auth_error_body(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let mut error = Map::new();
    error.insert(
        "message".to_string(),
        Value::String(PROVIDER_AUTH_ERROR_MESSAGE.to_string()),
    );

    if let Some(error_type) = parsed
        .as_ref()
        .and_then(|value| provider_error_field(value, "type"))
        .map(str::trim)
        .filter(|value| is_safe_provider_error_type(value))
    {
        error.insert("type".to_string(), Value::String(error_type.to_string()));
    }
    if let Some(code) = parsed
        .as_ref()
        .and_then(|value| provider_error_field(value, "code"))
        .map(str::trim)
        .filter(|value| is_safe_provider_error_code(value))
    {
        error.insert("code".to_string(), Value::String(code.to_string()));
    }

    let mut root = Map::new();
    root.insert("error".to_string(), Value::Object(error));
    Value::Object(root).to_string()
}

fn provider_error_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get("error")
        .and_then(|error| error.get(field))
        .or_else(|| value.get(field))
        .and_then(Value::as_str)
}

fn is_safe_provider_error_type(value: &str) -> bool {
    is_safe_provider_error_token(value)
        && (matches!(value, "api_error" | "invalid_request_error")
            || is_authentication_error_token(value))
}

fn is_safe_provider_error_code(value: &str) -> bool {
    is_safe_provider_error_token(value) && is_known_authentication_error_code(value)
}

fn is_safe_provider_error_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= PROVIDER_ERROR_TOKEN_MAX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn is_authentication_error_token(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("api_key")
        || value.contains("apikey")
        || value.contains("auth")
        || value.contains("credential")
        || value.contains("access_token")
        || value.contains("refresh_token")
        || value.contains("permission")
        || value.contains("unauthorized")
        || matches!(
            value.as_str(),
            "invalid_token" | "missing_token" | "token_expired" | "expired_token"
        )
}

fn is_known_authentication_error_code(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "invalid_api_key"
            | "invalid_api_key_error"
            | "invalidapikey"
            | "authentication_error"
            | "authorization_error"
            | "invalid_auth"
            | "invalid_authentication"
            | "invalid_authentication_token"
            | "invalidauthenticationtoken"
            | "invalid_credential"
            | "invalid_credentials"
            | "invalid_access_token"
            | "invalid_refresh_token"
            | "invalid_token"
            | "missing_token"
            | "token_expired"
            | "expired_token"
            | "unauthorized"
            | "unauthenticated"
            | "permission_denied"
            | "insufficient_permission"
            | "insufficient_permissions"
    )
}

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("stream failed: {0}")]
    Stream(String),
    #[error("timeout")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_transport_display_redacts_reflected_credentials() {
        for credential_fragment in ["xy", "synthetic-credential-piece"] {
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
            let error = TransportError::Http {
                status: StatusCode::UNAUTHORIZED,
                url: Some("https://example.com/v1/responses".to_string()),
                headers: None,
                body: Some(body),
            };

            let rendered = error.to_string();
            assert!(
                !rendered.contains(credential_fragment),
                "transport display retained synthetic credential fragment {credential_fragment:?}: {rendered}"
            );
            assert!(rendered.contains("401 Unauthorized"));
            assert!(rendered.contains("invalid_api_key"));
        }
    }

    #[test]
    fn http_constructor_sanitizes_auth_body_before_storage() {
        let credential_fragment = "xy";
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

        let error = TransportError::http(
            StatusCode::UNAUTHORIZED,
            Some("https://example.com/v1/responses".to_string()),
            None,
            Some(body),
        );
        let TransportError::Http { body, .. } = error else {
            panic!("expected HTTP transport error");
        };
        let body = body.expect("expected sanitized body");
        assert!(!body.contains(credential_fragment));
        assert!(body.contains(PROVIDER_AUTH_ERROR_MESSAGE));
        assert!(body.contains("invalid_api_key"));
    }

    #[test]
    fn http_constructor_sanitizes_auth_code_on_nonstandard_status() {
        let credential_fragment = "xy";
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

        let error = TransportError::http(
            StatusCode::BAD_REQUEST,
            Some("https://example.com/v1/responses".to_string()),
            None,
            Some(body),
        );
        let TransportError::Http { body, .. } = error else {
            panic!("expected HTTP transport error");
        };
        let body = body.expect("expected sanitized body");
        assert!(!body.contains(credential_fragment));
        assert!(body.contains(PROVIDER_AUTH_ERROR_MESSAGE));
        assert!(body.contains("invalid_api_key"));
    }

    #[test]
    fn http_constructor_drops_credential_like_provider_code() {
        let credential_fragment = "xy";
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

        let error = TransportError::http(
            StatusCode::BAD_REQUEST,
            Some("https://example.com/v1/responses".to_string()),
            None,
            Some(body),
        );
        let TransportError::Http { body, .. } = error else {
            panic!("expected HTTP transport error");
        };
        let body = body.expect("expected sanitized body");
        assert!(!body.contains(credential_fragment));
        assert!(body.contains(PROVIDER_AUTH_ERROR_MESSAGE));
        assert!(!body.contains("invalid_api_key_xy"));
    }

    #[test]
    fn http_constructor_preserves_non_auth_error_body() {
        let body = r#"{"error":{"message":"upstream unavailable","code":"server_error"}}"#;
        let error = TransportError::http(
            StatusCode::BAD_GATEWAY,
            Some("https://example.com/v1/responses".to_string()),
            None,
            Some(body.to_string()),
        );
        let TransportError::Http {
            body: stored_body, ..
        } = error
        else {
            panic!("expected HTTP transport error");
        };
        assert_eq!(stored_body.as_deref(), Some(body));
    }
}
