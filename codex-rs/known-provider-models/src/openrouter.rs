use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "openai/gpt-oss-120b",
        "OpenAI GPT OSS 120B",
        "OpenRouter hosted OpenAI gpt-oss model. Requires OPENROUTER_API_KEY for turns.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "deepseek/deepseek-v4-flash",
        "DeepSeek V4 Flash",
        "OpenRouter hosted DeepSeek V4 Flash model. Requires OPENROUTER_API_KEY for turns.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "z-ai/glm-4.7",
        "Z.ai GLM 4.7",
        "OpenRouter hosted Z.ai GLM 4.7 model. Requires OPENROUTER_API_KEY for turns.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "z-ai/glm-5.1",
        "Z.ai GLM 5.1",
        "OpenRouter hosted Z.ai GLM 5.1 model. Requires OPENROUTER_API_KEY for turns.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "openai/gpt-oss-120b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "openai/gpt-oss-120b:free" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B (Free)",
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
        "openai/gpt-oss-20b:free" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 20B (Free)",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "openai/gpt-oss-safeguard-20b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS Safeguard 20B",
            /*context_window*/ 131_072,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "z-ai/glm-4.7" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7",
            /*context_window*/ 202_752,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "z-ai/glm-4.7-flash" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7 Flash",
            /*context_window*/ 202_752,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "z-ai/glm-5.1" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 5.1",
            /*context_window*/ 202_752,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ true,
            /*supports_reasoning*/ true,
        )),
        "deepseek/deepseek-v4-flash" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Flash",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        "deepseek/deepseek-v4-pro" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Pro",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        "nvidia/nemotron-3.5-content-safety:free" => Some(KnownProviderModelMetadata::new(
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
        "openai/gpt-oss-120b"
        | "openai/gpt-oss-120b:free"
        | "openai/gpt-oss-20b"
        | "openai/gpt-oss-20b:free"
        | "openai/gpt-oss-safeguard-20b" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::Low, "Minimal reasoning"),
                reasoning_preset(ReasoningEffort::Medium, "Moderate reasoning"),
                reasoning_preset(ReasoningEffort::High, "Extensive reasoning"),
            ],
        ),
        "z-ai/glm-4.7" | "z-ai/glm-4.7-flash" | "z-ai/glm-5.1" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::None, "Reasoning disabled"),
                reasoning_preset(ReasoningEffort::Medium, "Reasoning enabled"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}
