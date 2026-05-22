use super::*;
use crate::ModelsManagerConfig;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(true),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn fallback_model_instructions_name_selected_model() {
    let model = model_info_from_slug("openrouter/example-model");

    assert!(model.base_instructions.contains("openrouter/example-model"));
    assert!(!model.base_instructions.contains("based on GPT-5"));
}

#[test]
fn personality_template_does_not_claim_gpt_5_base() {
    let model = model_info_from_slug("gpt-5.2-codex");
    let template = model
        .model_messages
        .expect("personality model should have messages")
        .instructions_template
        .expect("personality model should have a template");

    assert!(!template.contains("based on GPT-5"));
    assert!(template.contains("selected model"));
}

#[test]
fn bundled_catalog_instructions_do_not_claim_gpt_5_base() {
    let response = crate::bundled_models_response().expect("bundled catalog should parse");

    for model in response.models {
        assert!(!model.base_instructions.contains("based on GPT-5"));
        if let Some(messages) = model.model_messages
            && let Some(template) = messages.instructions_template
        {
            assert!(!template.contains("based on GPT-5"));
        }
    }
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn model_context_window_override_clamps_to_max_context_window() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig {
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.context_window = Some(400_000);

    assert_eq!(updated, expected);
}

#[test]
fn model_context_window_uses_model_value_without_override() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig::default();

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}
