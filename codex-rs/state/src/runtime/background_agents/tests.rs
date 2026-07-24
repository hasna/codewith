use super::*;
use crate::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;
use crate::BackgroundAgentExecutionHandleParams;
use crate::BackgroundAgentPendingInteractionKind;
use crate::BackgroundAgentWorkspaceMode;
use crate::DirectionalThreadSpawnEdgeStatus;
#[cfg(unix)]
use crate::runtime::managed_worktrees::managed_worktree_path_key;
use crate::runtime::managed_worktrees::path_to_db_string;
use crate::runtime::test_support::test_thread_metadata;
use crate::runtime::test_support::unique_temp_dir;
use pretty_assertions::assert_eq;
use serde_json::json;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

fn repo_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "codewith-background-agent-{}",
        name.trim_start_matches('/').replace('/', "-")
    ))
}

fn worktree_path(name: &str) -> PathBuf {
    repo_path("/repo").join(".git").join("worktrees").join(name)
}

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

fn admission_params(
    id: &str,
    idempotency_key: &str,
    auth_profile_ref: &str,
) -> BackgroundAgentRunCreateParams {
    BackgroundAgentRunCreateParams {
        id: id.to_string(),
        idempotency_key: Some(idempotency_key.to_string()),
        request_id: Some(format!("request-{idempotency_key}")),
        source: "admission-test".to_string(),
        prompt_snapshot_ref: format!("inline:{idempotency_key}:prompt"),
        input_snapshot_ref: None,
        thread_id: Some(format!("thread-{idempotency_key}")),
        thread_store_kind: "background-agent".to_string(),
        thread_store_id: Some("state.sqlite".to_string()),
        rollout_path: None,
        parent_thread_id: Some("parent-thread".to_string()),
        parent_agent_run_id: Some("parent-run".to_string()),
        spawn_linkage_json: Some(json!({"agentPath": ["worker"]})),
        auth_profile_ref: Some(auth_profile_ref.to_string()),
        status_reason: Some("queued by admission test".to_string()),
        config_fingerprint: Some("config-v1".to_string()),
        version_fingerprint: Some("codewith.background-agent.admission.v1".to_string()),
    }
}

async fn admit_run(
    runtime: &StateRuntime,
    params: &BackgroundAgentRunCreateParams,
    max_active_runs: i64,
) -> anyhow::Result<(BackgroundAgentRun, bool)> {
    let snapshot_params = admission_snapshot_params(params);
    let (run, created, _, _, _) = runtime
        .admit_background_agent_run(
            params,
            &admission_start_payload(params),
            &snapshot_params,
            max_active_runs,
        )
        .await?;
    Ok((run, created))
}

fn admission_start_payload(params: &BackgroundAgentRunCreateParams) -> serde_json::Value {
    let prompt = format!(
        "prompt for {}",
        params
            .idempotency_key
            .as_deref()
            .unwrap_or(params.id.as_str())
    );
    json!({
        "cwd": "/tmp/admission-test",
        "prompt": prompt,
        "promptSnapshotRef": params.prompt_snapshot_ref,
        "initialGoalObjective": "test admission",
    })
}

fn admission_snapshot_params(
    params: &BackgroundAgentRunCreateParams,
) -> BackgroundAgentExecutionSnapshotParams {
    BackgroundAgentExecutionSnapshotParams {
        run_id: params.id.clone(),
        snapshot_kind: "initial_execution_context".to_string(),
        payload_json: json!({
            "cwd": "/tmp/admission-test",
            "workspaceRoots": ["/tmp/admission-test"],
            "permissionProfile": {"type": "managed"},
            "networkPolicy": "restricted",
            "model": "test-model",
            "provider": "test-provider",
            "serviceTier": "default",
            "authProfileIdentitySha256": params.auth_profile_ref.as_deref().map(|profile| {
                StateRuntime::background_agent_identity_sha256(profile.as_bytes())
            }),
            "configFingerprint": params.config_fingerprint,
            "versionFingerprint": params.version_fingerprint,
            "packageFingerprint": "codex-state:test",
            "recoveryPolicy": "abort_mid_turn_resume_at_safe_boundary",
        }),
        recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
        config_fingerprint: params.config_fingerprint.clone(),
    }
}

#[tokio::test]
async fn background_agent_run_create_is_idempotent() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let first = create_run(runtime.as_ref()).await?;
    let second = runtime
        .create_background_agent_run(&BackgroundAgentRunCreateParams {
            id: "run-duplicate".to_string(),
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
            status_reason: None,
            config_fingerprint: Some("cfg-1".to_string()),
            version_fingerprint: Some("version-1".to_string()),
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
async fn background_agent_admission_create_or_adopt_is_atomic_and_receipted() -> anyhow::Result<()>
{
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let first_params = admission_params("admitted-1", "admission-key", "profile-a");
    let (first, created) =
        admit_run(runtime.as_ref(), &first_params, /*max_active_runs*/ 2).await?;
    assert!(created);

    let retry_params = admission_params("admitted-retry", "admission-key", "profile-a");
    let (retry, created) =
        admit_run(runtime.as_ref(), &retry_params, /*max_active_runs*/ 2).await?;
    assert!(!created);
    assert_eq!(retry.id, first.id);
    assert_eq!(runtime.list_background_agent_runs(None).await?.len(), 1);
    let events = runtime
        .list_background_agent_events_after(first.id.as_str(), None, None)
        .await?;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "agent.admitted");
    assert_eq!(events[1].event_type, "agent.started");
    assert_ne!(
        events[0]
            .payload_json
            .get("receiptKey")
            .and_then(serde_json::Value::as_str),
        Some("admission:admission-key")
    );
    assert!(
        runtime
            .get_latest_background_agent_execution_snapshot(first.id.as_str())
            .await?
            .is_some()
    );
    assert!(
        runtime
            .get_background_agent_status_snapshot(first.id.as_str())
            .await?
            .is_some()
    );
    runtime
        .create_background_agent_execution_snapshot(&BackgroundAgentExecutionSnapshotParams {
            run_id: first.id.clone(),
            snapshot_kind: "worker_thread_bound".to_string(),
            payload_json: json!({"threadId": "thread-after-admission"}),
            recovery_policy: "resume_or_orphan".to_string(),
            config_fingerprint: first.config_fingerprint.clone(),
        })
        .await?;
    assert_eq!(
        runtime
            .get_background_agent_initial_execution_snapshot(first.id.as_str())
            .await?
            .expect("initial execution context must remain authoritative")
            .snapshot_kind,
        "initial_execution_context"
    );
    sqlx::query(
        "DELETE FROM background_agent_lifecycle_receipts \
         WHERE run_id = ? AND event_type = 'agent.admitted'",
    )
    .bind(first.id.as_str())
    .execute(runtime.pool.as_ref())
    .await?;
    assert!(
        !runtime
            .background_agent_admission_is_ready(
                first.id.as_str(),
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
    );
    assert!(
        runtime
            .claim_background_agent_supervisor_compatible(
                first.id.as_str(),
                "supervisor-without-admission-receipt",
                "lease-without-admission-receipt",
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
            .is_none()
    );
    let (_, recovered) = admit_run(runtime.as_ref(), &retry_params, /*max_active_runs*/ 2).await?;
    assert!(!recovered);
    assert!(
        runtime
            .background_agent_admission_is_ready(
                first.id.as_str(),
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
    );
    sqlx::query(
        "DELETE FROM background_agent_execution_snapshots \
         WHERE run_id = ? AND snapshot_kind = 'initial_execution_context'",
    )
    .bind(first.id.as_str())
    .execute(runtime.pool.as_ref())
    .await?;
    assert!(
        !runtime
            .background_agent_admission_is_ready(
                first.id.as_str(),
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
    );
    assert!(
        runtime
            .claim_background_agent_supervisor_compatible(
                first.id.as_str(),
                "supervisor-after-corruption",
                "lease-after-corruption",
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn background_agent_admission_rejects_idempotency_identity_mismatch() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    admit_run(
        runtime.as_ref(),
        &admission_params("admitted-1", "admission-key", "profile-a"),
        2,
    )
    .await?;

    let error = admit_run(
        runtime.as_ref(),
        &admission_params("admitted-2", "admission-key", "profile-b"),
        2,
    )
    .await
    .expect_err("profile mismatch must not adopt the existing run");

    assert!(
        error
            .to_string()
            .contains("background_agent_admission_identity_mismatch")
    );
    assert_eq!(runtime.list_background_agent_runs(None).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn background_agent_admission_preserves_opaque_identity_values() -> anyhow::Result<()> {
    let sqlite_home = unique_temp_dir();
    let runtime = StateRuntime::init(sqlite_home.clone(), "test-provider".to_string()).await?;
    let idempotency_key = format!("{}{}", "sk-proj-", "a".repeat(32));
    let auth_profile_ref = format!("{}{}", "sk-proj-", "b".repeat(32));
    assert!(crate::local_state_string_contains_secret(&idempotency_key));
    assert!(crate::local_state_string_contains_secret(&auth_profile_ref));
    let mut params = admission_params(
        "opaque-admission",
        idempotency_key.as_str(),
        auth_profile_ref.as_str(),
    );
    params.request_id = Some("opaque-admission-request".to_string());
    params.prompt_snapshot_ref = "inline:opaque-admission:prompt".to_string();
    params.thread_id = Some("thread-opaque-admission".to_string());
    let (run, created) = admit_run(runtime.as_ref(), &params, /*max_active_runs*/ 2).await?;

    assert!(created);
    assert_eq!(
        run.idempotency_key.as_deref(),
        Some(idempotency_key.as_str())
    );
    assert_eq!(
        run.auth_profile_ref.as_deref(),
        Some(auth_profile_ref.as_str())
    );
    let stored = sqlx::query_as::<_, (String, String)>(
        "SELECT idempotency_key, auth_profile_ref FROM background_agent_runs WHERE id = ?",
    )
    .bind(run.id.as_str())
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_ne!(stored.0, idempotency_key);
    assert_ne!(stored.1, auth_profile_ref);
    assert_eq!(crate::redact_local_state_string(&stored.0), stored.0);
    assert_eq!(crate::redact_local_state_string(&stored.1), stored.1);
    let execution_snapshot = runtime
        .get_background_agent_initial_execution_snapshot(run.id.as_str())
        .await?
        .expect("admission snapshot should exist");
    assert_eq!(
        execution_snapshot
            .payload_json
            .get("authProfileIdentitySha256"),
        Some(&json!(StateRuntime::background_agent_identity_sha256(
            auth_profile_ref.as_bytes()
        )))
    );
    assert!(!serde_json::to_string(&execution_snapshot.payload_json)?.contains(&auth_profile_ref));
    let doctor_report =
        crate::run_local_state_secrets_doctor(crate::LocalStateSecretsDoctorOptions {
            codex_home: sqlite_home.clone(),
            sqlite_home,
            repair: true,
        })
        .await?;
    assert_eq!(doctor_report.redacted_sqlite_cells, 0);
    let after_doctor = runtime
        .get_background_agent_run(run.id.as_str())
        .await?
        .expect("run should survive secrets repair");
    assert_eq!(
        after_doctor.idempotency_key.as_deref(),
        Some(idempotency_key.as_str())
    );
    assert_eq!(
        after_doctor.auth_profile_ref.as_deref(),
        Some(auth_profile_ref.as_str())
    );
    assert_eq!(
        runtime
            .get_background_agent_run_by_idempotency_key(idempotency_key.as_str())
            .await?
            .map(|run| run.id),
        Some(run.id)
    );
    Ok(())
}

#[tokio::test]
async fn background_agent_admission_counts_only_live_or_recoverable_runs() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run_with_id(runtime.as_ref(), "legacy-incompatible").await?;
    assert_eq!(
        runtime.count_background_agent_runs_by_status().await?,
        Vec::<(BackgroundAgentRunStatus, i64)>::new()
    );
    let (first, _) = admit_run(
        runtime.as_ref(),
        &admission_params("admitted-1", "admission-key-1", "profile-a"),
        1,
    )
    .await?;
    let generation = runtime
        .claim_background_agent_supervisor(first.id.as_str(), "supervisor-1", "lease-1")
        .await?
        .expect("admitted run should be claimable");
    assert!(
        runtime
            .request_background_agent_stop_for_generation(
                first.id.as_str(),
                Some("supervisor-1"),
                generation,
                "capacity test stop",
                &json!({"reason": "capacity_test"}),
            )
            .await?
    );
    let error = admit_run(
        runtime.as_ref(),
        &admission_params("admitted-2", "admission-key-2", "profile-a"),
        1,
    )
    .await
    .expect_err("claimed stopping run must consume capacity");
    assert!(
        error
            .to_string()
            .contains("background_agent_admission_capacity_exceeded")
    );
    assert!(
        runtime
            .finalize_stopped_background_agent_process(
                first.id.as_str(),
                "supervisor-1",
                generation,
                "capacity test process stopped",
                &json!({"reason": "capacity_test_process_stopped"}),
            )
            .await?
    );
    let (_, created) = admit_run(
        runtime.as_ref(),
        &admission_params("admitted-2", "admission-key-2", "profile-a"),
        1,
    )
    .await?;
    assert!(created);

    runtime
        .update_background_agent_run_status(
            "admitted-2",
            BackgroundAgentRunStatus::Orphaned,
            Some("recoverable orphan"),
        )
        .await?;
    let error = admit_run(
        runtime.as_ref(),
        &admission_params("admitted-3", "admission-key-3", "profile-a"),
        1,
    )
    .await
    .expect_err("recoverable orphan must consume capacity");
    assert!(
        error
            .to_string()
            .contains("background_agent_admission_capacity_exceeded")
    );
    Ok(())
}

#[tokio::test]
async fn partial_admission_recovery_cannot_bypass_full_capacity() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let partial_params = admission_params("partial-run", "partial-key", "profile-a");
    runtime.create_background_agent_run(&partial_params).await?;
    let (active_run, created) = admit_run(
        runtime.as_ref(),
        &admission_params("active-run", "active-key", "profile-a"),
        /*max_active_runs*/ 1,
    )
    .await?;
    assert!(created);

    let error = admit_run(
        runtime.as_ref(),
        &partial_params,
        /*max_active_runs*/ 1,
    )
    .await
    .expect_err("recovering a partial admission must consume capacity atomically");
    assert!(
        error
            .to_string()
            .contains("background_agent_admission_capacity_exceeded")
    );
    assert!(
        runtime
            .list_background_agent_events_after(
                "partial-run",
                /*after_seq*/ None,
                /*limit*/ None,
            )
            .await?
            .is_empty()
    );
    assert!(
        !runtime
            .background_agent_admission_is_ready(
                "partial-run",
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
    );

    assert!(
        runtime
            .request_background_agent_stop_for_generation(
                active_run.id.as_str(),
                /*expected_supervisor_id*/ None,
                active_run.generation,
                "capacity released for partial recovery",
                &json!({"reason": "capacity_released"}),
            )
            .await?
    );
    let (recovered, created) = admit_run(
        runtime.as_ref(),
        &partial_params,
        /*max_active_runs*/ 1,
    )
    .await?;
    assert!(!created);
    assert_eq!(recovered.id, "partial-run");
    assert_eq!(
        runtime
            .list_background_agent_events_after(
                "partial-run",
                /*after_seq*/ None,
                /*limit*/ None,
            )
            .await?
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec!["agent.admitted".to_string(), "agent.started".to_string()]
    );
    assert!(
        runtime
            .background_agent_admission_is_ready(
                "partial-run",
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn partial_admission_recovery_requires_exact_persisted_execution_identity()
-> anyhow::Result<()> {
    let field_cases = [
        ("permission missing", "permissionProfile", None),
        (
            "permission different",
            "permissionProfile",
            Some(json!({"type": "danger-full-access"})),
        ),
        ("model missing", "model", None),
        ("model different", "model", Some(json!("other-model"))),
        ("workspace missing", "workspaceRoots", None),
        (
            "workspace different",
            "workspaceRoots",
            Some(json!(["/tmp/other-workspace"])),
        ),
        ("provider missing", "provider", None),
        (
            "provider different",
            "provider",
            Some(json!("other-provider")),
        ),
        ("service tier missing", "serviceTier", None),
        (
            "service tier different",
            "serviceTier",
            Some(json!("priority")),
        ),
        (
            "auth profile identity missing",
            "authProfileIdentitySha256",
            None,
        ),
        (
            "auth profile identity different",
            "authProfileIdentitySha256",
            Some(json!(StateRuntime::background_agent_identity_sha256(
                b"profile-b"
            ))),
        ),
        ("package missing", "packageFingerprint", None),
        (
            "package different",
            "packageFingerprint",
            Some(json!("codex-state:other")),
        ),
        ("recovery policy missing", "recoveryPolicy", None),
        (
            "recovery policy different",
            "recoveryPolicy",
            Some(json!("restart_from_beginning")),
        ),
    ];
    for (index, (case, field, stored_value)) in field_cases.into_iter().enumerate() {
        let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
        let params = admission_params(
            format!("partial-identity-{index}").as_str(),
            format!("partial-identity-key-{index}").as_str(),
            "profile-a",
        );
        runtime.create_background_agent_run(&params).await?;
        let mut stored_snapshot = admission_snapshot_params(&params);
        let stored_payload = stored_snapshot
            .payload_json
            .as_object_mut()
            .expect("test execution snapshot must be an object");
        match stored_value {
            Some(value) => {
                stored_payload.insert(field.to_string(), value);
            }
            None => {
                stored_payload.remove(field);
            }
        }
        runtime
            .create_background_agent_execution_snapshot(&stored_snapshot)
            .await?;

        let error = admit_run(runtime.as_ref(), &params, /*max_active_runs*/ 1)
            .await
            .expect_err(case);
        assert!(
            error
                .to_string()
                .contains("background_agent_admission_identity_mismatch"),
            "{case}: {error}"
        );
    }

    for (index, (case, recovery_policy, config_fingerprint)) in [
        (
            "recovery policy column different",
            "restart_from_beginning",
            Some("config-v1"),
        ),
        (
            "config fingerprint column missing",
            "abort_mid_turn_resume_at_safe_boundary",
            None,
        ),
        (
            "config fingerprint column different",
            "abort_mid_turn_resume_at_safe_boundary",
            Some("config-v2"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
        let params = admission_params(
            format!("partial-column-{index}").as_str(),
            format!("partial-column-key-{index}").as_str(),
            "profile-a",
        );
        runtime.create_background_agent_run(&params).await?;
        let mut stored_snapshot = admission_snapshot_params(&params);
        stored_snapshot.recovery_policy = recovery_policy.to_string();
        stored_snapshot.config_fingerprint = config_fingerprint.map(str::to_string);
        runtime
            .create_background_agent_execution_snapshot(&stored_snapshot)
            .await?;

        let error = admit_run(runtime.as_ref(), &params, /*max_active_runs*/ 1)
            .await
            .expect_err(case);
        assert!(
            error
                .to_string()
                .contains("background_agent_admission_identity_mismatch"),
            "{case}: {error}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn compatible_claim_rejects_persisted_runtime_package_skew() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let (run, created) = admit_run(
        runtime.as_ref(),
        &admission_params("package-run", "package-key", "profile-a"),
        /*max_active_runs*/ 1,
    )
    .await?;
    assert!(created);
    assert!(
        !runtime
            .background_agent_admission_is_ready(
                run.id.as_str(),
                "codewith.background-agent.admission.v1",
                "codex-state:next",
            )
            .await?
    );
    assert!(
        runtime
            .claim_background_agent_supervisor_compatible(
                run.id.as_str(),
                "newer-supervisor",
                "newer-lease",
                "codewith.background-agent.admission.v1",
                "codex-state:next",
            )
            .await?
            .is_none()
    );
    assert!(
        runtime
            .claim_background_agent_supervisor_compatible(
                run.id.as_str(),
                "compatible-supervisor",
                "compatible-lease",
                "codewith.background-agent.admission.v1",
                "codex-state:test",
            )
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn background_agent_lifecycle_receipts_dedupe_redact_and_bound_diagnostics()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let run = create_run(runtime.as_ref()).await?;
    let diagnostics = json!({
        "apiKey": "sk-secret-test-value",
        "blob": "x".repeat(8 * 1024),
    });

    let first = runtime
        .append_background_agent_lifecycle_receipt(
            run.id.as_str(),
            "agent.testReceipt",
            "test-receipt",
            1,
            Some(1),
            &diagnostics,
        )
        .await?;
    let conflict = runtime
        .append_background_agent_lifecycle_receipt(
            run.id.as_str(),
            "agent.testReceipt",
            "test-receipt",
            1,
            Some(2),
            &diagnostics,
        )
        .await
        .expect_err("receipt attempt mismatch must fail");
    assert!(
        conflict
            .to_string()
            .contains("background agent lifecycle receipt identity mismatch")
    );
    let redaction_collision = runtime
        .append_background_agent_lifecycle_receipt(
            run.id.as_str(),
            "agent.testReceipt",
            "test-receipt",
            1,
            Some(1),
            &json!({
                "apiKey": "sk-different-secret-value",
                "blob": "x".repeat(8 * 1024),
            }),
        )
        .await
        .expect_err("distinct raw diagnostics must not collapse after redaction");
    assert!(
        redaction_collision
            .to_string()
            .contains("background agent lifecycle receipt identity mismatch")
    );
    let retry = runtime
        .append_background_agent_lifecycle_receipt(
            run.id.as_str(),
            "agent.testReceipt",
            "test-receipt",
            1,
            Some(1),
            &diagnostics,
        )
        .await?;

    assert_eq!(retry.id, first.id);
    assert_eq!(retry.seq, first.seq);
    let serialized = serde_json::to_string(&retry.payload_json)?;
    assert!(!serialized.contains("sk-secret-test-value"));
    assert!(serialized.len() < 2 * 1024);
    assert_eq!(
        retry
            .payload_json
            .pointer("/diagnostics/truncated")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        runtime
            .compact_background_agent_events_before_seq("run-1", first.seq + 1)
            .await?,
        1
    );
    let compacted_retry = runtime
        .append_background_agent_lifecycle_receipt(
            run.id.as_str(),
            "agent.testReceipt",
            "test-receipt",
            1,
            Some(1),
            &diagnostics,
        )
        .await?;
    assert_eq!(compacted_retry, first);

    let oversized_receipt_key = "x".repeat(300);
    let error = runtime
        .append_background_agent_lifecycle_receipt(
            run.id.as_str(),
            "agent.oversizedReceipt",
            oversized_receipt_key.as_str(),
            1,
            Some(1),
            &json!({}),
        )
        .await
        .expect_err("caller-controlled receipt keys must be bounded");
    assert!(
        error
            .to_string()
            .contains("background agent lifecycle receipt key exceeds")
    );
    Ok(())
}

#[tokio::test]
async fn stop_and_delete_retries_replay_exact_lifecycle_operation_identity() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let run = create_run_with_id(runtime.as_ref(), "stop-retry").await?;
    let stop_diagnostics = json!({"reason": "operator_requested"});
    assert!(
        runtime
            .request_background_agent_stop_for_generation(
                run.id.as_str(),
                /*expected_supervisor_id*/ None,
                run.generation,
                "operator requested stop",
                &stop_diagnostics,
            )
            .await?
    );
    let first_stop_events = runtime
        .list_background_agent_events_after(run.id.as_str(), None, None)
        .await?;
    assert!(
        runtime
            .request_background_agent_stop_for_generation(
                run.id.as_str(),
                /*expected_supervisor_id*/ None,
                run.generation,
                "operator requested stop",
                &stop_diagnostics,
            )
            .await?
    );
    assert_eq!(
        runtime
            .list_background_agent_events_after(run.id.as_str(), None, None)
            .await?,
        first_stop_events
    );
    for (status_reason, diagnostics) in [
        ("different stop reason", stop_diagnostics.clone()),
        (
            "operator requested stop",
            json!({"reason": "different_request"}),
        ),
    ] {
        let error = runtime
            .request_background_agent_stop_for_generation(
                run.id.as_str(),
                /*expected_supervisor_id*/ None,
                run.generation,
                status_reason,
                &diagnostics,
            )
            .await
            .expect_err("conflicting stop receipt retry must fail closed");
        assert!(
            error
                .to_string()
                .contains("background agent lifecycle receipt identity mismatch")
        );
    }

    let delete_run = create_run_with_id(runtime.as_ref(), "delete-retry").await?;
    assert!(
        runtime
            .request_background_agent_delete(delete_run.id.as_str())
            .await?
    );
    let first_delete_events = runtime
        .list_background_agent_events_after(delete_run.id.as_str(), None, None)
        .await?;
    assert!(
        runtime
            .request_background_agent_delete(delete_run.id.as_str())
            .await?
    );
    assert_eq!(
        runtime
            .list_background_agent_events_after(delete_run.id.as_str(), None, None)
            .await?,
        first_delete_events
    );
    sqlx::query("UPDATE background_agent_runs SET status_reason = ? WHERE id = ?")
        .bind("conflicting delete reason")
        .bind(delete_run.id.as_str())
        .execute(runtime.pool.as_ref())
        .await?;
    let error = runtime
        .request_background_agent_delete(delete_run.id.as_str())
        .await
        .expect_err("conflicting persisted delete reason must fail receipt replay");
    assert!(
        error
            .to_string()
            .contains("background agent lifecycle receipt identity mismatch")
    );
    Ok(())
}

#[tokio::test]
async fn background_agent_terminal_status_receipt_replays_after_commit_and_compaction()
-> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    let run = create_run(runtime.as_ref()).await?;
    let generation = runtime
        .claim_background_agent_supervisor(run.id.as_str(), "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimable");
    let event_payload = json!({"outcome": "completed"});
    let status_payload = json!({"phase": "completed"});
    let params = || BackgroundAgentStatusEventForSupervisorParams {
        run_id: run.id.as_str(),
        supervisor_id: "supervisor-1",
        generation,
        status: BackgroundAgentRunStatus::Completed,
        status_reason: Some("worker completed"),
        event_type: "agent.completed",
        event_payload_json: &event_payload,
        summary: Some("Completed"),
        pending_interaction_count: 0,
        status_payload_json: &status_payload,
    };

    let first = runtime
        .append_background_agent_status_event_for_supervisor(params())
        .await?
        .expect("current generation should complete");
    let conflict = runtime
        .append_background_agent_status_event_for_supervisor(
            BackgroundAgentStatusEventForSupervisorParams {
                status_reason: Some("different terminal outcome"),
                ..params()
            },
        )
        .await
        .expect_err("terminal receipt replay must bind the full projected operation");
    assert!(
        conflict
            .to_string()
            .contains("background agent lifecycle receipt identity mismatch")
    );
    let retry = runtime
        .append_background_agent_status_event_for_supervisor(params())
        .await?
        .expect("terminal receipt should replay after an ambiguous acknowledgement");
    assert_eq!(retry, first);

    assert!(
        runtime
            .compact_background_agent_events_before_seq(run.id.as_str(), first.seq + 1)
            .await?
            > 0
    );
    let compacted_retry = runtime
        .append_background_agent_status_event_for_supervisor(params())
        .await?
        .expect("terminal receipt should survive event compaction");
    assert_eq!(compacted_retry, first);
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
        vec![
            "agent.claimed".to_string(),
            "agent.heartbeat".to_string(),
            "agent.orphaned".to_string(),
            "agent.claimed".to_string()
        ]
    );
    Ok(())
}

#[tokio::test]
async fn orphaning_waiting_run_terminalizes_pending_interactions() -> anyhow::Result<()> {
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
    runtime
        .create_background_agent_pending_interaction_for_supervisor(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "pending-1".to_string(),
                run_id: "run-1".to_string(),
                worker_request_id: Some("worker-request-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({"command": "deploy"}),
                no_client_policy: "deny".to_string(),
                timeout_at: None,
            },
            "supervisor-1",
            first_generation,
            BackgroundAgentRunStatus::WaitingOnApproval,
        )
        .await?
        .expect("pending interaction should be created");

    assert_eq!(
        runtime
            .orphan_stale_background_agent_runs(Duration::ZERO)
            .await?,
        1
    );

    let interaction = runtime
        .get_background_agent_pending_interaction("pending-1")
        .await?
        .expect("interaction should exist");
    assert_eq!(
        interaction.status,
        BackgroundAgentPendingInteractionStatus::WorkerNoLongerWaiting
    );
    let status_snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(status_snapshot.status, BackgroundAgentRunStatus::Orphaned);
    assert_eq!(status_snapshot.pending_interaction_count, 0);
    assert_eq!(status_snapshot.last_event_seq, 5);
    assert_eq!(
        runtime
            .list_background_agent_events_after(
                "run-1", /*after_seq*/ None, /*limit*/ None
            )
            .await?
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            "agent.claimed".to_string(),
            "agent.heartbeat".to_string(),
            "interaction.created".to_string(),
            "interaction.workerNoLongerWaiting".to_string(),
            "agent.orphaned".to_string()
        ]
    );

    let second_generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-2", "lease-2")
        .await?
        .expect("orphaned run should be reclaimable");
    assert_eq!(second_generation, first_generation + 1);
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
    let handle_conflict = runtime
        .record_background_agent_execution_handle(BackgroundAgentExecutionHandleParams {
            run_id: "run-1",
            supervisor_id: "supervisor-1",
            generation: first_generation,
            pid: Some(200),
            pgid: Some(200),
            job_id: Some("different-job"),
            start_token: Some("different-start"),
            stderr_log_path: Some("/tmp/different.stderr.log"),
        })
        .await
        .expect_err("execution handle receipt must bind the exact operation");
    assert!(
        handle_conflict
            .to_string()
            .contains("background agent lifecycle receipt identity mismatch")
    );
    assert_eq!(
        runtime
            .get_background_agent_run("run-1")
            .await?
            .expect("run should remain current")
            .pid,
        Some(100)
    );
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
    assert!(
        runtime
            .append_background_agent_event_for_supervisor(
                "run-1",
                "supervisor-1",
                first_generation,
                "agent.staleEvent",
                &json!({"generation": first_generation}),
                /*allow_terminal_current*/ false,
            )
            .await?
            .is_none()
    );
    assert!(
        runtime
            .create_background_agent_execution_snapshot_for_supervisor(
                &BackgroundAgentExecutionSnapshotParams {
                    run_id: "run-1".to_string(),
                    snapshot_kind: "stale_generation".to_string(),
                    payload_json: json!({"generation": first_generation}),
                    recovery_policy: "resume_or_orphan".to_string(),
                    config_fingerprint: None,
                },
                "supervisor-1",
                first_generation,
            )
            .await?
            .is_none()
    );
    assert!(
        runtime
            .upsert_background_agent_status_snapshot_for_supervisor(
                &BackgroundAgentStatusSnapshotParams {
                    run_id: "run-1".to_string(),
                    seq: 99,
                    status: BackgroundAgentRunStatus::Completed,
                    desired_state: BackgroundAgentDesiredState::Running,
                    summary: Some("stale completion".to_string()),
                    pending_interaction_count: 0,
                    last_event_seq: 99,
                    payload_json: json!({"generation": first_generation}),
                },
                "supervisor-1",
                first_generation,
            )
            .await?
            .is_none()
    );
    assert!(
        runtime
            .append_background_agent_status_event_for_supervisor(
                BackgroundAgentStatusEventForSupervisorParams {
                    run_id: "run-1",
                    supervisor_id: "supervisor-1",
                    generation: first_generation,
                    status: BackgroundAgentRunStatus::Completed,
                    status_reason: Some("stale completion"),
                    event_type: "agent.completed",
                    event_payload_json: &json!({"generation": first_generation}),
                    summary: Some("Completed"),
                    pending_interaction_count: 0,
                    status_payload_json: &json!({"phase": "completed"}),
                },
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
            .list_background_agent_pending_interactions("run-1", /*status*/ None)
            .await?,
        Vec::<BackgroundAgentPendingInteraction>::new()
    );
    Ok(())
}

#[tokio::test]
async fn stale_generation_cannot_cancel_reclaimed_run() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    let first_generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
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

    assert!(
        !runtime
            .request_background_agent_stop_for_generation(
                "run-1",
                Some("supervisor-1"),
                first_generation,
                "stale stop",
                &json!({"reason": "stale_stop"}),
            )
            .await?
    );
    let running = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(running.desired_state, BackgroundAgentDesiredState::Running);
    assert_eq!(running.supervisor_id.as_deref(), Some("supervisor-2"));

    assert!(
        runtime
            .request_background_agent_stop_for_generation(
                "run-1",
                Some("supervisor-2"),
                second_generation,
                "current stop",
                &json!({"reason": "current_stop"}),
            )
            .await?
    );
    let stopped = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(stopped.desired_state, BackgroundAgentDesiredState::Stopped);
    assert_eq!(stopped.status, BackgroundAgentRunStatus::Stopping);
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
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "pending-1".to_string(),
                run_id: "run-1".to_string(),
                worker_request_id: Some("worker-request-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({"command": "deploy"}),
                no_client_policy: "deny".to_string(),
                timeout_at: None,
            },
        )
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
    runtime
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: "run-1".to_string(),
            seq: 1,
            status: BackgroundAgentRunStatus::Stopping,
            desired_state: BackgroundAgentDesiredState::Stopped,
            summary: Some("stop requested".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 0,
            payload_json: json!({"phase": "stopping"}),
        })
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
    let status_snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(status_snapshot.status, BackgroundAgentRunStatus::Cancelled);
    assert_eq!(
        status_snapshot.desired_state,
        BackgroundAgentDesiredState::Stopped
    );
    assert_eq!(status_snapshot.pending_interaction_count, 0);
    assert_eq!(
        status_snapshot.summary.as_deref(),
        Some("stop heartbeat stale")
    );
    assert_eq!(status_snapshot.last_event_seq, 5);
    let interaction = runtime
        .get_background_agent_pending_interaction("pending-1")
        .await?
        .expect("interaction should exist");
    assert_eq!(
        interaction.status,
        BackgroundAgentPendingInteractionStatus::Cancelled
    );
    Ok(())
}

#[tokio::test]
async fn stopped_worker_process_is_cancelled_and_lease_stopped_immediately() -> anyhow::Result<()> {
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
        .create_background_agent_pending_interaction(
            &BackgroundAgentPendingInteractionCreateParams {
                id: "pending-1".to_string(),
                run_id: "run-1".to_string(),
                worker_request_id: Some("worker-request-1".to_string()),
                kind: BackgroundAgentPendingInteractionKind::Approval,
                request_payload_json: json!({"command": "deploy"}),
                no_client_policy: "deny".to_string(),
                timeout_at: None,
            },
        )
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

    assert!(
        runtime
            .finalize_stopped_background_agent_process(
                "run-1",
                "supervisor-1",
                generation,
                "worker process stopped after stop request",
                &json!({
                    "reason": "worker_process_stopped_after_desired_state_change",
                }),
            )
            .await?
    );
    assert!(
        !runtime
            .finalize_stopped_background_agent_process(
                "run-1",
                "supervisor-1",
                generation,
                "worker process stopped after stop request",
                &json!({
                    "reason": "worker_process_stopped_after_desired_state_change",
                }),
            )
            .await?
    );

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.status, BackgroundAgentRunStatus::Cancelled);
    assert_eq!(
        run.status_reason.as_deref(),
        Some("worker process stopped after stop request")
    );
    let process_lease: (String, Option<String>) = sqlx::query_as(
        "SELECT status, exit_reason FROM background_agent_process_leases WHERE run_id = ? AND generation = ?",
    )
    .bind("run-1")
    .bind(generation)
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(
        process_lease,
        (
            "stopped".to_string(),
            Some("worker process stopped after stop request".to_string())
        )
    );
    let status_snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(status_snapshot.status, BackgroundAgentRunStatus::Cancelled);
    assert_eq!(
        status_snapshot.desired_state,
        BackgroundAgentDesiredState::Stopped
    );
    assert_eq!(status_snapshot.pending_interaction_count, 0);
    assert_eq!(
        status_snapshot.summary.as_deref(),
        Some("worker process stopped after stop request")
    );
    assert_eq!(status_snapshot.last_event_seq, run.last_event_seq);
    let interaction = runtime
        .get_background_agent_pending_interaction("pending-1")
        .await?
        .expect("interaction should exist");
    assert_eq!(
        interaction.status,
        BackgroundAgentPendingInteractionStatus::Cancelled
    );
    Ok(())
}

#[tokio::test]
async fn unclaimed_worker_process_spawn_failure_marks_run_failed() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;

    assert!(
        runtime
            .fail_unclaimed_background_agent_process_spawn(
                "run-1",
                "worker process exited before claiming run",
                &json!({
                    "reason": "worker_process_exited_before_claim",
                }),
            )
            .await?
    );
    assert!(
        !runtime
            .fail_unclaimed_background_agent_process_spawn(
                "run-1",
                "worker process exited before claiming run",
                &json!({
                    "reason": "worker_process_exited_before_claim",
                }),
            )
            .await?
    );

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.status, BackgroundAgentRunStatus::Failed);
    assert_eq!(
        run.status_reason.as_deref(),
        Some("worker process exited before claiming run")
    );
    assert_eq!(
        run.crash_reason.as_deref(),
        Some("worker process exited before claiming run")
    );
    let status_snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(status_snapshot.status, BackgroundAgentRunStatus::Failed);
    assert_eq!(status_snapshot.last_event_seq, run.last_event_seq);
    Ok(())
}

#[tokio::test]
async fn claimed_queued_run_is_not_failed_as_unclaimed_process_spawn() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    let generation = runtime
        .claim_background_agent_supervisor("run-1", "supervisor-1", "lease-1")
        .await?
        .expect("run should be claimed");
    runtime
        .append_background_agent_status_event_for_supervisor(
            BackgroundAgentStatusEventForSupervisorParams {
                run_id: "run-1",
                supervisor_id: "supervisor-1",
                generation,
                status: BackgroundAgentRunStatus::Queued,
                status_reason: Some("waiting for usage profile reset"),
                event_type: "agent.usageProfileWait",
                event_payload_json: &json!({"retryAt": 100}),
                summary: Some("Queued"),
                pending_interaction_count: 0,
                status_payload_json: &json!({"phase": "usage profile wait"}),
            },
        )
        .await?
        .expect("run should accept queued status from supervisor");

    assert!(
        !runtime
            .fail_unclaimed_background_agent_process_spawn(
                "run-1",
                "worker process exited before claiming run",
                &json!({
                    "reason": "worker_process_exited_before_claim",
                }),
            )
            .await?
    );

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.status, BackgroundAgentRunStatus::Queued);
    assert_eq!(
        run.status_reason.as_deref(),
        Some("waiting for usage profile reset")
    );
    Ok(())
}

#[tokio::test]
async fn orphaned_run_is_not_failed_as_unclaimed_process_spawn() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    runtime
        .claim_background_agent_supervisor("run-1", "old-supervisor", "lease-1")
        .await?
        .expect("run should be claimed");
    runtime
        .update_background_agent_run_status(
            "run-1",
            BackgroundAgentRunStatus::Orphaned,
            Some("supervisor heartbeat stale"),
        )
        .await?;

    assert!(
        !runtime
            .fail_unclaimed_background_agent_process_spawn(
                "run-1",
                "worker process exited before claiming run",
                &json!({
                    "reason": "worker_process_exited_before_claim",
                }),
            )
            .await?
    );

    let run = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(run.status, BackgroundAgentRunStatus::Orphaned);
    assert_eq!(
        run.status_reason.as_deref(),
        Some("supervisor heartbeat stale")
    );
    Ok(())
}

#[tokio::test]
async fn delete_request_for_claimed_run_becomes_stopping_and_stale_cancelled() -> anyhow::Result<()>
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

    assert!(runtime.request_background_agent_delete("run-1").await?);
    let stopping = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(stopping.status, BackgroundAgentRunStatus::Stopping);
    assert_eq!(stopping.status_reason.as_deref(), Some("delete requested"));
    runtime
        .upsert_background_agent_status_snapshot(&BackgroundAgentStatusSnapshotParams {
            run_id: "run-1".to_string(),
            seq: 1,
            status: BackgroundAgentRunStatus::Stopping,
            desired_state: BackgroundAgentDesiredState::Deleted,
            summary: Some("delete requested".to_string()),
            pending_interaction_count: 0,
            last_event_seq: 0,
            payload_json: json!({"phase": "delete requested"}),
        })
        .await?;

    assert_eq!(
        runtime
            .orphan_stale_background_agent_runs(Duration::ZERO)
            .await?,
        1
    );

    let cancelled = runtime
        .get_background_agent_run("run-1")
        .await?
        .expect("run should exist");
    assert_eq!(cancelled.status, BackgroundAgentRunStatus::Cancelled);
    assert_eq!(
        cancelled.retention_state,
        crate::BackgroundAgentRetentionState::DeleteRequested
    );
    let process_lease_status: String = sqlx::query_scalar(
        "SELECT status FROM background_agent_process_leases WHERE run_id = ? AND generation = ?",
    )
    .bind("run-1")
    .bind(generation)
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(process_lease_status, "stopped");
    let status_snapshot = runtime
        .get_background_agent_status_snapshot("run-1")
        .await?
        .expect("status snapshot should exist");
    assert_eq!(status_snapshot.status, BackgroundAgentRunStatus::Cancelled);
    assert_eq!(
        status_snapshot.desired_state,
        BackgroundAgentDesiredState::Deleted
    );
    assert_eq!(
        status_snapshot.summary.as_deref(),
        Some("stop heartbeat stale")
    );
    let events = runtime
        .list_background_agent_events_after("run-1", /*after_seq*/ None, /*limit*/ None)
        .await?;
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "agent.claimed",
            "agent.heartbeat",
            "agent.deleteRequested",
            "agent.cancelled",
        ]
    );
    assert_eq!(
        status_snapshot.last_event_seq,
        events.last().expect("terminal event should exist").seq
    );

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

#[cfg(unix)]
#[tokio::test]
async fn background_agent_worktree_lease_writes_native_path_keys_immediately() -> anyhow::Result<()>
{
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    let repo = PathBuf::from(OsString::from_vec(b"/repo/\xffbase".to_vec()));
    let worktree = PathBuf::from(OsString::from_vec(
        b"/repo/\xffbase/.git/worktrees/agent/../lease-\xfe".to_vec(),
    ));

    runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-native-path-keys".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: repo.clone(),
            worktree_path: worktree.clone(),
            branch: Some("codewith/bg-run-1".to_string()),
            head_sha: Some("abc123".to_string()),
            status_snapshot_json: json!({"dirty": false}),
            dirty: false,
            cleanup_after: None,
        })
        .await?;

    let (_, persisted_worktree_path, base_repo_path_key, worktree_path_key):
        (String, String, Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT base_repo_path, worktree_path, base_repo_path_key, worktree_path_key FROM managed_worktrees WHERE worktree_id = ?",
    )
    .bind("lease-native-path-keys")
    .fetch_one(runtime.pool.as_ref())
    .await?;

    assert_eq!(base_repo_path_key, managed_worktree_path_key(&repo));
    assert_eq!(worktree_path_key, managed_worktree_path_key(&worktree));
    assert_ne!(
        worktree_path_key,
        managed_worktree_path_key(PathBuf::from(persisted_worktree_path).as_path())
    );
    Ok(())
}

#[tokio::test]
async fn worktree_lease_records_workspace_and_protects_dirty_cleanup() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run(runtime.as_ref()).await?;
    let repo = repo_path("/repo");
    let worktree = worktree_path("run-1");

    let lease = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: repo,
            worktree_path: worktree,
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
    let active_assignment: (i64,) = sqlx::query_as(
        r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ? AND agent_run_id = ? AND detached_at_ms IS NULL
        "#,
    )
    .bind("lease-1")
    .bind("run-1")
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(active_assignment, (1,));
    assert!(
        runtime
            .managed_worktrees()
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "lease-1".to_string(),
                target: ManagedWorktreeAssignmentTarget::AgentRun("run-2".to_string()),
            })
            .await
            .expect_err("background worktree owner should reject another agent")
            .to_string()
            .contains("cannot be assigned to agent run run-2")
    );
    runtime
        .managed_worktrees()
        .attach_managed_worktree(ManagedWorktreeAttachParams {
            worktree_id: "lease-1".to_string(),
            target: ManagedWorktreeAssignmentTarget::AgentRun("run-1".to_string()),
        })
        .await?;

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
    let managed_retained = runtime
        .managed_worktrees()
        .get_managed_worktree("lease-1")
        .await?
        .expect("managed worktree should exist");
    assert_eq!(
        managed_retained.lifecycle_status,
        crate::ManagedWorktreeLifecycleStatus::CleanupPending
    );
    assert!(managed_retained.dirty);
    assert_eq!(managed_retained.deleted_at, None);
    let active_assignment_after_release: (i64,) = sqlx::query_as(
        r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ? AND detached_at_ms IS NULL
        "#,
    )
    .bind("lease-1")
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(active_assignment_after_release, (0,));

    let tombstone: (String, i64) = sqlx::query_as(
        "SELECT reason, dirty_worktree FROM background_agent_cleanup_tombstones WHERE run_id = ?",
    )
    .bind("run-1")
    .fetch_one(runtime.pool.as_ref())
    .await?;
    assert_eq!(tombstone, ("worktree cleanup pending".to_string(), 1));

    let forced = runtime
        .release_background_agent_worktree_lease(
            "lease-1",
            BackgroundAgentWorkspaceCleanup::ForceDelete,
        )
        .await?
        .expect("lease should exist");
    assert!(forced.force_delete_requested);
    assert_eq!(forced.deleted_at, None);
    let managed_forced = runtime
        .managed_worktrees()
        .get_managed_worktree("lease-1")
        .await?
        .expect("managed worktree should exist");
    assert_eq!(
        managed_forced.lifecycle_status,
        crate::ManagedWorktreeLifecycleStatus::CleanupPending
    );
    assert!(managed_forced.force_delete_requested);
    assert_eq!(managed_forced.deleted_at, None);
    assert_eq!(
        runtime
            .managed_worktrees()
            .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
            .await?,
        vec![managed_forced]
    );

    let deleted = runtime
        .mark_managed_worktree_cleanup_succeeded("lease-1")
        .await?
        .expect("cleanup success should mark the worktree deleted");
    assert_eq!(
        deleted.lifecycle_status,
        crate::ManagedWorktreeLifecycleStatus::Deleted
    );
    assert!(deleted.deleted_at.is_some());
    let deleted_lease = runtime
        .get_background_agent_worktree_lease("lease-1")
        .await?
        .expect("lease should remain readable after cleanup");
    assert!(deleted_lease.deleted_at.is_some());
    Ok(())
}

#[tokio::test]
async fn shared_repository_leases_reject_parallel_runs_until_released() -> anyhow::Result<()> {
    let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string()).await?;
    create_run_with_id(runtime.as_ref(), "run-1").await?;
    create_run_with_id(runtime.as_ref(), "run-2").await?;
    let repo = repo_path("/repo");
    let repo_display = path_to_db_string(&repo);

    runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::SharedRepository,
            base_repo_path: repo.clone(),
            worktree_path: repo.clone(),
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
            base_repo_path: repo.clone(),
            worktree_path: repo.clone(),
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
    assert!(err.to_string().contains(&format!(
        "shared repository {repo_display} is already leased"
    )));

    runtime
        .release_background_agent_worktree_lease("lease-1", BackgroundAgentWorkspaceCleanup::Retain)
        .await?;
    let lease = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::SharedRepository,
            base_repo_path: repo.clone(),
            worktree_path: repo,
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
    let repo = repo_path("/repo");
    let worktree = worktree_path("bg-run-1");
    let worktree_display = path_to_db_string(&worktree);

    runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
            identity: "bg-run-1".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: repo.clone(),
            worktree_path: worktree.clone(),
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
            base_repo_path: repo.clone(),
            worktree_path: worktree.clone(),
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
    assert!(err.to_string().contains(&format!(
        "isolated worktree path {worktree_display} is already leased"
    )));

    runtime
        .release_background_agent_worktree_lease(
            "lease-1",
            BackgroundAgentWorkspaceCleanup::DeleteIfClean,
        )
        .await?;
    let still_protected = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: repo.clone(),
            worktree_path: worktree.clone(),
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
        .expect_err("released isolated worktree path should stay protected until cleanup succeeds");
    assert!(still_protected.to_string().contains(&format!(
        "isolated worktree path {worktree_display} is already leased"
    )));

    let cleanup_candidate = runtime
        .managed_worktrees()
        .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
        .await?;
    assert_eq!(cleanup_candidate.len(), 1);
    assert_eq!(cleanup_candidate[0].worktree_id, "lease-1");

    runtime
        .mark_managed_worktree_cleanup_succeeded("lease-1")
        .await?
        .expect("cleanup success should mark the isolated path deleted");
    let lease = runtime
        .create_background_agent_worktree_lease(&BackgroundAgentWorktreeLeaseCreateParams {
            id: "lease-2".to_string(),
            run_id: "run-2".to_string(),
            identity: "bg-run-2".to_string(),
            mode: BackgroundAgentWorkspaceMode::IsolatedWorktree,
            base_repo_path: repo,
            worktree_path: worktree,
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
