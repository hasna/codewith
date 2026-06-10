use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "claude-fable-5",
        "Claude Fable 5",
        "Anthropic's most capable widely released model. Requires ANTHROPIC_API_KEY for turns.",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "claude-opus-4-8",
        "Claude Opus 4.8",
        "Anthropic's most capable Opus-tier model for complex reasoning and agentic coding.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        "Anthropic's latest Sonnet model for coding, agents, and enterprise workflows.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "claude-haiku-4-5-20251001",
        "Claude Haiku 4.5",
        "Anthropic's fastest current Claude model with near-frontier intelligence.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "claude-fable-5" => Some(model(
            "Claude Fable 5",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
        "claude-opus-4-8" => Some(model(
            "Claude Opus 4.8",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
        "claude-opus-4-7" => Some(model(
            "Claude Opus 4.7",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
        "claude-sonnet-4-6" => Some(model(
            "Claude Sonnet 4.6",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
        "claude-opus-4-6" => Some(model(
            "Claude Opus 4.6",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
        "claude-opus-4-5-20251101" => Some(model(
            "Claude Opus 4.5",
            /*context_window*/ 200_000,
            /*supports_reasoning*/ true,
        )),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => {
            Some(model(
                "Claude Haiku 4.5",
                /*context_window*/ 200_000,
                /*supports_reasoning*/ false,
            ))
        }
        "claude-sonnet-4-5-20250929" => Some(model(
            "Claude Sonnet 4.5",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ false,
        )),
        "claude-opus-4-1-20250805" => Some(model(
            "Claude Opus 4.1",
            /*context_window*/ 200_000,
            /*supports_reasoning*/ false,
        )),
        "claude-opus-4-20250514" => Some(model(
            "Claude Opus 4",
            /*context_window*/ 200_000,
            /*supports_reasoning*/ false,
        )),
        "claude-sonnet-4-20250514" => Some(model(
            "Claude Sonnet 4",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ false,
        )),
        "claude-mythos-5" => Some(model(
            "Claude Mythos 5",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
        "claude-mythos-preview" => Some(model(
            "Claude Mythos Preview",
            /*context_window*/ 1_000_000,
            /*supports_reasoning*/ true,
        )),
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
