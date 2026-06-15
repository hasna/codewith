use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "openai/gpt-oss-120b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B",
            131_072,
            true,
            false,
            true,
            true,
        )),
        "z-ai/glm-4.7" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7",
            202_752,
            true,
            false,
            false,
            true,
        )),
        "z-ai/glm-4.7-flash" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7 Flash",
            202_752,
            true,
            false,
            false,
            true,
        )),
        "deepseek/deepseek-v4-flash" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Flash",
            1_048_576,
            true,
            false,
            false,
            true,
        )),
        "deepseek/deepseek-v4-pro" => Some(KnownProviderModelMetadata::new(
            "DeepSeek V4 Pro",
            1_048_576,
            true,
            false,
            false,
            true,
        )),
        "nvidia/nemotron-3.5-content-safety:free" => Some(KnownProviderModelMetadata::new(
            "NVIDIA Nemotron 3.5 Content Safety",
            128_000,
            false,
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
        "openai/gpt-oss-120b" => (
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
