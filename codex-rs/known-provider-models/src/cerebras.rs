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
        true,
    ),
    KnownProviderFallbackModel::new(
        "zai-glm-4.7",
        "Z.ai GLM 4.7",
        "Cerebras hosted Z.ai GLM 4.7 model. Requires CEREBRAS_API_KEY for turns.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "gpt-oss-120b" => Some(KnownProviderModelMetadata::new(
            "OpenAI GPT OSS 120B",
            131_072,
            true,
            false,
            true,
        )),
        "zai-glm-4.7" => Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 4.7",
            131_072,
            true,
            true,
            true,
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
