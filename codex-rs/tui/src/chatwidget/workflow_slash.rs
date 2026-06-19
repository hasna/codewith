pub(super) const WORKFLOW_USAGE: &str = "Usage: /workflow [list|draft <request>]";
pub(super) const WORKFLOW_USAGE_HINT: &str =
    "Examples: /workflow list, /workflow draft build a SaaS that collects dentist leads";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WorkflowSlashCommand<'a> {
    List,
    Draft { request: &'a str },
}

pub(super) fn parse_workflow_slash_args(trimmed: &str) -> Result<WorkflowSlashCommand<'_>, String> {
    let trimmed = trimmed.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "" | "list" => Ok(WorkflowSlashCommand::List),
        "help" | "--help" | "-h" => Err(WORKFLOW_USAGE.to_string()),
        _ => {
            let Some((command, request)) = trimmed.split_once(char::is_whitespace) else {
                return Err(format!("Unknown /workflow command `{trimmed}`."));
            };
            match command.to_ascii_lowercase().as_str() {
                "draft" if request.trim().is_empty() => Err(WORKFLOW_USAGE.to_string()),
                "draft" => Ok(WorkflowSlashCommand::Draft {
                    request: request.trim(),
                }),
                _ => Err(format!("Unknown /workflow command `{command}`.")),
            }
        }
    }
}

pub(super) fn workflow_generation_prompt(request: &str) -> String {
    format!(
        r#"Create a Codewith workflow YAML document for this request:

{request}

Return only raw YAML. Do not use Markdown fences, Mermaid, prose, or diagrams.

The YAML must be deep enough for long-running production work:
- use schema_version: "workflow.codex.codewith/v0"
- decompose the work into many concrete steps with explicit dependencies
- include parallel steps where useful
- include model routing on execution_defaults, every agent, and every step: model_gateway, provider, model, reasoning
- name agents after ancient Greek or Roman figures, mathematicians, scientists, or philosophers
- include at least two adversarial agents or adversarial steps
- include deterministic completion verifiers, including tests and at least one bounded test-loop style verifier definition when appropriate
- include no placeholders for provider, model, reasoning, agent names, or verifier details

This is design-only. Do not start goals, schedules, monitors, agents, worktrees, timers, or shell commands. If the validate_workflow_yaml tool is available, use it only to validate the YAML; do not execute verifier commands or create workflow runs."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_list_aliases() {
        assert_eq!(
            parse_workflow_slash_args(""),
            Ok(WorkflowSlashCommand::List)
        );
        assert_eq!(
            parse_workflow_slash_args("list"),
            Ok(WorkflowSlashCommand::List)
        );
    }

    #[test]
    fn parses_explicit_draft_command() {
        assert_eq!(
            parse_workflow_slash_args("draft build a dentist lead SaaS"),
            Ok(WorkflowSlashCommand::Draft {
                request: "build a dentist lead SaaS"
            })
        );
    }

    #[test]
    fn rejects_implicit_draft_command() {
        assert_eq!(
            parse_workflow_slash_args("build a dentist lead SaaS"),
            Err("Unknown /workflow command `build`.".to_string())
        );
    }

    #[test]
    fn generation_prompt_enforces_yaml_only_deep_adversarial_shape() {
        let prompt = workflow_generation_prompt("build a dentist lead SaaS");
        assert!(prompt.contains("Return only raw YAML"));
        assert!(prompt.contains("Do not use Markdown fences, Mermaid, prose, or diagrams."));
        assert!(prompt.contains("workflow.codex.codewith/v0"));
        assert!(prompt.contains("model_gateway, provider, model, reasoning"));
        assert!(prompt.contains("ancient Greek or Roman"));
        assert!(prompt.contains("at least two adversarial"));
        assert!(prompt.contains("bounded test-loop"));
        assert!(prompt.contains("Do not start goals, schedules, monitors, agents"));
        assert!(prompt.contains("build a dentist lead SaaS"));
    }
}
