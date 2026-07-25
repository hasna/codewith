use codex_git_utils::GitWorktreeSnapshot;
use codex_git_utils::capture_git_worktree_snapshot;
use codex_git_utils::diff_git_worktree_snapshots;
use codex_state::ThreadGoalLineChangeStats;
use std::path::Path;

use crate::accounting::GoalAccountingState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalLineChangeBaseline {
    worktree: GitWorktreeSnapshot,
    persisted_stats: ThreadGoalLineChangeStats,
}

pub(crate) async fn capture_baseline(
    cwd: &Path,
    goal: &codex_state::ThreadGoal,
) -> Option<GoalLineChangeBaseline> {
    let worktree = capture_git_worktree_snapshot(cwd).await.ok()?;
    Some(GoalLineChangeBaseline {
        worktree,
        persisted_stats: ThreadGoalLineChangeStats {
            lines_added: goal.lines_added,
            lines_deleted: goal.lines_deleted,
        },
    })
}

pub(crate) async fn establish_current_turn_baseline(
    accounting: &GoalAccountingState,
    goal: &codex_state::ThreadGoal,
) {
    let Some((turn_id, cwd)) = accounting.current_turn_line_change_context(&goal.goal_id) else {
        return;
    };
    let Some(baseline) = capture_baseline(cwd.as_path(), goal).await else {
        return;
    };
    accounting.set_turn_line_change_baseline(&turn_id, &goal.goal_id, baseline);
}

impl GoalLineChangeBaseline {
    pub(crate) fn persisted_stats(&self) -> ThreadGoalLineChangeStats {
        self.persisted_stats
    }
}

/// Returns the goal-wide line-change totals implied by the current worktree.
///
/// The fixed worktree snapshot makes attribution independent of pre-existing
/// changes and of intermediate accounting flushes. `None` means the snapshot
/// was unavailable or the totals have already been persisted.
pub(crate) async fn stats_since_baseline(
    cwd: &Path,
    baseline: &GoalLineChangeBaseline,
    last_accounted_stats: ThreadGoalLineChangeStats,
) -> Option<ThreadGoalLineChangeStats> {
    let current = capture_git_worktree_snapshot(cwd).await.ok()?;
    let delta = diff_git_worktree_snapshots(&baseline.worktree, &current)
        .await
        .ok()?;
    let stats = ThreadGoalLineChangeStats {
        lines_added: baseline
            .persisted_stats
            .lines_added
            .max(0)
            .saturating_add(i64::try_from(delta.lines_added).unwrap_or(i64::MAX)),
        lines_deleted: baseline
            .persisted_stats
            .lines_deleted
            .max(0)
            .saturating_add(i64::try_from(delta.lines_deleted).unwrap_or(i64::MAX)),
    };
    (stats != last_accounted_stats).then_some(stats)
}
