use codex_git_utils::GitWorktreeSnapshot;
use codex_git_utils::capture_git_worktree_snapshot;
use codex_git_utils::diff_git_worktree_snapshots;
use codex_git_utils::resolve_git_worktree_root;
use codex_state::ThreadGoalLineChangeStats;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::PoisonError;

use crate::accounting::GoalAccountingState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalLineChangeBaseline {
    worktree: GitWorktreeSnapshot,
    persisted_stats: ThreadGoalLineChangeStats,
    _lease: Arc<GoalWorktreeLineChangeLease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoalLineChangeOwner {
    thread_id: String,
    goal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoalWorktreeLineChangeLease {
    worktree_root: PathBuf,
    owner: GoalLineChangeOwner,
}

#[derive(Debug)]
struct GoalWorktreeLineChangeLeaseEntry {
    owner: GoalLineChangeOwner,
    ref_count: usize,
}

static WORKTREE_LINE_CHANGE_LEASES: LazyLock<
    Mutex<HashMap<PathBuf, GoalWorktreeLineChangeLeaseEntry>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) async fn capture_baseline(
    cwd: &Path,
    goal: &codex_state::ThreadGoal,
) -> Option<GoalLineChangeBaseline> {
    let owner = GoalLineChangeOwner {
        thread_id: goal.thread_id.to_string(),
        goal_id: goal.goal_id.clone(),
    };
    let lease = GoalWorktreeLineChangeLease::acquire(cwd, owner).await?;
    let worktree = capture_git_worktree_snapshot(cwd).await.ok()?;
    Some(GoalLineChangeBaseline {
        worktree,
        persisted_stats: ThreadGoalLineChangeStats {
            lines_added: goal.lines_added,
            lines_deleted: goal.lines_deleted,
        },
        _lease: Arc::new(lease),
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

impl GoalWorktreeLineChangeLease {
    async fn acquire(cwd: &Path, owner: GoalLineChangeOwner) -> Option<Self> {
        let worktree_root = resolve_git_worktree_root(cwd).await.ok()?;
        let mut leases = worktree_line_change_leases();
        match leases.get_mut(worktree_root.as_path()) {
            Some(entry) if entry.owner == owner => {
                entry.ref_count = entry.ref_count.saturating_add(1);
            }
            Some(_) => return None,
            None => {
                leases.insert(
                    worktree_root.clone(),
                    GoalWorktreeLineChangeLeaseEntry {
                        owner: owner.clone(),
                        ref_count: 1,
                    },
                );
            }
        }
        Some(Self {
            worktree_root,
            owner,
        })
    }
}

impl Drop for GoalWorktreeLineChangeLease {
    fn drop(&mut self) {
        let mut leases = worktree_line_change_leases();
        let Some(entry) = leases.get_mut(self.worktree_root.as_path()) else {
            return;
        };
        if entry.owner != self.owner {
            return;
        }
        if entry.ref_count > 1 {
            entry.ref_count -= 1;
        } else {
            leases.remove(self.worktree_root.as_path());
        }
    }
}

fn worktree_line_change_leases()
-> std::sync::MutexGuard<'static, HashMap<PathBuf, GoalWorktreeLineChangeLeaseEntry>> {
    WORKTREE_LINE_CHANGE_LEASES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
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
