use std::sync::Arc;

use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelServiceTier;
use codex_app_server_protocol::ModelUpgradeInfo;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_core::ThreadManager;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

const CEREBRAS_PROVIDER_ID: &str = "cerebras";
const NVIDIA_PROVIDER_ID: &str = "nvidia";
const XAI_PROVIDER_ID: &str = "xai";

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
    matches!(
        provider_id,
        CEREBRAS_PROVIDER_ID | NVIDIA_PROVIDER_ID | XAI_PROVIDER_ID
    )
}

pub fn fallback_supported_models_for_provider(
    provider_id: &str,
    include_hidden: bool,
) -> Vec<Model> {
    let models = match provider_id {
        CEREBRAS_PROVIDER_ID => vec![fallback_model(
            "gpt-oss-120b",
            "gpt-oss-120b",
            "Cerebras default model. Requires CEREBRAS_API_KEY for turns.",
            /*is_default*/ true,
        )],
        NVIDIA_PROVIDER_ID => vec![
            fallback_model(
                "openai/gpt-oss-120b",
                "openai/gpt-oss-120b",
                "NVIDIA hosted OpenAI gpt-oss model. Requires NVIDIA_API_KEY for turns.",
                /*is_default*/ true,
            ),
            fallback_model(
                "deepseek-ai/deepseek-v4-flash",
                "deepseek-ai/deepseek-v4-flash",
                "NVIDIA hosted DeepSeek model. Requires NVIDIA_API_KEY for turns.",
                /*is_default*/ false,
            ),
        ],
        XAI_PROVIDER_ID => vec![
            fallback_model(
                "grok-build-0.1",
                "Grok Build 0.1",
                "xAI coding model. Requires XAI_API_KEY for turns.",
                /*is_default*/ true,
            ),
            fallback_model(
                "grok-4.3",
                "Grok 4.3",
                "xAI Grok chat model. Requires XAI_API_KEY for turns.",
                /*is_default*/ false,
            ),
        ],
        _ => Vec::new(),
    };

    if include_hidden {
        models
    } else {
        models.into_iter().filter(|model| !model.hidden).collect()
    }
}

fn fallback_model(id: &str, display_name: &str, description: &str, is_default: bool) -> Model {
    Model {
        id: id.to_string(),
        model: id.to_string(),
        upgrade: None,
        upgrade_info: None,
        availability_nux: None,
        display_name: display_name.to_string(),
        description: description.to_string(),
        hidden: false,
        supported_reasoning_efforts: Vec::new(),
        default_reasoning_effort: ReasoningEffort::None,
        input_modalities: vec![InputModality::Text],
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        is_default,
    }
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
        .iter()
        .map(|preset| ReasoningEffortOption {
            reasoning_effort: preset.effort,
            description: preset.description.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cerebras_fallback_models_include_selectable_default() {
        let models = fallback_supported_models_for_provider(
            CEREBRAS_PROVIDER_ID,
            /*include_hidden*/ false,
        );

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "gpt-oss-120b");
        assert!(models[0].is_default);
        assert!(!models[0].hidden);
    }

    #[test]
    fn xai_fallback_models_include_coding_default() {
        let models =
            fallback_supported_models_for_provider(XAI_PROVIDER_ID, /*include_hidden*/ false);

        assert_eq!(
            models
                .iter()
                .map(|model| (
                    model.model.as_str(),
                    model.display_name.as_str(),
                    model.is_default
                ))
                .collect::<Vec<_>>(),
            vec![
                ("grok-build-0.1", "Grok Build 0.1", true),
                ("grok-4.3", "Grok 4.3", false),
            ]
        );
        assert!(models.iter().all(|model| !model.hidden));
    }
}
