use super::DEFAULT_INPUT_MODALITIES;
use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;
use super::TEXT_INPUT_MODALITIES;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "kimi-k3",
        "Kimi K3",
        "Moonshot's flagship Kimi model with a 1M-token context for agentic coding. \
Requires a Kimi Code subscription API key (MOONSHOT_API_KEY).",
        /*is_default*/ true,
    ),
    KnownProviderFallbackModel::new(
        "kimi-k2.7-code",
        "Kimi K2.7 Code",
        "Moonshot's dedicated coding Kimi model with a 256K-token context.",
        /*is_default*/ false,
    ),
    KnownProviderFallbackModel::new(
        "kimi-k2.6",
        "Kimi K2.6",
        "Moonshot's general-purpose Kimi model with vision and a 256K-token context.",
        /*is_default*/ false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "kimi-k3" => Some(flagship("Kimi K3")),
        "kimi-k2.7-code" => Some(code("Kimi K2.7 Code")),
        "kimi-k2.6" => Some(vision("Kimi K2.6")),
        _ => None,
    }
}

// Kimi's flagship model advertises a 1M-token context and multimodal (text +
// image) input. Kimi toggles thinking with a provider-specific parameter rather
// than the OpenAI-style reasoning-effort scale, so it advertises reasoning
// support (like DeepSeek) without wiring reasoning-effort presets.
const fn flagship(display_name: &'static str) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool_and_input_modalities(
        display_name,
        /*context_window*/ 1_000_000,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ false,
        DEFAULT_INPUT_MODALITIES,
    )
}

// The dedicated coding model is text-only and exposes a 256K-token context.
const fn code(display_name: &'static str) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool_and_input_modalities(
        display_name,
        /*context_window*/ 262_144,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ false,
        TEXT_INPUT_MODALITIES,
    )
}

// The general-purpose model supports vision (text + image) with a 256K-token
// context and thinking/non-thinking modes.
const fn vision(display_name: &'static str) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::with_search_tool_and_input_modalities(
        display_name,
        /*context_window*/ 262_144,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ false,
        DEFAULT_INPUT_MODALITIES,
    )
}
