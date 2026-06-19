use crate::MAX_WORKFLOW_PROMPT_FIELD_CHARS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowBranchPrompt<'a> {
    pub run_id: &'a str,
    pub step_id: &'a str,
    pub title: &'a str,
    pub agent_id: &'a str,
    pub parallel_group: Option<&'a str>,
}

pub fn render_workflow_branch_prompt(input: WorkflowBranchPrompt<'_>) -> String {
    let title = truncate_prompt_field(input.title);
    let mut lines = vec![
        format!("Workflow step `{}`: {title}", input.step_id),
        format!("Workflow run: {}", input.run_id),
        format!("Agent: {}", input.agent_id),
        "Complete this workflow branch in its scoped session. Dependencies for this step are already satisfied.".to_string(),
        "When finished, mark the branch work complete; deterministic workflow verifiers will decide final success.".to_string(),
    ];
    if let Some(parallel_group) = input.parallel_group {
        lines.push(format!("Parallel group: {parallel_group}"));
    }
    lines.join("\n")
}

fn truncate_prompt_field(value: &str) -> String {
    let mut chars = value.chars();
    let truncated = chars
        .by_ref()
        .take(MAX_WORKFLOW_PROMPT_FIELD_CHARS)
        .collect::<String>();
    if chars.next().is_none() {
        value.to_string()
    } else {
        format!("{truncated}...")
    }
}
