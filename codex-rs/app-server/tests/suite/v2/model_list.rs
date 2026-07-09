use std::time::Duration;

use anyhow::Error;
use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use app_test_support::write_models_cache;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelGatewayKind;
use codex_app_server_protocol::ModelGatewayListParams;
use codex_app_server_protocol::ModelGatewayListResponse;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ModelProviderAuthKind;
use codex_app_server_protocol::ModelProviderListParams;
use codex_app_server_protocol::ModelProviderListResponse;
use codex_app_server_protocol::ModelServiceTier;
use codex_app_server_protocol::ModelUpgradeInfo;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_app_server_protocol::RequestId;
use codex_config::types::AuthCredentialsStoreMode;
use codex_model_provider_info::HASNA_GATEWAY_ID;
use codex_model_provider_info::HASNA_GATEWAY_NAME;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::OPENROUTER_PROVIDER_ID;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use core_test_support::responses::mount_models_once;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const INTERNAL_ERROR_CODE: i64 = -32603;

fn model_from_preset(preset: &ModelPreset) -> Model {
    Model {
        id: preset.id.clone(),
        model: preset.model.clone(),
        model_provider: OPENAI_PROVIDER_ID.to_string(),
        model_gateway: HASNA_GATEWAY_ID.to_string(),
        model_gateway_name: HASNA_GATEWAY_NAME.to_string(),
        model_gateway_kind: ModelGatewayKind::Direct,
        upstream_provider: upstream_provider_from_model_id(&preset.model),
        upgrade: preset.upgrade.as_ref().map(|upgrade| upgrade.id.clone()),
        upgrade_info: preset.upgrade.as_ref().map(|upgrade| ModelUpgradeInfo {
            model: upgrade.id.clone(),
            upgrade_copy: upgrade.upgrade_copy.clone(),
            model_link: upgrade.model_link.clone(),
            migration_markdown: upgrade.migration_markdown.clone(),
        }),
        availability_nux: preset.availability_nux.clone().map(Into::into),
        display_name: preset.display_name.clone(),
        description: preset.description.clone(),
        hidden: !preset.show_in_picker,
        supported_reasoning_efforts: preset
            .supported_reasoning_efforts
            .iter()
            .map(|preset| ReasoningEffortOption {
                reasoning_effort: preset.effort.clone(),
                description: preset.description.clone(),
            })
            .collect(),
        default_reasoning_effort: preset.default_reasoning_effort.clone(),
        input_modalities: preset.input_modalities.clone(),
        supports_personality: preset.supports_personality,
        additional_speed_tiers: preset.additional_speed_tiers.clone(),
        service_tiers: preset
            .service_tiers
            .iter()
            .map(|service_tier| ModelServiceTier {
                id: service_tier.id.clone(),
                name: service_tier.name.clone(),
                description: service_tier.description.clone(),
            })
            .collect(),
        default_service_tier: preset.default_service_tier.clone(),
        is_default: preset.is_default,
    }
}

fn upstream_provider_from_model_id(model_id: &str) -> Option<String> {
    let (provider, _) = model_id.split_once('/')?;
    if provider.is_empty() {
        None
    } else {
        Some(provider.to_string())
    }
}

fn expected_visible_models() -> Vec<Model> {
    // Filter by supported_in_api to support testing with both ChatGPT and non-ChatGPT auth modes.
    let mut presets = ModelPreset::filter_by_auth(
        codex_core::test_support::all_model_presets().clone(),
        /*chatgpt_mode*/ false,
    );

    // Mirror `ModelsManager::build_available_models()` default selection after auth filtering.
    ModelPreset::mark_default_by_picker_visibility(&mut presets);

    presets
        .iter()
        .filter(|preset| preset.show_in_picker)
        .map(model_from_preset)
        .collect()
}

fn remote_model(slug: &str, display_name: &str, priority: i32) -> Result<ModelInfo> {
    Ok(serde_json::from_value(json!({
        "slug": slug,
        "display_name": display_name,
        "description": format!("{display_name} provider model"),
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "low", "description": "low"},
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
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
    }))?)
}

#[tokio::test]
async fn list_models_returns_all_models_with_large_limit() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: None,
            model_provider: None,
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    let expected_models = expected_visible_models();

    assert_eq!(items, expected_models);
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_includes_hidden_models() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: Some(true),
            model_provider: None,
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    assert!(items.iter().any(|item| item.hidden));
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_uses_chatgpt_remote_catalog_as_source_of_truth() -> Result<()> {
    let server = MockServer::start().await;
    let remote_model: ModelInfo = serde_json::from_value(json!({
        "slug": "chatgpt-remote-only",
        "display_name": "ChatGPT Remote Only",
        "description": "Remote-only model for app-server model/list coverage",
        "default_reasoning_level": "max",
        "supported_reasoning_levels": [
            {"effort": "max", "description": "Maximum"},
            {"effort": "low", "description": "Low"},
            {"effort": "focused", "description": "Focused"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "minimal_client_version": [0, 1, 0],
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
    }))?;
    let models_mock = mount_models_once(
        &server,
        ModelsResponse {
            models: vec![remote_model.clone()],
        },
    )
    .await;

    let codex_home = TempDir::new()?;
    let server_uri = server.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"
openai_base_url = "{server_uri}/v1"
"#
        ),
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-access-token").plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp =
        TestAppServer::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: None,
            model_provider: None,
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;
    let mut expected_presets: Vec<ModelPreset> = vec![remote_model.into()];
    ModelPreset::mark_default_by_picker_visibility(&mut expected_presets);
    let mut expected_items = expected_presets
        .iter()
        .map(model_from_preset)
        .collect::<Vec<_>>();
    expected_items[0].supported_reasoning_efforts = vec![
        ReasoningEffortOption {
            reasoning_effort: "max".parse().map_err(Error::msg)?,
            description: "Maximum".to_string(),
        },
        ReasoningEffortOption {
            reasoning_effort: "low".parse().map_err(Error::msg)?,
            description: "Low".to_string(),
        },
        ReasoningEffortOption {
            reasoning_effort: "focused".parse().map_err(Error::msg)?,
            description: "Focused".to_string(),
        },
    ];

    assert_eq!(
        items.first(),
        expected_items.first(),
        "remote catalog model should remain the first returned model"
    );
    assert!(next_cursor.is_none());
    assert_eq!(
        models_mock.requests().len(),
        1,
        "expected a single /models request"
    );
    Ok(())
}

#[tokio::test]
async fn list_model_providers_returns_safe_provider_summaries() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"
model_provider = "corp"

[model_providers.corp]
name = "Corp Provider"
base_url = "https://corp.example.com/v1"
env_key = "CORP_PROVIDER_TOKEN"
wire_api = "responses"
"#,
    )?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_model_provider_list_request(ModelProviderListParams {
            model_gateway: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelProviderListResponse { data } = to_response::<ModelProviderListResponse>(response)?;
    let current = data
        .iter()
        .find(|provider| provider.id == "corp")
        .expect("custom provider should be listed");
    assert_eq!(current.name, "Corp Provider");
    assert_eq!(current.model_gateway, HASNA_GATEWAY_ID);
    assert_eq!(current.model_gateway_name, HASNA_GATEWAY_NAME);
    assert_eq!(current.model_gateway_kind, ModelGatewayKind::Direct);
    assert_eq!(current.auth_kind, ModelProviderAuthKind::Environment);
    assert!(current.is_current);
    let openrouter = data
        .iter()
        .find(|provider| provider.id == OPENROUTER_PROVIDER_ID)
        .expect("OpenRouter provider should be listed");
    assert_eq!(openrouter.model_gateway, OPENROUTER_PROVIDER_ID);
    assert_eq!(openrouter.model_gateway_kind, ModelGatewayKind::Aggregator);

    let serialized = serde_json::to_value(&data)?;
    assert!(!serialized.to_string().contains("CORP_PROVIDER_TOKEN"));
    assert!(!serialized.to_string().contains("corp.example.com"));
    Ok(())
}

#[tokio::test]
async fn list_model_gateways_returns_hasna_and_openrouter() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_model_gateway_list_request(ModelGatewayListParams {})
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelGatewayListResponse { data } = to_response::<ModelGatewayListResponse>(response)?;
    assert_eq!(
        data.iter()
            .map(|gateway| (gateway.id.as_str(), gateway.name.as_str(), gateway.kind))
            .collect::<Vec<_>>(),
        vec![
            (
                HASNA_GATEWAY_ID,
                HASNA_GATEWAY_NAME,
                ModelGatewayKind::Direct
            ),
            (
                OPENROUTER_PROVIDER_ID,
                "OpenRouter",
                ModelGatewayKind::Aggregator
            ),
        ]
    );
    assert!(
        data.iter()
            .find(|gateway| gateway.id == HASNA_GATEWAY_ID)
            .is_some_and(|gateway| gateway.is_current)
    );
    Ok(())
}

#[tokio::test]
async fn list_models_can_target_a_specific_provider() -> Result<()> {
    let provider_a = MockServer::start().await;
    let provider_b = MockServer::start().await;
    let provider_a_model = remote_model("provider-a-model", "Provider A", /*priority*/ 0)?;
    let provider_b_model = remote_model("provider-b-model", "Provider B", /*priority*/ 0)?;
    let provider_a_mock = mount_models_once(
        &provider_a,
        ModelsResponse {
            models: vec![provider_a_model],
        },
    )
    .await;
    let provider_b_mock = mount_models_once(
        &provider_b,
        ModelsResponse {
            models: vec![provider_b_model],
        },
    )
    .await;

    let codex_home = TempDir::new()?;
    let provider_a_uri = provider_a.uri();
    let provider_b_uri = provider_b.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
model_provider = "openai"

[model_providers.provider-a]
name = "Provider A"
base_url = "{provider_a_uri}/v1"
env_key = "PROVIDER_A_KEY"
wire_api = "responses"

[model_providers.provider-b]
name = "Provider B"
base_url = "{provider_b_uri}/v1"
env_key = "PROVIDER_B_KEY"
wire_api = "responses"
"#,
        ),
    )?;
    let mut mcp = TestAppServer::new_with_env(
        codex_home.path(),
        &[
            ("PROVIDER_A_KEY", Some("provider-a-key")),
            ("PROVIDER_B_KEY", Some("provider-b-key")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: None,
            model_provider: Some("provider-b".to_string()),
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
    assert_eq!(
        data.iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["provider-b-model"]
    );
    assert!(next_cursor.is_none());
    assert_eq!(provider_a_mock.requests().len(), 0);
    assert_eq!(provider_b_mock.requests().len(), 1);
    Ok(())
}

#[tokio::test]
async fn list_models_falls_back_for_cerebras_provider_discovery_failure() -> Result<()> {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(500).set_body_string("catalog down"))
        .mount(&provider)
        .await;

    let codex_home = TempDir::new()?;
    let provider_uri = provider.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
[model_providers.cerebras]
name = "Cerebras"
base_url = "{provider_uri}/v1"
experimental_bearer_token = "test-token"
wire_api = "responses"
"#
        ),
    )?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: Some("cerebras".to_string()),
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
    assert_eq!(
        data.iter()
            .map(|model| (model.id.as_str(), model.is_default))
            .collect::<Vec<_>>(),
        vec![("gpt-oss-120b", true), ("zai-glm-4.7", false)]
    );
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_falls_back_for_groq_provider_discovery_failure() -> Result<()> {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(500).set_body_string("catalog down"))
        .mount(&provider)
        .await;

    let codex_home = TempDir::new()?;
    let provider_uri = provider.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
[model_providers.groq]
name = "Groq"
base_url = "{provider_uri}/v1"
auth = {{ command = "printf", args = ["model-list-fixture"] }}
wire_api = "chat"
"#
        ),
    )?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: Some("groq".to_string()),
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
    assert_eq!(
        data.iter()
            .map(|model| (model.id.as_str(), model.is_default))
            .collect::<Vec<_>>(),
        vec![
            ("openai/gpt-oss-120b", true),
            ("openai/gpt-oss-20b", false),
            ("llama-3.3-70b-versatile", false),
            ("llama-3.1-8b-instant", false),
        ]
    );
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_falls_back_for_every_known_provider_discovery_failure() -> Result<()> {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(500).set_body_string("catalog down"))
        .mount(&provider)
        .await;

    let provider_uri = provider.uri();
    let cases = [
        ("anthropic", "claude-fable-5"),
        ("cerebras", "gpt-oss-120b"),
        ("deepseek", "deepseek-v4-flash"),
        ("google", "gemini-3.5-flash"),
        ("groq", "openai/gpt-oss-120b"),
        ("minimax", "MiniMax-M3"),
        ("nvidia", "nvidia/nemotron-3-ultra-550b-a55b"),
        ("openrouter", "z-ai/glm-5.2"),
        ("qwen", "qwen3.5-flash"),
        ("xai", "grok-4.3"),
        ("xiaomi", "mimo-v2.5-pro"),
        ("zai", "glm-5.2"),
    ];
    let mut config = String::new();
    for (provider_id, _) in cases {
        config.push_str(&format!(
            r#"
[model_providers.{provider_id}]
name = "{provider_id}"
base_url = "{provider_uri}/v1"
experimental_bearer_token = "test-token-{provider_id}"
wire_api = "responses"
"#
        ));
    }

    let codex_home = TempDir::new()?;
    std::fs::write(codex_home.path().join("config.toml"), config)?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    for (provider_id, expected_default) in cases {
        let request_id = mcp
            .send_list_models_request(ModelListParams {
                limit: None,
                cursor: None,
                include_hidden: None,
                model_provider: Some(provider_id.to_string()),
                model_gateway: None,
                upstream_provider: None,
            })
            .await?;
        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;

        let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
        assert_eq!(
            data.first().map(|model| model.id.as_str()),
            Some(expected_default),
            "{provider_id} should fall back to its static default model"
        );
        assert!(data.iter().any(|model| model.is_default));
        assert!(next_cursor.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn list_models_can_target_openrouter_gateway() -> Result<()> {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(500).set_body_string("catalog down"))
        .mount(&provider)
        .await;

    let codex_home = TempDir::new()?;
    let provider_uri = provider.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
[model_providers.openrouter]
base_url = "{provider_uri}/v1"
experimental_bearer_token = "test-token"
"#
        ),
    )?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: None,
            model_gateway: Some(OPENROUTER_PROVIDER_ID.to_string()),
            upstream_provider: Some("z-ai".to_string()),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
    assert_eq!(
        data.iter()
            .map(|model| {
                (
                    model.model.as_str(),
                    model.model_provider.as_str(),
                    model.model_gateway.as_str(),
                    model.model_gateway_kind,
                    model.upstream_provider.as_deref(),
                    model.default_reasoning_effort.clone(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "z-ai/glm-5.2",
                OPENROUTER_PROVIDER_ID,
                OPENROUTER_PROVIDER_ID,
                ModelGatewayKind::Aggregator,
                Some("z-ai"),
                ReasoningEffort::High,
            ),
            (
                "z-ai/glm-4.7",
                OPENROUTER_PROVIDER_ID,
                OPENROUTER_PROVIDER_ID,
                ModelGatewayKind::Aggregator,
                Some("z-ai"),
                ReasoningEffort::Medium,
            ),
            (
                "z-ai/glm-5.1",
                OPENROUTER_PROVIDER_ID,
                OPENROUTER_PROVIDER_ID,
                ModelGatewayKind::Aggregator,
                Some("z-ai"),
                ReasoningEffort::Medium,
            ),
        ]
    );
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_does_not_fall_back_for_cerebras_auth_failure() -> Result<()> {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "detail": "Not authenticated"
        })))
        .mount(&provider)
        .await;

    let codex_home = TempDir::new()?;
    let provider_uri = provider.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
[model_providers.cerebras]
name = "Cerebras"
base_url = "{provider_uri}/v1"
env_key = "CODEWITH_TEST_MODEL_LIST_MISSING_KEY"
wire_api = "responses"
"#
        ),
    )?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: Some("cerebras".to_string()),
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("failed to list models for provider `cerebras`"),
        "unexpected error message: {}",
        error.error.message
    );
    Ok(())
}

#[tokio::test]
async fn list_models_for_custom_provider_discovery_failure_returns_cached_result() -> Result<()> {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&provider)
        .await;

    let codex_home = TempDir::new()?;
    let provider_uri = provider.uri();
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
[model_providers.provider-b]
name = "Provider B"
base_url = "{provider_uri}/v1"
wire_api = "responses"
"#
        ),
    )?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: Some("provider-b".to_string()),
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
    assert!(data.is_empty());
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_rejects_unknown_provider() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: Some("missing-provider".to_string()),
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "model provider not found: missing-provider"
    );
    Ok(())
}

#[tokio::test]
async fn list_models_rejects_unknown_gateway() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
            include_hidden: None,
            model_provider: None,
            model_gateway: Some("missing-gateway".to_string()),
            upstream_provider: None,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "model gateway not found: missing-gateway"
    );
    Ok(())
}

#[tokio::test]
async fn list_models_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let expected_models = expected_visible_models();
    let mut cursor = None;
    let mut items = Vec::new();

    for _ in 0..expected_models.len() {
        let request_id = mcp
            .send_list_models_request(ModelListParams {
                limit: Some(1),
                cursor: cursor.clone(),
                include_hidden: None,
                model_provider: None,
                model_gateway: None,
                upstream_provider: None,
            })
            .await?;

        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;

        let ModelListResponse {
            data: page_items,
            next_cursor,
        } = to_response::<ModelListResponse>(response)?;

        assert_eq!(page_items.len(), 1);
        items.extend(page_items);

        if let Some(next_cursor) = next_cursor {
            cursor = Some(next_cursor);
        } else {
            assert_eq!(items, expected_models);
            return Ok(());
        }
    }

    panic!(
        "model pagination did not terminate after {} pages",
        expected_models.len()
    );
}

#[tokio::test]
async fn list_models_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: Some("invalid".to_string()),
            include_hidden: None,
            model_provider: None,
            model_gateway: None,
            upstream_provider: None,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, "invalid cursor: invalid");
    Ok(())
}
