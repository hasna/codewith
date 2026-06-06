use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "openai/gpt-oss-120b",
        "OpenAI GPT OSS 120B",
        "NVIDIA hosted OpenAI gpt-oss model. Requires NVIDIA_API_KEY for turns.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "z-ai/glm-5.1",
        "Z.ai GLM 5.1",
        "NVIDIA hosted Z.ai GLM 5.1 model. Requires NVIDIA_API_KEY for turns.",
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
        "openai/gpt-oss-20b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 20B",
            131_072,
            true,
            false,
            true,
        )),
        "deepseek-ai/deepseek-v4-flash" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Flash",
            1_048_576,
            false,
            false,
            false,
        )),
        "deepseek-ai/deepseek-v4-pro" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Pro",
            1_048_576,
            false,
            false,
            false,
        )),
        "z-ai/glm-5.1" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 5.1",
            131_072,
            true,
            false,
            false,
        )),
        "nvidia/nemotron-3.5-content-safety" => Some(KnownProviderModelMetadata::new(
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
