use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "grok-4.3",
        "Grok 4.3",
        "xAI Grok chat model. Requires XAI_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "grok-4.5",
        "Grok 4.5",
        "xAI's most intelligent Grok model for code and chat. Requires XAI_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "grok-build-0.1",
        "Grok Build 0.1",
        "xAI coding model for agentic coding workflows.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "grok-4.3" => Some(model(
            "Grok 4.3", /*context_window*/ 1_000_000, /*supports_search_tool*/ true,
        )),
        "grok-4.5" => Some(model(
            "Grok 4.5", /*context_window*/ 500_000, /*supports_search_tool*/ true,
        )),
        "grok-build-0.1" => Some(model(
            "Grok Build 0.1",
            /*context_window*/ 256_000,
            /*supports_search_tool*/ false,
        )),
        _ => None,
    }
}

/// Reasoning-effort presets per the xAI docs.
///
/// `grok-4.5` accepts `reasoning_effort` with `low`, `medium`, or `high` and defaults to `high`;
/// reasoning cannot be disabled, so `none` is deliberately not offered. No other xAI chat model is
/// documented as accepting `reasoning_effort`, so they keep the empty (provider-default) list.
pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "grok-4.5" => (
            Some(ReasoningEffort::High),
            vec![
                reasoning_preset(ReasoningEffort::Low, "Fast, lighter reasoning"),
                reasoning_preset(ReasoningEffort::Medium, "Balanced reasoning"),
                reasoning_preset(ReasoningEffort::High, "Most thorough reasoning"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}

const fn model(
    display_name: &'static str,
    context_window: i64,
    supports_search_tool: bool,
) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        supports_search_tool,
    )
}
