use super::*;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::AbsolutePathBufGuard;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn test_deserialize_ollama_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Ollama"
base_url = "http://localhost:11434/v1"
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Ollama".into(),
        base_url: Some("http://localhost:11434/v1".into()),
        env_key: None,
        env_key_instructions: None,
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
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_azure_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Azure"
base_url = "https://xxxxx.openai.azure.com/openai"
env_key = "AZURE_OPENAI_API_KEY"
query_params = { api-version = "2025-04-01-preview" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://xxxxx.openai.azure.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: Some(maplit::hashmap! {
            "api-version".to_string() => "2025-04-01-preview".to_string(),
        }),
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_example_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Example"
base_url = "https://example.com"
env_key = "API_KEY"
http_headers = { "X-Example-Header" = "example-value" }
env_http_headers = { "X-Example-Env-Header" = "EXAMPLE_ENV_VAR" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(maplit::hashmap! {
            "X-Example-Header".to_string() => "example-value".to_string(),
        }),
        env_http_headers: Some(maplit::hashmap! {
            "X-Example-Env-Header".to_string() => "EXAMPLE_ENV_VAR".to_string(),
        }),
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_chat_wire_api() {
    let provider_toml = r#"
name = "OpenAI using Chat Completions"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
wire_api = "chat"
        "#;

    let provider = toml::from_str::<ModelProviderInfo>(provider_toml).unwrap();
    assert_eq!(provider.wire_api, WireApi::Chat);
}

#[test]
fn test_deserialize_websocket_connect_timeout() {
    let provider_toml = r#"
name = "OpenAI"
base_url = "https://api.openai.com/v1"
websocket_connect_timeout_ms = 15000
supports_websockets = true
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.websocket_connect_timeout_ms, Some(15_000));
}

#[test]
fn test_supports_remote_compaction_for_openai() {
    let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);

    assert!(provider.supports_remote_compaction());
}

#[test]
fn test_personal_access_token_uses_chatgpt_codex_base_url() {
    let api_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        .to_api_provider(Some(AuthMode::PersonalAccessToken))
        .expect("OpenAI provider should build API provider");

    assert_eq!(api_provider.base_url, CHATGPT_CODEX_BASE_URL);
}

#[test]
fn test_supports_remote_compaction_for_azure_name() {
    let provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://example.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
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
    };

    assert!(provider.supports_remote_compaction());
}

#[test]
fn test_supports_remote_compaction_for_non_openai_non_azure_provider() {
    let provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com/v1".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
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
    };

    assert!(!provider.supports_remote_compaction());
}

#[test]
fn test_deserialize_provider_auth_config_defaults() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
args = ["--format=text"]
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    assert_eq!(
        provider.auth,
        Some(ModelProviderAuthInfo {
            command: "./scripts/print-token".to_string(),
            args: vec!["--format=text".to_string()],
            timeout_ms: NonZeroU64::new(5_000).unwrap(),
            refresh_interval_ms: 300_000,
            cwd: AbsolutePathBuf::resolve_path_against_base(".", base_dir.path()),
        })
    );
}

#[test]
fn test_deserialize_provider_aws_config() {
    let provider_toml = r#"
name = "Amazon Bedrock"
base_url = "https://bedrock.example.com/v1"

[aws]
profile = "codex-bedrock"
region = "us-west-2"
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();

    assert_eq!(
        provider.aws,
        Some(ModelProviderAwsAuthInfo {
            profile: Some("codex-bedrock".to_string()),
            region: Some("us-west-2".to_string()),
        })
    );
}

#[test]
fn test_create_amazon_bedrock_provider() {
    assert_eq!(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        ModelProviderInfo {
            name: "Amazon Bedrock".to_string(),
            base_url: Some("https://bedrock-mantle.us-east-1.api.aws/openai/v1".to_string()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            }),
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(maplit::hashmap! {
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER.to_string() =>
                    AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE.to_string(),
            }),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    );
}

#[test]
fn test_amazon_bedrock_provider_adds_mantle_client_agent_header() {
    let api_provider = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None)
        .to_api_provider(/*auth_mode*/ None)
        .expect("Amazon Bedrock provider should build API provider");

    assert_eq!(
        api_provider
            .headers
            .get(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE)
    );
}

#[test]
fn test_built_in_model_providers_include_expected_picker_providers() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        built_in_model_provider_ids()
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
    );
}

#[test]
fn test_built_in_model_providers_include_anthropic() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(ANTHROPIC_PROVIDER_ID),
        Some(&ModelProviderInfo::create_anthropic_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_cerebras() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(CEREBRAS_PROVIDER_ID),
        Some(&ModelProviderInfo::create_cerebras_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_nvidia() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(NVIDIA_PROVIDER_ID),
        Some(&ModelProviderInfo::create_nvidia_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_openrouter() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(OPENROUTER_PROVIDER_ID),
        Some(&ModelProviderInfo::create_openrouter_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_xai() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(XAI_PROVIDER_ID),
        Some(&ModelProviderInfo::create_xai_provider())
    );
}

#[test]
fn test_merge_configured_model_providers_allows_anthropic_override() {
    let anthropic_provider = ModelProviderInfo {
        name: "Anthropic Dedicated".to_string(),
        base_url: Some("https://dedicated.anthropic.example.com/v1".to_string()),
        env_key: Some("ANTHROPIC_DEDICATED_API_KEY".to_string()),
        wire_api: WireApi::Chat,
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([(ANTHROPIC_PROVIDER_ID.to_string(), anthropic_provider)]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_anthropic_provider();
    expected_provider.name = "Anthropic Dedicated".to_string();
    expected_provider.base_url = Some("https://dedicated.anthropic.example.com/v1".to_string());
    expected_provider.env_key = Some("ANTHROPIC_DEDICATED_API_KEY".to_string());
    expected_provider.env_key_instructions = None;
    expected.insert(ANTHROPIC_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_allows_cerebras_override() {
    let cerebras_provider = ModelProviderInfo {
        name: "Cerebras Dedicated".to_string(),
        base_url: Some("https://dedicated.cerebras.example.com/v1".to_string()),
        env_key: Some("CEREBRAS_DEDICATED_API_KEY".to_string()),
        wire_api: WireApi::Chat,
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([(CEREBRAS_PROVIDER_ID.to_string(), cerebras_provider)]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_cerebras_provider();
    expected_provider.name = "Cerebras Dedicated".to_string();
    expected_provider.base_url = Some("https://dedicated.cerebras.example.com/v1".to_string());
    expected_provider.env_key = Some("CEREBRAS_DEDICATED_API_KEY".to_string());
    expected_provider.env_key_instructions = None;
    expected.insert(CEREBRAS_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_built_in_model_providers_include_xiaomi() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(XIAOMI_PROVIDER_ID),
        Some(&ModelProviderInfo::create_xiaomi_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_deepseek() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(DEEPSEEK_PROVIDER_ID),
        Some(&ModelProviderInfo::create_deepseek_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_qwen() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(QWEN_PROVIDER_ID),
        Some(&ModelProviderInfo::create_qwen_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_google() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(GOOGLE_PROVIDER_ID),
        Some(&ModelProviderInfo::create_google_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_zai() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(ZAI_PROVIDER_ID),
        Some(&ModelProviderInfo::create_zai_provider())
    );
}

#[test]
fn test_built_in_model_providers_include_minimax() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers.get(MINIMAX_PROVIDER_ID),
        Some(&ModelProviderInfo::create_minimax_provider())
    );
}

#[test]
fn test_merge_configured_model_providers_allows_table_declared_builtin_overrides() {
    for provider_id in built_in_model_provider_ids()
        .filter(|provider_id| allows_partial_builtin_provider_override(provider_id))
    {
        let provider = ModelProviderInfo {
            name: format!("{provider_id} Dedicated"),
            base_url: Some(format!("https://dedicated.{provider_id}.example.com/v1")),
            env_key: Some(format!("{}_DEDICATED_API_KEY", provider_id.to_uppercase())),
            wire_api: default_wire_api_for_builtin_provider_override(provider_id),
            ..ModelProviderInfo::default()
        };
        let configured_model_providers =
            std::collections::HashMap::from([(provider_id.to_string(), provider.clone())]);

        let merged = merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        )
        .expect("table-declared built-in provider override should merge");
        let merged_provider = merged
            .get(provider_id)
            .expect("merged providers should include overridden built-in provider");

        assert_eq!(merged_provider.name, provider.name);
        assert_eq!(merged_provider.base_url, provider.base_url);
        assert_eq!(merged_provider.env_key, provider.env_key);
        assert_eq!(merged_provider.env_key_instructions, None);
        assert_eq!(
            merged_provider.wire_api,
            default_wire_api_for_builtin_provider_override(provider_id)
        );
    }
}

#[test]
fn test_merge_configured_model_providers_allows_xiaomi_override() {
    let xiaomi_provider = ModelProviderInfo {
        name: "Xiaomi MiMo Dedicated".to_string(),
        base_url: Some("https://dedicated.xiaomimimo.example.com/v1".to_string()),
        env_key: Some("MIMO_DEDICATED_API_KEY".to_string()),
        wire_api: WireApi::Chat,
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([(XIAOMI_PROVIDER_ID.to_string(), xiaomi_provider)]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_xiaomi_provider();
    expected_provider.name = "Xiaomi MiMo Dedicated".to_string();
    expected_provider.base_url = Some("https://dedicated.xiaomimimo.example.com/v1".to_string());
    expected_provider.env_key = Some("MIMO_DEDICATED_API_KEY".to_string());
    expected_provider.env_key_instructions = None;
    expected.insert(XIAOMI_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_allows_explicit_responses_wire_api_override() {
    let cerebras_provider = ModelProviderInfo {
        wire_api: WireApi::Responses,
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([(CEREBRAS_PROVIDER_ID.to_string(), cerebras_provider)]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_cerebras_provider();
    expected_provider.wire_api = WireApi::Responses;
    expected.insert(CEREBRAS_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_adds_custom_provider() {
    let custom_provider = ModelProviderInfo {
        name: "Custom".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([("custom".to_string(), custom_provider.clone())]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected.insert("custom".to_string(), custom_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_allows_openrouter_override() {
    let openrouter_provider = ModelProviderInfo {
        name: "OpenRouter Mirror".to_string(),
        base_url: Some("https://openrouter.example.com/api/v1".to_string()),
        env_key: Some("OPENROUTER_MIRROR_API_KEY".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers = std::collections::HashMap::from([(
        OPENROUTER_PROVIDER_ID.to_string(),
        openrouter_provider,
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_openrouter_provider();
    expected_provider.name = "OpenRouter Mirror".to_string();
    expected_provider.base_url = Some("https://openrouter.example.com/api/v1".to_string());
    expected_provider.env_key = Some("OPENROUTER_MIRROR_API_KEY".to_string());
    expected_provider.env_key_instructions = None;
    expected.insert(OPENROUTER_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_inherits_builtin_provider_auth_defaults() {
    let configured_model_providers = std::collections::HashMap::from([(
        OPENROUTER_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            base_url: Some("https://openrouter.example.com/api/v1".to_string()),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_openrouter_provider();
    expected_provider.base_url = Some("https://openrouter.example.com/api/v1".to_string());
    expected.insert(OPENROUTER_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_command_auth_override_clears_builtin_env_key() {
    let base_dir = tempdir().expect("tempdir should be created");
    let cwd = AbsolutePathBuf::resolve_path_against_base(".", base_dir.path());
    let auth = ModelProviderAuthInfo {
        command: "provider-token".to_string(),
        args: Vec::new(),
        cwd,
        timeout_ms: NonZeroU64::new(1_000).expect("timeout should be non-zero"),
        refresh_interval_ms: 60_000,
    };
    let configured_model_providers = std::collections::HashMap::from([(
        OPENROUTER_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            auth: Some(auth.clone()),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut expected_provider = ModelProviderInfo::create_openrouter_provider();
    expected_provider.env_key = None;
    expected_provider.env_key_instructions = None;
    expected_provider.auth = Some(auth);
    expected.insert(OPENROUTER_PROVIDER_ID.to_string(), expected_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_applies_amazon_bedrock_profile_override() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("codex-bedrock".to_string()),
                region: Some("us-west-2".to_string()),
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let mut bedrock_provider = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None);
    bedrock_provider.aws = Some(ModelProviderAwsAuthInfo {
        profile: Some("codex-bedrock".to_string()),
        region: Some("us-west-2".to_string()),
    });
    expected.insert(AMAZON_BEDROCK_PROVIDER_ID.to_string(), bedrock_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_rejects_amazon_bedrock_non_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            name: "Custom Bedrock".to_string(),
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("codex-bedrock".to_string()),
                region: None,
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Err(
            "model_providers.amazon-bedrock only supports changing `aws.profile` and `aws.region`; other non-default provider fields are not supported"
                .to_string()
        )
    );
}

#[test]
fn test_merge_configured_model_providers_allows_amazon_bedrock_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            }),
            wire_api: WireApi::Responses,
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected.insert(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
    );

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_validate_provider_aws_rejects_conflicting_auth() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
        }),
        env_key: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
        supports_websockets: false,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with env_key, requires_openai_auth".to_string())
    );
}

#[test]
fn test_validate_provider_aws_rejects_websockets() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
        }),
        requires_openai_auth: false,
        supports_websockets: true,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with supports_websockets".to_string())
    );
}

#[test]
fn test_deserialize_provider_auth_config_allows_zero_refresh_interval() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
refresh_interval_ms = 0
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    let auth = provider.auth.expect("auth config should deserialize");
    assert_eq!(auth.refresh_interval_ms, 0);
    assert_eq!(auth.refresh_interval(), None);
}

#[test]
fn model_gateway_helpers_map_direct_and_aggregator_providers() {
    assert_eq!(
        model_gateway_for_provider(OPENAI_PROVIDER_ID),
        HASNA_GATEWAY_ID
    );
    assert_eq!(
        model_gateway_for_provider(OPENROUTER_PROVIDER_ID),
        OPENROUTER_GATEWAY_ID
    );
    assert_eq!(
        model_gateway_family(HASNA_GATEWAY_ID),
        Some(ModelGatewayFamily::Direct)
    );
    assert_eq!(
        model_gateway_family(OPENROUTER_GATEWAY_ID),
        Some(ModelGatewayFamily::Aggregator)
    );
    assert!(provider_belongs_to_model_gateway(
        OPENAI_PROVIDER_ID,
        HASNA_GATEWAY_ID
    ));
    assert!(!provider_belongs_to_model_gateway(
        OPENAI_PROVIDER_ID,
        OPENROUTER_GATEWAY_ID
    ));
}

#[test]
fn provider_base_url_matches_ignores_case_and_trailing_slashes() {
    assert!(provider_base_url_matches(
        " https://OPENROUTER.ai/api/v1/ ",
        OPENROUTER_BASE_URL
    ));
    assert!(!provider_base_url_matches(
        "https://evil.example/https://openrouter.ai/api/v1",
        OPENROUTER_BASE_URL
    ));
}

#[test]
fn built_in_provider_env_keys_have_trusted_secret_backend_scope() {
    for provider in built_in_model_providers(/*openai_base_url*/ None).values() {
        let Some(env_key) = provider.env_key.as_deref() else {
            continue;
        };
        let Some(base_url) = provider.base_url.as_deref() else {
            panic!("built-in provider env keys must have a trusted base URL");
        };

        assert_eq!(
            trusted_secret_backend_base_url_for_env_key(env_key),
            Some(base_url)
        );
        assert_eq!(
            provider.secret_backend_fallback(env_key),
            provider_credentials::SecretBackendFallback::Enabled
        );
    }
}

#[test]
fn built_in_secret_backend_fallback_is_disabled_for_untrusted_base_url() {
    let provider = ModelProviderInfo {
        name: "OpenRouter Mirror".to_string(),
        base_url: Some("https://openrouter-mirror.example/api/v1".to_string()),
        env_key: Some("OPENROUTER_API_KEY".to_string()),
        ..ModelProviderInfo::default()
    };

    assert_eq!(
        provider.secret_backend_fallback("OPENROUTER_API_KEY"),
        provider_credentials::SecretBackendFallback::Disabled
    );
}

#[test]
fn built_in_secret_backend_fallback_uses_derived_secret_name_scope() {
    let provider = ModelProviderInfo {
        name: "OpenRouter Mirror".to_string(),
        base_url: Some("https://openrouter-mirror.example/api/v1".to_string()),
        env_key: Some("_OPENROUTER_API_KEY".to_string()),
        ..ModelProviderInfo::default()
    };

    assert_eq!(
        provider.secret_backend_fallback("_OPENROUTER_API_KEY"),
        provider_credentials::SecretBackendFallback::Disabled
    );
    assert_eq!(
        provider.secret_backend_fallback("OPENROUTER__API_KEY"),
        provider_credentials::SecretBackendFallback::Disabled
    );
    assert_eq!(
        provider.secret_backend_fallback("OPENROUTER_TOKEN"),
        provider_credentials::SecretBackendFallback::Disabled
    );
    assert_eq!(
        provider.secret_backend_fallback("ANTHROPIC_ACCESS_TOKEN"),
        provider_credentials::SecretBackendFallback::Disabled
    );
}

#[test]
fn custom_provider_secret_backend_fallback_remains_available_for_custom_env_key() {
    let provider = ModelProviderInfo {
        name: "Corp".to_string(),
        base_url: Some("https://models.corp.example/v1".to_string()),
        env_key: Some("CORP_MODELS_API_KEY".to_string()),
        ..ModelProviderInfo::default()
    };

    assert_eq!(
        provider.secret_backend_fallback("CORP_MODELS_API_KEY"),
        provider_credentials::SecretBackendFallback::Enabled
    );
}
