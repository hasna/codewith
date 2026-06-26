use super::*;
use crate::ModelsManagerConfig;
use pretty_assertions::assert_eq;

fn assert_effort_estimate_guidance(text: &str, label: &str) {
    for expected in [
        "## Effort estimates",
        "`Human time`",
        "`AI-agent time`",
        "provider/model",
        "50 output tokens/sec",
        "wall-clock delivery time",
    ] {
        assert!(
            text.contains(expected),
            "{label} missing effort estimate guidance string {expected:?}"
        );
    }
}

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
    assert_effort_estimate_guidance(&model.base_instructions, "fallback base instructions");
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
            .map(|preset| preset.effort.clone())
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
fn codex_spark_uses_text_only_local_metadata() {
    let model = model_info_from_slug(GPT_5_3_CODEX_SPARK);

    assert_eq!(model.slug, GPT_5_3_CODEX_SPARK);
    assert_eq!(model.display_name, "GPT-5.3-Codex-Spark");
    assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::High));
    assert_eq!(
        model.supported_reasoning_levels,
        vec![ReasoningEffortPreset {
            effort: ReasoningEffort::High,
            description: "Greater reasoning depth for coding iteration".to_string(),
        }]
    );
    assert_eq!(model.input_modalities, vec![InputModality::Text]);
    assert!(!model.supported_in_api);
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
fn known_anthropic_fable_model_uses_local_metadata() {
    let model = model_info_from_slug_for_provider("claude-fable-5", Some("anthropic"));

    assert_eq!(model.display_name, "Claude Fable 5");
    assert_eq!(model.context_window, Some(1_000_000));
    assert_eq!(model.max_context_window, Some(1_000_000));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(model.supports_parallel_tool_calls);
    assert!(!model.supports_reasoning_summaries);
    assert_eq!(model.default_reasoning_level, None);
    assert_eq!(model.supported_reasoning_levels, Vec::new());
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_anthropic_latest_models_have_context_windows() {
    let cases = [
        ("claude-opus-4-8", "Claude Opus 4.8", 1_000_000),
        ("claude-sonnet-4-6", "Claude Sonnet 4.6", 1_000_000),
        ("claude-haiku-4-5-20251001", "Claude Haiku 4.5", 200_000),
        ("claude-haiku-4-5", "Claude Haiku 4.5", 200_000),
    ];

    for (slug, display_name, context_window) in cases {
        let model = model_info_from_slug_for_provider(slug, Some("anthropic"));

        assert_eq!(model.display_name, display_name);
        assert_eq!(model.context_window, Some(context_window));
        assert_eq!(model.max_context_window, Some(context_window));
        assert!(!model.used_fallback_model_metadata);
    }
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
fn known_xiaomi_ultraspeed_model_uses_local_metadata() {
    let model = model_info_from_slug_for_provider("mimo-v2.5-pro-ultraspeed", Some("xiaomi"));

    assert_eq!(model.display_name, "MiMo V2.5 Pro UltraSpeed");
    assert_eq!(model.context_window, Some(1_048_576));
    assert_eq!(model.max_context_window, Some(1_048_576));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(!model.supports_parallel_tool_calls);
    assert_eq!(model.default_reasoning_level, None);
    assert!(model.supported_reasoning_levels.is_empty());
    assert!(!model.supports_reasoning_summaries);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_qwen_model_uses_local_metadata_with_search_support() {
    let model = model_info_from_slug_for_provider("qwen3.5-flash", Some("qwen"));

    assert_eq!(model.display_name, "Qwen3.5 Flash");
    assert_eq!(model.context_window, Some(1_000_000));
    assert_eq!(model.max_context_window, Some(1_000_000));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(!model.supports_parallel_tool_calls);
    assert!(model.supports_search_tool);
    assert!(!model.used_fallback_model_metadata);
}

#[test]
fn known_zai_model_uses_local_metadata_with_reasoning_and_search_support() {
    let model = model_info_from_slug_for_provider("glm-5.2", Some("zai"));

    assert_eq!(model.display_name, "GLM-5.2");
    assert_eq!(model.context_window, Some(1_000_000));
    assert_eq!(model.max_context_window, Some(1_000_000));
    assert_eq!(model.experimental_supported_tools, vec!["tools"]);
    assert!(!model.supports_parallel_tool_calls);
    assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::Medium));
    assert!(!model.supported_reasoning_levels.is_empty());
    assert!(model.supports_reasoning_summaries);
    assert!(model.supports_search_tool);
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
    assert_effort_estimate_guidance(&template, "personality template");
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
fn bundled_catalog_instructions_include_effort_estimate_guidance() {
    let response = crate::bundled_models_response().expect("bundled catalog should parse");

    for model in response.models {
        assert_effort_estimate_guidance(
            &model.base_instructions,
            &format!("{} base instructions", model.slug),
        );

        if let Some(messages) = model.model_messages
            && let Some(template) = messages.instructions_template
        {
            assert_effort_estimate_guidance(&template, &format!("{} template", model.slug));
        }
    }
}

#[test]
fn bundled_openai_gpt_5_5_uses_endpoint_context_window() {
    let response = crate::bundled_models_response().expect("bundled catalog should parse");
    let model = response
        .models
        .into_iter()
        .find(|model| model.slug == GPT_5_5_MODEL_ID)
        .expect("bundled catalog should include GPT-5.5");

    assert_eq!(model.context_window, Some(GPT_5_5_OPENAI_CONTEXT_WINDOW));
    assert_eq!(
        model.max_context_window,
        Some(GPT_5_5_OPENAI_CONTEXT_WINDOW)
    );
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
fn openai_gpt_5_5_context_window_is_capped_after_overrides() {
    let mut model = model_info_from_slug("unknown-model");
    model.slug = GPT_5_5_MODEL_ID.to_string();
    model.context_window = Some(272_000);
    model.max_context_window = Some(272_000);
    let config = ModelsManagerConfig {
        model_provider_id: Some(OPENAI_MODEL_PROVIDER_ID.to_string()),
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model, &config);

    assert_eq!(updated.context_window, Some(GPT_5_5_OPENAI_CONTEXT_WINDOW));
    assert_eq!(
        updated.max_context_window,
        Some(GPT_5_5_OPENAI_CONTEXT_WINDOW)
    );
}

#[test]
fn gpt_5_5_context_cap_does_not_apply_to_other_providers() {
    let mut model = model_info_from_slug("unknown-model");
    model.slug = GPT_5_5_MODEL_ID.to_string();
    model.context_window = Some(272_000);
    model.max_context_window = Some(272_000);
    let config = ModelsManagerConfig {
        model_provider_id: Some("openrouter".to_string()),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
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
