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
            131_072,
            true,
            false,
            true,
        )),
        "openai/gpt-oss-120b:free" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B (Free)",
            131_072,
            true,
            false,
            true,
        )),
        "openai/gpt-oss-20b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 20B",
            131_072,
            true,
            false,
            true,
        )),
        "openai/gpt-oss-20b:free" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 20B (Free)",
            131_072,
            true,
            false,
            true,
        )),
        "openai/gpt-oss-safeguard-20b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS Safeguard 20B",
            131_072,
            true,
            false,
            true,
        )),
        "z-ai/glm-4.7" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7",
            202_752,
            true,
            false,
            true,
        )),
        "z-ai/glm-4.7-flash" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7 Flash",
            202_752,
            true,
            false,
            true,
        )),
        "z-ai/glm-5.1" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 5.1",
            202_752,
            true,
            true,
            true,
        )),
        "deepseek/deepseek-v4-flash" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Flash",
            1_048_576,
            true,
            false,
            false,
        )),
        "deepseek/deepseek-v4-pro" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Pro",
            1_048_576,
            true,
            false,
            false,
        )),
        "nvidia/nemotron-3.5-content-safety:free" => Some(KnownProviderModelMetadata::new(
            "NVIDIA Nemotron 3.5 Content Safety",
            128_000,
            false,
            false,
            false,
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
