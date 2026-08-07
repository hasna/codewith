use http::HeaderMap;
use http::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("http {status}: {body:?}")]
    Http {
        status: StatusCode,
        url: Option<String>,
        headers: Option<HeaderMap>,
        body: Option<String>,
    },
    #[error("retry limit reached")]
    RetryLimit,
    #[error("timeout")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("request build error: {0}")]
    Build(String),
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
}
