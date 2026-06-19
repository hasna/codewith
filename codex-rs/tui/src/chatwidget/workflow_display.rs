use super::ChatWidget;
use codex_app_server_protocol::ThreadWorkflow;
use codex_app_server_protocol::ThreadWorkflowListResponse;
use codex_app_server_protocol::ThreadWorkflowStatus;
use ratatui::style::Stylize;
use ratatui::text::Line;

impl ChatWidget {
    pub(crate) fn show_thread_workflow_summary(&mut self, response: ThreadWorkflowListResponse) {
        if response.data.is_empty() {
            self.add_info_message(
                super::workflow_slash::WORKFLOW_USAGE.to_string(),
                Some("No saved workflows for this thread.".to_string()),
            );
            return;
        }

        self.add_plain_history_lines(thread_workflow_summary_lines(&response.data));
    }
}

pub(crate) fn thread_workflow_summary_lines(workflows: &[ThreadWorkflow]) -> Vec<Line<'static>> {
    let mut lines = vec!["Saved workflow spec metadata".bold().into()];
    for workflow in workflows {
        lines.push(
            vec![
                "• ".into(),
                public_workflow_display_name(&workflow.display_name).bold(),
                "  ".into(),
                workflow_status_label(workflow.status).dim(),
                "  ".into(),
                workflow.spec_workflow_id.clone().dim(),
            ]
            .into(),
        );
        lines.push(
            format!(
                "  agents {} | steps {} | parallel groups {} | verifiers {} | model-routed steps {}",
                workflow.agent_count,
                workflow.step_count,
                workflow.parallel_group_count,
                workflow.verifier_count,
                workflow.model_routed_step_count
            )
            .dim()
            .into(),
        );
        lines.push(
            format!(
                "  record {} | yaml sha256 {} | updated {}",
                short_id(&workflow.workflow_record_id),
                short_id(&workflow.source_yaml_sha256),
                workflow.updated_at
            )
            .dim()
            .into(),
        );
    }
    lines
}

fn public_workflow_display_name(value: &str) -> String {
    let mut cleaned = String::new();
    let mut last_was_space = false;
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if character.is_whitespace() {
            if !last_was_space {
                cleaned.push(' ');
            }
            last_was_space = true;
        } else {
            cleaned.push(character);
            last_was_space = false;
        }
    }

    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "[unnamed workflow]".to_string();
    }
    if metadata_label_looks_sensitive(cleaned) {
        return "[redacted workflow name]".to_string();
    }
    truncate_display_name(cleaned, /*max_chars*/ 80)
}

fn metadata_label_looks_sensitive(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "source_prompt",
        "sourceprompt",
        "raw yaml",
        "secret",
        "password",
        "api_key",
        "apikey",
        "token",
        "bearer ",
        "sk-",
    ]
    .into_iter()
    .any(|pattern| value.contains(pattern))
}

fn truncate_display_name(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn workflow_status_label(status: ThreadWorkflowStatus) -> &'static str {
    match status {
        ThreadWorkflowStatus::Draft => "draft",
        ThreadWorkflowStatus::NeedsClarification => "needs clarification",
        ThreadWorkflowStatus::Blocked => "blocked",
    }
}

fn short_id(value: &str) -> &str {
    value
        .char_indices()
        .nth(12)
        .map_or(value, |(idx, _)| &value[..idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Paragraph;
    use ratatui::widgets::Widget;

    #[test]
    fn workflow_summary_renders_metadata_without_raw_yaml_or_prompt_leaks() {
        let workflows = vec![
            ThreadWorkflow {
                thread_id: "thread-1".to_string(),
                workflow_record_id: "workflow-record-123456789".to_string(),
                spec_workflow_id: "wf_dentist_lead_saas".to_string(),
                schema_version: "workflow.codex.codewith/v0".to_string(),
                display_name: "Dental Lead SaaS Launch".to_string(),
                status: ThreadWorkflowStatus::Draft,
                source_yaml_sha256:
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                agent_count: 5,
                step_count: 12,
                parallel_group_count: 3,
                verifier_count: 9,
                run_command_verifier_count: 2,
                model_routed_step_count: 12,
                created_at: 1_800_000_000,
                updated_at: 1_800_000_123,
            },
            ThreadWorkflow {
                thread_id: "thread-1".to_string(),
                workflow_record_id: "workflow-record-987654321".to_string(),
                spec_workflow_id: "wf_sensitive_label".to_string(),
                schema_version: "workflow.codex.codewith/v0".to_string(),
                display_name: "source_prompt RAW_WORKFLOW_SECRET_SHOULD_NOT_LEAK\u{1b}".to_string(),
                status: ThreadWorkflowStatus::NeedsClarification,
                source_yaml_sha256:
                    "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210".to_string(),
                agent_count: 2,
                step_count: 4,
                parallel_group_count: 1,
                verifier_count: 3,
                run_command_verifier_count: 1,
                model_routed_step_count: 4,
                created_at: 1_800_000_010,
                updated_at: 1_800_000_456,
            },
        ];
        let rendered = render_lines(&thread_workflow_summary_lines(&workflows));

        insta::assert_snapshot!(
            rendered,
            @"Saved workflow spec metadata
• Dental Lead SaaS Launch  draft  wf_dentist_lead_saas
  agents 5 | steps 12 | parallel groups 3 | verifiers 9 | model-routed steps 12
  record workflow-rec | yaml sha256 0123456789ab | updated 1800000123
• [redacted workflow name]  needs clarification  wf_sensitive_label
  agents 2 | steps 4 | parallel groups 1 | verifiers 3 | model-routed steps 4
  record workflow-rec | yaml sha256 fedcba987654 | updated 1800000456"
        );
        for forbidden in [
            "source_prompt",
            "sourcePrompt",
            "raw yaml",
            "commands:",
            "touch ",
            "RAW_WORKFLOW_SECRET_SHOULD_NOT_LEAK",
            "\u{1b}",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "workflow summary leaked forbidden content `{forbidden}`"
            );
        }
    }

    #[test]
    fn short_id_preserves_short_values() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("abcdefghijklmnop"), "abcdefghijkl");
    }

    fn render_lines(lines: &[Line<'static>]) -> String {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 120, lines.len() as u16));
        Paragraph::new(lines.to_vec()).render(buffer.area, &mut buffer);
        (0..buffer.area.height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..buffer.area.width {
                    line.push_str(buffer[(x, y)].symbol());
                }
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
