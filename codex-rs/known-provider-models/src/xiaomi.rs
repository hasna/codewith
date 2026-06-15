use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "mimo-v2.5-pro-ultraspeed",
        "MiMo V2.5 Pro UltraSpeed",
        "Xiaomi MiMo's high-speed flagship coding model. Requires MIMO_API_KEY for turns.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "mimo-v2.5-pro",
        "MiMo V2.5 Pro",
        "Xiaomi MiMo's flagship model for agentic coding and long-context work.",
        false,
    ),
    KnownProviderFallbackModel::new(
        "mimo-v2.5",
        "MiMo V2.5",
        "Xiaomi MiMo's multimodal agent foundation model.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "mimo-v2.5-pro-ultraspeed" => Some(model("MiMo V2.5 Pro UltraSpeed", 1_048_576)),
        "mimo-v2.5-pro" => Some(model("MiMo V2.5 Pro", 1_048_576)),
        "mimo-v2.5" => Some(model("MiMo V2.5", 262_144)),
        _ => None,
    }
}

const fn model(display_name: &'static str, context_window: i64) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        context_window,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ true,
        /*supports_search_tool*/ true,
    )
}
