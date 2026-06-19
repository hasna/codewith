use super::DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML;
use super::WORKFLOW_YAML_SYSTEM_PROMPT;

#[test]
fn workflow_prompt_requires_yaml_only_output() {
    assert!(WORKFLOW_YAML_SYSTEM_PROMPT.contains("Output exactly one YAML document"));
    assert!(WORKFLOW_YAML_SYSTEM_PROMPT.contains("Do not include headings, prose"));
    assert!(WORKFLOW_YAML_SYSTEM_PROMPT.contains("Do not output Mermaid"));
    assert!(WORKFLOW_YAML_SYSTEM_PROMPT.contains("Do not output MMD"));
    assert!(WORKFLOW_YAML_SYSTEM_PROMPT.contains("Do not suggest Mermaid"));
    assert!(!WORKFLOW_YAML_SYSTEM_PROMPT.contains("Mermaid or MMD may be implemented later"));
}

#[test]
fn workflow_prompt_requires_deep_adversarial_verified_workflows() {
    for required in [
        "Never emit a shallow task list",
        "Every workflow must include adversarial work",
        "Every implementation or launch path must include deterministic verification",
        "Adversarial review is a required workflow artifact",
        "candidate_succeeded",
        "verifier commands",
        "Verifier exit status or exact machine-checkable output overrides model judgment",
        "succeeded",
    ] {
        assert!(
            WORKFLOW_YAML_SYSTEM_PROMPT.contains(required),
            "missing required prompt fragment: {required}"
        );
    }
}

#[test]
fn workflow_prompt_requires_ancient_agent_names_and_model_routing() {
    for required in [
        "Ancient Greek or Roman",
        "ancient mathematician, scientist, philosopher",
        "model_gateway",
        "provider",
        "model",
        "reasoning",
        "Every agent must include a `model` object",
        "Every model-executed step must include a `model` object",
        "Do not use placeholders",
    ] {
        assert!(
            WORKFLOW_YAML_SYSTEM_PROMPT.contains(required),
            "missing required prompt fragment: {required}"
        );
    }
}

#[test]
fn workflow_prompt_requires_parallel_dag_semantics() {
    for required in [
        "parallel DAG steps",
        "explicit `depends_on`",
        "acyclic graph",
        "Cycles are invalid",
        "Independent ready steps may run concurrently",
        "fan-in reconciliation steps",
    ] {
        assert!(
            WORKFLOW_YAML_SYSTEM_PROMPT.contains(required),
            "missing required prompt fragment: {required}"
        );
    }
}

#[test]
fn dental_example_covers_parallel_dag_model_routing_and_verifiers() {
    for required in [
        "model_gateway:",
        "provider:",
        "model:",
        "reasoning:",
        "parallel_group:",
        "verifiers:",
        "type: \"run_commands\"",
        "candidate_succeeded",
        "depends_on:",
        "Architect-Archimedes",
        "Adversary-Hypatia",
        "Adversary-Cicero",
        "sample_only",
        "synthetic_only",
        "domain_invariants:",
        "negative_cases:",
        "missing_consent",
        "emergency_or_urgent_dental_issue",
        "duplicate_lead",
        "unsupported_location_or_service",
        "health_adjacent_pii",
    ] {
        assert!(
            DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.contains(required),
            "missing required fixture fragment: {required}"
        );
    }
    let model_gateway_count = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML
        .matches("model_gateway:")
        .count();
    assert!(
        model_gateway_count >= 20,
        "expected effective model routing at workflow, agent, and step scopes; found {model_gateway_count}"
    );
}

#[test]
fn dental_example_does_not_use_mermaid() {
    let fixture = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.to_ascii_lowercase();
    assert!(!fixture.contains("mermaid"));
    assert!(!fixture.contains("mmd"));
}
