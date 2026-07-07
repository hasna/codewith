use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "glm-5.2",
        "GLM-5.2",
        "Z.ai's latest flagship coding model with native web search support. Requires ZAI_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "glm-5.1",
        "GLM-5.1",
        "Z.ai's latest GLM model with native web search support. Requires ZAI_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "glm-5",
        "GLM-5",
        "Z.ai GLM-5 model with native web search support.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "glm-4.7",
        "GLM-4.7",
        "Z.ai GLM-4.7 model with native web search support.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "glm-5.2" => Some(model("GLM-5.2", /*context_window*/ 1_000_000)),
        "glm-5.2[1m]" => Some(model("GLM-5.2 1M", /*context_window*/ 1_000_000)),
        "glm-5.1" => Some(model("GLM-5.1", /*context_window*/ 1_000_000)),
        "glm-5" => Some(model("GLM-5", /*context_window*/ 1_000_000)),
        "glm-5-turbo" => Some(model("GLM-5 Turbo", /*context_window*/ 128_000)),
        "glm-4.7" => Some(model("GLM-4.7", /*context_window*/ 202_752)),
        "glm-4.7-flashx" => Some(model("GLM-4.7 FlashX", /*context_window*/ 202_752)),
        "glm-4.7-flash" => Some(model("GLM-4.7 Flash", /*context_window*/ 202_752)),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        // Direct Z.ai docs (https://docs.z.ai/guides/llm/glm-5.2 and
        // https://docs.z.ai/api-reference/llm/chat-completion, accessed 2026-07-09) show that only
        // GLM-5.2 exposes the granular `reasoning_effort` scale (max, xhigh, high, medium, low,
        // minimal, none) with a default of `max`. Other GLM models use Z.ai's provider-specific
        // `thinking` toggle, so Codewith should not expose those as OpenAI-compatible
        // `reasoning_effort` options.
        //
        // Codewith's `ReasoningEffort` enum has no first-class `max` variant, so `max` is
        // represented via `Custom("max")`, which serializes to the exact wire value `max`. If a
        // first-class `Max` variant is ever needed, that is a separate protocol follow-up.
        "glm-5.2" | "glm-5.2[1m]" => (
            Some(ReasoningEffort::Custom("max".to_string())),
            vec![
                reasoning_preset(ReasoningEffort::None, "Reasoning disabled"),
                reasoning_preset(ReasoningEffort::Minimal, "Minimal reasoning"),
                reasoning_preset(ReasoningEffort::Low, "Low reasoning"),
                reasoning_preset(ReasoningEffort::Medium, "Moderate reasoning"),
                reasoning_preset(ReasoningEffort::High, "High reasoning"),
                reasoning_preset(ReasoningEffort::XHigh, "Extended reasoning"),
                reasoning_preset(
                    ReasoningEffort::Custom("max".to_string()),
                    "Maximum reasoning",
                ),
            ],
        ),
        _ => (None, Vec::new()),
    }
}

const fn model(display_name: &'static str, context_window: i64) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool_and_input_modalities(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ true,
        super::TEXT_INPUT_MODALITIES,
    )
}
