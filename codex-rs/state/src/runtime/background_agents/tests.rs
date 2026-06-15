use super::*;
use crate::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;
use crate::BackgroundAgentExecutionHandleParams;
use crate::BackgroundAgentPendingInteractionKind;
use crate::BackgroundAgentWorkspaceMode;
use crate::DirectionalThreadSpawnEdgeStatus;
use crate::runtime::test_support::test_thread_metadata;
use crate::runtime::test_support::unique_temp_dir;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;

async fn create_run(runtime: &StateRuntime) -> anyhow::Result<BackgroundAgentRun> {
    runtime
        .create_background_agent_run(&BackgroundAgentRunCreateParams {
            id: "run-1".to_string(),
            idempotency_key: Some("idem-1".to_string()),
            request_id: Some("req-1".to_string()),
            source: "cli".to_string(),
            prompt_snapshot_ref: "prompt://run-1".to_string(),
            input_snapshot_ref: Some("input://run-1".to_string()),
            thread_id: Some("thread-1".to_string()),
            thread_store_kind: "local".to_string(),
            thread_store_id: Some("state_5.sqlite".to_string()),
            rollout_path: Some("/tmp/rollout.jsonl".to_string()),
            parent_thread_id: Some("parent-thread".to_string()),
            parent_agent_run_id: None,
            spawn_linkage_json: Some(json!({"agentPath": ["reviewer"]})),
            auth_profile_ref: Some("profile:default".to_string()),
            status_reason: Some("created".to_string()),
            config_fingerprint: Some("cfg-1".to_string()),
            version_fingerprint: Some("version-1".to_string()),
        })
        .await
}

async fn create_run_with_id(
    runtime: &StateRuntime,
    id: &str,
) -> anyhow::Result<BackgroundAgentRun> {
    runtime
        .create_background_agent_run(&BackgroundAgentRunCreateParams {
            id: id.to_string(),
            idempotency_key: Some(format!("idem-{id}")),
            request_id: Some(format!("req-{id}")),
            source: "cli".to_string(),
            prompt_snapshot_ref: format!("prompt://{id}"),
            input_snapshot_ref: Some(format!("input://{id}")),
            thread_id: Some(format!("thread-{id}")),
            thread_store_kind: "local".to_string(),
            thread_store_id: Some("state_5.sqlite".to_string()),
            rollout_path: Some(format!("/tmp/{id}.jsonl")),
            parent_thread_id: Some("parent-thread".to_string()),
            parent_agent_run_id: None,
            spawn_linkage_json: Some(json!({"agentPath": ["reviewer"]})),
            auth_profile_ref: Some("profile:default".to_string()),
            status_reason: Some("created".to_string()),
            config_fingerprint: Some("cfg-1".to_string()),
            version_fingerprint: Some("version-1".to_string()),
        })
        .await
}

#[tokio::test]
async fn background_agent_run_create_is_idempotent() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let first = create_run(runtime.as_ref()).await?;
    let second = runtime
        .create_background_agent_run(&BackgroundAgentRunCreateParams {
            id: "run-duplicate".to_string(),
            idempotency_key: Some("idem-1".to_string()),
            request_id: Some("req-duplicate".to_string()),
            source: "cli".to_string(),
            prompt_snapshot_ref: "prompt://duplicate".to_string(),
            input_snapshot_ref: None,
            thread_id: None,
            thread_store_kind: "local".to_string(),
            thread_store_id: None,
            rollout_path: None,
            parent_thread_id: None,
            parent_agent_run_id: None,
            spawn_linkage_json: None,
            auth_profile_ref: None,
            status_reason: None,
            config_fingerprint: None,
            version_fingerprint: None,
        })
        .await?;

    assert_eq!(second, first);
    assert_eq!(
        runtime.list_background_agent_runs(/*limit*/ None).await?,
        vec![first]
    );
    Ok(())
}

#[tokio::test]
async fn legacy_thread_and_agent_job_rows_do_not_populate_background_agent_roster()
-> anyhow::Result<()> {
    let codex_home = unique_temp_dir();
    let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
    let parent_thread_id = ThreadId::new();
    let child_thread_id = ThreadId::new();
    let parent_metadata = test_thread_metadata(
        codex_home.as_path(),
        parent_thread_id,
        codex_home.join("repo"),
    );
    let mut child_metadata = test_thread_metadata(
        codex_home.as_path(),
        child_thread_id,
        codex_home.join("repo"),
    );
    child_metadata.agent_nickname = Some("Scout".to_string());
    child_metadata.agent_role = Some("reviewer".to_string());
    child_metadata.agent_path = Some("review/scout".to_string());
    runtime.upsert_thread(&parent_metadata).await?;
    runtime.upsert_thread(&child_metadata).await?;
    runtime
        .upsert_thread_spawn_edge(
            parent_thread_id,
            child_thread_id,
            DirectionalThreadSpawnEdgeStatus::Open,
        )
        .await?;
    runtime
        .create_agent_job(
            &AgentJobCreateParams {
                id: "legacy-job-1".to_string(),
                name: "Legacy CSV job".to_string(),
                instruction: "review these rows".to_string(),
                auto_export: false,
                max_runtime_seconds: None,
                output_schema_json: None,
                input_headers: vec!["path".to_string()],
                input_csv_path: "legacy-input.csv".to_string(),
                output_csv_path: "legacy-output.csv".to_string(),
            },
            &[AgentJobItemCreateParams {
                item_id: "legacy-item-1".to_string(),
                row_index: 0,
                source_id: Some("row-1".to_string()),
                row_json: json!({"path": "src/lib.rs"}),
            }],
        )
        .await?;

    assert_eq!(
        runtime.list_background_agent_runs(/*limit*/ None).await?,
        Vec::<BackgroundAgentRun>::new()
    );

    let child_thread_id_string = child_thread_id.to_string();
    let parent_thread_id_string = parent_thread_id.to_string();
    let run = runtime
        .create_background_agent_run(&BackgroundAgentRunCreateParams {
            id: "run-linked-to-child-thread".to_string(),
            idempotency_key: None,
            request_id: None,
            source: "compatibility-test".to_string(),
            prompt_snapshot_ref: "prompt://run-linked-to-child-thread".to_string(),
            input_snapshot_ref: None,
            thread_id: Some(child_thread_id_string.clone()),
            thread_store_kind: "local".to_string(),
            thread_store_id: Some("state.sqlite".to_string()),
            rollout_path: Some(child_metadata.rollout_path.display().to_string()),
            parent_thread_id: Some(parent_thread_id_string.clone()),
            parent_agent_run_id: None,
            spawn_linkage_json: Some(json!({
                "agentPath": child_metadata.agent_path,
                "legacyAgentJobId": "legacy-job-1",
                "legacyAgentJobItemId": "legacy-item-1",
            })),
            auth_profile_ref: None,
            status_reason: Some("explicit background-agent run".to_string()),
            config_fingerprint: None,
            version_fingerprint: None,
        })
        .await?;

    assert_eq!(
        runtime.list_background_agent_runs(/*limit*/ None).await?,
        vec![run.clone()]
    );
    assert_eq!(
        run.thread_id.as_deref(),
        Some(child_thread_id_string.as_str())
    );
    assert_eq!(
        run.parent_thread_id.as_deref(),
        Some(parent_thread_id_string.as_str())
    );
    assert_eq!(
        runtime.list_thread_spawn_children(parent_thread_id).await?,
        vec![child_thread_id]
    );
    assert_eq!(
        runtime
            .get_agent_job("legacy-job-1")
            .await?
            .expect("legacy job should remain readable")
            .status,
        AgentJobStatus::Pending
    );
    Ok(())
}

#[tokio::test]
async fn background_agent_events_are_monotonic_and_replayable() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let started = runtime
        .append_background_agent_event("run-1", "run.started", &json!({"pid": 42}))
        .await?;
    let waiting = runtime
        .append_background_agent_event("run-1", "interaction.created", &json!({"id": "pi-1"}))
        .await?;

    assert_eq!(started.seq, 1);
    assert_eq!(waiting.seq, 2);
    let replay = runtime
        .list_background_agent_events_after(
            "run-1",
            /*after_seq*/ Some(1),
            /*limit*/ None,
        )
        .await?;
    assert_eq!(replay, vec![waiting]);
    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.last_event_seq, 2);
    Ok(())
}

#[tokio::test]
async fn compacted_background_agent_events_reject_stale_cursors() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    runtime
        .append_background_agent_event("run-1", "run.started", &json!({"pid": 42}))
        .await?;
    runtime
        .append_background_agent_event("run-1", "agent.progress", &json!({"step": 1}))
        .await?;
    let retained = runtime
        .append_background_agent_event("run-1", "agent.progress", &json!({"step": 2}))
        .await?;

    assert_eq!(
        runtime
            .compact_background_agent_events_before_seq("run-1", /*before_seq*/ 3)
            .await?,
        2
    );
    assert_eq!(
        runtime
            .list_background_agent_events_after(
                "run-1",
                /*after_seq*/ Some(2),
                /*limit*/ None
            )
            .await?,
        vec![retained]
    );

    let stale_cursor_error = runtime
        .list_background_agent_events_after(
            "run-1",
            /*after_seq*/ Some(1),
            /*limit*/ None,
        )
        .await
        .expect_err("cursor older than replay floor should fail");
    assert!(
        stale_cursor_error
            .to_string()
            .contains(BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED),
        "unexpected stale cursor error: {stale_cursor_error}"
    );

    assert_eq!(
        runtime
            .compact_background_agent_events_before_seq("run-1", /*before_seq*/ 4)
            .await?,
        1
    );
    assert_eq!(
        runtime
            .list_background_agent_events_after(
                "run-1",
                /*after_seq*/ Some(3),
                /*limit*/ None
            )
            .await?,
        Vec::<BackgroundAgentEvent>::new()
    );
    let fully_compacted_error = runtime
        .list_background_agent_events_after(
            "run-1",
            /*after_seq*/ Some(2),
            /*limit*/ None,
        )
        .await
        .expect_err("cursor before an empty retained journal should fail");
    assert!(
        fully_compacted_error
            .to_string()
            .contains(BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED),
        "unexpected fully compacted error: {fully_compacted_error}"
    );
    Ok(())
}

#[tokio::test]
async fn latest_execution_snapshot_returns_newest_context() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let first = runtime
        .create_background_agent_execution_snapshot(&BackgroundAgentExecutionSnapshotParams {
            run_id: "run-1".to_string(),
            snapshot_kind: "initial_execution_context".to_string(),
            payload_json: json!({
                "cwd": "/repo",
                "model": "gpt-5",
                "permissionProfile": {"sandbox": "read-only"},
            }),
            recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
            config_fingerprint: Some("cfg-1".to_string()),
        })
        .await?;
    let second = runtime
        .create_background_agent_execution_snapshot(&BackgroundAgentExecutionSnapshotParams {
            run_id: "run-1".to_string(),
            snapshot_kind: "safe_boundary".to_string(),
            payload_json: json!({
                "cwd": "/repo/worktree",
                "model": "gpt-5",
                "permissionProfile": {"sandbox": "read-only"},
            }),
            recovery_policy: "resume_from_safe_boundary".to_string(),
            config_fingerprint: Some("cfg-1".to_string()),
        })
        .await?;

    assert_eq!(first.seq, 1);
    assert_eq!(second.seq, 2);
    assert_eq!(
        runtime
            .get_latest_background_agent_execution_snapshot("run-1")
            .await?,
        Some(second)
    );
    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.last_snapshot_seq, 2);
    Ok(())
}

#[tokio::test]
async fn pending_interaction_response_is_idempotently_terminal() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    runtime
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: "run-1".to_string(),
            seq: 1,
            status: BackgroundAgentRunStatus::Running,
            desired_state: BackgroundAgentDesiredState::Running,
            summary: Some("working".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 0,
            payload_json: json!({"phase": "running"}),
        })
        .await?;

    let pending = runtime
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "pending-1".to_string(),
                run_id: "run-1".to_string(),
                worker_request_id: Some("worker-req-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({"command": "rm file"}),
                no_client_policy: "deny".to_string(),
                timeout_at: None,
            },
        )
        .await?;
    assert_eq!(
        pending.status,
        BackgroundAgentPendingInteractionStatus::Pending
    );
    let snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(snapshot.pending_interaction_count, 1);
    assert_eq!(snapshot.last_event_seq, 1);

    assert!(
        runtime
            .mark_background_agent_pending_interaction_delivered("pending-1")
            .await?
    );
    let snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(snapshot.pending_interaction_count, 1);
    assert_eq!(snapshot.last_event_seq, 2);
    assert!(
        runtime
            .respond_background_agent_pending_interaction(
                "pending-1",
                &json!({"decision": "denied"}),
                BackgroundAgentPendingInteractionStatus::Denied,
            )
            .await?
    );
    assert!(
        !runtime
            .respond_background_agent_pending_interaction(
                "pending-1",
                &json!({"decision": "approved"}),
                BackgroundAgentPendingInteractionStatus::Responded,
            )
            .await?
    );

    let interactions = runtime
        .list_background_agent_pending_interactions("run-1", /*status*/ None)
        .await?;
    assert_eq!(interactions.len(), 1);
    assert_eq!(
        interactions[0].status,
        BackgroundAgentPendingInteractionStatus::Denied
    );
    assert_eq!(
        interactions[0].response_payload_json,
        Some(json!({"decision": "denied"}))
    );
    let snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(snapshot.pending_interaction_count, 0);
    assert_eq!(snapshot.last_event_seq, 3);
    let events = runtime
        .list_background_agent_events_after("run-1", /*after_seq*/ None, /*limit*/ None)
        .await?;
    assert_eq!(
        events
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            "interaction.created".to_string(),
            "interaction.delivered".to_string(),
            "interaction.denied".to_string(),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn pending_interaction_expiration_is_durable_and_audited() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    runtime
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: "run-1".to_string(),
            seq: 1,
            status: BackgroundAgentRunStatus::WaitingOnUser,
            desired_state: BackgroundAgentDesiredState::Running,
            summary: Some("needs input".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 0,
            payload_json: json!({"phase": "waiting"}),
        })
        .await?;

    runtime
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "expired-1".to_string(),
                run_id: "run-1".to_string(),
                worker_request_id: Some("worker-expired-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::UserInput,
                request_payload_json: json!({"prompt": "continue?"}),
                no_client_policy: "cancel".to_string(),
                timeout_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
            },
        )
        .await?;
    runtime
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "future-1".to_string(),
                run_id: "run-1".to_string(),
                worker_request_id: Some("worker-future-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({"command": "true"}),
                no_client_policy: "deny".to_string(),
                timeout_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            },
        )
        .await?;
    let snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(snapshot.pending_interaction_count, 2);
    assert_eq!(snapshot.last_event_seq, 2);

    assert_eq!(
        runtime
            .expire_background_agent_pending_interactions()
            .await?,
        1
    );
    let expired = runtime
        .get_background_agent_pending_interaction("expired-1")
        .await?
        .expect("expired interaction should exist");
    assert_eq!(
        expired.status,
        BackgroundAgentPendingInteractionStatus::Expired
    );
    assert_eq!(
        expired.response_payload_json,
        Some(json!({
            "reason": "timeout",
            "noClientPolicy": "cancel",
        }))
    );
    let future = runtime
        .get_background_agent_pending_interaction("future-1")
        .await?
        .expect("future interaction should exist");
    assert_eq!(
        future.status,
        BackgroundAgentPendingInteractionStatus::Pending
    );
    let snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(snapshot.pending_interaction_count, 1);
    assert_eq!(snapshot.last_event_seq, 3);

    let events = runtime
        .list_background_agent_events_after("run-1", /*after_seq*/ None, /*limit*/ None)
        .await?;
    assert_eq!(
        events
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            "interaction.created".to_string(),
            "interaction.created".to_string(),
            "interaction.expired".to_string(),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn supervisor_claim_heartbeat_and_snapshots_update_run() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
    assert_eq!(generation, 1);
    assert_eq!(
        runtime
            .claim_background_agent_supervisor("run-1", "supervisor-2", "lease-2",)
            .await?,
        None
    );
    assert!(
        runtime
            .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
                run_id: "run-1",
                supervisor_id: "supervisor-1",
                generation,
                pid: Some(100),
                pgid: Some(100),
                job_id: Some("job-1"),
                start_token: None,
                stderr_log_path: None,
            },)
            .await?
    );
    assert!(
        !runtime
            .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
                run_id: "run-1",
                supervisor_id: "supervisor-2",
                generation,
                pid: Some(200),
                pgid: Some(200),
                job_id: Some("job-2"),
                start_token: None,
                stderr_log_path: None,
            },)
            .await?
    );
    assert!(
        runtime
            .heartbeat_background_agent_run("run-1", "supervisor-1", generation)
            .await?
    );
    assert!(
        !runtime
            .heartbeat_background_agent_run("run-1", "supervisor-2", generation)
            .await?
    );

    let status_snapshot = runtime
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: "run-1".to_string(),
            seq: 1,
            status: BackgroundAgentRunStatus::Running,
            desired_state: BackgroundAgentDesiredState::Running,
            summary: Some("working".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 0,
            payload_json: json!({"phase": "running"}),
        })
        .await?;
    let execution_snapshot = runtime
        .create_background_agent_execution_snapshot(&BackgroundAgentExecutionSnapshotParams {
            run_id: "run-1".to_string(),
            snapshot_kind: "worker".to_string(),
            payload_json: json!({"cwd": "/tmp/worktree"}),
            recovery_policy: "resume_or_orphan".to_string(),
            config_fingerprint: Some("cfg-1".to_string()),
        })
        .await?;

    assert_eq!(status_snapshot.seq, 1);
    assert_eq!(execution_snapshot.seq, 1);
    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.supervisor_id, Some("supervisor-1".to_string()));
    assert_eq!(run.generation, 1);
    assert_eq!(run.pid, Some(100));
    assert_eq!(run.status, BackgroundAgentRunStatus::Starting);
    assert_eq!(run.last_snapshot_seq, 1);
    assert!(run.heartbeat_at.is_some());
    Ok(())
}

#[tokio::test]
async fn stale_supervisor_lease_is_orphaned_and_reclaimable() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let first_generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
    runtime
        .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
            run_id: "run-1",
            supervisor_id: "supervisor-1",
            generation: first_generation,
            pid: Some(100),
            pgid: Some(100),
            job_id: Some("job-1"),
            start_token: None,
            stderr_log_path: None,
        })
        .await?;

    assert_eq!(
        runtime
            .orphan_stale_background_agent_runs(Duration::ZERO)
            .await?,
        1
    );

    let orphaned = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(orphaned.status, BackgroundAgentRunStatus::Orphaned);
    assert_eq!(
        orphaned.status_reason,
        Some("supervisor heartbeat stale".to_string())
    );
    assert_eq!(
        orphaned.crash_reason,
        Some("supervisor heartbeat stale".to_string())
    );
    let process_lease_status: String = sqlx::query_scalar(
        "SELECT status FROM background_agent_process_leases WHERE run_id = ? AND generation = ?",
    )
    .bind("run-1")
    .bind(first_generation)
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(process_lease_status, "orphaned");
    assert!(
        !runtime
            .heartbeat_background_agent_run("run-1", "supervisor-1", first_generation)
            .await?
    );

    let second_generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-2", "lease-2")
        .await?
        .expect("orphaned run should be reclaimable");
    assert_eq!(second_generation, first_generation + 1);

    let events = runtime
        .list_background_agent_events_after("run-1", /*after_seq*/ None, /*limit*/ None)
        .await?;
    assert_eq!(
        events
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec!["agent.orphaned".to_string()]
    );
    Ok(())
}

#[tokio::test]
async fn stale_generation_cannot_update_status_or_create_interactions_after_reclaim()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let first_generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
    runtime
        .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
            run_id: "run-1",
            supervisor_id: "supervisor-1",
            generation: first_generation,
            pid: Some(100),
            pgid: Some(100),
            job_id: Some("job-1"),
            start_token: Some("start-1"),
            stderr_log_path: Some("/tmp/run-1.stderr.log"),
        })
        .await?;
    assert_eq!(
        runtime
            .orphan_stale_background_agent_runs(Duration::ZERO)
            .await?,
        1
    );
    let second_generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-2", "lease-2")
        .await?
        .expect("orphaned run should be reclaimed");
    assert_eq!(second_generation, first_generation + 1);

    assert!(
        !runtime
            .update_background_agent_run_status_for_supervisor(
                "run-1",
                "supervisor-1",
                first_generation,
                BackgroundAgentRunStatus::Completed,
                Some("stale completion"),
            )
            .await?
    );
    assert!(
        runtime
            .create_background_agent_pending_interaction_for_supervisor(
                &BackgroundAgentPendingInteractionCreateParams {
                    id: "stale-interaction".to_string(),
                    run_id: "run-1".to_string(),
                    worker_request_id: Some("stale-worker-request".to_string()),
                    kind: BackgroundAgentPendingInteractionKind::Approval,
                    request_payload_json: json!({"command": "true"}),
                    no_client_policy: "deny".to_string(),
                    timeout_at: None,
                },
                "supervisor-1",
                first_generation,
                BackgroundAgentRunStatus::WaitingOnApproval,
            )
            .await?
            .is_none()
    );

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.status, BackgroundAgentRunStatus::Starting);
    assert_eq!(run.supervisor_id.as_deref(), Some("supervisor-2"));
    assert_eq!(
        runtime
            .list_background_agent_pending_interactions("run-1", None)
            .await?,
        Vec::<BackgroundAgentPendingInteraction>::new()
    );
    Ok(())
}

#[tokio::test]
async fn stale_stopping_run_is_cancelled_and_lease_stopped() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
    runtime
        .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
            run_id: "run-1",
            supervisor_id: "supervisor-1",
            generation,
            pid: Some(100),
            pgid: Some(100),
            job_id: Some("job-1"),
            start_token: Some("start-1"),
            stderr_log_path: Some("/tmp/run-1.stderr.log"),
        })
        .await?;
    runtime
        .set_background_agent_desired_state("run-1", BackgroundAgentDesiredState::Stopped)
        .await?;
    runtime
        .update_background_agent_run_status(
            "run-1",
            BackgroundAgentRunStatus::Stopping,
            Some("stop requested"),
        )
        .await?;

    assert_eq!(
        runtime
            .orphan_stale_background_agent_runs(Duration::ZERO)
            .await?,
        1
    );

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.status, BackgroundAgentRunStatus::Cancelled);
    assert_eq!(run.status_reason.as_deref(), Some("stop heartbeat stale"));
    let process_lease_status: String = sqlx::query_scalar(
        "SELECT status FROM background_agent_process_leases WHERE run_id = ? AND generation = ?",
    )
    .bind("run-1")
    .bind(generation)
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(process_lease_status, "stopped");
    Ok(())
}

#[tokio::test]
async fn active_process_handles_include_persisted_start_token_and_stderr_path() -> anyhow::Result<()>
{
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
    runtime
        .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
            run_id: "run-1",
            supervisor_id: "supervisor-1",
            generation,
            pid: Some(100),
            pgid: Some(100),
            job_id: Some("job-1"),
            start_token: Some("start-1"),
            stderr_log_path: Some("/tmp/run-1.stderr.log"),
        })
        .await?;

    assert_eq!(
        runtime
            .list_background_agent_active_process_handles()
            .await?,
        vec![BackgroundAgentProcessHandleRecord {
            run_id: "run-1".to_string(),
            generation,
            pid: 100,
            pgid: Some(100),
            start_token: "start-1".to_string(),
            stderr_log_path: "/tmp/run-1.stderr.log".into(),
        }]
    );
    Ok(())
}

#[tokio::test]
async fn worktree_lease_records_workspace_and_protects_dirty_cleanup() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    let lease = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo/.git/worktrees/run-1".to_string(),
            branch: Some("codewith/bg-run-1".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await?;
    assert_eq!(lease.identity, "bg-run-1");
    assert_eq!(lease.mode, BackgroundAgentWorkspaceMode::IsolatedWorktree);
    assert_eq!(lease.dirty, false);

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.worktree_lease_id, Some("lease-1".to_string()));
    assert_eq!(
        runtime
            .get_background_agent_worktree_lease_for_run("run-1")
            .await?,
        Some(lease)
    );

    assert!(
        runtime
            .update_background_agent_worktree_lease_status(
                "lease-1",
                /*dirty*/ true,
                &json!({
                    "dirty": true,
                    "untracked": ["notes.txt"],
                }),
            )
            .await?
    );
    let retained = runtime
        .release_background_agent_worktree_lease(
            "lease-1",
            BackgroundAgentWorkspaceCleanup::DeleteIfClean,
        )
        .await?
        .expect("lease should exist");
    assert!(retained.dirty);
    assert!(retained.released_at.is_some());
    assert_eq!(retained.deleted_at, None);
    assert_eq!(retained.force_delete_requested, false);

    let tombstone: (String, i64) = sqlx::query_as(
        "SELECT reason, dirty_worktree FROM background_agent_cleanup_tombstones WHERE run_id = ?",
    )
    .bind("run-1")
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(tombstone, ("dirty worktree retained".to_string(), 1));

    let forced = runtime
        .release_background_agent_worktree_lease(
            "lease-1",
            BackgroundAgentWorkspaceCleanup::ForceDelete,
        )
        .await?
        .expect("lease should exist");
    assert!(forced.force_delete_requested);
    assert!(forced.deleted_at.is_some());
    Ok(())
}

#[tokio::test]
async fn shared_repository_leases_reject_parallel_runs_until_released() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run_with_id(runtime.as_ref(), "run-1").await?;
    create_run_with_id(runtime.as_ref(), "run-2").await?;

    runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::SharedRepository,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo".to_string(),
            branch: Some("main".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await?;

    let err = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::SharedRepository,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo".to_string(),
            branch: Some("main".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await
        .expect_err("parallel shared-repository lease should be rejected");
    assert!(
        err.to_string()
            .contains("shared repository /repo is already leased")
    );

    runtime
        .release_background_agent_worktree_lease("lease-1", BackgroundAgentWorkspaceCleanup::Retain)
        .await?;
    let lease = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::SharedRepository,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo".to_string(),
            branch: Some("main".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await?;
    assert_eq!(lease.run_id, "run-2");
    Ok(())
}

#[tokio::test]
async fn isolated_worktree_path_cannot_be_reused_until_deleted() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run_with_id(runtime.as_ref(), "run-1").await?;
    create_run_with_id(runtime.as_ref(), "run-2").await?;

    runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo/.git/worktrees/bg-run-1".to_string(),
            branch: Some("codewith/bg-run-1".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await?;

    let err = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo/.git/worktrees/bg-run-1".to_string(),
            branch: Some("codewith/bg-run-2".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await
        .expect_err("active isolated worktree path should be protected");
    assert!(
        err.to_string()
            .contains("isolated worktree path /repo/.git/worktrees/bg-run-1 is already leased")
    );

    runtime
        .release_background_agent_worktree_lease(
            "lease-1",
            BackgroundAgentWorkspaceCleanup::DeleteIfClean,
        )
        .await?;
    let lease = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: "/repo".to_string(),
            worktree_path: "/repo/.git/worktrees/bg-run-1".to_string(),
            branch: Some("codewith/bg-run-2".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({
                "branch": "main",
                "dirty": false,
                "untracked": [],
            }),
            dirty: false,
            cleanup_after: None,
        })
        .await?;
    assert_eq!(lease.run_id, "run-2");
    Ok(())
}
