use super::*;
use crate::runtime::managed_worktrees::path_to_db_string;
use crate::runtime::test_support::unique_temp_dir;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Barrier;

fn admission_params(
    run_id: &str,
    worktree_id: &str,
    worktree_path: &std::path::Path,
) -> BackgroundAgentRunAdmissionParams {
    BackgroundAgentRunAdmissionParams {
        run: BackgroundAgentRunCreateParams {
            id: run_id.to_string(),
            idempotency_key: None,
            request_id: Some(format!("request-{run_id}")),
            source: "test".to_string(),
            prompt_snapshot_ref: format!("inline:{run_id}:prompt"),
            input_snapshot_ref: None,
            thread_id: None,
            thread_store_kind: "background-agent".to_string(),
            thread_store_id: None,
            rollout_path: None,
            parent_thread_id: None,
            parent_agent_run_id: None,
            spawn_linkage_json: None,
            auth_profile_ref: Some("profile:test".to_string()),
            status_reason: Some("queued for test".to_string()),
            config_fingerprint: Some("config-test".to_string()),
            version_fingerprint: Some("version-test".to_string()),
        },
        worktree_id: worktree_id.to_string(),
        execution_snapshot: BackgroundAgentExecutionSnapshotParams {
            run_id: run_id.to_string(),
            snapshot_kind: "initial_execution_context".to_string(),
            payload_json: json!({"cwd": path_to_db_string(worktree_path)}),
            recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
            config_fingerprint: Some("config-test".to_string()),
        },
        started_event_payload_json: json!({
            "cwd": path_to_db_string(worktree_path),
            "prompt": "test admission",
            "promptSnapshotRef": format!("inline:{run_id}:prompt"),
            "initialGoalObjective": null,
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_managed_worktree_admission_has_one_winner_and_no_loser_residue()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = base_repo_path
        .join(".codewith")
        .join("worktrees")
        .join("shared");
    std::fs::create_dir_all(&worktree_path)?;
    runtime
        .managed_worktrees()
        .create_managed_worktree(ManagedWorktreeCreateParams {
            worktree_id: Some("worktree-1".to_string()),
            identity: Some("session:worktree-1".to_string()),
            mode: crate::ManagedWorktreeMode::IsolatedWorktree,
            base_repo_path,
            worktree_path: worktree_path.clone(),
            branch: Some("codewith/worktree-1".to_string()),
            base_sha: Some("base-sha".to_string()),
            head_sha: Some("head-sha".to_string()),
            status_snapshot_json: json!({}),
            dirty: false,
            cleanup_policy: crate::ManagedWorktreeCleanupPolicy::DeleteIfClean,
            owner_kind: crate::ManagedWorktreeOwnerKind::Manual,
            owner_thread_id: None,
            owner_agent_run_id: None,
            cleanup_after: None,
        })
        .await?;

    let first_params = admission_params("run-first", "worktree-1", &worktree_path);
    let second_params = admission_params("run-second", "worktree-1", &worktree_path);
    let barrier = Arc::new(Barrier::new(2));
    let first_runtime = Arc::clone(&runtime);
    let first_barrier = Arc::clone(&barrier);
    let first = async move {
        first_barrier.wait().await;
        first_runtime
            .admit_background_agent_run(&first_params)
            .await
    };
    let second_runtime = Arc::clone(&runtime);
    let second = async move {
        barrier.wait().await;
        second_runtime
            .admit_background_agent_run(&second_params)
            .await
    };
    let (first, second) = tokio::join!(first, second);
    let winner = match (first, second) {
        (Ok(winner), Err(_)) | (Err(_), Ok(winner)) => winner,
        (Ok(_), Ok(_)) => anyhow::bail!("both competing worktree admissions succeeded"),
        (Err(first), Err(second)) => {
            anyhow::bail!("both competing worktree admissions failed: {first}; {second}")
        }
    };
    assert!(winner.created_new_run);

    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_runs")
        .fetch_one(runtime.pool.as_ref())
        .await?;
    let execution_snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_execution_snapshots")
            .fetch_one(runtime.pool.as_ref())
            .await?;
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_events")
        .fetch_one(runtime.pool.as_ref())
        .await?;
    let status_snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_status_snapshots")
            .fetch_one(runtime.pool.as_ref())
            .await?;
    let assignment_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM managed_worktree_assignments WHERE detached_at_ms IS NULL",
    )
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(run_count, 1);
    assert_eq!(execution_snapshot_count, 1);
    assert_eq!(event_count, 1);
    assert_eq!(status_snapshot_count, 1);
    assert_eq!(assignment_count, 1);

    let loser_id = if winner.run.id == "run-first" {
        "run-second"
    } else {
        "run-first"
    };
    for table in [
        "background_agent_runs",
        "background_agent_execution_snapshots",
        "background_agent_events",
        "background_agent_status_snapshots",
    ] {
        let column = if table == "background_agent_runs" {
            "id"
        } else {
            "run_id"
        };
        let query = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?");
        let residue: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .bind(loser_id)
            .fetch_one(runtime.pool.as_ref())
            .await?;
        assert_eq!(residue, 0, "losing admission left residue in {table}");
    }
    let assignment_run_id: Option<String> = sqlx::query_scalar(
        "SELECT agent_run_id FROM managed_worktree_assignments \
         WHERE worktree_id = ? AND detached_at_ms IS NULL",
    )
    .bind("worktree-1")
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(assignment_run_id.as_deref(), Some(winner.run.id.as_str()));
    assert_eq!(winner.execution_snapshot.run_id, winner.run.id);
    assert_eq!(winner.event.run_id, winner.run.id);
    assert_eq!(winner.status_snapshot.run_id, winner.run.id);

    Ok(())
}
