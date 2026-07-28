#![allow(dead_code)]

#[path = "../src/accounting.rs"]
mod accounting;
#[path = "../src/line_changes.rs"]
mod line_changes;

use accounting::BudgetLimitedGoalDisposition;
use accounting::GoalAccountingState;
use anyhow::Result;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ModeKind;
use codex_protocol::protocol::TokenUsage;
use codex_state::ThreadGoalLineChangeStats;
use codex_state::ThreadGoalStatus;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const LINE_CHANGE_LEASE_HELPER_REPO_ENV: &str = "CODEWITH_LINE_CHANGE_LEASE_HELPER_REPO";
const LINE_CHANGE_LEASE_HELPER_EXPECT_ENV: &str = "CODEWITH_LINE_CHANGE_LEASE_HELPER_EXPECT";
const LINE_CHANGE_LEASE_HELPER_TEST: &str = "subprocess_shared_cwd_line_change_lease_helper";

#[test]
fn goal_accounting_uses_turn_start_baseline_for_exact_deltas() {
    let state = GoalAccountingState::default();
    state.start_turn(
        "turn-1",
        ModeKind::Default,
        &token_usage(
            /*input_tokens*/ 100, /*cached_input_tokens*/ 10, /*output_tokens*/ 30,
            /*reasoning_output_tokens*/ 5, /*total_tokens*/ 135,
        ),
        /*local_cwd*/ None,
    );

    let recorded = state
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 120, /*cached_input_tokens*/ 14,
                /*output_tokens*/ 42, /*reasoning_output_tokens*/ 8,
                /*total_tokens*/ 162,
            ),
        )
        .expect("token delta should be recorded");

    assert_eq!(28, recorded.turn_delta);
    assert_eq!(28, recorded.thread_unflushed_delta);
}

#[test]
fn goal_accounting_ignores_plan_mode_turns() {
    let state = GoalAccountingState::default();
    state.start_turn(
        "turn-1",
        ModeKind::Plan,
        &TokenUsage::default(),
        /*local_cwd*/ None,
    );

    let recorded = state.record_token_usage(
        "turn-1",
        &token_usage(
            /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
            /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
        ),
    );

    assert_eq!(None, recorded);
}

#[tokio::test]
async fn goal_accounting_remembers_persisted_line_change_totals() -> anyhow::Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    run_git(repo, &["init"]).await?;
    std::fs::write(repo.join("tracked.txt"), "baseline\n")?;
    run_git(repo, &["add", "."]).await?;
    run_git(
        repo,
        &[
            "-c",
            "user.name=Codewith Test",
            "-c",
            "user.email=codewith@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-m",
            "initial",
        ],
    )
    .await?;

    let state = GoalAccountingState::default();
    state.start_turn(
        "turn-1",
        ModeKind::Default,
        &TokenUsage::default(),
        Some(repo.to_path_buf()),
    );
    let now = Utc::now();
    let goal = codex_state::ThreadGoal {
        thread_id: ThreadId::from_string("00000000-0000-4000-8000-000000000001")?,
        goal_id: "goal-1".to_string(),
        objective: "Track changes".to_string(),
        title: None,
        status: ThreadGoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        lines_added: 3,
        lines_deleted: 1,
        created_at: now,
        updated_at: now,
    };
    state.mark_turn_goal_active("turn-1", goal.goal_id.clone());
    line_changes::establish_current_turn_baseline(&state, &goal).await;

    let first = state
        .progress_snapshot("turn-1")
        .expect("line-change baseline should produce a snapshot");
    assert_eq!(
        Some(ThreadGoalLineChangeStats {
            lines_added: 3,
            lines_deleted: 1,
        }),
        first
            .line_changes
            .as_ref()
            .map(|changes| changes.last_accounted_stats)
    );
    let persisted = ThreadGoalLineChangeStats {
        lines_added: 8,
        lines_deleted: 2,
    };
    state.mark_progress_accounted_for_status(
        "turn-1",
        &first,
        Some(persisted),
        ThreadGoalStatus::Active,
        BudgetLimitedGoalDisposition::KeepActive,
    );
    let second = state
        .progress_snapshot("turn-1")
        .expect("active baseline should remain available");
    assert_eq!(
        Some(persisted),
        second
            .line_changes
            .map(|changes| changes.last_accounted_stats)
    );
    Ok(())
}

#[tokio::test]
async fn update_since_baseline_counts_only_changes_after_baseline() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(repo, "src/lib.rs", "fn base() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;

    write_file(repo, "src/lib.rs", "fn base() {}\nfn before_goal() {}\n")?;
    write_file(repo, "src/before.rs", "fn before_untracked() {}\n")?;
    let baseline = capture_baseline(repo, &test_goal(/*lines_added*/ 3, /*lines_deleted*/ 1)?)
        .await
        .ok_or_else(|| anyhow::anyhow!("baseline should capture"))?;

    write_file(
        repo,
        "src/lib.rs",
        "fn base() {}\nfn before_goal() {}\nfn after_goal() {}\n",
    )?;
    write_file(repo, "src/before.rs", "fn before_untracked() {}\n")?;
    write_file(
        repo,
        "src/after.rs",
        "fn after_untracked() {}\nfn more_after() {}\n",
    )?;

    assert_eq!(
        Some(line_change_update(
            /*current_lines_added*/ 6, /*current_lines_deleted*/ 1,
            /*persistence_lines_added*/ 3, /*persistence_lines_deleted*/ 0,
        )),
        line_changes::update_since_baseline(repo, &baseline, baseline.persisted_stats()).await
    );
    Ok(())
}

#[tokio::test]
async fn update_since_baseline_counts_deleted_tracked_lines() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(
        repo,
        "src/lib.rs",
        "fn one() {}\nfn two() {}\nfn three() {}\n",
    )?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;
    let baseline = capture_baseline(repo, &test_goal(/*lines_added*/ 0, /*lines_deleted*/ 0)?)
        .await
        .ok_or_else(|| anyhow::anyhow!("baseline should capture"))?;

    write_file(repo, "src/lib.rs", "fn one() {}\nfn three() {}\n")?;

    assert_eq!(
        Some(line_change_update(
            /*current_lines_added*/ 0, /*current_lines_deleted*/ 1,
            /*persistence_lines_added*/ 0, /*persistence_lines_deleted*/ 1,
        )),
        line_changes::update_since_baseline(repo, &baseline, baseline.persisted_stats()).await
    );
    Ok(())
}

#[tokio::test]
async fn update_since_baseline_reports_nothing_when_worktree_is_untouched() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(repo, "src/lib.rs", "fn base() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;
    let baseline = capture_baseline(repo, &test_goal(/*lines_added*/ 9, /*lines_deleted*/ 2)?)
        .await
        .ok_or_else(|| anyhow::anyhow!("baseline should capture"))?;

    assert_eq!(
        None,
        line_changes::update_since_baseline(repo, &baseline, baseline.persisted_stats()).await
    );
    Ok(())
}

#[tokio::test]
async fn update_since_baseline_counts_same_file_replacements_once() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(repo, "src/lib.rs", "fn one() {}\nfn before() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;
    let baseline = capture_baseline(repo, &test_goal(/*lines_added*/ 4, /*lines_deleted*/ 2)?)
        .await
        .ok_or_else(|| anyhow::anyhow!("baseline should capture"))?;

    write_file(repo, "src/lib.rs", "fn one() {}\nfn after() {}\n")?;
    let expected = ThreadGoalLineChangeStats {
        lines_added: 5,
        lines_deleted: 3,
    };
    assert_eq!(
        Some(line_change_update(
            /*current_lines_added*/ 5, /*current_lines_deleted*/ 3,
            /*persistence_lines_added*/ 1, /*persistence_lines_deleted*/ 1,
        )),
        line_changes::update_since_baseline(repo, &baseline, baseline.persisted_stats()).await
    );
    assert_eq!(
        None,
        line_changes::update_since_baseline(repo, &baseline, expected).await
    );
    Ok(())
}

#[tokio::test]
async fn update_since_baseline_reports_signed_delta_when_change_is_reverted() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    let baseline_contents = "fn one() {}\nfn before() {}\n";
    write_file(repo, "src/lib.rs", baseline_contents)?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;
    let baseline = capture_baseline(repo, &test_goal(/*lines_added*/ 4, /*lines_deleted*/ 2)?)
        .await
        .ok_or_else(|| anyhow::anyhow!("baseline should capture"))?;

    write_file(repo, "src/lib.rs", "fn one() {}\nfn after() {}\n")?;
    let counted = line_change_update(
        /*current_lines_added*/ 5, /*current_lines_deleted*/ 3,
        /*persistence_lines_added*/ 1, /*persistence_lines_deleted*/ 1,
    );
    assert_eq!(
        Some(counted),
        line_changes::update_since_baseline(repo, &baseline, baseline.persisted_stats()).await
    );

    write_file(repo, "src/lib.rs", baseline_contents)?;
    assert_eq!(
        Some(line_change_update(
            /*current_lines_added*/ 4, /*current_lines_deleted*/ 2,
            /*persistence_lines_added*/ -1, /*persistence_lines_deleted*/ -1,
        )),
        line_changes::update_since_baseline(repo, &baseline, counted.current_stats).await
    );
    Ok(())
}

#[tokio::test]
async fn update_since_baseline_counts_changes_committed_during_turn() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(repo, "src/lib.rs", "fn base() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;
    let baseline = capture_baseline(repo, &test_goal(/*lines_added*/ 7, /*lines_deleted*/ 1)?)
        .await
        .ok_or_else(|| anyhow::anyhow!("baseline should capture"))?;

    write_file(repo, "src/lib.rs", "fn base() {}\nfn committed() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "goal change").await?;

    assert_eq!(
        Some(line_change_update(
            /*current_lines_added*/ 8, /*current_lines_deleted*/ 1,
            /*persistence_lines_added*/ 1, /*persistence_lines_deleted*/ 0,
        )),
        line_changes::update_since_baseline(repo, &baseline, baseline.persisted_stats()).await
    );
    Ok(())
}

#[tokio::test]
async fn same_cwd_losing_goal_reacquires_lease_during_same_turn() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(repo, "src/lib.rs", "fn base() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;

    let goal_a = test_goal_with_owner(
        "00000000-0000-4000-8000-0000000000a1",
        "goal-a",
        /*lines_added*/ 0,
        /*lines_deleted*/ 0,
    )?;
    let state_a = GoalAccountingState::default();
    state_a.start_turn(
        "turn-a",
        ModeKind::Default,
        &TokenUsage::default(),
        Some(repo.to_path_buf()),
    );
    state_a.mark_turn_goal_active("turn-a", goal_a.goal_id.clone());
    line_changes::establish_current_turn_baseline(&state_a, &goal_a).await;

    let goal_b = test_goal_with_owner(
        "00000000-0000-4000-8000-0000000000b2",
        "goal-b",
        /*lines_added*/ 0,
        /*lines_deleted*/ 0,
    )?;
    let state_b = GoalAccountingState::default();
    state_b.start_turn(
        "turn-b",
        ModeKind::Default,
        &TokenUsage::default(),
        Some(repo.to_path_buf()),
    );
    state_b.mark_turn_goal_active("turn-b", goal_b.goal_id.clone());
    line_changes::establish_current_turn_baseline(&state_b, &goal_b).await;
    assert_eq!(
        None,
        state_b
            .progress_snapshot("turn-b")
            .and_then(|snapshot| snapshot.line_changes)
    );

    write_file(repo, "src/lib.rs", "fn base() {}\nfn first_goal() {}\n")?;
    {
        let snapshot_a = state_a
            .progress_snapshot("turn-a")
            .expect("first owner should keep the worktree baseline");
        let line_changes = snapshot_a
            .line_changes
            .expect("first owner snapshot should include line changes");
        assert_eq!(
            Some(line_change_update(
                /*current_lines_added*/ 1, /*current_lines_deleted*/ 0,
                /*persistence_lines_added*/ 1, /*persistence_lines_deleted*/ 0,
            )),
            line_changes::update_since_baseline(
                line_changes.cwd.as_path(),
                &line_changes.baseline,
                line_changes.last_accounted_stats,
            )
            .await
        );
    }
    state_a.finish_turn("turn-a");
    // A later tool start retries the baseline in the same active turn after
    // the winner has released the worktree lease.
    line_changes::establish_current_turn_baseline(&state_b, &goal_b).await;
    write_file(
        repo,
        "src/lib.rs",
        "fn base() {}\nfn first_goal() {}\nfn second_goal() {}\n",
    )?;
    let snapshot_b = state_b
        .progress_snapshot("turn-b")
        .expect("second owner should acquire a baseline in the same turn after release");
    let line_changes = snapshot_b
        .line_changes
        .expect("second owner snapshot should include line changes");
    assert_eq!(
        Some(line_change_update(
            /*current_lines_added*/ 1, /*current_lines_deleted*/ 0,
            /*persistence_lines_added*/ 1, /*persistence_lines_deleted*/ 0,
        )),
        line_changes::update_since_baseline(
            line_changes.cwd.as_path(),
            &line_changes.baseline,
            line_changes.last_accounted_stats,
        )
        .await
    );
    Ok(())
}

#[tokio::test]
async fn same_cwd_line_change_lease_is_exclusive_across_processes() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let repo = tempdir.path();
    init_repo(repo).await?;
    write_file(repo, "src/lib.rs", "fn base() {}\n")?;
    run_git(repo, &["add", "."]).await?;
    commit(repo, "initial").await?;

    let goal = test_goal_with_owner(
        "00000000-0000-4000-8000-0000000000d4",
        "parent-goal",
        /*lines_added*/ 0,
        /*lines_deleted*/ 0,
    )?;
    let state = GoalAccountingState::default();
    state.start_turn(
        "turn-parent",
        ModeKind::Default,
        &TokenUsage::default(),
        Some(repo.to_path_buf()),
    );
    state.mark_turn_goal_active("turn-parent", goal.goal_id.clone());
    line_changes::establish_current_turn_baseline(&state, &goal).await;
    assert!(
        state
            .progress_snapshot("turn-parent")
            .and_then(|snapshot| snapshot.line_changes)
            .is_some(),
        "parent process should acquire the shared worktree lease"
    );

    run_line_change_lease_helper(repo, "denied").await?;
    state.finish_turn("turn-parent");
    run_line_change_lease_helper(repo, "acquired").await?;
    Ok(())
}

#[tokio::test]
async fn subprocess_shared_cwd_line_change_lease_helper() -> Result<()> {
    let Some(repo) = std::env::var_os(LINE_CHANGE_LEASE_HELPER_REPO_ENV) else {
        return Ok(());
    };
    let repo = PathBuf::from(repo);
    let expectation = std::env::var(LINE_CHANGE_LEASE_HELPER_EXPECT_ENV)?;
    let baseline = capture_baseline(
        repo.as_path(),
        &test_goal_with_owner(
            "00000000-0000-4000-8000-0000000000e5",
            "child-goal",
            /*lines_added*/ 0,
            /*lines_deleted*/ 0,
        )?,
    )
    .await;

    match expectation.as_str() {
        "denied" => assert_eq!(None, baseline),
        "acquired" => assert!(
            baseline.is_some(),
            "child process should acquire the shared worktree lease"
        ),
        other => anyhow::bail!("unknown lease helper expectation {other:?}"),
    }
    Ok(())
}

async fn run_line_change_lease_helper(repo: &Path, expectation: &str) -> Result<()> {
    let output = tokio::process::Command::new(std::env::current_exe()?)
        .arg(LINE_CHANGE_LEASE_HELPER_TEST)
        .arg("--exact")
        .arg("--nocapture")
        .env(LINE_CHANGE_LEASE_HELPER_REPO_ENV, repo.as_os_str())
        .env(LINE_CHANGE_LEASE_HELPER_EXPECT_ENV, expectation)
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "line-change lease helper expected {expectation:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    Ok(())
}

async fn init_repo(repo: &Path) -> Result<()> {
    run_git(repo, &["init"]).await
}

async fn run_git(repo: &Path, args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .current_dir(repo)
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(args)
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

async fn commit(repo: &Path, message: &str) -> Result<()> {
    run_git(
        repo,
        &[
            "-c",
            "user.name=Codewith Test",
            "-c",
            "user.email=codewith@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-m",
            message,
        ],
    )
    .await
}

fn write_file(repo: &Path, path: &str, contents: &str) -> Result<()> {
    let path = repo.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn test_goal(lines_added: i64, lines_deleted: i64) -> Result<codex_state::ThreadGoal> {
    test_goal_with_owner(
        "00000000-0000-4000-8000-000000000001",
        "goal-1",
        lines_added,
        lines_deleted,
    )
}

fn test_goal_with_owner(
    thread_id: &str,
    goal_id: &str,
    lines_added: i64,
    lines_deleted: i64,
) -> Result<codex_state::ThreadGoal> {
    let now = Utc::now();
    Ok(codex_state::ThreadGoal {
        thread_id: ThreadId::from_string(thread_id)?,
        goal_id: goal_id.to_string(),
        objective: "Track line changes".to_string(),
        title: None,
        status: ThreadGoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        lines_added,
        lines_deleted,
        created_at: now,
        updated_at: now,
    })
}

async fn capture_baseline(
    cwd: &Path,
    goal: &codex_state::ThreadGoal,
) -> Option<line_changes::GoalLineChangeBaseline> {
    match line_changes::capture_baseline_outcome(cwd, goal).await {
        line_changes::BaselineCaptureOutcome::Captured(baseline) => Some(baseline),
        line_changes::BaselineCaptureOutcome::LeaseUnavailable
        | line_changes::BaselineCaptureOutcome::SnapshotUnavailable => None,
    }
}

fn line_change_update(
    current_lines_added: i64,
    current_lines_deleted: i64,
    persistence_lines_added: i64,
    persistence_lines_deleted: i64,
) -> line_changes::GoalLineChangeUpdate {
    line_changes::GoalLineChangeUpdate {
        current_stats: ThreadGoalLineChangeStats {
            lines_added: current_lines_added,
            lines_deleted: current_lines_deleted,
        },
        persistence_delta: ThreadGoalLineChangeStats {
            lines_added: persistence_lines_added,
            lines_deleted: persistence_lines_deleted,
        },
    }
}

fn token_usage(
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
) -> TokenUsage {
    TokenUsage {
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens: 0,
        output_tokens,
        reasoning_output_tokens,
        total_tokens,
    }
}
