use super::KnownProviderFallbackModel;
use super::KnownProviderModelMetadata;

pub(crate) const FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[
    KnownProviderFallbackModel::new(
        "deepseek-v4-flash",
        "DeepSeek V4 Flash",
        "DeepSeek's high-speed coding model. Requires DEEPSEEK_API_KEY for turns.",
        true,
    ),
    KnownProviderFallbackModel::new(
        "deepseek-v4-pro",
        "DeepSeek V4 Pro",
        "DeepSeek's higher-capability V4 model for agentic coding and long-context work.",
        false,
    ),
];

pub(crate) fn metadata(slug: &str) -> Option<KnownProviderModelMetadata> {
    match slug {
        "deepseek-v4-flash" => Some(model("DeepSeek V4 Flash")),
        "deepseek-v4-pro" => Some(model("DeepSeek V4 Pro")),
        _ => None,
    }
}

const fn model(display_name: &'static str) -> KnownProviderModelMetadata {
    KnownProviderModelMetadata::new(
        display_name,
        1_048_576,
        /*supports_tools*/ true,
        /*supports_parallel_tool_calls*/ false,
        /*supports_reasoning*/ false,
    )
}
