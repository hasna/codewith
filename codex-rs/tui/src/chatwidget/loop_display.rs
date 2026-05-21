//! Loop schedule summaries for the `/loop` command.

use super::*;
use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use codex_app_server_protocol::ThreadSchedule;
use codex_app_server_protocol::ThreadScheduleIntervalUnit;
use codex_app_server_protocol::ThreadScheduleRun;
use codex_app_server_protocol::ThreadScheduleRunStatus;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;

impl ChatWidget {
    pub(crate) fn show_loop_summary(&mut self, schedules: Vec<ThreadSchedule>) {
        self.add_plain_history_lines(loop_summary_lines(&schedules));
    }

    pub(crate) fn show_loop_manager(
        &mut self,
        thread_id: ThreadId,
        schedules: Vec<ThreadSchedule>,
    ) {
        self.show_selection_view(loop_manager_params(thread_id, schedules));
    }

    pub(crate) fn show_loop_schedule_actions(
        &mut self,
        thread_id: ThreadId,
        schedule: ThreadSchedule,
    ) {
        self.show_selection_view(loop_schedule_actions_params(thread_id, schedule));
    }

    pub(crate) fn show_loop_edit_prompt(&mut self, thread_id: ThreadId, schedule: ThreadSchedule) {
        let tx = self.app_event_tx.clone();
        let schedule_id = schedule.schedule_id.clone();
        let view = CustomPromptView::new(
            "Edit loop".to_string(),
            "Type the scheduled prompt and press Enter".to_string(),
            schedule.prompt,
            /*context_label*/ None,
            Box::new(move |prompt: String| {
                tx.send(AppEvent::UpdateThreadLoopSchedulePrompt {
                    thread_id,
                    schedule_id: schedule_id.clone(),
                    prompt,
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn on_thread_schedule_updated(&mut self, schedule: ThreadSchedule) {
        if self
            .thread_id
            .is_none_or(|active_thread_id| active_thread_id.to_string() != schedule.thread_id)
        {
            return;
        }
        if matches!(schedule.status, ThreadScheduleStatus::Expired) {
            self.add_info_message(
                "Loop expired".to_string(),
                Some(format!("{} expired.", loop_schedule_summary(&schedule))),
            );
        }
    }

    pub(crate) fn on_thread_schedule_deleted(&mut self, thread_id: &str, schedule_id: &str) {
        if self
            .thread_id
            .is_some_and(|active_thread_id| active_thread_id.to_string() == thread_id)
        {
            tracing::debug!(schedule_id, "thread loop schedule deleted");
        }
    }

    pub(crate) fn on_thread_schedule_run_updated(&mut self, run: ThreadScheduleRun) {
        if self
            .thread_id
            .is_none_or(|active_thread_id| active_thread_id.to_string() != run.thread_id)
        {
            return;
        }
        match run.status {
            ThreadScheduleRunStatus::Failed => self.add_warning_message(format!(
                "Loop run failed for {}: {}",
                run.schedule_id,
                run.error.unwrap_or_else(|| "unknown error".to_string())
            )),
            ThreadScheduleRunStatus::Leased
            | ThreadScheduleRunStatus::Running
            | ThreadScheduleRunStatus::Completed => {}
        }
    }
}

fn loop_manager_params(
    thread_id: ThreadId,
    mut schedules: Vec<ThreadSchedule>,
) -> SelectionViewParams {
    schedules.sort_by_key(|schedule| {
        (
            thread_schedule_status_sort_key(schedule.status),
            schedule.next_run_at.unwrap_or(i64::MAX),
            schedule.schedule_id.clone(),
        )
    });

    let mut items = Vec::with_capacity(schedules.len() + 1);
    if schedules.is_empty() {
        items.push(SelectionItem {
            name: "No loops scheduled".to_string(),
            description: Some("Create one with /loop 5m check whether CI is green".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        for schedule in schedules {
            let schedule_id = schedule.schedule_id.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenThreadLoopScheduleActions {
                    thread_id,
                    schedule_id: schedule_id.clone(),
                });
            })];
            items.push(SelectionItem {
                name: loop_manager_row_name(&schedule),
                description: Some(loop_manager_row_description(&schedule)),
                selected_description: Some(loop_schedule_detail(&schedule)),
                actions,
                dismiss_on_select: true,
                search_value: Some(loop_schedule_search_value(&schedule)),
                ..Default::default()
            });
        }
    }

    SelectionViewParams {
        title: Some("Loops".to_string()),
        subtitle: Some("Select a recurring prompt to manage".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Search loops".to_string()),
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn loop_schedule_actions_params(
    thread_id: ThreadId,
    schedule: ThreadSchedule,
) -> SelectionViewParams {
    let schedule_id = schedule.schedule_id.clone();
    let is_expired = schedule.status == ThreadScheduleStatus::Expired;
    let run_schedule_id = schedule_id.clone();
    let mut items = vec![loop_action_item(
        "Run now",
        "Queue this prompt immediately",
        is_expired,
        disabled_reason_if(is_expired, "Expired loops cannot be run"),
        move || AppEvent::RunThreadLoopScheduleNow {
            thread_id,
            schedule_id: Some(run_schedule_id.clone()),
        },
    )];

    let edit_schedule_id = schedule_id.clone();
    items.push(loop_action_item(
        "Edit prompt",
        "Change the prompt used on future runs",
        is_expired,
        disabled_reason_if(is_expired, "Expired loops cannot be edited"),
        move || AppEvent::OpenThreadLoopEditor {
            thread_id,
            schedule_id: Some(edit_schedule_id.clone()),
        },
    ));

    match schedule.status {
        ThreadScheduleStatus::Active => {
            let pause_schedule_id = schedule_id.clone();
            items.push(loop_action_item(
                "Pause",
                "Stop future automatic runs until resumed",
                false,
                None,
                move || AppEvent::PauseThreadLoopSchedule {
                    thread_id,
                    schedule_id: Some(pause_schedule_id.clone()),
                },
            ));
        }
        ThreadScheduleStatus::Paused => {
            let resume_schedule_id = schedule_id.clone();
            items.push(loop_action_item(
                "Resume",
                "Start scheduling future runs again",
                false,
                None,
                move || AppEvent::ResumeThreadLoopSchedule {
                    thread_id,
                    schedule_id: Some(resume_schedule_id.clone()),
                },
            ));
        }
        ThreadScheduleStatus::Expired => {
            let resume_schedule_id = schedule_id.clone();
            items.push(loop_action_item(
                "Resume",
                "Expired loops cannot be resumed",
                true,
                Some("Expired loops are kept for history only".to_string()),
                move || AppEvent::ResumeThreadLoopSchedule {
                    thread_id,
                    schedule_id: Some(resume_schedule_id.clone()),
                },
            ));
        }
    }

    let delete_schedule_id = schedule_id;
    items.push(loop_action_item(
        "Delete",
        "Remove this loop from the thread",
        false,
        None,
        move || AppEvent::DeleteThreadLoopSchedule {
            thread_id,
            schedule_id: Some(delete_schedule_id.clone()),
        },
    ));
    items.push(loop_action_item(
        "Back to loops",
        "Return to all scheduled prompts",
        false,
        None,
        move || AppEvent::OpenThreadLoopManager { thread_id },
    ));

    SelectionViewParams {
        title: Some(format!("Loop {}", schedule.schedule_id)),
        subtitle: Some(loop_schedule_detail(&schedule)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn loop_action_item(
    name: &'static str,
    description: &'static str,
    is_disabled: bool,
    disabled_reason: Option<String>,
    event: impl Fn() -> AppEvent + Send + Sync + 'static,
) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(event());
    })];
    SelectionItem {
        name: name.to_string(),
        description: Some(description.to_string()),
        is_disabled,
        disabled_reason,
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn disabled_reason_if(is_disabled: bool, reason: &'static str) -> Option<String> {
    is_disabled.then(|| reason.to_string())
}

pub(crate) fn loop_schedule_summary(schedule: &ThreadSchedule) -> String {
    let next = schedule
        .next_run_at
        .map(format_schedule_timestamp)
        .unwrap_or_else(|| "not scheduled".to_string());
    format!(
        "{} ({}, {}, next {next})",
        schedule.schedule_id,
        thread_schedule_status_label(schedule.status),
        thread_schedule_spec_label(&schedule.schedule)
    )
}

fn loop_manager_row_name(schedule: &ThreadSchedule) -> String {
    format!(
        "{}  {}",
        thread_schedule_status_label(schedule.status),
        thread_schedule_spec_label(&schedule.schedule)
    )
}

fn loop_manager_row_description(schedule: &ThreadSchedule) -> String {
    let next = schedule
        .next_run_at
        .map(format_schedule_timestamp)
        .unwrap_or_else(|| "not scheduled".to_string());
    let prompt = truncate_text(&schedule.prompt, 72);
    let mut parts = vec![
        format!("id {}", schedule.schedule_id),
        format!("next {next}"),
        format!("prompt {prompt}"),
    ];
    if schedule.failure_count > 0 {
        parts.push(pluralize_with_amount(schedule.failure_count, "failure"));
    }
    parts.join(" | ")
}

fn loop_schedule_detail(schedule: &ThreadSchedule) -> String {
    let next = schedule
        .next_run_at
        .map(format_schedule_timestamp)
        .unwrap_or_else(|| "not scheduled".to_string());
    let mut parts = vec![thread_schedule_status_label(schedule.status).to_string()];
    parts.push(thread_schedule_spec_label(&schedule.schedule));
    parts.push(format!("next {next}"));
    if let Some(last_run_at) = schedule.last_run_at {
        parts.push(format!("last {}", format_schedule_timestamp(last_run_at)));
    }
    if let Some(expires_at) = schedule.expires_at {
        parts.push(format!("expires {}", format_schedule_timestamp(expires_at)));
    }
    parts.push(format!("tz {}", schedule.timezone));
    parts.push(pluralize_with_amount(schedule.failure_count, "failure"));
    parts.join(" | ")
}

fn loop_schedule_search_value(schedule: &ThreadSchedule) -> String {
    format!(
        "{} {} {} {} {}",
        schedule.schedule_id,
        thread_schedule_status_label(schedule.status),
        thread_schedule_spec_label(&schedule.schedule),
        schedule.timezone,
        schedule.prompt
    )
}

fn thread_schedule_status_sort_key(status: ThreadScheduleStatus) -> u8 {
    match status {
        ThreadScheduleStatus::Active => 0,
        ThreadScheduleStatus::Paused => 1,
        ThreadScheduleStatus::Expired => 2,
    }
}

fn loop_summary_lines(schedules: &[ThreadSchedule]) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("Loops".bold())];
    if schedules.is_empty() {
        lines.push(Line::from("No loops scheduled.".dim()));
        lines.push(Line::default());
        lines.push(Line::from("Try /loop 5m check whether CI is green".dim()));
        return lines;
    }

    for schedule in schedules {
        lines.push(Line::from(vec![
            "• ".dim(),
            schedule.schedule_id.clone().into(),
            " ".dim(),
            thread_schedule_status_label(schedule.status).into(),
            " ".dim(),
            thread_schedule_spec_label(&schedule.schedule).into(),
        ]));
        lines.push(Line::from(vec![
            "  Prompt: ".dim(),
            schedule.prompt.clone().into(),
        ]));
        let next = schedule
            .next_run_at
            .map(format_schedule_timestamp)
            .unwrap_or_else(|| "not scheduled".to_string());
        lines.push(Line::from(vec![
            "  Next: ".dim(),
            next.into(),
            "  Timezone: ".dim(),
            schedule.timezone.clone().into(),
        ]));
    }

    lines.push(Line::default());
    lines.push(Line::from(
        "Commands: /loop edit <id>, /loop pause <id>, /loop resume <id>, /loop run-now <id>, /loop delete <id>"
            .dim(),
    ));
    lines
}

fn thread_schedule_status_label(status: ThreadScheduleStatus) -> &'static str {
    match status {
        ThreadScheduleStatus::Active => "active",
        ThreadScheduleStatus::Paused => "paused",
        ThreadScheduleStatus::Expired => "expired",
    }
}

fn thread_schedule_spec_label(schedule: &ThreadScheduleSpec) -> String {
    match schedule {
        ThreadScheduleSpec::Dynamic => "dynamic".to_string(),
        ThreadScheduleSpec::Interval { amount, unit } => {
            let unit = match unit {
                ThreadScheduleIntervalUnit::Minutes => pluralize(*amount, "minute"),
                ThreadScheduleIntervalUnit::Hours => pluralize(*amount, "hour"),
                ThreadScheduleIntervalUnit::Days => pluralize(*amount, "day"),
            };
            format!("every {amount} {unit}")
        }
        ThreadScheduleSpec::Cron { expression } => format!("cron {expression}"),
    }
}

fn pluralize(amount: i64, unit: &'static str) -> &'static str {
    if amount == 1 {
        unit
    } else {
        match unit {
            "minute" => "minutes",
            "hour" => "hours",
            "day" => "days",
            _ => unit,
        }
    }
}

fn pluralize_with_amount(amount: i64, unit: &'static str) -> String {
    let unit = if amount == 1 {
        unit
    } else {
        match unit {
            "failure" => "failures",
            _ => unit,
        }
    };
    format!("{amount} {unit}")
}

fn format_schedule_timestamp(seconds: i64) -> String {
    let Some(utc) = DateTime::<Utc>::from_timestamp(seconds, 0) else {
        return seconds.to_string();
    };
    utc.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ThreadSchedulePromptSource;
    use pretty_assertions::assert_eq;

    fn test_schedule(
        schedule_id: &str,
        status: ThreadScheduleStatus,
        next_run_at: Option<i64>,
    ) -> ThreadSchedule {
        ThreadSchedule {
            thread_id: "thread-1".to_string(),
            schedule_id: schedule_id.to_string(),
            prompt: "check CI".to_string(),
            prompt_source: ThreadSchedulePromptSource::Inline,
            schedule: ThreadScheduleSpec::Interval {
                amount: 5,
                unit: ThreadScheduleIntervalUnit::Minutes,
            },
            timezone: "Europe/Bucharest".to_string(),
            status,
            next_run_at,
            last_run_at: None,
            expires_at: None,
            failure_count: 0,
            lease_expires_at: None,
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn interval_schedule_summary_uses_human_label() {
        let schedule = test_schedule("sch_123", ThreadScheduleStatus::Active, Some(1_700_000_000));

        assert!(
            loop_schedule_summary(&schedule).contains("every 5 minutes"),
            "summary: {}",
            loop_schedule_summary(&schedule)
        );
    }

    #[test]
    fn pluralize_single_units() {
        assert_eq!(
            thread_schedule_spec_label(&ThreadScheduleSpec::Interval {
                amount: 1,
                unit: ThreadScheduleIntervalUnit::Hours,
            }),
            "every 1 hour"
        );
    }

    #[test]
    fn manager_sorts_active_before_paused_and_expired() {
        let params = loop_manager_params(
            ThreadId::new(),
            vec![
                test_schedule("expired", ThreadScheduleStatus::Expired, None),
                test_schedule("paused", ThreadScheduleStatus::Paused, Some(5)),
                test_schedule("active", ThreadScheduleStatus::Active, Some(10)),
            ],
        );

        let item_names = params
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            item_names,
            vec![
                "active  every 5 minutes".to_string(),
                "paused  every 5 minutes".to_string(),
                "expired  every 5 minutes".to_string(),
            ]
        );
    }

    #[test]
    fn schedule_actions_match_status() {
        let params = loop_schedule_actions_params(
            ThreadId::new(),
            test_schedule("sch_123", ThreadScheduleStatus::Paused, Some(10)),
        );

        let item_names = params
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            item_names,
            vec![
                "Run now".to_string(),
                "Edit prompt".to_string(),
                "Resume".to_string(),
                "Delete".to_string(),
                "Back to loops".to_string(),
            ]
        );
    }

    #[test]
    fn expired_schedule_disables_mutating_actions() {
        let params = loop_schedule_actions_params(
            ThreadId::new(),
            test_schedule("sch_123", ThreadScheduleStatus::Expired, None),
        );

        let disabled = params
            .items
            .iter()
            .map(|item| (item.name.as_str(), item.is_disabled))
            .collect::<Vec<_>>();
        assert_eq!(
            disabled,
            vec![
                ("Run now", true),
                ("Edit prompt", true),
                ("Resume", true),
                ("Delete", false),
                ("Back to loops", false),
            ]
        );
    }
}
