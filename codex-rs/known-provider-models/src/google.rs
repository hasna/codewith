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
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3.6-flash",
        "Gemini 3.6 Flash",
        "Google Gemini's latest Flash model, balancing speed with strong agentic and multimodal performance.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3.1-pro-preview",
        "Gemini 3.1 Pro Preview",
        "Google Gemini's preview Pro model for complex reasoning and coding.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3-flash-preview",
        "Gemini 3 Flash Preview",
        "Google Gemini's preview Flash model with a large context window.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3.1-flash-lite",
        "Gemini 3.1 Flash-Lite",
        "Google Gemini's stable low-latency Flash-Lite model.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "gemini-3.5-flash-lite",
        "Gemini 3.5 Flash-Lite",
        "Google Gemini's fastest, most cost-effective 3.5 model for high-throughput execution.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "gemini-3.5-flash" => Some(model("Gemini 3.5 Flash")),
        "gemini-3.6-flash" => Some(model("Gemini 3.6 Flash")),
        "gemini-3.1-pro-preview" => Some(model("Gemini 3.1 Pro Preview")),
        "gemini-3.1-pro-preview-customtools" => Some(model("Gemini 3.1 Pro Preview Custom Tools")),
        "gemini-3-flash-preview" => Some(model("Gemini 3 Flash Preview")),
        "gemini-3.1-flash-lite" => Some(model("Gemini 3.1 Flash-Lite")),
        "gemini-3.5-flash-lite" => Some(model("Gemini 3.5 Flash-Lite")),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "gemini-3.5-flash"
        | "gemini-3.6-flash"
        | "gemini-3.1-pro-preview"
        | "gemini-3.1-pro-preview-customtools"
        | "gemini-3-flash-preview"
        | "gemini-3.1-flash-lite"
        | "gemini-3.5-flash-lite" => (
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
        /*context_window*/ 1_048_576,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
    )
}
