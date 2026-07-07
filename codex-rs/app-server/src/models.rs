use std::sync::Arc;

use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelGatewayKind;
use codex_app_server_protocol::ModelServiceTier;
use codex_app_server_protocol::ModelUpgradeInfo;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_core::ThreadManager;
use codex_known_provider_models as known_provider_models;
use codex_model_provider_info::HASNA_GATEWAY_NAME;
use codex_model_provider_info::ModelGatewayFamily;
use codex_model_provider_info::model_gateway_family;
use codex_model_provider_info::model_gateway_for_provider;
use codex_model_provider_info::model_gateway_name;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

pub use codex_known_provider_models::provider_for_fallback_model;

pub async fn supported_models(
    thread_manager: Arc<ThreadManager>,
    include_hidden: bool,
    provider_id: &str,
) -> Vec<Model> {
    thread_manager
        .list_models(RefreshStrategy::OnlineIfUncached)
        .await
        .into_iter()
        .filter(|preset| include_hidden || preset.show_in_picker)
        .map(|preset| model_from_preset(provider_id, preset))
        .collect()
}

pub async fn try_supported_models_from_manager(
    models_manager: SharedModelsManager,
    include_hidden: bool,
    provider_id: &str,
) -> CoreResult<Vec<Model>> {
    Ok(models_manager
        .list_models_result(RefreshStrategy::OnlineIfUncached)
        .await?
        .into_iter()
        .filter(|preset| include_hidden || preset.show_in_picker)
        .map(|preset| model_from_preset(provider_id, preset))
        .collect())
}

pub async fn supported_models_from_manager(
    models_manager: SharedModelsManager,
    include_hidden: bool,
    provider_id: &str,
) -> Vec<Model> {
    models_manager
        .list_models(RefreshStrategy::OnlineIfUncached)
        .await
        .into_iter()
        .filter(|preset| include_hidden || preset.show_in_picker)
        .map(|preset| model_from_preset(provider_id, preset))
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
    let route = model_route_fields(provider_id, model.id);
    Model {
        id: model.id.to_string(),
        model: model.id.to_string(),
        model_provider: provider_id.to_string(),
        model_gateway: route.gateway_id,
        model_gateway_name: route.gateway_name,
        model_gateway_kind: route.gateway_kind,
        upstream_provider: route.upstream_provider,
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

fn model_from_preset(provider_id: &str, preset: ModelPreset) -> Model {
    let route = model_route_fields(provider_id, &preset.model);
    Model {
        id: preset.id.to_string(),
        model: preset.model.to_string(),
        model_provider: provider_id.to_string(),
        model_gateway: route.gateway_id,
        model_gateway_name: route.gateway_name,
        model_gateway_kind: route.gateway_kind,
        upstream_provider: route.upstream_provider,
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

struct ModelRouteFields {
    gateway_id: String,
    gateway_name: String,
    gateway_kind: ModelGatewayKind,
    upstream_provider: Option<String>,
}

fn model_route_fields(provider_id: &str, model_id: &str) -> ModelRouteFields {
    let gateway_id = model_gateway_for_provider(provider_id);
    let gateway_name = model_gateway_name(gateway_id).unwrap_or(HASNA_GATEWAY_NAME);
    let gateway_kind = model_gateway_family(gateway_id)
        .map(api_gateway_kind)
        .unwrap_or(ModelGatewayKind::Direct);
    ModelRouteFields {
        gateway_id: gateway_id.to_string(),
        gateway_name: gateway_name.to_string(),
        gateway_kind,
        upstream_provider: upstream_provider_from_model_id(model_id),
    }
}

fn api_gateway_kind(family: ModelGatewayFamily) -> ModelGatewayKind {
    match family {
        ModelGatewayFamily::Direct => ModelGatewayKind::Direct,
        ModelGatewayFamily::Aggregator => ModelGatewayKind::Aggregator,
    }
}

fn upstream_provider_from_model_id(model_id: &str) -> Option<String> {
    let (provider, _) = model_id.split_once('/')?;
    if provider.is_empty() {
        None
    } else {
        Some(provider.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cerebras_fallback_models_include_selectable_default() {
        let models =
            fallback_supported_models_for_provider("cerebras", /*include_hidden*/ false);

        assert_eq!(models.len(), 3);
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
        assert_eq!(models[2].model, "gemma-4-31b");
        assert_eq!(models[2].display_name, "Gemma 4 31B");
        assert!(!models[2].is_default);
    }

    #[test]
    fn nvidia_fallback_models_include_valid_live_smoke_models() {
        let models =
            fallback_supported_models_for_provider("nvidia", /*include_hidden*/ false);

        assert_eq!(models.len(), 3);
        assert_eq!(models[0].model, "nvidia/nemotron-3-ultra-550b-a55b");
        assert_eq!(models[0].display_name, "NVIDIA Nemotron 3 Ultra");
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::None);
        assert_eq!(models[1].model, "openai/gpt-oss-120b");
        assert_eq!(models[1].display_name, "OpenAI GPT OSS 120B");
        assert!(!models[1].is_default);
        assert_eq!(models[1].default_reasoning_effort, ReasoningEffort::Medium);
        assert_eq!(models[2].model, "z-ai/glm-5.2");
        assert_eq!(models[2].display_name, "Z.ai GLM 5.2");
        assert!(!models[2].is_default);
        assert_eq!(models[2].default_reasoning_effort, ReasoningEffort::None);
    }

    #[test]
    fn anthropic_fallback_models_include_fable_default() {
        let models =
            fallback_supported_models_for_provider("anthropic", /*include_hidden*/ false);

        assert_eq!(models.len(), 5);
        assert_eq!(models[0].model, "claude-fable-5");
        assert_eq!(models[0].display_name, "Claude Fable 5");
        assert!(models[0].is_default);
        assert!(!models[0].hidden);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::None);
        assert_eq!(models[0].supported_reasoning_efforts, Vec::new());
        assert_eq!(models[1].model, "claude-opus-4-8");
        assert_eq!(models[2].model, "claude-sonnet-5");
        assert_eq!(models[3].model, "claude-sonnet-4-6");
        assert_eq!(models[4].model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn openrouter_fallback_models_include_current_gateway_defaults() {
        let models =
            fallback_supported_models_for_provider("openrouter", /*include_hidden*/ false);

        assert_eq!(models.len(), 9);
        assert_eq!(models[0].model, "z-ai/glm-5.2");
        assert_eq!(models[0].display_name, "Z.ai GLM 5.2");
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::High);
        assert_eq!(models[1].model, "openai/gpt-oss-120b");
        assert_eq!(models[1].display_name, "OpenAI GPT OSS 120B");
        assert!(!models[1].is_default);
        assert_eq!(models[2].model, "deepseek/deepseek-v4-flash");
        assert_eq!(models[2].display_name, "DeepSeek V4 Flash");
        assert!(!models[2].is_default);
        assert_eq!(models[3].model, "z-ai/glm-4.7");
        assert_eq!(models[3].display_name, "Z.ai GLM 4.7");
        assert!(!models[3].is_default);
        assert_eq!(models[4].model, "z-ai/glm-5.1");
        assert_eq!(models[5].model, "qwen/qwen3.7-plus");
        assert_eq!(models[6].model, "x-ai/grok-4.20");
        assert_eq!(models[7].model, "xiaomi/mimo-v2.5-pro");
        assert_eq!(models[8].model, "nvidia/nemotron-3-ultra-550b-a55b");
    }

    #[test]
    fn zai_fallback_models_expose_glm_5_2_reasoning_efforts() {
        let models = fallback_supported_models_for_provider("zai", /*include_hidden*/ false);

        assert_eq!(models[0].model, "glm-5.2");
        assert_eq!(models[0].display_name, "GLM-5.2");
        assert!(models[0].is_default);
        assert!(!models[0].hidden);
        // Direct Z.ai docs default GLM-5.2 to `reasoning_effort = max`, represented via
        // `Custom("max")` since the enum has no first-class `Max` variant.
        assert_eq!(
            models[0].default_reasoning_effort,
            ReasoningEffort::Custom("max".to_string())
        );
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|effort| effort.reasoning_effort.clone())
                .collect::<Vec<_>>(),
            vec![
                ReasoningEffort::None,
                ReasoningEffort::Minimal,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Custom("max".to_string()),
            ]
        );

        // Other GLM fallbacks use Z.ai's provider-specific `thinking` parameter rather than
        // OpenAI-compatible `reasoning_effort`, so the fallback list should not advertise
        // selectable reasoning efforts for them.
        let glm_5_1 = models
            .iter()
            .find(|model| model.model == "glm-5.1")
            .expect("glm-5.1 fallback should exist");
        assert_eq!(glm_5_1.default_reasoning_effort, ReasoningEffort::None);
        assert_eq!(glm_5_1.supported_reasoning_efforts, Vec::new());
    }

    #[test]
    fn xiaomi_fallback_models_include_current_default_and_legacy_alias() {
        let models =
            fallback_supported_models_for_provider("xiaomi", /*include_hidden*/ false);

        assert_eq!(models.len(), 3);
        assert_eq!(models[0].model, "mimo-v2.5-pro");
        assert_eq!(models[0].display_name, "MiMo V2.5 Pro");
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort, ReasoningEffort::None);
        assert_eq!(models[1].model, "mimo-v2.5");
        assert!(!models[1].is_default);
        assert_eq!(models[2].model, "mimo-v2.5-pro-ultraspeed");
        assert_eq!(models[2].display_name, "MiMo V2.5 Pro UltraSpeed");
        assert!(!models[2].is_default);
    }
}
