use super::*;
use codex_app_server_protocol::AgentDesiredState;
use codex_app_server_protocol::AgentRetentionState;
use codex_app_server_protocol::AgentRun;
use codex_app_server_protocol::AgentRunStatus;
use codex_app_server_protocol::Worktree;
use codex_app_server_protocol::WorktreeCleanupPolicy;
use codex_app_server_protocol::WorktreeLifecycleStatus;
use codex_app_server_protocol::WorktreeMode;
use codex_app_server_protocol::WorktreeOwnerKind;
use codex_app_server_protocol::WorktreePolicy;
use codex_app_server_protocol::WorktreeSessionMode;

#[tokio::test]
async fn worktree_manager_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_manager(
        vec![
            test_worktree(
                "018f-active-worktree",
                WorktreeLifecycleStatus::Active,
                WorktreeOwnerKind::MainSession,
                Some("thread-main-session"),
                None,
            ),
            test_worktree(
                "018f-cleanup-pending",
                WorktreeLifecycleStatus::CleanupPending,
                WorktreeOwnerKind::BackgroundAgent,
                None,
                Some("agent-cleanup-pending"),
            ),
            test_worktree(
                "018f-deleted-worktree",
                WorktreeLifecycleStatus::Deleted,
                WorktreeOwnerKind::Manual,
                None,
                None,
            ),
        ],
        test_policy(),
    );

    assert_chatwidget_snapshot!(
        "worktree_manager",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn worktree_manager_empty_no_repo_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_manager(Vec::new(), test_no_repo_policy());

    assert_chatwidget_snapshot!(
        "worktree_manager_empty_no_repo",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn worktree_read_selector_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_read_selector(
        vec![
            test_worktree(
                "018f-active-worktree",
                WorktreeLifecycleStatus::Active,
                WorktreeOwnerKind::MainSession,
                Some("thread-main-session"),
                None,
            ),
            test_worktree(
                "018f-cleanup-pending",
                WorktreeLifecycleStatus::CleanupPending,
                WorktreeOwnerKind::BackgroundAgent,
                None,
                Some("agent-cleanup-pending"),
            ),
        ],
        test_policy(),
    );

    assert_chatwidget_snapshot!(
        "worktree_read_selector",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn worktree_read_selector_emits_read_event_with_repo_scope() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_read_selector(
        vec![test_worktree(
            "018f-active-worktree",
            WorktreeLifecycleStatus::Active,
            WorktreeOwnerKind::MainSession,
            Some("thread-main-session"),
            None,
        )],
        test_policy(),
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::ReadWorktree {
            worktree_id: Some(worktree_id),
            base_repo_path: Some(base_repo_path),
        }) if worktree_id == "018f-active-worktree" && base_repo_path == test_path_display("/tmp/project")
    );
}

#[tokio::test]
async fn worktree_actions_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_actions(
        test_worktree(
            "018f-actions-worktree",
            WorktreeLifecycleStatus::Active,
            WorktreeOwnerKind::SubSession,
            Some("thread-sub-session"),
            Some("agent-actions-owner"),
        ),
        test_policy(),
    );

    assert_chatwidget_snapshot!(
        "worktree_actions",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn worktree_actions_use_emits_event_with_repo_scope() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_actions(
        test_worktree(
            "018f-actions-worktree",
            WorktreeLifecycleStatus::Active,
            WorktreeOwnerKind::SubSession,
            Some("thread-sub-session"),
            Some("agent-actions-owner"),
        ),
        test_policy(),
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::UseWorktree {
            worktree_id,
            base_repo_path: Some(base_repo_path),
        }) if worktree_id == "018f-actions-worktree" && base_repo_path == test_path_display("/tmp/project")
    );
}

#[tokio::test]
async fn worktree_read_detail_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_worktree_read(test_worktree(
        "018f-actions-worktree",
        WorktreeLifecycleStatus::CleanupPending,
        WorktreeOwnerKind::BackgroundAgent,
        None,
        Some("agent-actions-owner"),
    ));

    let combined = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<String>();
    assert_chatwidget_snapshot!("worktree_read_detail", combined);
}

fn test_worktree(
    worktree_id: &str,
    lifecycle_status: WorktreeLifecycleStatus,
    owner_kind: WorktreeOwnerKind,
    owner_thread_id: Option<&str>,
    owner_agent_run_id: Option<&str>,
) -> Worktree {
    let cleanup_policy = if lifecycle_status == WorktreeLifecycleStatus::Deleted {
        WorktreeCleanupPolicy::ForceDelete
    } else {
        WorktreeCleanupPolicy::DeleteIfClean
    };
    Worktree {
        worktree_id: worktree_id.to_string(),
        agent_id: owner_agent_run_id.map(str::to_string),
        identity: Some(format!("{owner_kind:?}:{worktree_id}")),
        mode: WorktreeMode::IsolatedWorktree,
        lifecycle_status,
        base_repo_path: test_path_display("/tmp/project"),
        worktree_path: test_path_display(&format!(
            "/tmp/project/.codewith/worktrees/{worktree_id}"
        )),
        branch: Some(format!("codewith/{worktree_id}")),
        base_sha: Some("base-sha-1234567890".to_string()),
        head_sha: Some("head-sha-1234567890".to_string()),
        status_snapshot: json!({"status": "ready", "phase": "review"}),
        dirty: lifecycle_status == WorktreeLifecycleStatus::CleanupPending,
        cleanup_policy,
        cleanup_after: Some(1_781_776_000),
        force_delete_requested: cleanup_policy == WorktreeCleanupPolicy::ForceDelete,
        owner_kind,
        owner_thread_id: owner_thread_id.map(str::to_string),
        owner_agent_run_id: owner_agent_run_id.map(str::to_string),
        created_at: 1_781_775_000,
        updated_at: 1_781_776_000,
        released_at: (lifecycle_status != WorktreeLifecycleStatus::Active).then_some(1_781_776_100),
        deleted_at: (lifecycle_status == WorktreeLifecycleStatus::Deleted).then_some(1_781_776_200),
        agent: owner_agent_run_id.map(test_agent),
    }
}

fn test_policy() -> WorktreePolicy {
    WorktreePolicy {
        enabled: true,
        root: Some(test_path_display("/tmp/project/.codewith/worktrees")),
        cleanup_default: WorktreeCleanupPolicy::DeleteIfClean,
        main_sessions: WorktreeSessionMode::Manual,
        sub_sessions: WorktreeSessionMode::Auto,
        current_base_repo_path: Some(test_path_display("/tmp/project")),
    }
}

fn test_no_repo_policy() -> WorktreePolicy {
    WorktreePolicy {
        enabled: false,
        root: None,
        cleanup_default: WorktreeCleanupPolicy::Retain,
        main_sessions: WorktreeSessionMode::Off,
        sub_sessions: WorktreeSessionMode::Off,
        current_base_repo_path: None,
    }
}

fn test_agent(agent_id: &str) -> AgentRun {
    AgentRun {
        agent_id: agent_id.to_string(),
        idempotency_key: None,
        request_id: None,
        source: "test".to_string(),
        prompt_snapshot_ref: "inline:test:prompt".to_string(),
        input_snapshot_ref: None,
        thread_id: None,
        thread_store_kind: "background-agent".to_string(),
        thread_store_id: None,
        rollout_path: None,
        parent_thread_id: None,
        parent_agent_run_id: None,
        spawn_linkage: None,
        worktree_lease_id: Some(agent_id.to_string()),
        auth_profile_ref: None,
        desired_state: AgentDesiredState::Running,
        status: AgentRunStatus::WaitingOnUser,
        status_reason: Some("awaiting review".to_string()),
        config_fingerprint: None,
        version_fingerprint: None,
        retention_state: AgentRetentionState::Active,
        archive_after: None,
        delete_after: None,
        archived_at: None,
        deleted_at: None,
        supervisor_id: None,
        generation: 0,
        pid: None,
        pgid: None,
        job_id: None,
        heartbeat_at: None,
        crash_reason: None,
        exit_code: None,
        exit_signal: None,
        last_event_seq: 0,
        last_snapshot_seq: 0,
        created_at: 1_781_775_000,
        updated_at: 1_781_776_000,
        started_at: None,
        completed_at: None,
    }
}
