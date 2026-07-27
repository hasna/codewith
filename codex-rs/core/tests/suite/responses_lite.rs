use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use codex_config::types::McpServerConfig;
use codex_config::types::McpServerTransportConfig;
use codex_core::config::Config;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::HostToolCapability;
use codex_features::Feature;
use codex_image_generation_extension::install_with_handle as install_image_generation_extension;
use codex_login::CodexAuth;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::InputModality;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_web_search_extension::install_with_handle as install_web_search_extension;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::stdio_server_bin;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_mcp_server;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

fn responses_extensions(auth: &CodexAuth) -> Arc<ExtensionRegistry<Config>> {
    let auth_manager = codex_core::test_support::auth_manager_from_auth(auth.clone());
    let mut extension_builder = ExtensionRegistryBuilder::<Config>::new();
    let web_search =
        install_web_search_extension(&mut extension_builder, Arc::clone(&auth_manager));
    assert!(
        extension_builder.assign_host_tool_capability(&web_search, HostToolCapability::WebSearch)
    );
    let image_generation = install_image_generation_extension(&mut extension_builder, auth_manager);
    assert!(
        extension_builder
            .assign_host_tool_capability(&image_generation, HostToolCapability::ImageGeneration,)
    );
    Arc::new(extension_builder.build())
}

fn configure_responses_tools(config: &mut Config) {
    config.model_provider_id = OPENAI_PROVIDER_ID.to_string();
    // Keep the fixture's dummy ChatGPT auth active even when a developer
    // environment exports an auth profile. Hosted tool gates depend on auth mode.
    config.selected_auth_profile = None;
    assert!(config.web_search_mode.set(WebSearchMode::Live).is_ok());
    assert!(
        config
            .features
            .disable(Feature::StandaloneWebSearch)
            .is_ok()
    );
    assert!(config.features.enable(Feature::ImageGeneration).is_ok());
    assert!(config.features.disable(Feature::ImageGenExt).is_ok());
}

fn enable_standalone_image_generation(config: &mut Config) {
    configure_responses_tools(config);
    assert!(config.features.enable(Feature::ImageGenExt).is_ok());
}

fn configure_image_capable_model(model_info: &mut codex_protocol::openai_models::ModelInfo) {
    model_info.input_modalities = vec![InputModality::Text, InputModality::Image];
}

fn has_hosted_tool(tools: &[Value], tool_type: &str) -> bool {
    tools
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some(tool_type))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_uses_standalone_web_search_and_hides_unavailable_image_generation()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let extensions = responses_extensions(&auth);

    let mut builder = test_codex()
        .with_auth(auth)
        .with_extensions(extensions)
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            configure_image_capable_model(model_info);
        })
        .with_config(configure_responses_tools);
    let test = builder.build(&server).await?;

    test.submit_turn("Use standalone tools").await?;

    let request = response_mock.single_request();
    assert_eq!(
        request.header(RESPONSES_LITE_HEADER).as_deref(),
        Some("true")
    );
    let body = request.body_json();
    let web_run = request
        .tool_by_name("web", "run")
        .context("Responses Lite should expose standalone web search")?;
    let canonical_web = serde_json::to_value(codex_tools::canonical_web_search_namespace())
        .context("canonical web namespace should serialize")?;
    assert_eq!(web_run, canonical_web["tools"][0]);
    assert!(request.tool_by_name("images", "imagegen").is_none());
    let tools = body["tools"]
        .as_array()
        .context("Responses request tools should be an array")?;
    assert!(!has_hosted_tool(tools, "web_search"));
    assert!(!has_hosted_tool(tools, "image_generation"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_exposes_enabled_standalone_image_generation() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-image-enabled"),
            responses::ev_completed("resp-image-enabled"),
        ]),
    )
    .await;
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let extensions = responses_extensions(&auth);
    let mut builder = test_codex()
        .with_auth(auth)
        .with_extensions(extensions)
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            configure_image_capable_model(model_info);
        })
        .with_config(enable_standalone_image_generation);
    let test = builder.build(&server).await?;

    test.submit_turn("Generate an image").await?;

    let request = response_mock.single_request();
    let imagegen = request
        .tool_by_name("images", "imagegen")
        .context("Responses Lite should expose enabled standalone image generation")?;
    let canonical_image = serde_json::to_value(codex_tools::ToolSpec::Namespace(
        codex_tools::canonical_image_generation_namespace(),
    ))
    .context("canonical image namespace should serialize")?;
    assert_eq!(imagegen, canonical_image["tools"][0]);
    let tools = request.body_json()["tools"]
        .as_array()
        .context("Responses request tools should be an array")?
        .clone();
    assert!(!has_hosted_tool(&tools, "image_generation"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_prefers_image_extension_over_non_prefixed_mcp_collision() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-image-collision"),
            responses::ev_completed("resp-image-collision"),
        ]),
    )
    .await;
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let extensions = responses_extensions(&auth);
    let rmcp_test_server_bin = stdio_server_bin()?;
    let mut builder = test_codex()
        .with_auth(auth)
        .with_extensions(extensions)
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            configure_image_capable_model(model_info);
        })
        .with_config(move |config| {
            enable_standalone_image_generation(config);
            assert!(
                config
                    .features
                    .enable(Feature::NonPrefixedMcpToolNames)
                    .is_ok()
            );
            let mut servers = config.mcp_servers.get().clone();
            servers.insert(
                "images".to_string(),
                McpServerConfig {
                    transport: McpServerTransportConfig::Stdio {
                        command: rmcp_test_server_bin,
                        args: Vec::new(),
                        env: Some(HashMap::from([(
                            "MCP_TEST_INCLUDE_IMAGEGEN_TOOL".to_string(),
                            "1".to_string(),
                        )])),
                        env_vars: Vec::new(),
                        cwd: None,
                    },
                    environment_id: "local".to_string(),
                    enabled: true,
                    required: false,
                    disabled_reason: None,
                    startup_timeout_sec: Some(Duration::from_secs(10)),
                    tool_timeout_sec: None,
                    default_tools_approval_mode: None,
                    enabled_tools: Some(vec!["imagegen".to_string()]),
                    disabled_tools: None,
                    scopes: None,
                    oauth: None,
                    oauth_resource: None,
                    supports_parallel_tool_calls: false,
                    tools: HashMap::new(),
                },
            );
            config
                .mcp_servers
                .set(servers)
                .expect("test mcp servers should accept any configuration");
        });
    let test = builder.build(&server).await?;
    wait_for_mcp_server(&test.codex, "images").await?;

    test.submit_turn("Use the trusted image generator").await?;

    let request = response_mock.single_request();
    let canonical_image = serde_json::to_value(codex_tools::ToolSpec::Namespace(
        codex_tools::canonical_image_generation_namespace(),
    ))
    .context("canonical image namespace should serialize")?;
    assert_eq!(
        request
            .body_json()
            .get("tools")
            .and_then(Value::as_array)
            .and_then(|tools| {
                tools
                    .iter()
                    .find(|tool| tool.get("name").and_then(Value::as_str) == Some("images"))
            }),
        Some(&canonical_image),
        "the trusted extension must own the colliding images.imagegen declaration"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_compact_request_uses_lite_transport_contract() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let compact_mock =
        responses::mount_compact_json_once(&server, serde_json::json!({ "output": [] })).await;

    let mut builder = test_codex().with_model_info_override("gpt-5.4", |model_info| {
        model_info.use_responses_lite = true;
        model_info.supports_parallel_tool_calls = true;
    });
    let test = builder.build(&server).await?;

    test.submit_turn("Compact this conversation").await?;
    test.codex.submit(Op::Compact).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    response_mock.single_request();
    let compact_request = compact_mock.single_request();
    assert_eq!(
        compact_request.header(RESPONSES_LITE_HEADER).as_deref(),
        Some("true")
    );
    let compact_body = compact_request.body_json();
    assert_eq!(
        compact_body
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("context"))
            .and_then(Value::as_str),
        Some("all_turns")
    );
    assert_eq!(
        compact_body.get("parallel_tool_calls"),
        Some(&Value::Bool(false))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_omits_hosted_tools_without_standalone_extensions() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            configure_image_capable_model(model_info);
        })
        .with_config(configure_responses_tools);
    let test = builder.build(&server).await?;

    test.submit_turn("Do not use hosted tools").await?;

    let body = response_mock.single_request().body_json();
    let tools = body["tools"]
        .as_array()
        .context("Responses request tools should be an array")?;
    assert!(!has_hosted_tool(tools, "web_search"));
    assert!(!has_hosted_tool(tools, "image_generation"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_lite_uses_hosted_tools_when_standalone_features_are_disabled() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let extensions = responses_extensions(&auth);
    let mut builder = test_codex()
        .with_auth(auth)
        .with_extensions(extensions)
        .with_model_info_override("gpt-5.4", configure_image_capable_model)
        .with_config(configure_responses_tools);
    let test = builder.build(&server).await?;

    test.submit_turn("Use hosted tools").await?;

    let request = response_mock.single_request();
    assert_eq!(request.header(RESPONSES_LITE_HEADER), None);
    assert!(request.tool_by_name("web", "run").is_none());
    assert!(request.tool_by_name("images", "imagegen").is_none());
    let body = request.body_json();
    let tools = body["tools"]
        .as_array()
        .context("Responses request tools should be an array")?;
    assert!(has_hosted_tool(tools, "web_search"));
    assert!(
        has_hosted_tool(tools, "image_generation"),
        "expected hosted image_generation for model {:?} in tools: {tools:?}",
        body.get("model")
    );

    Ok(())
}
