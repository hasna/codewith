use codex_tools::FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES;
use codex_tools::ToolSpec;

fn classify_tool_spec(spec: &ToolSpec) -> &'static str {
    match spec {
        ToolSpec::Function(_) => "function",
        ToolSpec::Namespace(_) => "namespace",
        ToolSpec::ToolSearch { .. } => "tool_search",
        ToolSpec::ImageGeneration { .. } => "image_generation",
        ToolSpec::WebSearch { .. } => "web_search",
        ToolSpec::AnthropicWebSearch { .. } => "anthropic_web_search",
        ToolSpec::OpenRouterWebSearch { .. } => "openrouter_web_search",
        ToolSpec::XaiWebSearch { .. } => "xai_web_search",
        ToolSpec::XiaomiWebSearch { .. } => "xiaomi_web_search",
        ToolSpec::QwenWebSearch { .. } => "qwen_web_search",
        ToolSpec::ZaiWebSearch { .. } => "zai_web_search",
        ToolSpec::Freeform(_) => "freeform",
    }
}

#[test]
fn legacy_public_tool_spec_and_reserved_namespace_allowlist_compile() {
    assert_eq!(FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES, &["web"]);
    let classify: fn(&ToolSpec) -> &'static str = classify_tool_spec;
    assert_eq!(
        classify(&ToolSpec::OpenRouterWebSearch {}),
        "openrouter_web_search"
    );
}
