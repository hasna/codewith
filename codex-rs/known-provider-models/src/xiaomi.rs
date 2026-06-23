use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "mimo-v2.5-pro",
        "MiMo V2.5 Pro",
        "Xiaomi MiMo 1T agentic model. Requires MIMO_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "mimo-v2.5",
        "MiMo V2.5",
        "Xiaomi MiMo full-modal V2.5 model with 1M context. Requires MIMO_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "mimo-v2.5-pro-ultraspeed",
        "MiMo V2.5 Pro UltraSpeed",
        "Legacy Xiaomi MiMo V2.5 Pro UltraSpeed alias. Requires MIMO_API_KEY for turns.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "mimo-v2.5-pro" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "MiMo V2.5 Pro",
                /*context_window*/ 1_048_576,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
                super::TEXT_INPUT_MODALITIES,
            ),
        ),
        "mimo-v2.5" => Some(KnownProviderModelMetadata::new(
            "MiMo V2.5",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "mimo-v2.5-pro-ultraspeed" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "MiMo V2.5 Pro UltraSpeed",
                /*context_window*/ 1_048_576,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
                super::TEXT_INPUT_MODALITIES,
            ),
        ),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    _slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    (None, Vec::new())
}
