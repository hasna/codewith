//! Turn separators and runtime-metrics labels for transcript history.

use super::*;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;

pub(crate) const DEFAULT_MESSAGE_SUMMARY_ITEMS: &[MessageSummaryItem] = &[
    MessageSummaryItem::WorkedFor,
    MessageSummaryItem::LocalTools,
    MessageSummaryItem::Inference,
    MessageSummaryItem::Streams,
    MessageSummaryItem::WebSocket,
    MessageSummaryItem::ResponsesApi,
    MessageSummaryItem::Ttft,
    MessageSummaryItem::Tbt,
];

#[derive(
    EnumIter, EnumString, Display, Debug, Clone, Copy, Eq, Hash, PartialEq, Ord, PartialOrd,
)]
#[strum(serialize_all = "kebab_case")]
pub(crate) enum MessageSummaryItem {
    /// Assistant turn duration, shown after one minute.
    #[strum(to_string = "worked-for", serialize = "duration")]
    WorkedFor,
    /// Local tool call count and duration.
    LocalTools,
    /// Inference API call count and duration.
    Inference,
    /// Stream event count and duration.
    Streams,
    /// WebSocket send and receive timing.
    WebSocket,
    /// Responses API overhead and inference timing.
    #[strum(to_string = "responses-api", serialize = "responses")]
    ResponsesApi,
    /// Time to first token.
    #[strum(to_string = "ttft", serialize = "time-to-first-token")]
    Ttft,
    /// Time between tokens.
    #[strum(to_string = "tbt", serialize = "time-between-tokens")]
    Tbt,
}

impl MessageSummaryItem {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            MessageSummaryItem::WorkedFor => "Worked for",
            MessageSummaryItem::LocalTools => "Local tools",
            MessageSummaryItem::Inference => "Inference",
            MessageSummaryItem::Streams => "Streams",
            MessageSummaryItem::WebSocket => "WebSocket",
            MessageSummaryItem::ResponsesApi => "Responses API",
            MessageSummaryItem::Ttft => "TTFT",
            MessageSummaryItem::Tbt => "TBT",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            MessageSummaryItem::WorkedFor => "Assistant turn duration after one minute",
            MessageSummaryItem::LocalTools => "Local tool call count and duration",
            MessageSummaryItem::Inference => "Inference request count and duration",
            MessageSummaryItem::Streams => "Streaming event count and duration",
            MessageSummaryItem::WebSocket => "WebSocket send and receive timing",
            MessageSummaryItem::ResponsesApi => "Responses API overhead and inference timing",
            MessageSummaryItem::Ttft => "Time to first token",
            MessageSummaryItem::Tbt => "Time between tokens",
        }
    }
}

#[derive(Debug)]
/// A visual divider between turns, optionally showing how long the assistant "worked for".
///
/// This separator is only emitted for turns that performed concrete work (e.g., running commands,
/// applying patches, making MCP tool calls), so purely conversational turns do not show an empty
/// divider.
pub struct FinalMessageSeparator {
    elapsed_seconds: Option<u64>,
    runtime_metrics: Option<RuntimeMetricsSummary>,
    summary_items: Option<Vec<MessageSummaryItem>>,
}
impl FinalMessageSeparator {
    /// Creates a separator; completed turns should pass protocol turn duration when available.
    pub(crate) fn new(
        elapsed_seconds: Option<u64>,
        runtime_metrics: Option<RuntimeMetricsSummary>,
    ) -> Self {
        Self {
            elapsed_seconds,
            runtime_metrics,
            summary_items: None,
        }
    }

    pub(crate) fn new_with_items(
        elapsed_seconds: Option<u64>,
        runtime_metrics: Option<RuntimeMetricsSummary>,
        summary_items: impl IntoIterator<Item = MessageSummaryItem>,
    ) -> Self {
        Self {
            elapsed_seconds,
            runtime_metrics,
            summary_items: Some(summary_items.into_iter().collect()),
        }
    }

    fn label_parts(&self) -> Vec<String> {
        match self.summary_items.as_deref() {
            Some(summary_items) => {
                message_summary_labels(self.elapsed_seconds, self.runtime_metrics, summary_items)
            }
            None => default_message_summary_labels(self.elapsed_seconds, self.runtime_metrics),
        }
    }
}
impl HistoryCell for FinalMessageSeparator {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let label_parts = self.label_parts();

        if label_parts.is_empty() {
            return vec![Line::from_iter(["─".repeat(width as usize).dim()])];
        }

        let label = format!("─ {} ─", label_parts.join(" • "));
        let (label, _suffix, label_width) = take_prefix_by_width(&label, width as usize);
        vec![
            Line::from_iter([
                label,
                "─".repeat((width as usize).saturating_sub(label_width)),
            ])
            .dim(),
        ]
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let label_parts = self.label_parts();
        if label_parts.is_empty() {
            Vec::new()
        } else {
            vec![Line::from(label_parts.join(" • "))]
        }
    }
}

pub(crate) fn runtime_metrics_label(summary: RuntimeMetricsSummary) -> Option<String> {
    let labels = default_message_summary_labels(/*elapsed_seconds*/ None, Some(summary));
    (!labels.is_empty()).then(|| labels.join(" • "))
}

fn default_message_summary_labels(
    elapsed_seconds: Option<u64>,
    runtime_metrics: Option<RuntimeMetricsSummary>,
) -> Vec<String> {
    let mut parts = Vec::new();
    push_worked_for_label(&mut parts, elapsed_seconds);

    let Some(summary) = runtime_metrics.as_ref() else {
        return parts;
    };

    push_local_tools_label(&mut parts, summary);
    push_inference_label(&mut parts, summary);
    push_websocket_send_label(&mut parts, summary);
    push_streams_label(&mut parts, summary);
    push_websocket_received_label(&mut parts, summary);
    push_responses_api_labels(&mut parts, summary);
    push_ttft_label(&mut parts, summary);
    push_tbt_label(&mut parts, summary);
    parts
}

pub(crate) fn message_summary_labels(
    elapsed_seconds: Option<u64>,
    runtime_metrics: Option<RuntimeMetricsSummary>,
    summary_items: &[MessageSummaryItem],
) -> Vec<String> {
    let mut parts = Vec::new();

    for item in summary_items {
        match item {
            MessageSummaryItem::WorkedFor => {
                push_worked_for_label(&mut parts, elapsed_seconds);
            }
            MessageSummaryItem::LocalTools => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_local_tools_label(&mut parts, summary);
                }
            }
            MessageSummaryItem::Inference => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_inference_label(&mut parts, summary);
                }
            }
            MessageSummaryItem::Streams => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_streams_label(&mut parts, summary);
                }
            }
            MessageSummaryItem::WebSocket => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_websocket_send_label(&mut parts, summary);
                    push_websocket_received_label(&mut parts, summary);
                }
            }
            MessageSummaryItem::ResponsesApi => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_responses_api_labels(&mut parts, summary);
                }
            }
            MessageSummaryItem::Ttft => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_ttft_label(&mut parts, summary);
                }
            }
            MessageSummaryItem::Tbt => {
                if let Some(summary) = runtime_metrics.as_ref() {
                    push_tbt_label(&mut parts, summary);
                }
            }
        }
    }
    parts
}

fn push_worked_for_label(parts: &mut Vec<String>, elapsed_seconds: Option<u64>) {
    if let Some(elapsed_seconds) = elapsed_seconds
        .filter(|seconds| *seconds > 60)
        .map(crate::status_indicator_widget::fmt_elapsed_compact)
    {
        parts.push(format!("Worked for {elapsed_seconds}"));
    }
}

fn push_local_tools_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.tool_calls.count > 0 {
        let duration = format_duration_ms(summary.tool_calls.duration_ms);
        let calls = pluralize(summary.tool_calls.count, "call", "calls");
        parts.push(format!(
            "Local tools: {} {calls} ({duration})",
            summary.tool_calls.count
        ));
    }
}

fn push_inference_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.api_calls.count > 0 {
        let duration = format_duration_ms(summary.api_calls.duration_ms);
        let calls = pluralize(summary.api_calls.count, "call", "calls");
        parts.push(format!(
            "Inference: {} {calls} ({duration})",
            summary.api_calls.count
        ));
    }
}

fn push_streams_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.streaming_events.count > 0 {
        let duration = format_duration_ms(summary.streaming_events.duration_ms);
        let stream_label = pluralize(summary.streaming_events.count, "Stream", "Streams");
        let events = pluralize(summary.streaming_events.count, "event", "events");
        parts.push(format!(
            "{stream_label}: {} {events} ({duration})",
            summary.streaming_events.count
        ));
    }
}

fn push_websocket_send_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.websocket_calls.count > 0 {
        let duration = format_duration_ms(summary.websocket_calls.duration_ms);
        parts.push(format!(
            "WebSocket: {} events send ({duration})",
            summary.websocket_calls.count
        ));
    }
}

fn push_websocket_received_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.websocket_events.count > 0 {
        let duration = format_duration_ms(summary.websocket_events.duration_ms);
        parts.push(format!(
            "{} events received ({duration})",
            summary.websocket_events.count
        ));
    }
}

fn push_responses_api_labels(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.responses_api_overhead_ms > 0 {
        let duration = format_duration_ms(summary.responses_api_overhead_ms);
        parts.push(format!("Responses API overhead: {duration}"));
    }
    if summary.responses_api_inference_time_ms > 0 {
        let duration = format_duration_ms(summary.responses_api_inference_time_ms);
        parts.push(format!("Responses API inference: {duration}"));
    }
}

fn push_ttft_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.responses_api_engine_iapi_ttft_ms > 0
        || summary.responses_api_engine_service_ttft_ms > 0
    {
        let mut ttft_parts = Vec::new();
        if summary.responses_api_engine_iapi_ttft_ms > 0 {
            let duration = format_duration_ms(summary.responses_api_engine_iapi_ttft_ms);
            ttft_parts.push(format!("{duration} (iapi)"));
        }
        if summary.responses_api_engine_service_ttft_ms > 0 {
            let duration = format_duration_ms(summary.responses_api_engine_service_ttft_ms);
            ttft_parts.push(format!("{duration} (service)"));
        }
        parts.push(format!("TTFT: {}", ttft_parts.join(" ")));
    }
}

fn push_tbt_label(parts: &mut Vec<String>, summary: &RuntimeMetricsSummary) {
    if summary.responses_api_engine_iapi_tbt_ms > 0
        || summary.responses_api_engine_service_tbt_ms > 0
    {
        let mut tbt_parts = Vec::new();
        if summary.responses_api_engine_iapi_tbt_ms > 0 {
            let duration = format_duration_ms(summary.responses_api_engine_iapi_tbt_ms);
            tbt_parts.push(format!("{duration} (iapi)"));
        }
        if summary.responses_api_engine_service_tbt_ms > 0 {
            let duration = format_duration_ms(summary.responses_api_engine_service_tbt_ms);
            tbt_parts.push(format!("{duration} (service)"));
        }
        parts.push(format!("TBT: {}", tbt_parts.join(" ")));
    }
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms >= 1_000 {
        let seconds = duration_ms as f64 / 1_000.0;
        format!("{seconds:.1}s")
    } else {
        format!("{duration_ms}ms")
    }
}

fn pluralize(count: u64, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}
