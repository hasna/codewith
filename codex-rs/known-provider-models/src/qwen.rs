use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "qwen3.5-flash",
        "Qwen3.5 Flash",
        "Alibaba Qwen's fast model with native Model Studio web search support.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "qwen3.5-plus",
        "Qwen3.5 Plus",
        "Alibaba Qwen's balanced model with native Model Studio web search support.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "qwen3-max",
        "Qwen3 Max",
        "Alibaba Qwen's highest-capability model with native Model Studio web search support.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "qwen3.7-plus",
        "Qwen3.7 Plus",
        "Alibaba Qwen3.7 balanced model with native Model Studio web search support.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "qwen3.7-max",
        "Qwen3.7 Max",
        "Alibaba Qwen3.7 flagship model with native Model Studio web search support.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "qwen3.5-flash" | "qwen3.5-flash-2026-02-23" => Some(model("Qwen3.5 Flash")),
        "qwen3.5-plus" | "qwen3.5-plus-2026-02-15" => Some(model("Qwen3.5 Plus")),
        "qwen3-max" | "qwen3-max-2026-01-23" => Some(model("Qwen3 Max")),
        "qwen3.7-plus" => Some(model("Qwen3.7 Plus")),
        "qwen3.7-max" => Some(model("Qwen3.7 Max")),
        "qwen3.6-flash" => Some(model("Qwen3.6 Flash")),
        _ => None,
    }
}

const fn model(display_name: &'static str) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool(
        display_name,
        1_000_000,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ true,
    )
}
