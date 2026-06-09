use std::sync::Arc;

use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelServiceTier;
use codex_app_server_protocol::ModelUpgradeInfo;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_core::ThreadManager;
use codex_known_provider_models as known_provider_models;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

pub async fn supported_models(
    thread_manager: Arc<ThreadManager>,
    include_hidden: bool,
) -> Vec<Model> {
    thread_manager
        .list_models(RefreshStrategy::OnlineIfUncached)
        .await
        .into_iter()
        .filter(|preset| include_hidden || preset.show_in_picker)
        .map(model_from_preset)
        .collect()
}

pub async fn try_supported_models_from_manager(
    models_manager: SharedModelsManager,
    include_hidden: bool,
) -> CoreResult<Vec<Model>> {
    Ok(models_manager
        .list_models_result(RefreshStrategy::OnlineIfUncached)
        .await?
        .into_iter()
        .filter(|preset| include_hidden || preset.show_in_picker)
        .map(model_from_preset)
        .collect())
}

pub async fn supported_models_from_manager(
    models_manager: SharedModelsManager,
    include_hidden: bool,
) -> Vec<Model> {
    models_manager
        .list_models(RefreshStrategy::OnlineIfUncached)
        .await
        .into_iter()
        .filter(|preset| include_hidden || preset.show_in_picker)
        .map(model_from_preset)
        .collect()
}

pub fn provider_has_fallback_models(provider_id: &str) -> bool {
    !known_provider_models::fallback_models_for_provider(provider_id).is_empty()
}

pub fn fallback_supported_models_for_provider(
    provider_id: &str,
    include_hidden: bool,
) -> Vec<Model> {
    let models = known_provider_models::fallback_models_for_provider(provider_id)
        .iter()
        .map(|model| fallback_model(provider_id, model))
        .collect::<Vec<_>>();

    if include_hidden {
        models
    } else {
        models.into_iter().filter(|model| !model.hidden).collect()
    }
}

fn fallback_model(
    provider_id: &str,
    model: &known_provider_models::KnownProviderFallbackModel,
) -> Model {
    let (default_reasoning_effort, supported_reasoning_efforts) =
        fallback_reasoning_efforts(provider_id, model.id);
    Model {
        id: model.id.to_string(),
        model: model.id.to_string(),
        upgrade: None,
        upgrade_info: None,
        availability_nux: None,
        display_name: model.display_name.to_string(),
        description: model.description.to_string(),
        hidden: false,
        supported_reasoning_efforts,
        default_reasoning_effort,
        input_modalities: vec![InputModality::Text],
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        is_default: model.is_default,
    }
}

fn fallback_reasoning_efforts(
    provider_id: &str,
    id: &str,
) -> (ReasoningEffort, Vec<ReasoningEffortOption>) {
    let (default_effort, presets) =
        known_provider_models::reasoning_levels_for_local_fallback(Some(provider_id), id);
    (
        default_effort.unwrap_or(ReasoningEffort::None),
        reasoning_efforts_from_preset(presets),
    )
}

fn model_from_preset(preset: ModelPreset) -> Model {
    Model {
        id: preset.id.to_string(),
        model: preset.model.to_string(),
        upgrade: preset.upgrade.as_ref().map(|upgrade| upgrade.id.clone()),
        upgrade_info: preset.upgrade.as_ref().map(|upgrade| ModelUpgradeInfo {
            model: upgrade.id.clone(),
            upgrade_copy: upgrade.upgrade_copy.clone(),
            model_link: upgrade.model_link.clone(),
            migration_markdown: upgrade.migration_markdown.clone(),
        }),
        availability_nux: preset.availability_nux.map(Into::into),
        display_name: preset.display_name.to_string(),
        description: preset.description.to_string(),
        hidden: !preset.show_in_picker,
        supported_reasoning_efforts: reasoning_efforts_from_preset(
            preset.supported_reasoning_efforts,
        ),
        default_reasoning_effort: preset.default_reasoning_effort,
        input_modalities: preset.input_modalities,
        supports_personality: preset.supports_personality,
        additional_speed_tiers: preset.additional_speed_tiers,
        service_tiers: preset
            .service_tiers
            .into_iter()
            .map(|service_tier| ModelServiceTier {
                id: service_tier.id,
                name: service_tier.name,
                description: service_tier.description,
            })
            .collect(),
        default_service_tier: preset.default_service_tier,
        is_default: preset.is_default,
    }
}

fn reasoning_efforts_from_preset(
    efforts: Vec<ReasoningEffortPreset>,
) -> Vec<ReasoningEffortOption> {
    efforts
        .into_iter()
        .map(|preset| ReasoningEffortOption {
            reasoning_effort: preset.effort,
            description: preset.description,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cerebras_fallback_models_include_selectable_default() {
        let models =
            fallback_supported_models_for_provider("cerebras", /*include_hidden*/ false);

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model, "gpt-oss-120b");
        assert_eq!(models[0].display_name, "OpenAI GPT OSS 120B");
        assert!(models[0].is_default);
        assert!(!models[0].hidden);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::Medium);
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|effort| effort.reasoning_effort.clone())
                .collect::<Vec<_>>(),
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        );
        assert_eq!(models[1].model, "zai-glm-4.7");
        assert_eq!(models[1].display_name, "Z.ai GLM 4.7");
        assert!(!models[1].is_default);
        assert_eq!(models[1].default_reasoning_effort, ReasoningEffort::Medium);
    }

    #[test]
    fn nvidia_fallback_models_include_valid_live_smoke_models() {
        let models =
            fallback_supported_models_for_provider("nvidia", /*include_hidden*/ false);

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model, "openai/gpt-oss-120b");
        assert_eq!(models[0].display_name, "OpenAI GPT OSS 120B");
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::Medium);
        assert_eq!(models[1].model, "z-ai/glm-5.1");
        assert_eq!(models[1].display_name, "Z.ai GLM 5.1");
        assert!(!models[1].is_default);
        assert_eq!(models[1].default_reasoning_effort, ReasoningEffort::None);
    }

    #[test]
    fn openrouter_fallback_models_include_valid_deepseek_and_glm() {
        let models =
            fallback_supported_models_for_provider("openrouter", /*include_hidden*/ false);

        assert_eq!(models.len(), 4);
        assert_eq!(models[0].model, "openai/gpt-oss-120b");
        assert_eq!(models[0].display_name, "OpenAI GPT OSS 120B");
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::None);
        assert_eq!(models[1].model, "deepseek/deepseek-v4-flash");
        assert_eq!(models[1].display_name, "DeepSeek V4 Flash");
        assert!(!models[1].is_default);
        assert_eq!(models[2].model, "z-ai/glm-4.7");
        assert_eq!(models[2].display_name, "Z.ai GLM 4.7");
        assert!(!models[2].is_default);
        assert_eq!(models[3].model, "z-ai/glm-5.1");
        assert_eq!(models[3].display_name, "Z.ai GLM 5.1");
        assert!(!models[3].is_default);
    }

    #[test]
    fn xiaomi_fallback_models_include_ultraspeed_default() {
        let models =
            fallback_supported_models_for_provider("xiaomi", /*include_hidden*/ false);

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "mimo-v2.5-pro-ultraspeed");
        assert_eq!(models[0].display_name, "MiMo V2.5 Pro UltraSpeed");
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::None);
    }
}
