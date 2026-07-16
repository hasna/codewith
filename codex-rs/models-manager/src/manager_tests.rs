use super::*;
use crate::ModelsManagerConfig;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::ExternalAuth;
use codex_login::ExternalAuthRefreshContext;
use codex_login::ExternalAuthTokens;
use codex_login::TokenData;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::tempdir;

const GPT_5_5_MODEL_ID: &str = "gpt-5.5";
const GPT_5_5_CONTEXT_WINDOW: i64 = 1_050_000;
const GPT_5_4_MODEL_ID: &str = "gpt-5.4";
const GPT_5_4_CONTEXT_WINDOW: i64 = 1_050_000;
const CHATGPT_DEFAULT_GPT_CONTEXT_WINDOW: i64 = 272_000;

#[path = "model_info_overrides_tests.rs"]
mod model_info_overrides_tests;

fn remote_model(slug: &str, display: &str, priority: i32) -> ModelInfo {
    remote_model_with_visibility(slug, display, priority, "list")
}

fn remote_model_with_visibility(
    slug: &str,
    display: &str,
    priority: i32,
    visibility: &str,
) -> ModelInfo {
    serde_json::from_value(json!({
            "slug": slug,
            "display_name": display,
            "description": format!("{display} desc"),
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort": "low", "description": "low"}, {"effort": "medium", "description": "medium"}],
            "shell_type": "shell_command",
            "visibility": visibility,
            "minimal_client_version": [0, 1, 0],
            "supported_in_api": true,
            "priority": priority,
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

fn stale_openai_gpt_context_models() -> Vec<ModelInfo> {
    let mut remote_gpt_5_5 = remote_model(GPT_5_5_MODEL_ID, "Remote GPT-5.5", /*priority*/ 0);
    remote_gpt_5_5.context_window = Some(272_000);
    remote_gpt_5_5.max_context_window = Some(272_000);
    let mut remote_gpt_5_4 = remote_model(GPT_5_4_MODEL_ID, "Remote GPT-5.4", /*priority*/ 1);
    remote_gpt_5_4.context_window = Some(272_000);
    remote_gpt_5_4.max_context_window = Some(272_000);
    vec![remote_gpt_5_5, remote_gpt_5_4]
}

async fn assert_api_gpt_context_windows(manager: &OpenAiModelsManager) {
    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };
    for (slug, expected_context_window) in [
        (GPT_5_5_MODEL_ID, GPT_5_5_CONTEXT_WINDOW),
        (GPT_5_4_MODEL_ID, GPT_5_4_CONTEXT_WINDOW),
    ] {
        let model_info = manager.get_model_info(slug, &config).await;
        assert_eq!(model_info.context_window, Some(expected_context_window));
        assert_eq!(model_info.max_context_window, Some(expected_context_window));
        assert!(
            !model_info.used_fallback_model_metadata,
            "{slug} should use refreshed model metadata"
        );
    }
}

fn assert_models_contain(actual: &[ModelInfo], expected: &[ModelInfo]) {
    for model in expected {
        assert!(
            actual.iter().any(|candidate| candidate.slug == model.slug),
            "expected model {} in cached list",
            model.slug
        );
    }
}

fn required_bundled_remote_gap_models(required_models: &[ModelInfo]) -> Vec<ModelInfo> {
    let hidden_required = required_models
        .first()
        .expect("bundled registry should mark at least one required local model");
    vec![
        remote_model_with_visibility(
            &hidden_required.slug,
            &hidden_required.display_name,
            /*priority*/ 0,
            "hide",
        ),
        remote_model(
            "chatgpt-authoritative-model-info",
            "ChatGPT Model Info",
            /*priority*/ 10,
        ),
    ]
}

fn assert_required_bundled_models_available_once(
    actual_models: &[ModelInfo],
    expected_required: &[ModelInfo],
) {
    for expected in expected_required {
        let matches = actual_models
            .iter()
            .filter(|model| model.slug == expected.slug)
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "required model {} should appear exactly once",
            expected.slug
        );
        assert_eq!(matches[0], expected);
    }
}

#[derive(Debug)]
struct TestModelsEndpoint {
    has_provider_auth: bool,
    uses_codex_backend: bool,
    responses: Mutex<VecDeque<Vec<ModelInfo>>>,
    fetch_count: AtomicUsize,
}

impl TestModelsEndpoint {
    fn new(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            has_provider_auth: false,
            uses_codex_backend: true,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn without_refresh(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            has_provider_auth: false,
            uses_codex_backend: false,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct TestExternalApiKeyAuth;

#[async_trait]
impl ExternalAuth for TestExternalApiKeyAuth {
    fn auth_mode(&self) -> AuthMode {
        AuthMode::ApiKey
    }

    async fn resolve(&self) -> std::io::Result<Option<ExternalAuthTokens>> {
        Ok(Some(ExternalAuthTokens::access_token_only(
            "test-external-api-key",
        )))
    }

    async fn refresh(
        &self,
        _context: ExternalAuthRefreshContext,
    ) -> std::io::Result<ExternalAuthTokens> {
        Ok(ExternalAuthTokens::access_token_only(
            "test-external-api-key",
        ))
    }
}

#[derive(Debug)]
struct TestUnresolvedExternalApiKeyAuth;

#[async_trait]
impl ExternalAuth for TestUnresolvedExternalApiKeyAuth {
    fn auth_mode(&self) -> AuthMode {
        AuthMode::ApiKey
    }

    async fn refresh(
        &self,
        _context: ExternalAuthRefreshContext,
    ) -> std::io::Result<ExternalAuthTokens> {
        Err(std::io::Error::other("unresolved test auth"))
    }
}

#[async_trait]
impl ModelsEndpointClient for TestModelsEndpoint {
    fn has_provider_auth(&self) -> bool {
        self.has_provider_auth
    }

    async fn uses_codex_backend(&self) -> bool {
        self.uses_codex_backend
    }

    async fn list_models(
        &self,
        _client_version: &str,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let models = self
            .responses
            .lock()
            .expect("responses lock should not be poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok((models, None))
    }
}

fn openai_manager_for_tests(
    codex_home: std::path::PathBuf,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
) -> OpenAiModelsManager {
    openai_manager_for_tests_with_auth(
        codex_home,
        endpoint_client,
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
    )
}

fn openai_manager_for_tests_with_auth(
    codex_home: std::path::PathBuf,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
    auth_manager: Option<Arc<AuthManager>>,
) -> OpenAiModelsManager {
    openai_manager_for_tests_with_provider_key(
        codex_home,
        "openai".to_string(),
        endpoint_client,
        auth_manager,
    )
}

fn openai_manager_for_tests_with_provider_key(
    codex_home: std::path::PathBuf,
    provider_cache_key: String,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
    auth_manager: Option<Arc<AuthManager>>,
) -> OpenAiModelsManager {
    openai_manager_for_tests_with_provider_key_and_catalog(
        codex_home,
        provider_cache_key,
        BundledModelCatalog::UseAsFallback,
        endpoint_client,
        auth_manager,
    )
}

#[tokio::test]
async fn chatgpt_explicit_bare_gpt_5_6_resolves_to_sol() {
    let temp_dir = tempdir().expect("tempdir");
    let manager = openai_manager_for_tests(
        temp_dir.path().to_path_buf(),
        TestModelsEndpoint::without_refresh(Vec::new()),
    );

    let model = manager
        .get_default_model(
            &Some("gpt-5.6".to_string()),
            &ModelsManagerConfig {
                model_provider_id: Some("openai".to_string()),
                ..Default::default()
            },
            RefreshStrategy::Offline,
        )
        .await;

    assert_eq!(model, "gpt-5.6-sol");
}

#[tokio::test]
async fn api_key_explicit_bare_gpt_5_6_remains_unchanged() {
    let temp_dir = tempdir().expect("tempdir");
    let manager = openai_manager_for_tests_with_auth(
        temp_dir.path().to_path_buf(),
        TestModelsEndpoint::without_refresh(Vec::new()),
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );

    let model = manager
        .get_default_model(
            &Some("gpt-5.6".to_string()),
            &ModelsManagerConfig {
                model_provider_id: Some("openai".to_string()),
                ..Default::default()
            },
            RefreshStrategy::Offline,
        )
        .await;

    assert_eq!(model, "gpt-5.6");
}

#[tokio::test]
async fn chatgpt_model_resolution_only_migrates_exact_bare_openai_gpt_5_6() {
    let temp_dir = tempdir().expect("tempdir");
    let manager = openai_manager_for_tests(
        temp_dir.path().to_path_buf(),
        TestModelsEndpoint::without_refresh(Vec::new()),
    );
    let openai_config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };

    let migrated = manager.get_model_info("gpt-5.6", &openai_config).await;
    assert_eq!(migrated.slug, "gpt-5.6-sol");

    for model in [
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "openai/gpt-5.6",
    ] {
        let resolved = manager.get_model_info(model, &openai_config).await;
        assert_eq!(resolved.slug, model);
    }

    let custom_config = ModelsManagerConfig {
        model_provider_id: Some("custom-openai-compatible".to_string()),
        ..Default::default()
    };
    let custom_default = manager
        .get_default_model(
            &Some("gpt-5.6".to_string()),
            &custom_config,
            RefreshStrategy::Offline,
        )
        .await;
    assert_eq!(custom_default, "gpt-5.6");
    let custom_info = manager.get_model_info("gpt-5.6", &custom_config).await;
    assert_eq!(custom_info.slug, "gpt-5.6");
}

fn openai_manager_for_tests_with_provider_key_and_catalog(
    codex_home: std::path::PathBuf,
    provider_cache_key: String,
    bundled_model_catalog: BundledModelCatalog,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
    auth_manager: Option<Arc<AuthManager>>,
) -> OpenAiModelsManager {
    OpenAiModelsManager::new(
        codex_home,
        provider_cache_key,
        bundled_model_catalog,
        endpoint_client,
        auth_manager,
    )
}

fn static_manager_for_tests(model_catalog: ModelsResponse) -> StaticModelsManager {
    StaticModelsManager::new(/*auth_manager*/ None, model_catalog)
}

fn with_required_local_models(mut models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    crate::model_info::ensure_required_local_models(&mut models);
    models
}

async fn chatgpt_auth_tokens_for_tests(codex_home: &Path) -> CodexAuth {
    let auth_dot_json = codex_login::AuthDotJson {
        auth_mode: Some(AuthMode::ChatgptAuthTokens),
        openai_api_key: None,
        tokens: Some(TokenData {
            id_token: codex_login::token_data::parse_chatgpt_jwt_claims(
                "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.\
eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwcm8iLCJjaGF0Z3B0X3VzZXJfaWQiOiJ1c2VyLWlkIiwiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjb3VudC1pZCJ9fQ.\
c2ln",
            )
            .expect("fake id token should parse"),
            access_token: "Access Token".to_string(),
            refresh_token: "test".to_string(),
            account_id: Some("account_id".to_string()),
        }),
        last_refresh: Some(Utc::now()),
        agent_identity: None,
        personal_access_token: None,
    };
    std::fs::create_dir_all(codex_home).expect("codex home should be created");
    std::fs::write(
        codex_home.join("auth.json"),
        serde_json::to_string(&auth_dot_json).expect("auth should serialize"),
    )
    .expect("auth.json should be written");

    CodexAuth::from_auth_storage(
        codex_home,
        AuthCredentialsStoreMode::File,
        /*chatgpt_base_url*/ None,
    )
    .await
    .expect("auth should load")
    .expect("auth should be present")
}

#[tokio::test]
async fn get_model_info_tracks_fallback_usage() {
    let codex_home = tempdir().expect("temp dir");
    let config = ModelsManagerConfig::default();
    let manager = openai_manager_for_tests(
        codex_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let known_slug = manager
        .get_remote_models()
        .await
        .first()
        .expect("bundled models should include at least one model")
        .slug
        .clone();

    let known = manager.get_model_info(known_slug.as_str(), &config).await;
    assert!(!known.used_fallback_model_metadata);
    assert_eq!(known.slug, known_slug);

    let unknown = manager
        .get_model_info("model-that-does-not-exist", &config)
        .await;
    assert!(unknown.used_fallback_model_metadata);
    assert_eq!(unknown.slug, "model-that-does-not-exist");
}

#[tokio::test]
async fn get_model_info_uses_provider_scoped_fallback_metadata() {
    let manager = static_manager_for_tests(ModelsResponse { models: Vec::new() });
    let config = ModelsManagerConfig {
        model_provider_id: Some("openrouter".to_string()),
        ..Default::default()
    };

    let stale_nvidia_slug = manager
        .get_model_info("deepseek-ai/deepseek-v4-flash", &config)
        .await;
    assert!(stale_nvidia_slug.used_fallback_model_metadata);
    assert_eq!(
        stale_nvidia_slug.experimental_supported_tools,
        Vec::<String>::new()
    );

    let openrouter_slug = manager
        .get_model_info("deepseek/deepseek-v4-flash", &config)
        .await;
    assert!(!openrouter_slug.used_fallback_model_metadata);
    assert_eq!(openrouter_slug.display_name, "DeepSeek V4 Flash");
    assert_eq!(openrouter_slug.context_window, Some(1_048_576));
    assert_eq!(openrouter_slug.experimental_supported_tools, vec!["tools"]);
}

#[tokio::test]
async fn get_model_info_uses_custom_catalog() {
    let config = ModelsManagerConfig::default();
    let mut overlay = remote_model("gpt-overlay", "Overlay", /*priority*/ 0);
    overlay.supports_image_detail_original = true;

    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![overlay],
    });

    let model_info = manager
        .get_model_info("gpt-overlay-experiment", &config)
        .await;

    assert_eq!(model_info.slug, "gpt-overlay-experiment");
    assert_eq!(model_info.display_name, "Overlay");
    assert_eq!(model_info.context_window, Some(272_000));
    assert!(model_info.supports_image_detail_original);
    assert!(!model_info.supports_parallel_tool_calls);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_matches_namespaced_suffix() {
    let config = ModelsManagerConfig::default();
    let mut remote = remote_model("gpt-image", "Image", /*priority*/ 0);
    remote.supports_image_detail_original = true;
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote],
    });
    let namespaced_model = "custom/gpt-image".to_string();

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(model_info.supports_image_detail_original);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_matches_hyphenated_provider_namespace_suffix() {
    let config = ModelsManagerConfig::default();
    let remote = remote_model("gpt-image", "Image", /*priority*/ 0);
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote],
    });
    let namespaced_model = "openai-codex/gpt-image".to_string();

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_rejects_multi_segment_namespace_suffix_matching() {
    let codex_home = tempdir().expect("temp dir");
    let config = ModelsManagerConfig::default();
    let manager = openai_manager_for_tests(
        codex_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let known_slug = manager
        .get_remote_models()
        .await
        .first()
        .expect("bundled models should include at least one model")
        .slug
        .clone();
    let namespaced_model = format!("ns1/ns2/{known_slug}");

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn refresh_available_models_sorts_by_priority() {
    let remote_models = vec![
        remote_model("priority-low", "Low", /*priority*/ 1),
        remote_model("priority-high", "High", /*priority*/ 0),
    ];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");
    let cached_remote = manager.get_remote_models().await;
    assert_models_contain(&cached_remote, &remote_models);

    let available = manager.list_models(RefreshStrategy::OnlineIfUncached).await;
    let high_idx = available
        .iter()
        .position(|model| model.model == "priority-high")
        .expect("priority-high should be listed");
    let low_idx = available
        .iter()
        .position(|model| model.model == "priority-low")
        .expect("priority-low should be listed");
    assert!(
        high_idx < low_idx,
        "higher priority should be listed before lower priority"
    );
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_uses_remote_only_catalog_for_chatgpt_auth() {
    let remote_models = vec![remote_model(
        "chatgpt-visible-source-of-truth",
        "ChatGPT Visible",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(
        manager.get_remote_models().await,
        with_required_local_models(remote_models)
    );
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn get_model_info_keeps_chatgpt_remote_gpt_5_5_context_window() {
    let remote_context_window = 273_000;
    let remote_max_context_window = 274_000;
    let mut remote = remote_model(GPT_5_5_MODEL_ID, "GPT-5.5", /*priority*/ 0);
    remote.context_window = Some(remote_context_window);
    remote.max_context_window = Some(remote_max_context_window);
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![remote]]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };
    let model_info = manager.get_model_info(GPT_5_5_MODEL_ID, &config).await;

    assert_eq!(model_info.context_window, Some(remote_context_window));
    assert_eq!(
        model_info.max_context_window,
        Some(remote_max_context_window)
    );
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_caps_bundled_api_sized_gpt_context_window_for_chatgpt_auth() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(Vec::new());
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };

    for (slug, expected_fallback_metadata) in [
        ("gpt-5.4", false),
        ("gpt-5.5", false),
        ("gpt-5.6", false),
        ("gpt-5.6-sol", false),
        ("gpt-5.6-terra", false),
        ("gpt-5.6-luna", false),
        ("openai/gpt-5.5", false),
    ] {
        let model_info = manager.get_model_info(slug, &config).await;
        assert_eq!(
            model_info.context_window,
            Some(CHATGPT_DEFAULT_GPT_CONTEXT_WINDOW),
            "{slug} should use the subscription-sized default context window"
        );
        assert_eq!(
            model_info.max_context_window,
            Some(CHATGPT_DEFAULT_GPT_CONTEXT_WINDOW),
            "{slug} should use the subscription-sized default max context window"
        );
        assert_eq!(
            model_info.used_fallback_model_metadata, expected_fallback_metadata,
            "{slug} fallback metadata state"
        );
    }
}

#[tokio::test]
async fn chatgpt_auth_caps_api_sized_remote_gpt_context_window() {
    let mut remote = remote_model(GPT_5_5_MODEL_ID, "GPT-5.5", /*priority*/ 0);
    remote.context_window = Some(GPT_5_5_CONTEXT_WINDOW);
    remote.max_context_window = Some(GPT_5_5_CONTEXT_WINDOW);
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![remote]]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };
    let model_info = manager.get_model_info(GPT_5_5_MODEL_ID, &config).await;

    assert_eq!(
        model_info.context_window,
        Some(CHATGPT_DEFAULT_GPT_CONTEXT_WINDOW)
    );
    assert_eq!(
        model_info.max_context_window,
        Some(CHATGPT_DEFAULT_GPT_CONTEXT_WINDOW)
    );
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn chatgpt_auth_preserves_explicit_gpt_context_window_override() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(Vec::new());
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let model_info = manager.get_model_info(GPT_5_5_MODEL_ID, &config).await;

    assert_eq!(model_info.context_window, Some(500_000));
    assert_eq!(model_info.max_context_window, Some(GPT_5_5_CONTEXT_WINDOW));
}

#[tokio::test]
async fn chatgpt_auth_preserves_api_sized_gpt_context_window_for_non_openai_provider() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(Vec::new());
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let config = ModelsManagerConfig {
        model_provider_id: Some("openrouter".to_string()),
        ..Default::default()
    };

    for slug in [GPT_5_5_MODEL_ID, "openrouter/gpt-5.5"] {
        let model_info = manager.get_model_info(slug, &config).await;
        assert_eq!(model_info.context_window, Some(GPT_5_5_CONTEXT_WINDOW));
        assert_eq!(model_info.max_context_window, Some(GPT_5_5_CONTEXT_WINDOW));
    }
}

#[tokio::test]
async fn get_model_info_keeps_chatgpt_remote_gpt_5_4_context_window() {
    let remote_context_window = 273_000;
    let remote_max_context_window = 274_000;
    let mut remote = remote_model(GPT_5_4_MODEL_ID, "GPT-5.4", /*priority*/ 0);
    remote.context_window = Some(remote_context_window);
    remote.max_context_window = Some(remote_max_context_window);
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![remote]]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };
    let model_info = manager.get_model_info(GPT_5_4_MODEL_ID, &config).await;

    assert_eq!(model_info.context_window, Some(remote_context_window));
    assert_eq!(
        model_info.max_context_window,
        Some(remote_max_context_window)
    );
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_keeps_api_auth_bundled_gpt_5_5_context_window() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(Vec::new());
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint,
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );
    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };

    let model_info = manager.get_model_info(GPT_5_5_MODEL_ID, &config).await;

    assert_eq!(model_info.context_window, Some(GPT_5_5_CONTEXT_WINDOW));
    assert_eq!(model_info.max_context_window, Some(GPT_5_5_CONTEXT_WINDOW));
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_keeps_api_auth_bundled_gpt_5_4_context_window() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(Vec::new());
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint,
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );
    let config = ModelsManagerConfig {
        model_provider_id: Some("openai".to_string()),
        ..Default::default()
    };

    let model_info = manager.get_model_info(GPT_5_4_MODEL_ID, &config).await;

    assert_eq!(model_info.context_window, Some(GPT_5_4_CONTEXT_WINDOW));
    assert_eq!(model_info.max_context_window, Some(GPT_5_4_CONTEXT_WINDOW));
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn api_auth_refresh_keeps_bundled_gpt_context_window_over_stale_remote_response() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = Arc::new(TestModelsEndpoint {
        has_provider_auth: true,
        uses_codex_backend: false,
        responses: Mutex::new(vec![stale_openai_gpt_context_models()].into()),
        fetch_count: AtomicUsize::new(0),
    });
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint,
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_api_gpt_context_windows(&manager).await;
}

#[tokio::test]
async fn api_auth_cache_hit_keeps_bundled_gpt_context_window_over_stale_remote_cache() {
    let codex_home = tempdir().expect("temp dir");
    let seed_endpoint = Arc::new(TestModelsEndpoint {
        has_provider_auth: true,
        uses_codex_backend: false,
        responses: Mutex::new(vec![stale_openai_gpt_context_models()].into()),
        fetch_count: AtomicUsize::new(0),
    });
    let seed_manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        seed_endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );
    seed_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("seed refresh succeeds");
    assert_eq!(
        seed_endpoint.fetch_count(),
        1,
        "seed manager should write stale remote metadata into the cache"
    );

    let cache_endpoint = Arc::new(TestModelsEndpoint {
        has_provider_auth: true,
        uses_codex_backend: false,
        responses: Mutex::new(VecDeque::new()),
        fetch_count: AtomicUsize::new(0),
    });
    let cache_manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        cache_endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );
    cache_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");

    assert_eq!(
        cache_endpoint.fetch_count(),
        0,
        "fresh cache should avoid a model fetch"
    );
    assert_api_gpt_context_windows(&cache_manager).await;
}

#[tokio::test]
async fn refresh_available_models_uses_cached_remote_only_catalog_for_chatgpt_auth() {
    let remote_models = vec![remote_model(
        "chatgpt-cached-source-of-truth",
        "ChatGPT Cached",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let fetch_endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let fetch_manager =
        openai_manager_for_tests(codex_home.path().to_path_buf(), fetch_endpoint.clone());

    fetch_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    let cache_endpoint = TestModelsEndpoint::new(Vec::new());
    let cache_manager =
        openai_manager_for_tests(codex_home.path().to_path_buf(), cache_endpoint.clone());

    cache_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");

    assert_eq!(
        cache_manager.get_remote_models().await,
        with_required_local_models(remote_models)
    );
    assert_eq!(
        cache_endpoint.fetch_count(),
        0,
        "fresh cache should avoid a model fetch"
    );
}

#[tokio::test]
async fn get_model_info_uses_fallback_for_bundled_models_when_chatgpt_remote_is_authoritative() {
    let remote_models = vec![remote_model(
        "chatgpt-authoritative-model-info",
        "ChatGPT Model Info",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");
    let remote_models = manager.get_remote_models().await;
    let bundled_slug = load_remote_models_from_file()
        .expect("bundled models should parse")
        .into_iter()
        .find(|bundled| {
            !remote_models
                .iter()
                .any(|remote| remote.slug == bundled.slug)
        })
        .expect("bundled models should contain at least one non-required model")
        .slug;

    let model_info = manager
        .get_model_info(&bundled_slug, &ModelsManagerConfig::default())
        .await;

    assert_eq!(model_info.slug, bundled_slug);
    assert!(model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn refresh_available_models_keeps_codex_spark_when_chatgpt_remote_omits_it() {
    let remote_models = vec![remote_model(
        "chatgpt-authoritative-model-info",
        "ChatGPT Model Info",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let models = manager.get_remote_models().await;
    assert_models_contain(&models, &remote_models);
    let spark = models
        .iter()
        .find(|model| model.slug == crate::model_info::GPT_5_3_CODEX_SPARK)
        .expect("Spark should remain locally available for ChatGPT auth");

    assert_eq!(spark.input_modalities, vec![InputModality::Text]);
    assert_eq!(spark.default_reasoning_level, Some(ReasoningEffort::High));
    assert!(!spark.supported_in_api);
}

#[tokio::test]
async fn refresh_available_models_keeps_required_bundled_models_for_chatgpt_remote() {
    let required_models = crate::model_info::required_bundled_model_infos();
    let remote_models = required_bundled_remote_gap_models(&required_models);
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let models = manager.get_remote_models().await;
    assert_models_contain(&models, &remote_models);
    assert_required_bundled_models_available_once(&models, &required_models);
}

#[tokio::test]
async fn chatgpt_remote_catalog_omission_injects_only_supported_gpt_5_6_variants() {
    let remote_models = vec![remote_model(
        "chatgpt-authoritative-model-info",
        "ChatGPT Model Info",
        /*priority*/ 10,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let gpt_5_6_models = manager
        .list_models(RefreshStrategy::Offline)
        .await
        .into_iter()
        .filter(|model| model.model.starts_with("gpt-5.6"))
        .collect::<Vec<_>>();

    assert_eq!(
        gpt_5_6_models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        vec!["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]
    );
    assert!(gpt_5_6_models[0].is_default);
}

#[tokio::test]
async fn refresh_available_models_keeps_required_bundled_models_for_chatgpt_cache() {
    let required_models = crate::model_info::required_bundled_model_infos();
    let remote_models = required_bundled_remote_gap_models(&required_models);
    let codex_home = tempdir().expect("temp dir");
    let fetch_endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let fetch_manager =
        openai_manager_for_tests(codex_home.path().to_path_buf(), fetch_endpoint.clone());

    fetch_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    let cache_endpoint = TestModelsEndpoint::new(Vec::new());
    let cache_manager =
        openai_manager_for_tests(codex_home.path().to_path_buf(), cache_endpoint.clone());

    cache_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");

    assert_eq!(
        cache_endpoint.fetch_count(),
        0,
        "fresh cache should avoid a model fetch"
    );

    let models = cache_manager.get_remote_models().await;
    assert_models_contain(&models, &remote_models);
    assert_required_bundled_models_available_once(&models, &required_models);
}

#[tokio::test]
async fn refresh_available_models_preserves_bundled_catalog_for_empty_chatgpt_remote() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![Vec::new()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let expected = load_remote_models_from_file().expect("bundled models should parse");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
}

#[tokio::test]
async fn refresh_available_models_merges_hidden_only_chatgpt_remote_with_bundled_catalog() {
    let hidden_remote = remote_model_with_visibility(
        "chatgpt-hidden-only",
        "ChatGPT Hidden",
        /*priority*/ 0,
        "hide",
    );
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![hidden_remote.clone()]]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.push(hidden_remote);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
}

#[tokio::test]
async fn refresh_available_models_keeps_merging_for_api_auth() {
    let remote_models = vec![remote_model(
        "api-auth-visible-remote",
        "API Auth Visible",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = Arc::new(TestModelsEndpoint {
        has_provider_auth: true,
        uses_codex_backend: false,
        responses: Mutex::new(vec![remote_models.clone()].into()),
        fetch_count: AtomicUsize::new(0),
    });
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "test-api-key",
        ))),
    );
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_uses_provider_catalog_without_bundled_fallback() {
    let remote_models = vec![remote_model(
        "provider-visible-remote",
        "Provider Visible",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = Arc::new(TestModelsEndpoint {
        has_provider_auth: true,
        uses_codex_backend: false,
        responses: Mutex::new(vec![remote_models.clone()].into()),
        fetch_count: AtomicUsize::new(0),
    });
    let manager = openai_manager_for_tests_with_provider_key_and_catalog(
        codex_home.path().to_path_buf(),
        "openrouter".to_string(),
        BundledModelCatalog::Disabled,
        endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "provider-api-key",
        ))),
    );

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, remote_models);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_uses_cache_when_fresh() {
    let remote_models = vec![remote_model("cached", "Cached", /*priority*/ 5)];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("first refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &remote_models);

    // Second call should read from cache and avoid the network.
    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &remote_models);
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "cache hit should avoid a second model fetch"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_cache_stale() {
    let initial_models = vec![remote_model("stale", "Stale", /*priority*/ 1)];
    let codex_home = tempdir().expect("temp dir");
    let updated_models = vec![remote_model("fresh", "Fresh", /*priority*/ 9)];
    let endpoint = TestModelsEndpoint::new(vec![initial_models.clone(), updated_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    // Rewrite cache with an old timestamp so it is treated as stale.
    manager
        .cache_manager
        .manipulate_cache_for_test(|fetched_at| {
            *fetched_at = Utc::now() - chrono::Duration::hours(1);
        })
        .await
        .expect("cache manipulation succeeds");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("second refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &updated_models);
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "stale cache refresh should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_version_mismatch() {
    let initial_models = vec![remote_model("old", "Old", /*priority*/ 1)];
    let codex_home = tempdir().expect("temp dir");
    let updated_models = vec![remote_model("new", "New", /*priority*/ 2)];
    let endpoint = TestModelsEndpoint::new(vec![initial_models.clone(), updated_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    manager
        .cache_manager
        .mutate_cache_for_test(|cache| {
            let client_version = crate::client_version_to_whole();
            cache.client_version = Some(format!("{client_version}-mismatch"));
        })
        .await
        .expect("cache mutation succeeds");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("second refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &updated_models);
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "version mismatch should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_provider_cache_key_mismatches() {
    let initial_models = vec![remote_model("first-provider", "First", /*priority*/ 1)];
    let other_provider_models = vec![remote_model(
        "second-provider",
        "Second",
        /*priority*/ 2,
    )];
    let codex_home = tempdir().expect("temp dir");
    let first_endpoint = TestModelsEndpoint::new(vec![initial_models.clone()]);
    let first_manager = openai_manager_for_tests_with_provider_key(
        codex_home.path().to_path_buf(),
        "first-provider".to_string(),
        first_endpoint,
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
    );

    first_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    let second_endpoint = TestModelsEndpoint::new(vec![other_provider_models.clone()]);
    let second_manager = openai_manager_for_tests_with_provider_key(
        codex_home.path().to_path_buf(),
        "second-provider".to_string(),
        second_endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
    );
    second_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("provider switch refresh succeeds");

    assert_models_contain(
        &second_manager.get_remote_models().await,
        &other_provider_models,
    );
    assert_eq!(
        second_endpoint.fetch_count(),
        1,
        "provider mismatch should fetch instead of reusing another provider cache"
    );
}

#[tokio::test]
async fn refresh_available_models_drops_removed_remote_models() {
    let initial_models = vec![remote_model(
        "remote-old",
        "Remote Old",
        /*priority*/ 1,
    )];
    let codex_home = tempdir().expect("temp dir");
    let refreshed_models = vec![remote_model(
        "remote-new",
        "Remote New",
        /*priority*/ 1,
    )];
    let endpoint = TestModelsEndpoint::new(vec![initial_models, refreshed_models]);
    let mut manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());
    manager.cache_manager.set_ttl(Duration::ZERO);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("second refresh succeeds");

    let available = manager
        .try_list_models()
        .expect("models should be available");
    assert!(
        available.iter().any(|preset| preset.model == "remote-new"),
        "new remote model should be listed"
    );
    assert!(
        !available.iter().any(|preset| preset.model == "remote-old"),
        "removed remote model should not be listed"
    );
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "second refresh should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_skips_network_without_chatgpt_auth() {
    let dynamic_slug = "dynamic-model-only-for-test-noauth";
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::without_refresh(vec![vec![remote_model(
        dynamic_slug,
        "No Auth",
        /*priority*/ 1,
    )]]);
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint.clone(),
        /*auth_manager*/ None,
    );

    manager
        .refresh_available_models(RefreshStrategy::Online)
        .await
        .expect("refresh should no-op without chatgpt auth");
    let cached_remote = manager.get_remote_models().await;
    assert!(
        !cached_remote
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should be skipped without chatgpt auth"
    );
    assert_eq!(
        endpoint.fetch_count(),
        0,
        "endpoint that cannot refresh should avoid model fetches"
    );
}

#[derive(Debug)]
struct TestAuthAwareModelsEndpoint {
    auth_manager: Option<Arc<AuthManager>>,
    responses: Mutex<VecDeque<Vec<ModelInfo>>>,
    fetch_count: AtomicUsize,
}

impl TestAuthAwareModelsEndpoint {
    fn new(auth_manager: Option<Arc<AuthManager>>, responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            auth_manager,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelsEndpointClient for TestAuthAwareModelsEndpoint {
    fn has_provider_auth(&self) -> bool {
        false
    }

    async fn uses_codex_backend(&self) -> bool {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager
                .auth()
                .await
                .as_ref()
                .is_some_and(CodexAuth::uses_codex_backend),
            None => false,
        }
    }

    async fn list_models(
        &self,
        _client_version: &str,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let models = self
            .responses
            .lock()
            .expect("responses lock should not be poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok((models, None))
    }
}

#[tokio::test]
async fn refresh_available_models_skips_network_when_external_api_key_overrides_chatgpt_auth() {
    let dynamic_slug = "dynamic-model-only-for-test-external-api-key";
    let codex_home = tempdir().expect("temp dir");
    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    auth_manager.set_external_auth(Arc::new(TestExternalApiKeyAuth));
    let endpoint = TestAuthAwareModelsEndpoint::new(
        Some(Arc::clone(&auth_manager)),
        vec![vec![remote_model(
            dynamic_slug,
            "External API Key",
            /*priority*/ 1,
        )]],
    );
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint.clone(),
        Some(auth_manager),
    );

    manager
        .refresh_available_models(RefreshStrategy::Online)
        .await
        .expect("refresh should no-op with API key auth");
    let cached_remote = manager.get_remote_models().await;

    assert!(
        !cached_remote
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should be skipped when external API key auth is active"
    );
    assert_eq!(
        endpoint.fetch_count(),
        0,
        "endpoint should avoid model fetches when external API key auth is active"
    );
}

#[tokio::test]
async fn refresh_available_models_uses_cached_chatgpt_when_external_api_key_is_unresolved() {
    let dynamic_slug = "dynamic-model-only-for-test-unresolved-external-api-key";
    let codex_home = tempdir().expect("temp dir");
    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    auth_manager.set_external_auth(Arc::new(TestUnresolvedExternalApiKeyAuth));
    let endpoint = TestAuthAwareModelsEndpoint::new(
        Some(Arc::clone(&auth_manager)),
        vec![vec![remote_model(
            dynamic_slug,
            "Unresolved External API Key",
            /*priority*/ 1,
        )]],
    );
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint.clone(),
        Some(auth_manager),
    );

    manager
        .refresh_available_models(RefreshStrategy::Online)
        .await
        .expect("refresh should fall back to cached ChatGPT auth");

    assert!(
        manager
            .get_remote_models()
            .await
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should include models fetched with cached ChatGPT auth"
    );
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "endpoint should fetch models when unresolved external API key falls back to ChatGPT auth"
    );
}

#[tokio::test]
async fn refresh_available_models_fetches_with_chatgpt_auth_tokens() {
    let dynamic_slug = "dynamic-model-only-for-test-chatgpt-auth-tokens";
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![remote_model(
        dynamic_slug,
        "ChatGPT Auth Tokens",
        /*priority*/ 1,
    )]]);
    let auth = chatgpt_auth_tokens_for_tests(codex_home.path()).await;
    let manager = openai_manager_for_tests_with_auth(
        codex_home.path().to_path_buf(),
        endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(auth)),
    );

    manager
        .refresh_available_models(RefreshStrategy::Online)
        .await
        .expect("refresh should fetch with ChatGPT auth tokens");

    assert!(
        manager
            .get_remote_models()
            .await
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should include models fetched with ChatGPT auth tokens"
    );
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "endpoint should fetch models with ChatGPT auth tokens"
    );
}

#[test]
fn build_available_models_picks_default_after_hiding_hidden_models() {
    let manager = static_manager_for_tests(ModelsResponse { models: Vec::new() });

    let hidden_model =
        remote_model_with_visibility("hidden", "Hidden", /*priority*/ 0, "hide");
    let visible_model =
        remote_model_with_visibility("visible", "Visible", /*priority*/ 1, "list");

    let expected_hidden = ModelPreset::from(hidden_model.clone());
    let mut expected_visible = ModelPreset::from(visible_model.clone());
    expected_visible.is_default = true;

    let available = manager.build_available_models(vec![hidden_model, visible_model]);

    assert_eq!(available, vec![expected_hidden, expected_visible]);
}

#[tokio::test]
async fn static_manager_reads_latest_auth_mode() {
    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    let chatgpt_only_model = {
        let mut model = remote_model("chatgpt-only", "ChatGPT Only", /*priority*/ 0);
        model.supported_in_api = false;
        model
    };
    let api_model = remote_model("api-model", "API Model", /*priority*/ 1);
    let manager = StaticModelsManager::new(
        Some(Arc::clone(&auth_manager)),
        ModelsResponse {
            models: vec![chatgpt_only_model, api_model],
        },
    );

    let chatgpt_models = manager.list_models(RefreshStrategy::Online).await;
    assert_eq!(
        chatgpt_models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        vec!["chatgpt-only", "api-model"]
    );

    auth_manager.set_external_auth(Arc::new(TestExternalApiKeyAuth));
    let api_models = manager.list_models(RefreshStrategy::Online).await;

    assert_eq!(
        api_models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        vec!["api-model"]
    );
}

#[test]
fn bundled_models_json_roundtrips() {
    let response = crate::bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));

    let serialized =
        serde_json::to_string(&response).expect("bundled models.json should serialize");
    let roundtripped: ModelsResponse =
        serde_json::from_str(&serialized).expect("serialized models.json should deserialize");

    assert_eq!(
        response, roundtripped,
        "bundled models.json should round trip through serde"
    );
    assert!(
        !response.models.is_empty(),
        "bundled models.json should contain at least one model"
    );
    let spark = response
        .models
        .iter()
        .find(|model| model.slug == crate::model_info::GPT_5_3_CODEX_SPARK)
        .expect("bundled models should include Spark");
    assert_eq!(spark.input_modalities, vec![InputModality::Text]);
    assert_eq!(spark.default_reasoning_level, Some(ReasoningEffort::High));
    assert!(!spark.supported_in_api);
}
