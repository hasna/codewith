use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "nvidia/nemotron-3-ultra-550b-a55b",
        "NVIDIA Nemotron 3 Ultra",
        "NVIDIA's Nemotron 3 Ultra model. Requires NVIDIA_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "openai/gpt-oss-120b",
        "OpenAI GPT OSS 120B",
        "NVIDIA hosted OpenAI gpt-oss model. Requires NVIDIA_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "z-ai/glm-5.1",
        "Z.ai GLM 5.1",
        "NVIDIA hosted Z.ai GLM 5.1 model. Requires NVIDIA_API_KEY for turns.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "nvidia/nemotron-3-ultra-550b-a55b" => Some(
            KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                "NVIDIA Nemotron 3 Ultra",
                /*context_window*/ 1_000_000,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
                super::TEXT_INPUT_MODALITIES,
            ),
        ),
        "openai/gpt-oss-120b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "openai/gpt-oss-20b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 20B",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "deepseek-ai/deepseek-v4-flash" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Flash",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        "deepseek-ai/deepseek-v4-pro" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Pro",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        "z-ai/glm-5.1" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 5.1",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        "nvidia/nemotron-3.5-content-safety" => Some(KnownProviderModelMetadata::new(
            "NVIDIA Nemotron 3.5 Content Safety",
            /*context_window*/ 128_000,
            /*supports_tools*/ false,
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
