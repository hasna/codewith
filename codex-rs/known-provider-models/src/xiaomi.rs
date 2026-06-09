use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] =
    &[KnownProviderFallbackModel::new(
        "mimo-v2.5-pro-ultraspeed",
        "MiMo V2.5 Pro UltraSpeed",
        "Xiaomi MiMo 1T ultra-fast model. Requires MIMO_API_KEY for turns.",
        /*is_default*/ true,
    )];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "mimo-v2.5-pro-ultraspeed" => Some(KnownProviderModelMetadata::new(
            "MiMo V2.5 Pro UltraSpeed",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    _slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    (None, Vec::new())
}
