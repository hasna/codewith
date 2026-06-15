use codex_model_provider::HostedWebSearchProvider;
use codex_protocol::config_types::WebSearchConfig;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::WebSearchToolType;
use codex_tools::ToolSpec;
use codex_tools::ZaiWebSearchConfig;

const WEB_SEARCH_TEXT_AND_IMAGE_CONTENT_TYPES: [&str; 2] = ["text", "image"];

pub struct WebSearchToolOptions<'a> {
    pub web_search_mode: Option<WebSearchMode>,
    pub web_search_config: Option<&'a WebSearchConfig>,
    pub web_search_tool_type: WebSearchToolType,
    pub hosted_web_search_provider: HostedWebSearchProvider,
}

pub fn create_image_generation_tool(output_format: &str) -> ToolSpec {
    ToolSpec::ImageGeneration {
        output_format: output_format.to_string(),
    }
}

pub fn create_web_search_tool(options: WebSearchToolOptions<'_>) -> Option<ToolSpec> {
    if !options.hosted_web_search_provider.is_enabled() {
        return None;
    }

    let external_web_access = match options.web_search_mode {
        Some(WebSearchMode::Cached) => Some(false),
        Some(WebSearchMode::Live) => Some(true),
        Some(WebSearchMode::Disabled) | None => None,
    }?;

    let search_content_types = match options.web_search_tool_type {
        WebSearchToolType::Text => None,
        WebSearchToolType::TextAndImage => Some(
            WEB_SEARCH_TEXT_AND_IMAGE_CONTENT_TYPES
                .into_iter()
                .map(str::to_string)
                .collect(),
        ),
    };

    match options.hosted_web_search_provider {
        HostedWebSearchProvider::Disabled => None,
        HostedWebSearchProvider::OpenAiResponses => Some(ToolSpec::WebSearch {
            external_web_access: Some(external_web_access),
            filters: options
                .web_search_config
                .and_then(|config| config.filters.clone().map(Into::into)),
            user_location: options
                .web_search_config
                .and_then(|config| config.user_location.clone().map(Into::into)),
            search_context_size: options
                .web_search_config
                .and_then(|config| config.search_context_size),
            search_content_types,
        }),
        HostedWebSearchProvider::Anthropic => Some(ToolSpec::AnthropicWebSearch {
            name: "web_search".to_string(),
            max_uses: None,
            allowed_domains: options
                .web_search_config
                .and_then(|config| config.filters.as_ref())
                .and_then(|filters| filters.allowed_domains.clone()),
        }),
        HostedWebSearchProvider::OpenRouter => Some(ToolSpec::OpenRouterWebSearch {}),
        HostedWebSearchProvider::Xai => Some(ToolSpec::XaiWebSearch {}),
        HostedWebSearchProvider::Xiaomi => Some(ToolSpec::XiaomiWebSearch {}),
        HostedWebSearchProvider::Qwen => Some(ToolSpec::QwenWebSearch {}),
        HostedWebSearchProvider::Zai => Some(ToolSpec::ZaiWebSearch {
            web_search: ZaiWebSearchConfig {
                enable: true,
                search_engine: "search-prime".to_string(),
                search_result: true,
            },
        }),
    }
}

#[cfg(test)]
#[path = "hosted_spec_tests.rs"]
mod tests;
