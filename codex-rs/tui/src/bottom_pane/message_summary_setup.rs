//! Message summary configuration view for customizing the final assistant-message separator.

use std::collections::HashSet;

use codex_otel::RuntimeMetricTotals;
use codex_otel::RuntimeMetricsSummary;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use strum::IntoEnumIterator;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::bottom_pane::multi_select_picker::MultiSelectItem;
use crate::bottom_pane::multi_select_picker::MultiSelectPicker;
use crate::history_cell::MessageSummaryItem;
use crate::history_cell::message_summary_labels;
use crate::keymap::ListKeymap;
use crate::render::renderable::Renderable;

fn parse_message_summary_items<T>(ids: impl Iterator<Item = T>) -> Option<Vec<MessageSummaryItem>>
where
    T: AsRef<str>,
{
    ids.map(|id| id.as_ref().parse::<MessageSummaryItem>())
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

fn preview_line_for_summary_items(items: &[MessageSummaryItem]) -> Option<Line<'static>> {
    let sample_metrics = RuntimeMetricsSummary {
        tool_calls: RuntimeMetricTotals {
            count: 3,
            duration_ms: 2_450,
        },
        api_calls: RuntimeMetricTotals {
            count: 2,
            duration_ms: 1_200,
        },
        streaming_events: RuntimeMetricTotals {
            count: 6,
            duration_ms: 900,
        },
        websocket_calls: RuntimeMetricTotals {
            count: 1,
            duration_ms: 700,
        },
        websocket_events: RuntimeMetricTotals {
            count: 4,
            duration_ms: 1_200,
        },
        responses_api_overhead_ms: 650,
        responses_api_inference_time_ms: 1_940,
        responses_api_engine_iapi_ttft_ms: 410,
        responses_api_engine_service_ttft_ms: 460,
        responses_api_engine_iapi_tbt_ms: 1_180,
        responses_api_engine_service_tbt_ms: 1_240,
        turn_ttft_ms: 0,
        turn_ttfm_ms: 0,
    };
    let labels = message_summary_labels(Some(125), Some(sample_metrics), items);
    (!labels.is_empty()).then(|| Line::from(labels.join(" • ")))
}

/// Interactive view for configuring final message summary items.
pub(crate) struct MessageSummarySetupView {
    picker: MultiSelectPicker,
}

impl MessageSummarySetupView {
    /// Creates the message-summary picker, preserving the configured item order first.
    pub(crate) fn new(
        summary_items: Option<&[String]>,
        app_event_tx: AppEventSender,
        list_keymap: ListKeymap,
    ) -> Self {
        let mut used_ids = HashSet::new();
        let mut items = Vec::new();
        if let Some(selected_items) = summary_items {
            for id in selected_items {
                let Ok(item) = id.parse::<MessageSummaryItem>() else {
                    continue;
                };
                let item_id = item.to_string();
                if !used_ids.insert(item_id.clone()) {
                    continue;
                }
                items.push(Self::summary_select_item(item, /*enabled*/ true));
            }
        }

        for item in MessageSummaryItem::iter() {
            let item_id = item.to_string();
            if used_ids.contains(&item_id) {
                continue;
            }
            items.push(Self::summary_select_item(item, /*enabled*/ false));
        }

        Self {
            picker: MultiSelectPicker::builder(
                "Configure Message Summary".to_string(),
                Some("Select which items to display after final assistant messages.".to_string()),
                app_event_tx,
            )
            .list_keymap(list_keymap)
            .items(items)
            .enable_ordering()
            .on_preview(|items| {
                let items = parse_message_summary_items(
                    items
                        .iter()
                        .filter(|item| item.enabled)
                        .map(|item| item.id.as_str()),
                )?;
                preview_line_for_summary_items(&items)
            })
            .on_confirm(|ids, app_event| {
                let Some(items) = parse_message_summary_items(ids.iter().map(String::as_str))
                else {
                    return;
                };
                app_event.send(AppEvent::MessageSummarySetup { items });
            })
            .on_cancel(|app_event| {
                app_event.send(AppEvent::MessageSummarySetupCancelled);
            })
            .build(),
        }
    }

    fn summary_select_item(item: MessageSummaryItem, enabled: bool) -> MultiSelectItem {
        MultiSelectItem {
            id: item.to_string(),
            name: item.display_name().to_string(),
            description: Some(item.description().to_string()),
            enabled,
            orderable: true,
            section_break_after: false,
        }
    }
}

impl BottomPaneView for MessageSummarySetupView {
    fn handle_key_event(&mut self, key_event: crossterm::event::KeyEvent) {
        self.picker.handle_key_event(key_event);
    }

    fn is_complete(&self) -> bool {
        self.picker.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.picker.close();
        CancellationEvent::Handled
    }
}

impl Renderable for MessageSummarySetupView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.picker.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.picker.desired_height(width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use tokio::sync::mpsc::unbounded_channel;

    fn render_lines(view: &MessageSummarySetupView, width: u16) -> String {
        let height = view.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        (0..area.height)
            .map(|row| {
                let mut line = String::new();
                for col in 0..area.width {
                    let symbol = buf[(area.x + col, area.y + row)].symbol();
                    if symbol.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(symbol);
                    }
                }
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_message_summary_setup_popup() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let selected = ["worked-for".to_string(), "ttft".to_string()];
        let view = MessageSummarySetupView::new(
            Some(&selected),
            AppEventSender::new(tx_raw),
            crate::keymap::RuntimeKeymap::defaults().list,
        );

        assert_snapshot!(
            "message_summary_setup_basic",
            render_lines(&view, /*width*/ 84)
        );
    }
}
