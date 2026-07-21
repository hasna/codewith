//! Durable background-agent summaries and interactive manager for `/agent`.

use super::*;
use crate::text_formatting::truncate_text;
use chrono::DateTime;
use chrono::Local;
use codex_app_server_protocol::AgentAttachResponse;
use codex_app_server_protocol::AgentDaemonDiagnosticsResponse;
use codex_app_server_protocol::AgentDesiredState;
use codex_app_server_protocol::AgentEvent;
use codex_app_server_protocol::AgentEventsListResponse;
use codex_app_server_protocol::AgentExecutionSnapshot;
use codex_app_server_protocol::AgentPendingInteraction;
use codex_app_server_protocol::AgentPendingInteractionKind;
use codex_app_server_protocol::AgentPendingInteractionStatus;
use codex_app_server_protocol::AgentReadResponse;
use codex_app_server_protocol::AgentRetentionState;
use codex_app_server_protocol::AgentRun;
use codex_app_server_protocol::AgentRunStatus;
use codex_app_server_protocol::AgentStatusSnapshot;
use ratatui::text::Span;
use serde_json::Value as JsonValue;

impl ChatWidget {
    pub(crate) fn show_background_agent_manager(&mut self, agents: Vec<AgentRun>) {
        self.show_selection_view(background_agent_manager_params(agents));
    }

    pub(crate) fn show_background_agent_actions(&mut self, agent: AgentRun) {
        self.show_selection_view(background_agent_actions_params(agent));
    }

    pub(crate) fn show_background_agent_summary(&mut self, agents: Vec<AgentRun>) {
        self.add_plain_history_lines(background_agent_summary_lines(&agents));
    }

    pub(crate) fn show_background_agent_read(&mut self, response: AgentReadResponse) {
        let Some(agent) = response.agent else {
            self.add_info_message(
                "No matching background agent".to_string(),
                /*hint*/ None,
            );
            return;
        };
        let mut lines = vec![background_agent_header_line(&agent)];
        lines.extend(background_agent_detail_lines(
            &agent,
            response.status_snapshot.as_ref(),
            response.execution_snapshot.as_ref(),
        ));
        lines.extend(background_agent_pending_interaction_lines(
            &response.pending_interactions,
        ));
        self.add_plain_history_lines(lines);
    }

    pub(crate) fn show_background_agent_attach(&mut self, response: AgentAttachResponse) {
        let Some(agent) = response.agent.as_ref() else {
            self.add_info_message(
                "No matching background agent".to_string(),
                /*hint*/ None,
            );
            return;
        };
        let mut lines = vec![Line::from(vec![
            "Attached to background agent ".dim(),
            short_background_agent_id(agent.agent_id.as_str()).bold(),
            " ".into(),
            background_agent_status_label(agent.status),
        ])];
        if response.events.is_empty() {
            lines.push("No background-agent events recorded yet.".dim().into());
        } else {
            for event in response.events {
                lines.push(background_agent_event_line(&event));
            }
        }
        lines.extend(background_agent_pending_interaction_lines(
            &response.pending_interactions,
        ));
        if response.next_cursor.is_some() {
            lines.push(
                "More events are available; use the CLI for cursor paging."
                    .dim()
                    .into(),
            );
        }
        self.add_plain_history_lines(lines);
    }

    pub(crate) fn show_background_agent_logs(
        &mut self,
        agent_id: String,
        response: AgentEventsListResponse,
    ) {
        let mut lines = vec![Line::from(vec![
            "Background-agent logs ".bold(),
            short_background_agent_id(agent_id.as_str()).bold(),
        ])];
        if response.data.is_empty() {
            lines.push("No background-agent events recorded yet.".dim().into());
        } else {
            for event in response.data {
                lines.push(background_agent_event_line(&event));
            }
        }
        if response.next_cursor.is_some() {
            lines.push(
                "More events are available; use the CLI for cursor paging."
                    .dim()
                    .into(),
            );
        }
        self.add_plain_history_lines(lines);
    }

    pub(crate) fn show_background_agent_diagnostics(
        &mut self,
        diagnostics: AgentDaemonDiagnosticsResponse,
    ) {
        let mut lines = vec!["Background-agent daemon".bold().into()];
        lines.push(Line::from(vec![
            "  state store ".dim(),
            if diagnostics.state_store_available {
                "available".green()
            } else {
                "unavailable".red()
            },
            "  active ".dim(),
            diagnostics.active_run_count.to_string().into(),
            "/".into(),
            diagnostics.max_active_runs_per_user.to_string().into(),
            "  slots ".dim(),
            diagnostics.available_active_run_slots.to_string().into(),
        ]));
        lines.push(Line::from(vec![
            "  queued ".dim(),
            diagnostics.queued_run_count.to_string().into(),
            "  running ".dim(),
            diagnostics.running_run_count.to_string().into(),
            "  waiting ".dim(),
            diagnostics.waiting_run_count.to_string().into(),
            "  pending interactions ".dim(),
            diagnostics.pending_interaction_count.to_string().into(),
        ]));
        if diagnostics.backpressure_reasons.is_empty() {
            lines.push("  admission allowed".green().into());
        } else {
            lines.push(Line::from(vec![
                "  admission blocked: ".red(),
                diagnostics.backpressure_reasons.join(", ").into(),
            ]));
        }
        if !diagnostics.runs_by_status.is_empty() {
            let counts = diagnostics
                .runs_by_status
                .into_iter()
                .map(|entry| format!("{}={}", agent_status_name(entry.status), entry.count))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(vec!["  runs ".dim(), counts.into()]));
        }
        self.add_plain_history_lines(lines);
    }
}

fn background_agent_manager_params(mut agents: Vec<AgentRun>) -> SelectionViewParams {
    agents.sort_by_key(|agent| {
        (
            background_agent_status_sort_key(agent.status),
            std::cmp::Reverse(agent.updated_at),
            agent.agent_id.clone(),
        )
    });

    let mut items = Vec::with_capacity(agents.len() + 3);
    items.push(background_agent_action_item(
        "Start background agent",
        "Start a new durable background-agent run",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        || AppEvent::PrefillComposer {
            text: "/agent start ".to_string(),
        },
    ));
    items.push(background_agent_action_item(
        "Daemon diagnostics",
        "Show supervisor and queue diagnostics",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        || AppEvent::ShowBackgroundAgentDiagnostics,
    ));
    if agents.is_empty() {
        items.push(SelectionItem {
            name: "No background agents created".to_string(),
            description: Some("Start one with /agent start <prompt>".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        let group_counts = background_agent_group_counts(&agents);
        let mut current_group = None;
        for agent in agents {
            let group = background_agent_group(agent.status);
            if current_group != Some(group) {
                current_group = Some(group);
                items.push(background_agent_group_item(
                    group,
                    group_counts[group as usize],
                ));
            }
            let agent_id = agent.agent_id.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenBackgroundAgentActions {
                    agent_id: agent_id.clone(),
                });
            })];
            items.push(SelectionItem {
                name: background_agent_row_name(&agent),
                description: Some(background_agent_row_description(&agent)),
                selected_description: Some(background_agent_detail(&agent)),
                actions,
                dismiss_on_select: true,
                search_value: Some(background_agent_search_value(&agent)),
                ..Default::default()
            });
        }
    }

    SelectionViewParams {
        title: Some("Background Agents".to_string()),
        subtitle: Some("Select an agent to manage".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Search background agents".to_string()),
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn background_agent_actions_params(agent: AgentRun) -> SelectionViewParams {
    let agent_id = agent.agent_id.clone();
    let read_agent_id = agent_id.clone();
    let attach_agent_id = agent_id.clone();
    let detach_agent_id = agent_id.clone();
    let stop_agent_id = agent_id.clone();
    let delete_agent_id = agent_id;
    let mut items = vec![
        background_agent_action_item(
            "Read state",
            "Show latest status, execution snapshot, and pending interactions",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::ReadBackgroundAgent {
                agent_id: Some(read_agent_id.clone()),
            },
        ),
        background_agent_action_item(
            "Attach",
            "Replay events and mark pending interactions delivered",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::AttachBackgroundAgent {
                agent_id: Some(attach_agent_id.clone()),
            },
        ),
        background_agent_action_item(
            "Detach",
            "Detach this client from the agent",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::DetachBackgroundAgent {
                agent_id: Some(detach_agent_id.clone()),
            },
        ),
    ];
    let stop_disabled = background_agent_is_terminal(agent.status);
    items.push(background_agent_action_item(
        "Stop",
        "Request worker cancellation",
        stop_disabled,
        stop_disabled.then(|| "Agent is already terminal".to_string()),
        move || AppEvent::StopBackgroundAgent {
            agent_id: Some(stop_agent_id.clone()),
        },
    ));
    items.push(background_agent_action_item(
        "Delete",
        "Delete the run or mark it for deletion",
        agent.retention_state == AgentRetentionState::Deleted,
        (agent.retention_state == AgentRetentionState::Deleted)
            .then_some("Agent is already deleted".to_string()),
        move || AppEvent::DeleteBackgroundAgent {
            agent_id: Some(delete_agent_id.clone()),
        },
    ));
    items.push(background_agent_action_item(
        "Back to background agents",
        "Return to all background agents",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        || AppEvent::OpenBackgroundAgentManager,
    ));

    SelectionViewParams {
        title: Some(format!(
            "Background Agent {}",
            short_background_agent_id(agent.agent_id.as_str())
        )),
        subtitle: Some(background_agent_detail(&agent)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn background_agent_action_item(
    name: impl Into<String>,
    description: impl Into<String>,
    is_disabled: bool,
    disabled_reason: Option<String>,
    event: impl Fn() -> AppEvent + Send + Sync + 'static,
) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(event());
    })];
    SelectionItem {
        name: name.into(),
        description: Some(description.into()),
        is_disabled,
        disabled_reason,
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn background_agent_group_item(group: BackgroundAgentGroup, count: usize) -> SelectionItem {
    SelectionItem {
        name: format!("{} ({count})", group.label()),
        description: Some(group.description().to_string()),
        is_disabled: true,
        search_value: Some(group.label().to_string()),
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum BackgroundAgentGroup {
    NeedsInput = 0,
    Working = 1,
    Idle = 2,
    Stopping = 3,
    ReadyForReview = 4,
    Failed = 5,
    Stopped = 6,
}

impl BackgroundAgentGroup {
    const COUNT: usize = 7;

    fn label(self) -> &'static str {
        match self {
            BackgroundAgentGroup::NeedsInput => "Needs input",
            BackgroundAgentGroup::Working => "Working",
            BackgroundAgentGroup::Idle => "Idle",
            BackgroundAgentGroup::Stopping => "Stopping",
            BackgroundAgentGroup::ReadyForReview => "Ready for review",
            BackgroundAgentGroup::Failed => "Failed",
            BackgroundAgentGroup::Stopped => "Stopped",
        }
    }

    fn description(self) -> &'static str {
        match self {
            BackgroundAgentGroup::NeedsInput => "approval, permission, or user input required",
            BackgroundAgentGroup::Working => "starting or actively running",
            BackgroundAgentGroup::Idle => "queued or recoverable",
            BackgroundAgentGroup::Stopping => "cancellation requested",
            BackgroundAgentGroup::ReadyForReview => "completed and ready to inspect",
            BackgroundAgentGroup::Failed => "terminal error",
            BackgroundAgentGroup::Stopped => "cancelled or stopped",
        }
    }
}

fn background_agent_group_counts(agents: &[AgentRun]) -> [usize; BackgroundAgentGroup::COUNT] {
    let mut counts = [0; BackgroundAgentGroup::COUNT];
    for agent in agents {
        counts[background_agent_group(agent.status) as usize] += 1;
    }
    counts
}

fn background_agent_summary_lines(agents: &[AgentRun]) -> Vec<Line<'static>> {
    if agents.is_empty() {
        return vec!["No background agents created.".dim().into()];
    }
    let mut lines = vec![
        format!(
            "{} background agent{}",
            agents.len(),
            if agents.len() == 1 { "" } else { "s" }
        )
        .bold()
        .into(),
    ];
    for agent in agents {
        lines.push(background_agent_header_line(agent));
        if let Some(reason) = agent.status_reason.as_ref() {
            lines.push(
                format!("    {}", truncate_text(reason, /*max_graphemes*/ 96))
                    .dim()
                    .into(),
            );
        }
    }
    lines
}

fn background_agent_detail_lines(
    agent: &AgentRun,
    status_snapshot: Option<&AgentStatusSnapshot>,
    execution_snapshot: Option<&AgentExecutionSnapshot>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        "  desired ".dim(),
        agent_desired_state_name(agent.desired_state).into(),
        "  retention ".dim(),
        agent_retention_state_name(agent.retention_state).into(),
        "  updated ".dim(),
        format_timestamp(agent.updated_at).into(),
    ]));
    if let Some(thread_id) = agent.thread_id.as_ref() {
        lines.push(Line::from(vec![
            "  thread ".dim(),
            truncate_text(thread_id, /*max_graphemes*/ 48).into(),
        ]));
    }
    if let Some(parent_thread_id) = agent.parent_thread_id.as_ref() {
        lines.push(Line::from(vec![
            "  parent ".dim(),
            truncate_text(parent_thread_id, /*max_graphemes*/ 48).into(),
        ]));
    }
    if let Some(reason) = agent.status_reason.as_ref() {
        lines.push(Line::from(vec![
            "  reason ".dim(),
            truncate_text(reason, /*max_graphemes*/ 120).into(),
        ]));
    }
    if let Some(snapshot) = status_snapshot {
        let summary = snapshot
            .summary
            .clone()
            .unwrap_or_else(|| json_summary(&snapshot.payload));
        lines.push(Line::from(vec![
            "  status snapshot #".dim(),
            snapshot.seq.to_string().dim(),
            " ".into(),
            truncate_text(&summary, /*max_graphemes*/ 120).into(),
            "  pending ".dim(),
            snapshot.pending_interaction_count.to_string().into(),
        ]));
    }
    if let Some(snapshot) = execution_snapshot {
        lines.push(Line::from(vec![
            "  execution #".dim(),
            snapshot.seq.to_string().dim(),
            " ".into(),
            snapshot.snapshot_kind.clone().into(),
            "  recovery ".dim(),
            snapshot.recovery_policy.clone().dim(),
        ]));
    }
    lines
}

fn background_agent_pending_interaction_lines(
    interactions: &[AgentPendingInteraction],
) -> Vec<Line<'static>> {
    if interactions.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["Pending interactions".bold().into()];
    for interaction in interactions {
        lines.push(Line::from(vec![
            "  ".into(),
            pending_interaction_status_label(interaction.status),
            " ".into(),
            pending_interaction_kind_name(interaction.kind).into(),
            " ".into(),
            short_background_agent_id(interaction.interaction_id.as_str()).dim(),
            "  ".dim(),
            truncate_text(
                &json_summary(&interaction.request_payload),
                /*max_graphemes*/ 100,
            )
            .dim(),
        ]));
    }
    lines
}

fn background_agent_header_line(agent: &AgentRun) -> Line<'static> {
    Line::from(vec![
        "  ".into(),
        background_agent_status_label(agent.status),
        " ".into(),
        short_background_agent_id(agent.agent_id.as_str()).bold(),
        " ".into(),
        background_agent_source_label(agent).dim(),
        " ".into(),
        format_timestamp(agent.updated_at).dim(),
    ])
}

fn background_agent_row_name(agent: &AgentRun) -> String {
    format!(
        "{}  {}",
        short_background_agent_id(agent.agent_id.as_str()),
        agent_status_name(agent.status)
    )
}

fn background_agent_row_description(agent: &AgentRun) -> String {
    let mut parts = vec![
        agent_desired_state_name(agent.desired_state).to_string(),
        agent.source.clone(),
        format!("updated {}", format_timestamp(agent.updated_at)),
    ];
    if matches!(
        agent.status,
        AgentRunStatus::WaitingOnApproval | AgentRunStatus::WaitingOnUser
    ) {
        parts.push("needs attention".to_string());
    }
    parts.join(" - ")
}

fn background_agent_detail(agent: &AgentRun) -> String {
    let mut detail = format!(
        "{} - desired {} - retention {}",
        agent.source,
        agent_desired_state_name(agent.desired_state),
        agent_retention_state_name(agent.retention_state)
    );
    if let Some(reason) = agent.status_reason.as_ref() {
        detail.push_str(" - ");
        detail.push_str(&truncate_text(reason, /*max_graphemes*/ 120));
    }
    detail
}

fn background_agent_search_value(agent: &AgentRun) -> String {
    format!(
        "{} {} {} {:?} {:?}",
        agent.agent_id, agent.source, agent.prompt_snapshot_ref, agent.status, agent.desired_state
    )
}

fn background_agent_event_line(event: &AgentEvent) -> Line<'static> {
    Line::from(vec![
        format_timestamp(event.created_at).dim(),
        " ".into(),
        event.event_type.clone().fg(accent_color()),
        " #".dim(),
        event.seq.to_string().dim(),
        " ".into(),
        truncate_text(&json_summary(&event.payload), /*max_graphemes*/ 120).into(),
    ])
}

fn background_agent_status_label(status: AgentRunStatus) -> Span<'static> {
    match status {
        AgentRunStatus::Queued => "queued".fg(accent_color()),
        AgentRunStatus::Starting => "starting".fg(accent_color()),
        AgentRunStatus::Running => "running".fg(accent_color()),
        AgentRunStatus::WaitingOnApproval => "approval".magenta(),
        AgentRunStatus::WaitingOnUser => "waiting".magenta(),
        AgentRunStatus::Stopping => "stopping".magenta(),
        AgentRunStatus::Completed => "done".green(),
        AgentRunStatus::Failed => "failed".red(),
        AgentRunStatus::Cancelled => "cancelled".dim(),
        AgentRunStatus::Orphaned => "orphaned".magenta(),
    }
}

fn pending_interaction_status_label(status: AgentPendingInteractionStatus) -> Span<'static> {
    match status {
        AgentPendingInteractionStatus::Pending => "pending".magenta(),
        AgentPendingInteractionStatus::Delivered => "delivered".magenta(),
        AgentPendingInteractionStatus::Responded => "responded".green(),
        AgentPendingInteractionStatus::Expired => "expired".red(),
        AgentPendingInteractionStatus::Cancelled => "cancelled".dim(),
        AgentPendingInteractionStatus::Denied => "denied".red(),
        AgentPendingInteractionStatus::WorkerNoLongerWaiting => "stale".dim(),
    }
}

fn background_agent_status_sort_key(status: AgentRunStatus) -> u8 {
    background_agent_group(status) as u8
}

fn background_agent_group(status: AgentRunStatus) -> BackgroundAgentGroup {
    match status {
        AgentRunStatus::WaitingOnApproval | AgentRunStatus::WaitingOnUser => {
            BackgroundAgentGroup::NeedsInput
        }
        AgentRunStatus::Running | AgentRunStatus::Starting => BackgroundAgentGroup::Working,
        AgentRunStatus::Queued | AgentRunStatus::Orphaned => BackgroundAgentGroup::Idle,
        AgentRunStatus::Stopping => BackgroundAgentGroup::Stopping,
        AgentRunStatus::Completed => BackgroundAgentGroup::ReadyForReview,
        AgentRunStatus::Failed => BackgroundAgentGroup::Failed,
        AgentRunStatus::Cancelled => BackgroundAgentGroup::Stopped,
    }
}

fn background_agent_is_terminal(status: AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
    )
}

fn agent_status_name(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Queued => "queued",
        AgentRunStatus::Starting => "starting",
        AgentRunStatus::Running => "running",
        AgentRunStatus::WaitingOnApproval => "waitingOnApproval",
        AgentRunStatus::WaitingOnUser => "waitingOnUser",
        AgentRunStatus::Stopping => "stopping",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::Orphaned => "orphaned",
    }
}

fn agent_desired_state_name(state: AgentDesiredState) -> &'static str {
    match state {
        AgentDesiredState::Running => "running",
        AgentDesiredState::Stopped => "stopped",
        AgentDesiredState::Deleted => "deleted",
    }
}

fn agent_retention_state_name(state: AgentRetentionState) -> &'static str {
    match state {
        AgentRetentionState::Active => "active",
        AgentRetentionState::Archived => "archived",
        AgentRetentionState::DeleteRequested => "deleteRequested",
        AgentRetentionState::Deleted => "deleted",
    }
}

fn pending_interaction_kind_name(kind: AgentPendingInteractionKind) -> &'static str {
    match kind {
        AgentPendingInteractionKind::Approval => "approval",
        AgentPendingInteractionKind::UserInput => "userInput",
        AgentPendingInteractionKind::McpElicitation => "mcpElicitation",
        AgentPendingInteractionKind::PermissionGrant => "permissionGrant",
    }
}

fn background_agent_source_label(agent: &AgentRun) -> String {
    truncate_text(agent.source.as_str(), /*max_graphemes*/ 72)
}

fn json_summary(value: &JsonValue) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    for key in ["summary", "message", "text", "reason", "phase", "prompt"] {
        if let Some(text) = value.get(key).and_then(JsonValue::as_str) {
            return text.to_string();
        }
    }
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn short_background_agent_id(agent_id: &str) -> String {
    agent_id.chars().take(8).collect()
}

fn format_timestamp(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|datetime| {
            datetime
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn test_agent(agent_id: &str, status: AgentRunStatus, updated_at: i64) -> AgentRun {
        AgentRun {
            agent_id: agent_id.to_string(),
            idempotency_key: None,
            request_id: None,
            source: "test".to_string(),
            prompt_snapshot_ref: "inline:test:prompt".to_string(),
            input_snapshot_ref: None,
            thread_id: None,
            thread_store_kind: "background-agent".to_string(),
            thread_store_id: None,
            rollout_path: None,
            parent_thread_id: None,
            parent_agent_run_id: None,
            spawn_linkage: None,
            worktree_lease_id: None,
            auth_profile_ref: None,
            desired_state: AgentDesiredState::Running,
            status,
            status_reason: None,
            config_fingerprint: None,
            version_fingerprint: None,
            retention_state: AgentRetentionState::Active,
            archive_after: None,
            delete_after: None,
            archived_at: None,
            deleted_at: None,
            supervisor_id: None,
            generation: 0,
            pid: None,
            pgid: None,
            job_id: None,
            heartbeat_at: None,
            crash_reason: None,
            exit_code: None,
            exit_signal: None,
            last_event_seq: 0,
            last_snapshot_seq: 0,
            created_at: 1,
            updated_at,
            started_at: None,
            completed_at: None,
        }
    }

    #[test]
    fn manager_sorts_waiting_and_running_first() {
        let params = background_agent_manager_params(vec![
            test_agent(
                "done-agent",
                AgentRunStatus::Completed,
                /*updated_at*/ 3,
            ),
            test_agent("run-agent", AgentRunStatus::Running, /*updated_at*/ 2),
            test_agent(
                "wait-agent",
                AgentRunStatus::WaitingOnUser,
                /*updated_at*/ 1,
            ),
        ]);

        let item_names = params
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            item_names,
            vec![
                "Start background agent".to_string(),
                "Daemon diagnostics".to_string(),
                "Needs input (1)".to_string(),
                "wait-age  waitingOnUser".to_string(),
                "Working (1)".to_string(),
                "run-agen  running".to_string(),
                "Ready for review (1)".to_string(),
                "done-age  completed".to_string(),
            ]
        );
    }

    #[test]
    fn manager_offers_start_action_when_empty() {
        let params = background_agent_manager_params(Vec::new());

        let item_names = params
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            item_names,
            vec![
                "Start background agent".to_string(),
                "Daemon diagnostics".to_string(),
                "No background agents created".to_string(),
            ]
        );
        assert!(!params.items[0].is_disabled);
        assert!(!params.items[1].is_disabled);
        assert!(params.items[2].is_disabled);
    }

    #[test]
    fn source_label_uses_agent_source_when_status_reason_exists() {
        let mut agent = test_agent("run-agent", AgentRunStatus::Running, /*updated_at*/ 2);
        agent.source = "cli".to_string();
        agent.status_reason = Some("claimed by background supervisor".to_string());

        assert_eq!(background_agent_source_label(&agent), "cli");
    }
}
