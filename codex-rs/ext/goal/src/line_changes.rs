use codex_git_utils::GitWorktreeSnapshot;
use codex_git_utils::capture_git_worktree_snapshot;
use codex_git_utils::diff_git_worktree_snapshots;
use codex_git_utils::resolve_git_worktree_private_git_dir;
use codex_state::ThreadGoalLineChangeStats;
use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GoalLineChangeUpdate {
    pub(crate) current_stats: ThreadGoalLineChangeStats,
    pub(crate) persistence_delta: ThreadGoalLineChangeStats,
}

pub(crate) enum BaselineCaptureOutcome {
    Captured(GoalLineChangeBaseline),
    LeaseUnavailable,
    SnapshotUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoalLineChangeOwner {
    thread_id: String,
    goal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoalWorktreeLineChangeLease {
    private_git_dir: PathBuf,
    owner: GoalLineChangeOwner,
}

#[derive(Debug)]
struct GoalWorktreeLineChangeLeaseEntry {
    owner: GoalLineChangeOwner,
    ref_count: usize,
    _lock_file: File,
}

static WORKTREE_LINE_CHANGE_LEASES: LazyLock<
    Mutex<HashMap<PathBuf, GoalWorktreeLineChangeLeaseEntry>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) async fn capture_baseline_outcome(
    cwd: &Path,
    goal: &codex_state::ThreadGoal,
) -> BaselineCaptureOutcome {
    let owner = GoalLineChangeOwner {
        thread_id: goal.thread_id.to_string(),
        goal_id: goal.goal_id.clone(),
    };
    let Some(lease) = GoalWorktreeLineChangeLease::acquire(cwd, owner).await else {
        return BaselineCaptureOutcome::LeaseUnavailable;
    };
    let Ok(worktree) = capture_git_worktree_snapshot(cwd).await else {
        return BaselineCaptureOutcome::SnapshotUnavailable;
    };
    BaselineCaptureOutcome::Captured(GoalLineChangeBaseline {
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
    match capture_baseline_outcome(cwd.as_path(), goal).await {
        BaselineCaptureOutcome::Captured(baseline) => {
            accounting.set_turn_line_change_baseline(&turn_id, &goal.goal_id, baseline);
        }
        BaselineCaptureOutcome::LeaseUnavailable => {
            accounting.set_turn_line_change_baseline_retry_pending(
                &turn_id,
                &goal.goal_id,
                /*retry_pending*/ true,
            );
        }
        BaselineCaptureOutcome::SnapshotUnavailable => {
            accounting.set_turn_line_change_baseline_retry_pending(
                &turn_id,
                &goal.goal_id,
                /*retry_pending*/ false,
            );
        }
    }
}

impl GoalLineChangeBaseline {
    pub(crate) fn persisted_stats(&self) -> ThreadGoalLineChangeStats {
        self.persisted_stats
    }
}

impl GoalWorktreeLineChangeLease {
    async fn acquire(cwd: &Path, owner: GoalLineChangeOwner) -> Option<Self> {
        let private_git_dir = resolve_git_worktree_private_git_dir(cwd).await.ok()?;
        let mut leases = worktree_line_change_leases();
        match leases.get_mut(private_git_dir.as_path()) {
            Some(entry) if entry.owner == owner => {
                entry.ref_count = entry.ref_count.saturating_add(1);
            }
            Some(_) => return None,
            None => {
                let lock_file = try_open_line_change_lock(private_git_dir.as_path())?;
                leases.insert(
                    private_git_dir.clone(),
                    GoalWorktreeLineChangeLeaseEntry {
                        owner: owner.clone(),
                        ref_count: 1,
                        _lock_file: lock_file,
                    },
                );
            }
        }
        Some(Self {
            private_git_dir,
            owner,
        })
    }
}

impl Drop for GoalWorktreeLineChangeLease {
    fn drop(&mut self) {
        let mut leases = worktree_line_change_leases();
        let Some(entry) = leases.get_mut(self.private_git_dir.as_path()) else {
            return;
        };
        if entry.owner != self.owner {
            return;
        }
        if entry.ref_count > 1 {
            entry.ref_count -= 1;
        } else {
            leases.remove(self.private_git_dir.as_path());
        }
    }
}

fn try_open_line_change_lock(private_git_dir: &Path) -> Option<File> {
    // Keep the file in place after release; dropping the handle releases the
    // OS lock, and a stable inode avoids replacement races between processes.
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(private_git_dir.join("codewith-line-change-accounting.lock"))
        .ok()?;
    match lock_file.try_lock() {
        Ok(()) => Some(lock_file),
        Err(_) => None,
    }
}

fn worktree_line_change_leases()
-> std::sync::MutexGuard<'static, HashMap<PathBuf, GoalWorktreeLineChangeLeaseEntry>> {
    WORKTREE_LINE_CHANGE_LEASES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Returns the current local totals and the signed persistence delta implied by
/// the current worktree.
///
/// The fixed worktree snapshot makes attribution independent of pre-existing
/// changes and of intermediate accounting flushes. `None` means the snapshot
/// was unavailable or the totals have already been persisted.
pub(crate) async fn update_since_baseline(
    cwd: &Path,
    baseline: &GoalLineChangeBaseline,
    last_accounted_stats: ThreadGoalLineChangeStats,
) -> Option<GoalLineChangeUpdate> {
    let current = capture_git_worktree_snapshot(cwd).await.ok()?;
    let delta = diff_git_worktree_snapshots(&baseline.worktree, &current)
        .await
        .ok()?;
    let current_stats = ThreadGoalLineChangeStats {
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
    let persistence_delta = ThreadGoalLineChangeStats {
        lines_added: current_stats
            .lines_added
            .saturating_sub(last_accounted_stats.lines_added),
        lines_deleted: current_stats
            .lines_deleted
            .saturating_sub(last_accounted_stats.lines_deleted),
    };
    (persistence_delta.lines_added != 0 || persistence_delta.lines_deleted != 0).then_some(
        GoalLineChangeUpdate {
            current_stats,
            persistence_delta,
        },
    )
}
