use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "gpt-oss-120b",
        "OpenAI GPT OSS 120B",
        "Cerebras hosted OpenAI gpt-oss model. Requires CEREBRAS_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "zai-glm-4.7",
        "Z.ai GLM 4.7",
        "Cerebras hosted Z.ai GLM 4.7 model. Requires CEREBRAS_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "gemma-4-31b",
        "Gemma 4 31B",
        "Cerebras hosted Gemma 4 31B preview model. Requires CEREBRAS_API_KEY for turns.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "gpt-oss-120b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "zai-glm-4.7" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ true,
            /*supports_reasoning*/ true,
        )),
        "gemma-4-31b" => Some(KnownProviderModelMetadata::new(
            "Gemma 4 31B",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "gpt-oss-120b" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::Low, "Minimal reasoning"),
                reasoning_preset(ReasoningEffort::Medium, "Moderate reasoning"),
                reasoning_preset(ReasoningEffort::High, "Extensive reasoning"),
            ],
        ),
        "zai-glm-4.7" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::None, "Reasoning disabled"),
                reasoning_preset(ReasoningEffort::Medium, "Reasoning enabled"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}
