//! Goal summary for the bare `/goal` command.

use super::*;
#[cfg(test)]
use crate::goal_display::format_goal_elapsed_seconds;
use crate::goal_display::goal_usage_summary;
#[cfg(test)]
use crate::status::format_tokens_compact;
use codex_app_server_protocol::ThreadGoalListResponse;
use codex_app_server_protocol::ThreadGoalPlanAutoExecute;
use codex_app_server_protocol::ThreadGoalPlanNode;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;

impl ChatWidget {
    #[cfg(test)]
    pub(crate) fn show_goal_summary(&mut self, goal: AppThreadGoal) {
        self.add_plain_history_lines(goal_summary_lines(&goal));
    }

    pub(crate) fn show_goal_manager(
        &mut self,
        thread_id: ThreadId,
        response: ThreadGoalListResponse,
    ) {
        let mut items = Vec::new();
        if let Some(goal) = response.goal {
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenThreadGoalEditor {
                    thread_id: Some(thread_id),
                });
            })];
            items.push(SelectionItem {
                name: format!("Current: {}", goal.objective),
                description: Some(format!(
                    "{} | {}",
                    goal_status_label(goal.status),
                    goal_usage_summary(&goal)
                )),
                selected_description: Some("Press Enter to edit the current goal.".to_string()),
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let mut planned_goal_count = 0usize;
        for plan in response.goal_plans {
            planned_goal_count += plan.nodes.len();
            items.push(SelectionItem {
                name: format!("Plan {}", short_goal_id(&plan.plan_id)),
                description: Some(format!(
                    "{} | {} | {} goals{}",
                    plan_status_label(plan.status),
                    plan_auto_execute_label(plan.auto_execute),
                    plan.nodes.len(),
                    plan.max_tokens
                        .map(|max_tokens| format!(" | cap {max_tokens} tokens"))
                        .unwrap_or_default()
                )),
                is_disabled: true,
                ..Default::default()
            });
            for node in plan.nodes {
                items.push(goal_plan_node_item(node));
            }
        }
        if response.next_cursor.is_some() {
            items.push(SelectionItem {
                name: "More goal plans available".to_string(),
                description: Some("Only the first page is shown here".to_string()),
                is_disabled: true,
                ..Default::default()
            });
        }

        self.show_selection_view(SelectionViewParams {
            title: Some("Goals".to_string()),
            subtitle: Some(format!(
                "{} planned goal{}",
                planned_goal_count,
                if planned_goal_count == 1 { "" } else { "s" }
            )),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search goals".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn show_goal_edit_prompt(&mut self, thread_id: ThreadId, goal: AppThreadGoal) {
        let tx = self.app_event_tx.clone();
        let status = edited_goal_status(goal.status);
        let token_budget = goal.token_budget;
        let view = CustomPromptView::new(
            "Edit goal".to_string(),
            "Type a goal objective and press Enter".to_string(),
            goal.objective,
            /*context_label*/ None,
            Box::new(move |objective: String| {
                tx.send(AppEvent::SetThreadGoalObjective {
                    thread_id,
                    objective,
                    mode: crate::app_event::ThreadGoalSetMode::UpdateExisting {
                        status,
                        token_budget,
                    },
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn show_resume_paused_goal_prompt(
        &mut self,
        thread_id: ThreadId,
        objective: String,
    ) {
        let resume_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SetThreadGoalStatus {
                thread_id,
                status: AppThreadGoalStatus::Active,
            });
        })];
        self.show_selection_view(SelectionViewParams {
            title: Some("Resume paused goal?".to_string()),
            subtitle: Some(format!("Goal: {objective}")),
            footer_hint: Some(standard_popup_hint_line()),
            initial_selected_idx: Some(0),
            items: vec![
                SelectionItem {
                    name: "Resume goal".to_string(),
                    description: Some("Mark it active and continue when idle".to_string()),
                    actions: resume_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Leave paused".to_string(),
                    description: Some("Keep it paused; use /goal resume later".to_string()),
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }

    pub(crate) fn on_thread_goal_cleared(&mut self, thread_id: &str) {
        if self
            .thread_id
            .is_some_and(|active_thread_id| active_thread_id.to_string() == thread_id)
        {
            self.current_goal_status = None;
            self.update_collaboration_mode_indicator();
        }
    }
}

#[cfg(test)]
fn goal_summary_lines(goal: &AppThreadGoal) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("Goal".bold()),
        Line::from(vec![
            "Status: ".dim(),
            goal_status_label(goal.status).to_string().into(),
        ]),
        Line::from(vec!["Objective: ".dim(), goal.objective.clone().into()]),
        Line::from(vec![
            "Time used: ".dim(),
            format_goal_elapsed_seconds(goal.time_used_seconds).into(),
        ]),
        Line::from(vec![
            "Tokens used: ".dim(),
            format_tokens_compact(goal.tokens_used).into(),
        ]),
    ];
    if let Some(token_budget) = goal.token_budget {
        lines.push(Line::from(vec![
            "Token budget: ".dim(),
            format_tokens_compact(token_budget).into(),
        ]));
    }
    let command_hint = match goal.status {
        AppThreadGoalStatus::Active => "Commands: /goal edit, /goal pause, /goal clear",
        AppThreadGoalStatus::Paused
        | AppThreadGoalStatus::Blocked
        | AppThreadGoalStatus::UsageLimited => "Commands: /goal edit, /goal resume, /goal clear",
        AppThreadGoalStatus::BudgetLimited | AppThreadGoalStatus::Complete => {
            "Commands: /goal edit, /goal clear"
        }
    };
    lines.push(Line::default());
    lines.push(Line::from(command_hint.dim()));
    lines
}

fn goal_status_label(status: AppThreadGoalStatus) -> &'static str {
    match status {
        AppThreadGoalStatus::Active => "active",
        AppThreadGoalStatus::Paused => "paused",
        AppThreadGoalStatus::Blocked => "blocked",
        AppThreadGoalStatus::UsageLimited => "usage limited",
        AppThreadGoalStatus::BudgetLimited => "limited by budget",
        AppThreadGoalStatus::Complete => "complete",
    }
}

fn edited_goal_status(status: AppThreadGoalStatus) -> AppThreadGoalStatus {
    match status {
        AppThreadGoalStatus::Active => AppThreadGoalStatus::Active,
        AppThreadGoalStatus::Paused
        | AppThreadGoalStatus::Blocked
        | AppThreadGoalStatus::UsageLimited => status,
        AppThreadGoalStatus::BudgetLimited | AppThreadGoalStatus::Complete => {
            AppThreadGoalStatus::Active
        }
    }
}

fn goal_plan_node_item(node: ThreadGoalPlanNode) -> SelectionItem {
    SelectionItem {
        name: format!("{}: {}", node.key, node.objective),
        description: Some(goal_plan_node_description(&node)),
        is_current: node.status == ThreadGoalPlanNodeStatus::Active,
        search_value: Some(format!(
            "{} {} {} {:?}",
            node.key, node.objective, node.node_id, node.depends_on
        )),
        ..Default::default()
    }
}

fn goal_plan_node_description(node: &ThreadGoalPlanNode) -> String {
    let dependencies = if node.depends_on.is_empty() {
        "deps none".to_string()
    } else {
        format!("deps {}", node.depends_on.join(", "))
    };
    format!(
        "{} | {} | {} | {}",
        plan_node_status_label(node.status),
        dependencies,
        plan_node_usage_summary(node),
        node.token_budget
            .map(|budget| format!("budget {budget}"))
            .unwrap_or_else(|| "budget unlimited".to_string())
    )
}

fn plan_node_usage_summary(node: &ThreadGoalPlanNode) -> String {
    if node.time_used_seconds > 0 {
        format!("{} tokens, {}s", node.tokens_used, node.time_used_seconds)
    } else {
        format!("{} tokens", node.tokens_used)
    }
}

fn plan_status_label(status: ThreadGoalPlanStatus) -> &'static str {
    match status {
        ThreadGoalPlanStatus::Active => "active",
        ThreadGoalPlanStatus::Paused => "paused",
        ThreadGoalPlanStatus::Blocked => "blocked",
        ThreadGoalPlanStatus::BudgetLimited => "budget-limited",
        ThreadGoalPlanStatus::Complete => "complete",
    }
}

fn plan_node_status_label(status: ThreadGoalPlanNodeStatus) -> &'static str {
    match status {
        ThreadGoalPlanNodeStatus::Pending => "pending",
        ThreadGoalPlanNodeStatus::Active => "active",
        ThreadGoalPlanNodeStatus::Paused => "paused",
        ThreadGoalPlanNodeStatus::Blocked => "blocked",
        ThreadGoalPlanNodeStatus::UsageLimited => "usage-limited",
        ThreadGoalPlanNodeStatus::BudgetLimited => "budget-limited",
        ThreadGoalPlanNodeStatus::Complete => "complete",
    }
}

fn plan_auto_execute_label(auto_execute: ThreadGoalPlanAutoExecute) -> &'static str {
    match auto_execute {
        ThreadGoalPlanAutoExecute::Off => "off",
        ThreadGoalPlanAutoExecute::ReadyOnly => "ready-only",
        ThreadGoalPlanAutoExecute::AiDirected => "ai-directed",
    }
}

fn short_goal_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}
