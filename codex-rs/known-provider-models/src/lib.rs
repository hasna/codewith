use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
// Provider IDs and base URLs are owned by `codex_protocol::provider_identity` so
// this metadata crate and `codex-model-provider-info` share a single maintained
// boundary instead of repeating drift-prone literals.
use codex_protocol::provider_identity::ANTHROPIC_BASE_URL;
use codex_protocol::provider_identity::ANTHROPIC_PROVIDER_ID;
use codex_protocol::provider_identity::CEREBRAS_BASE_URL;
use codex_protocol::provider_identity::CEREBRAS_PROVIDER_ID;
use codex_protocol::provider_identity::DEEPSEEK_BASE_URL;
use codex_protocol::provider_identity::DEEPSEEK_PROVIDER_ID;
use codex_protocol::provider_identity::GOOGLE_BASE_URL;
use codex_protocol::provider_identity::GOOGLE_PROVIDER_ID;
use codex_protocol::provider_identity::KIMI_BASE_URL;
use codex_protocol::provider_identity::KIMI_PROVIDER_ID;
use codex_protocol::provider_identity::MINIMAX_BASE_URL;
use codex_protocol::provider_identity::MINIMAX_PROVIDER_ID;
use codex_protocol::provider_identity::NVIDIA_BASE_URL;
use codex_protocol::provider_identity::NVIDIA_PROVIDER_ID;
use codex_protocol::provider_identity::OPENROUTER_BASE_URL;
use codex_protocol::provider_identity::OPENROUTER_PROVIDER_ID;
use codex_protocol::provider_identity::QWEN_BASE_URL;
use codex_protocol::provider_identity::QWEN_PROVIDER_ID;
use codex_protocol::provider_identity::XAI_BASE_URL;
use codex_protocol::provider_identity::XAI_PROVIDER_ID;
use codex_protocol::provider_identity::XIAOMI_BASE_URL;
use codex_protocol::provider_identity::XIAOMI_PROVIDER_ID;
use codex_protocol::provider_identity::ZAI_BASE_URL;
use codex_protocol::provider_identity::ZAI_PROVIDER_ID;

mod anthropic;
mod cerebras;
mod deepseek;
mod google;
mod kimi;
mod minimax;
mod nvidia;
mod openai;
mod openrouter;
mod qwen;
mod xai;
mod xiaomi;
mod zai;

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
        ANTHROPIC_PROVIDER_ID,
        ANTHROPIC_BASE_URL,
    ) {
        return anthropic::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        NVIDIA_PROVIDER_ID,
        NVIDIA_BASE_URL,
    ) {
        return nvidia::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        CEREBRAS_PROVIDER_ID,
        CEREBRAS_BASE_URL,
    ) {
        return cerebras::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        DEEPSEEK_PROVIDER_ID,
        DEEPSEEK_BASE_URL,
    ) {
        return deepseek::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        GOOGLE_PROVIDER_ID,
        GOOGLE_BASE_URL,
    ) {
        return google::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        KIMI_PROVIDER_ID,
        KIMI_BASE_URL,
    ) {
        return kimi::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        MINIMAX_PROVIDER_ID,
        MINIMAX_BASE_URL,
    ) {
        return minimax::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        OPENROUTER_PROVIDER_ID,
        OPENROUTER_BASE_URL,
    ) {
        return openrouter::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        QWEN_PROVIDER_ID,
        QWEN_BASE_URL,
    ) {
        return qwen::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        XAI_PROVIDER_ID,
        XAI_BASE_URL,
    ) {
        return xai::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        XIAOMI_PROVIDER_ID,
        XIAOMI_BASE_URL,
    ) {
        return xiaomi::metadata(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        ZAI_PROVIDER_ID,
        ZAI_BASE_URL,
    ) {
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
        Some(provider_id) if provider_id_matches(Some(provider_id), ANTHROPIC_PROVIDER_ID) => {
            anthropic::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), NVIDIA_PROVIDER_ID) => {
            nvidia::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), CEREBRAS_PROVIDER_ID) => {
            cerebras::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), DEEPSEEK_PROVIDER_ID) => {
            deepseek::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), GOOGLE_PROVIDER_ID) => {
            google::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), KIMI_PROVIDER_ID) => {
            kimi::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), MINIMAX_PROVIDER_ID) => {
            minimax::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), OPENROUTER_PROVIDER_ID) => {
            openrouter::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), QWEN_PROVIDER_ID) => {
            qwen::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), XAI_PROVIDER_ID) => {
            xai::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), XIAOMI_PROVIDER_ID) => {
            xiaomi::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), ZAI_PROVIDER_ID) => {
            zai::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), openai::OPENAI_PROVIDER_ID) => {
            openai::metadata(slug)
        }
        Some(_) => None,
        None => metadata_for_unqualified_slug(slug),
    }
}

pub fn provider_supports_reasoning_effort(provider_id: Option<&str>) -> bool {
    provider_id_matches(provider_id, NVIDIA_PROVIDER_ID)
        || provider_id_matches(provider_id, CEREBRAS_PROVIDER_ID)
        || provider_id_matches(provider_id, GOOGLE_PROVIDER_ID)
        || provider_id_matches(provider_id, MINIMAX_PROVIDER_ID)
        || provider_id_matches(provider_id, OPENROUTER_PROVIDER_ID)
        || provider_id_matches(provider_id, ZAI_PROVIDER_ID)
}

pub fn openai_compatible_provider_supports_reasoning_effort(
    provider_id: Option<&str>,
    provider_base_url: Option<&str>,
) -> bool {
    provider_matches(
        provider_id,
        provider_base_url,
        NVIDIA_PROVIDER_ID,
        NVIDIA_BASE_URL,
    ) || provider_matches(
        provider_id,
        provider_base_url,
        CEREBRAS_PROVIDER_ID,
        CEREBRAS_BASE_URL,
    ) || provider_matches(
        provider_id,
        provider_base_url,
        GOOGLE_PROVIDER_ID,
        GOOGLE_BASE_URL,
    ) || provider_matches(
        provider_id,
        provider_base_url,
        MINIMAX_PROVIDER_ID,
        MINIMAX_BASE_URL,
    ) || provider_matches(
        provider_id,
        provider_base_url,
        OPENROUTER_PROVIDER_ID,
        OPENROUTER_BASE_URL,
    ) || provider_matches(
        provider_id,
        provider_base_url,
        ZAI_PROVIDER_ID,
        ZAI_BASE_URL,
    )
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
        ANTHROPIC_PROVIDER_ID,
        ANTHROPIC_BASE_URL,
    ) {
        return no_reasoning_levels();
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        NVIDIA_PROVIDER_ID,
        NVIDIA_BASE_URL,
    ) {
        return nvidia::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        CEREBRAS_PROVIDER_ID,
        CEREBRAS_BASE_URL,
    ) {
        return cerebras::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        GOOGLE_PROVIDER_ID,
        GOOGLE_BASE_URL,
    ) {
        return google::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        MINIMAX_PROVIDER_ID,
        MINIMAX_BASE_URL,
    ) {
        return minimax::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        OPENROUTER_PROVIDER_ID,
        OPENROUTER_BASE_URL,
    ) {
        return openrouter::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        XIAOMI_PROVIDER_ID,
        XIAOMI_BASE_URL,
    ) {
        return xiaomi::reasoning_levels(slug);
    }
    if provider_matches(
        provider_id,
        provider_base_url,
        ZAI_PROVIDER_ID,
        ZAI_BASE_URL,
    ) {
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
        Some(provider_id) if provider_id_matches(Some(provider_id), ANTHROPIC_PROVIDER_ID) => {
            no_reasoning_levels()
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), NVIDIA_PROVIDER_ID) => {
            nvidia::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), CEREBRAS_PROVIDER_ID) => {
            cerebras::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), GOOGLE_PROVIDER_ID) => {
            google::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), MINIMAX_PROVIDER_ID) => {
            minimax::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), OPENROUTER_PROVIDER_ID) => {
            openrouter::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), XIAOMI_PROVIDER_ID) => {
            no_reasoning_levels()
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), ZAI_PROVIDER_ID) => {
            zai::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), openai::OPENAI_PROVIDER_ID) => {
            openai::reasoning_levels(slug)
        }
        Some(_) => no_reasoning_levels(),
        None => reasoning_levels_for_unqualified_slug(slug),
    }
}

pub fn fallback_models_for_provider(provider_id: &str) -> &'static [KnownProviderFallbackModel] {
    if provider_id_matches(Some(provider_id), ANTHROPIC_PROVIDER_ID) {
        return anthropic::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), CEREBRAS_PROVIDER_ID) {
        return cerebras::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), DEEPSEEK_PROVIDER_ID) {
        return deepseek::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), GOOGLE_PROVIDER_ID) {
        return google::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), KIMI_PROVIDER_ID) {
        return kimi::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), MINIMAX_PROVIDER_ID) {
        return minimax::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), NVIDIA_PROVIDER_ID) {
        return nvidia::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), OPENROUTER_PROVIDER_ID) {
        return openrouter::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), QWEN_PROVIDER_ID) {
        return qwen::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), XAI_PROVIDER_ID) {
        return xai::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), XIAOMI_PROVIDER_ID) {
        return xiaomi::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), ZAI_PROVIDER_ID) {
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
    // Unqualified slugs default to OpenAI's own API models; only fall through to
    // the Cerebras-hosted OpenAI-compatible catalog (e.g. `gpt-oss-120b`) when the
    // slug is not a known first-party OpenAI model.
    openai::metadata(slug).or_else(|| cerebras::metadata(slug))
}

fn reasoning_levels_for_unqualified_slug(
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
    if openai::metadata(slug).is_some() {
        openai::reasoning_levels(slug)
    } else {
        cerebras::reasoning_levels(slug)
    }
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
            metadata_for_local_fallback(Some(ANTHROPIC_PROVIDER_ID), "claude-fable-5"),
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
        let models = fallback_models_for_provider(ANTHROPIC_PROVIDER_ID);

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
            metadata_for_local_fallback(Some(ANTHROPIC_PROVIDER_ID), "claude-sonnet-5"),
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
            let metadata = metadata_for_local_fallback(Some(DEEPSEEK_PROVIDER_ID), slug)
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
            reasoning_levels_for_local_fallback(Some(DEEPSEEK_PROVIDER_ID), "deepseek-v4-pro"),
            (None, Vec::new())
        );
    }

    #[test]
    fn kimi_fallback_models_default_to_k3() {
        let models = fallback_models_for_provider(KIMI_PROVIDER_ID);

        assert_eq!(models[0].id, "kimi-k3");
        assert!(models[0].is_default);
        assert!(
            models
                .iter()
                .any(|model| model.id == "kimi-k2.7-code" && !model.is_default)
        );
        assert!(
            models
                .iter()
                .any(|model| model.id == "kimi-k2.6" && !model.is_default)
        );
    }

    #[test]
    fn kimi_k3_metadata_advertises_long_context_and_multimodal_input() {
        assert_eq!(
            metadata_for_local_fallback(Some(KIMI_PROVIDER_ID), "kimi-k3"),
            Some(
                KnownProviderModelMetadata::with_search_tool_and_input_modalities(
                    "Kimi K3",
                    /*context_window*/ 1_000_000,
                    /*supports_tools*/ true,
                    /*supports_parallel_tool_calls*/ false,
                    /*supports_reasoning*/ true,
                    /*supports_search_tool*/ false,
                    DEFAULT_INPUT_MODALITIES,
                )
            )
        );

        // The dedicated coding model is text-only with a 256K context.
        let code = metadata_for_openai_compatible_response(
            Some(KIMI_PROVIDER_ID),
            None,
            None,
            "kimi-k2.7-code",
        )
        .expect("kimi-k2.7-code metadata should exist");
        assert_eq!(code.context_window, 262_144);
        assert_eq!(code.input_modalities, TEXT_INPUT_MODALITIES);
        assert!(code.supports_tools);

        // The general-purpose model supports vision (text + image).
        assert_eq!(
            metadata_for_local_fallback(Some(KIMI_PROVIDER_ID), "kimi-k2.6")
                .expect("kimi-k2.6 metadata should exist")
                .input_modalities,
            DEFAULT_INPUT_MODALITIES
        );
    }

    #[test]
    fn kimi_toggles_thinking_without_reasoning_effort_presets() {
        // Like DeepSeek, Kimi toggles thinking with a provider-specific parameter
        // rather than the OpenAI-style reasoning-effort scale, so it advertises
        // reasoning support without exposing effort presets.
        assert!(
            metadata_for_local_fallback(Some(KIMI_PROVIDER_ID), "kimi-k3")
                .expect("kimi-k3 metadata should exist")
                .supports_reasoning
        );
        assert!(!provider_supports_reasoning_effort(Some(KIMI_PROVIDER_ID)));
        assert_eq!(
            reasoning_levels_for_local_fallback(Some(KIMI_PROVIDER_ID), "kimi-k3"),
            (None, Vec::new())
        );
    }

    #[test]
    fn cerebras_exposes_gemma_4_31b_preview_fallback() {
        let models = fallback_models_for_provider(CEREBRAS_PROVIDER_ID);

        assert_eq!(models[0].id, "gpt-oss-120b");
        assert!(models[0].is_default);
        assert!(
            models
                .iter()
                .any(|model| model.id == "gemma-4-31b" && !model.is_default)
        );
        assert_eq!(
            metadata_for_local_fallback(Some(CEREBRAS_PROVIDER_ID), "gemma-4-31b"),
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
    fn qwen_fallback_models_expose_qwen36_flash() {
        let models = fallback_models_for_provider(QWEN_PROVIDER_ID);

        assert_eq!(models[0].id, "qwen3.5-flash");
        assert!(models[0].is_default);
        assert!(
            models
                .iter()
                .any(|model| model.id == "qwen3.6-flash" && !model.is_default)
        );
        // The orphaned metadata entry is now reachable from the fallback list.
        assert!(
            metadata_for_local_fallback(Some(QWEN_PROVIDER_ID), "qwen3.6-flash")
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
                metadata_for_local_fallback(Some(NVIDIA_PROVIDER_ID), slug)
                    .expect("nvidia deepseek metadata should exist")
                    .supports_tools,
                "{slug} should support tools like the direct and OpenRouter catalogs"
            );
        }
    }

    #[test]
    fn nvidia_fallback_tracks_zai_glm_5_2_catalog() {
        let models = fallback_models_for_provider("nvidia");

        assert!(
            models.iter().any(|model| model.id == "z-ai/glm-5.2"),
            "nvidia fallback should include z-ai/glm-5.2 from the current NVIDIA catalog"
        );
        assert!(
            !models.iter().any(|model| model.id == "z-ai/glm-5.1"),
            "nvidia fallback should drop z-ai/glm-5.1 which is no longer in the NVIDIA catalog"
        );

        assert_eq!(
            metadata_for_local_fallback(Some("nvidia"), "z-ai/glm-5.2")
                .expect("nvidia z-ai/glm-5.2 metadata should exist")
                .display_name,
            "Z.ai GLM 5.2"
        );
        assert_eq!(
            metadata_for_local_fallback(Some("nvidia"), "z-ai/glm-5.1"),
            None,
            "nvidia should no longer expose z-ai/glm-5.1 metadata"
        );
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
            metadata_for_local_fallback(Some(OPENROUTER_PROVIDER_ID), "anthropic/claude-sonnet-5"),
            expected
        );
        assert_eq!(
            metadata_for_openai_compatible_response(
                Some(OPENROUTER_PROVIDER_ID),
                None,
                None,
                "anthropic/claude-sonnet-5",
            ),
            expected
        );
        assert!(
            metadata_for_local_fallback(Some(OPENROUTER_PROVIDER_ID), "anthropic/claude-fable-5")
                .is_some()
        );
        assert!(
            metadata_for_local_fallback(Some(OPENROUTER_PROVIDER_ID), "anthropic/claude-opus-4-8")
                .is_some()
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
            metadata_for_local_fallback(Some(OPENROUTER_PROVIDER_ID), "z-ai/glm-5.2"),
            expected_metadata
        );
        assert_eq!(
            metadata_for_openai_compatible_response(
                Some(OPENROUTER_PROVIDER_ID),
                None,
                None,
                "z-ai/glm-5.2-20260616",
            ),
            expected_metadata
        );

        assert!(provider_supports_reasoning_effort(Some(
            OPENROUTER_PROVIDER_ID
        )));
        assert!(openai_compatible_provider_supports_reasoning_effort(
            Some(OPENROUTER_PROVIDER_ID),
            None
        ));

        let (default_reasoning, presets) = reasoning_levels_for_openai_compatible_response(
            Some(OPENROUTER_PROVIDER_ID),
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
            reasoning_levels_for_local_fallback(Some(OPENROUTER_PROVIDER_ID), "z-ai/glm-5.2"),
            (default_reasoning, presets)
        );
    }

    #[test]
    fn openrouter_fallback_models_keep_default_and_include_glm52() {
        let models = fallback_models_for_provider(OPENROUTER_PROVIDER_ID);

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
            (ANTHROPIC_PROVIDER_ID, "claude-fable-5"),
            (CEREBRAS_PROVIDER_ID, "gpt-oss-120b"),
            (DEEPSEEK_PROVIDER_ID, "deepseek-v4-flash"),
            (GOOGLE_PROVIDER_ID, "gemini-3.5-flash"),
            (KIMI_PROVIDER_ID, "kimi-k3"),
            (MINIMAX_PROVIDER_ID, "MiniMax-M3"),
            (NVIDIA_PROVIDER_ID, "nvidia/nemotron-3-ultra-550b-a55b"),
            (OPENROUTER_PROVIDER_ID, "z-ai/glm-5.2"),
            (QWEN_PROVIDER_ID, "qwen3.5-flash"),
            (XAI_PROVIDER_ID, "grok-4.3"),
            (XIAOMI_PROVIDER_ID, "mimo-v2.5-pro"),
            (ZAI_PROVIDER_ID, "glm-5.2"),
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
            metadata_for_local_fallback(Some(QWEN_PROVIDER_ID), "qwen3.5-flash")
                .expect("qwen metadata should exist")
                .supports_search_tool
        );
        assert!(
            metadata_for_local_fallback(Some(ZAI_PROVIDER_ID), "glm-5.2")
                .expect("zai metadata should exist")
                .supports_search_tool
        );
        assert!(
            metadata_for_local_fallback(Some(XAI_PROVIDER_ID), "grok-4.3")
                .expect("grok metadata should exist")
                .supports_search_tool
        );
        assert!(
            !metadata_for_local_fallback(Some(XAI_PROVIDER_ID), "grok-build-0.1")
                .expect("grok build metadata should exist")
                .supports_search_tool
        );
        assert!(
            !metadata_for_local_fallback(Some(XIAOMI_PROVIDER_ID), "mimo-v2.5-pro")
                .expect("xiaomi metadata should exist")
                .supports_search_tool
        );
    }

    #[test]
    fn xai_exposes_grok_4_5_flagship_metadata() {
        assert_eq!(
            metadata_for_local_fallback(Some(XAI_PROVIDER_ID), "grok-4.5"),
            Some(KnownProviderModelMetadata::with_search_tool(
                "Grok 4.5", /*context_window*/ 500_000, /*supports_tools*/ true,
                /*supports_parallel_tool_calls*/ false, /*supports_reasoning*/ true,
                /*supports_search_tool*/ true,
            ))
        );
        assert_eq!(
            metadata_for_openai_compatible_response(Some(XAI_PROVIDER_ID), None, None, "grok-4.5"),
            metadata_for_local_fallback(Some(XAI_PROVIDER_ID), "grok-4.5"),
        );

        // grok-4.5 is offered alongside the existing grok-4.3 default without
        // displacing it (grok-4.3 keeps its larger 1M-token context window).
        let models = fallback_models_for_provider(XAI_PROVIDER_ID);
        assert_eq!(models[0].id, "grok-4.3");
        assert!(models[0].is_default);
        assert!(
            models
                .iter()
                .any(|model| model.id == "grok-4.5" && !model.is_default)
        );
    }

    #[test]
    fn zai_glm_5_2_metadata_matches_documented_capabilities() {
        assert_eq!(
            metadata_for_local_fallback(Some(ZAI_PROVIDER_ID), "glm-5.2"),
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
            metadata_for_local_fallback(Some(ZAI_PROVIDER_ID), "glm-5.2[1m]"),
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

        let (default_effort, presets) =
            reasoning_levels_for_local_fallback(Some(ZAI_PROVIDER_ID), "glm-5.2");
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
            reasoning_levels_for_local_fallback(Some(ZAI_PROVIDER_ID), "glm-5.1"),
            (None, Vec::new())
        );
    }

    #[test]
    fn provider_for_fallback_model_finds_unique_configured_provider() {
        assert_eq!(
            provider_for_fallback_model(
                "mimo-v2.5-pro",
                ["openai", XIAOMI_PROVIDER_ID, ANTHROPIC_PROVIDER_ID]
            ),
            Some(XIAOMI_PROVIDER_ID)
        );
    }

    #[test]
    fn provider_for_fallback_model_ignores_unconfigured_provider() {
        assert_eq!(
            provider_for_fallback_model("mimo-v2.5-pro", ["openai", ANTHROPIC_PROVIDER_ID]),
            None
        );
    }

    #[test]
    fn provider_for_fallback_model_requires_unique_match() {
        assert_eq!(
            provider_for_fallback_model("mimo-v2.5-pro", [XIAOMI_PROVIDER_ID, XIAOMI_PROVIDER_ID]),
            None
        );
    }

    /// Regression guard for the shared provider-identity boundary.
    ///
    /// These literals are the canonical provider IDs and base URLs owned by
    /// `codex_protocol::provider_identity`. Pinning them here means that if a
    /// shared constant drifts, this test fails and forces an intentional review
    /// instead of a silent, cross-registry behavior change. The literals are the
    /// explicit fixture allowed by the acceptance criteria; the non-test code
    /// no longer repeats them.
    #[test]
    fn shared_provider_identity_constants_match_canonical_values() {
        assert_eq!(ANTHROPIC_PROVIDER_ID, "anthropic");
        assert_eq!(ANTHROPIC_BASE_URL, "https://api.anthropic.com/v1");
        assert_eq!(CEREBRAS_PROVIDER_ID, "cerebras");
        assert_eq!(CEREBRAS_BASE_URL, "https://api.cerebras.ai/v1");
        assert_eq!(DEEPSEEK_PROVIDER_ID, "deepseek");
        assert_eq!(DEEPSEEK_BASE_URL, "https://api.deepseek.com/v1");
        assert_eq!(GOOGLE_PROVIDER_ID, "google");
        assert_eq!(
            GOOGLE_BASE_URL,
            "https://generativelanguage.googleapis.com/v1beta/openai"
        );
        assert_eq!(KIMI_PROVIDER_ID, "kimi");
        assert_eq!(KIMI_BASE_URL, "https://api.moonshot.ai/v1");
        assert_eq!(MINIMAX_PROVIDER_ID, "minimax");
        assert_eq!(MINIMAX_BASE_URL, "https://api.minimax.io/v1");
        assert_eq!(NVIDIA_PROVIDER_ID, "nvidia");
        assert_eq!(NVIDIA_BASE_URL, "https://integrate.api.nvidia.com/v1");
        assert_eq!(OPENROUTER_PROVIDER_ID, "openrouter");
        assert_eq!(OPENROUTER_BASE_URL, "https://openrouter.ai/api/v1");
        assert_eq!(QWEN_PROVIDER_ID, "qwen");
        assert_eq!(
            QWEN_BASE_URL,
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(XAI_PROVIDER_ID, "xai");
        assert_eq!(XAI_BASE_URL, "https://api.x.ai/v1");
        assert_eq!(XIAOMI_PROVIDER_ID, "xiaomi");
        assert_eq!(XIAOMI_BASE_URL, "https://api.xiaomimimo.com/v1");
        assert_eq!(ZAI_PROVIDER_ID, "zai");
        assert_eq!(ZAI_BASE_URL, "https://api.z.ai/api/paas/v4");
    }

    /// Provider matching still works for representative providers, exercised
    /// through both the provider id and the base URL with and without a trailing
    /// slash using the shared identity constants.
    #[test]
    fn provider_matching_works_for_representative_providers() {
        for (provider_id, base_url) in [
            (OPENROUTER_PROVIDER_ID, OPENROUTER_BASE_URL),
            (ZAI_PROVIDER_ID, ZAI_BASE_URL),
            (ANTHROPIC_PROVIDER_ID, ANTHROPIC_BASE_URL),
            (GOOGLE_PROVIDER_ID, GOOGLE_BASE_URL),
        ] {
            assert!(
                provider_matches(Some(provider_id), None, provider_id, base_url),
                "{provider_id} should match by provider id"
            );
            let uppercased = provider_id.to_ascii_uppercase();
            assert!(
                provider_matches(Some(&uppercased), None, provider_id, base_url),
                "{provider_id} should match case-insensitively by provider id"
            );
            let with_trailing_slash = format!("{base_url}/");
            assert!(
                provider_matches(None, Some(&with_trailing_slash), provider_id, base_url),
                "{provider_id} should match by base url ignoring a trailing slash"
            );
            assert!(
                !provider_matches(
                    Some("not-a-provider"),
                    Some("https://example.invalid"),
                    provider_id,
                    base_url
                ),
                "{provider_id} should not match an unrelated provider"
            );
        }
    }

    /// GPT-4.1-class OpenAI API models expose their documented 1,047,576-token
    /// context window instead of the generic 272k fallback, whether the provider
    /// id is the explicit `openai` id or an unqualified default.
    #[test]
    fn openai_gpt_4_1_family_uses_documented_context_window() {
        let expected = Some(KnownProviderModelMetadata::new(
            "GPT-4.1", /*context_window*/ 1_047_576, /*supports_tools*/ true,
            /*supports_parallel_tool_calls*/ true, /*supports_reasoning*/ false,
        ));

        assert_eq!(
            metadata_for_local_fallback(Some(openai::OPENAI_PROVIDER_ID), "gpt-4.1"),
            expected
        );
        // Unqualified (no provider id) resolves the same first-party metadata.
        assert_eq!(metadata_for_local_fallback(None, "gpt-4.1"), expected);

        for slug in ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"] {
            let metadata = metadata_for_local_fallback(Some(openai::OPENAI_PROVIDER_ID), slug)
                .unwrap_or_else(|| panic!("{slug} metadata should exist"));
            assert_eq!(
                metadata.context_window, 1_047_576,
                "{slug} should report the documented GPT-4.1 context window"
            );
            assert!(
                !metadata.supports_reasoning,
                "{slug} is not a reasoning model"
            );
        }

        // GPT-4.1 models are not reasoning models, so no effort presets are exposed.
        assert_eq!(
            reasoning_levels_for_local_fallback(Some(openai::OPENAI_PROVIDER_ID), "gpt-4.1"),
            (None, Vec::new())
        );
    }

    /// Current GPT-5.x OpenAI API models report their documented 1,050,000-token
    /// context window in the local fallback so known models are never pinned to
    /// the stale 272k default when they are missing from the live catalog.
    #[test]
    fn openai_gpt_5_x_models_use_documented_context_window() {
        for slug in [
            "gpt-5.4",
            "gpt-5.5",
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
        ] {
            let metadata = metadata_for_local_fallback(Some(openai::OPENAI_PROVIDER_ID), slug)
                .unwrap_or_else(|| panic!("{slug} metadata should exist"));
            assert_eq!(
                metadata.context_window, 1_050_000,
                "{slug} should report the documented GPT-5.x context window"
            );
            assert!(metadata.supports_reasoning, "{slug} is a reasoning model");

            let (default_effort, presets) =
                reasoning_levels_for_local_fallback(Some(openai::OPENAI_PROVIDER_ID), slug);
            assert_eq!(default_effort, Some(ReasoningEffort::Medium));
            assert_eq!(
                presets,
                vec![
                    reasoning_preset(ReasoningEffort::Low, "Minimal reasoning"),
                    reasoning_preset(ReasoningEffort::Medium, "Moderate reasoning"),
                    reasoning_preset(ReasoningEffort::High, "Extensive reasoning"),
                ]
            );
        }
    }

    /// Genuinely unknown OpenAI slugs stay conservative (no metadata), so callers
    /// fall through to the documented generic fallback.
    #[test]
    fn openai_unknown_slug_stays_conservative() {
        assert_eq!(
            metadata_for_local_fallback(Some(openai::OPENAI_PROVIDER_ID), "gpt-does-not-exist"),
            None
        );
    }

    /// Adding OpenAI first-party metadata must not shadow the Cerebras-hosted
    /// `gpt-oss-120b` model reachable through the unqualified path.
    #[test]
    fn unqualified_gpt_oss_still_resolves_to_cerebras_catalog() {
        assert_eq!(
            metadata_for_local_fallback(None, "gpt-oss-120b"),
            metadata_for_local_fallback(Some(CEREBRAS_PROVIDER_ID), "gpt-oss-120b")
        );
    }
}
