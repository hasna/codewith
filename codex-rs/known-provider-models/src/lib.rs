use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

mod cerebras;
mod nvidia;
mod openrouter;
mod xiaomi;

const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";
const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const XIAOMI_BASE_URL: &str = "https://api.xiaomimimo.com/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownProviderModelMetadata {
    pub display_name: &'static str,
    pub context_window: i64,
    pub supports_tools: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_reasoning: bool,
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
        Self {
            display_name,
            context_window,
            supports_tools,
            supports_parallel_tool_calls,
            supports_reasoning,
        }
    }
}

pub fn metadata_for_openai_compatible_response(
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    slug: &str,
) -> Option<KnownProviderModelMetadata> {
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
        "openrouter",
        OPENROUTER_BASE_URL,
    ) {
        return openrouter::metadata(slug);
    }
    if provider_matches(provider_id, provider_base_url, "xiaomi", XIAOMI_BASE_URL) {
        return xiaomi::metadata(slug);
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
        Some(provider_id) if provider_id_matches(Some(provider_id), "nvidia") => {
            nvidia::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "cerebras") => {
            cerebras::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "openrouter") => {
            openrouter::metadata(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "xiaomi") => {
            xiaomi::metadata(slug)
        }
        Some(_) => None,
        None => metadata_for_unqualified_slug(slug),
    }
}

pub fn provider_supports_reasoning_effort(provider_id: Option<&str>) -> bool {
    provider_id_matches(provider_id, "nvidia") || provider_id_matches(provider_id, "cerebras")
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
}

pub fn reasoning_levels_for_openai_compatible_response(
    provider_id: Option<&str>,
    provider_name: Option<&str>,
    provider_base_url: Option<&str>,
    slug: &str,
) -> (Option<ReasoningEffort>, Vec<ReasoningEffortPreset>) {
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
        Some(provider_id) if provider_id_matches(Some(provider_id), "nvidia") => {
            nvidia::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "cerebras") => {
            cerebras::reasoning_levels(slug)
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "openrouter") => {
            no_reasoning_levels()
        }
        Some(provider_id) if provider_id_matches(Some(provider_id), "xiaomi") => {
            no_reasoning_levels()
        }
        Some(_) => no_reasoning_levels(),
        None => reasoning_levels_for_unqualified_slug(slug),
    }
}

pub fn fallback_models_for_provider(provider_id: &str) -> &'static [KnownProviderFallbackModel] {
    if provider_id_matches(Some(provider_id), "cerebras") {
        return cerebras::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "nvidia") {
        return nvidia::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "openrouter") {
        return openrouter::FALLBACK_MODELS;
    }
    if provider_id_matches(Some(provider_id), "xiaomi") {
        return xiaomi::FALLBACK_MODELS;
    }

    &[]
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
