//! Context-window metadata for OpenAI's own first-party API models.
//!
//! OpenAI is the built-in provider, so its models are normally resolved from the
//! authoritative bundled catalog (`models-manager/models.json`) or the live
//! `/models` endpoint. When a known OpenAI API model is *not* present in those
//! sources (e.g. a slug configured directly against an OpenAI API key, or a build
//! whose bundled catalog predates a model), core previously fell back to a generic
//! 272k context window. That stale limit truncates/compacts turns far below the
//! documented capacity of these models.
//!
//! This module keeps the fallback conservative for genuinely unknown slugs while
//! reporting the documented context window for known OpenAI API models. Values are
//! taken from the published OpenAI API model docs
//! (<https://developers.openai.com/api/docs/models>).

use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderModelMetadata;
use super::reasoning_preset;

/// Provider id for OpenAI's own API. OpenAI intentionally lives outside the shared
/// `provider_identity` boundary, so the literal is kept local to this crate.
pub(crate) const OPENAI_PROVIDER_ID: &str = "openai";

/// GPT-4.1 family context window (`gpt-4.1`, `gpt-4.1-mini`, `gpt-4.1-nano`).
/// Docs: <https://developers.openai.com/api/docs/models/gpt-4.1> reports a
/// 1,047,576-token context window.
const GPT_4_1_CONTEXT_WINDOW: i64 = 1_047_576;

/// GPT-5.4 / GPT-5.5 / GPT-5.6 API context window. Docs report a 1M
/// (1,050,000-token) context window for these current OpenAI API models. This
/// matches the authoritative value carried in `models-manager/models.json`.
const GPT_5_API_CONTEXT_WINDOW: i64 = 1_050_000;

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        // GPT-4.1 family: multimodal, tool-capable, non-reasoning.
        "gpt-4.1" => Some(non_reasoning_model("GPT-4.1", GPT_4_1_CONTEXT_WINDOW)),
        "gpt-4.1-mini" => Some(non_reasoning_model("GPT-4.1 mini", GPT_4_1_CONTEXT_WINDOW)),
        "gpt-4.1-nano" => Some(non_reasoning_model("GPT-4.1 nano", GPT_4_1_CONTEXT_WINDOW)),
        // Current GPT-5.x API models: multimodal, tool-capable, reasoning.
        "gpt-5.4" => Some(reasoning_model("GPT-5.4", GPT_5_API_CONTEXT_WINDOW)),
        "gpt-5.5" => Some(reasoning_model("GPT-5.5", GPT_5_API_CONTEXT_WINDOW)),
        "gpt-5.6" => Some(reasoning_model("GPT-5.6", GPT_5_API_CONTEXT_WINDOW)),
        "gpt-5.6-sol" => Some(reasoning_model("GPT-5.6 Sol", GPT_5_API_CONTEXT_WINDOW)),
        "gpt-5.6-terra" => Some(reasoning_model("GPT-5.6 Terra", GPT_5_API_CONTEXT_WINDOW)),
        "gpt-5.6-luna" => Some(reasoning_model("GPT-5.6 Luna", GPT_5_API_CONTEXT_WINDOW)),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "gpt-5.4" | "gpt-5.5" | "gpt-5.6" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::Low, "Minimal reasoning"),
                reasoning_preset(ReasoningEffort::Medium, "Moderate reasoning"),
                reasoning_preset(ReasoningEffort::High, "Extensive reasoning"),
            ],
        ),
        // GPT-4.1 models are not reasoning models.
        _ => (None, Vec::new()),
    }
}

fn non_reasoning_model(
    display_name: &'static str,
    context_window: i64,
) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ true,
        /*supports_reasoning*/ false,
    )
}

fn reasoning_model(display_name: &'static str, context_window: i64) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ true,
        /*supports_reasoning*/ true,
    )
}
