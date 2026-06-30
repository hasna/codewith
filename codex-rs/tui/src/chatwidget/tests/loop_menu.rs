use super::*;
use codex_app_server_protocol::ThreadSchedule;
use codex_app_server_protocol::ThreadScheduleIntervalUnit;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleRun;
use codex_app_server_protocol::ThreadScheduleRunStatus;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;

fn test_schedule(schedule_id: &str, status: ThreadScheduleStatus) -> ThreadSchedule {
    ThreadSchedule {
        thread_id: "thread-1".to_string(),
        schedule_id: schedule_id.to_string(),
        parent_schedule_id: None,
        nesting_depth: 1,
        prompt: "check whether CI is green and write the next action".to_string(),
        prompt_source: ThreadSchedulePromptSource::Inline,
        schedule: ThreadScheduleSpec::Interval {
            amount: 5,
            unit: ThreadScheduleIntervalUnit::Minutes,
        },
        timezone: "UTC".to_string(),
        status,
        next_run_at: None,
        last_run_at: None,
        expires_at: None,
        failure_count: if status == ThreadScheduleStatus::Paused {
            2
        } else {
            0
        },
        lease_expires_at: None,
        created_at: 1,
        updated_at: 2,
    }
}

fn test_once_schedule(schedule_id: &str, status: ThreadScheduleStatus) -> ThreadSchedule {
    ThreadSchedule {
        schedule: ThreadScheduleSpec::Once,
        next_run_at: Some(1_700_000_300),
        ..test_schedule(schedule_id, status)
    }
}

fn test_nested_schedule(
    schedule_id: &str,
    parent_schedule_id: &str,
    nesting_depth: i64,
) -> ThreadSchedule {
    ThreadSchedule {
        parent_schedule_id: Some(parent_schedule_id.to_string()),
        nesting_depth,
        schedule: ThreadScheduleSpec::Interval {
            amount: 30,
            unit: ThreadScheduleIntervalUnit::Minutes,
        },
        ..test_schedule(schedule_id, ThreadScheduleStatus::Active)
    }
}

fn test_schedule_run(
    thread_id: ThreadId,
    schedule_id: &str,
    run_id: &str,
    status: ThreadScheduleRunStatus,
) -> ThreadScheduleRun {
    let completed_at = if matches!(
        status,
        ThreadScheduleRunStatus::Completed | ThreadScheduleRunStatus::Failed
    ) {
        Some(1_700_000_002)
    } else {
        None
    };
    ThreadScheduleRun {
        thread_id: thread_id.to_string(),
        schedule_id: schedule_id.to_string(),
        run_id: run_id.to_string(),
        status,
        lease_id: "lease-1".to_string(),
        turn_id: Some("turn-1".to_string()),
        error: None,
        scheduled_for_at: Some(1_700_000_000),
        started_at: 1_700_000_001,
        completed_at,
    }
}

#[tokio::test]
async fn loop_manager_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_loop_manager(
        thread_id,
        vec![
            test_schedule("sch_expired", ThreadScheduleStatus::Expired),
            test_schedule("sch_paused", ThreadScheduleStatus::Paused),
            test_schedule("sch_active", ThreadScheduleStatus::Active),
        ],
    );

    assert_chatwidget_snapshot!(
        "loop_manager_popup",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn loop_manager_nested_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_loop_manager(
        thread_id,
        vec![
            test_schedule("sch_parent", ThreadScheduleStatus::Active),
            test_nested_schedule("child_one", "sch_parent", 2),
            test_nested_schedule("child_two", "sch_parent", 2),
        ],
    );

    assert_chatwidget_snapshot!(
        "loop_manager_nested_popup",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn loop_actions_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_loop_schedule_actions(
        thread_id,
        test_schedule("sch_paused", ThreadScheduleStatus::Paused),
    );

    assert_chatwidget_snapshot!(
        "loop_actions_popup",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn loop_nested_actions_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_loop_schedule_actions(
        thread_id,
        test_nested_schedule("child_one", "sch_parent", 2),
    );

    assert_chatwidget_snapshot!(
        "loop_nested_actions_popup",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn schedule_manager_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_schedule_manager(
        thread_id,
        vec![
            test_once_schedule("sch_expired", ThreadScheduleStatus::Expired),
            test_once_schedule("sch_paused", ThreadScheduleStatus::Paused),
            test_once_schedule("sch_active", ThreadScheduleStatus::Active),
        ],
    );

    assert_chatwidget_snapshot!(
        "schedule_manager_popup",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn schedule_actions_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();

    chat.show_schedule_actions(
        thread_id,
        test_once_schedule("sch_paused", ThreadScheduleStatus::Paused),
    );

    assert_chatwidget_snapshot!(
        "schedule_actions_popup",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn schedule_created_notification_announces_loop_once() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let mut schedule = test_schedule("sch_active", ThreadScheduleStatus::Active);
    schedule.thread_id = thread_id.to_string();
    schedule.updated_at = schedule.created_at;

    chat.on_thread_schedule_updated(schedule.clone());

    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("");
    insta::assert_snapshot!(
        rendered,
        @r###"
Loops
• sch_active active every 5 minutes
  Prompt: check whether CI is green and write the next action
  Next: not scheduled  Timezone: UTC

Commands: /loop edit <id>, /loop pause <id>, /loop resume <id>, /loop run-now <id>, /loop delete <id>

• Loop scheduled Use /loop pause sch_active, /loop run-now sch_active, or /loop delete sch_active.
"###
    );

    chat.on_thread_schedule_updated(schedule);

    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "follow-up updates for the same schedule should not duplicate the acknowledgement"
    );
}

#[tokio::test]
async fn loop_run_updates_surface_active_thread_progress() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    chat.on_thread_schedule_run_updated(test_schedule_run(
        thread_id,
        "02f1072a-c22e-447e-9448-c41bc7717ab1",
        "0eb8d7d4-a324-47a8-9e1c-6912c6d76e87",
        ThreadScheduleRunStatus::Running,
    ));
    chat.on_thread_schedule_run_updated(test_schedule_run(
        thread_id,
        "02f1072a-c22e-447e-9448-c41bc7717ab1",
        "0eb8d7d4-a324-47a8-9e1c-6912c6d76e87",
        ThreadScheduleRunStatus::Completed,
    ));
    let mut failed = test_schedule_run(
        thread_id,
        "02f1072a-c22e-447e-9448-c41bc7717ab1",
        "1f6b57a9-c105-422f-b0cb-dff17d53ee00",
        ThreadScheduleRunStatus::Failed,
    );
    failed.error = Some("scheduled turn completed without a final assistant message".to_string());
    chat.on_thread_schedule_run_updated(failed);

    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("");

    assert!(rendered.contains("Loop run started"), "{rendered}");
    assert!(
        rendered.contains("0eb8d7d4 started for 02f1072a"),
        "{rendered}"
    );
    assert!(rendered.contains("Loop run completed"), "{rendered}");
    assert!(
        rendered.contains("0eb8d7d4 completed for 02f1072a"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Loop run failed for 02f1072a-c22e-447e-9448-c41bc7717ab1"),
        "{rendered}"
    );
    assert!(
        rendered.contains("completed without a final assistant message"),
        "{rendered}"
    );
}
