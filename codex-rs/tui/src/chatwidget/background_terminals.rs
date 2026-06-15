//! Background terminal manager for unified exec processes.

use super::*;

const BACKGROUND_TERMINALS_VIEW_ID: &str = "background-terminals";
const BACKGROUND_TERMINALS_STOP_CONFIRM_VIEW_ID: &str = "background-terminals-stop-confirm";

impl ChatWidget {
    pub(crate) fn open_background_terminal_manager(&mut self) {
        self.bottom_pane
            .show_selection_view(self.background_terminal_manager_popup_params());
        self.request_redraw();
    }

    pub(crate) fn open_background_terminal_stop_confirmation(&mut self) {
        self.bottom_pane
            .show_selection_view(self.background_terminal_stop_confirmation_popup_params());
        self.request_redraw();
    }

    fn background_terminal_manager_popup_params(&self) -> SelectionViewParams {
        let processes = self.background_terminal_processes();
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Background Terminals".bold()));
        header.push(Line::from(
            "Inspect running background terminals or stop all of them.".dim(),
        ));

        let mut items = Vec::new();
        if processes.is_empty() {
            items.push(print_background_terminals_item());
            items.push(no_background_terminals_item());
        } else {
            items.push(print_background_terminals_item());
            items.push(stop_all_background_terminals_item(processes.len()));
            items.extend(processes.iter().enumerate().map(background_terminal_item));
        }

        SelectionViewParams {
            view_id: Some(BACKGROUND_TERMINALS_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search background terminals".to_string()),
            col_width_mode: ColumnWidthMode::Fixed,
            ..Default::default()
        }
    }

    fn background_terminal_stop_confirmation_popup_params(&self) -> SelectionViewParams {
        let count = self.background_terminal_processes().len();
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Stop Background Terminals".bold()));
        header.push(Line::from(format!(
            "This will stop {count} running background terminal process(es)."
        )));

        let items = if count == 0 {
            vec![no_background_terminals_item()]
        } else {
            vec![
                back_to_background_terminals_item(),
                confirm_stop_all_background_terminals_item(count),
            ]
        };

        SelectionViewParams {
            view_id: Some(BACKGROUND_TERMINALS_STOP_CONFIRM_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: false,
            col_width_mode: ColumnWidthMode::Fixed,
            ..Default::default()
        }
    }
}

fn no_background_terminals_item() -> SelectionItem {
    SelectionItem {
        name: "No background terminals".to_string(),
        description: Some("There are no running background terminal processes.".to_string()),
        is_disabled: true,
        ..Default::default()
    }
}

fn stop_all_background_terminals_item(count: usize) -> SelectionItem {
    SelectionItem {
        name: "Stop all...".to_string(),
        description: Some(format!(
            "Review before stopping {count} running background terminal process(es)."
        )),
        search_value: Some("stop all background terminals running processes".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::OpenBackgroundTerminalStopConfirmation);
        })],
        ..Default::default()
    }
}

fn print_background_terminals_item() -> SelectionItem {
    SelectionItem {
        name: "Print snapshot".to_string(),
        description: Some(
            "Add the current background terminal list to the transcript.".to_string(),
        ),
        search_value: Some("print snapshot transcript history ps".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::PrintBackgroundTerminals);
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn back_to_background_terminals_item() -> SelectionItem {
    SelectionItem {
        name: "Back".to_string(),
        description: Some("Return to the background terminal list.".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::OpenBackgroundTerminalManager);
        })],
        ..Default::default()
    }
}

fn confirm_stop_all_background_terminals_item(count: usize) -> SelectionItem {
    SelectionItem {
        name: "Stop all".to_string(),
        description: Some(format!(
            "Stop {count} running background terminal process(es)."
        )),
        search_value: Some("confirm stop all background terminals".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::StopBackgroundTerminals);
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn background_terminal_item(
    (index, process): (usize, &history_cell::UnifiedExecProcessDetails),
) -> SelectionItem {
    let description = if process.recent_chunks.is_empty() {
        "No recent output captured.".to_string()
    } else {
        process.recent_chunks.join(" | ")
    };
    SelectionItem {
        name: format!("{}. {}", index + 1, process.command_display),
        description: Some(description),
        search_value: Some(format!(
            "{} {}",
            process.command_display,
            process.recent_chunks.join(" ")
        )),
        disabled_reason: Some(
            "Individual background terminal stop is not available yet.".to_string(),
        ),
        ..Default::default()
    }
}
