//! Monitor summaries and interactive manager for `/monitor`.

use super::*;
use chrono::DateTime;
use chrono::Local;
use codex_app_server_protocol::ThreadMonitor;
use codex_app_server_protocol::ThreadMonitorEvent;
use codex_app_server_protocol::ThreadMonitorEventStream;
use codex_app_server_protocol::ThreadMonitorRouting;
use codex_app_server_protocol::ThreadMonitorStatus;
use ratatui::text::Span;

impl ChatWidget {
    pub(crate) fn show_monitor_manager(
        &mut self,
        thread_id: ThreadId,
        monitors: Vec<ThreadMonitor>,
    ) {
        self.show_selection_view(thread_monitor_manager_params(thread_id, monitors));
    }

    pub(crate) fn show_monitor_actions(&mut self, thread_id: ThreadId, monitor: ThreadMonitor) {
        self.show_selection_view(thread_monitor_actions_params(thread_id, monitor));
    }

    pub(crate) fn show_monitor_summary(&mut self, monitors: Vec<ThreadMonitor>) {
        self.add_plain_history_lines(thread_monitor_summary_lines(&monitors));
    }

    pub(crate) fn show_monitor_events(
        &mut self,
        monitor: ThreadMonitor,
        events: Vec<ThreadMonitorEvent>,
    ) {
        let mut lines = vec![Line::from(vec![
            "Monitor ".dim(),
            monitor.name.bold(),
            " ".into(),
            short_monitor_id(&monitor.monitor_id).dim(),
        ])];
        if events.is_empty() {
            lines.push("No monitor output recorded yet.".dim().into());
        } else {
            for event in events {
                lines.push(monitor_event_line(&event));
            }
        }
        self.add_plain_history_lines(lines);
    }

    pub(crate) fn on_thread_monitor_updated(&mut self, monitor: ThreadMonitor) {
        if self
            .thread_id
            .is_none_or(|active_thread_id| active_thread_id.to_string() != monitor.thread_id)
        {
            return;
        }
        if should_announce_monitor(&monitor)
            && self
                .announced_monitor_ids
                .insert(monitor.monitor_id.clone())
        {
            self.add_plain_history_lines(thread_monitor_summary_lines(std::slice::from_ref(
                &monitor,
            )));
            self.add_info_message(
                "Monitor started".to_string(),
                Some(format!(
                    "Use /monitor to manage it, or /monitor read {} to inspect output.",
                    short_monitor_id(&monitor.monitor_id)
                )),
            );
        } else if monitor.status == ThreadMonitorStatus::Failed {
            self.add_warning_message(format!(
                "Monitor {} failed: {}",
                short_monitor_id(&monitor.monitor_id),
                monitor
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string())
            ));
        }
    }

    pub(crate) fn on_thread_monitor_deleted(&mut self, thread_id: &str, monitor_id: &str) {
        self.announced_monitor_ids.remove(monitor_id);
        if self
            .thread_id
            .is_some_and(|active_thread_id| active_thread_id.to_string() == thread_id)
        {
            tracing::debug!(monitor_id, "thread monitor deleted");
        }
    }

    pub(crate) fn on_thread_monitor_event(
        &mut self,
        monitor: ThreadMonitor,
        event: ThreadMonitorEvent,
    ) {
        if self
            .thread_id
            .is_none_or(|active_thread_id| active_thread_id.to_string() != monitor.thread_id)
        {
            return;
        }
        if event.stream == ThreadMonitorEventStream::System
            && matches!(
                monitor.status,
                ThreadMonitorStatus::Stopped | ThreadMonitorStatus::Failed
            )
        {
            self.add_plain_history_lines(vec![monitor_event_line(&event)]);
        }
    }
}

fn thread_monitor_manager_params(
    thread_id: ThreadId,
    mut monitors: Vec<ThreadMonitor>,
) -> SelectionViewParams {
    monitors.sort_by_key(|monitor| {
        (
            thread_monitor_status_sort_key(monitor.status),
            std::cmp::Reverse(monitor.updated_at),
            monitor.monitor_id.clone(),
        )
    });

    let mut items = Vec::with_capacity(monitors.len() + 1);
    if monitors.is_empty() {
        items.push(SelectionItem {
            name: "No monitors created".to_string(),
            description: Some("Create one with /monitor <request>".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        for monitor in monitors {
            let monitor_id = monitor.monitor_id.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenThreadMonitorActions {
                    thread_id,
                    monitor_id: monitor_id.clone(),
                });
            })];
            items.push(SelectionItem {
                name: monitor_manager_row_name(&monitor),
                description: Some(monitor_manager_row_description(&monitor)),
                selected_description: Some(monitor_detail(&monitor)),
                actions,
                dismiss_on_select: true,
                search_value: Some(monitor_search_value(&monitor)),
                ..Default::default()
            });
        }
    }

    SelectionViewParams {
        title: Some("Monitors".to_string()),
        subtitle: Some("Select a monitor to manage".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Search monitors".to_string()),
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn thread_monitor_actions_params(
    thread_id: ThreadId,
    monitor: ThreadMonitor,
) -> SelectionViewParams {
    let monitor_id = monitor.monitor_id.clone();
    let read_monitor_id = monitor_id.clone();
    let mut items = vec![monitor_action_item(
        "Read output",
        "Show recent monitor events",
        false,
        None,
        move || AppEvent::ReadThreadMonitor {
            thread_id,
            monitor_id: Some(read_monitor_id.clone()),
        },
    )];

    match monitor.status {
        ThreadMonitorStatus::Running => {
            let stop_monitor_id = monitor_id.clone();
            items.push(monitor_action_item(
                "Stop",
                "Stop the monitor process",
                false,
                None,
                move || AppEvent::StopThreadMonitor {
                    thread_id,
                    monitor_id: Some(stop_monitor_id.clone()),
                },
            ));
        }
        ThreadMonitorStatus::Stopped | ThreadMonitorStatus::Failed => {
            let restart_monitor_id = monitor_id.clone();
            items.push(monitor_action_item(
                "Restart",
                "Run the monitor command again",
                false,
                None,
                move || AppEvent::RestartThreadMonitor {
                    thread_id,
                    monitor_id: Some(restart_monitor_id.clone()),
                },
            ));
        }
    }

    let delete_monitor_id = monitor_id;
    items.push(monitor_action_item(
        "Delete",
        "Remove this monitor from the thread",
        false,
        None,
        move || AppEvent::DeleteThreadMonitor {
            thread_id,
            monitor_id: Some(delete_monitor_id.clone()),
        },
    ));
    items.push(monitor_action_item(
        "Back to monitors",
        "Return to all monitors",
        false,
        None,
        move || AppEvent::OpenThreadMonitorManager { thread_id },
    ));

    SelectionViewParams {
        title: Some(format!("Monitor {}", short_monitor_id(&monitor.monitor_id))),
        subtitle: Some(monitor_detail(&monitor)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn monitor_action_item(
    name: impl Into<String>,
    description: impl Into<String>,
    is_disabled: bool,
    disabled_reason: Option<String>,
    event: impl Fn() -> AppEvent + Send + Sync + 'static,
) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(event());
    })];
    SelectionItem {
        name: name.into(),
        description: Some(description.into()),
        is_disabled,
        disabled_reason,
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn thread_monitor_summary_lines(monitors: &[ThreadMonitor]) -> Vec<Line<'static>> {
    if monitors.is_empty() {
        return vec!["No monitors created.".dim().into()];
    }
    let mut lines = vec![
        format!(
            "{} monitor{}",
            monitors.len(),
            if monitors.len() == 1 { "" } else { "s" }
        )
        .bold()
        .into(),
    ];
    for monitor in monitors {
        lines.push(Line::from(vec![
            "  ".into(),
            monitor_status_label(monitor.status),
            " ".into(),
            monitor.name.clone().into(),
            " ".into(),
            short_monitor_id(&monitor.monitor_id).dim(),
            "  ".dim(),
            monitor_routing_label(monitor.routing).dim(),
        ]));
        lines.push(format!("    {}", monitor.prompt).dim().into());
    }
    lines
}

fn monitor_manager_row_name(monitor: &ThreadMonitor) -> String {
    format!(
        "{}  {}",
        short_monitor_id(&monitor.monitor_id),
        monitor.name
    )
}

fn monitor_manager_row_description(monitor: &ThreadMonitor) -> String {
    let status = match monitor.status {
        ThreadMonitorStatus::Running => "running",
        ThreadMonitorStatus::Stopped => "stopped",
        ThreadMonitorStatus::Failed => "failed",
    };
    let last = monitor
        .last_event_at
        .map(format_timestamp)
        .map(|value| format!("last {value}"))
        .unwrap_or_else(|| "no output yet".to_string());
    format!(
        "{status} · {} · {last}",
        monitor_routing_label(monitor.routing)
    )
}

fn monitor_detail(monitor: &ThreadMonitor) -> String {
    let mut detail = format!(
        "{} · {} · {}",
        monitor.prompt,
        monitor_routing_label(monitor.routing),
        monitor.command
    );
    if let Some(error) = monitor.last_error.as_ref() {
        detail.push_str(" · ");
        detail.push_str(error);
    }
    detail
}

fn monitor_search_value(monitor: &ThreadMonitor) -> String {
    format!(
        "{} {} {} {}",
        monitor.monitor_id, monitor.name, monitor.prompt, monitor.command
    )
}

fn monitor_event_line(event: &ThreadMonitorEvent) -> Line<'static> {
    Line::from(vec![
        format_timestamp(event.created_at).dim(),
        " ".into(),
        monitor_event_stream_label(event.stream),
        " ".into(),
        event.text.clone().into(),
    ])
}

fn monitor_status_label(status: ThreadMonitorStatus) -> Span<'static> {
    match status {
        ThreadMonitorStatus::Running => "running".green(),
        ThreadMonitorStatus::Stopped => "stopped".dim(),
        ThreadMonitorStatus::Failed => "failed".red(),
    }
}

fn monitor_event_stream_label(stream: ThreadMonitorEventStream) -> Span<'static> {
    match stream {
        ThreadMonitorEventStream::Stdout => "stdout".cyan(),
        ThreadMonitorEventStream::Stderr => "stderr".red(),
        ThreadMonitorEventStream::System => "system".dim(),
    }
}

fn monitor_routing_label(routing: ThreadMonitorRouting) -> &'static str {
    match routing {
        ThreadMonitorRouting::Stream => "stream",
        ThreadMonitorRouting::File => "file",
        ThreadMonitorRouting::Both => "stream+file",
    }
}

fn thread_monitor_status_sort_key(status: ThreadMonitorStatus) -> u8 {
    match status {
        ThreadMonitorStatus::Running => 0,
        ThreadMonitorStatus::Failed => 1,
        ThreadMonitorStatus::Stopped => 2,
    }
}

fn should_announce_monitor(monitor: &ThreadMonitor) -> bool {
    monitor.status == ThreadMonitorStatus::Running
        && monitor.created_at == monitor.updated_at
        && monitor.process_id.is_none()
        && monitor.last_event_at.is_none()
}

fn short_monitor_id(monitor_id: &str) -> String {
    monitor_id.chars().take(8).collect()
}

fn format_timestamp(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|datetime| {
            datetime
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn test_monitor(
        monitor_id: &str,
        status: ThreadMonitorStatus,
        updated_at: i64,
    ) -> ThreadMonitor {
        ThreadMonitor {
            thread_id: "thread-1".to_string(),
            monitor_id: monitor_id.to_string(),
            name: "CI watcher".to_string(),
            prompt: "watch CI".to_string(),
            command: "while true; do echo ok; sleep 60; done".to_string(),
            cwd: None,
            routing: ThreadMonitorRouting::Stream,
            output_file: None,
            status,
            generation: 0,
            process_id: None,
            last_event_at: None,
            last_error: None,
            created_at: 1,
            updated_at,
        }
    }

    #[test]
    fn manager_sorts_running_before_failed_and_stopped() {
        let params = thread_monitor_manager_params(
            ThreadId::new(),
            vec![
                test_monitor("stopped", ThreadMonitorStatus::Stopped, 3),
                test_monitor("failed", ThreadMonitorStatus::Failed, 2),
                test_monitor("running", ThreadMonitorStatus::Running, 1),
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
                "running  CI watcher".to_string(),
                "failed  CI watcher".to_string(),
                "stopped  CI watcher".to_string(),
            ]
        );
    }
}
