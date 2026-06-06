use crate::auth::SharedAuthProvider;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use codex_client::HttpTransport;
use codex_client::RequestTelemetry;
use codex_known_provider_models as known_provider_models;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;
use http::HeaderMap;
use http::Method;
use http::header::ETAG;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ModelsClient<T: HttpTransport> {
    session: EndpointSession<T>,
    provider_id: Option<String>,
}

impl<T: HttpTransport> ModelsClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            provider_id: None,
        }
    }

    pub fn with_telemetry(self, request: Option<Arc<dyn RequestTelemetry>>) -> Self {
        let Self {
            session,
            provider_id,
        } = self;
        Self {
            session: session.with_request_telemetry(request),
            provider_id,
        }
    }

    pub fn with_provider_id(mut self, provider_id: Option<String>) -> Self {
        self.provider_id = provider_id;
        self
    }

    fn path() -> &'static str {
        "models"
    }

    fn append_client_version_query(req: &mut codex_client::Request, client_version: &str) {
        let separator = if req.url.contains('?') { '&' } else { '?' };
        req.url = format!("{}{}client_version={client_version}", req.url, separator);
    }

    pub async fn list_models(
        &self,
        client_version: &str,
        extra_headers: HeaderMap,
    ) -> Result<(Vec<ModelInfo>, Option<String>), ApiError> {
        let resp = self
            .session
            .execute_with(
                Method::GET,
                Self::path(),
                extra_headers,
                /*body*/ None,
                |req| {
                    Self::append_client_version_query(req, client_version);
                },
            )
            .await?;

        let header_etag = resp
            .headers
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);

        let provider = self.session.provider();
        let models = decode_models_response(
            &resp.body,
            self.provider_id.as_deref(),
            Some(provider.name.as_str()),
            Some(provider.base_url.as_str()),
        )
        .map_err(|e| {
            ApiError::Stream(format!(
                "failed to decode models response: {e}; body: {}",
                String::from_utf8_lossy(&resp.body)
            ))
        })?;

        Ok((models, header_etag))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelsEndpointResponse {
    Codex(ModelsResponse),
    OpenAiCompatible(OpenAiCompatibleModelsResponse),
}

#[derive(Deserialize)]
struct OpenAiCompatibleModelsResponse {
    data: Vec<OpenAiCompatibleModel>,
}

#[derive(Deserialize)]
struct OpenAiCompatibleModel {
    id: String,
    name: Option<String>,
    description: Option<String>,
    context_length: Option<i64>,
    architecture: Option<OpenAiCompatibleArchitecture>,
    capabilities: Option<OpenAiCompatibleCapabilities>,
    limits: Option<OpenAiCompatibleLimits>,
    pricing: Option<OpenAiCompatiblePricing>,
    top_provider: Option<OpenAiCompatibleTopProvider>,
    supported_parameters: Option<OpenAiCompatibleSupportedParameters>,
}

#[derive(Deserialize)]
struct OpenAiCompatibleArchitecture {
    input_modalities: Option<Vec<String>>,
    modality: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiCompatibleCapabilities {
    function_calling: Option<bool>,
    tools: Option<bool>,
    parallel_tool_calls: Option<bool>,
    reasoning: Option<bool>,
}

#[derive(Deserialize)]
struct OpenAiCompatibleLimits {
    max_context_length: Option<i64>,
}

#[derive(Deserialize)]
struct OpenAiCompatiblePricing {
    prompt: Option<String>,
    completion: Option<String>,
    input_cache_read: Option<String>,
    input_cache_write: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiCompatibleTopProvider {
    context_length: Option<i64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OpenAiCompatibleSupportedParameters {
    List(Vec<String>),
    Flags(HashMap<String, bool>),
}

impl OpenAiCompatibleSupportedParameters {
    fn support_state(&self, parameter: &str) -> Option<bool> {
        match self {
            Self::List(parameters) => Some(parameters.iter().any(|value| value == parameter)),
            Self::Flags(parameters) => parameters.get(parameter).copied(),
        }
    }

    fn supports_any(&self, parameters: &[&str]) -> Option<bool> {
        match self {
            Self::List(supported_parameters) => Some(parameters.iter().any(|parameter| {
                supported_parameters
                    .iter()
                    .any(|supported_parameter| supported_parameter == *parameter)
            })),
            Self::Flags(supported_parameters) => {
                let mut saw_parameter = false;
                for parameter in parameters {
                    if let Some(supported) = supported_parameters.get(*parameter) {
                        if *supported {
                            return Some(true);
                        }
                        saw_parameter = true;
                    }
                }
                saw_parameter.then_some(false)
            }
        }
    }
}

fn decode_models_response(
    body: &[u8],
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
) -> Result<Vec<ModelInfo>, serde_json::Error> {
    let response = serde_json::from_slice::<ModelsEndpointResponse>(body)?;
    Ok(match response {
        ModelsEndpointResponse::Codex(ModelsResponse { models }) => models,
        ModelsEndpointResponse::OpenAiCompatible(OpenAiCompatibleModelsResponse { data }) => data
            .into_iter()
            .enumerate()
            .map(|(index, model)| {
                model.into_model_info(
                    i32::try_from(index).unwrap_or(i32::MAX),
                    provider_id,
                    provider_name,
                    provider_base_url,
                )
            })
            .collect(),
    })
}

impl OpenAiCompatibleModel {
    fn into_model_info(
        self,
        priority: i32,
        provider_id: Option<&str>,
        provider_name: Option<&str>,
        provider_base_url: Option<&str>,
    ) -> ModelInfo {
        let OpenAiCompatibleModel {
            id,
            name,
            description,
            context_length,
            architecture,
            capabilities,
            limits,
            pricing,
            top_provider,
            supported_parameters,
        } = self;
        let known_metadata = known_provider_models::metadata_for_openai_compatible_response(
            provider_id,
            provider_name,
            provider_base_url,
            &id,
        );
        let supports_tools = supported_parameters
            .as_ref()
            .and_then(|params| params.supports_any(&["tools", "function_calling"]))
            .or_else(|| {
                capabilities.as_ref().and_then(|capabilities| {
                    (capabilities.tools.is_some() || capabilities.function_calling.is_some()).then(
                        || {
                            capabilities.tools.unwrap_or(false)
                                || capabilities.function_calling.unwrap_or(false)
                        },
                    )
                })
            })
            .unwrap_or_else(|| known_metadata.is_some_and(|metadata| metadata.supports_tools));
        let supports_parallel_tool_calls = supported_parameters
            .as_ref()
            .and_then(|params| params.support_state("parallel_tool_calls"))
            .or_else(|| {
                capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.parallel_tool_calls)
            })
            .or_else(|| known_metadata.map(|metadata| metadata.supports_parallel_tool_calls))
            .unwrap_or(supports_tools)
            && supports_tools;
        let provider_context_length = top_provider.and_then(|provider| provider.context_length);
        let limits_context_length = limits.and_then(|limits| limits.max_context_length);
        let effective_context_length = provider_context_length
            .or(context_length)
            .or(limits_context_length)
            .or_else(|| known_metadata.map(|metadata| metadata.context_window));
        let max_context_window = context_length.or(effective_context_length);
        let supports_reasoning = capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning)
            .unwrap_or_else(|| known_metadata.is_some_and(|metadata| metadata.supports_reasoning));
        let supports_reasoning_requests = supports_reasoning
            && known_provider_models::openai_compatible_provider_supports_reasoning_effort(
                provider_id,
                provider_base_url,
            );
        let (default_reasoning_level, supported_reasoning_levels) =
            reasoning_levels_for_openai_compatible_model(
                &id,
                provider_id,
                provider_name,
                provider_base_url,
                supports_reasoning_requests,
            );
        ModelInfo {
            slug: id.clone(),
            display_name: name.unwrap_or_else(|| {
                known_metadata
                    .map(|metadata| metadata.display_name.to_string())
                    .unwrap_or_else(|| id.clone())
            }),
            description: with_pricing_description(description, pricing.as_ref()),
            default_reasoning_level,
            supported_reasoning_levels,
            shell_type: ConfigShellToolType::Default,
            visibility: ModelVisibility::List,
            supported_in_api: true,
            priority,
            additional_speed_tiers: Vec::new(),
            service_tiers: Vec::new(),
            default_service_tier: None,
            availability_nux: None,
            upgrade: None,
            base_instructions: String::new(),
            model_messages: None,
            supports_reasoning_summaries: supports_reasoning_requests,
            default_reasoning_summary: ReasoningSummary::Auto,
            support_verbosity: false,
            default_verbosity: None,
            apply_patch_tool_type: None,
            web_search_tool_type: WebSearchToolType::Text,
            truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
            supports_parallel_tool_calls,
            supports_image_detail_original: false,
            context_window: effective_context_length,
            max_context_window,
            auto_compact_token_limit: None,
            effective_context_window_percent: 95,
            experimental_supported_tools: if supports_tools {
                vec!["tools".to_string()]
            } else {
                Vec::new()
            },
            input_modalities: input_modalities_from_openai_compatible_architecture(architecture),
            used_fallback_model_metadata: false,
            supports_search_tool: false,
            tool_mode: None,
        }
    }
}

fn reasoning_levels_for_openai_compatible_model(
    id: &str,
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    supports_reasoning: bool,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    if !supports_reasoning {
        return (None, Vec::new());
    }

    known_provider_models::reasoning_levels_for_openai_compatible_response(
        provider_id,
        provider_name,
        provider_base_url,
        id,
    )
}

fn with_pricing_description(
    description: Option<String>,
    pricing: Option<&OpenAiCompatiblePricing>,
) -> Option<String> {
    let Some(pricing) = pricing else {
        return description;
    };
    let Some(pricing_text) = compact_pricing_description(pricing) else {
        return description;
    };

    Some(match description {
        Some(description) if !description.trim().is_empty() => {
            format!("{description}\n\n{pricing_text}")
        }
        _ => pricing_text,
    })
}

fn compact_pricing_description(pricing: &OpenAiCompatiblePricing) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(price) = pricing.prompt.as_deref().and_then(price_per_million) {
        parts.push(format!("input {price}"));
    }
    if let Some(price) = pricing.completion.as_deref().and_then(price_per_million) {
        parts.push(format!("output {price}"));
    }
    if let Some(price) = pricing
        .input_cache_read
        .as_deref()
        .and_then(price_per_million)
    {
        parts.push(format!("cache read {price}"));
    }
    if let Some(price) = pricing
        .input_cache_write
        .as_deref()
        .and_then(price_per_million)
    {
        parts.push(format!("cache write {price}"));
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!("Pricing: {} per 1M tokens.", parts.join(", ")))
    }
}

fn price_per_million(price_per_token: &str) -> Option<String> {
    let amount = price_per_token.parse::<f64>().ok()? * 1_000_000.0;
    Some(format!("${}", trim_price(amount)))
}

fn trim_price(amount: f64) -> String {
    let mut formatted = format!("{amount:.4}");
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
}

fn input_modalities_from_openai_compatible_architecture(
    architecture: Option<OpenAiCompatibleArchitecture>,
) -> Vec<InputModality> {
    let Some(architecture) = architecture else {
        return vec![InputModality::Text];
    };

    if let Some(modalities) = architecture.input_modalities {
        return input_modalities_from_openai_compatible(modalities);
    }

    architecture
        .modality
        .map(|modality| input_modalities_from_openai_compatible(vec![modality]))
        .unwrap_or_else(|| vec![InputModality::Text])
}

fn input_modalities_from_openai_compatible(modalities: Vec<String>) -> Vec<InputModality> {
    let parsed = modalities
        .into_iter()
        .filter_map(|modality| match modality.as_str() {
            "text" => Some(InputModality::Text),
            "image" => Some(InputModality::Image),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parsed.is_empty() {
        vec![InputModality::Text]
    } else {
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthProvider;
    use crate::provider::RetryConfig;
    use async_trait::async_trait;
    use codex_client::Request;
    use codex_client::Response;
    use codex_client::StreamResponse;
    use codex_client::TransportError;
    use http::HeaderMap;
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone)]
    struct CapturingTransport {
        last_request: Arc<Mutex<Option<Request>>>,
        body: Arc<ModelsResponse>,
        etag: Option<String>,
    }

    impl Default for CapturingTransport {
        fn default() -> Self {
            Self {
                last_request: Arc::new(Mutex::new(None)),
                body: Arc::new(ModelsResponse { models: Vec::new() }),
                etag: None,
            }
        }
    }

    #[async_trait]
    impl HttpTransport for CapturingTransport {
        async fn execute(&self, req: Request) -> Result<Response, TransportError> {
            *self.last_request.lock().unwrap() = Some(req);
            let body = serde_json::to_vec(&*self.body).unwrap();
            let mut headers = HeaderMap::new();
            if let Some(etag) = &self.etag {
                headers.insert(ETAG, etag.parse().unwrap());
            }
            Ok(Response {
                status: StatusCode::OK,
                headers,
                body: body.into(),
            })
        }

        async fn stream(&self, _req: Request) -> Result<StreamResponse, TransportError> {
            Err(TransportError::Build("stream should not run".to_string()))
        }
    }

    #[derive(Clone, Default)]
    struct DummyAuth;

    impl AuthProvider for DummyAuth {
        fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
    }

    fn provider(base_url: &str) -> Provider {
        Provider {
            name: "test".to_string(),
            base_url: base_url.to_string(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: true,
                retry_transport: true,
            },
            stream_idle_timeout: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn appends_client_version_query() {
        let response = ModelsResponse { models: Vec::new() };

        let transport = CapturingTransport {
            last_request: Arc::new(Mutex::new(None)),
            body: Arc::new(response),
            etag: None,
        };

        let client = ModelsClient::new(
            transport.clone(),
            provider("https://example.com/api/codex"),
            Arc::new(DummyAuth),
        );

        let (models, _) = client
            .list_models("0.99.0", HeaderMap::new())
            .await
            .expect("request should succeed");

        assert_eq!(models.len(), 0);

        let url = transport
            .last_request
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .url
            .clone();
        assert_eq!(
            url,
            "https://example.com/api/codex/models?client_version=0.99.0"
        );
    }

    #[tokio::test]
    async fn parses_models_response() {
        let response = ModelsResponse {
            models: vec![
                serde_json::from_value(json!({
                    "slug": "gpt-test",
                    "display_name": "gpt-test",
                    "description": "desc",
                    "default_reasoning_level": "medium",
                    "supported_reasoning_levels": [{"effort": "low", "description": "low"}, {"effort": "medium", "description": "medium"}, {"effort": "high", "description": "high"}],
                    "shell_type": "shell_command",
                    "visibility": "list",
                    "minimal_client_version": [0, 99, 0],
                    "supported_in_api": true,
                    "priority": 1,
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
                    "experimental_supported_tools": [],
                }))
                .unwrap(),
            ],
        };

        let transport = CapturingTransport {
            last_request: Arc::new(Mutex::new(None)),
            body: Arc::new(response),
            etag: None,
        };

        let client = ModelsClient::new(
            transport,
            provider("https://example.com/api/codex"),
            Arc::new(DummyAuth),
        );

        let (models, _) = client
            .list_models("0.99.0", HeaderMap::new())
            .await
            .expect("request should succeed");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "gpt-test");
        assert_eq!(models[0].supported_in_api, true);
        assert_eq!(models[0].priority, 1);
    }

    #[test]
    fn parses_openai_compatible_models_response() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "openrouter/auto",
                        "name": "OpenRouter Auto",
                        "description": "Routes to a model automatically",
                        "context_length": 1_048_576,
                        "architecture": {
                            "input_modalities": ["text", "image", "file"]
                        },
                        "pricing": {
                            "prompt": "0.0000003",
                            "completion": "0.0000012",
                            "input_cache_read": "0.00000006",
                            "input_cache_write": "0"
                        },
                        "top_provider": {
                            "context_length": 524_288
                        },
                        "supported_parameters": ["tools", "temperature"]
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("openrouter"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        assert_eq!(
            models,
            vec![ModelInfo {
                slug: "openrouter/auto".to_string(),
                display_name: "OpenRouter Auto".to_string(),
                description: Some(
                    "Routes to a model automatically\n\nPricing: input $0.3, output $1.2, cache read $0.06, cache write $0 per 1M tokens."
                        .to_string()
                ),
                default_reasoning_level: None,
                supported_reasoning_levels: Vec::new(),
                shell_type: ConfigShellToolType::Default,
                visibility: ModelVisibility::List,
                supported_in_api: true,
                priority: 0,
                additional_speed_tiers: Vec::new(),
                service_tiers: Vec::new(),
                default_service_tier: None,
                availability_nux: None,
                upgrade: None,
                base_instructions: String::new(),
                model_messages: None,
                supports_reasoning_summaries: false,
                default_reasoning_summary: ReasoningSummary::Auto,
                support_verbosity: false,
                default_verbosity: None,
                apply_patch_tool_type: None,
                web_search_tool_type: WebSearchToolType::Text,
                truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
                supports_parallel_tool_calls: false,
                supports_image_detail_original: false,
                context_window: Some(524_288),
                max_context_window: Some(1_048_576),
                auto_compact_token_limit: None,
                effective_context_window_percent: 95,
                experimental_supported_tools: vec!["tools".to_string()],
                input_modalities: vec![InputModality::Text, InputModality::Image],
                used_fallback_model_metadata: false,
                supports_search_tool: false,
                tool_mode: None,
            }]
        );
    }

    #[test]
    fn openai_compatible_explicit_tool_flags_override_known_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "deepseek/deepseek-v4-flash",
                        "name": "DeepSeek V4 Flash",
                        "supported_parameters": {
                            "tools": false,
                            "parallel_tool_calls": true
                        }
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("openrouter"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.slug, "deepseek/deepseek-v4-flash");
        assert_eq!(model.context_window, Some(1_048_576));
        assert_eq!(model.experimental_supported_tools, Vec::<String>::new());
        assert!(!model.supports_parallel_tool_calls);
    }

    #[test]
    fn openai_compatible_function_calling_capability_enables_tools() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "custom-model",
                        "capabilities": {
                            "tools": false,
                            "function_calling": true
                        }
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("custom"),
            Some("Custom Provider"),
            Some("https://models.example.com/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.slug, "custom-model");
        assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    }

    #[test]
    fn openrouter_sparse_deepseek_response_uses_openrouter_slug_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "deepseek/deepseek-v4-flash"
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("openrouter"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.display_name, "DeepSeek V4 Flash");
        assert_eq!(model.context_window, Some(1_048_576));
        assert_eq!(model.max_context_window, Some(1_048_576));
        assert_eq!(model.experimental_supported_tools, vec!["tools"]);
        assert!(!model.supports_parallel_tool_calls);
    }

    #[test]
    fn renamed_openrouter_provider_uses_base_url_for_known_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "deepseek/deepseek-v4-flash"
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            None,
            Some("OpenRouter Mirror"),
            Some("https://openrouter.ai/api/v1/"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.display_name, "DeepSeek V4 Flash");
        assert_eq!(model.context_window, Some(1_048_576));
        assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    }

    #[test]
    fn unknown_provider_does_not_use_unqualified_known_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "gpt-oss-120b"
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("custom"),
            Some("Custom Provider"),
            Some("https://models.example.com/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.display_name, "gpt-oss-120b");
        assert_eq!(model.context_window, None);
        assert_eq!(model.experimental_supported_tools, Vec::<String>::new());
        assert!(!model.supports_reasoning_summaries);
    }

    #[test]
    fn custom_provider_name_does_not_select_known_provider_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "gpt-oss-120b"
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("custom"),
            Some("Cerebras"),
            Some("https://models.example.com/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.display_name, "gpt-oss-120b");
        assert_eq!(model.context_window, None);
        assert_eq!(model.experimental_supported_tools, Vec::<String>::new());
        assert!(!model.supports_reasoning_summaries);
    }

    #[test]
    fn dedicated_cerebras_provider_id_uses_known_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "gpt-oss-120b"
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("cerebras"),
            Some("Cerebras Dedicated"),
            Some("https://dedicated.cerebras.example.com/v1"),
        )
        .expect("OpenAI-compatible model response should decode");

        let model = &models[0];
        assert_eq!(model.display_name, "OpenAI GPT OSS 120B");
        assert_eq!(model.context_window, Some(131_072));
        assert_eq!(model.experimental_supported_tools, vec!["tools"]);
        assert!(model.supports_reasoning_summaries);
    }

    #[test]
    fn parses_cerebras_authenticated_models_response() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "object": "list",
                "data": [
                    {
                        "id": "gpt-oss-120b",
                        "object": "model",
                        "created": 0,
                        "owned_by": "Cerebras"
                    },
                    {
                        "id": "zai-glm-4.7",
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
            }))
            .unwrap()
            .as_bytes(),
            Some("cerebras"),
            Some("cerebras"),
            Some("https://api.cerebras.ai/v1"),
        )
        .expect("Cerebras authenticated model response should decode");

        assert_eq!(
            models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-oss-120b", "zai-glm-4.7", "account-scoped-model"]
        );
        assert_eq!(models[0].display_name, "OpenAI GPT OSS 120B");
        assert_eq!(models[0].context_window, Some(131_072));
        assert_eq!(models[0].max_context_window, Some(131_072));
        assert_eq!(models[0].experimental_supported_tools, vec!["tools"]);
        assert!(!models[0].supports_parallel_tool_calls);
        assert_eq!(
            models[0].default_reasoning_level,
            Some(ReasoningEffort::Medium)
        );
        assert!(models[0].supports_reasoning_summaries);
        assert_eq!(models[1].display_name, "Z.ai GLM 4.7");
        assert_eq!(models[1].context_window, Some(131_072));
        assert_eq!(models[1].max_context_window, Some(131_072));
        assert_eq!(models[1].experimental_supported_tools, vec!["tools"]);
        assert!(models[1].supports_parallel_tool_calls);
        assert_eq!(
            models[1].default_reasoning_level,
            Some(ReasoningEffort::Medium)
        );
        assert!(models[1].supports_reasoning_summaries);
        assert_eq!(models[2].display_name, "account-scoped-model");
        assert_eq!(models[2].input_modalities, vec![InputModality::Text]);
    }

    #[test]
    fn parses_cerebras_detailed_models_response() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "object": "list",
                "data": [
                    {
                        "id": "zai-glm-4.7",
                        "object": "model",
                        "created": 1767744000,
                        "owned_by": "Z.ai",
                        "name": "Z.ai GLM 4.7",
                        "description": "Fast coding model",
                        "capabilities": {
                            "function_calling": true,
                            "tools": true,
                            "parallel_tool_calls": true,
                            "reasoning": true
                        },
                        "supported_parameters": {
                            "temperature": true,
                            "tools": true,
                            "parallel_tool_calls": false
                        },
                        "architecture": {
                            "modality": "text"
                        },
                        "limits": {
                            "max_context_length": 131072
                        },
                        "pricing": {
                            "prompt": "0.00000225",
                            "completion": "0.00000275"
                        }
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("cerebras"),
            Some("cerebras"),
            Some("https://api.cerebras.ai/v1"),
        )
        .expect("Cerebras detailed model response should decode");

        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.slug, "zai-glm-4.7");
        assert_eq!(model.display_name, "Z.ai GLM 4.7");
        assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::Medium));
        assert_eq!(
            model
                .supported_reasoning_levels
                .iter()
                .map(|preset| preset.effort)
                .collect::<Vec<_>>(),
            vec![ReasoningEffort::None, ReasoningEffort::Medium]
        );
        assert!(!model.supports_parallel_tool_calls);
        assert!(model.supports_reasoning_summaries);
        assert_eq!(model.experimental_supported_tools, vec!["tools"]);
        assert_eq!(model.context_window, Some(131072));
        assert_eq!(model.max_context_window, Some(131072));
        assert_eq!(model.input_modalities, vec![InputModality::Text]);
    }

    #[test]
    fn explicit_reasoning_false_overrides_known_metadata() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "data": [
                    {
                        "id": "zai-glm-4.7",
                        "capabilities": {
                            "tools": true,
                            "reasoning": false
                        }
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("cerebras"),
            Some("cerebras"),
            Some("https://api.cerebras.ai/v1"),
        )
        .expect("Cerebras detailed model response should decode");

        let model = &models[0];
        assert_eq!(model.display_name, "Z.ai GLM 4.7");
        assert_eq!(model.context_window, Some(131_072));
        assert_eq!(model.default_reasoning_level, None);
        assert_eq!(model.supported_reasoning_levels, Vec::new());
        assert!(!model.supports_reasoning_summaries);
        assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    }

    #[test]
    fn parses_nvidia_models_response() {
        let models = decode_models_response(
            serde_json::to_string(&json!({
                "object": "list",
                "data": [
                    {
                        "id": "nvidia/llama-3.3-nemotron-super-49b-v1.5",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "nvidia"
                    },
                    {
                        "id": "qwen/qwen3-coder-480b-a35b-instruct",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "qwen"
                    },
                    {
                        "id": "openai/gpt-oss-120b",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "openai"
                    },
                    {
                        "id": "deepseek-ai/deepseek-v4-flash",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "deepseek-ai"
                    },
                    {
                        "id": "z-ai/glm-5.1",
                        "object": "model",
                        "created": 735790403,
                        "owned_by": "z-ai"
                    }
                ]
            }))
            .unwrap()
            .as_bytes(),
            Some("nvidia"),
            Some("nvidia"),
            Some("https://integrate.api.nvidia.com/v1"),
        )
        .expect("NVIDIA hosted model response should decode");

        assert_eq!(
            models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec![
                "nvidia/llama-3.3-nemotron-super-49b-v1.5",
                "qwen/qwen3-coder-480b-a35b-instruct",
                "openai/gpt-oss-120b",
                "deepseek-ai/deepseek-v4-flash",
                "z-ai/glm-5.1"
            ]
        );
        let gpt_oss = &models[2];
        assert_eq!(gpt_oss.display_name, "OpenAI GPT OSS 120B");
        assert_eq!(gpt_oss.context_window, Some(131_072));
        assert_eq!(gpt_oss.max_context_window, Some(131_072));
        assert_eq!(gpt_oss.experimental_supported_tools, vec!["tools"]);
        assert!(!gpt_oss.supports_parallel_tool_calls);
        assert!(gpt_oss.supports_reasoning_summaries);
        let deepseek = &models[3];
        assert_eq!(deepseek.display_name, "DeepSeek V4 Flash");
        assert_eq!(deepseek.context_window, Some(1_048_576));
        assert_eq!(deepseek.max_context_window, Some(1_048_576));
        assert_eq!(deepseek.experimental_supported_tools, vec!["tools"]);
        assert!(!deepseek.supports_parallel_tool_calls);
        let glm = &models[4];
        assert_eq!(glm.display_name, "Z.ai GLM 5.1");
        assert_eq!(glm.context_window, Some(131_072));
        assert_eq!(glm.max_context_window, Some(131_072));
        assert_eq!(glm.experimental_supported_tools, vec!["tools"]);
        assert!(!glm.supports_parallel_tool_calls);
        assert_eq!(glm.default_reasoning_level, None);
        assert_eq!(glm.supported_reasoning_levels, Vec::new());
        assert!(!glm.supports_reasoning_summaries);
    }

    #[tokio::test]
    async fn list_models_includes_etag() {
        let response = ModelsResponse { models: Vec::new() };

        let transport = CapturingTransport {
            last_request: Arc::new(Mutex::new(None)),
            body: Arc::new(response),
            etag: Some("\"abc\"".to_string()),
        };

        let client = ModelsClient::new(
            transport,
            provider("https://example.com/api/codex"),
            Arc::new(DummyAuth),
        );

        let (models, etag) = client
            .list_models("0.1.0", HeaderMap::new())
            .await
            .expect("request should succeed");

        assert_eq!(models.len(), 0);
        assert_eq!(etag, Some("\"abc\"".to_string()));
    }
}
