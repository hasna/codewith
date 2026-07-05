use codex_prompts::DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML;
use pretty_assertions::assert_eq;

use crate::MAX_WORKFLOW_PROMPT_FIELD_CHARS;
use crate::WorkflowBranchPrompt;
use crate::WorkflowLoopIntervalUnit;
use crate::WorkflowLoopSchedule;
use crate::WorkflowModelRouter;
use crate::WorkflowModelRoutingCapability;
use crate::WorkflowModelRoutingDecisionStatus;
use crate::WorkflowSpecError;
use crate::WorkflowStatus;
use crate::WorkflowStopCondition;
use crate::parse_workflow_yaml;
use crate::render_workflow_branch_prompt;

fn yaml_key<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    let key = serde_yaml::Value::String(key.to_string());
    value.as_mapping()?.get(&key)
}

fn workflow_with_execution_defaults_routing(routing_yaml: &str) -> String {
    insert_execution_defaults_routing(DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML, routing_yaml)
}

fn insert_execution_defaults_routing(workflow_yaml: &str, routing_yaml: &str) -> String {
    let workflow_yaml = workflow_yaml.replace("\r\n", "\n");
    let routing_yaml = routing_yaml.replace("\r\n", "\n");
    let with_routing = workflow_yaml.replacen(
        "  permission_profile: \"workspace-write\"\nlimits:",
        &format!("  permission_profile: \"workspace-write\"\n{routing_yaml}limits:"),
        1,
    );
    if with_routing == workflow_yaml {
        panic!("workflow fixture must include execution_defaults.permission_profile before limits");
    }
    with_routing
}

#[test]
fn injects_execution_defaults_routing_from_crlf_fixture() {
    let workflow_yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace('\n', "\r\n");
    let routing_yaml = r#"  routing:
    contract_version: "open-router.codewith/v0"
    router: "open_router"
    request:
      requested_capability: "planning"
"#
    .replace('\n', "\r\n");

    let yaml = insert_execution_defaults_routing(&workflow_yaml, &routing_yaml);
    let spec = parse_workflow_yaml(&yaml).expect("CRLF fixture routing contract should parse");

    let routing = spec
        .execution_defaults
        .routing
        .expect("execution defaults should include injected routing contract");
    assert_eq!("open-router.codewith/v0", routing.contract_version);
    assert_eq!(WorkflowModelRouter::OpenRouter, routing.router);
    assert_eq!(
        WorkflowModelRoutingCapability::Planning,
        routing.request.requested_capability
    );
}

#[test]
fn parses_dental_lead_saas_fixture() {
    let spec = parse_workflow_yaml(DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML)
        .expect("dental workflow fixture should parse");

    assert_eq!("workflow.codex.codewith/v0", spec.schema_version);
    assert_eq!("wf_dental_lead_saas_launch", spec.workflow_id);
    assert_eq!(WorkflowStatus::Draft, spec.status);
    assert_eq!("hasna", spec.execution_defaults.model_gateway);
    assert_eq!("openai", spec.execution_defaults.provider);
    assert_eq!("gpt-5.4", spec.execution_defaults.model);
    assert_eq!("high", spec.execution_defaults.reasoning);
    let fixture = spec
        .fixture
        .as_ref()
        .expect("fixture metadata should parse");
    assert_eq!("sample_only", fixture.purpose);
    assert!(!fixture.executable);
    assert_eq!("synthetic_only", fixture.data);
    assert_eq!(None, fixture.generated_at);
    let invariants = spec
        .domain_invariants
        .as_ref()
        .expect("domain invariants should parse");
    assert_eq!(
        Some("health_adjacent_pii"),
        yaml_key(invariants, "pii_handling")
            .and_then(|pii| yaml_key(pii, "classification"))
            .and_then(serde_yaml::Value::as_str)
    );
    assert_eq!(
        Some("redact_contact_details"),
        yaml_key(invariants, "pii_handling")
            .and_then(|pii| yaml_key(pii, "logging"))
            .and_then(serde_yaml::Value::as_str)
    );
    let negative_case_ids = yaml_key(invariants, "negative_cases")
        .and_then(serde_yaml::Value::as_sequence)
        .expect("negative cases should parse")
        .iter()
        .filter_map(|case| yaml_key(case, "id").and_then(serde_yaml::Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        vec![
            "missing_consent",
            "emergency_or_urgent_dental_issue",
            "duplicate_lead",
            "incomplete_contact_info",
            "unsupported_location_or_service",
        ],
        negative_case_ids
    );
    assert!(spec.agents.len() >= 6);
    assert!(spec.steps.len() >= 12);
    assert_eq!(1, spec.loops.len());
    assert_eq!("continuous_quality_gate", spec.loops[0].id);
    assert_eq!(
        "merge_and_reconcile",
        spec.loops[0].trigger_step.as_deref().unwrap()
    );
    assert_eq!(144, spec.loops[0].max_iterations);
    assert_eq!(Some(86_400), spec.loops[0].expires_after_seconds);
    assert_eq!(
        WorkflowLoopSchedule::Interval {
            amount: 5,
            unit: WorkflowLoopIntervalUnit::Minutes,
        },
        spec.loops[0].schedule
    );
    assert_eq!(
        WorkflowStopCondition::StepSucceeded {
            step: "launch_gate".to_string()
        },
        spec.loops[0].stop_condition
    );
    assert_eq!(1, spec.monitors.len());
    assert_eq!("ci_signal_observer", spec.monitors[0].id);
    assert_eq!("existing_thread_monitor", spec.monitors[0].source);
    assert_eq!(
        "build_foundation",
        spec.monitors[0].trigger_step.as_deref().unwrap()
    );
    assert_eq!(10, spec.monitors[0].max_events_per_tick);
    assert_eq!(
        Some(&WorkflowStopCondition::WorkflowComplete),
        spec.monitors[0].stop_condition.as_ref()
    );
    assert!(
        spec.agents
            .iter()
            .any(|agent| agent.display_name == "Adversary-Hypatia")
            && spec
                .agents
                .iter()
                .any(|agent| agent.display_name == "Adversary-Cicero")
    );
    assert!(
        spec.artifacts
            .required
            .contains(&"launch_evidence.md".to_string())
    );
    let marketplace = spec
        .steps
        .iter()
        .find(|step| step.id == "build_dentist_marketplace")
        .expect("marketplace step should exist");
    assert_eq!("gpt-5.5", marketplace.model.as_ref().unwrap().model);
    assert!(
        spec.steps
            .iter()
            .any(|step| step.parallel_group.as_deref() == Some("implementation"))
    );
    assert!(
        spec.steps.iter().any(|step| step
            .completion
            .as_ref()
            .is_some_and(|completion| completion.verifiers.iter().any(|verifier| {
                verifier.kind == "run_commands" && verifier.commands.len() > 1
            })))
    );
    let full_suite = spec
        .steps
        .iter()
        .flat_map(|step| {
            step.completion
                .iter()
                .flat_map(|completion| completion.verifiers.iter())
        })
        .find(|verifier| verifier.id == "full_test_suite")
        .expect("full suite verifier should exist");
    assert_eq!(
        vec![
            "npm run typecheck".to_string(),
            "npm run lint".to_string(),
            "npm test".to_string(),
            "npm run build".to_string(),
        ],
        full_suite.commands
    );
}

#[test]
fn rejects_invalid_workflow_loop_trigger_step() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
        "trigger_step: \"merge_and_reconcile\"",
        "trigger_step: \"missing_step\"",
    );

    let err = parse_workflow_yaml(&yaml).expect_err("invalid loop trigger should be rejected");

    assert!(
        err.to_string()
            .contains("loop.trigger_step references unknown step `missing_step`"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_zero_interval_workflow_loop() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace("amount: 5", "amount: 0");

    let err = parse_workflow_yaml(&yaml).expect_err("zero interval should be rejected");

    assert!(
        err.to_string().contains("interval amount must be positive"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_duplicate_workflow_monitor_ids() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
        "    max_events_per_tick: 10",
        "    max_events_per_tick: 10\n  - id: \"ci_signal_observer\"\n    title: \"Duplicate monitor id\"\n    source: \"existing_thread_monitor\"\n    max_events_per_tick: 5",
    );

    let err = parse_workflow_yaml(&yaml).expect_err("duplicate monitor id should be rejected");

    assert!(
        err.to_string()
            .contains("monitor id `ci_signal_observer` is duplicated"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_invalid_workflow_monitor_source() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
        "source: \"existing_thread_monitor\"",
        "source: \"shell_command\"",
    );

    let err = parse_workflow_yaml(&yaml).expect_err("invalid monitor source should be rejected");

    assert!(
        err.to_string()
            .contains("source must be `existing_thread_monitor`"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_markdown_fenced_yaml() {
    let err = parse_workflow_yaml(&format!(
        "```yaml\n{DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML}\n```"
    ))
    .expect_err("Markdown fences should be rejected");

    assert_eq!(WorkflowSpecError::MarkdownFence, err);
}

#[test]
fn rejects_oversized_yaml_before_parsing() {
    let yaml = "a".repeat(crate::MAX_WORKFLOW_YAML_BYTES + 1);
    let err = parse_workflow_yaml(&yaml).expect_err("oversized YAML should be rejected");

    assert_eq!(
        WorkflowSpecError::DocumentTooLarge {
            bytes: crate::MAX_WORKFLOW_YAML_BYTES + 1,
            max_bytes: crate::MAX_WORKFLOW_YAML_BYTES,
        },
        err
    );
}

#[test]
fn rejects_duplicate_yaml_keys() {
    let yaml = r#"
schema_version: "workflow.codex.codewith/v0"
schema_version: "workflow.codex.codewith/v1"
"#;

    let err = parse_workflow_yaml(yaml).expect_err("duplicate keys should be rejected");

    assert!(
        err.to_string().contains("duplicate entry"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_placeholder_model_routes() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replacen(
        "model_gateway: \"hasna\"",
        "model_gateway: \"inherit\"",
        1,
    );

    let err = parse_workflow_yaml(&yaml).expect_err("placeholder model route should be rejected");

    assert!(
        err.to_string().contains("model_gateway")
            && err.to_string().contains("placeholder `inherit`"),
        "unexpected error: {err}"
    );
}

#[test]
fn parses_open_router_model_routing_contract() {
    let yaml = workflow_with_execution_defaults_routing(
        r#"  routing:
    contract_version: "open-router.codewith/v0"
    router: "open_router"
    request:
      requested_capability: "code_edit"
      context:
        task_id: "cc912869-361e-4277-9608-65c0f5a05b38"
        task_title: "Prepare Codewith integration point for smart model routing"
        project_path: "/home/hasna/workspace/hasna/opensource/open-codewith"
        tags:
          - "model-routing"
          - "task-lifecycle"
        auth_profile: "codewith-worker"
        approval_policy: "never"
        permission_profile: "workspace-write"
        worktree_mode: "required"
      constraints:
        allowed_model_gateways:
          - "hasna"
          - "openrouter"
        preferred_model_gateways:
          - "openrouter"
        allowed_providers:
          - "openai"
          - "anthropic"
          - "openrouter"
        allowed_models:
          - "gpt-5.4"
          - "anthropic/claude-sonnet-4.5"
        preferred_reasoning:
          - "medium"
          - "high"
        allowed_service_tiers:
          - "priority"
        allowed_auth_profiles:
          - "codewith-worker"
        allowed_approval_policies:
          - "never"
          - "on-request"
        allowed_permission_profiles:
          - "workspace-write"
        allowed_worktree_modes:
          - "required"
        max_context_tokens: 200000
        budget_usd: "1.25"
        fallback_required: true
    decision:
      status: "selected"
      model_gateway: "openrouter"
      provider: "openrouter"
      model: "anthropic/claude-sonnet-4.5"
      reasoning: "high"
      service_tier: "priority"
      auth_profile: "codewith-worker"
      explanation: "Open-router selected a code-edit-capable model inside constraints."
      fallback:
        used: false
      warnings: []
      errors: []
"#,
    );

    let spec = parse_workflow_yaml(&yaml).expect("routing contract should parse");
    let routing = spec
        .execution_defaults
        .routing
        .expect("execution defaults should include routing contract");

    assert_eq!("open-router.codewith/v0", routing.contract_version);
    assert_eq!(WorkflowModelRouter::OpenRouter, routing.router);
    assert_eq!(
        WorkflowModelRoutingCapability::CodeEdit,
        routing.request.requested_capability
    );
    assert_eq!(
        vec!["hasna".to_string(), "openrouter".to_string()],
        routing.request.constraints.allowed_model_gateways
    );
    let decision = routing.decision.expect("decision should parse");
    assert_eq!(
        WorkflowModelRoutingDecisionStatus::Selected,
        decision.status
    );
    assert_eq!(Some("openrouter".to_string()), decision.model_gateway);
    assert_eq!(
        Some("anthropic/claude-sonnet-4.5".to_string()),
        decision.model
    );
}

#[test]
fn rejects_open_router_decision_without_exact_route() {
    let yaml = workflow_with_execution_defaults_routing(
        r#"  routing:
    contract_version: "open-router.codewith/v0"
    router: "open_router"
    request:
      requested_capability: "verification"
    decision:
      status: "selected"
      model_gateway: "inherit"
      provider: "openrouter"
      model: "anthropic/claude-sonnet-4.5"
      reasoning: "high"
      explanation: "This should not parse because the route is not exact."
"#,
    );

    let err = parse_workflow_yaml(&yaml).expect_err("placeholder decision route should fail");

    assert!(
        err.to_string().contains("decision.model_gateway")
            && err.to_string().contains("placeholder `inherit`"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_open_router_error_decision_without_errors() {
    let yaml = workflow_with_execution_defaults_routing(
        r#"  routing:
    contract_version: "open-router.codewith/v0"
    router: "open_router"
    request:
      requested_capability: "verification"
    decision:
      status: "error"
      explanation: "No available model matched the constraints."
"#,
    );

    let err = parse_workflow_yaml(&yaml).expect_err("error decision should explain failures");

    assert!(
        err.to_string()
            .contains("decision.errors must describe why routing failed"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_non_ancient_agent_display_names() {
    let yaml =
        DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace("Architect-Archimedes", "Architect-Turing");

    let err = parse_workflow_yaml(&yaml).expect_err("non-ancient display name should be rejected");

    assert!(
        err.to_string().contains("display_name")
            && err.to_string().contains("approved ancient name"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_cyclic_step_dependencies() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replace(
        "    depends_on: []",
        "    depends_on:\n      - \"launch_gate\"",
    );

    let err = parse_workflow_yaml(&yaml).expect_err("cyclic dependency should be rejected");

    assert!(err.to_string().contains("cycle"), "unexpected error: {err}");
}

#[test]
fn rejects_candidate_completion_without_verifiers() {
    let yaml = r#"
schema_version: "workflow.codex.codewith/v0"
workflow_id: "wf_no_verifiers"
display_name: "No Verifiers"
source_prompt: "build something serious"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 2
  max_agents: 2
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 1200
  max_tokens: 100000
  max_tool_calls: 100
approvals:
  required_before: []
agents:
  - id: "architect"
    display_name: "Architect-Archimedes"
    role: "Own the architecture."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "design"
    title: "Design the system"
    agent: "architect"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on: []
    outputs:
      - "design.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers: []
artifacts:
  retention: "preserve_evidence"
  required:
    - "design.md"
cleanup:
  on_cancel:
    - "stop_child_agents"
  on_complete:
    - "archive_events"
"#;

    let err = parse_workflow_yaml(yaml).expect_err("empty verifier list should be rejected");

    assert!(
        err.to_string()
            .contains("must include at least one verifier"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_unbounded_verifier_retry_policy() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replacen(
        "          output_limit_bytes: 20000",
        "          output_limit_bytes: 20000\n          retry_policy:\n            max_attempts: 6",
        1,
    );

    let err = parse_workflow_yaml(&yaml).expect_err("retry policy should be capped");

    assert!(
        err.to_string()
            .contains("retry_policy.max_attempts must be at most 5"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_missing_exact_step_model_route() {
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML
        .replace("\r\n", "\n")
        .replacen(
        "    model:\n      model_gateway: \"hasna\"\n      provider: \"openai\"\n      model: \"gpt-5.4\"\n      reasoning: \"high\"\n    depends_on: []",
        "    depends_on: []",
        1,
    );

    let err = parse_workflow_yaml(&yaml).expect_err("step model route should be required");

    assert!(
        err.to_string()
            .contains("step `scope_constraints` must include an exact model route"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_overlong_step_title() {
    let title = "x".repeat(MAX_WORKFLOW_PROMPT_FIELD_CHARS + 1);
    let yaml = DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML.replacen(
        "title: \"Clarify offer, constraints, and non-negotiables\"",
        &format!("title: \"{title}\""),
        1,
    );

    let err = parse_workflow_yaml(&yaml).expect_err("overlong step title should be rejected");

    assert!(
        err.to_string().contains("step.title is too long"),
        "unexpected error: {err}"
    );
}

#[test]
fn workflow_branch_prompt_includes_parallel_group_when_present() {
    let prompt = render_workflow_branch_prompt(WorkflowBranchPrompt {
        run_id: "run-1",
        step_id: "implement",
        title: "Build the feature",
        agent_id: "Agent-Archimedes",
        parallel_group: Some("review"),
    });

    assert_eq!(
        prompt,
        concat!(
            "Workflow step `implement`: Build the feature\n",
            "Workflow run: run-1\n",
            "Agent: Agent-Archimedes\n",
            "Complete this workflow branch in its scoped session. Dependencies for this step are already satisfied.\n",
            "When finished, mark the branch work complete; deterministic workflow verifiers will decide final success.\n",
            "Parallel group: review",
        )
    );
}

#[test]
fn workflow_branch_prompt_truncates_overlong_title() {
    let title = "x".repeat(MAX_WORKFLOW_PROMPT_FIELD_CHARS + 10);
    let prompt = render_workflow_branch_prompt(WorkflowBranchPrompt {
        run_id: "run-1",
        step_id: "implement",
        title: &title,
        agent_id: "Agent-Archimedes",
        parallel_group: None,
    });

    let first_line = prompt.lines().next().expect("prompt has first line");
    assert_eq!(
        first_line,
        format!(
            "Workflow step `implement`: {}...",
            "x".repeat(MAX_WORKFLOW_PROMPT_FIELD_CHARS)
        )
    );
}
