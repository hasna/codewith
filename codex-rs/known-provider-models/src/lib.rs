use codex_protocol::openai_models::InputModality;
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
pub(crate) const DEFAULT_INPUT_MODALITIES: &[InputModality] =
    &[InputModality::Text, InputModality::Image];
pub(crate) const TEXT_INPUT_MODALITIES: &[InputModality] = &[InputModality::Text];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownProviderModelMetadata {
    pub display_name: &'static str,
    pub context_window: i64,
    pub supports_tools: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_reasoning: bool,
    pub supports_search_tool: bool,
    pub input_modalities: &'static [InputModality],
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
    ) -> Self {
        Self::with_search_tool(
            display_name,
            context_window,
            supports_tools,
            supports_parallel_tool_calls,
            supports_reasoning,
            /*supports_search_tool*/ false,
        )
    }

    pub(crate) const fn with_search_tool(
        display_name: &'static str,
        context_window: i64,
        supports_tools: bool,
        supports_parallel_tool_calls: bool,
        supports_reasoning: bool,
        supports_search_tool: bool,
    ) -> Self {
        Self::with_search_tool_and_input_modalities(
            display_name,
            context_window,
            supports_tools,
            supports_parallel_tool_calls,
            supports_reasoning,
            supports_search_tool,
            DEFAULT_INPUT_MODALITIES,
        )
    }

    pub(crate) const fn with_search_tool_and_input_modalities(
        display_name: &'static str,
        context_window: i64,
        supports_tools: bool,
        supports_parallel_tool_calls: bool,
        supports_reasoning: bool,
        supports_search_tool: bool,
        input_modalities: &'static [InputModality],
    ) -> Self {
        Self {
            display_name,
            context_window,
            supports_tools,
            supports_parallel_tool_calls,
            supports_reasoning,
            supports_search_tool,
            input_modalities,
        }
    }
}

pub fn metadata_for_openai_compatible_response(
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    slug: &str,
) -> Option<KnownProviderModelMetadata> {
    if provider_matches(
        provider_id,
        provider_base_url,
        "anthropic",
        ANTHROPIC_BASE_URL,
    ) {
        return anthropic::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "nvidia", NVIDIA_BASE_URL) {
        return nvidia::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        "cerebras",
        CEREBRAS_BASE_URL,
    ) {
        return cerebras::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        "deepseek",
        DEEPSEEK_BASE_URL,
    ) {
        return deepseek::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "google", GOOGLE_BASE_URL) {
        return google::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "minimax", MINIMAX_BASE_URL) {
        return minimax::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        "openrouter",
        OPENROUTER_BASE_URL,
    ) {
        return openrouter::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "qwen", QWEN_BASE_URL) {
        return qwen::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "xai", XAI_BASE_URL) {
        return xai::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "xiaomi", XIAOMI_BASE_URL) {
        return xiaomi::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "zai", ZAI_BASE_URL) {
        return zai::metadata(slug);
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
    match provider_id {
        Some(provider_id) if provider_id_matches(Some(provider_id), "anthropic") => {
            anthropic::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "nvidia") => {
            nvidia::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "cerebras") => {
            cerebras::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "deepseek") => {
            deepseek::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "google") => {
            google::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "minimax") => {
            minimax::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "openrouter") => {
            openrouter::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "qwen") => qwen::metadata(slug),
        Some(provider_id) if provider_id_matches(Some(provider_id), "xai") => xai::metadata(slug),
        Some(provider_id) if provider_id_matches(Some(provider_id), "xiaomi") => {
            xiaomi::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "zai") => zai::metadata(slug),
        Some(_) => None,
        None => metadata_for_unqualified_slug(slug),
    }
}

pub fn provider_supports_reasoning_effort(provider_id: Option<&str>) -> bool {
    provider_id_matches(provider_id, "nvidia")
        || provider_id_matches(provider_id, "cerebras")
        || provider_id_matches(provider_id, "google")
        || provider_id_matches(provider_id, "minimax")
        || provider_id_matches(provider_id, "openrouter")
        || provider_id_matches(provider_id, "zai")
}

pub fn openai_compatible_provider_supports_reasoning_effort(
    provider_id: Option<&str>,
    provider_base_url: Option<&str>,
) -> bool {
    provider_matches(provider_id, provider_base_url, "nvidia", NVIDIA_BASE_URL)
        || provider_matches(
            provider_id,
            provider_base_url,
            "cerebras",
            CEREBRAS_BASE_URL,
        )
        || provider_matches(provider_id, provider_base_url, "google", GOOGLE_BASE_URL)
        || provider_matches(provider_id, provider_base_url, "minimax", MINIMAX_BASE_URL)
        || provider_matches(
            provider_id,
            provider_base_url,
            "openrouter",
            OPENROUTER_BASE_URL,
        )
        || provider_matches(provider_id, provider_base_url, "zai", ZAI_BASE_URL)
}

pub fn reasoning_levels_for_openai_compatible_response(
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    if provider_matches(
        provider_id,
        provider_base_url,
        "anthropic",
        ANTHROPIC_BASE_URL,
    ) {
        return no_reasoning_levels();
    }
    if provider_matches(provider_id, provider_base_url, "nvidia", NVIDIA_BASE_URL) {
        return nvidia::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        "cerebras",
        CEREBRAS_BASE_URL,
    ) {
        return cerebras::reasoning_levels(slug);
    }
    if provider_matches(provider_id, provider_base_url, "google", GOOGLE_BASE_URL) {
        return google::reasoning_levels(slug);
    }
    if provider_matches(provider_id, provider_base_url, "minimax", MINIMAX_BASE_URL) {
        return minimax::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        "openrouter",
        OPENROUTER_BASE_URL,
    ) {
        return openrouter::reasoning_levels(slug);
    }
    if provider_matches(provider_id, provider_base_url, "xiaomi", XIAOMI_BASE_URL) {
        return xiaomi::reasoning_levels(slug);
    }
    if provider_matches(provider_id, provider_base_url, "zai", ZAI_BASE_URL) {
        return zai::reasoning_levels(slug);
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
    match provider_id {
        Some(provider_id) if provider_id_matches(Some(provider_id), "anthropic") => {
            no_reasoning_levels()
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "nvidia") => {
            nvidia::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "cerebras") => {
            cerebras::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "google") => {
            google::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "minimax") => {
            minimax::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "openrouter") => {
            openrouter::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "xiaomi") => {
            no_reasoning_levels()
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "zai") => {
            zai::reasoning_levels(slug)
        }
        Some(_) => no_reasoning_levels(),
        None => reasoning_levels_for_unqualified_slug(slug),
    }
}

pub fn fallback_models_for_provider(provider_id: &str) -> &'static [KnownProviderFallbackModel] {
    if provider_id_matches(Some(provider_id), "anthropic") {
        return anthropic::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "cerebras") {
        return cerebras::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "deepseek") {
        return deepseek::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "google") {
        return google::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "minimax") {
        return minimax::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "nvidia") {
        return nvidia::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "openrouter") {
        return openrouter::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "qwen") {
        return qwen::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "xai") {
        return xai::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "xiaomi") {
        return xiaomi::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "zai") {
        return zai::FALLBACK_MODELS;
    }

    &[]
}

pub fn provider_for_fallback_model<'a>(
    model_id: &str,
    provider_ids: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut matches = provider_ids
        .into_iter()
        .filter(|provider_id| provider_fallback_models_contain(provider_id, model_id));
    let provider_id = matches.next()?;
    matches.next().is_none().then_some(provider_id)
}

fn provider_fallback_models_contain(provider_id: &str, model_id: &str) -> bool {
    fallback_models_for_provider(provider_id)
        .iter()
        .any(|model| model.id == model_id)
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
                /*context_window*/ 1_000_000,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
            ))
        );
    }

    #[test]
    fn anthropic_fallback_models_use_fable_as_default() {
        let models = fallback_models_for_provider("anthropic");

        assert_eq!(models[0].id, "claude-fable-5");
        assert!(models[0].is_default);
        assert_eq!(models[1].id, "claude-opus-4-8");
        assert_eq!(models[2].id, "claude-sonnet-5");
        assert_eq!(models[3].id, "claude-sonnet-4-6");
        assert_eq!(models[4].id, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn anthropic_exposes_claude_sonnet_5_metadata() {
        assert_eq!(
            metadata_for_local_fallback(Some("anthropic"), "claude-sonnet-5"),
            Some(KnownProviderModelMetadata::new(
                "Claude Sonnet 5",
                /*context_window*/ 1_000_000,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ true,
                /*supports_reasoning*/ true,
            ))
        );
    }

    #[test]
    fn deepseek_v4_models_advertise_reasoning_without_effort_presets() {
        for slug in ["deepseek-v4-flash", "deepseek-v4-pro"] {
            let metadata = metadata_for_local_fallback(Some("deepseek"), slug)
                .expect("deepseek metadata should exist");
            assert!(
                metadata.supports_reasoning,
                "{slug} should advertise reasoning (thinking mode)"
            );
            assert!(metadata.supports_tools, "{slug} should support tools");
        }

        // DeepSeek toggles thinking with a provider-specific parameter, not the
        // OpenAI-style reasoning-effort scale, so no effort presets are exposed.
        assert_eq!(
            reasoning_levels_for_local_fallback(Some("deepseek"), "deepseek-v4-pro"),
            (None, Vec::new())
        );
    }

    #[test]
    fn cerebras_exposes_gemma_4_31b_preview_fallback() {
        let models = fallback_models_for_provider("cerebras");

        assert_eq!(models[0].id, "gpt-oss-120b");
        assert!(models[0].is_default);
        assert!(
            models
                .iter()
                .any(|model| model.id == "gemma-4-31b" && !model.is_default)
        );
        assert_eq!(
            metadata_for_local_fallback(Some("cerebras"), "gemma-4-31b"),
            Some(KnownProviderModelMetadata::new(
                "Gemma 4 31B",
                /*context_window*/ 131_072,
                /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false,
                /*supports_reasoning*/ false,
            ))
        );
    }

    #[test]
    fn qwen_fallback_models_prefer_current_recommended_text_models() {
        let models = fallback_models_for_provider("qwen");

        assert_eq!(models[0].id, "qwen3.6-flash");
        assert!(models[0].is_default);
        assert_eq!(
            models
                .iter()
                .map(|model| (model.id, model.is_default))
                .collect::<Vec<_>>(),
            vec![
                ("qwen3.6-flash", true),
                ("qwen3.7-plus", false),
                ("qwen3.7-max", false),
                ("qwen3.5-flash", false),
                ("qwen3.5-plus", false),
                ("qwen3-max", false),
            ]
        );
        assert!(
            metadata_for_local_fallback(Some("qwen"), "qwen3.6-flash")
                .expect("qwen3.6-flash metadata should exist")
                .supports_search_tool
        );
    }

    #[test]
    fn nvidia_deepseek_v4_models_support_tools() {
        for slug in [
            "deepseek-ai/deepseek-v4-flash",
            "deepseek-ai/deepseek-v4-pro",
        ] {
            assert!(
                metadata_for_local_fallback(Some("nvidia"), slug)
                    .expect("nvidia deepseek metadata should exist")
                    .supports_tools,
                "{slug} should support tools like the direct and OpenRouter catalogs"
            );
        }
    }

    #[test]
    fn openrouter_exposes_anthropic_claude_metadata() {
        let expected = Some(KnownProviderModelMetadata::new(
            "Claude Sonnet 5",
            /*context_window*/ 1_000_000,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ true,
            /*supports_reasoning*/ true,
        ));

        assert_eq!(
            metadata_for_local_fallback(Some("openrouter"), "anthropic/claude-sonnet-5"),
            expected
        );
        assert_eq!(
            metadata_for_openai_compatible_response(
                Some("openrouter"),
                None,
                None,
                "anthropic/claude-sonnet-5",
            ),
            expected
        );
        assert!(
            metadata_for_local_fallback(Some("openrouter"), "anthropic/claude-fable-5").is_some()
        );
        assert!(
            metadata_for_local_fallback(Some("openrouter"), "anthropic/claude-opus-4-8").is_some()
        );
    }

    #[test]
    fn openrouter_glm52_metadata_matches_models_api() {
        let expected_metadata = Some(KnownProviderModelMetadata::new(
            "Z.ai GLM 5.2",
            /*context_window*/ 1_048_576,
            /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ true,
            /*supports_reasoning*/ true,
        ));

        assert_eq!(
            metadata_for_local_fallback(Some("openrouter"), "z-ai/glm-5.2"),
            expected_metadata
        );
        assert_eq!(
            metadata_for_openai_compatible_response(
                Some("openrouter"),
                None,
                None,
                "z-ai/glm-5.2-20260616",
            ),
            expected_metadata
        );

        assert!(provider_supports_reasoning_effort(Some("openrouter")));
        assert!(openai_compatible_provider_supports_reasoning_effort(
            Some("openrouter"),
            None
        ));

        let (default_reasoning, presets) = reasoning_levels_for_openai_compatible_response(
            Some("openrouter"),
            None,
            None,
            "z-ai/glm-5.2",
        );
        assert_eq!(default_reasoning, Some(ReasoningEffort::High));
        assert_eq!(
            presets,
            vec![
                reasoning_preset(ReasoningEffort::High, "High reasoning"),
                reasoning_preset(ReasoningEffort::XHigh, "Extra high reasoning"),
            ]
        );
        assert_eq!(
            reasoning_levels_for_local_fallback(Some("openrouter"), "z-ai/glm-5.2"),
            (default_reasoning, presets)
        );
    }

    #[test]
    fn openrouter_fallback_models_keep_default_and_include_glm52() {
        let models = fallback_models_for_provider("openrouter");

        assert_eq!(models[0].id, "z-ai/glm-5.2");
        assert!(models[0].is_default);
        assert!(
            models
                .iter()
                .any(|model| model.id == "openai/gpt-oss-120b" && !model.is_default)
        );
    }

    #[test]
    fn every_builtin_external_provider_has_fallback_models() {
        let cases = [
            ("anthropic", "claude-fable-5"),
            ("cerebras", "gpt-oss-120b"),
            ("deepseek", "deepseek-v4-flash"),
            ("google", "gemini-3.5-flash"),
            ("minimax", "MiniMax-M3"),
            ("nvidia", "nvidia/nemotron-3-ultra-550b-a55b"),
            ("openrouter", "z-ai/glm-5.2"),
            ("qwen", "qwen3.6-flash"),
            ("xai", "grok-4.3"),
            ("xiaomi", "mimo-v2.5-pro"),
            ("zai", "glm-5.2"),
        ];

        for (provider_id, default_model) in cases {
            let models = fallback_models_for_provider(provider_id);

            assert!(
                !models.is_empty(),
                "{provider_id} should have fallback models"
            );
            assert_eq!(models[0].id, default_model);
            assert!(models[0].is_default);
        }
    }

    #[test]
    fn provider_metadata_preserves_native_search_support_by_model() {
        assert!(
            metadata_for_local_fallback(Some("qwen"), "qwen3.5-flash")
                .expect("qwen metadata should exist")
                .supports_search_tool
        );
        assert!(
            metadata_for_local_fallback(Some("zai"), "glm-5.2")
                .expect("zai metadata should exist")
                .supports_search_tool
        );
        assert!(
            metadata_for_local_fallback(Some("xai"), "grok-4.3")
                .expect("grok metadata should exist")
                .supports_search_tool
        );
        assert!(
            !metadata_for_local_fallback(Some("xai"), "grok-build-0.1")
                .expect("grok build metadata should exist")
                .supports_search_tool
        );
        assert!(
            !metadata_for_local_fallback(Some("xiaomi"), "mimo-v2.5-pro")
                .expect("xiaomi metadata should exist")
                .supports_search_tool
        );
    }

    #[test]
    fn zai_glm_5_2_metadata_matches_documented_capabilities() {
        assert_eq!(
            metadata_for_local_fallback(Some("zai"), "glm-5.2"),
            Some(
                KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                    "GLM-5.2",
                    /*context_window*/ 1_000_000,
                    /*supports_tools*/ true,
                    /*supports_parallel_tool_calls*/ false,
                    /*supports_reasoning*/ true,
                    /*supports_search_tool*/ true,
                    TEXT_INPUT_MODALITIES,
                )
            )
        );
        assert_eq!(
            metadata_for_local_fallback(Some("zai"), "glm-5.2[1m]"),
            Some(
                KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                    "GLM-5.2 1M",
                    /*context_window*/ 1_000_000,
                    /*supports_tools*/ true,
                    /*supports_parallel_tool_calls*/ false,
                    /*supports_reasoning*/ true,
                    /*supports_search_tool*/ true,
                    TEXT_INPUT_MODALITIES,
                )
            )
        );

        let (default_effort, presets) = reasoning_levels_for_local_fallback(Some("zai"), "glm-5.2");
        assert_eq!(
            (default_effort, presets),
            (
                Some(ReasoningEffort::Custom("max".to_string())),
                vec![
                    reasoning_preset(ReasoningEffort::None, "Reasoning disabled"),
                    reasoning_preset(ReasoningEffort::High, "High reasoning"),
                    reasoning_preset(ReasoningEffort::Custom("max".to_string()), "Max reasoning"),
                ],
            )
        );
        assert_eq!(
            reasoning_levels_for_local_fallback(Some("zai"), "glm-5.1"),
            (None, Vec::new())
        );
    }

    #[test]
    fn provider_for_fallback_model_finds_unique_configured_provider() {
        assert_eq!(
            provider_for_fallback_model("mimo-v2.5-pro", ["openai", "xiaomi", "anthropic"]),
            Some("xiaomi")
        );
    }

    #[test]
    fn provider_for_fallback_model_ignores_unconfigured_provider() {
        assert_eq!(
            provider_for_fallback_model("mimo-v2.5-pro", ["openai", "anthropic"]),
            None
        );
    }

    #[test]
    fn provider_for_fallback_model_requires_unique_match() {
        assert_eq!(
            provider_for_fallback_model("mimo-v2.5-pro", ["xiaomi", "xiaomi"]),
            None
        );
    }
}
