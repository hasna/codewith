use codex_api::AuthProvider;
use http::HeaderMap;
use http::HeaderValue;

/// Header Anthropic uses for API-key authentication on the native Messages API.
const ANTHROPIC_API_KEY_HEADER: &str = "x-api-key";

/// API-key auth provider for the native Anthropic Messages API.
///
/// Anthropic's `/v1/messages` endpoint authenticates API keys via the
/// `x-api-key` header rather than `Authorization: Bearer`, so the native
/// runtime adapter uses this provider instead of [`BearerAuthProvider`] when a
/// provider is configured with `wire_api = "anthropic"`.
///
/// [`BearerAuthProvider`]: crate::bearer_auth_provider::BearerAuthProvider
#[derive(Clone, Debug)]
pub struct AnthropicApiKeyAuthProvider {
    api_key: String,
}

impl AnthropicApiKeyAuthProvider {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

impl AuthProvider for AnthropicApiKeyAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        if let Ok(header) = HeaderValue::from_str(&self.api_key) {
            let _ = headers.insert(ANTHROPIC_API_KEY_HEADER, header);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn anthropic_auth_provider_sets_x_api_key_header() {
        let auth = AnthropicApiKeyAuthProvider::new("secret-key".to_string());
        let mut headers = HeaderMap::new();

        auth.add_auth_headers(&mut headers);

        assert_eq!(
            headers
                .get(ANTHROPIC_API_KEY_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("secret-key")
        );
        // Anthropic API keys are not bearer tokens.
        assert!(!headers.contains_key(http::header::AUTHORIZATION));
    }
}
