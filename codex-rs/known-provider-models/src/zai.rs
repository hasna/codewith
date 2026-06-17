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
        true,
    ),
    KnownProviderFallbackModel::new(
        "glm-5.1",
        "GLM-5.1",
        "Z.ai's latest GLM model with native web search support. Requires ZAI_API_KEY for turns.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "glm-5",
        "GLM-5",
        "Z.ai GLM-5 model with native web search support.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "glm-4.7",
        "GLM-4.7",
        "Z.ai GLM-4.7 model with native web search support.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "glm-5.2" => Some(model("GLM-5.2", 1_000_000)),
        "glm-5.2[1m]" => Some(model("GLM-5.2 1M", 1_000_000)),
        "glm-5.1" => Some(model("GLM-5.1", 1_000_000)),
        "glm-5" => Some(model("GLM-5", 1_000_000)),
        "glm-5-turbo" => Some(model("GLM-5 Turbo", 128_000)),
        "glm-4.7" => Some(model("GLM-4.7", 202_752)),
        "glm-4.7-flashx" => Some(model("GLM-4.7 FlashX", 202_752)),
        "glm-4.7-flash" => Some(model("GLM-4.7 Flash", 202_752)),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "glm-5.2" | "glm-5.2[1m]" | "glm-5.1" | "glm-5" | "glm-5-turbo" | "glm-4.7"
        | "glm-4.7-flashx" | "glm-4.7-flash" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::None, "Reasoning disabled"),
                reasoning_preset(ReasoningEffort::Medium, "Reasoning enabled"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}

const fn model(display_name: &'static str, context_window: i64) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ true,
    )
}
