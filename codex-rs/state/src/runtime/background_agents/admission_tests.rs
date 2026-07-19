use super::*;
use crate::BackgroundAgentPendingInteractionKind;
use crate::runtime::managed_worktrees::path_to_db_string;
use crate::runtime::test_support::unique_temp_dir;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Barrier;

fn admission_params(
    run_id: &str,
    worktree_id: &str,
    worktree_path: &Path,
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
        worktree_id: Some(worktree_id.to_string()),
        max_active_runs: 8,
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

fn unmanaged_admission_params(run_id: &str) -> BackgroundAgentRunAdmissionParams {
    let mut params = admission_params(run_id, "unused-worktree", Path::new("/unused-worktree"));
    params.worktree_id = None;
    params
}

async fn create_isolated_worktree(
    runtime: &StateRuntime,
    worktree_id: &str,
    base_repo_path: &Path,
) -> anyhow::Result<std::path::PathBuf> {
    let worktree_path = base_repo_path
        .join(".codewith")
        .join("worktrees")
        .join(worktree_id);
    std::fs::create_dir_all(&worktree_path)?;
    runtime
        .managed_worktrees()
        .create_managed_worktree(ManagedWorktreeCreateParams {
            worktree_id: Some(worktree_id.to_string()),
            identity: Some(format!("session:{worktree_id}")),
            mode: crate::ManagedWorktreeMode::IsolatedWorktree,
            base_repo_path: base_repo_path.to_path_buf(),
            worktree_path: worktree_path.clone(),
            branch: Some(format!("codewith/{worktree_id}")),
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
    Ok(worktree_path)
}

async fn detach_assignment_for_restore_regression(
    runtime: &StateRuntime,
    worktree_id: &str,
    run_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE managed_worktree_assignments SET detached_at_ms = 1 \
         WHERE worktree_id = ? AND agent_run_id = ? AND detached_at_ms IS NULL",
    )
    .bind(worktree_id)
    .bind(run_id)
    .execute(runtime.pool.as_ref())
    .await?;
    sqlx::query(
        "UPDATE managed_worktrees SET owner_kind = 'manual', owner_agent_run_id = NULL \
         WHERE worktree_id = ?",
    )
    .bind(worktree_id)
    .execute(runtime.pool.as_ref())
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_managed_worktree_admission_has_one_winner_and_no_loser_residue()
-> anyhow::Result<()> {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let first_runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let second_runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;

    let first_params = admission_params("run-first", "worktree-1", &worktree_path);
    let second_params = admission_params("run-second", "worktree-1", &worktree_path);
    let barrier = Arc::new(Barrier::new(2));
    let first_runtime = Arc::clone(&first_runtime);
    let first_barrier = Arc::clone(&barrier);
    let first = async move {
        first_barrier.wait().await;
        first_runtime
            .admit_background_agent_run(&first_params)
            .await
    };
    let second_runtime = Arc::clone(&second_runtime);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_idempotent_admissions_from_two_runtimes_converge_on_one_run()
-> anyhow::Result<()> {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let first_runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let second_runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;

    let mut first_params = admission_params("run-first", "worktree-1", &worktree_path);
    first_params.run.idempotency_key = Some("same-key".to_string());
    let mut second_params = admission_params("run-second", "worktree-1", &worktree_path);
    second_params.run.idempotency_key = Some("same-key".to_string());
    let barrier = Arc::new(Barrier::new(2));
    let first_barrier = Arc::clone(&barrier);
    let first = async move {
        first_barrier.wait().await;
        first_runtime
            .admit_background_agent_run(&first_params)
            .await
    };
    let second = async move {
        barrier.wait().await;
        second_runtime
            .admit_background_agent_run(&second_params)
            .await
    };
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first same-key admission should succeed");
    let second = second.expect("second same-key admission should converge, not return SQLite busy");

    assert_eq!(first.run.id, second.run.id);
    assert_ne!(first.created_new_run, second.created_new_run);
    assert_eq!(first.execution_snapshot.run_id, first.run.id);
    assert_eq!(second.execution_snapshot.run_id, second.run.id);
    assert_eq!(first.event.run_id, first.run.id);
    assert_eq!(second.event.run_id, second.run.id);
    assert_eq!(first.status_snapshot.run_id, first.run.id);
    assert_eq!(second.status_snapshot.run_id, second.run.id);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_unmanaged_admissions_enforce_the_active_run_quota_in_sqlite()
-> anyhow::Result<()> {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    for index in 0..7 {
        runtime
            .admit_background_agent_run(&unmanaged_admission_params(&format!("seed-{index}")))
            .await?;
    }
    let first_runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let second_runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
    let first_params = unmanaged_admission_params("run-first");
    let second_params = unmanaged_admission_params("run-second");
    let barrier = Arc::new(Barrier::new(2));
    let first_barrier = Arc::clone(&barrier);
    let first = async move {
        first_barrier.wait().await;
        first_runtime
            .admit_background_agent_run(&first_params)
            .await
    };
    let second = async move {
        barrier.wait().await;
        second_runtime
            .admit_background_agent_run(&second_params)
            .await
    };
    let (first, second) = tokio::join!(first, second);
    let (winner, loser_id) = match (first, second) {
        (Ok(winner), Err(error)) => {
            assert!(matches!(
                error.downcast_ref::<BackgroundAgentAdmissionError>(),
                Some(BackgroundAgentAdmissionError::QuotaExceeded { .. })
            ));
            (winner, "run-second")
        }
        (Err(error), Ok(winner)) => {
            assert!(matches!(
                error.downcast_ref::<BackgroundAgentAdmissionError>(),
                Some(BackgroundAgentAdmissionError::QuotaExceeded { .. })
            ));
            (winner, "run-first")
        }
        (Ok(_), Ok(_)) => anyhow::bail!("both concurrent quota admissions succeeded"),
        (Err(first), Err(second)) => {
            anyhow::bail!("both concurrent quota admissions failed: {first}; {second}")
        }
    };

    assert!(winner.created_new_run);
    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM background_agent_runs")
        .fetch_one(runtime.pool.as_ref())
        .await?;
    assert_eq!(run_count, 8);
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
        assert_eq!(residue, 0, "quota loser left residue in {table}");
    }
    Ok(())
}

#[tokio::test]
async fn idempotent_admission_restores_only_its_persisted_worktree() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let first_path = create_isolated_worktree(&runtime, "worktree-first", &base_repo_path).await?;
    let second_path =
        create_isolated_worktree(&runtime, "worktree-second", &base_repo_path).await?;

    let mut first_params = admission_params("run-first", "worktree-first", &first_path);
    first_params.run.idempotency_key = Some("idempotency-key".to_string());
    let first = runtime.admit_background_agent_run(&first_params).await?;
    detach_assignment_for_restore_regression(&runtime, "worktree-first", first.run.id.as_str())
        .await?;

    let mut same_worktree_params = admission_params("run-retry", "worktree-first", &first_path);
    same_worktree_params.run.idempotency_key = Some("idempotency-key".to_string());
    let restored = runtime
        .admit_background_agent_run(&same_worktree_params)
        .await?;
    assert!(!restored.created_new_run);
    assert_eq!(restored.run.id, first.run.id);
    assert_eq!(restored.execution_snapshot, first.execution_snapshot);

    detach_assignment_for_restore_regression(&runtime, "worktree-first", first.run.id.as_str())
        .await?;
    let mut cross_worktree_params =
        admission_params("run-cross-worktree", "worktree-second", &second_path);
    cross_worktree_params.run.idempotency_key = Some("idempotency-key".to_string());
    let error = runtime
        .admit_background_agent_run(&cross_worktree_params)
        .await
        .expect_err("idempotent retries must not rebind a run to a different worktree");
    assert!(
        error.to_string().contains("different managed worktree"),
        "unexpected cross-worktree retry error: {error:#}"
    );

    let assignment_worktree_id: Option<String> = sqlx::query_scalar(
        "SELECT worktree_id FROM managed_worktree_assignments \
         WHERE agent_run_id = ? AND detached_at_ms IS NULL",
    )
    .bind(first.run.id.as_str())
    .fetch_optional(runtime.pool.as_ref())
    .await?;
    assert_eq!(assignment_worktree_id, None);
    Ok(())
}

#[tokio::test]
async fn idempotent_admission_reconstructs_a_missing_snapshot_from_current_run_state()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;
    let mut initial_params = admission_params("run-initial", "worktree-1", &worktree_path);
    initial_params.run.idempotency_key = Some("recover-current-snapshot".to_string());
    let initial = runtime.admit_background_agent_run(&initial_params).await?;

    runtime
        .update_background_agent_run_status(
            initial.run.id.as_str(),
            BackgroundAgentRunStatus::Starting,
            Some("supervisor claimed run"),
        )
        .await?;
    runtime
        .append_background_agent_event(
            initial.run.id.as_str(),
            "agent.claimed",
            &json!({"supervisor": "test"}),
        )
        .await?;
    runtime
        .append_background_agent_event(
            initial.run.id.as_str(),
            "agent.progress",
            &json!({"stage": "launching"}),
        )
        .await?;
    sqlx::query("DELETE FROM background_agent_status_snapshots WHERE run_id = ?")
        .bind(initial.run.id.as_str())
        .execute(runtime.pool.as_ref())
        .await?;

    let mut retry_params = admission_params("run-retry", "worktree-1", &worktree_path);
    retry_params.run.idempotency_key = Some("recover-current-snapshot".to_string());
    let recovered = runtime.admit_background_agent_run(&retry_params).await?;

    assert_eq!(recovered.run.id, initial.run.id);
    assert_eq!(recovered.event.seq, 1);
    assert_eq!(
        recovered.status_snapshot.status,
        BackgroundAgentRunStatus::Starting
    );
    assert_eq!(recovered.status_snapshot.seq, 3);
    assert_eq!(recovered.status_snapshot.last_event_seq, 3);
    assert_eq!(
        recovered.status_snapshot.payload_json,
        json!({"phase": "starting", "recovered": true})
    );
    Ok(())
}

#[tokio::test]
async fn idempotent_admission_refreshes_a_retained_stale_snapshot_from_current_run_state()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;
    let mut initial_params = admission_params("run-initial", "worktree-1", &worktree_path);
    initial_params.run.idempotency_key = Some("refresh-stale-snapshot".to_string());
    let initial = runtime.admit_background_agent_run(&initial_params).await?;

    runtime
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "pending-1".to_string(),
                run_id: initial.run.id.clone(),
                worker_request_id: Some("worker-request-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({"action": "continue"}),
                no_client_policy: "deny".to_string(),
                timeout_at: None,
            },
        )
        .await?;
    runtime
        .update_background_agent_run_status(
            initial.run.id.as_str(),
            BackgroundAgentRunStatus::Starting,
            Some("supervisor claimed run"),
        )
        .await?;
    sqlx::query("UPDATE background_agent_runs SET desired_state = ? WHERE id = ?")
        .bind(BackgroundAgentDesiredState::Stopped.as_str())
        .bind(initial.run.id.as_str())
        .execute(runtime.pool.as_ref())
        .await?;
    runtime
        .append_background_agent_event(
            initial.run.id.as_str(),
            "agent.progress",
            &json!({"stage": "launching"}),
        )
        .await?;

    let mut retry_params = admission_params("run-retry", "worktree-1", &worktree_path);
    retry_params.run.idempotency_key = Some("refresh-stale-snapshot".to_string());
    let recovered = runtime.admit_background_agent_run(&retry_params).await?;

    assert_eq!(recovered.run.id, initial.run.id);
    assert_eq!(recovered.event.seq, 1);
    assert_eq!(
        recovered.status_snapshot.status,
        BackgroundAgentRunStatus::Starting
    );
    assert_eq!(
        recovered.status_snapshot.desired_state,
        BackgroundAgentDesiredState::Stopped
    );
    assert_eq!(recovered.status_snapshot.seq, 3);
    assert_eq!(recovered.status_snapshot.last_event_seq, 3);
    assert_eq!(recovered.status_snapshot.pending_interaction_count, 1);
    assert_eq!(
        recovered.status_snapshot.payload_json,
        json!({"phase": "starting", "recovered": true})
    );
    Ok(())
}

#[tokio::test]
async fn terminal_idempotent_retry_cannot_claim_a_different_worktree() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let first_path = create_isolated_worktree(&runtime, "worktree-first", &base_repo_path).await?;
    let second_path =
        create_isolated_worktree(&runtime, "worktree-second", &base_repo_path).await?;
    let mut first_params = admission_params("run-first", "worktree-first", &first_path);
    first_params.run.idempotency_key = Some("idempotency-key".to_string());
    let first = runtime.admit_background_agent_run(&first_params).await?;
    detach_assignment_for_restore_regression(&runtime, "worktree-first", first.run.id.as_str())
        .await?;
    sqlx::query("UPDATE background_agent_runs SET status = 'completed' WHERE id = ?")
        .bind(first.run.id.as_str())
        .execute(runtime.pool.as_ref())
        .await?;

    let mut retry_params = admission_params("run-retry", "worktree-second", &second_path);
    retry_params.run.idempotency_key = Some("idempotency-key".to_string());
    let error = runtime
        .admit_background_agent_run(&retry_params)
        .await
        .expect_err("terminal idempotent retry must not claim a fresh worktree");
    assert!(
        error.to_string().contains("different managed worktree"),
        "unexpected terminal retry error: {error:#}"
    );
    let assignment_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM managed_worktree_assignments \
         WHERE worktree_id = 'worktree-second' AND detached_at_ms IS NULL",
    )
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(assignment_count, 0);
    Ok(())
}

#[tokio::test]
async fn terminal_same_key_replays_its_durable_records_without_reclaiming() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;

    for (status, label) in [
        (BackgroundAgentRunStatus::Completed, "completed"),
        (BackgroundAgentRunStatus::Failed, "failed"),
        (BackgroundAgentRunStatus::Cancelled, "cancelled"),
    ] {
        let mut first_params = admission_params(
            format!("run-{label}").as_str(),
            "worktree-1",
            &worktree_path,
        );
        first_params.run.idempotency_key = Some(format!("terminal-replay-{label}"));
        let first = runtime.admit_background_agent_run(&first_params).await?;
        detach_assignment_for_restore_regression(&runtime, "worktree-1", first.run.id.as_str())
            .await?;
        runtime
            .update_background_agent_run_status(
                first.run.id.as_str(),
                status,
                Some("terminal replay regression"),
            )
            .await?;

        let mut retry_params = admission_params(
            format!("retry-{label}").as_str(),
            "worktree-1",
            &worktree_path,
        );
        retry_params.run.idempotency_key = Some(format!("terminal-replay-{label}"));
        let replay = runtime.admit_background_agent_run(&retry_params).await?;

        assert!(!replay.created_new_run);
        assert_eq!(replay.run.id, first.run.id);
        assert_eq!(replay.execution_snapshot, first.execution_snapshot);
        assert_eq!(replay.event, first.event);
        assert_eq!(replay.status_snapshot.status, status);
        let assignment_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktree_assignments \
             WHERE worktree_id = ? AND agent_run_id = ? AND detached_at_ms IS NULL",
        )
        .bind("worktree-1")
        .bind(first.run.id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await?;
        assert_eq!(
            assignment_count, 0,
            "terminal replay must not reclaim the worktree"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn idempotent_recovery_accepts_a_legacy_snapshot_cwd_through_a_symlink_alias()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;
    let alias_base_repo_path = base_repo_path.with_file_name("repo-alias");
    std::os::unix::fs::symlink(&base_repo_path, &alias_base_repo_path)?;
    let alias_worktree_path = alias_base_repo_path
        .join(".codewith")
        .join("worktrees")
        .join("worktree-1");
    let mut first_params = admission_params("run-initial", "worktree-1", &worktree_path);
    first_params.run.idempotency_key = Some("legacy-symlink-recovery".to_string());
    let first = runtime.admit_background_agent_run(&first_params).await?;
    sqlx::query(
        "UPDATE background_agent_execution_snapshots SET payload_json = ? WHERE run_id = ?",
    )
    .bind(json!({"cwd": path_to_db_string(&alias_worktree_path)}).to_string())
    .bind(first.run.id.as_str())
    .execute(runtime.pool.as_ref())
    .await?;
    sqlx::query("DELETE FROM managed_worktree_assignments WHERE agent_run_id = ?")
        .bind(first.run.id.as_str())
        .execute(runtime.pool.as_ref())
        .await?;
    sqlx::query(
        "UPDATE managed_worktrees SET owner_kind = 'manual', owner_agent_run_id = NULL \
         WHERE worktree_id = ?",
    )
    .bind("worktree-1")
    .execute(runtime.pool.as_ref())
    .await?;

    let mut missing_worktree_params =
        admission_params("run-missing-worktree", "worktree-1", &worktree_path);
    missing_worktree_params.run.idempotency_key = Some("legacy-symlink-recovery".to_string());
    missing_worktree_params.worktree_id = None;
    let error = runtime
        .admit_background_agent_run(&missing_worktree_params)
        .await
        .expect_err("managed snapshot recovery must not resume without its managed worktree");
    assert!(
        error
            .to_string()
            .contains("associated with a managed worktree"),
        "unexpected missing-worktree replay error: {error:#}"
    );

    let mut retry_params = admission_params("run-retry", "worktree-1", &worktree_path);
    retry_params.run.idempotency_key = Some("legacy-symlink-recovery".to_string());
    let recovered = runtime.admit_background_agent_run(&retry_params).await?;

    assert_eq!(recovered.run.id, first.run.id);
    let assignment_worktree_id: Option<String> = sqlx::query_scalar(
        "SELECT worktree_id FROM managed_worktree_assignments \
         WHERE agent_run_id = ? AND detached_at_ms IS NULL",
    )
    .bind(first.run.id.as_str())
    .fetch_optional(runtime.pool.as_ref())
    .await?;
    assert_eq!(assignment_worktree_id.as_deref(), Some("worktree-1"));
    Ok(())
}

#[tokio::test]
async fn active_admission_fences_detach_release_and_cleanup() -> anyhow::Result<()> {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let control_runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
    let base_repo_path = unique_temp_dir().join("repo");
    let worktree_path = create_isolated_worktree(&runtime, "worktree-1", &base_repo_path).await?;
    let admitted = runtime
        .admit_background_agent_run(&admission_params("run-1", "worktree-1", &worktree_path))
        .await?;

    let barrier = Arc::new(Barrier::new(2));
    let detach_store = runtime.managed_worktrees().clone();
    let detach_barrier = Arc::clone(&barrier);
    let run_id = admitted.run.id.clone();
    let detach = async move {
        detach_barrier.wait().await;
        detach_store
            .detach_managed_worktree(ManagedWorktreeDetachParams {
                worktree_id: "worktree-1".to_string(),
                target: ManagedWorktreeAssignmentTarget::AgentRun(run_id),
            })
            .await
    };
    let release_store = control_runtime.managed_worktrees().clone();
    let release = async move {
        barrier.wait().await;
        release_store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "worktree-1".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::DeleteIfClean,
                force_delete: false,
                status_snapshot_json: json!({}),
                dirty: false,
            })
            .await
    };
    let (detach, release) = tokio::join!(detach, release);
    sqlx::query(
        "UPDATE managed_worktrees SET lifecycle_status = 'cleanup_pending', released_at_ms = 1 \
         WHERE worktree_id = 'worktree-1'",
    )
    .execute(runtime.pool.as_ref())
    .await?;
    let cleanup = control_runtime
        .mark_managed_worktree_cleanup_succeeded("worktree-1")
        .await;
    for result in [detach.map(|_| ()), release.map(|_| ()), cleanup.map(|_| ())] {
        let error = result.expect_err("active admitted runs must fence worktree lifecycle changes");
        assert!(
            error.to_string().contains("active background agent run"),
            "unexpected active-run fence error: {error:#}"
        );
    }
    Ok(())
}
