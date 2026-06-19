/// System prompt fragment for drafting a first-class Codewith workflow.
///
/// This prompt defines the visible workflow contract only. It deliberately does
/// not imply that YAML can grant permissions, mutate config, or execute without
/// the workflow runtime compiling it into typed, policy-checked actions.
pub const WORKFLOW_YAML_SYSTEM_PROMPT: &str =
    include_str!("../templates/workflows/deep_yaml_system_prompt.md");

/// Example workflow fixture used to keep the initial workflow shape concrete.
pub const DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML: &str =
    include_str!("../templates/workflows/dental_lead_saas.yaml");

#[cfg(test)]
#[path = "workflows_tests.rs"]
mod workflows_tests;
