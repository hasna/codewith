use super::*;
use chrono::Local;
use chrono::LocalResult;
use chrono::NaiveDate;
use chrono::TimeZone;
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

const TEST_BASE_REPO_PATH: &str = "/tmp/project";

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
        normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 120))
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
        normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 120))
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
        }) if worktree_id == "018f-active-worktree" && base_repo_path == TEST_BASE_REPO_PATH
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
        normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 120))
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
        }) if worktree_id == "018f-actions-worktree" && base_repo_path == TEST_BASE_REPO_PATH
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
    assert_chatwidget_snapshot!("worktree_read_detail", normalize_snapshot_paths(combined));
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
        base_repo_path: TEST_BASE_REPO_PATH.to_string(),
        worktree_path: format!("{TEST_BASE_REPO_PATH}/.codewith/worktrees/{worktree_id}"),
        branch: Some(format!("codewith/{worktree_id}")),
        base_sha: Some("base-sha-1234567890".to_string()),
        head_sha: Some("head-sha-1234567890".to_string()),
        status_snapshot: json!({"status": "ready", "phase": "review"}),
        dirty: lifecycle_status == WorktreeLifecycleStatus::CleanupPending,
        cleanup_policy,
        cleanup_after: Some(local_timestamp_for_snapshot(2026, 6, 18, 12, 46, 40)),
        force_delete_requested: cleanup_policy == WorktreeCleanupPolicy::ForceDelete,
        owner_kind,
        owner_thread_id: owner_thread_id.map(str::to_string),
        owner_agent_run_id: owner_agent_run_id.map(str::to_string),
        created_at: local_timestamp_for_snapshot(2026, 6, 18, 12, 30, 0),
        updated_at: local_timestamp_for_snapshot(2026, 6, 18, 12, 46, 40),
        released_at: (lifecycle_status != WorktreeLifecycleStatus::Active)
            .then_some(local_timestamp_for_snapshot(2026, 6, 18, 12, 48, 20)),
        deleted_at: (lifecycle_status == WorktreeLifecycleStatus::Deleted)
            .then_some(local_timestamp_for_snapshot(2026, 6, 18, 12, 50, 0)),
        agent: owner_agent_run_id.map(test_agent),
    }
}

fn local_timestamp_for_snapshot(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> i64 {
    let naive = NaiveDate::from_ymd_opt(year, month, day)
        .expect("valid date")
        .and_hms_opt(hour, minute, second)
        .expect("valid time");
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        LocalResult::None => naive.and_utc().timestamp(),
    }
}

fn test_policy() -> WorktreePolicy {
    WorktreePolicy {
        enabled: true,
        root: Some(format!("{TEST_BASE_REPO_PATH}/.codewith/worktrees")),
        cleanup_default: WorktreeCleanupPolicy::DeleteIfClean,
        main_sessions: WorktreeSessionMode::Manual,
        sub_sessions: WorktreeSessionMode::Auto,
        current_base_repo_path: Some(TEST_BASE_REPO_PATH.to_string()),
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
