use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::TEXT_INPUT_MODALITIES;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "openai/gpt-oss-120b",
        "OpenAI GPT OSS 120B",
        "Groq hosted OpenAI gpt-oss 120B model. Requires GROQ_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "openai/gpt-oss-20b",
        "OpenAI GPT OSS 20B",
        "Groq hosted OpenAI gpt-oss 20B model. Requires GROQ_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "llama-3.3-70b-versatile",
        "Llama 3.3 70B Versatile",
        "Groq hosted Llama 3.3 70B Versatile model. Requires GROQ_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "llama-3.1-8b-instant",
        "Llama 3.1 8B Instant",
        "Groq hosted Llama 3.1 8B Instant model. Requires GROQ_API_KEY for turns.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "openai/gpt-oss-120b" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "OpenAI GPT OSS 120B",
                /*context_window*/ 131_072,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
                TEXT_INPUT_MODALITIES,
            ),
        ),
        "openai/gpt-oss-20b" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "OpenAI GPT OSS 20B",
                /*context_window*/ 131_072,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
                TEXT_INPUT_MODALITIES,
            ),
        ),
        "llama-3.3-70b-versatile" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "Llama 3.3 70B Versatile",
                /*context_window*/ 131_072,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ false,
                /*supports_search_tool*/ false,
                TEXT_INPUT_MODALITIES,
            ),
        ),
        "llama-3.1-8b-instant" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "Llama 3.1 8B Instant",
                /*context_window*/ 131_072,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ false,
                /*supports_search_tool*/ false,
                TEXT_INPUT_MODALITIES,
            ),
        ),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "openai/gpt-oss-120b" | "openai/gpt-oss-20b" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::Low, "Minimal reasoning"),
                reasoning_preset(ReasoningEffort::Medium, "Moderate reasoning"),
                reasoning_preset(ReasoningEffort::High, "Extensive reasoning"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}
