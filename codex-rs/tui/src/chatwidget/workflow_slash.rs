pub(super) const WORKFLOW_USAGE: &str = concat!(
    "Usage: /workflow [list|show <workflow_record_id>|draft <request>|",
    "run [list|show <run_id>|start <workflow_record_id>|pause <run_id>|resume <run_id>|cancel <run_id>]]"
);
pub(super) const WORKFLOW_USAGE_HINT: &str = concat!(
    "Examples: /workflow list, /workflow show <workflow_record_id>, ",
    "/workflow run, /workflow run start <workflow_record_id>, /workflow run pause <run_id>"
);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WorkflowSlashCommand<'a> {
    List,
    Show { workflow_record_id: &'a str },
    Draft { request: &'a str },
    RunList,
    RunShow { run_id: &'a str },
    RunStart { workflow_record_id: &'a str },
    RunPause { run_id: &'a str },
    RunResume { run_id: &'a str },
    RunCancel { run_id: &'a str },
}

pub(super) fn parse_workflow_slash_args(trimmed: &str) -> Result<WorkflowSlashCommand<'_>, String> {
    let trimmed = trimmed.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "" | "list" => Ok(WorkflowSlashCommand::List),
        "run" | "run list" => Ok(WorkflowSlashCommand::RunList),
        "help" | "--help" | "-h" => Err(WORKFLOW_USAGE.to_string()),
        _ => {
            let Some((command, rest)) = trimmed.split_once(char::is_whitespace) else {
                return Err(format!("Unknown /workflow command `{trimmed}`."));
            };
            let rest = rest.trim();
            match command.to_ascii_lowercase().as_str() {
                "draft" if rest.is_empty() => Err(WORKFLOW_USAGE.to_string()),
                "draft" => Ok(WorkflowSlashCommand::Draft { request: rest }),
                "show" => required_id(rest, "workflow_record_id")
                    .map(|workflow_record_id| WorkflowSlashCommand::Show { workflow_record_id }),
                "run" => parse_workflow_run_slash_args(rest),
                _ => Err(format!("Unknown /workflow command `{command}`.")),
            }
        }
    }
}

fn parse_workflow_run_slash_args(trimmed: &str) -> Result<WorkflowSlashCommand<'_>, String> {
    let trimmed = trimmed.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
        return Ok(WorkflowSlashCommand::RunList);
    }
    let Some((command, value)) = trimmed.split_once(char::is_whitespace) else {
        return Err(format!("Unknown /workflow run command `{trimmed}`."));
    };
    let value = value.trim();
    match command.to_ascii_lowercase().as_str() {
        "show" => {
            required_id(value, "run_id").map(|run_id| WorkflowSlashCommand::RunShow { run_id })
        }
        "start" => required_id(value, "workflow_record_id")
            .map(|workflow_record_id| WorkflowSlashCommand::RunStart { workflow_record_id }),
        "pause" => {
            required_id(value, "run_id").map(|run_id| WorkflowSlashCommand::RunPause { run_id })
        }
        "resume" => {
            required_id(value, "run_id").map(|run_id| WorkflowSlashCommand::RunResume { run_id })
        }
        "cancel" => {
            required_id(value, "run_id").map(|run_id| WorkflowSlashCommand::RunCancel { run_id })
        }
        _ => Err(format!("Unknown /workflow run command `{command}`.")),
    }
}

fn required_id<'a>(value: &'a str, name: &str) -> Result<&'a str, String> {
    let mut parts = value.split_whitespace();
    let Some(id) = parts.next() else {
        return Err(WORKFLOW_USAGE.to_string());
    };
    if parts.next().is_some() {
        return Err(format!("Expected one {name}."));
    }
    Ok(id)
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
    fn parses_show_and_run_commands() {
        assert_eq!(
            parse_workflow_slash_args("show workflow-1"),
            Ok(WorkflowSlashCommand::Show {
                workflow_record_id: "workflow-1"
            })
        );
        assert_eq!(
            parse_workflow_slash_args("run"),
            Ok(WorkflowSlashCommand::RunList)
        );
        assert_eq!(
            parse_workflow_slash_args("run list"),
            Ok(WorkflowSlashCommand::RunList)
        );
        assert_eq!(
            parse_workflow_slash_args("run show run-1"),
            Ok(WorkflowSlashCommand::RunShow { run_id: "run-1" })
        );
        assert_eq!(
            parse_workflow_slash_args("run start workflow-1"),
            Ok(WorkflowSlashCommand::RunStart {
                workflow_record_id: "workflow-1"
            })
        );
        assert_eq!(
            parse_workflow_slash_args("run pause run-1"),
            Ok(WorkflowSlashCommand::RunPause { run_id: "run-1" })
        );
        assert_eq!(
            parse_workflow_slash_args("run resume run-1"),
            Ok(WorkflowSlashCommand::RunResume { run_id: "run-1" })
        );
        assert_eq!(
            parse_workflow_slash_args("run cancel run-1"),
            Ok(WorkflowSlashCommand::RunCancel { run_id: "run-1" })
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
