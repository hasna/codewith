use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] =
    &[KnownProviderFallbackModel::new(
        "grok-4.3",
        "Grok 4.3",
        "xAI Grok chat model. Requires XAI_API_KEY for turns.",
        true,
    )];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "grok-4.3" => Some(model("Grok 4.3", 1_000_000, true)),
        _ => None,
    }
}

const fn model(
    display_name: &'static str,
    context_window: i64,
    supports_search_tool: bool,
) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        supports_search_tool,
    )
}
