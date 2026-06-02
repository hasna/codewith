use chrono::DateTime;
use chrono::Utc;
use codex_core::test_support::all_model_presets;
use codex_models_manager::bundled_models_response;
use codex_models_manager::client_version_to_whole;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::default_input_modalities;
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// Convert a ModelPreset to ModelInfo for cache storage.
fn preset_to_info(
    preset: &ModelPreset,
    priority: i32,
    catalog_info: Option<&ModelInfo>,
) -> ModelInfo {
    let mut info = catalog_info.cloned().unwrap_or_else(|| ModelInfo {
        slug: preset.id.clone(),
        display_name: preset.display_name.clone(),
        description: Some(preset.description.clone()),
        default_reasoning_level: Some(preset.default_reasoning_effort),
        supported_reasoning_levels: preset.supported_reasoning_efforts.clone(),
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: if preset.show_in_picker {
            ModelVisibility::List
        } else {
            ModelVisibility::Hide
        },
        supported_in_api: preset.supported_in_api,
        priority,
        additional_speed_tiers: preset.additional_speed_tiers.clone(),
        service_tiers: preset.service_tiers.clone(),
        default_service_tier: preset.default_service_tier.clone(),
        upgrade: preset.upgrade.as_ref().map(Into::into),
        base_instructions: "base instructions".to_string(),
        model_messages: None,
        supports_reasoning_summaries: false,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        availability_nux: preset.availability_nux.clone(),
        apply_patch_tool_type: None,
        web_search_tool_type: Default::default(),
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: Some(272_000),
        max_context_window: None,
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: default_input_modalities(),
        used_fallback_model_metadata: false,
        supports_search_tool: false,
        tool_mode: None,
    });

    info.slug = preset.id.clone();
    info.display_name = preset.display_name.clone();
    info.description = Some(preset.description.clone());
    info.default_reasoning_level = Some(preset.default_reasoning_effort);
    info.supported_reasoning_levels = preset.supported_reasoning_efforts.clone();
    info.visibility = if preset.show_in_picker {
        ModelVisibility::List
    } else {
        ModelVisibility::Hide
    };
    info.supported_in_api = preset.supported_in_api;
    info.priority = priority;
    info.additional_speed_tiers = preset.additional_speed_tiers.clone();
    info.service_tiers = preset.service_tiers.clone();
    info.default_service_tier = preset.default_service_tier.clone();
    info.upgrade = preset.upgrade.as_ref().map(Into::into);
    info.availability_nux = preset.availability_nux.clone();
    info.input_modalities = preset.input_modalities.clone();
    info.used_fallback_model_metadata = false;
    info
}

fn append_mock_model_alias(models: &mut Vec<ModelInfo>) {
    if models.iter().any(|model| model.slug == "mock-model") {
        return;
    }

    let Some(mut mock_model) = models
        .iter()
        .find(|model| model.slug == "gpt-5.3-codex")
        .cloned()
        .or_else(|| models.first().cloned())
    else {
        return;
    };

    mock_model.slug = "mock-model".to_string();
    mock_model.display_name = "mock-model".to_string();
    mock_model.description = Some("Mock model for app-server tests".to_string());
    mock_model.priority = -1;
    mock_model.upgrade = None;
    mock_model.service_tiers.clear();
    mock_model.default_service_tier = None;
    mock_model.availability_nux = None;
    models.push(mock_model);
}

/// Write a models_cache.json file to the codex home directory.
/// This prevents ModelsManager from making network requests to refresh models.
/// The cache will be treated as fresh (within TTL) and used instead of fetching from the network.
/// Uses bundled-catalog-derived presets, converted to ModelInfo format.
pub fn write_models_cache(codex_home: &Path) -> std::io::Result<()> {
    write_models_cache_for_provider(codex_home, "openai")
}

pub fn write_mock_provider_models_cache(codex_home: &Path) -> std::io::Result<()> {
    write_models_cache_for_provider(codex_home, "mock_provider")
}

/// Write a models_cache.json file for a specific provider cache key.
pub fn write_models_cache_for_provider(
    codex_home: &Path,
    provider_cache_key: &str,
) -> std::io::Result<()> {
    let catalog_models = bundled_models_response()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
        .models;
    let catalog_by_slug: HashMap<&str, &ModelInfo> = catalog_models
        .iter()
        .map(|model| (model.slug.as_str(), model))
        .collect();

    // Get a stable bundled-catalog-derived preset list and filter for picker-visible entries.
    let presets: Vec<&ModelPreset> = all_model_presets()
        .iter()
        .filter(|preset| preset.show_in_picker)
        .collect();
    // Convert presets to ModelInfo, assigning priorities (lower = earlier in list).
    // Priority is used for sorting, so the first model gets the lowest priority.
    let models: Vec<ModelInfo> = presets
        .iter()
        .enumerate()
        .map(|(idx, preset)| {
            // Lower priority = earlier in list.
            let priority = idx as i32;
            preset_to_info(
                preset,
                priority,
                catalog_by_slug.get(preset.id.as_str()).copied(),
            )
        })
        .collect();
    let mut models = models;
    if provider_cache_key == "mock_provider" {
        append_mock_model_alias(&mut models);
    }

    write_models_cache_with_models_for_provider(codex_home, models, provider_cache_key)
}

/// Write a models_cache.json file with specific models.
/// Useful when tests need specific models to be available.
pub fn write_models_cache_with_models(
    codex_home: &Path,
    models: Vec<ModelInfo>,
) -> std::io::Result<()> {
    write_models_cache_with_models_for_provider(codex_home, models, "openai")
}

/// Write a models_cache.json file with specific models and provider cache key.
/// Useful when tests need specific models to be available for custom providers.
pub fn write_models_cache_with_models_for_provider(
    codex_home: &Path,
    models: Vec<ModelInfo>,
    provider_cache_key: &str,
) -> std::io::Result<()> {
    let cache_path = codex_home.join("models_cache.json");
    // DateTime<Utc> serializes to RFC3339 format by default with serde
    let fetched_at: DateTime<Utc> = Utc::now();
    let client_version = client_version_to_whole();
    let cache = json!({
        "fetched_at": fetched_at,
        "etag": null,
        "client_version": client_version,
        "provider_cache_key": provider_cache_key,
        "models": models
    });
    std::fs::write(cache_path, serde_json::to_string_pretty(&cache)?)
}
