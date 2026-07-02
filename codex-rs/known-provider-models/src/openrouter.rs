use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "z-ai/glm-5.2",
        "Z.ai GLM 5.2",
        "OpenRouter hosted Z.ai GLM 5.2 model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "openai/gpt-oss-120b",
        "OpenAI GPT OSS 120B",
        "OpenRouter hosted OpenAI gpt-oss model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "deepseek/deepseek-v4-flash",
        "DeepSeek V4 Flash",
        "OpenRouter hosted DeepSeek V4 Flash model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "z-ai/glm-4.7",
        "Z.ai GLM 4.7",
        "OpenRouter hosted Z.ai GLM 4.7 model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "z-ai/glm-5.1",
        "Z.ai GLM 5.1",
        "OpenRouter hosted Z.ai GLM 5.1 model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "qwen/qwen3.7-plus",
        "Qwen3.7 Plus",
        "OpenRouter hosted Qwen3.7 Plus model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "x-ai/grok-4.20",
        "Grok 4.20",
        "OpenRouter hosted xAI Grok 4.20 model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "xiaomi/mimo-v2.5-pro",
        "MiMo V2.5 Pro",
        "OpenRouter hosted Xiaomi MiMo V2.5 Pro model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "nvidia/nemotron-3-ultra-550b-a55b",
        "NVIDIA Nemotron 3 Ultra",
        "OpenRouter hosted NVIDIA Nemotron 3 Ultra model. Requires OPENROUTER_API_KEY for turns.",
        /*is_default*/ false,
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
        "z-ai/glm-5.2" | "z-ai/glm-5.2-20260616" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 5.2",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ true,
            /*supports_reasoning*/ true,
        )),
        "deepseek/deepseek-v4-flash" => Some(model(
            "DeepSeek V4 Flash",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_reasoning*/ false,
        )),
        "deepseek/deepseek-v4-pro" => Some(model(
            "DeepSeek V4 Pro",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_reasoning*/ false,
        )),
        "nvidia/nemotron-3.5-content-safety:free" => Some(KnownProviderModelMetadata::new(
            "NVIDIA Nemotron 3.5 Content Safety",
            /*context_window*/ 128_000,
            /*supports_tools*/ false,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ false,
        )),
        "nvidia/nemotron-3-ultra-550b-a55b" => Some(model(
            "NVIDIA Nemotron 3 Ultra",
            /*context_window*/ 1_000_000,
            /*supports_tools*/ true,
            /*supports_reasoning*/ true,
        )),
        "nvidia/nemotron-3-ultra-550b-a55b:free" => Some(model(
            "NVIDIA Nemotron 3 Ultra (Free)",
            /*context_window*/ 1_000_000,
            /*supports_tools*/ true,
            /*supports_reasoning*/ true,
        )),
        "qwen/qwen3.7-plus" => Some(KnownProviderModelMetadata::with_search_tool(
            "Qwen3.7 Plus",
            /*context_window*/ 1_000_000,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
            /*supports_search_tool*/ false,
        )),
        "qwen/qwen3.7-max" => Some(model(
            "Qwen3.7 Max",
            /*context_window*/ 1_000_000,
            /*supports_tools*/ true,
            /*supports_reasoning*/ true,
        )),
        "x-ai/grok-4.20" => Some(KnownProviderModelMetadata::new(
            "Grok 4.20",
            /*context_window*/ 2_000_000,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "x-ai/grok-4.20-multi-agent" => Some(KnownProviderModelMetadata::new(
            "Grok 4.20 Multi-Agent",
            /*context_window*/ 2_000_000,
            /*supports_tools*/ false,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
        )),
        "xiaomi/mimo-v2.5-pro" => Some(model(
            "MiMo V2.5 Pro",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_reasoning*/ true,
        )),
        "xiaomi/mimo-v2.5" => Some(KnownProviderModelMetadata::new(
            "MiMo V2.5",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ false,
            /*supports_reasoning*/ true,
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
        "z-ai/glm-5.2" | "z-ai/glm-5.2-20260616" => (
            Some(ReasoningEffort::High),
            vec![
                reasoning_preset(ReasoningEffort::High, "High reasoning"),
                reasoning_preset(ReasoningEffort::XHigh, "Extra high reasoning"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}

const fn model(
    display_name: &'static str,
    context_window: i64,
    supports_tools: bool,
    supports_reasoning: bool,
) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool_and_input_modalities(
        display_name,
        context_window,
        supports_tools,
        /*supports_parallel_tool_calls*/ false,
        supports_reasoning,
        /*supports_search_tool*/ false,
        super::TEXT_INPUT_MODALITIES,
    )
}
