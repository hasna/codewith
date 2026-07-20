use std::sync::Arc;

use codex_agent_identity::AgentIdentityKey;
use codex_agent_identity::AgentTaskAuthorizationTarget;
use codex_agent_identity::authorization_header_for_agent_task;
use codex_api::AuthProvider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::error::CodexErr;
use codex_protocol::error::EnvVarError;
use http::HeaderMap;
use http::HeaderValue;

use crate::anthropic_auth_provider::AnthropicApiKeyAuthProvider;
use crate::bearer_auth_provider::BearerAuthProvider;

#[derive(Clone, Debug)]
struct AgentIdentityAuthProvider {
    auth: codex_login::auth::AgentIdentityAuth,
}

impl AuthProvider for AgentIdentityAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        let record = self.auth.record();
        let header_value = authorization_header_for_agent_task(
            AgentIdentityKey {
                agent_runtime_id: &record.agent_runtime_id,
                private_key_pkcs8_base64: &record.agent_private_key,
            },
            AgentTaskAuthorizationTarget {
                agent_runtime_id: &record.agent_runtime_id,
                task_id: self.auth.process_task_id(),
            },
        )
        .map_err(std::io::Error::other);

        if let Ok(header_value) = header_value
            && let Ok(header) = HeaderValue::from_str(&header_value)
        {
            let _ = headers.insert(http::header::AUTHORIZATION, header);
        }

        if let Ok(header) = HeaderValue::from_str(self.auth.account_id()) {
            let _ = headers.insert("ChatGPT-Account-ID", header);
        }

        if self.auth.is_fedramp_account() {
            let _ = headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
    }
}

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_openai_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

/// Returns the provider-scoped auth manager for command-backed auth, or the
/// base auth manager only for providers that explicitly require OpenAI auth.
pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    match provider.auth.clone() {
        Some(config) => Some(AuthManager::external_bearer_only(config)),
        None if provider.requires_openai_auth => auth_manager,
        None => None,
    }
}

pub(crate) fn resolve_provider_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    // The native Anthropic Messages API authenticates API keys via the
    // `x-api-key` header rather than `Authorization: Bearer`, so route the
    // resolved key through the Anthropic-specific auth provider. When no API key
    // is available (e.g. an `experimental_bearer_token` is configured instead)
    // fall through to the bearer path below.
    if provider.wire_api == WireApi::Anthropic
        && let Some(api_key) = provider_api_key(provider)?
    {
        return Ok(Arc::new(AnthropicApiKeyAuthProvider::new(api_key)));
    }
    if let Some(auth) = bearer_auth_for_provider(provider, MissingProviderKey::Error)? {
        return Ok(Arc::new(auth));
    }

    Ok(match auth {
        Some(auth) => auth_provider_from_auth(auth),
        None => unauthenticated_auth_provider(),
    })
}

pub(crate) fn resolve_provider_model_list_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if !provider.requires_openai_auth
        && (provider.env_key.is_some() || provider.experimental_bearer_token.is_some())
    {
        if let Some(auth) = bearer_auth_for_provider(provider, MissingProviderKey::AllowMissing)? {
            return Ok(Arc::new(auth));
        }
        return Ok(unauthenticated_auth_provider());
    }

    resolve_provider_auth(auth, provider)
}

/// Returns this provider's API key from supported runtime credential sources.
///
/// This deliberately returns `None` when a configured non-env token source can
/// satisfy auth instead, so callers can continue their fallback chain.
pub fn provider_api_key(
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<Option<String>> {
    provider_api_key_with_key_resolver(provider, |_| provider.api_key_if_available())
}

fn provider_api_key_with_key_resolver(
    provider: &ModelProviderInfo,
    provider_key: impl Fn(&str) -> Option<String>,
) -> codex_protocol::error::Result<Option<String>> {
    match provider.env_key.as_deref() {
        Some(env_key) => {
            if let Some(api_key) = provider_key(env_key) {
                return Ok(Some(api_key));
            }
            if provider.experimental_bearer_token.is_some() {
                return Ok(None);
            }
            Err(CodexErr::EnvVar(EnvVarError {
                var: env_key.to_string(),
                instructions: provider.env_key_instructions.clone(),
            }))
        }
        None => Ok(None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingProviderKey {
    Error,
    AllowMissing,
}

fn bearer_auth_for_provider_with_key_resolver(
    provider: &ModelProviderInfo,
    missing_provider_key: MissingProviderKey,
    provider_key: impl Fn(&str) -> Option<String>,
) -> codex_protocol::error::Result<Option<BearerAuthProvider>> {
    if let Some(env_key) = provider.env_key.as_deref() {
        if let Some(api_key) = provider_key(env_key) {
            return Ok(Some(BearerAuthProvider::new(api_key)));
        }
        if let Some(token) = provider.experimental_bearer_token.clone() {
            return Ok(Some(BearerAuthProvider::new(token)));
        }
        if missing_provider_key == MissingProviderKey::Error {
            return Err(CodexErr::EnvVar(EnvVarError {
                var: env_key.to_string(),
                instructions: provider.env_key_instructions.clone(),
            }));
        }
        return Ok(None);
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(BearerAuthProvider::new(token)));
    }

    Ok(None)
}

fn bearer_auth_for_provider(
    provider: &ModelProviderInfo,
    missing_provider_key: MissingProviderKey,
) -> codex_protocol::error::Result<Option<BearerAuthProvider>> {
    bearer_auth_for_provider_with_key_resolver(provider, missing_provider_key, |_| {
        provider.api_key_if_available()
    })
}

/// Builds request-header auth for a first-party Codewith auth snapshot.
pub fn auth_provider_from_auth(auth: &CodexAuth) -> SharedAuthProvider {
    match auth {
        CodexAuth::AgentIdentity(auth) => {
            Arc::new(AgentIdentityAuthProvider { auth: auth.clone() })
        }
        CodexAuth::ApiKey(_)
        | CodexAuth::Chatgpt(_)
        | CodexAuth::ChatgptAuthTokens(_)
        | CodexAuth::PersonalAccessToken(_) => Arc::new(BearerAuthProvider {
            token: auth.get_token().ok(),
            account_id: auth.get_account_id(),
            is_fedramp_account: auth.is_fedramp_account(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use codex_model_provider_info::WireApi;
    use codex_model_provider_info::create_oss_provider_with_base_url;
    use codex_protocol::error::CodexErr;
    use http::HeaderMap;
    use pretty_assertions::assert_eq;

    use super::*;

    fn authorization_header(auth: &BearerAuthProvider) -> Option<String> {
        let mut headers = HeaderMap::new();
        auth.add_auth_headers(&mut headers);
        headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    #[test]
    fn unauthenticated_auth_provider_adds_no_headers() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let auth = resolve_provider_auth(/*auth*/ None, &provider).expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }

    #[test]
    fn provider_api_key_uses_resolved_provider_key() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            experimental_bearer_token: Some("fallback-token".to_string()),
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let api_key = provider_api_key_with_key_resolver(&provider, |env_key| {
            assert_eq!(env_key, "CEREBRAS_API_KEY");
            Some("resolved-token".to_string())
        })
        .expect("provider API key should resolve");

        assert_eq!(api_key, Some("resolved-token".to_string()));
    }

    #[test]
    fn provider_api_key_defers_when_experimental_token_can_satisfy_auth() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            experimental_bearer_token: Some("fallback-token".to_string()),
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let api_key = provider_api_key_with_key_resolver(&provider, |_| None)
            .expect("missing provider key should defer to token fallback");

        assert_eq!(api_key, None);
    }

    #[test]
    fn provider_api_key_errors_when_required_provider_key_is_missing() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            env_key_instructions: Some("Set CEREBRAS_API_KEY.".to_string()),
            experimental_bearer_token: None,
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let err = match provider_api_key_with_key_resolver(&provider, |_| None) {
            Ok(_) => panic!("missing required provider key should error"),
            Err(err) => err,
        };

        match err {
            CodexErr::EnvVar(EnvVarError { var, instructions }) => {
                assert_eq!(var, "CEREBRAS_API_KEY");
                assert_eq!(instructions, Some("Set CEREBRAS_API_KEY.".to_string()));
            }
            other => panic!("expected EnvVar error, got {other:?}"),
        }
    }

    #[test]
    fn bearer_auth_uses_resolved_provider_key_before_experimental_token() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            experimental_bearer_token: Some("fallback-token".to_string()),
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let auth = bearer_auth_for_provider_with_key_resolver(
            &provider,
            MissingProviderKey::Error,
            |env_key| {
                assert_eq!(env_key, "CEREBRAS_API_KEY");
                Some("resolved-token".to_string())
            },
        )
        .expect("provider key should resolve")
        .expect("provider key should create bearer auth");

        assert_eq!(
            authorization_header(&auth),
            Some("Bearer resolved-token".to_string())
        );
    }

    #[test]
    fn bearer_auth_uses_experimental_token_after_missing_provider_key() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            experimental_bearer_token: Some("fallback-token".to_string()),
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let auth = bearer_auth_for_provider_with_key_resolver(
            &provider,
            MissingProviderKey::Error,
            |_| None,
        )
        .expect("fallback token should resolve")
        .expect("fallback token should create bearer auth");

        assert_eq!(
            authorization_header(&auth),
            Some("Bearer fallback-token".to_string())
        );
    }

    #[test]
    fn bearer_auth_errors_when_required_provider_key_is_missing() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            env_key_instructions: Some("Set CEREBRAS_API_KEY.".to_string()),
            experimental_bearer_token: None,
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let err = match bearer_auth_for_provider_with_key_resolver(
            &provider,
            MissingProviderKey::Error,
            |_| None,
        ) {
            Ok(_) => panic!("missing required provider key should error"),
            Err(err) => err,
        };

        match err {
            CodexErr::EnvVar(EnvVarError { var, instructions }) => {
                assert_eq!(var, "CEREBRAS_API_KEY");
                assert_eq!(instructions, Some("Set CEREBRAS_API_KEY.".to_string()));
            }
            other => panic!("expected EnvVar error, got {other:?}"),
        }
    }

    #[test]
    fn bearer_auth_allows_missing_provider_key_for_model_list_auth() {
        let provider = ModelProviderInfo {
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            experimental_bearer_token: None,
            ..ModelProviderInfo::create_cerebras_provider()
        };

        let auth = bearer_auth_for_provider_with_key_resolver(
            &provider,
            MissingProviderKey::AllowMissing,
            |_| None,
        )
        .expect("missing model list auth should be allowed");

        assert!(auth.is_none());
    }

    #[test]
    fn model_list_auth_does_not_use_openai_auth_for_provider_named_openai() {
        let provider = ModelProviderInfo {
            name: "OpenAI".to_string(),
            env_key: Some("CODEWITH_TEST_MISSING_PROVIDER_API_KEY".to_string()),
            experimental_bearer_token: None,
            requires_openai_auth: false,
            ..ModelProviderInfo::create_cerebras_provider()
        };
        let openai_auth = CodexAuth::from_api_key("openai-api-key");

        let auth = resolve_provider_model_list_auth(Some(&openai_auth), &provider)
            .expect("model list auth should resolve without OpenAI credentials");

        assert!(auth.to_auth_headers().is_empty());
    }
}
