use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

mod anthropic;
mod cerebras;
mod deepseek;
mod google;
mod minimax;
mod nvidia;
mod openrouter;
mod qwen;
mod xai;
mod xiaomi;
mod zai;

const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";
const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";
const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const QWEN_BASE_URL: &str =
    "https://dashscope-intl.aliyuncs.com/api/v2/apps/protocols/compatible-mode/v1";
const XAI_BASE_URL: &str = "https://api.x.ai/v1";
const XIAOMI_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const ZAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4";
const NO_FALLBACK_MODELS: &[KnownProviderFallbackModel] = &[];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownProviderModelMetadata {
    pub display_name: &'static str,
    pub context_window: i64,
    pub supports_tools: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_reasoning: bool,
    pub supports_search_tool: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownProviderFallbackModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub is_default: bool,
}

impl KnownProviderFallbackModel {
    pub(crate) const fn new(
        id: &'static str,
        display_name: &'static str,
        description: &'static str,
        is_default: bool,
    ) -> Self {
        Self {
            id,
            display_name,
            description,
            is_default,
        }
    }
}

impl KnownProviderModelMetadata {
    pub(crate) const fn new(
        display_name: &'static str,
        context_window: i64,
        supports_tools: bool,
        supports_parallel_tool_calls: bool,
        supports_reasoning: bool,
        supports_search_tool: bool,
    ) -> Self {
        Self {
            display_name,
            context_window,
            supports_tools,
            supports_parallel_tool_calls,
            supports_reasoning,
            supports_search_tool,
        }
    }
}

struct ProviderMetadataSource {
    id: &'static str,
    base_url: &'static str,
    metadata: fn(&str) -> Option<KnownProviderModelMetadata>,
    reasoning_levels: fn(&str) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>),
    fallback_models: &'static [KnownProviderFallbackModel],
    supports_reasoning_effort: bool,
}

const PROVIDER_METADATA_SOURCES: &[ProviderMetadataSource] = &[
    ProviderMetadataSource {
        id: "anthropic",
        base_url: ANTHROPIC_BASE_URL,
        metadata: anthropic::metadata,
        reasoning_levels: no_reasoning_levels_for_slug,
        fallback_models: anthropic::FALLBACK_MODELS,
        supports_reasoning_effort: false,
    },
    ProviderMetadataSource {
        id: "nvidia",
        base_url: NVIDIA_BASE_URL,
        metadata: nvidia::metadata,
        reasoning_levels: nvidia::reasoning_levels,
        fallback_models: nvidia::FALLBACK_MODELS,
        supports_reasoning_effort: true,
    },
    ProviderMetadataSource {
        id: "deepseek",
        base_url: DEEPSEEK_BASE_URL,
        metadata: deepseek::metadata,
        reasoning_levels: no_reasoning_levels_for_slug,
        fallback_models: deepseek::FALLBACK_MODELS,
        supports_reasoning_effort: false,
    },
    ProviderMetadataSource {
        id: "cerebras",
        base_url: CEREBRAS_BASE_URL,
        metadata: cerebras::metadata,
        reasoning_levels: cerebras::reasoning_levels,
        fallback_models: cerebras::FALLBACK_MODELS,
        supports_reasoning_effort: true,
    },
    ProviderMetadataSource {
        id: "openrouter",
        base_url: OPENROUTER_BASE_URL,
        metadata: openrouter::metadata,
        reasoning_levels: openrouter::reasoning_levels,
        fallback_models: NO_FALLBACK_MODELS,
        supports_reasoning_effort: false,
    },
    ProviderMetadataSource {
        id: "qwen",
        base_url: QWEN_BASE_URL,
        metadata: qwen::metadata,
        reasoning_levels: no_reasoning_levels_for_slug,
        fallback_models: qwen::FALLBACK_MODELS,
        supports_reasoning_effort: false,
    },
    ProviderMetadataSource {
        id: "google",
        base_url: GOOGLE_BASE_URL,
        metadata: google::metadata,
        reasoning_levels: google::reasoning_levels,
        fallback_models: google::FALLBACK_MODELS,
        supports_reasoning_effort: true,
    },
    ProviderMetadataSource {
        id: "xai",
        base_url: XAI_BASE_URL,
        metadata: xai::metadata,
        reasoning_levels: no_reasoning_levels_for_slug,
        fallback_models: xai::FALLBACK_MODELS,
        supports_reasoning_effort: false,
    },
    ProviderMetadataSource {
        id: "xiaomi",
        base_url: XIAOMI_BASE_URL,
        metadata: xiaomi::metadata,
        reasoning_levels: no_reasoning_levels_for_slug,
        fallback_models: xiaomi::FALLBACK_MODELS,
        supports_reasoning_effort: false,
    },
    ProviderMetadataSource {
        id: "zai",
        base_url: ZAI_BASE_URL,
        metadata: zai::metadata,
        reasoning_levels: zai::reasoning_levels,
        fallback_models: zai::FALLBACK_MODELS,
        supports_reasoning_effort: true,
    },
    ProviderMetadataSource {
        id: "minimax",
        base_url: MINIMAX_BASE_URL,
        metadata: minimax::metadata,
        reasoning_levels: minimax::reasoning_levels,
        fallback_models: minimax::FALLBACK_MODELS,
        supports_reasoning_effort: true,
    },
];

pub fn metadata_for_openai_compatible_response(
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    slug: &str,
) -> Option<KnownProviderModelMetadata> {
    if let Some(source) = metadata_source_for_openai_compatible(provider_id, provider_base_url) {
        return (source.metadata)(slug);
    }

    if provider_name.is_none() && provider_base_url.is_none() {
        metadata_for_unqualified_slug(slug)
    } else {
        None
    }
}

pub fn metadata_for_local_fallback(
    provider_id: Option<&str>,
    slug: &str,
) -> Option<KnownProviderModelMetadata> {
    if let Some(provider_id) = provider_id {
        return metadata_source_for_provider_id(provider_id)
            .and_then(|source| (source.metadata)(slug));
    }

    metadata_for_unqualified_slug(slug)
}

pub fn provider_supports_reasoning_effort(provider_id: Option<&str>) -> bool {
    provider_id
        .and_then(metadata_source_for_provider_id)
        .is_some_and(|source| source.supports_reasoning_effort)
}

pub fn openai_compatible_provider_supports_reasoning_effort(
    provider_id: Option<&str>,
    provider_base_url: Option<&str>,
) -> bool {
    metadata_source_for_openai_compatible(provider_id, provider_base_url)
        .is_some_and(|source| source.supports_reasoning_effort)
}

pub fn reasoning_levels_for_openai_compatible_response(
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    if let Some(source) = metadata_source_for_openai_compatible(provider_id, provider_base_url) {
        return (source.reasoning_levels)(slug);
    }

    if provider_name.is_none() && provider_base_url.is_none() {
        reasoning_levels_for_unqualified_slug(slug)
    } else {
        no_reasoning_levels()
    }
}

pub fn reasoning_levels_for_local_fallback(
    provider_id: Option<&str>,
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    if let Some(provider_id) = provider_id {
        return metadata_source_for_provider_id(provider_id)
            .map_or_else(no_reasoning_levels, |source| {
                (source.reasoning_levels)(slug)
            });
    }

    reasoning_levels_for_unqualified_slug(slug)
}

pub fn fallback_models_for_provider(provider_id: &str) -> &'static [KnownProviderFallbackModel] {
    metadata_source_for_provider_id(provider_id)
        .map_or(NO_FALLBACK_MODELS, |source| source.fallback_models)
}

fn metadata_source_for_provider_id(provider_id: &str) -> Option<&'static ProviderMetadataSource> {
    PROVIDER_METADATA_SOURCES
        .iter()
        .find(|source| provider_id_matches(Some(provider_id), source.id))
}

fn metadata_source_for_openai_compatible(
    provider_id: Option<&str>,
    provider_base_url: Option<&str>,
) -> Option<&'static ProviderMetadataSource> {
    PROVIDER_METADATA_SOURCES
        .iter()
        .find(|source| provider_matches(provider_id, provider_base_url, source.id, source.base_url))
}

fn provider_id_matches(provider_id: Option<&str>, expected: &str) -> bool {
    provider_id.is_some_and(|provider_id| provider_id.eq_ignore_ascii_case(expected))
}

fn provider_matches(
    provider_id: Option<&str>,
    provider_base_url: Option<&str>,
    expected_id: &str,
    expected_base_url: &str,
) -> bool {
    provider_id_matches(provider_id, expected_id)
        || base_url_matches(provider_base_url, expected_base_url)
}

fn base_url_matches(provider_base_url: Option<&str>, expected_base_url: &str) -> bool {
    provider_base_url.is_some_and(|provider_base_url| {
        provider_base_url
            .trim_end_matches('/')
            .eq_ignore_ascii_case(expected_base_url)
    })
}

fn metadata_for_unqualified_slug(slug: &str) -> Option<KnownProviderModelMetadata> {
    cerebras::metadata(slug)
}

fn reasoning_levels_for_unqualified_slug(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    cerebras::reasoning_levels(slug)
}

fn no_reasoning_levels() -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    (None, Vec::new())
}

fn no_reasoning_levels_for_slug(_: &str) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    no_reasoning_levels()
}

fn reasoning_preset(effort: ReasoningEffort, description: &str) -> ReasoningEffortPreset {
    ReasoningEffortPreset {
        effort,
        description: description.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_fable_metadata_has_current_context_window() {
        assert_eq!(
            metadata_for_local_fallback(Some("anthropic"), "claude-fable-5"),
            Some(KnownProviderModelMetadata::new(
                "Claude Fable 5",
                1_000_000,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ true,
            ))
        );
    }

    #[test]
    fn anthropic_fallback_models_use_fable_as_default() {
        let models = fallback_models_for_provider("anthropic");

        assert_eq!(models[0].id, "claude-fable-5");
        assert!(models[0].is_default);
        assert_eq!(models[1].id, "claude-opus-4-8");
        assert_eq!(models[2].id, "claude-sonnet-4-6");
        assert_eq!(models[3].id, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn xiaomi_ultraspeed_metadata_advertises_tools() {
        assert_eq!(
            metadata_for_local_fallback(Some("xiaomi"), "mimo-v2.5-pro-ultraspeed"),
            Some(KnownProviderModelMetadata::new(
                "MiMo V2.5 Pro UltraSpeed",
                1_048_576,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ true,
            ))
        );
    }

    #[test]
    fn xiaomi_fallback_models_use_ultraspeed_as_default() {
        let models = fallback_models_for_provider("xiaomi");

        assert_eq!(models[0].id, "mimo-v2.5-pro-ultraspeed");
        assert!(models[0].is_default);
        assert_eq!(models[1].id, "mimo-v2.5-pro");
        assert_eq!(models[2].id, "mimo-v2.5");
    }

    #[test]
    fn google_gemini_flash_metadata_uses_current_context_window() {
        assert_eq!(
            metadata_for_local_fallback(Some("google"), "gemini-3.5-flash"),
            Some(KnownProviderModelMetadata::new(
                "Gemini 3.5 Flash",
                1_048_576,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
            ))
        );
    }

    #[test]
    fn google_fallback_models_use_gemini_flash_as_default() {
        let models = fallback_models_for_provider("google");

        assert_eq!(models[0].id, "gemini-3.5-flash");
        assert!(models[0].is_default);
        assert_eq!(models[1].id, "gemini-3.1-pro-preview");
        assert_eq!(models[2].id, "gemini-3-flash-preview");
        assert_eq!(models[3].id, "gemini-3.1-flash-lite");
    }

    #[test]
    fn minimax_metadata_has_current_openai_compatible_models() {
        assert_eq!(
            metadata_for_local_fallback(Some("minimax"), "MiniMax-M3"),
            Some(KnownProviderModelMetadata::new(
                "MiniMax M3",
                1_000_000,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
            ))
        );
        assert_eq!(
            metadata_for_local_fallback(Some("minimax"), "MiniMax-M2.7-highspeed"),
            Some(KnownProviderModelMetadata::new(
                "MiniMax M2.7 Highspeed",
                204_800,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
                /*supports_search_tool*/ false,
            ))
        );
    }

    #[test]
    fn minimax_fallback_models_use_m3_as_default() {
        let models = fallback_models_for_provider("minimax");

        assert_eq!(models.len(), 8);
        assert_eq!(models[0].id, "MiniMax-M3");
        assert!(models[0].is_default);
        assert_eq!(models[7].id, "MiniMax-M2");
    }

    #[test]
    fn deprecated_provider_slugs_do_not_use_curated_metadata() {
        let cases = [
            ("anthropic", "claude-opus-4-1-20250805"),
            ("anthropic", "claude-opus-4-20250514"),
            ("anthropic", "claude-sonnet-4-20250514"),
            ("anthropic", "claude-mythos-5"),
            ("anthropic", "claude-mythos-preview"),
            ("deepseek", "deepseek-chat"),
            ("deepseek", "deepseek-reasoner"),
            ("google", "gemini-2.5-pro"),
            ("google", "gemini-2.5-flash"),
            ("google", "gemini-2.5-flash-lite"),
        ];

        for (provider_id, slug) in cases {
            assert_eq!(metadata_for_local_fallback(Some(provider_id), slug), None);
        }
    }

    #[test]
    fn xai_fallback_models_do_not_expose_grok_build_as_raw_provider() {
        let models = fallback_models_for_provider("xai");

        assert_eq!(models[0].id, "grok-4.3");
        assert!(models[0].is_default);
        assert!(models.iter().all(|model| model.id != "grok-build-0.1"));
        assert_eq!(
            metadata_for_local_fallback(Some("xai"), "grok-build-0.1"),
            None
        );
    }
}
