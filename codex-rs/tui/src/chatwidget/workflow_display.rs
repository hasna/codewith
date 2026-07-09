use super::ChatWidget;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_app_server_protocol::ThreadWorkflow;
use codex_app_server_protocol::ThreadWorkflowGetResponse;
use codex_app_server_protocol::ThreadWorkflowListResponse;
use codex_app_server_protocol::ThreadWorkflowRun;
use codex_app_server_protocol::ThreadWorkflowRunGetResponse;
use codex_app_server_protocol::ThreadWorkflowRunListResponse;
use codex_app_server_protocol::ThreadWorkflowRunSnapshot;
use codex_app_server_protocol::ThreadWorkflowRunStartResponse;
use codex_app_server_protocol::ThreadWorkflowRunStatus;
use codex_app_server_protocol::ThreadWorkflowRunStep;
use codex_app_server_protocol::ThreadWorkflowRunStepStatus;
use codex_app_server_protocol::ThreadWorkflowRunStepVerifier;
use codex_app_server_protocol::ThreadWorkflowRunStepVerifierStatus;
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

    pub(crate) fn show_thread_workflow_detail(&mut self, response: ThreadWorkflowGetResponse) {
        let Some(workflow) = response.workflow else {
            self.add_info_message(
                "No workflow found for this thread.".to_string(),
                Some(super::workflow_slash::WORKFLOW_USAGE_HINT.to_string()),
            );
            return;
        };

        self.add_plain_history_lines(thread_workflow_detail_lines(&workflow));
    }

    pub(crate) fn show_thread_workflow_run_summary(
        &mut self,
        response: ThreadWorkflowRunListResponse,
    ) {
        if response.data.is_empty() {
            self.add_info_message(
                "No workflow runs for this thread.".to_string(),
                Some("/workflow run start <workflow_record_id>".to_string()),
            );
            return;
        }

        self.add_plain_history_lines(thread_workflow_run_summary_lines(&response.data));
    }

    pub(crate) fn show_thread_workflow_run_detail(
        &mut self,
        response: ThreadWorkflowRunGetResponse,
    ) {
        let Some(run) = response.run else {
            self.add_info_message(
                "No workflow run found for this thread.".to_string(),
                Some("/workflow run list".to_string()),
            );
            return;
        };

        self.add_plain_history_lines(thread_workflow_run_detail_lines(&run));
    }

    pub(crate) fn show_thread_workflow_run_started(
        &mut self,
        response: ThreadWorkflowRunStartResponse,
    ) {
        let mut lines = vec!["Started workflow run".bold().into()];
        lines.extend(thread_workflow_run_snapshot_lines(&response.run));
        if let Some(goal_plan) = response.goal_plan {
            lines.push(goal_plan_summary_line(&goal_plan));
        }
        self.add_plain_history_lines(lines);
    }

    pub(crate) fn show_thread_workflow_run_update(
        &mut self,
        title: &'static str,
        run: Option<ThreadWorkflowRunSnapshot>,
    ) {
        let Some(run) = run else {
            self.add_info_message(
                "No workflow run found for this thread.".to_string(),
                Some("/workflow run list".to_string()),
            );
            return;
        };

        let mut lines = vec![title.bold().into()];
        lines.extend(thread_workflow_run_snapshot_lines(&run));
        self.add_plain_history_lines(lines);
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

pub(crate) fn thread_workflow_detail_lines(workflow: &ThreadWorkflow) -> Vec<Line<'static>> {
    let mut lines = vec!["Workflow spec metadata".bold().into()];
    lines.extend(thread_workflow_summary_lines(std::slice::from_ref(
        workflow,
    )));
    lines.push(super::workflow_slash::WORKFLOW_USAGE_HINT.dim().into());
    lines
}

pub(crate) fn thread_workflow_run_summary_lines(runs: &[ThreadWorkflowRun]) -> Vec<Line<'static>> {
    let mut lines = vec!["Workflow runs".bold().into()];
    for run in runs {
        lines.push(run_header_line(run));
        lines.push(run_counts_line(run).dim().into());
        lines.push(run_approval_line(run));
        lines.push(
            format!(
                "  run {} | workflow {} | yaml sha256 {} | updated {}",
                short_id(&run.run_id),
                short_id(&run.workflow_record_id),
                short_id(&run.source_yaml_sha256),
                run.updated_at
            )
            .dim()
            .into(),
        );
    }
    lines
}

pub(crate) fn thread_workflow_run_detail_lines(
    snapshot: &ThreadWorkflowRunSnapshot,
) -> Vec<Line<'static>> {
    let mut lines = vec!["Workflow run detail".bold().into()];
    lines.extend(thread_workflow_run_snapshot_lines(snapshot));
    if !snapshot.steps.is_empty() {
        lines.push("Steps".bold().into());
        for step in snapshot.steps.iter().take(8) {
            lines.push(step_line(step));
        }
        push_remaining_count_line(
            &mut lines,
            snapshot.steps.len(),
            /*displayed*/ 8,
            "steps",
        );
    }
    if !snapshot.verifiers.is_empty() {
        lines.push("Verifiers".bold().into());
        for verifier in snapshot.verifiers.iter().take(8) {
            lines.push(verifier_line(verifier));
        }
        push_remaining_count_line(
            &mut lines,
            snapshot.verifiers.len(),
            /*displayed*/ 8,
            "verifiers",
        );
    }
    if !snapshot.events.is_empty() {
        lines.push("Recent events".bold().into());
        let event_count = snapshot.events.len();
        for event in snapshot.events.iter().rev().take(8).rev() {
            lines.push(
                format!(
                    "  #{} {} {} {}",
                    event.seq,
                    sanitize_metadata_label(&event.event_type),
                    sanitize_metadata_label(&event.actor_kind),
                    event.created_at
                )
                .dim()
                .into(),
            );
        }
        push_remaining_count_line(&mut lines, event_count, /*displayed*/ 8, "events");
    }
    lines
}

fn thread_workflow_run_snapshot_lines(snapshot: &ThreadWorkflowRunSnapshot) -> Vec<Line<'static>> {
    vec![
        run_header_line(&snapshot.run),
        run_counts_line(&snapshot.run).dim().into(),
        run_approval_line(&snapshot.run),
        format!(
            "  run {} | workflow {} | yaml sha256 {} | updated {}",
            short_id(&snapshot.run.run_id),
            short_id(&snapshot.run.workflow_record_id),
            short_id(&snapshot.run.source_yaml_sha256),
            snapshot.run.updated_at
        )
        .dim()
        .into(),
    ]
}

fn run_header_line(run: &ThreadWorkflowRun) -> Line<'static> {
    vec![
        "• ".into(),
        sanitize_metadata_label(&run.spec_workflow_id).bold(),
        "  ".into(),
        workflow_run_status_label(run.status).dim(),
        "  ".into(),
        short_id(&run.run_id).to_string().dim(),
    ]
    .into()
}

fn run_counts_line(run: &ThreadWorkflowRun) -> String {
    let mut step_counts = Vec::new();
    if run.pending_step_count > 0 {
        step_counts.push(format!("pending {}", run.pending_step_count));
    }
    if run.ready_step_count > 0 {
        step_counts.push(format!("ready {}", run.ready_step_count));
    }
    if run.active_step_count > 0 {
        step_counts.push(format!("active {}", run.active_step_count));
    }
    if run.waiting_verifier_step_count > 0 {
        step_counts.push(format!(
            "waiting verifier {}",
            run.waiting_verifier_step_count
        ));
    }
    if run.blocked_step_count > 0 {
        step_counts.push(format!("blocked {}", run.blocked_step_count));
    }
    if run.failed_step_count > 0 {
        step_counts.push(format!("failed {}", run.failed_step_count));
    }
    if run.succeeded_step_count > 0 {
        step_counts.push(format!("succeeded {}", run.succeeded_step_count));
    }
    if run.skipped_step_count > 0 {
        step_counts.push(format!("skipped {}", run.skipped_step_count));
    }
    if step_counts.is_empty() {
        step_counts.push("none".to_string());
    }

    format!(
        "  steps {} | verifiers {} | events {}",
        step_counts.join(", "),
        run.verifier_count,
        run.event_count
    )
}

/// Render the workflow-scoped approval-review affordance for a run.
///
/// The pending-review row is only shown when there is a pending approval to act
/// on. Otherwise the row stays present but disabled — either "no pending
/// review" when approval gates are configured, or "not configured" when the run
/// declares none. Gate labels are sanitized so a crafted spec cannot leak
/// content here.
fn run_approval_line(run: &ThreadWorkflowRun) -> Line<'static> {
    let review = &run.approval_review;
    if review.actionable {
        let gates = review
            .pending_gates
            .iter()
            .map(|gate| sanitize_metadata_label(&gate.gate))
            .collect::<Vec<_>>()
            .join(", ");
        vec![
            "  approvals ".into(),
            format!("{} pending review", review.pending_count).bold(),
            " · review in /workflows".into(),
            format!(" · gates {gates}").dim(),
        ]
        .into()
    } else if review.has_approval_config {
        "  approvals · no pending review".dim().into()
    } else {
        "  approvals · not configured".dim().into()
    }
}

fn step_line(step: &ThreadWorkflowRunStep) -> Line<'static> {
    vec![
        "  ".into(),
        format!("{}.", step.sequence).dim(),
        " ".into(),
        sanitize_metadata_label(&step.title).into(),
        "  ".into(),
        workflow_run_step_status_label(step.status).dim(),
        "  ".into(),
        step.step_id.clone().dim(),
    ]
    .into()
}

fn verifier_line(verifier: &ThreadWorkflowRunStepVerifier) -> Line<'static> {
    vec![
        "  ".into(),
        sanitize_metadata_label(&verifier.verifier_id).into(),
        "  ".into(),
        workflow_run_step_verifier_status_label(verifier.status).dim(),
        "  ".into(),
        sanitize_metadata_label(&verifier.verifier_type).dim(),
        "  ".into(),
        verifier.step_id.clone().dim(),
    ]
    .into()
}

fn goal_plan_summary_line(goal_plan: &ThreadGoalPlan) -> Line<'static> {
    format!(
        "  task plan {} | {} | nodes {} | ready {} | active {} | pending {}",
        short_id(&goal_plan.plan_id),
        goal_plan_status_label(goal_plan.status),
        goal_plan.node_count,
        goal_plan.ready_node_count,
        goal_plan.active_node_count,
        goal_plan.pending_node_count
    )
    .dim()
    .into()
}

fn public_workflow_display_name(value: &str) -> String {
    sanitize_metadata_label_with_redaction(value, "[redacted workflow name]")
}

fn sanitize_metadata_label(value: &str) -> String {
    sanitize_metadata_label_with_redaction(value, "[redacted]")
}

fn sanitize_metadata_label_with_redaction(value: &str, redacted_label: &str) -> String {
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
        return redacted_label.to_string();
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

fn workflow_run_status_label(status: ThreadWorkflowRunStatus) -> &'static str {
    match status {
        ThreadWorkflowRunStatus::Pending => "pending",
        ThreadWorkflowRunStatus::Running => "running",
        ThreadWorkflowRunStatus::Waiting => "waiting",
        ThreadWorkflowRunStatus::Blocked => "blocked",
        ThreadWorkflowRunStatus::Paused => "paused",
        ThreadWorkflowRunStatus::CancelRequested => "cancel requested",
        ThreadWorkflowRunStatus::Cancelled => "cancelled",
        ThreadWorkflowRunStatus::Failed => "failed",
        ThreadWorkflowRunStatus::Completed => "completed",
        ThreadWorkflowRunStatus::Other => "other",
    }
}

fn workflow_run_step_status_label(status: ThreadWorkflowRunStepStatus) -> &'static str {
    match status {
        ThreadWorkflowRunStepStatus::Pending => "pending",
        ThreadWorkflowRunStepStatus::Ready => "ready",
        ThreadWorkflowRunStepStatus::Active => "active",
        ThreadWorkflowRunStepStatus::WaitingVerifier => "waiting verifier",
        ThreadWorkflowRunStepStatus::Blocked => "blocked",
        ThreadWorkflowRunStepStatus::Skipped => "skipped",
        ThreadWorkflowRunStepStatus::Cancelled => "cancelled",
        ThreadWorkflowRunStepStatus::Failed => "failed",
        ThreadWorkflowRunStepStatus::Succeeded => "succeeded",
        ThreadWorkflowRunStepStatus::Other => "other",
    }
}

fn workflow_run_step_verifier_status_label(
    status: ThreadWorkflowRunStepVerifierStatus,
) -> &'static str {
    match status {
        ThreadWorkflowRunStepVerifierStatus::Pending => "pending",
        ThreadWorkflowRunStepVerifierStatus::Running => "running",
        ThreadWorkflowRunStepVerifierStatus::Blocked => "blocked",
        ThreadWorkflowRunStepVerifierStatus::Passed => "passed",
        ThreadWorkflowRunStepVerifierStatus::Failed => "failed",
        ThreadWorkflowRunStepVerifierStatus::Skipped => "skipped",
        ThreadWorkflowRunStepVerifierStatus::Other => "other",
    }
}

fn goal_plan_status_label(status: ThreadGoalPlanStatus) -> &'static str {
    match status {
        ThreadGoalPlanStatus::Active => "active",
        ThreadGoalPlanStatus::Paused => "paused",
        ThreadGoalPlanStatus::Blocked => "blocked",
        ThreadGoalPlanStatus::BudgetLimited => "budget limited",
        ThreadGoalPlanStatus::Complete => "complete",
        ThreadGoalPlanStatus::Cancelled => "cancelled",
    }
}

fn short_id(value: &str) -> &str {
    value
        .char_indices()
        .nth(12)
        .map_or(value, |(idx, _)| &value[..idx])
}

fn push_remaining_count_line(
    lines: &mut Vec<Line<'static>>,
    total: usize,
    displayed: usize,
    noun: &str,
) {
    if total > displayed {
        lines.push(format!("  +{} more {noun}", total - displayed).dim().into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ThreadWorkflowApprovalGate;
    use codex_app_server_protocol::ThreadWorkflowApprovalReview;
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

    #[test]
    fn workflow_run_detail_renders_sanitized_snapshot() {
        let snapshot = ThreadWorkflowRunSnapshot {
            run: ThreadWorkflowRun {
                thread_id: Some("thread-1".to_string()),
                run_id: "run-123456789abcdef".to_string(),
                workflow_record_id: "workflow-record-123456789".to_string(),
                spec_workflow_id: "wf_dentist_lead_saas".to_string(),
                schema_version: "workflow.codex.codewith/v0".to_string(),
                source_yaml_sha256:
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                status: ThreadWorkflowRunStatus::Paused,
                status_reason: Some("workflow run paused".to_string()),
                reason_code: Some("paused_by_user".to_string()),
                generation: 2,
                pending_step_count: 1,
                ready_step_count: 0,
                active_step_count: 0,
                waiting_verifier_step_count: 0,
                blocked_step_count: 0,
                failed_step_count: 0,
                succeeded_step_count: 1,
                skipped_step_count: 0,
                verifier_count: 1,
                event_count: 2,
                approval_review: ThreadWorkflowApprovalReview {
                    has_approval_config: false,
                    available: false,
                    actionable: false,
                    pending_count: 0,
                    pending_gates: Vec::new(),
                },
                created_at: 1_800_000_000,
                updated_at: 1_800_000_123,
                started_at: None,
                completed_at: None,
            },
            steps: vec![
                ThreadWorkflowRunStep {
                    step_run_id: "step-run-1".to_string(),
                    step_id: "collect_leads".to_string(),
                    sequence: 1,
                    title: "Collect lead requirements".to_string(),
                    agent_id: "Socrates".to_string(),
                    status: ThreadWorkflowRunStepStatus::Succeeded,
                    status_reason: None,
                    reason_code: None,
                    depends_on: Vec::new(),
                    background_agent_run_id: None,
                    created_at: 1_800_000_001,
                    updated_at: 1_800_000_010,
                    started_at: Some(1_800_000_002),
                    completed_at: Some(1_800_000_010),
                },
                ThreadWorkflowRunStep {
                    step_run_id: "step-run-2".to_string(),
                    step_id: "sensitive_step".to_string(),
                    sequence: 2,
                    title: "source_prompt secret command should not leak".to_string(),
                    agent_id: "Cicero".to_string(),
                    status: ThreadWorkflowRunStepStatus::Pending,
                    status_reason: None,
                    reason_code: None,
                    depends_on: vec!["collect_leads".to_string()],
                    background_agent_run_id: None,
                    created_at: 1_800_000_011,
                    updated_at: 1_800_000_012,
                    started_at: None,
                    completed_at: None,
                },
            ],
            verifiers: vec![ThreadWorkflowRunStepVerifier {
                verifier_run_id: "verifier-run-1".to_string(),
                step_id: "collect_leads".to_string(),
                verifier_id: "test-suite".to_string(),
                verifier_type: "run_commands".to_string(),
                status: ThreadWorkflowRunStepVerifierStatus::Passed,
                status_reason: None,
                reason_code: None,
                attempt_count: 1,
                max_attempts: Some(2),
                created_at: 1_800_000_010,
                updated_at: 1_800_000_011,
                completed_at: Some(1_800_000_011),
            }],
            events: vec![
                codex_app_server_protocol::ThreadWorkflowRunEvent {
                    seq: 1,
                    event_type: "created".to_string(),
                    actor_kind: "system".to_string(),
                    actor_id: None,
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "public".to_string(),
                    created_at: 1_800_000_000,
                },
                codex_app_server_protocol::ThreadWorkflowRunEvent {
                    seq: 2,
                    event_type: "paused".to_string(),
                    actor_kind: "user".to_string(),
                    actor_id: None,
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "public".to_string(),
                    created_at: 1_800_000_123,
                },
            ],
        };
        let rendered = render_lines(&thread_workflow_run_detail_lines(&snapshot));

        insta::assert_snapshot!(
            rendered,
            @r###"
Workflow run detail
• wf_dentist_lead_saas  paused  run-12345678
  steps pending 1, succeeded 1 | verifiers 1 | events 2
  approvals · not configured
  run run-12345678 | workflow workflow-rec | yaml sha256 0123456789ab | updated 1800000123
Steps
  1. Collect lead requirements  succeeded  collect_leads
  2. [redacted]  pending  sensitive_step
Verifiers
  test-suite  passed  run_commands  collect_leads
Recent events
  #1 created system 1800000000
  #2 paused user 1800000123
"###
        );
        assert!(!rendered.contains("source_prompt"));
        assert!(!rendered.contains("secret command"));
    }

    fn run_with_approval_review(review: ThreadWorkflowApprovalReview) -> ThreadWorkflowRun {
        ThreadWorkflowRun {
            thread_id: Some("thread-1".to_string()),
            run_id: "run-123456789abcdef".to_string(),
            workflow_record_id: "workflow-record-123456789".to_string(),
            spec_workflow_id: "wf_dentist_lead_saas".to_string(),
            schema_version: "workflow.codex.codewith/v0".to_string(),
            source_yaml_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            status: ThreadWorkflowRunStatus::Running,
            status_reason: None,
            reason_code: None,
            generation: 1,
            pending_step_count: 1,
            ready_step_count: 1,
            active_step_count: 0,
            waiting_verifier_step_count: 0,
            blocked_step_count: 0,
            failed_step_count: 0,
            succeeded_step_count: 0,
            skipped_step_count: 0,
            verifier_count: 0,
            event_count: 0,
            approval_review: review,
            created_at: 1_800_000_000,
            updated_at: 1_800_000_123,
            started_at: Some(1_800_000_001),
            completed_at: None,
        }
    }

    #[test]
    fn workflow_run_summary_renders_enabled_pending_approval_review() {
        let run = run_with_approval_review(ThreadWorkflowApprovalReview {
            has_approval_config: true,
            available: true,
            actionable: true,
            pending_count: 2,
            pending_gates: vec![
                ThreadWorkflowApprovalGate {
                    step_id: "deploy".to_string(),
                    gate: "before_deploy".to_string(),
                },
                ThreadWorkflowApprovalGate {
                    step_id: "rollout".to_string(),
                    // A crafted gate label must never leak here.
                    gate: "source_prompt RAW_WORKFLOW_SECRET_SHOULD_NOT_LEAK".to_string(),
                },
            ],
        });
        let rendered = render_lines(&thread_workflow_run_summary_lines(std::slice::from_ref(
            &run,
        )));

        insta::assert_snapshot!(
            rendered,
            @r###"
Workflow runs
• wf_dentist_lead_saas  running  run-12345678
  steps pending 1, ready 1 | verifiers 0 | events 0
  approvals 2 pending review · review in /workflows · gates before_deploy, [redacted]
  run run-12345678 | workflow workflow-rec | yaml sha256 0123456789ab | updated 1800000123
"###
        );
        assert!(!rendered.contains("/workflow approve"));
        assert!(rendered.contains("review in /workflows"));
        assert!(!rendered.contains("source_prompt"));
        assert!(!rendered.contains("RAW_WORKFLOW_SECRET_SHOULD_NOT_LEAK"));
    }

    #[test]
    fn workflow_run_summary_disables_approval_row_without_pending_review() {
        let configured_no_pending = run_with_approval_review(ThreadWorkflowApprovalReview {
            has_approval_config: true,
            available: true,
            actionable: false,
            pending_count: 0,
            pending_gates: Vec::new(),
        });
        let not_configured = run_with_approval_review(ThreadWorkflowApprovalReview {
            has_approval_config: false,
            available: false,
            actionable: false,
            pending_count: 0,
            pending_gates: Vec::new(),
        });
        let rendered = render_lines(&thread_workflow_run_summary_lines(&[
            configured_no_pending,
            not_configured,
        ]));

        insta::assert_snapshot!(
            rendered,
            @r###"
Workflow runs
• wf_dentist_lead_saas  running  run-12345678
  steps pending 1, ready 1 | verifiers 0 | events 0
  approvals · no pending review
  run run-12345678 | workflow workflow-rec | yaml sha256 0123456789ab | updated 1800000123
• wf_dentist_lead_saas  running  run-12345678
  steps pending 1, ready 1 | verifiers 0 | events 0
  approvals · not configured
  run run-12345678 | workflow workflow-rec | yaml sha256 0123456789ab | updated 1800000123
"###
        );
        // No enabled approval action is offered when there is nothing to act on.
        assert!(!rendered.contains("/workflow approve"));
        assert!(!rendered.contains("pending review · review"));
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
