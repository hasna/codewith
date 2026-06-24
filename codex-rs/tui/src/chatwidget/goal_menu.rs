//! Goal summary for the bare `/goal` command.

use super::*;
use crate::goal_display::format_goal_elapsed_seconds;
use crate::status::format_tokens_compact;
use codex_app_server_protocol::ThreadGoalListResponse;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanAutoExecute;
use codex_app_server_protocol::ThreadGoalPlanNode;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_protocol::protocol::thread_goal_display_title;

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
        let current_goal = response.goal;
        if let Some(goal) = current_goal.as_ref() {
            items.push(current_goal_item(thread_id, goal));
            if goal_can_cancel(goal.status) {
                items.push(cancel_current_goal_item(thread_id, goal));
            }
        }

        let goal_plans = response.goal_plans;
        let subtitle = goal_manager_subtitle(current_goal.as_ref(), &goal_plans);
        self.current_goal_plan = goal_plans
            .iter()
            .find(|plan| plan.status == ThreadGoalPlanStatus::Active)
            .or_else(|| goal_plans.first())
            .cloned();

        let mut planned_goal_count = 0usize;
        for plan in goal_plans {
            planned_goal_count += plan.nodes.len();
            items.push(goal_plan_item(thread_id, plan));
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
            subtitle: Some(subtitle.unwrap_or_else(|| {
                format!(
                    "{} planned goal{}",
                    planned_goal_count,
                    if planned_goal_count == 1 { "" } else { "s" }
                )
            })),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search goals".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn show_goal_plan_detail(&mut self, thread_id: ThreadId, plan: ThreadGoalPlan) {
        let mut items = Vec::with_capacity(plan.nodes.len() + 1);
        for node in plan.nodes.iter().cloned() {
            items.push(goal_plan_node_item(thread_id, node));
        }
        if plan.nodes.is_empty() {
            items.push(SelectionItem {
                name: "No goals in this plan".to_string(),
                is_disabled: true,
                ..Default::default()
            });
        }

        let back_thread_id = thread_id;
        let back_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenThreadGoalMenu {
                thread_id: back_thread_id,
            });
        })];
        items.push(SelectionItem {
            name: "Back to goals".to_string(),
            actions: back_actions,
            dismiss_on_select: true,
            ..Default::default()
        });

        self.show_selection_view(SelectionViewParams {
            title: Some(goal_plan_display_name(&plan)),
            subtitle: Some(goal_plan_detail_summary(&plan)),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search plan goals".to_string()),
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
            self.current_goal_plan = None;
            self.update_collaboration_mode_indicator();
            self.refresh_status_line();
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
        AppThreadGoalStatus::Active => {
            "Commands: /goal edit, /goal pause, /goal cancel, /goal clear"
        }
        AppThreadGoalStatus::Paused
        | AppThreadGoalStatus::Blocked
        | AppThreadGoalStatus::UsageLimited => {
            "Commands: /goal edit, /goal resume, /goal cancel, /goal clear"
        }
        AppThreadGoalStatus::BudgetLimited => "Commands: /goal edit, /goal cancel, /goal clear",
        AppThreadGoalStatus::Complete | AppThreadGoalStatus::Cancelled => {
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
        AppThreadGoalStatus::Cancelled => "cancelled",
    }
}

fn edited_goal_status(status: AppThreadGoalStatus) -> AppThreadGoalStatus {
    match status {
        AppThreadGoalStatus::Active => AppThreadGoalStatus::Active,
        AppThreadGoalStatus::Paused
        | AppThreadGoalStatus::Blocked
        | AppThreadGoalStatus::UsageLimited => status,
        AppThreadGoalStatus::BudgetLimited
        | AppThreadGoalStatus::Complete
        | AppThreadGoalStatus::Cancelled => AppThreadGoalStatus::Active,
    }
}

fn current_goal_item(thread_id: ThreadId, goal: &AppThreadGoal) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenThreadGoalEditor {
            thread_id: Some(thread_id),
        });
    })];
    SelectionItem {
        name: goal_row_name(goal, /*is_current*/ true),
        selected_description: Some(current_goal_detail(goal)),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!("{} {}", goal.objective, goal.goal_id)),
        ..Default::default()
    }
}

fn cancel_current_goal_item(thread_id: ThreadId, goal: &AppThreadGoal) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::SetThreadGoalStatus {
            thread_id,
            status: AppThreadGoalStatus::Cancelled,
        });
    })];
    SelectionItem {
        name: "Cancel current goal".to_string(),
        description: Some(goal.objective.clone()),
        selected_description: Some(middle_dot(vec![
            "Stops the goal without deleting its usage history".to_string(),
            goal_time_part(goal.time_used_seconds),
        ])),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!("cancel {}", goal.objective)),
        ..Default::default()
    }
}

fn goal_plan_item(thread_id: ThreadId, plan: ThreadGoalPlan) -> SelectionItem {
    let plan_for_action = plan.clone();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenThreadGoalPlanDetail {
            thread_id,
            plan: plan_for_action.clone(),
        });
    })];
    SelectionItem {
        name: goal_plan_row_name(&plan),
        selected_description: Some(goal_plan_selected_detail(&plan)),
        actions,
        dismiss_on_select: true,
        search_value: Some(goal_plan_search_value(&plan)),
        ..Default::default()
    }
}

fn goal_plan_node_item(thread_id: ThreadId, node: ThreadGoalPlanNode) -> SelectionItem {
    let can_activate = node.ready && node.assigned_thread_id == thread_id.to_string();
    let actions: Vec<SelectionAction> = if can_activate {
        let node_id = node.node_id.clone();
        vec![Box::new(move |tx| {
            tx.send(AppEvent::ActivateThreadGoalPlanNode {
                thread_id,
                node_id: node_id.clone(),
            });
        })]
    } else {
        Vec::new()
    };
    SelectionItem {
        name: goal_plan_node_row_name(&node),
        selected_description: Some(goal_plan_node_selected_detail(&node)),
        actions,
        dismiss_on_select: can_activate,
        search_value: Some(format!(
            "{} {} {} {:?}",
            node.key, node.objective, node.node_id, node.depends_on
        )),
        ..Default::default()
    }
}

fn goal_row_name(goal: &AppThreadGoal, is_current: bool) -> String {
    let title = thread_goal_display_title(goal.title.as_deref(), &goal.objective);
    let label = if is_current {
        format!("Current: {title}")
    } else {
        title
    };
    middle_dot(vec![
        label,
        goal_status_label(goal.status).to_string(),
        goal_time_part(goal.time_used_seconds),
    ])
}

fn current_goal_detail(goal: &AppThreadGoal) -> String {
    let mut parts = vec![goal.objective.clone()];
    parts.extend(goal_usage_parts(
        goal.status,
        goal.tokens_used,
        goal.token_budget,
        goal.time_used_seconds,
    ));
    middle_dot(parts)
}

fn goal_plan_row_name(plan: &ThreadGoalPlan) -> String {
    middle_dot(vec![
        format!("Plan: {}", goal_plan_display_name(plan)),
        plan_status_label(plan.status).to_string(),
        goal_count_summary(
            plan.node_count,
            plan.active_node_count,
            plan.completed_node_count,
            plan.cancelled_node_count,
        ),
        goal_time_part(plan.total_time_used_seconds),
    ])
}

fn goal_plan_selected_detail(plan: &ThreadGoalPlan) -> String {
    let mut parts = vec![plan_usage_summary(plan)];
    if plan.ready_node_count > 0 {
        parts.push(format!("{} ready", plan.ready_node_count));
    }
    if plan.pending_node_count > 0 {
        parts.push(format!("{} waiting", plan.pending_node_count));
    }
    if plan.paused_node_count > 0 {
        parts.push(format!("{} paused", plan.paused_node_count));
    }
    if plan.blocked_node_count > 0 {
        parts.push(format!("{} blocked", plan.blocked_node_count));
    }
    if plan.usage_limited_node_count > 0 {
        parts.push(format!("{} usage limited", plan.usage_limited_node_count));
    }
    if plan.budget_limited_node_count > 0 {
        parts.push(format!("{} budget limited", plan.budget_limited_node_count));
    }
    if plan.cancelled_node_count > 0 {
        parts.push(format!("{} cancelled", plan.cancelled_node_count));
    }
    middle_dot(parts)
}

fn goal_plan_detail_summary(plan: &ThreadGoalPlan) -> String {
    middle_dot(vec![
        plan_status_label(plan.status).to_string(),
        format!(
            "{}/{} goals complete",
            plan.completed_node_count, plan.node_count
        ),
        plan_auto_execute_label(plan.auto_execute).to_string(),
        plan_usage_summary(plan),
    ])
}

fn goal_plan_node_row_name(node: &ThreadGoalPlanNode) -> String {
    let mut label = thread_goal_display_title(node.title.as_deref(), &node.objective);
    if node.status == ThreadGoalPlanNodeStatus::Active {
        label = format!("Current: {label}");
    }
    let mut parts = vec![
        label,
        plan_node_status_label(node.status).to_string(),
        goal_time_part(node.time_used_seconds),
    ];
    if node.ready {
        parts.push("ready".to_string());
    }
    middle_dot(parts)
}

fn goal_plan_node_selected_detail(node: &ThreadGoalPlanNode) -> String {
    let dependencies = if node.depends_on.is_empty() {
        "no dependencies".to_string()
    } else {
        format!(
            "waiting for {}",
            node.depends_on
                .iter()
                .map(|dependency| humanize_goal_key(dependency))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut parts = vec![node.objective.clone(), dependencies];
    parts.extend(goal_usage_parts(
        thread_goal_status_from_node_status(node.status),
        node.tokens_used,
        node.token_budget,
        node.time_used_seconds,
    ));
    middle_dot(parts)
}

fn goal_manager_subtitle(
    current_goal: Option<&AppThreadGoal>,
    goal_plans: &[ThreadGoalPlan],
) -> Option<String> {
    if current_goal.is_none() && goal_plans.is_empty() {
        return None;
    }
    let include_current_goal = goal_plans.is_empty();
    let current_status = if include_current_goal {
        current_goal.map(|goal| goal.status)
    } else {
        None
    };
    let total_planned: i64 = goal_plans.iter().map(|plan| plan.node_count).sum();
    let active = current_status.is_some_and(|status| status == AppThreadGoalStatus::Active) as i64
        + goal_plans
            .iter()
            .map(|plan| plan.active_node_count)
            .sum::<i64>();
    let done = current_status.is_some_and(|status| status == AppThreadGoalStatus::Complete) as i64
        + goal_plans
            .iter()
            .map(|plan| plan.completed_node_count)
            .sum::<i64>();
    let cancelled = current_status.is_some_and(|status| status == AppThreadGoalStatus::Cancelled)
        as i64
        + goal_plans
            .iter()
            .map(|plan| plan.cancelled_node_count)
            .sum::<i64>();
    let blocked = current_status.is_some_and(|status| {
        matches!(
            status,
            AppThreadGoalStatus::Blocked | AppThreadGoalStatus::UsageLimited
        )
    }) as i64
        + goal_plans
            .iter()
            .map(|plan| plan.blocked_node_count + plan.usage_limited_node_count)
            .sum::<i64>();
    let limited = current_status.is_some_and(|status| status == AppThreadGoalStatus::BudgetLimited)
        as i64
        + goal_plans
            .iter()
            .map(|plan| plan.budget_limited_node_count)
            .sum::<i64>();
    let ready: i64 = goal_plans.iter().map(|plan| plan.ready_node_count).sum();

    let mut parts = if total_planned > 0 {
        vec![format!("{total_planned} planned")]
    } else {
        vec!["1 goal".to_string()]
    };
    if active > 0 {
        parts.push(format!("{active} in progress"));
    }
    if ready > 0 {
        parts.push(format!("{ready} ready"));
    }
    if done > 0 {
        parts.push(format!("{done} done"));
    }
    if blocked > 0 {
        parts.push(format!("{blocked} blocked"));
    }
    if limited > 0 {
        parts.push(format!("{limited} limited"));
    }
    if cancelled > 0 {
        parts.push(format!("{cancelled} cancelled"));
    }
    if goal_plans.len() > 1 {
        parts.push(format!("{} plans", goal_plans.len()));
    }
    Some(middle_dot(parts))
}

fn goal_can_cancel(status: AppThreadGoalStatus) -> bool {
    matches!(
        status,
        AppThreadGoalStatus::Active
            | AppThreadGoalStatus::Paused
            | AppThreadGoalStatus::Blocked
            | AppThreadGoalStatus::UsageLimited
            | AppThreadGoalStatus::BudgetLimited
    )
}

fn goal_plan_search_value(plan: &ThreadGoalPlan) -> String {
    let node_text = plan
        .nodes
        .iter()
        .map(|node| format!("{} {}", node.key, node.objective))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {}",
        plan.plan_id,
        goal_plan_display_name(plan),
        node_text
    )
}

fn goal_plan_display_name(plan: &ThreadGoalPlan) -> String {
    plan.nodes
        .iter()
        .find(|node| node.status == ThreadGoalPlanNodeStatus::Active)
        .or_else(|| plan.nodes.iter().find(|node| node.ready))
        .or_else(|| {
            plan.nodes
                .iter()
                .find(|node| node.status != ThreadGoalPlanNodeStatus::Complete)
        })
        .or_else(|| plan.nodes.first())
        .map(|node| node.objective.clone())
        .unwrap_or_else(|| "Goal plan".to_string())
}

fn humanize_goal_key(key: &str) -> String {
    let normalized = key
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = if normalized.is_empty() {
        key.trim().to_string()
    } else {
        normalized
    };
    let mut chars = normalized.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Goal".to_string(),
    }
}

fn plan_usage_summary(plan: &ThreadGoalPlan) -> String {
    goal_usage_parts(
        AppThreadGoalStatus::Active,
        plan.total_tokens_used,
        plan.max_tokens,
        plan.total_time_used_seconds,
    )
    .join(" · ")
}

fn goal_usage_parts(
    status: AppThreadGoalStatus,
    tokens_used: i64,
    token_budget: Option<i64>,
    time_used_seconds: i64,
) -> Vec<String> {
    let mut parts = vec![match token_budget {
        Some(token_budget) => format!(
            "{}/{} tokens",
            format_tokens_compact(tokens_used),
            format_tokens_compact(token_budget)
        ),
        None => format!("{} tokens", format_tokens_compact(tokens_used)),
    }];
    if time_used_seconds > 0 {
        parts.push(goal_time_part(time_used_seconds));
    }
    if status == AppThreadGoalStatus::Cancelled {
        parts.push("cancelled".to_string());
    }
    parts
}

fn goal_time_part(time_used_seconds: i64) -> String {
    format!("time {}", format_goal_elapsed_seconds(time_used_seconds))
}

fn goal_count_summary(node_count: i64, active: i64, complete: i64, cancelled: i64) -> String {
    let mut parts = vec![format!("{node_count} goals")];
    if active > 0 {
        parts.push(format!("{active} current"));
    }
    if complete > 0 {
        parts.push(format!("{complete} done"));
    }
    if cancelled > 0 {
        parts.push(format!("{cancelled} cancelled"));
    }
    parts.join(", ")
}

fn middle_dot(parts: Vec<String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn plan_status_label(status: ThreadGoalPlanStatus) -> &'static str {
    match status {
        ThreadGoalPlanStatus::Active => "active",
        ThreadGoalPlanStatus::Paused => "paused",
        ThreadGoalPlanStatus::Blocked => "blocked",
        ThreadGoalPlanStatus::BudgetLimited => "budget-limited",
        ThreadGoalPlanStatus::Complete => "complete",
        ThreadGoalPlanStatus::Cancelled => "cancelled",
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
        ThreadGoalPlanNodeStatus::Cancelled => "cancelled",
    }
}

fn thread_goal_status_from_node_status(status: ThreadGoalPlanNodeStatus) -> AppThreadGoalStatus {
    match status {
        ThreadGoalPlanNodeStatus::Pending => AppThreadGoalStatus::Paused,
        ThreadGoalPlanNodeStatus::Active => AppThreadGoalStatus::Active,
        ThreadGoalPlanNodeStatus::Paused => AppThreadGoalStatus::Paused,
        ThreadGoalPlanNodeStatus::Blocked => AppThreadGoalStatus::Blocked,
        ThreadGoalPlanNodeStatus::UsageLimited => AppThreadGoalStatus::UsageLimited,
        ThreadGoalPlanNodeStatus::BudgetLimited => AppThreadGoalStatus::BudgetLimited,
        ThreadGoalPlanNodeStatus::Complete => AppThreadGoalStatus::Complete,
        ThreadGoalPlanNodeStatus::Cancelled => AppThreadGoalStatus::Cancelled,
    }
}

fn plan_auto_execute_label(auto_execute: ThreadGoalPlanAutoExecute) -> &'static str {
    match auto_execute {
        ThreadGoalPlanAutoExecute::Off => "off",
        ThreadGoalPlanAutoExecute::ReadyOnly => "ready-only",
        ThreadGoalPlanAutoExecute::AiDirected => "ai-directed",
    }
}
