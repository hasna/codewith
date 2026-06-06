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
fn known_provider_model_uses_local_metadata() {
    let model = model_info_from_slug("gpt-oss-120b");

    assert_eq!(model.display_name, "OpenAI GPT OSS 120B");
    assert_eq!(model.context_window, Some(131_072));
    assert_eq!(model.max_context_window, Some(131_072));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(!model.supports_parallel_tool_calls);
    assert!(model.supports_reasoning_summaries);
    assert_eq!(
        model
            .supported_reasoning_levels
            .iter()
            .map(|preset| preset.effort)
            .collect::<Vec<_>>(),
        vec![
            codex_protocol::openai_models::ReasoningEffort::Low,
            codex_protocol::openai_models::ReasoningEffort::Medium,
            codex_protocol::openai_models::ReasoningEffort::High,
        ]
    );
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_provider_glm_model_uses_local_metadata() {
    let model = model_info_from_slug("zai-glm-4.7");

    assert_eq!(model.display_name, "Z.ai GLM 4.7");
    assert_eq!(model.context_window, Some(131_072));
    assert_eq!(model.max_context_window, Some(131_072));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(model.supports_parallel_tool_calls);
    assert!(model.supports_reasoning_summaries);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_nvidia_deepseek_model_uses_local_metadata() {
    let model = model_info_from_slug_for_provider("deepseek-ai/deepseek-v4-flash", Some("nvidia"));

    assert_eq!(model.display_name, "DeepSeek V4 Flash");
    assert_eq!(model.context_window, Some(1_048_576));
    assert_eq!(model.max_context_window, Some(1_048_576));
    assert_eq!(model.experimental_supported_tools, Vec::<String>::new());
    assert!(!model.supports_parallel_tool_calls);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_nvidia_glm_model_uses_local_metadata() {
    let model = model_info_from_slug_for_provider("z-ai/glm-5.1", Some("nvidia"));

    assert_eq!(model.display_name, "Z.ai GLM 5.1");
    assert_eq!(model.context_window, Some(131_072));
    assert_eq!(model.max_context_window, Some(131_072));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(!model.supports_parallel_tool_calls);
    assert_eq!(model.default_reasoning_level, None);
    assert!(model.supported_reasoning_levels.is_empty());
    assert!(!model.supports_reasoning_summaries);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn stale_openrouter_deepseek_slug_uses_unknown_fallback() {
    let model =
        model_info_from_slug_for_provider("deepseek-ai/deepseek-v4-flash", Some("openrouter"));

    assert_eq!(model.display_name, "deepseek-ai/deepseek-v4-flash");
    assert_eq!(model.context_window, Some(272_000));
    assert_eq!(model.experimental_supported_tools, Vec::<String>::new());
    assert!(model.used_fallback_model_metadata);
}

#[test]
fn known_openrouter_deepseek_model_uses_local_metadata() {
    let model = model_info_from_slug_for_provider("deepseek/deepseek-v4-flash", Some("openrouter"));

    assert_eq!(model.display_name, "DeepSeek V4 Flash");
    assert_eq!(model.context_window, Some(1_048_576));
    assert_eq!(model.max_context_window, Some(1_048_576));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(!model.supports_parallel_tool_calls);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_openrouter_glm_5_1_model_uses_local_metadata() {
    let model = model_info_from_slug_for_provider("z-ai/glm-5.1", Some("openrouter"));

    assert_eq!(model.display_name, "Z.ai GLM 5.1");
    assert_eq!(model.context_window, Some(202_752));
    assert_eq!(model.max_context_window, Some(202_752));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(model.supports_parallel_tool_calls);
    assert_eq!(model.default_reasoning_level, None);
    assert!(model.supported_reasoning_levels.is_empty());
    assert!(!model.supports_reasoning_summaries);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_openrouter_reasoning_model_does_not_advertise_reasoning_effort() {
    let model = model_info_from_slug_for_provider("z-ai/glm-4.7", Some("openrouter"));

    assert_eq!(model.display_name, "Z.ai GLM 4.7");
    assert_eq!(model.context_window, Some(202_752));
    assert_eq!(model.default_reasoning_level, None);
    assert_eq!(model.supported_reasoning_levels, Vec::new());
    assert!(!model.supports_reasoning_summaries);
    assert!(!model.used_fallback_model_metadata);
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
        assert!(!model.base_instructions.contains("You are Codex"));
        if let Some(messages) = model.model_messages
            && let Some(template) = messages.instructions_template
        {
            assert!(!template.contains("based on GPT-5"));
            assert!(!template.contains("You are Codex"));
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
