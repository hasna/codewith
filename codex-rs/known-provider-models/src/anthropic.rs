use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "claude-fable-5",
        "Claude Fable 5",
        "Anthropic's most capable widely released model. Requires ANTHROPIC_API_KEY for turns.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "claude-opus-4-8",
        "Claude Opus 4.8",
        "Anthropic's most capable Opus-tier model for complex reasoning and agentic coding.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        "Anthropic's latest Sonnet model for coding, agents, and enterprise workflows.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "claude-haiku-4-5-20251001",
        "Claude Haiku 4.5",
        "Anthropic's fastest current Claude model with near-frontier intelligence.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "claude-fable-5" => Some(model("Claude Fable 5", 1_000_000, true)),
        "claude-opus-4-8" => Some(model("Claude Opus 4.8", 1_000_000, true)),
        "claude-opus-4-7" => Some(model("Claude Opus 4.7", 1_000_000, true)),
        "claude-sonnet-4-6" => Some(model("Claude Sonnet 4.6", 1_000_000, true)),
        "claude-opus-4-6" => Some(model("Claude Opus 4.6", 1_000_000, true)),
        "claude-opus-4-5-20251101" => Some(model("Claude Opus 4.5", 200_000, true)),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => {
            Some(model("Claude Haiku 4.5", 200_000, false))
        }
        "claude-sonnet-4-5-20250929" => Some(model("Claude Sonnet 4.5", 1_000_000, false)),
        "claude-opus-4-1-20250805" => Some(model("Claude Opus 4.1", 200_000, false)),
        "claude-opus-4-20250514" => Some(model("Claude Opus 4", 200_000, false)),
        "claude-sonnet-4-20250514" => Some(model("Claude Sonnet 4", 1_000_000, false)),
        "claude-mythos-5" => Some(model("Claude Mythos 5", 1_000_000, true)),
        "claude-mythos-preview" => Some(model("Claude Mythos Preview", 1_000_000, true)),
        _ => None,
    }
}

const fn model(
    display_name: &'static str,
    context_window: i64,
    supports_reasoning: bool,
) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ true,
        supports_reasoning,
    )
}
