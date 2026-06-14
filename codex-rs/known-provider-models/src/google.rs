use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "gemini-3.5-flash",
        "Gemini 3.5 Flash",
        "Google Gemini's stable frontier Flash model. Requires GEMINI_API_KEY for turns.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3.1-pro-preview",
        "Gemini 3.1 Pro Preview",
        "Google Gemini's preview Pro model for complex reasoning and coding.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3-flash-preview",
        "Gemini 3 Flash Preview",
        "Google Gemini's preview Flash model with a large context window.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3.1-flash-lite",
        "Gemini 3.1 Flash-Lite",
        "Google Gemini's stable low-latency Flash-Lite model.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "gemini-3.5-flash" => Some(model("Gemini 3.5 Flash")),
        "gemini-3.1-pro-preview" => Some(model("Gemini 3.1 Pro Preview")),
        "gemini-3.1-pro-preview-customtools" => Some(model("Gemini 3.1 Pro Preview Custom Tools")),
        "gemini-3-flash-preview" => Some(model("Gemini 3 Flash Preview")),
        "gemini-3.1-flash-lite" => Some(model("Gemini 3.1 Flash-Lite")),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "gemini-3.5-flash"
        | "gemini-3.1-pro-preview"
        | "gemini-3.1-pro-preview-customtools"
        | "gemini-3-flash-preview"
        | "gemini-3.1-flash-lite" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::Minimal, "Minimal thinking"),
                reasoning_preset(ReasoningEffort::Low, "Low thinking"),
                reasoning_preset(ReasoningEffort::Medium, "Moderate thinking"),
                reasoning_preset(ReasoningEffort::High, "Extensive thinking"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}

const fn model(display_name: &'static str) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        1_048_576,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ false,
    )
}
