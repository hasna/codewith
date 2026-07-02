use codex_known_provider_models as known_provider_models;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelInstructionsVariables;
use codex_protocol::openai_models::ModelMessages;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationMode;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;
use codex_protocol::openai_models::default_input_modalities;

use crate::config::ModelsManagerConfig;
use codex_utils_output_truncation::approx_bytes_for_tokens;
use tracing::warn;

pub const BASE_INSTRUCTIONS: &str = include_str!("../prompt.md");
const DEFAULT_PERSONALITY_HEADER: &str = "You are Codewith, a coding agent running on the selected model. You and the user share the same workspace and collaborate to achieve the user's goals.";
const LOCAL_FRIENDLY_TEMPLATE: &str =
    "You optimize for team morale and being a supportive teammate as much as code quality.";
const LOCAL_PRAGMATIC_TEMPLATE: &str = "You are a deeply pragmatic, effective software engineer.";
const PERSONALITY_PLACEHOLDER: &str = "{{ personality }}";
const OPENAI_MODEL_PROVIDER_ID: &str = "openai";
const GPT_5_5_MODEL_ID: &str = "gpt-5.5";
const GPT_5_5_OPENAI_CONTEXT_WINDOW: i64 = 128_000;
pub const GPT_5_3_CODEX_SPARK: &str = "gpt-5.3-codex-spark";

pub fn with_config_overrides(mut model: ModelInfo, config: &ModelsManagerConfig) -> ModelInfo {
    if let Some(supports_reasoning_summaries) = config.model_supports_reasoning_summaries
        && supports_reasoning_summaries
    {
        model.supports_reasoning_summaries = true;
    }
    if let Some(context_window) = config.model_context_window {
        model.context_window = Some(
            model
                .max_context_window
                .map_or(context_window, |max_context_window| {
                    context_window.min(max_context_window)
                }),
        );
    }
    if let Some(auto_compact_token_limit) = config.model_auto_compact_token_limit {
        model.auto_compact_token_limit = Some(auto_compact_token_limit);
    }
    if let Some(token_limit) = config.tool_output_token_limit {
        model.truncation_policy = match model.truncation_policy.mode {
            TruncationMode::Bytes => {
                let byte_limit =
                    i64::try_from(approx_bytes_for_tokens(token_limit)).unwrap_or(i64::MAX);
                TruncationPolicyConfig::bytes(byte_limit)
            }
            TruncationMode::Tokens => {
                let limit = i64::try_from(token_limit).unwrap_or(i64::MAX);
                TruncationPolicyConfig::tokens(limit)
            }
        };
    }

    if let Some(base_instructions) = &config.base_instructions {
        model.base_instructions = base_instructions.clone();
        model.model_messages = None;
    } else if !config.personality_enabled {
        model.model_messages = None;
    }

    apply_endpoint_context_caps(&mut model, config);

    model
}

fn apply_endpoint_context_caps(model: &mut ModelInfo, config: &ModelsManagerConfig) {
    let is_gpt_5_5_endpoint_slug = model
        .slug
        .strip_prefix(GPT_5_5_MODEL_ID)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('-'));
    if config.model_provider_id.as_deref() != Some(OPENAI_MODEL_PROVIDER_ID)
        || !is_gpt_5_5_endpoint_slug
    {
        return;
    }

    model.context_window = Some(
        model
            .context_window
            .unwrap_or(GPT_5_5_OPENAI_CONTEXT_WINDOW)
            .min(GPT_5_5_OPENAI_CONTEXT_WINDOW),
    );
    model.max_context_window = Some(
        model
            .max_context_window
            .unwrap_or(GPT_5_5_OPENAI_CONTEXT_WINDOW)
            .min(GPT_5_5_OPENAI_CONTEXT_WINDOW),
    );
}

/// Build a minimal fallback model descriptor for missing/unknown slugs.
pub fn model_info_from_slug(slug: &str) -> ModelInfo {
    model_info_from_slug_for_provider(slug, /*provider_id*/ None)
}

pub(crate) fn model_info_from_slug_for_provider(
    slug: &str,
    provider_id: Option<&str>,
) -> ModelInfo {
    if slug == GPT_5_3_CODEX_SPARK && provider_id.is_none_or(|id| id == "openai") {
        return codex_spark_model_info();
    }

    if let Some(metadata) = known_provider_models::metadata_for_local_fallback(provider_id, slug) {
        let base_instructions = fallback_base_instructions_for_slug(slug);
        let (default_reasoning_level, supported_reasoning_levels) =
            known_provider_models::reasoning_levels_for_local_fallback(provider_id, slug);
        let supports_reasoning_summaries =
            metadata.supports_reasoning && fallback_supports_reasoning_summaries(provider_id);
        return ModelInfo {
            slug: slug.to_string(),
            display_name: metadata.display_name.to_string(),
            description: None,
            default_reasoning_level,
            supported_reasoning_levels,
            shell_type: ConfigShellToolType::Default,
            visibility: ModelVisibility::None,
            supported_in_api: true,
            priority: 99,
            additional_speed_tiers: Vec::new(),
            service_tiers: Vec::new(),
            default_service_tier: None,
            availability_nux: None,
            upgrade: None,
            base_instructions,
            model_messages: local_personality_messages_for_slug(slug),
            supports_reasoning_summaries,
            default_reasoning_summary: ReasoningSummary::Auto,
            support_verbosity: false,
            default_verbosity: None,
            apply_patch_tool_type: None,
            web_search_tool_type: WebSearchToolType::Text,
            truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
            supports_parallel_tool_calls: metadata.supports_parallel_tool_calls,
            supports_image_detail_original: false,
            context_window: Some(metadata.context_window),
            max_context_window: Some(metadata.context_window),
            auto_compact_token_limit: None,
            effective_context_window_percent: 95,
            experimental_supported_tools: if metadata.supports_tools {
                vec!["tools".to_string()]
            } else {
                Vec::new()
            },
            input_modalities: metadata.input_modalities.to_vec(),
            used_fallback_model_metadata: false,
            supports_search_tool: metadata.supports_search_tool,
            use_responses_lite: false,
            auto_review_model_override: None,
            tool_mode: None,
            multi_agent_version: None,
        };
    }

    warn!("Unknown model {slug} is used. This will use fallback model metadata.");
    let base_instructions = fallback_base_instructions_for_slug(slug);
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: None,
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: ConfigShellToolType::Default,
        visibility: ModelVisibility::None,
        supported_in_api: true,
        priority: 99,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions,
        model_messages: local_personality_messages_for_slug(slug),
        supports_reasoning_summaries: false,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: Some(272_000),
        max_context_window: Some(272_000),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: default_input_modalities(),
        used_fallback_model_metadata: true, // this is the fallback model metadata
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

pub fn ensure_required_local_models(models: &mut Vec<ModelInfo>) {
    if !models.iter().any(|model| model.slug == GPT_5_3_CODEX_SPARK) {
        models.push(codex_spark_model_info());
    }
}

pub fn codex_spark_model_info() -> ModelInfo {
    ModelInfo {
        slug: GPT_5_3_CODEX_SPARK.to_string(),
        display_name: "GPT-5.3-Codex-Spark".to_string(),
        description: Some(
            "Text-only research preview model optimized for near-instant coding iteration."
                .to_string(),
        ),
        default_reasoning_level: Some(ReasoningEffort::High),
        supported_reasoning_levels: vec![ReasoningEffortPreset {
            effort: ReasoningEffort::High,
            description: "Greater reasoning depth for coding iteration".to_string(),
        }],
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: false,
        priority: 5,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: fallback_base_instructions_for_slug(GPT_5_3_CODEX_SPARK),
        model_messages: None,
        supports_reasoning_summaries: true,
        default_reasoning_summary: ReasoningSummary::None,
        support_verbosity: true,
        default_verbosity: None,
        apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::tokens(/*limit*/ 10_000),
        supports_parallel_tool_calls: true,
        supports_image_detail_original: false,
        context_window: Some(272_000),
        max_context_window: Some(272_000),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: vec![InputModality::Text],
        used_fallback_model_metadata: false,
        supports_search_tool: true,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

fn fallback_supports_reasoning_summaries(provider_id: Option<&str>) -> bool {
    match provider_id {
        Some(provider_id) => {
            known_provider_models::provider_supports_reasoning_effort(Some(provider_id))
        }
        None => true,
    }
}

fn fallback_base_instructions_for_slug(slug: &str) -> String {
    format!("You are currently running on model `{slug}`.\n\n{BASE_INSTRUCTIONS}")
}

fn local_personality_messages_for_slug(slug: &str) -> Option<ModelMessages> {
    match slug {
        "gpt-5.2-codex" | "exp-codex-personality" => Some(ModelMessages {
            instructions_template: Some(format!(
                "{DEFAULT_PERSONALITY_HEADER}\n\n{PERSONALITY_PLACEHOLDER}\n\n{BASE_INSTRUCTIONS}"
            )),
            instructions_variables: Some(ModelInstructionsVariables {
                personality_default: Some(String::new()),
                personality_friendly: Some(LOCAL_FRIENDLY_TEMPLATE.to_string()),
                personality_pragmatic: Some(LOCAL_PRAGMATIC_TEMPLATE.to_string()),
            }),
        }),
        _ => None,
    }
}

#[cfg(test)]
#[path = "model_info_tests.rs"]
mod tests;
