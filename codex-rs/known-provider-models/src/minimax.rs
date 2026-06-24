use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::reasoning_preset;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "MiniMax-M3",
        "MiniMax M3",
        "MiniMax's latest M-series model for agentic reasoning, tool use, coding, and long-context tasks. Requires MINIMAX_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2.7",
        "MiniMax M2.7",
        "MiniMax M2.7 model for recursive self-improvement workflows.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2.7-highspeed",
        "MiniMax M2.7 Highspeed",
        "MiniMax M2.7 highspeed variant with lower latency.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2.5",
        "MiniMax M2.5",
        "MiniMax M2.5 model for complex tasks and agentic coding.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2.5-highspeed",
        "MiniMax M2.5 Highspeed",
        "MiniMax M2.5 highspeed variant with lower latency.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2.1",
        "MiniMax M2.1",
        "MiniMax M2.1 model with enhanced multi-language programming capabilities.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2.1-highspeed",
        "MiniMax M2.1 Highspeed",
        "MiniMax M2.1 highspeed variant with lower latency.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "MiniMax-M2",
        "MiniMax M2",
        "MiniMax M2 model with agentic capabilities and advanced reasoning.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "MiniMax-M3" => Some(model("MiniMax M3", /*context_window*/ 1_000_000)),
        "MiniMax-M2.7" => Some(model("MiniMax M2.7", /*context_window*/ 204_800)),
        "MiniMax-M2.7-highspeed" => {
            Some(model(
                "MiniMax M2.7 Highspeed",
                /*context_window*/ 204_800,
            ))
        }
        "MiniMax-M2.5" => Some(model("MiniMax M2.5", /*context_window*/ 204_800)),
        "MiniMax-M2.5-highspeed" => {
            Some(model(
                "MiniMax M2.5 Highspeed",
                /*context_window*/ 204_800,
            ))
        }
        "MiniMax-M2.1" => Some(model("MiniMax M2.1", /*context_window*/ 204_800)),
        "MiniMax-M2.1-highspeed" => {
            Some(model(
                "MiniMax M2.1 Highspeed",
                /*context_window*/ 204_800,
            ))
        }
        "MiniMax-M2" => Some(model("MiniMax M2", /*context_window*/ 204_800)),
        _ => None,
    }
}

pub(crate) fn reasoning_levels(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    match slug {
        "MiniMax-M3"
        | "MiniMax-M2.7"
        | "MiniMax-M2.7-highspeed"
        | "MiniMax-M2.5"
        | "MiniMax-M2.5-highspeed"
        | "MiniMax-M2.1"
        | "MiniMax-M2.1-highspeed"
        | "MiniMax-M2" => (
            Some(ReasoningEffort::Medium),
            vec![
                reasoning_preset(ReasoningEffort::None, "Reasoning disabled when supported"),
                reasoning_preset(ReasoningEffort::Medium, "Reasoning enabled"),
            ],
        ),
        _ => (None, Vec::new()),
    }
}

const fn model(display_name: &'static str, context_window: i64) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ true,
        /*supports_reasoning*/ true,
    )
}
