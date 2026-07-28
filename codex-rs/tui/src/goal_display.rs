use crate::status::format_tokens_compact;
use codex_app_server_protocol::ThreadGoal;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_protocol::protocol::thread_goal_display_title;

pub(crate) const GOAL_USAGE: &str =
    "Usage: /goal [<objective>|cancel|clear|defer|edit|pause|resume]";

pub(crate) fn format_goal_elapsed_seconds(seconds: i64) -> String {
    let seconds = seconds.max(0) as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }

    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }

    let hours = minutes / 60;
    let remaining_minutes = minutes % 60;
    if hours >= 24 {
        let days = hours / 24;
        let remaining_hours = hours % 24;
        return format!("{days}d {remaining_hours}h {remaining_minutes}m");
    }

    if remaining_minutes == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {remaining_minutes}m")
    }
}

pub(crate) fn goal_status_label(status: ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::Blocked => "blocked",
        ThreadGoalStatus::UsageLimited => "usage limited",
        ThreadGoalStatus::BudgetLimited => "limited by budget",
        ThreadGoalStatus::Deferred => "deferred",
        ThreadGoalStatus::Complete => "complete",
        ThreadGoalStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn goal_usage_summary(goal: &ThreadGoal) -> String {
    let display_title = thread_goal_display_title(goal.title.as_deref(), &goal.objective);
    let mut parts = vec![format!("Goal: {display_title}")];
    if goal.time_used_seconds > 0 {
        parts.push(format!(
            "Time: {}.",
            format_goal_elapsed_seconds(goal.time_used_seconds)
        ));
    }
    if let Some(token_budget) = goal.token_budget {
        parts.push(format!(
            "Tokens: {}/{}.",
            format_tokens_compact(goal.tokens_used),
            format_tokens_compact(token_budget)
        ));
    }
    parts.push(format!(
        "LOC: {}.",
        format_goal_line_changes(goal.lines_added, goal.lines_deleted)
    ));
    parts.join(" ")
}

pub(crate) fn format_goal_line_changes(lines_added: i64, lines_deleted: i64) -> String {
    let lines_added = lines_added.max(0);
    let lines_deleted = lines_deleted.max(0);
    if lines_added == 0 && lines_deleted == 0 {
        return "0 LOC".to_string();
    }
    format!(
        "+{} -{} LOC",
        format_tokens_compact(lines_added),
        format_tokens_compact(lines_deleted)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ThreadGoal;
    use codex_app_server_protocol::ThreadGoalStatus;
    use pretty_assertions::assert_eq;

    #[test]
    fn format_goal_elapsed_seconds_is_compact() {
        assert_eq!(format_goal_elapsed_seconds(/*seconds*/ 0), "0s");
        assert_eq!(format_goal_elapsed_seconds(/*seconds*/ 59), "59s");
        assert_eq!(format_goal_elapsed_seconds(/*seconds*/ 60), "1m");
        assert_eq!(format_goal_elapsed_seconds(30 * 60), "30m");
        assert_eq!(format_goal_elapsed_seconds(90 * 60), "1h 30m");
        assert_eq!(format_goal_elapsed_seconds(2 * 60 * 60), "2h");
        let just_before_one_day = 24 * 60 * 60 - 1;
        assert_eq!(format_goal_elapsed_seconds(just_before_one_day), "23h 59m");

        let one_day = 24 * 60 * 60;
        assert_eq!(format_goal_elapsed_seconds(one_day), "1d 0h 0m");

        let almost_three_days = 2 * 24 * 60 * 60 + 23 * 60 * 60 + 42 * 60;
        assert_eq!(format_goal_elapsed_seconds(almost_three_days), "2d 23h 42m");
    }

    fn test_thread_goal(token_budget: Option<i64>, tokens_used: i64) -> ThreadGoal {
        ThreadGoal {
            thread_id: "thread-1".to_string(),
            goal_id: "goal-1".to_string(),
            objective: "Complete the task described in ../gameboy-long-running-prompt5.txt"
                .to_string(),
            title: None,
            status: ThreadGoalStatus::BudgetLimited,
            token_budget,
            tokens_used,
            time_used_seconds: 120,
            lines_added: 0,
            lines_deleted: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn goal_usage_summary_formats_time_and_budgeted_tokens() {
        assert_eq!(
            goal_usage_summary(&test_thread_goal(
                /*token_budget*/ Some(50_000),
                /*tokens_used*/ 63_876,
            )),
            "Goal: Complete the task described in Time: 2m. Tokens: 63.9K/50K. LOC: 0 LOC."
        );
    }

    #[test]
    fn format_goal_line_changes_shows_added_and_deleted_lines() {
        assert_eq!(format_goal_line_changes(12, 3), "+12 -3 LOC");
        assert_eq!(format_goal_line_changes(12_345, 1_234), "+12.3K -1.23K LOC");
        assert_eq!(format_goal_line_changes(0, 0), "0 LOC");
    }
}
