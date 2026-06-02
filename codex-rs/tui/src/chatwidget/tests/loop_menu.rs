use super::*;
use codex_app_server_protocol::ThreadSchedule;
use codex_app_server_protocol::ThreadScheduleIntervalUnit;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;

fn test_schedule(schedule_id: &str, status: ThreadScheduleStatus) -> ThreadSchedule {
    ThreadSchedule {
        thread_id: "thread-1".to_string(),
        schedule_id: schedule_id.to_string(),
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
async fn schedule_created_notification_announces_loop_once() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let mut schedule = test_schedule("sch_active", ThreadScheduleStatus::Active);
    schedule.thread_id = thread_id.to_string();
    schedule.next_run_at = Some(1_700_000_060);
    schedule.updated_at = schedule.created_at;

    chat.on_thread_schedule_updated(schedule.clone());

    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Loop scheduled"),
        "expected scheduled notification acknowledgement, got: {rendered}"
    );
    assert!(
        rendered.contains("/loop pause sch_active"),
        "expected loop action hint, got: {rendered}"
    );

    chat.on_thread_schedule_updated(schedule);

    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "follow-up updates for the same schedule should not duplicate the acknowledgement"
    );
}
