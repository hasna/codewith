//! Helpers for mapping thread-goal state into the compact status-line indicator.

use codex_app_server_protocol::ThreadGoal as AppThreadGoal;
use codex_app_server_protocol::ThreadGoalPlan as AppThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_app_server_protocol::ThreadGoalStatus as AppThreadGoalStatus;
use codex_protocol::protocol::thread_goal_display_title;
use std::time::Instant;

use crate::bottom_pane::GoalStatusIndicator;
use crate::goal_display::format_goal_elapsed_seconds;
use crate::status::format_tokens_compact;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct GoalStatusState {
    goal: AppThreadGoal,
    observed_at: Instant,
}

impl GoalStatusState {
    pub(super) fn new(goal: AppThreadGoal, observed_at: Instant) -> Self {
        Self { goal, observed_at }
    }

    pub(super) fn updated(
        previous: Option<&Self>,
        mut goal: AppThreadGoal,
        observed_at: Instant,
        active_turn_started_at: Option<Instant>,
    ) -> Self {
        if let Some(previous) = previous
            && previous.goal.goal_id == goal.goal_id
        {
            goal.time_used_seconds = goal
                .time_used_seconds
                .max(previous.time_used_seconds_at(observed_at, active_turn_started_at));
        }
        Self::new(goal, observed_at)
    }

    pub(super) fn is_active(&self) -> bool {
        self.goal.status == AppThreadGoalStatus::Active
    }

    pub(super) fn display_title(&self) -> String {
        thread_goal_display_title(self.goal.title.as_deref(), &self.goal.objective)
    }

    pub(super) fn indicator(
        &self,
        now: Instant,
        active_turn_started_at: Option<Instant>,
    ) -> Option<GoalStatusIndicator> {
        let mut goal = self.goal.clone();
        goal.time_used_seconds = self.time_used_seconds_at(now, active_turn_started_at);
        goal_status_indicator_from_app_goal(&goal)
    }

    fn time_used_seconds_at(&self, now: Instant, active_turn_started_at: Option<Instant>) -> i64 {
        let mut time_used_seconds = self.goal.time_used_seconds;
        if self.goal.status == AppThreadGoalStatus::Active
            && let Some(active_turn_started_at) = active_turn_started_at
        {
            let baseline = self.observed_at.max(active_turn_started_at);
            let active_seconds = now.saturating_duration_since(baseline).as_secs();
            time_used_seconds =
                time_used_seconds.saturating_add(i64::try_from(active_seconds).unwrap_or(i64::MAX));
        }
        time_used_seconds
    }
}

pub(super) fn goal_status_indicator_from_app_goal(
    goal: &AppThreadGoal,
) -> Option<GoalStatusIndicator> {
    match goal.status {
        AppThreadGoalStatus::Active => Some(GoalStatusIndicator::Active {
            usage: active_goal_usage(goal.token_budget, goal.tokens_used, goal.time_used_seconds),
        }),
        AppThreadGoalStatus::Paused => Some(GoalStatusIndicator::Paused),
        AppThreadGoalStatus::Blocked => Some(GoalStatusIndicator::Blocked),
        AppThreadGoalStatus::UsageLimited => Some(GoalStatusIndicator::UsageLimited),
        AppThreadGoalStatus::Deferred => Some(GoalStatusIndicator::Deferred),
        AppThreadGoalStatus::BudgetLimited => Some(GoalStatusIndicator::BudgetLimited {
            usage: stopped_goal_budget_usage(goal.token_budget, goal.tokens_used),
        }),
        AppThreadGoalStatus::Complete => Some(GoalStatusIndicator::Complete {
            usage: Some(completed_goal_usage(
                goal.token_budget,
                goal.tokens_used,
                goal.time_used_seconds,
            )),
        }),
        AppThreadGoalStatus::Cancelled => Some(GoalStatusIndicator::Cancelled {
            usage: Some(completed_goal_usage(
                goal.token_budget,
                goal.tokens_used,
                goal.time_used_seconds,
            )),
        }),
    }
}

pub(super) fn goal_status_indicator_with_goal_plan(
    indicator: GoalStatusIndicator,
    goal_plan: Option<&AppThreadGoalPlan>,
) -> GoalStatusIndicator {
    let GoalStatusIndicator::Active { usage } = indicator else {
        return indicator;
    };
    let Some(goal_plan) = goal_plan else {
        return GoalStatusIndicator::Active { usage };
    };
    if goal_plan.status != ThreadGoalPlanStatus::Active || goal_plan.node_count <= 0 {
        return GoalStatusIndicator::Active { usage };
    }
    let Some((index, _)) = goal_plan
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.status == ThreadGoalPlanNodeStatus::Active)
    else {
        return GoalStatusIndicator::Active { usage };
    };
    let current_goal = i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1);

    GoalStatusIndicator::ActivePlan {
        usage,
        current_goal,
        total_goals: goal_plan.node_count.max(current_goal),
    }
}

fn active_goal_usage(
    token_budget: Option<i64>,
    tokens_used: i64,
    time_used_seconds: i64,
) -> Option<String> {
    if let Some(token_budget) = token_budget {
        return Some(format!(
            "{} / {}",
            format_tokens_compact(tokens_used),
            format_tokens_compact(token_budget)
        ));
    }

    Some(format_goal_elapsed_seconds(time_used_seconds))
}

fn stopped_goal_budget_usage(token_budget: Option<i64>, tokens_used: i64) -> Option<String> {
    token_budget.map(|token_budget| {
        format!(
            "{} / {} tokens",
            format_tokens_compact(tokens_used),
            format_tokens_compact(token_budget)
        )
    })
}

fn completed_goal_usage(
    token_budget: Option<i64>,
    tokens_used: i64,
    time_used_seconds: i64,
) -> String {
    if token_budget.is_some() {
        return format!("{} tokens", format_tokens_compact(tokens_used));
    }

    format_goal_elapsed_seconds(time_used_seconds)
}

#[cfg(test)]
mod tests {
    use super::GoalStatusState;
    use super::active_goal_usage;
    use super::completed_goal_usage;
    use super::goal_status_indicator_with_goal_plan;
    use super::stopped_goal_budget_usage;
    use crate::bottom_pane::GoalStatusIndicator;
    use codex_app_server_protocol::ThreadGoal as AppThreadGoal;
    use codex_app_server_protocol::ThreadGoalPlan as AppThreadGoalPlan;
    use codex_app_server_protocol::ThreadGoalPlanAutoExecute;
    use codex_app_server_protocol::ThreadGoalPlanNode;
    use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
    use codex_app_server_protocol::ThreadGoalPlanStatus;
    use codex_app_server_protocol::ThreadGoalStatus as AppThreadGoalStatus;
    use pretty_assertions::assert_eq;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn active_goal_usage_prefers_token_budget() {
        assert_eq!(
            active_goal_usage(
                Some(50_000),
                /*tokens_used*/ 12_500,
                /*time_used_seconds*/ 90
            ),
            Some("12.5K / 50K".to_string())
        );
    }

    #[test]
    fn active_goal_usage_reports_time_without_budget() {
        assert_eq!(
            active_goal_usage(
                /*token_budget*/ None, /*tokens_used*/ 12_500,
                /*time_used_seconds*/ 120,
            ),
            Some("2m".to_string())
        );
    }

    #[test]
    fn stopped_goal_budget_usage_reports_budgeted_tokens() {
        assert_eq!(
            stopped_goal_budget_usage(Some(50_000), /*tokens_used*/ 63_876),
            Some("63.9K / 50K tokens".to_string())
        );
    }

    #[test]
    fn stopped_goal_budget_usage_omits_unbudgeted_usage() {
        assert_eq!(
            stopped_goal_budget_usage(/*token_budget*/ None, /*tokens_used*/ 12_500),
            None
        );
    }

    #[test]
    fn completed_goal_usage_reports_tokens_when_budgeted() {
        assert_eq!(
            completed_goal_usage(
                Some(50_000),
                /*tokens_used*/ 40_000,
                /*time_used_seconds*/ 120,
            ),
            "40K tokens".to_string()
        );
    }

    #[test]
    fn completed_goal_usage_reports_time_without_token_budget() {
        assert_eq!(
            completed_goal_usage(
                /*token_budget*/ None, /*tokens_used*/ 40_000,
                /*time_used_seconds*/ 36_720,
            ),
            "10h 12m".to_string()
        );
    }

    #[test]
    fn active_goal_status_includes_current_turn_elapsed_time() {
        let observed_at = Instant::now();
        let state = active_goal_state(observed_at, /*time_used_seconds*/ 60);

        assert_eq!(
            state.indicator(
                observed_at + Duration::from_secs(60),
                Some(observed_at - Duration::from_secs(120)),
            ),
            Some(GoalStatusIndicator::Active {
                usage: Some("2m".to_string())
            })
        );
    }

    #[test]
    fn active_goal_status_does_not_count_idle_time_before_turn_start() {
        let observed_at = Instant::now();
        let active_turn_started_at = observed_at + Duration::from_secs(120);
        let state = active_goal_state(observed_at, /*time_used_seconds*/ 60);

        assert_eq!(
            state.indicator(
                active_turn_started_at + Duration::from_secs(60),
                Some(active_turn_started_at),
            ),
            Some(GoalStatusIndicator::Active {
                usage: Some("2m".to_string())
            })
        );
    }

    #[test]
    fn active_goal_status_includes_goal_plan_position() {
        let indicator = GoalStatusIndicator::Active {
            usage: Some("40s".to_string()),
        };
        let goal_plan = test_goal_plan(&[
            ThreadGoalPlanNodeStatus::Complete,
            ThreadGoalPlanNodeStatus::Active,
            ThreadGoalPlanNodeStatus::Pending,
            ThreadGoalPlanNodeStatus::Pending,
        ]);

        assert_eq!(
            goal_status_indicator_with_goal_plan(indicator, Some(&goal_plan)),
            GoalStatusIndicator::ActivePlan {
                usage: Some("40s".to_string()),
                current_goal: 2,
                total_goals: 4,
            }
        );
    }

    #[test]
    fn same_goal_update_keeps_displayed_elapsed_time_monotonic() {
        let observed_at = Instant::now();
        let active_turn_started_at = observed_at;
        let previous = active_goal_state(observed_at, /*time_used_seconds*/ 60);
        let mut stale_goal = active_goal(/*time_used_seconds*/ 0);
        stale_goal.goal_id = "goal".to_string();

        let updated = GoalStatusState::updated(
            Some(&previous),
            stale_goal,
            observed_at + Duration::from_secs(90),
            Some(active_turn_started_at),
        );

        assert_eq!(
            updated.indicator(
                observed_at + Duration::from_secs(90),
                Some(active_turn_started_at),
            ),
            Some(GoalStatusIndicator::Active {
                usage: Some("2m".to_string())
            })
        );
    }

    #[test]
    fn new_goal_update_can_restart_elapsed_time() {
        let observed_at = Instant::now();
        let previous = active_goal_state(observed_at, /*time_used_seconds*/ 60);
        let mut next_goal = active_goal(/*time_used_seconds*/ 0);
        next_goal.goal_id = "next-goal".to_string();

        let updated = GoalStatusState::updated(
            Some(&previous),
            next_goal,
            observed_at,
            /*active_turn_started_at*/ None,
        );

        assert_eq!(
            updated.indicator(observed_at, /*active_turn_started_at*/ None),
            Some(GoalStatusIndicator::Active {
                usage: Some("0s".to_string())
            })
        );
    }

    fn active_goal_state(observed_at: Instant, time_used_seconds: i64) -> GoalStatusState {
        GoalStatusState::new(active_goal(time_used_seconds), observed_at)
    }

    fn active_goal(time_used_seconds: i64) -> AppThreadGoal {
        AppThreadGoal {
            thread_id: "thread".to_string(),
            goal_id: "goal".to_string(),
            objective: "do the thing".to_string(),
            title: None,
            status: AppThreadGoalStatus::Active,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn test_goal_plan(statuses: &[ThreadGoalPlanNodeStatus]) -> AppThreadGoalPlan {
        let count_status = |needle| {
            i64::try_from(statuses.iter().filter(|status| **status == needle).count())
                .unwrap_or(i64::MAX)
        };

        AppThreadGoalPlan {
            plan_id: "plan-1".to_string(),
            thread_id: "thread-1".to_string(),
            status: ThreadGoalPlanStatus::Active,
            auto_execute: ThreadGoalPlanAutoExecute::AiDirected,
            max_tokens: None,
            total_tokens_used: 0,
            total_time_used_seconds: 0,
            remaining_tokens: None,
            node_count: i64::try_from(statuses.len()).unwrap_or(i64::MAX),
            completed_node_count: count_status(ThreadGoalPlanNodeStatus::Complete),
            ready_node_count: 0,
            active_node_count: count_status(ThreadGoalPlanNodeStatus::Active),
            pending_node_count: count_status(ThreadGoalPlanNodeStatus::Pending),
            deferred_node_count: count_status(ThreadGoalPlanNodeStatus::Deferred),
            paused_node_count: 0,
            blocked_node_count: 0,
            usage_limited_node_count: 0,
            budget_limited_node_count: 0,
            cancelled_node_count: 0,
            created_at: 0,
            updated_at: 0,
            nodes: statuses
                .iter()
                .enumerate()
                .map(|(index, status)| ThreadGoalPlanNode {
                    node_id: format!("node-{index}"),
                    plan_id: "plan-1".to_string(),
                    thread_id: "thread-1".to_string(),
                    assigned_thread_id: "thread-1".to_string(),
                    key: format!("goal-{index}"),
                    sequence: i64::try_from(index).unwrap_or(i64::MAX),
                    priority: 0,
                    objective: format!("Goal {index}"),
                    title: None,
                    status: *status,
                    ready: false,
                    token_budget: None,
                    tokens_used: 0,
                    time_used_seconds: 0,
                    projected_goal_id: None,
                    depends_on: Vec::new(),
                    created_at: 0,
                    updated_at: 0,
                })
                .collect(),
        }
    }
}
