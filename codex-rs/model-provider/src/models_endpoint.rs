use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use codex_api::ModelsClient;
use codex_api::RequestTelemetry;
use codex_api::ReqwestTransport;
use codex_api::TransportError;
use codex_api::auth_header_telemetry;
use codex_api::map_api_error;
use codex_feedback::FeedbackRequestTags;
use codex_feedback::emit_feedback_request_tags_with_auth_env;
use codex_login::AuthEnvTelemetry;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::collect_auth_env_telemetry;
use codex_login::default_client::build_reqwest_client;
use codex_model_provider_info::ANTHROPIC_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::manager::ModelsEndpointClient;
use codex_otel::TelemetryAuthMode;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::openai_models::ModelInfo;
use codex_response_debug_context::extract_response_debug_context;
use codex_response_debug_context::telemetry_transport_error_message;
use http::HeaderMap;
use http::HeaderValue;
use http::header::HeaderName;
use tokio::time::timeout;

use crate::auth::resolve_provider_model_list_auth;

const MODELS_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);
const MODELS_ENDPOINT: &str = "/models";
const ANTHROPIC_API_KEY_HEADER: HeaderName = HeaderName::from_static("x-api-key");
const ANTHROPIC_VERSION_HEADER: HeaderName = HeaderName::from_static("anthropic-version");
const ANTHROPIC_VERSION_VALUE: HeaderValue = HeaderValue::from_static("2023-06-01");

/// Provider-owned OpenAI-compatible `/models` endpoint.
#[derive(Debug)]
pub(crate) struct OpenAiModelsEndpoint {
    provider_id: String,
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl OpenAiModelsEndpoint {
    pub(crate) fn new(
        provider_id: impl Into<String>,
        provider_info: ModelProviderInfo,
        auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_info,
            auth_manager,
        }
    }

    async fn auth(&self) -> Option<CodexAuth> {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager.auth().await,
            None => None,
        }
    }

    fn auth_env(&self) -> AuthEnvTelemetry {
        let codex_api_key_env_enabled = self
            .auth_manager
            .as_ref()
            .is_some_and(|auth_manager| auth_manager.codex_api_key_env_enabled());
        collect_auth_env_telemetry(&self.provider_info, codex_api_key_env_enabled)
    }

    fn extra_model_list_headers(&self) -> CoreResult<HeaderMap> {
        if !self.provider_id.eq_ignore_ascii_case(ANTHROPIC_PROVIDER_ID) {
            return Ok(HeaderMap::new());
        }

        let api_key = self.provider_info.api_key()?.unwrap_or_default();
        anthropic_model_list_headers(&api_key)
    }
}

fn anthropic_model_list_headers(api_key: &str) -> CoreResult<HeaderMap> {
    let api_key = HeaderValue::from_str(api_key).map_err(|err| {
        CodexErr::InvalidRequest(format!("invalid Anthropic API key header: {err}"))
    })?;
    let mut headers = HeaderMap::new();
    headers.insert(ANTHROPIC_API_KEY_HEADER, api_key);
    headers.insert(ANTHROPIC_VERSION_HEADER, ANTHROPIC_VERSION_VALUE);
    Ok(headers)
}

#[async_trait]
impl ModelsEndpointClient for OpenAiModelsEndpoint {
    fn has_provider_auth(&self) -> bool {
        self.provider_info.has_command_auth()
            || self.provider_info.env_key.is_some()
            || self.provider_info.experimental_bearer_token.is_some()
            || self.provider_info.aws.is_some()
    }

    async fn uses_codex_backend(&self) -> bool {
        self.auth()
            .await
            .as_ref()
            .is_some_and(CodexAuth::uses_codex_backend)
    }

    async fn list_models(
        &self,
        client_version: &str,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        let _timer =
            codex_otel::start_global_timer("codex.remote_models.fetch_update.duration_ms", &[]);
        let auth = self.auth().await;
        // The `client_version` query parameter is only understood by the Codex
        // backend. Third-party OpenAI-compatible `/models` endpoints (e.g.
        // Google Gemini's `/v1beta/openai`) reject unknown query parameters
        // with a 400, so only append it when talking to the Codex backend.
        let uses_codex_backend = auth.as_ref().is_some_and(CodexAuth::uses_codex_backend);
        let auth_mode = auth.as_ref().map(CodexAuth::auth_mode);
        let api_provider = self.provider_info.to_api_provider(auth_mode)?;
        let api_auth = resolve_provider_model_list_auth(auth.as_ref(), &self.provider_info)?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let auth_telemetry = auth_header_telemetry(api_auth.as_ref());
        let request_telemetry: Arc<dyn RequestTelemetry> = Arc::new(ModelsRequestTelemetry {
            auth_mode: auth_mode.map(|mode| TelemetryAuthMode::from(mode).to_string()),
            auth_header_attached: auth_telemetry.attached,
            auth_header_name: auth_telemetry.name,
            auth_env: self.auth_env(),
        });
        let client = ModelsClient::new(transport, api_provider, api_auth)
            .with_provider_id(Some(self.provider_id.clone()))
            .with_client_version_query(uses_codex_backend)
            .with_telemetry(Some(request_telemetry));
        let extra_headers = self.extra_model_list_headers()?;

        timeout(
            MODELS_REFRESH_TIMEOUT,
            client.list_models(client_version, extra_headers),
        )
        .await
        .map_err(|_| CodexErr::Timeout)?
        .map_err(map_api_error)
    }
}

#[derive(Clone)]
struct ModelsRequestTelemetry {
    auth_mode: Option<String>,
    auth_header_attached: bool,
    auth_header_name: Option<&'static str>,
    auth_env: AuthEnvTelemetry,
}

impl RequestTelemetry for ModelsRequestTelemetry {
    fn on_request(
        &self,
        attempt: u64,
        status: Option<http::StatusCode>,
        error: Option<&TransportError>,
        duration: Duration,
    ) {
        let success = status.is_some_and(|code| code.is_success()) && error.is_none();
        let error_message = error.map(telemetry_transport_error_message);
        let response_debug = error
            .map(extract_response_debug_context)
            .unwrap_or_default();
        let status = status.map(|status| status.as_u16());
        tracing::event!(
            target: "codex_otel.log_only",
            tracing::Level::INFO,
            event.name = "codex.api_request",
            duration_ms = %duration.as_millis(),
            http.response.status_code = status,
            success = success,
            error.message = error_message.as_deref(),
            attempt = attempt,
            endpoint = MODELS_ENDPOINT,
            auth.header_attached = self.auth_header_attached,
            auth.header_name = self.auth_header_name,
            auth.env_openai_api_key_present = self.auth_env.openai_api_key_env_present,
            auth.env_codex_api_key_present = self.auth_env.codex_api_key_env_present,
            auth.env_codex_api_key_enabled = self.auth_env.codex_api_key_env_enabled,
            auth.env_provider_key_name = self.auth_env.provider_env_key_name.as_deref(),
            auth.env_provider_key_present = self.auth_env.provider_env_key_present,
            auth.env_refresh_token_url_override_present = self.auth_env.refresh_token_url_override_present,
            auth.request_id = response_debug.request_id.as_deref(),
            auth.cf_ray = response_debug.cf_ray.as_deref(),
            auth.error = response_debug.auth_error.as_deref(),
            auth.error_code = response_debug.auth_error_code.as_deref(),
            auth.mode = self.auth_mode.as_deref(),
        );
        tracing::event!(
            target: "codex_otel.trace_safe",
            tracing::Level::INFO,
            event.name = "codex.api_request",
            duration_ms = %duration.as_millis(),
            http.response.status_code = status,
            success = success,
            error.message = error_message.as_deref(),
            attempt = attempt,
            endpoint = MODELS_ENDPOINT,
            auth.header_attached = self.auth_header_attached,
            auth.header_name = self.auth_header_name,
            auth.env_openai_api_key_present = self.auth_env.openai_api_key_env_present,
            auth.env_codex_api_key_present = self.auth_env.codex_api_key_env_present,
            auth.env_codex_api_key_enabled = self.auth_env.codex_api_key_env_enabled,
            auth.env_provider_key_name = self.auth_env.provider_env_key_name.as_deref(),
            auth.env_provider_key_present = self.auth_env.provider_env_key_present,
            auth.env_refresh_token_url_override_present = self.auth_env.refresh_token_url_override_present,
            auth.request_id = response_debug.request_id.as_deref(),
            auth.cf_ray = response_debug.cf_ray.as_deref(),
            auth.error = response_debug.auth_error.as_deref(),
            auth.error_code = response_debug.auth_error_code.as_deref(),
            auth.mode = self.auth_mode.as_deref(),
        );
        emit_feedback_request_tags_with_auth_env(
            &FeedbackRequestTags {
                endpoint: MODELS_ENDPOINT,
                auth_header_attached: self.auth_header_attached,
                auth_header_name: self.auth_header_name,
                auth_mode: self.auth_mode.as_deref(),
                auth_retry_after_unauthorized: None,
                auth_recovery_mode: None,
                auth_recovery_phase: None,
                auth_connection_reused: None,
                auth_request_id: response_debug.request_id.as_deref(),
                auth_cf_ray: response_debug.cf_ray.as_deref(),
                auth_error: response_debug.auth_error.as_deref(),
                auth_error_code: response_debug.auth_error_code.as_deref(),
                auth_recovery_followup_success: None,
                auth_recovery_followup_status: None,
            },
            &self.auth_env,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use codex_protocol::config_types::ModelProviderAuthInfo;

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

    #[test]
    fn command_auth_provider_reports_provider_auth_without_cached_auth() {
        let endpoint = OpenAiModelsEndpoint::new(
            "test-provider",
            provider_info_with_command_auth(),
            /*auth_manager*/ None,
        );

        assert!(endpoint.has_provider_auth());
    }

    #[test]
    fn env_key_provider_reports_provider_auth_without_cached_auth() {
        let endpoint = OpenAiModelsEndpoint::new(
            "test-provider",
            ModelProviderInfo {
                env_key: Some("OPENROUTER_API_KEY".to_string()),
                ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
            },
            /*auth_manager*/ None,
        );

        assert!(endpoint.has_provider_auth());
    }

    #[test]
    fn provider_without_provider_auth_reports_no_provider_auth() {
        let endpoint = OpenAiModelsEndpoint::new(
            "test-provider",
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert!(!endpoint.has_provider_auth());
    }

    #[test]
    fn anthropic_model_list_headers_include_native_auth_headers() {
        let headers =
            anthropic_model_list_headers("anthropic-token").expect("headers should build");

        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("anthropic-token")
        );
        assert_eq!(
            headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
    }
}
