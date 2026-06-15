use super::*;
use codex_model_provider::HostedWebSearchProvider;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use codex_tools::ResponsesApiWebSearchFilters;
use codex_tools::ResponsesApiWebSearchUserLocation;
use pretty_assertions::assert_eq;

#[test]
fn image_generation_tool_matches_expected_spec() {
    assert_eq!(
        create_image_generation_tool("png"),
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        }
    );
}

#[test]
fn web_search_tool_preserves_configured_options() {
    assert_eq!(
        create_web_search_tool(WebSearchToolOptions {
            web_search_mode: Some(WebSearchMode::Live),
            web_search_config: Some(&WebSearchConfig {
                filters: Some(WebSearchFilters {
                    allowed_domains: Some(vec!["example.com".to_string()]),
                }),
                user_location: Some(WebSearchUserLocation {
                    r#type: WebSearchUserLocationType::Approximate,
                    country: Some("US".to_string()),
                    region: None,
                    city: None,
                    timezone: Some("America/Los_Angeles".to_string()),
                }),
                search_context_size: Some(WebSearchContextSize::Low),
            }),
            web_search_tool_type: WebSearchToolType::TextAndImage,
            hosted_web_search_provider: HostedWebSearchProvider::OpenAiResponses,
        }),
        Some(ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec!["example.com".to_string()]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: WebSearchUserLocationType::Approximate,
                country: Some("US".to_string()),
                region: None,
                city: None,
                timezone: Some("America/Los_Angeles".to_string()),
            }),
            search_context_size: Some(WebSearchContextSize::Low),
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        })
    );
}

#[test]
fn web_search_tool_is_absent_when_disabled() {
    assert_eq!(
        create_web_search_tool(WebSearchToolOptions {
            web_search_mode: Some(WebSearchMode::Disabled),
            web_search_config: None,
            web_search_tool_type: WebSearchToolType::Text,
            hosted_web_search_provider: HostedWebSearchProvider::OpenAiResponses,
        }),
        None
    );
}

#[test]
fn provider_native_web_search_uses_provider_shape() {
    assert_eq!(
        create_web_search_tool(WebSearchToolOptions {
            web_search_mode: Some(WebSearchMode::Live),
            web_search_config: None,
            web_search_tool_type: WebSearchToolType::Text,
            hosted_web_search_provider: HostedWebSearchProvider::OpenRouter,
        }),
        Some(ToolSpec::OpenRouterWebSearch {})
    );

    assert_eq!(
        create_web_search_tool(WebSearchToolOptions {
            web_search_mode: Some(WebSearchMode::Live),
            web_search_config: None,
            web_search_tool_type: WebSearchToolType::Text,
            hosted_web_search_provider: HostedWebSearchProvider::Zai,
        }),
        Some(ToolSpec::ZaiWebSearch {
            web_search: ZaiWebSearchConfig {
                enable: true,
                search_engine: "search-prime".to_string(),
                search_result: true,
            },
        })
    );
}
