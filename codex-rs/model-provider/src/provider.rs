use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_models_manager::manager::BundledModelCatalog;
use codex_models_manager::manager::OpenAiModelsManager;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_protocol::account::ProviderAccount;
use codex_protocol::openai_models::ModelsResponse;
use sha2::Digest as _;
use sha2::Sha256;

use crate::amazon_bedrock::AmazonBedrockModelProvider;
use crate::auth::auth_manager_for_provider;
use crate::auth::resolve_provider_auth;
use crate::models_endpoint::OpenAiModelsEndpoint;

/// Optional provider-backed features that Codewith may expose at runtime.
///
/// These capabilities are a provider-owned upper bound. Callers can disable
/// more functionality through normal config, but should not expose a feature
/// that the active provider marks unsupported here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub namespace_tools: bool,
    pub image_generation: bool,
    pub web_search: bool,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            namespace_tools: true,
            image_generation: true,
            web_search: true,
        }
    }
}

/// Current app-visible account state for a model provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAccountState {
    pub account: Option<ProviderAccount>,
    pub requires_openai_auth: bool,
}

/// Error returned when a provider cannot construct its app-visible account state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAccountError {
    MissingChatgptAccountDetails,
}

impl fmt::Display for ProviderAccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingChatgptAccountDetails => {
                write!(
                    f,
                    "email and plan type are required for chatgpt authentication"
                )
            }
        }
    }
}

impl std::error::Error for ProviderAccountError {}

pub type ProviderAccountResult = std::result::Result<ProviderAccountState, ProviderAccountError>;

/// Default model used for automatic approval review when a provider does not
/// require a backend-specific model ID.
pub const DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL: &str = "codex-auto-review";

/// Runtime provider abstraction used by model execution.
///
/// Implementations own provider-specific behavior for a model backend. The
/// `ModelProviderInfo` returned by `info` is the serialized/configured provider
/// metadata used by the default OpenAI-compatible implementation.
#[async_trait::async_trait]
pub trait ModelProvider: fmt::Debug + Send + Sync {
    /// Returns the configured provider metadata.
    fn info(&self) -> &ModelProviderInfo;

    /// Returns the provider-owned capability upper bounds.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Returns the preferred model used for automatic approval review.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn approval_review_preferred_model(&self) -> &'static str {
        DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
    }

    /// Returns whether requests made through this provider should include attestation.
    fn supports_attestation(&self) -> bool {
        false
    }

    /// Returns the provider-scoped auth manager, when this provider uses one.
    ///
    /// TODO(celia-oai): Make auth manager access internal to this crate so callers
    /// resolve provider-specific auth only through `ModelProvider`. We first need
    /// to think through whether Codewith should have a unified provider-specific auth
    /// manager throughout the codebase; that is a larger refactor than this change.
    fn auth_manager(&self) -> Option<Arc<AuthManager>>;

    /// Returns the current provider-scoped auth value, if one is configured.
    async fn auth(&self) -> Option<CodexAuth>;

    /// Returns the current app-visible account state for this provider.
    fn account_state(&self) -> ProviderAccountResult;

    /// Returns provider configuration adapted for the API client.
    async fn api_provider(&self) -> codex_protocol::error::Result<Provider> {
        let auth = self.auth().await;
        self.info()
            .to_api_provider(auth.as_ref().map(CodexAuth::auth_mode))
    }

    /// Returns the provider base URL that will be used at request time.
    async fn runtime_base_url(&self) -> codex_protocol::error::Result<Option<String>> {
        Ok(self.info().base_url.clone())
    }

    /// Returns the auth provider used to attach request credentials.
    async fn api_auth(&self) -> codex_protocol::error::Result<SharedAuthProvider> {
        let auth = self.auth().await;
        resolve_provider_auth(auth.as_ref(), self.info())
    }

    /// Creates the model manager implementation appropriate for this provider.
    fn models_manager(
        &self,
        codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager;
}

/// Shared runtime model provider handle.
pub type SharedModelProvider = Arc<dyn ModelProvider>;

/// Creates the default runtime model provider for configured provider metadata.
pub fn create_model_provider(
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
) -> SharedModelProvider {
    let provider_id = default_provider_id(&provider_info);
    create_model_provider_with_id(provider_id, provider_info, auth_manager)
}

fn default_provider_id(provider_info: &ModelProviderInfo) -> String {
    if provider_info.requires_openai_auth && provider_info.is_openai() {
        OPENAI_PROVIDER_ID.to_string()
    } else {
        provider_info.name.clone()
    }
}

/// Creates the default runtime model provider for configured provider metadata
/// with a stable provider identifier for provider-scoped caches.
pub fn create_model_provider_with_id(
    provider_id: impl Into<String>,
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
) -> SharedModelProvider {
    if provider_info.is_amazon_bedrock() {
        Arc::new(AmazonBedrockModelProvider::new(provider_info))
    } else {
        Arc::new(ConfiguredModelProvider::new(
            provider_id.into(),
            provider_info,
            auth_manager,
        ))
    }
}

/// Runtime model provider backed by configured `ModelProviderInfo`.
#[derive(Clone, Debug)]
struct ConfiguredModelProvider {
    provider_id: String,
    info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl ConfiguredModelProvider {
    fn new(
        provider_id: String,
        provider_info: ModelProviderInfo,
        auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        let auth_manager = auth_manager_for_provider(auth_manager, &provider_info);
        Self {
            provider_id,
            info: provider_info,
            auth_manager,
        }
    }
}

/// Return the provider-scoped model cache key used by runtime model managers.
///
/// The `auth_manager` argument should already be scoped to the provider. For
/// providers that do not use an auth manager, pass `None` so the key is derived
/// from provider-owned credentials.
pub fn model_cache_key_for_provider(
    provider_id: &str,
    provider_info: &ModelProviderInfo,
    auth_manager: Option<&AuthManager>,
) -> String {
    let provider_fragment = provider_configuration_cache_key_fragment(provider_info);
    let auth_fragment = match auth_manager {
        Some(auth_manager) => auth_manager_cache_key_fragment(auth_manager),
        None => provider_credential_cache_key_fragment(provider_info),
    };

    format!("{provider_id}|{provider_fragment}|{auth_fragment}")
}

/// Returns the effective model-manager cache key for a configured provider.
///
/// This applies the same provider-auth scoping that runtime model providers use
/// before deriving the cache key, so callers do not accidentally key
/// external-key providers on the ambient OpenAI auth manager.
pub fn model_cache_key_for_configured_provider(
    provider_id: &str,
    provider_info: &ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
) -> String {
    let auth_manager = auth_manager_for_provider(auth_manager, provider_info);
    model_cache_key_for_provider(provider_id, provider_info, auth_manager.as_deref())
}

fn auth_manager_cache_key_fragment(auth_manager: &AuthManager) -> String {
    let profile_fragment = auth_manager
        .selected_auth_profile()
        .map(|profile| credential_cache_key_fragment("profile", &profile))
        .unwrap_or_else(|| "profile:default".to_string());
    let auth_fragment = match auth_manager.auth_cached() {
        Some(auth) => auth_cache_key_fragment(&auth),
        None if auth_manager.has_external_auth() => "auth:external".to_string(),
        None => "auth:none".to_string(),
    };

    format!("{profile_fragment}|{auth_fragment}")
}

fn provider_configuration_cache_key_fragment(provider_info: &ModelProviderInfo) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codewith-model-provider-config-v1\0");
    hash_field(&mut hasher, "name", &provider_info.name);
    hash_optional_field(&mut hasher, "base_url", provider_info.base_url.as_deref());
    hash_optional_field(&mut hasher, "env_key", provider_info.env_key.as_deref());
    hash_optional_field(
        &mut hasher,
        "env_key_instructions",
        provider_info.env_key_instructions.as_deref(),
    );
    hash_optional_secret_field(
        &mut hasher,
        "experimental_bearer_token",
        provider_info.experimental_bearer_token.as_deref(),
    );
    hash_debug_field(&mut hasher, "auth", &provider_info.auth);
    hash_debug_field(&mut hasher, "aws", &provider_info.aws);
    hash_field(&mut hasher, "wire_api", &provider_info.wire_api.to_string());
    hash_string_map(
        &mut hasher,
        "query_params",
        provider_info.query_params.as_ref(),
    );
    hash_secret_string_map(
        &mut hasher,
        "http_headers",
        provider_info.http_headers.as_ref(),
    );
    hash_env_header_map(
        &mut hasher,
        "env_http_headers",
        provider_info.env_http_headers.as_ref(),
    );
    hash_optional_field(
        &mut hasher,
        "request_max_retries",
        provider_info
            .request_max_retries
            .map(|value| value.to_string())
            .as_deref(),
    );
    hash_optional_field(
        &mut hasher,
        "stream_max_retries",
        provider_info
            .stream_max_retries
            .map(|value| value.to_string())
            .as_deref(),
    );
    hash_optional_field(
        &mut hasher,
        "stream_idle_timeout_ms",
        provider_info
            .stream_idle_timeout_ms
            .map(|value| value.to_string())
            .as_deref(),
    );
    hash_optional_field(
        &mut hasher,
        "websocket_connect_timeout_ms",
        provider_info
            .websocket_connect_timeout_ms
            .map(|value| value.to_string())
            .as_deref(),
    );
    hash_field(
        &mut hasher,
        "requires_openai_auth",
        &provider_info.requires_openai_auth.to_string(),
    );
    hash_field(
        &mut hasher,
        "supports_websockets",
        &provider_info.supports_websockets.to_string(),
    );

    format!("provider-config:{:x}", hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    hasher.update(b"\0");
}

fn hash_optional_field(hasher: &mut Sha256, label: &str, value: Option<&str>) {
    match value {
        Some(value) => hash_field(hasher, label, value),
        None => hash_field(hasher, label, "<none>"),
    }
}

fn hash_optional_secret_field(hasher: &mut Sha256, label: &str, value: Option<&str>) {
    let value = value.map(credential_fingerprint);
    hash_optional_field(hasher, label, value.as_deref());
}

fn hash_debug_field<T: fmt::Debug>(hasher: &mut Sha256, label: &str, value: &T) {
    hash_field(hasher, label, &format!("{value:?}"));
}

fn hash_string_map(
    hasher: &mut Sha256,
    label: &str,
    value: Option<&std::collections::HashMap<String, String>>,
) {
    hash_field(hasher, label, "<map>");
    let Some(value) = value else {
        hash_field(hasher, "present", "false");
        return;
    };
    hash_field(hasher, "present", "true");
    let mut entries = value.iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(right.1)));
    for (key, value) in entries {
        hash_field(hasher, key, value);
    }
}

fn hash_secret_string_map(
    hasher: &mut Sha256,
    label: &str,
    value: Option<&std::collections::HashMap<String, String>>,
) {
    hash_field(hasher, label, "<map>");
    let Some(value) = value else {
        hash_field(hasher, "present", "false");
        return;
    };
    hash_field(hasher, "present", "true");
    let mut entries = value.iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(right.1)));
    for (key, value) in entries {
        hash_field(hasher, key, &credential_fingerprint(value));
    }
}

fn hash_env_header_map(
    hasher: &mut Sha256,
    label: &str,
    value: Option<&std::collections::HashMap<String, String>>,
) {
    hash_field(hasher, label, "<map>");
    let Some(value) = value else {
        hash_field(hasher, "present", "false");
        return;
    };
    hash_field(hasher, "present", "true");
    let mut entries = value.iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(right.1)));
    for (header, env_var) in entries {
        hash_field(hasher, header, env_var);
        if let Ok(value) = std::env::var(env_var) {
            hash_field(hasher, "env-value", &credential_fingerprint(&value));
        }
    }
}

fn auth_cache_key_fragment(auth: &CodexAuth) -> String {
    if auth.is_api_key_auth() {
        auth.api_key()
            .map(|api_key| credential_cache_key_fragment("auth:api-key", api_key))
            .unwrap_or_else(|| "auth:api-key:missing".to_string())
    } else {
        let account_fragment = auth
            .get_account_id()
            .map(|account_id| credential_cache_key_fragment("account", &account_id))
            .unwrap_or_else(|| "account:unknown".to_string());
        let auth_mode = if auth.is_external_chatgpt_tokens() {
            "chatgpt-auth-tokens"
        } else if auth.is_chatgpt_auth() {
            "chatgpt"
        } else {
            "agent-identity"
        };
        format!("auth:{auth_mode}:{account_fragment}")
    }
}

fn provider_credential_cache_key_fragment(provider_info: &ModelProviderInfo) -> String {
    if let Some(api_key) = provider_info.api_key_if_available() {
        return credential_cache_key_fragment("provider-key", &api_key);
    }
    if let Some(token) = provider_info.experimental_bearer_token.as_deref() {
        return credential_cache_key_fragment("provider-token", token);
    }
    if provider_info.env_key.is_some() {
        return "provider-key:missing".to_string();
    }

    "auth:none".to_string()
}

fn credential_cache_key_fragment(prefix: &str, credential: &str) -> String {
    format!("{prefix}:{}", credential_fingerprint(credential))
}

fn credential_fingerprint(credential: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codewith-model-cache-key-v1\0");
    hasher.update(credential.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[async_trait::async_trait]
impl ModelProvider for ConfiguredModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.auth_manager.clone()
    }

    fn supports_attestation(&self) -> bool {
        self.auth_manager
            .as_ref()
            .and_then(|auth_manager| auth_manager.auth_cached())
            .is_some_and(|auth| auth.is_chatgpt_auth())
    }

    async fn auth(&self) -> Option<CodexAuth> {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager.auth().await,
            None => None,
        }
    }

    fn account_state(&self) -> ProviderAccountResult {
        let account = if self.info.requires_openai_auth {
            self.auth_manager
                .as_ref()
                .and_then(|auth_manager| {
                    let auth = auth_manager.auth_cached()?;
                    if auth_manager.refresh_failure_for_auth(&auth).is_some() {
                        return None;
                    }
                    Some(auth)
                })
                .map(|auth| match &auth {
                    CodexAuth::ApiKey(_) => Ok(ProviderAccount::ApiKey),
                    CodexAuth::Chatgpt(_)
                    | CodexAuth::ChatgptAuthTokens(_)
                    | CodexAuth::AgentIdentity(_)
                    | CodexAuth::PersonalAccessToken(_) => {
                        let email = auth.get_account_email();
                        let plan_type = auth.account_plan_type();

                        match (email, plan_type) {
                            (Some(email), Some(plan_type)) => {
                                Ok(ProviderAccount::Chatgpt { email, plan_type })
                            }
                            _ => Err(ProviderAccountError::MissingChatgptAccountDetails),
                        }
                    }
                })
                .transpose()?
        } else {
            None
        };

        Ok(ProviderAccountState {
            account,
            requires_openai_auth: self.info.requires_openai_auth,
        })
    }

    async fn api_provider(&self) -> codex_protocol::error::Result<Provider> {
        let auth = self.auth().await;
        let mut provider = self
            .info
            .to_api_provider(auth.as_ref().map(CodexAuth::auth_mode))?;
        provider.provider_id = Some(self.provider_id.clone());
        Ok(provider)
    }

    fn models_manager(
        &self,
        codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        match config_model_catalog {
            Some(model_catalog) => Arc::new(StaticModelsManager::new(
                self.auth_manager.clone(),
                model_catalog,
            )),
            None => {
                let endpoint = Arc::new(OpenAiModelsEndpoint::new(
                    self.provider_id.clone(),
                    self.info.clone(),
                    self.auth_manager.clone(),
                ));
                let bundled_model_catalog = if self.provider_id == OPENAI_PROVIDER_ID {
                    BundledModelCatalog::UseAsFallback
                } else {
                    BundledModelCatalog::Disabled
                };
                let model_cache_key = model_cache_key_for_provider(
                    &self.provider_id,
                    &self.info,
                    self.auth_manager.as_deref(),
                );
                Arc::new(OpenAiModelsManager::new(
                    codex_home,
                    model_cache_key,
                    bundled_model_catalog,
                    endpoint,
                    self.auth_manager.clone(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use codex_model_provider_info::ModelProviderAwsAuthInfo;
    use codex_model_provider_info::OPENROUTER_PROVIDER_ID;
    use codex_model_provider_info::WireApi;
    use codex_models_manager::manager::RefreshStrategy;
    use codex_protocol::config_types::ModelProviderAuthInfo;
    use codex_protocol::openai_models::ModelInfo;
    use codex_protocol::openai_models::ModelsResponse;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header_regex;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    use super::*;

    fn provider_info_with_command_auth() -> ModelProviderInfo {
        ModelProviderInfo {
            auth: Some(ModelProviderAuthInfo {
                command: "print-token".to_string(),
                args: Vec::new(),
                timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
                refresh_interval_ms: 300_000,
                cwd: std::env::current_dir()
                    .expect("current dir should be available")
                    .try_into()
                    .expect("current dir should be absolute"),
            }),
            requires_openai_auth: false,
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        }
    }

    fn test_codex_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("codex-model-provider-test-{}", std::process::id()))
    }

    fn provider_for(base_url: String) -> ModelProviderInfo {
        ModelProviderInfo {
            name: "mock".into(),
            base_url: Some(base_url),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(0),
            stream_max_retries: Some(0),
            stream_idle_timeout_ms: Some(5_000),
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    fn remote_model(slug: &str) -> ModelInfo {
        serde_json::from_value(json!({
            "slug": slug,
            "display_name": slug,
            "description": null,
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [],
            "shell_type": "shell_command",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 0,
            "upgrade": null,
            "base_instructions": "base instructions",
            "supports_reasoning_summaries": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10_000},
            "supports_parallel_tool_calls": false,
            "supports_image_detail_original": false,
            "context_window": 272_000,
            "max_context_window": 272_000,
            "experimental_supported_tools": [],
        }))
        .expect("valid model")
    }

    #[test]
    fn configured_provider_uses_default_capabilities() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(provider.capabilities(), ProviderCapabilities::default());
    }

    #[test]
    fn configured_provider_uses_default_approval_review_preferred_model() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.approval_review_preferred_model(),
            DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
        );
    }

    #[tokio::test]
    async fn configured_provider_runtime_base_url_uses_configured_base_url() {
        let provider = create_model_provider(
            provider_for("https://example.test/v1".to_string()),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider
                .runtime_base_url()
                .await
                .expect("runtime base URL should resolve"),
            Some("https://example.test/v1".to_string())
        );
    }

    #[test]
    fn create_model_provider_builds_command_auth_manager_without_base_manager() {
        let provider = create_model_provider(
            provider_info_with_command_auth(),
            /*auth_manager*/ None,
        );

        let auth_manager = provider
            .auth_manager()
            .expect("command auth provider should have an auth manager");

        assert!(auth_manager.has_external_auth());
    }

    #[test]
    fn create_model_provider_uses_openai_auth_manager_for_openai_provider() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert!(provider.auth_manager().is_some());
    }

    #[test]
    fn create_model_provider_uses_openai_auth_manager_for_provider_that_requires_openai_auth() {
        let provider = create_model_provider(
            ModelProviderInfo {
                name: "OpenAI-compatible".to_string(),
                base_url: Some("https://example.test/v1".to_string()),
                wire_api: WireApi::Responses,
                requires_openai_auth: true,
                ..Default::default()
            },
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert!(provider.auth_manager().is_some());
    }

    #[test]
    fn create_model_provider_does_not_use_openai_auth_manager_for_builtin_external_key_providers() {
        for provider_info in [
            ModelProviderInfo::create_anthropic_provider(),
            ModelProviderInfo::create_cerebras_provider(),
            ModelProviderInfo::create_nvidia_provider(),
            ModelProviderInfo::create_openrouter_provider(),
            ModelProviderInfo::create_xiaomi_provider(),
        ] {
            let provider = create_model_provider(
                provider_info,
                Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                    "openai-api-key",
                ))),
            );

            assert!(provider.auth_manager().is_none());
        }
    }

    #[test]
    fn create_model_provider_does_not_use_openai_auth_manager_for_provider_named_openai() {
        let provider = create_model_provider(
            ModelProviderInfo {
                name: "OpenAI".to_string(),
                base_url: Some("https://not-openai.example.test/v1".to_string()),
                wire_api: WireApi::Responses,
                requires_openai_auth: false,
                ..Default::default()
            },
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert!(provider.auth_manager().is_none());
    }

    #[test]
    fn model_cache_key_uses_auth_profile_and_api_key_fingerprint() {
        let provider_info = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        let first_auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("first-openai-api-key"));
        let second_auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("second-openai-api-key"));

        let first = model_cache_key_for_provider(
            OPENAI_PROVIDER_ID,
            &provider_info,
            Some(first_auth_manager.as_ref()),
        );
        let second = model_cache_key_for_provider(
            OPENAI_PROVIDER_ID,
            &provider_info,
            Some(second_auth_manager.as_ref()),
        );

        assert_ne!(first, second);
        assert!(!first.contains("first-openai-api-key"));
        assert!(!second.contains("second-openai-api-key"));
    }

    #[test]
    fn model_cache_key_uses_provider_credential_fingerprint() {
        let provider_info = ModelProviderInfo {
            env_key: None,
            experimental_bearer_token: Some("raw-provider-secret".to_string()),
            ..ModelProviderInfo::create_openrouter_provider()
        };

        let key =
            model_cache_key_for_provider("openrouter", &provider_info, /*auth_manager*/ None);

        assert!(key.contains("provider-token:"));
        assert!(!key.contains("raw-provider-secret"));
    }

    #[test]
    fn model_cache_key_uses_provider_base_url_fingerprint() {
        let first_provider = ModelProviderInfo {
            base_url: Some("https://first.example.test/v1".to_string()),
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        };
        let second_provider = ModelProviderInfo {
            base_url: Some("https://second.example.test/v1".to_string()),
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        };
        let auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("openai-api-key"));

        let first = model_cache_key_for_provider(
            OPENAI_PROVIDER_ID,
            &first_provider,
            Some(auth_manager.as_ref()),
        );
        let second = model_cache_key_for_provider(
            OPENAI_PROVIDER_ID,
            &second_provider,
            Some(auth_manager.as_ref()),
        );

        assert_ne!(first, second);
        assert!(!first.contains("first.example.test"));
        assert!(!second.contains("second.example.test"));
        assert!(!first.contains("openai-api-key"));
    }

    #[test]
    fn configured_provider_cache_key_ignores_ambient_openai_auth_for_external_key_provider() {
        let provider_info = ModelProviderInfo {
            env_key: None,
            experimental_bearer_token: Some("raw-provider-secret".to_string()),
            ..ModelProviderInfo::create_openrouter_provider()
        };
        let openai_auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("openai-api-key"));

        let key_with_ambient_auth = model_cache_key_for_configured_provider(
            OPENROUTER_PROVIDER_ID,
            &provider_info,
            Some(openai_auth_manager),
        );
        let key_without_auth = model_cache_key_for_provider(
            OPENROUTER_PROVIDER_ID,
            &provider_info,
            /*auth_manager*/ None,
        );

        assert_eq!(key_with_ambient_auth, key_without_auth);
        assert!(key_with_ambient_auth.contains("provider-token:"));
        assert!(!key_with_ambient_auth.contains("openai-api-key"));
        assert!(!key_with_ambient_auth.contains("raw-provider-secret"));
    }

    #[tokio::test]
    async fn configured_openrouter_mirror_api_provider_preserves_provider_id() {
        let provider_info = ModelProviderInfo {
            name: "OpenRouter Mirror".to_string(),
            base_url: Some("https://openrouter-mirror.example.test/v1".to_string()),
            ..ModelProviderInfo::create_openrouter_provider()
        };
        let provider = create_model_provider_with_id(
            OPENROUTER_PROVIDER_ID,
            provider_info,
            /*auth_manager*/ None,
        );

        let api_provider = provider.api_provider().await.expect("api provider");

        assert_eq!(
            api_provider.provider_id.as_deref(),
            Some(OPENROUTER_PROVIDER_ID)
        );
        assert!(api_provider.is_openrouter_endpoint());
    }

    #[test]
    fn create_model_provider_does_not_use_openai_auth_manager_for_amazon_bedrock_provider() {
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(Some(ModelProviderAwsAuthInfo {
                profile: Some("codex-bedrock".to_string()),
                region: None,
            })),
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert!(provider.auth_manager().is_none());
    }

    #[test]
    fn openai_provider_returns_unauthenticated_openai_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: None,
                requires_openai_auth: true,
            })
        );
    }

    #[test]
    fn openai_provider_returns_api_key_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
                "openai-api-key",
            ))),
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: Some(ProviderAccount::ApiKey),
                requires_openai_auth: true,
            })
        );
    }

    #[test]
    fn openai_provider_rejects_chatgpt_account_state_without_email() {
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
        );

        assert_eq!(
            provider.account_state(),
            Err(ProviderAccountError::MissingChatgptAccountDetails)
        );
    }

    #[test]
    fn custom_non_openai_provider_returns_no_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo {
                name: "Custom".to_string(),
                base_url: Some("http://localhost:1234/v1".to_string()),
                wire_api: WireApi::Responses,
                requires_openai_auth: false,
                ..Default::default()
            },
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: None,
                requires_openai_auth: false,
            })
        );
    }

    #[test]
    fn amazon_bedrock_provider_returns_bedrock_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: Some(ProviderAccount::AmazonBedrock),
                requires_openai_auth: false,
            })
        );
    }

    #[tokio::test]
    async fn amazon_bedrock_provider_creates_static_models_manager() {
        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );
        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);

        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;
        let model_ids = catalog
            .models
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>();

        assert_eq!(model_ids, vec!["openai.gpt-5.5", "openai.gpt-5.4"]);

        let default_model = manager
            .list_models(RefreshStrategy::Online)
            .await
            .into_iter()
            .find(|preset| preset.is_default)
            .expect("Bedrock catalog should have a default model");

        assert_eq!(default_model.model, "openai.gpt-5.5");
    }

    #[tokio::test]
    async fn configured_bedrock_catalog_only_allows_default_service_tier() {
        let configured_model = codex_models_manager::bundled_models_response()
            .expect("bundled models should parse")
            .models
            .into_iter()
            .find(|model| model.slug == "gpt-5.5")
            .expect("bundled models should include GPT-5.5");
        assert!(!configured_model.additional_speed_tiers.is_empty());
        assert!(!configured_model.service_tiers.is_empty());

        let provider = create_model_provider(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );
        let manager = provider.models_manager(
            test_codex_home(),
            Some(ModelsResponse {
                models: vec![configured_model],
            }),
        );

        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].slug, "gpt-5.5");
        assert_eq!(
            catalog.models[0].additional_speed_tiers,
            Vec::<String>::new()
        );
        assert_eq!(catalog.models[0].service_tiers, Vec::new());
        assert_eq!(catalog.models[0].default_service_tier, None);
    }

    #[tokio::test]
    async fn configured_provider_models_manager_uses_provider_bearer_token() {
        let server = MockServer::start().await;
        let remote_models = vec![remote_model("provider-model")];

        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header_regex("Authorization", "Bearer provider-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(ModelsResponse {
                        models: remote_models.clone(),
                    }),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut provider_info = provider_for(server.uri());
        provider_info.experimental_bearer_token = Some("provider-token".to_string());
        let provider = create_model_provider(
            provider_info,
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
        );

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert!(
            catalog
                .models
                .iter()
                .any(|model| model.slug == "provider-model")
        );
    }

    #[tokio::test]
    async fn custom_provider_named_openai_does_not_use_bundled_model_catalog() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(ModelsResponse { models: Vec::new() }),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider_info = ModelProviderInfo {
            name: "OpenAI".to_string(),
            experimental_bearer_token: Some("provider-token".to_string()),
            ..provider_for(server.uri())
        };
        let provider = create_model_provider_with_id(
            "custom-openai-name",
            provider_info,
            /*auth_manager*/ None,
        );

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(catalog.models, Vec::new());
    }

    #[tokio::test]
    async fn cerebras_provider_models_manager_preserves_authenticated_model_ids() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header_regex("Authorization", "Bearer cerebras-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    {
                        "id": "gpt-oss-120b",
                        "object": "model",
                        "created": 0,
                        "owned_by": "Cerebras"
                    },
                    {
                        "id": "account-scoped-model",
                        "object": "model",
                        "created": 0,
                        "owned_by": "Cerebras"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider_info = ModelProviderInfo {
            base_url: Some(format!("{}/v1", server.uri())),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: Some("cerebras-token".to_string()),
            ..ModelProviderInfo::create_cerebras_provider()
        };
        let provider = create_model_provider(provider_info, /*auth_manager*/ None);

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-oss-120b", "account-scoped-model"]
        );
    }

    #[tokio::test]
    async fn nvidia_provider_models_manager_preserves_hosted_model_ids() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header_regex("Authorization", "Bearer nvidia-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    {
                        "id": "nvidia/llama-3.3-nemotron-super-49b-v1.5",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "nvidia"
                    },
                    {
                        "id": "openai/gpt-oss-120b",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "openai"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider_info = ModelProviderInfo {
            base_url: Some(format!("{}/v1", server.uri())),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: Some("nvidia-token".to_string()),
            ..ModelProviderInfo::create_nvidia_provider()
        };
        let provider = create_model_provider(provider_info, /*auth_manager*/ None);

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec![
                "nvidia/llama-3.3-nemotron-super-49b-v1.5",
                "openai/gpt-oss-120b"
            ]
        );
    }

    #[tokio::test]
    async fn env_key_provider_allows_public_model_discovery_when_key_is_missing() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    {
                        "id": "public/model",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "public"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider_info = ModelProviderInfo {
            base_url: Some(format!("{}/v1", server.uri())),
            env_key: Some("CODEWITH_TEST_MISSING_PROVIDER_API_KEY".to_string()),
            env_key_instructions: None,
            experimental_bearer_token: None,
            ..ModelProviderInfo::create_nvidia_provider()
        };
        let provider = create_model_provider_with_id(
            "public-provider",
            provider_info,
            /*auth_manager*/ None,
        );

        let manager =
            provider.models_manager(test_codex_home(), /*config_model_catalog*/ None);
        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["public/model"]
        );
    }
}
