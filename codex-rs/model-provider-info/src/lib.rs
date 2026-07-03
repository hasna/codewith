//! Registry of model providers supported by Codewith.
//!
//! Providers can be defined in two places:
//!   1. Built-in defaults compiled into the binary so Codewith works out-of-the-box.
//!   2. User-defined entries inside `~/.codewith/config.toml` under the `model_providers`
//!      key. These override or extend the defaults at runtime.
//!
//! The built-in picker surface is intentionally small: OpenAI, Anthropic, Cerebras, NVIDIA,
//! OpenRouter, xAI, Xiaomi, DeepSeek, Alibaba Qwen, Google Gemini, Z.ai, and MiniMax.

mod provider_credentials;

use codex_api::Provider as ApiProvider;
use codex_api::RetryConfig as ApiRetryConfig;
use codex_api::is_azure_responses_provider;
use codex_app_server_protocol::AuthMode;
use codex_protocol::config_types::ModelProviderAuthInfo;
use codex_protocol::error::CodexErr;
use codex_protocol::error::EnvVarError;
use codex_protocol::error::Result as CodexResult;
use http::HeaderMap;
use http::header::HeaderName;
use http::header::HeaderValue;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_STREAM_MAX_RETRIES: u64 = 5;
const DEFAULT_REQUEST_MAX_RETRIES: u64 = 4;
pub const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// Hard cap for user-configured `stream_max_retries`.
const MAX_STREAM_MAX_RETRIES: u64 = 100;
/// Hard cap for user-configured `request_max_retries`.
const MAX_REQUEST_MAX_RETRIES: u64 = 100;

const OPENAI_PROVIDER_NAME: &str = "OpenAI";
pub const OPENAI_PROVIDER_ID: &str = "openai";
pub const OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const HASNA_GATEWAY_ID: &str = "hasna";
pub const HASNA_GATEWAY_NAME: &str = "Hasna";
const ANTHROPIC_PROVIDER_NAME: &str = "Anthropic";
pub const ANTHROPIC_PROVIDER_ID: &str = "anthropic";
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";
const CEREBRAS_PROVIDER_NAME: &str = "Cerebras";
pub const CEREBRAS_PROVIDER_ID: &str = "cerebras";
pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";
const NVIDIA_PROVIDER_NAME: &str = "NVIDIA";
pub const NVIDIA_PROVIDER_ID: &str = "nvidia";
pub const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const OPENROUTER_PROVIDER_NAME: &str = "OpenRouter";
pub const OPENROUTER_PROVIDER_ID: &str = "openrouter";
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_GATEWAY_ID: &str = OPENROUTER_PROVIDER_ID;
pub const OPENROUTER_GATEWAY_NAME: &str = OPENROUTER_PROVIDER_NAME;
const XAI_PROVIDER_NAME: &str = "xAI";
pub const XAI_PROVIDER_ID: &str = "xai";
pub const XAI_BASE_URL: &str = "https://api.x.ai/v1";
const XIAOMI_PROVIDER_NAME: &str = "Xiaomi MiMo";
pub const XIAOMI_PROVIDER_ID: &str = "xiaomi";
pub const XIAOMI_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEEPSEEK_PROVIDER_NAME: &str = "DeepSeek";
pub const DEEPSEEK_PROVIDER_ID: &str = "deepseek";
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const QWEN_PROVIDER_NAME: &str = "Alibaba Qwen";
pub const QWEN_PROVIDER_ID: &str = "qwen";
pub const QWEN_BASE_URL: &str =
    "https://dashscope-intl.aliyuncs.com/api/v2/apps/protocols/compatible-mode/v1";
const GOOGLE_PROVIDER_NAME: &str = "Google Gemini";
pub const GOOGLE_PROVIDER_ID: &str = "google";
pub const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const ZAI_PROVIDER_NAME: &str = "Z.ai";
pub const ZAI_PROVIDER_ID: &str = "zai";
pub const ZAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4";
const MINIMAX_PROVIDER_NAME: &str = "MiniMax";
pub const MINIMAX_PROVIDER_ID: &str = "minimax";
pub const MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";
const AMAZON_BEDROCK_PROVIDER_NAME: &str = "Amazon Bedrock";
pub const AMAZON_BEDROCK_PROVIDER_ID: &str = "amazon-bedrock";
pub const AMAZON_BEDROCK_GPT_5_5_MODEL_ID: &str = "openai.gpt-5.5";
pub const AMAZON_BEDROCK_GPT_5_4_MODEL_ID: &str = "openai.gpt-5.4";
pub const AMAZON_BEDROCK_DEFAULT_BASE_URL: &str =
    "https://bedrock-mantle.us-east-1.api.aws/openai/v1";
const AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER: &str = "x-amzn-mantle-client-agent";
const AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE: &str = "codex";
pub const LEGACY_OLLAMA_CHAT_PROVIDER_ID: &str = "ollama-chat";
pub const OLLAMA_CHAT_PROVIDER_REMOVED_ERROR: &str = "`ollama-chat` is no longer supported.\nHow to fix: replace `ollama-chat` with `ollama` in `model_provider`, `oss_provider`, or `--local-provider`.\nMore info: https://github.com/openai/codex/discussions/7782";

/// Wire protocol that the provider speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    /// The Responses API exposed by OpenAI at `/v1/responses`.
    #[default]
    Responses,
    /// The OpenAI-compatible Chat Completions API exposed at `/v1/chat/completions`.
    Chat,
}

impl fmt::Display for WireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Responses => "responses",
            Self::Chat => "chat",
        };
        f.write_str(value)
    }
}

impl<'de> Deserialize<'de> for WireApi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "responses" => Ok(Self::Responses),
            "chat" => Ok(Self::Chat),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["responses", "chat"],
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelGatewayFamily {
    Direct,
    Aggregator,
}

pub fn model_gateway_family(gateway_id: &str) -> Option<ModelGatewayFamily> {
    match gateway_id {
        HASNA_GATEWAY_ID => Some(ModelGatewayFamily::Direct),
        OPENROUTER_GATEWAY_ID => Some(ModelGatewayFamily::Aggregator),
        _ => None,
    }
}

pub fn model_gateway_name(gateway_id: &str) -> Option<&'static str> {
    match gateway_id {
        HASNA_GATEWAY_ID => Some(HASNA_GATEWAY_NAME),
        OPENROUTER_GATEWAY_ID => Some(OPENROUTER_GATEWAY_NAME),
        _ => None,
    }
}

pub fn model_gateway_provider_id(gateway_id: &str) -> Option<&'static str> {
    match gateway_id {
        OPENROUTER_GATEWAY_ID => Some(OPENROUTER_PROVIDER_ID),
        _ => None,
    }
}

pub fn model_gateway_for_provider(provider_id: &str) -> &'static str {
    if provider_id == OPENROUTER_PROVIDER_ID {
        OPENROUTER_GATEWAY_ID
    } else {
        HASNA_GATEWAY_ID
    }
}

pub fn provider_belongs_to_model_gateway(provider_id: &str, gateway_id: &str) -> bool {
    match gateway_id {
        HASNA_GATEWAY_ID => true,
        OPENROUTER_GATEWAY_ID => provider_id == OPENROUTER_PROVIDER_ID,
        _ => false,
    }
}

pub fn provider_base_url_matches(configured_base_url: &str, trusted_base_url: &str) -> bool {
    normalize_provider_base_url(configured_base_url)
        == normalize_provider_base_url(trusted_base_url)
}

pub fn provider_base_url_is_loopback(configured_base_url: &str) -> bool {
    let Ok(uri) = configured_base_url.trim().parse::<http::Uri>() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn normalize_provider_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn trusted_secret_backend_base_url_for_env_key(env_key: &str) -> Option<&'static str> {
    let secret_name = provider_credentials::default_secret_name_for_provider_env_key(env_key)?;
    let (provider_secret_namespace, _secret_leaf) = secret_name.split_once('/')?;
    match provider_secret_namespace {
        "openai" => Some(OPENAI_API_BASE_URL),
        "openrouter" => Some(OPENROUTER_BASE_URL),
        "xai" => Some(XAI_BASE_URL),
        "anthropic" => Some(ANTHROPIC_BASE_URL),
        "cerebras" => Some(CEREBRAS_BASE_URL),
        "nvidia" => Some(NVIDIA_BASE_URL),
        "mimo" => Some(XIAOMI_BASE_URL),
        "deepseek" => Some(DEEPSEEK_BASE_URL),
        "dashscope" => Some(QWEN_BASE_URL),
        "gemini" => Some(GOOGLE_BASE_URL),
        "zai" => Some(ZAI_BASE_URL),
        "minimax" => Some(MINIMAX_BASE_URL),
        _ => None,
    }
}

/// Serializable representation of a provider definition.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ModelProviderInfo {
    /// Friendly display name.
    #[serde(default)]
    pub name: String,
    /// Base URL for the provider's OpenAI-compatible API.
    pub base_url: Option<String>,
    /// Environment variable that stores the user's API key for this provider.
    pub env_key: Option<String>,

    /// Optional instructions to help the user get a valid value for the
    /// variable and set it.
    pub env_key_instructions: Option<String>,
    /// Value to use with `Authorization: Bearer <token>` header. Use of this
    /// config is discouraged in favor of `env_key` for security reasons, but
    /// this may be necessary when using this programmatically.
    pub experimental_bearer_token: Option<String>,
    /// Command-backed bearer-token configuration for this provider.
    pub auth: Option<ModelProviderAuthInfo>,
    /// AWS SigV4 auth configuration for this provider.
    pub aws: Option<ModelProviderAwsAuthInfo>,
    /// Which wire protocol this provider expects.
    #[serde(default)]
    pub wire_api: WireApi,
    /// Optional query parameters to append to the base URL.
    pub query_params: Option<HashMap<String, String>>,
    /// Additional HTTP headers to include in requests to this provider where
    /// the (key, value) pairs are the header name and value.
    pub http_headers: Option<HashMap<String, String>>,
    /// Optional HTTP headers to include in requests to this provider where the
    /// (key, value) pairs are the header name and _environment variable_ whose
    /// value should be used. If the environment variable is not set, or the
    /// value is empty, the header will not be included in the request.
    pub env_http_headers: Option<HashMap<String, String>>,
    /// Maximum number of times to retry a failed HTTP request to this provider.
    pub request_max_retries: Option<u64>,
    /// Number of times to retry reconnecting a dropped streaming response before failing.
    pub stream_max_retries: Option<u64>,
    /// Idle timeout (in milliseconds) to wait for activity on a streaming response before treating
    /// the connection as lost.
    pub stream_idle_timeout_ms: Option<u64>,
    /// Maximum time (in milliseconds) to wait for a websocket connection attempt before treating
    /// it as failed.
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Does this provider require an OpenAI API Key or ChatGPT login token? If true,
    /// user is presented with login screen on first run, and login preference and token/key
    /// are stored in auth.json. If false (which is the default), login screen is skipped,
    /// and API key (if needed) comes from the "env_key" environment variable.
    #[serde(default)]
    pub requires_openai_auth: bool,
    /// Whether this provider supports the Responses API WebSocket transport.
    #[serde(default)]
    pub supports_websockets: bool,
}

/// AWS SigV4 auth configuration for a model provider.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ModelProviderAwsAuthInfo {
    /// AWS profile name to use. When unset, the AWS SDK default chain decides.
    pub profile: Option<String>,
    /// AWS region to use for provider-specific endpoints.
    pub region: Option<String>,
}

impl ModelProviderInfo {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.aws.is_some() {
            if self.supports_websockets {
                // TODO(celia-oai): Support AWS SigV4 signing for WebSocket
                // upgrade requests before allowing AWS-authenticated providers
                // to enable Responses-over-WebSocket.
                return Err("provider aws cannot be combined with supports_websockets".to_string());
            }

            let mut conflicts = Vec::new();
            if self.env_key.is_some() {
                conflicts.push("env_key");
            }
            if self.experimental_bearer_token.is_some() {
                conflicts.push("experimental_bearer_token");
            }
            if self.auth.is_some() {
                conflicts.push("auth");
            }
            if self.requires_openai_auth {
                conflicts.push("requires_openai_auth");
            }

            if !conflicts.is_empty() {
                return Err(format!(
                    "provider aws cannot be combined with {}",
                    conflicts.join(", ")
                ));
            }
        }

        let Some(auth) = self.auth.as_ref() else {
            return Ok(());
        };

        if auth.command.trim().is_empty() {
            return Err("provider auth.command must not be empty".to_string());
        }

        let mut conflicts = Vec::new();
        if self.env_key.is_some() {
            conflicts.push("env_key");
        }
        if self.experimental_bearer_token.is_some() {
            conflicts.push("experimental_bearer_token");
        }
        if self.requires_openai_auth {
            conflicts.push("requires_openai_auth");
        }

        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "provider auth cannot be combined with {}",
                conflicts.join(", ")
            ))
        }
    }

    fn build_header_map(&self) -> CodexResult<HeaderMap> {
        let capacity = self.http_headers.as_ref().map_or(0, HashMap::len)
            + self.env_http_headers.as_ref().map_or(0, HashMap::len);
        let mut headers = HeaderMap::with_capacity(capacity);
        if let Some(extra) = &self.http_headers {
            for (k, v) in extra {
                if let (Ok(name), Ok(value)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                    headers.insert(name, value);
                }
            }
        }

        if let Some(env_headers) = &self.env_http_headers {
            for (header, env_var) in env_headers {
                if let Ok(val) = std::env::var(env_var)
                    && !val.trim().is_empty()
                    && let (Ok(name), Ok(value)) =
                        (HeaderName::try_from(header), HeaderValue::try_from(val))
                {
                    headers.insert(name, value);
                }
            }
        }

        Ok(headers)
    }

    pub fn to_api_provider(&self, auth_mode: Option<AuthMode>) -> CodexResult<ApiProvider> {
        let default_base_url = if matches!(
            auth_mode,
            Some(
                AuthMode::Chatgpt
                    | AuthMode::ChatgptAuthTokens
                    | AuthMode::AgentIdentity
                    | AuthMode::PersonalAccessToken
            )
        ) {
            CHATGPT_CODEX_BASE_URL
        } else {
            OPENAI_API_BASE_URL
        };
        let base_url = self
            .base_url
            .clone()
            .unwrap_or_else(|| default_base_url.to_string());

        let headers = self.build_header_map()?;
        let retry = ApiRetryConfig {
            max_attempts: self.request_max_retries(),
            base_delay: Duration::from_millis(200),
            retry_429: false,
            retry_5xx: true,
            retry_transport: true,
        };

        Ok(ApiProvider {
            provider_id: None,
            name: self.name.clone(),
            base_url,
            query_params: self.query_params.clone(),
            headers,
            retry,
            stream_idle_timeout: self.stream_idle_timeout(),
        })
    }

    /// If `env_key` is Some, returns the API key for this provider if present
    /// (and non-empty) in a supported runtime credential source. If `env_key`
    /// is required but cannot be found, returns an error.
    pub fn api_key(&self) -> CodexResult<Option<String>> {
        match &self.env_key {
            Some(env_key) => {
                let api_key = self.api_key_if_available().ok_or_else(|| {
                    CodexErr::EnvVar(EnvVarError {
                        var: env_key.clone(),
                        instructions: self.env_key_instructions.clone(),
                    })
                })?;
                Ok(Some(api_key))
            }
            None => Ok(None),
        }
    }

    /// If `env_key` is Some, returns the API key when it is present in a
    /// supported runtime credential source. Missing keys are allowed.
    pub fn api_key_if_available(&self) -> Option<String> {
        self.env_key.as_deref().and_then(|env_key| {
            provider_credentials::provider_key_from_env_or_secret(
                env_key,
                self.secret_backend_fallback(env_key),
            )
        })
    }

    fn secret_backend_fallback(
        &self,
        env_key: &str,
    ) -> provider_credentials::SecretBackendFallback {
        match trusted_secret_backend_base_url_for_env_key(env_key) {
            Some(trusted_base_url) => {
                let base_url_is_trusted = match self.base_url.as_deref() {
                    Some(base_url) => provider_base_url_matches(base_url, trusted_base_url),
                    None => provider_base_url_matches(trusted_base_url, OPENAI_API_BASE_URL),
                };
                if base_url_is_trusted {
                    provider_credentials::SecretBackendFallback::Enabled
                } else {
                    provider_credentials::SecretBackendFallback::Disabled
                }
            }
            None => provider_credentials::SecretBackendFallback::Enabled,
        }
    }

    /// Effective maximum number of request retries for this provider.
    pub fn request_max_retries(&self) -> u64 {
        self.request_max_retries
            .unwrap_or(DEFAULT_REQUEST_MAX_RETRIES)
            .min(MAX_REQUEST_MAX_RETRIES)
    }

    /// Effective maximum number of stream reconnection attempts for this provider.
    pub fn stream_max_retries(&self) -> u64 {
        self.stream_max_retries
            .unwrap_or(DEFAULT_STREAM_MAX_RETRIES)
            .min(MAX_STREAM_MAX_RETRIES)
    }

    /// Effective idle timeout for streaming responses.
    pub fn stream_idle_timeout(&self) -> Duration {
        self.stream_idle_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_STREAM_IDLE_TIMEOUT_MS))
    }

    /// Effective timeout for websocket connect attempts.
    pub fn websocket_connect_timeout(&self) -> Duration {
        self.websocket_connect_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS))
    }

    pub fn create_openai_provider(base_url: Option<String>) -> ModelProviderInfo {
        ModelProviderInfo {
            name: OPENAI_PROVIDER_NAME.into(),
            base_url,
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(
                [(
                    "version".to_string(),
                    codex_protocol::client_version::codex_api_version(),
                )]
                .into_iter()
                .collect(),
            ),
            env_http_headers: Some(
                [
                    (
                        "OpenAI-Organization".to_string(),
                        "OPENAI_ORGANIZATION".to_string(),
                    ),
                    ("OpenAI-Project".to_string(), "OPENAI_PROJECT".to_string()),
                ]
                .into_iter()
                .collect(),
            ),
            // Use global defaults for retry/timeout unless overridden in config.toml.
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: true,
            supports_websockets: true,
        }
    }

    pub fn create_openrouter_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: OPENROUTER_PROVIDER_NAME.into(),
            base_url: Some(OPENROUTER_BASE_URL.into()),
            env_key: Some("OPENROUTER_API_KEY".into()),
            env_key_instructions: Some("Set OPENROUTER_API_KEY to an OpenRouter API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_xai_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: XAI_PROVIDER_NAME.into(),
            base_url: Some(XAI_BASE_URL.into()),
            env_key: Some("XAI_API_KEY".into()),
            env_key_instructions: Some("Set XAI_API_KEY to an xAI API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_anthropic_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: ANTHROPIC_PROVIDER_NAME.into(),
            base_url: Some(ANTHROPIC_BASE_URL.into()),
            env_key: Some("ANTHROPIC_API_KEY".into()),
            env_key_instructions: Some("Set ANTHROPIC_API_KEY to an Anthropic API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_cerebras_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: CEREBRAS_PROVIDER_NAME.into(),
            base_url: Some(CEREBRAS_BASE_URL.into()),
            env_key: Some("CEREBRAS_API_KEY".into()),
            env_key_instructions: Some("Set CEREBRAS_API_KEY to a Cerebras API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_nvidia_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: NVIDIA_PROVIDER_NAME.into(),
            base_url: Some(NVIDIA_BASE_URL.into()),
            env_key: Some("NVIDIA_API_KEY".into()),
            env_key_instructions: Some("Set NVIDIA_API_KEY to an NVIDIA NIM API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_xiaomi_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: XIAOMI_PROVIDER_NAME.into(),
            base_url: Some(XIAOMI_BASE_URL.into()),
            env_key: Some("MIMO_API_KEY".into()),
            env_key_instructions: Some("Set MIMO_API_KEY to a Xiaomi MiMo API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: Some(HashMap::from([(
                "api-key".to_string(),
                "MIMO_API_KEY".to_string(),
            )])),
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_deepseek_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: DEEPSEEK_PROVIDER_NAME.into(),
            base_url: Some(DEEPSEEK_BASE_URL.into()),
            env_key: Some("DEEPSEEK_API_KEY".into()),
            env_key_instructions: Some("Set DEEPSEEK_API_KEY to a DeepSeek API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_qwen_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: QWEN_PROVIDER_NAME.into(),
            base_url: Some(QWEN_BASE_URL.into()),
            env_key: Some("DASHSCOPE_API_KEY".into()),
            env_key_instructions: Some(
                "Set DASHSCOPE_API_KEY to an Alibaba Model Studio API key.".into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_google_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: GOOGLE_PROVIDER_NAME.into(),
            base_url: Some(GOOGLE_BASE_URL.into()),
            env_key: Some("GEMINI_API_KEY".into()),
            env_key_instructions: Some(
                "Set GEMINI_API_KEY to a Google AI Studio Gemini API key.".into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_zai_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: ZAI_PROVIDER_NAME.into(),
            base_url: Some(ZAI_BASE_URL.into()),
            env_key: Some("ZAI_API_KEY".into()),
            env_key_instructions: Some("Set ZAI_API_KEY to a Z.ai API key.".into()),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_minimax_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: MINIMAX_PROVIDER_NAME.into(),
            base_url: Some(MINIMAX_BASE_URL.into()),
            env_key: Some("MINIMAX_API_KEY".into()),
            env_key_instructions: Some(
                "Set MINIMAX_API_KEY to a MiniMax Subscription Key or API key.".into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Chat,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_amazon_bedrock_provider(
        aws: Option<ModelProviderAwsAuthInfo>,
    ) -> ModelProviderInfo {
        ModelProviderInfo {
            name: AMAZON_BEDROCK_PROVIDER_NAME.into(),
            base_url: Some(AMAZON_BEDROCK_DEFAULT_BASE_URL.into()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(aws.unwrap_or(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            })),
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(HashMap::from([(
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER.to_string(),
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE.to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_ollama_oss_provider() -> ModelProviderInfo {
        create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::Responses)
    }

    pub fn create_lmstudio_oss_provider() -> ModelProviderInfo {
        create_oss_provider(DEFAULT_LMSTUDIO_PORT, WireApi::Responses)
    }

    pub fn is_openai(&self) -> bool {
        self.name == OPENAI_PROVIDER_NAME
    }

    pub fn is_amazon_bedrock(&self) -> bool {
        self.name == AMAZON_BEDROCK_PROVIDER_NAME
    }

    pub fn supports_remote_compaction(&self) -> bool {
        self.is_openai() || is_azure_responses_provider(&self.name, self.base_url.as_deref())
    }

    pub fn has_command_auth(&self) -> bool {
        self.auth.is_some()
    }
}

pub const DEFAULT_LMSTUDIO_PORT: u16 = 1234;
pub const DEFAULT_OLLAMA_PORT: u16 = 11434;

pub const LMSTUDIO_OSS_PROVIDER_ID: &str = "lmstudio";
pub const OLLAMA_OSS_PROVIDER_ID: &str = "ollama";

#[derive(Clone, Copy)]
enum BuiltInProviderFactory {
    OpenAi,
    Static(fn() -> ModelProviderInfo),
}

#[derive(Clone, Copy)]
struct BuiltInModelProviderSpec {
    id: &'static str,
    factory: BuiltInProviderFactory,
    allows_partial_override: bool,
    default_override_wire_api: WireApi,
}

impl BuiltInModelProviderSpec {
    fn create_provider(self, openai_base_url: Option<String>) -> ModelProviderInfo {
        match self.factory {
            BuiltInProviderFactory::OpenAi => {
                ModelProviderInfo::create_openai_provider(openai_base_url)
            }
            BuiltInProviderFactory::Static(factory) => factory(),
        }
    }
}

const BUILT_IN_MODEL_PROVIDER_SPECS: &[BuiltInModelProviderSpec] = &[
    BuiltInModelProviderSpec {
        id: OPENAI_PROVIDER_ID,
        factory: BuiltInProviderFactory::OpenAi,
        allows_partial_override: false,
        default_override_wire_api: WireApi::Responses,
    },
    BuiltInModelProviderSpec {
        id: ANTHROPIC_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_anthropic_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: CEREBRAS_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_cerebras_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: NVIDIA_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_nvidia_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: OPENROUTER_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_openrouter_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Responses,
    },
    BuiltInModelProviderSpec {
        id: XAI_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_xai_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Responses,
    },
    BuiltInModelProviderSpec {
        id: XIAOMI_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_xiaomi_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: DEEPSEEK_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_deepseek_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: QWEN_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_qwen_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Responses,
    },
    BuiltInModelProviderSpec {
        id: GOOGLE_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_google_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: ZAI_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_zai_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: MINIMAX_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_minimax_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Chat,
    },
    BuiltInModelProviderSpec {
        id: OLLAMA_OSS_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_ollama_oss_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Responses,
    },
    BuiltInModelProviderSpec {
        id: LMSTUDIO_OSS_PROVIDER_ID,
        factory: BuiltInProviderFactory::Static(ModelProviderInfo::create_lmstudio_oss_provider),
        allows_partial_override: true,
        default_override_wire_api: WireApi::Responses,
    },
];

fn built_in_provider_spec(provider_id: &str) -> Option<&'static BuiltInModelProviderSpec> {
    BUILT_IN_MODEL_PROVIDER_SPECS
        .iter()
        .find(|spec| spec.id == provider_id)
}

pub fn built_in_model_provider_ids() -> impl Iterator<Item = &'static str> {
    BUILT_IN_MODEL_PROVIDER_SPECS.iter().map(|spec| spec.id)
}

pub fn allows_partial_builtin_provider_override(provider_id: &str) -> bool {
    built_in_provider_spec(provider_id).is_some_and(|spec| spec.allows_partial_override)
}

pub fn default_wire_api_for_builtin_provider_override(provider_id: &str) -> WireApi {
    built_in_provider_spec(provider_id)
        .map_or(WireApi::Responses, |spec| spec.default_override_wire_api)
}

/// Built-in default provider list.
pub fn built_in_model_providers(
    openai_base_url: Option<String>,
) -> HashMap<String, ModelProviderInfo> {
    BUILT_IN_MODEL_PROVIDER_SPECS
        .iter()
        .map(|spec| {
            (
                spec.id.to_string(),
                spec.create_provider(openai_base_url.clone()),
            )
        })
        .collect()
}

/// Merge configured providers into the built-in provider catalog.
///
/// Configured providers extend the built-in set. Built-in providers are not
/// generally overridable. The built-in provider spec table marks which
/// provider defaults remain partially overridable so users can point them at
/// compatible mirrors. Amazon Bedrock is no longer built in, but an explicit
/// `[model_providers.amazon-bedrock.aws]` block still enables the provider with
/// the default Bedrock endpoint and optional AWS profile.
pub fn merge_configured_model_providers(
    mut model_providers: HashMap<String, ModelProviderInfo>,
    configured_model_providers: HashMap<String, ModelProviderInfo>,
) -> Result<HashMap<String, ModelProviderInfo>, String> {
    for (key, mut provider) in configured_model_providers {
        if key == AMAZON_BEDROCK_PROVIDER_ID {
            let aws_override = provider.aws.take();
            if provider != ModelProviderInfo::default() {
                return Err(format!(
                    "model_providers.{AMAZON_BEDROCK_PROVIDER_ID} only supports changing \
`aws.profile` and `aws.region`; other non-default provider fields are not supported"
                ));
            }

            let built_in_provider = model_providers.entry(key).or_insert_with(|| {
                ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None)
            });
            if let Some(aws_override) = aws_override
                && let Some(built_in_aws) = built_in_provider.aws.as_mut()
            {
                if let Some(profile) = aws_override.profile {
                    built_in_aws.profile = Some(profile);
                }
                if let Some(region) = aws_override.region {
                    built_in_aws.region = Some(region);
                }
            }
        } else if allows_partial_builtin_provider_override(&key) {
            if let Some(built_in_provider) = model_providers.get_mut(&key) {
                apply_provider_override(built_in_provider, provider);
            } else {
                model_providers.insert(key, provider);
            }
        } else {
            model_providers.entry(key).or_insert(provider);
        }
    }

    Ok(model_providers)
}

fn apply_provider_override(
    built_in_provider: &mut ModelProviderInfo,
    provider_override: ModelProviderInfo,
) {
    let ModelProviderInfo {
        name,
        base_url,
        env_key,
        env_key_instructions,
        experimental_bearer_token,
        auth,
        aws,
        wire_api,
        query_params,
        http_headers,
        env_http_headers,
        request_max_retries,
        stream_max_retries,
        stream_idle_timeout_ms,
        websocket_connect_timeout_ms,
        requires_openai_auth,
        supports_websockets,
    } = provider_override;
    let has_env_key_override = env_key.is_some();
    let has_env_key_instructions_override = env_key_instructions.is_some();

    if !name.is_empty() {
        built_in_provider.name = name;
    }
    if let Some(base_url) = base_url {
        built_in_provider.base_url = Some(base_url);
    }
    if let Some(env_key) = env_key {
        built_in_provider.env_key = Some(env_key);
        if !has_env_key_instructions_override {
            built_in_provider.env_key_instructions = None;
        }
    }
    if let Some(env_key_instructions) = env_key_instructions {
        built_in_provider.env_key_instructions = Some(env_key_instructions);
    }
    if let Some(experimental_bearer_token) = experimental_bearer_token {
        built_in_provider.experimental_bearer_token = Some(experimental_bearer_token);
    }
    if let Some(auth) = auth {
        built_in_provider.auth = Some(auth);
        if !has_env_key_override {
            built_in_provider.env_key = None;
            built_in_provider.env_key_instructions = None;
        }
    }
    if let Some(aws) = aws {
        built_in_provider.aws = Some(aws);
    }
    built_in_provider.wire_api = wire_api;
    if let Some(query_params) = query_params {
        built_in_provider.query_params = Some(query_params);
    }
    if let Some(http_headers) = http_headers {
        built_in_provider.http_headers = Some(http_headers);
    }
    if let Some(env_http_headers) = env_http_headers {
        built_in_provider.env_http_headers = Some(env_http_headers);
    }
    if let Some(request_max_retries) = request_max_retries {
        built_in_provider.request_max_retries = Some(request_max_retries);
    }
    if let Some(stream_max_retries) = stream_max_retries {
        built_in_provider.stream_max_retries = Some(stream_max_retries);
    }
    if let Some(stream_idle_timeout_ms) = stream_idle_timeout_ms {
        built_in_provider.stream_idle_timeout_ms = Some(stream_idle_timeout_ms);
    }
    if let Some(websocket_connect_timeout_ms) = websocket_connect_timeout_ms {
        built_in_provider.websocket_connect_timeout_ms = Some(websocket_connect_timeout_ms);
    }
    if requires_openai_auth {
        built_in_provider.requires_openai_auth = true;
    }
    if supports_websockets {
        built_in_provider.supports_websockets = true;
    }
}

pub fn create_oss_provider(default_provider_port: u16, wire_api: WireApi) -> ModelProviderInfo {
    // These CODEX_OSS_ environment variables are experimental: we may
    // switch to reading values from config.toml instead.
    let default_codex_oss_base_url = format!(
        "http://localhost:{codex_oss_port}/v1",
        codex_oss_port = std::env::var("CODEX_OSS_PORT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(default_provider_port)
    );

    let codex_oss_base_url = std::env::var("CODEX_OSS_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(default_codex_oss_base_url);
    create_oss_provider_with_base_url(&codex_oss_base_url, wire_api)
}

pub fn create_oss_provider_with_base_url(base_url: &str, wire_api: WireApi) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "gpt-oss".into(),
        base_url: Some(base_url.into()),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    }
}

#[cfg(test)]
#[path = "model_provider_info_tests.rs"]
mod tests;
