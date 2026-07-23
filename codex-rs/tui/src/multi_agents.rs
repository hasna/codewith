//! Helpers for rendering and navigating multi-agent state in the TUI.
//!
//! This module owns the shared presentation contracts for multi-agent history rows, `/session` picker
//! entries, and the fast-switch keyboard shortcuts. Higher-level coordination, such as deciding
//! which thread becomes active or when a thread closes, stays in [`crate::app::App`].

use crate::history_cell::PlainHistoryCell;
use crate::render::line_utils::prefix_lines;
use crate::status::format_tokens_compact;
use crate::status_indicator_widget::fmt_elapsed_compact;
use crate::style::accent_color;
use crate::text_formatting::truncate_text;
use codex_app_server_protocol::CollabAgentState;
use codex_app_server_protocol::CollabAgentStatus;
use codex_app_server_protocol::CollabAgentTool;
use codex_app_server_protocol::CollabAgentToolCallStatus;
use codex_app_server_protocol::ThreadItem;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
#[cfg(target_os = "macos")]
use crossterm::event::KeyEventKind;
#[cfg(target_os = "macos")]
use crossterm::event::KeyModifiers;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::collections::HashSet;

const COLLAB_PROMPT_PREVIEW_GRAPHEMES: usize = 160;
const COLLAB_AGENT_ERROR_PREVIEW_GRAPHEMES: usize = 160;
const COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES: usize = 240;
/// Max graphemes of the current-task preview shown in the agent picker description column.
const COLLAB_PICKER_TASK_PREVIEW_GRAPHEMES: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPickerThreadEntry {
    /// Human-friendly nickname shown in picker rows and footer labels.
    pub(crate) agent_nickname: Option<String>,
    /// Agent type shown in brackets when present, for example `worker`.
    pub(crate) agent_role: Option<String>,
    /// Optional persisted thread name set by `/rename` or the agent picker rename action.
    pub(crate) thread_name: Option<String>,
    /// Whether the thread has emitted a close event and should render dimmed.
    pub(crate) is_closed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentMetadata {
    /// Human-friendly nickname shown in rendered tool-call rows.
    pub(crate) agent_nickname: Option<String>,
    /// Agent type shown in brackets when present, for example `worker`.
    pub(crate) agent_role: Option<String>,
    /// Precomputed hierarchical tree path (for example `root/backend_audit/db_check`) rendered when
    /// no nickname is available, so collab rows never fall back to a raw thread UUID.
    pub(crate) tree_path: Option<String>,
}

#[derive(Clone, Copy)]
struct AgentLabel<'a> {
    thread_id: Option<ThreadId>,
    nickname: Option<&'a str>,
    role: Option<&'a str>,
    tree_path: Option<&'a str>,
}

/// Returns the first eight characters of a thread id, used as a last-resort agent label.
///
/// This is deliberately short so a wait/spawn row never prints a full 36-character UUID even when
/// no nickname, role, or tree path is available for the thread.
pub(crate) fn short_thread_id(thread_id: ThreadId) -> String {
    thread_id.to_string().chars().take(8).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnRequestSummary {
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffortConfig,
}

/// Resolves the lifecycle status to render for an agent picker row.
///
/// Live telemetry (folded from the event stream) is authoritative when present, except that a
/// picker entry the app has since marked closed downgrades a still-`Running`/`PendingInit` thread to
/// `Shutdown` so a stale "running" dot never lingers on a thread the backend reports as gone.
/// Threads with no telemetry yet render as `PendingInit` (a hollow, dim dot), or `Shutdown` when
/// already closed.
pub(crate) fn agent_picker_row_status(
    is_closed: bool,
    live_status: Option<CollabAgentStatus>,
) -> CollabAgentStatus {
    match live_status {
        Some(status) if is_terminal_collab_status(&status) => status,
        Some(_) if is_closed => CollabAgentStatus::Shutdown,
        Some(status) => status,
        None if is_closed => CollabAgentStatus::Shutdown,
        None => CollabAgentStatus::PendingInit,
    }
}

fn is_terminal_collab_status(status: &CollabAgentStatus) -> bool {
    matches!(
        status,
        CollabAgentStatus::Completed
            | CollabAgentStatus::Interrupted
            | CollabAgentStatus::Errored
            | CollabAgentStatus::Shutdown
            | CollabAgentStatus::NotFound
    )
}

/// Builds the leading status glyph for an agent picker row.
///
/// The glyph encodes lifecycle status by both shape and color, reusing the `CollabAgentStatus`
/// vocabulary: a running or pending thread shows a dot that is filled (`●`) when it is the currently
/// watched thread and hollow (`○`) otherwise, so the active agent stands out; terminal states show a
/// distinct glyph instead — a green check when completed, a yellow check when interrupted ("wrapped
/// up"), a red cross when errored, and a dim square when stopped.
pub(crate) fn agent_picker_status_dot_spans(
    status: CollabAgentStatus,
    is_active: bool,
) -> Vec<Span<'static>> {
    let glyph: Span<'static> = match status {
        CollabAgentStatus::Completed => "✓".green(),
        // Allow `.yellow()`
        #[allow(clippy::disallowed_methods)]
        CollabAgentStatus::Interrupted => "✓".yellow(),
        CollabAgentStatus::Errored | CollabAgentStatus::NotFound => "✗".red(),
        CollabAgentStatus::Shutdown => "■".dim(),
        CollabAgentStatus::Running if is_active => "●".green(),
        CollabAgentStatus::Running => "○".green(),
        CollabAgentStatus::PendingInit if is_active => "●".dim(),
        CollabAgentStatus::PendingInit => "○".dim(),
    };
    vec![glyph, " ".into()]
}

/// Formats the enriched agent picker description column: `<task>   <elapsed> · ↓ <tokens> tokens`.
///
/// Every segment is optional: a thread with no captured task, no started turn, or no reported token
/// usage simply omits that part. Returns `None` when there is nothing at all to show so the row can
/// fall back to a bare agent name.
pub(crate) fn format_agent_picker_metrics(
    task: Option<&str>,
    elapsed_secs: Option<u64>,
    token_total: i64,
) -> Option<String> {
    let mut stats: Vec<String> = Vec::new();
    if let Some(elapsed_secs) = elapsed_secs {
        stats.push(fmt_elapsed_compact(elapsed_secs));
    }
    if token_total > 0 {
        stats.push(format!("↓ {} tokens", format_tokens_compact(token_total)));
    }

    let task = task
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(|task| truncate_text(task, COLLAB_PICKER_TASK_PREVIEW_GRAPHEMES));

    match (task, stats.is_empty()) {
        (Some(task), true) => Some(task),
        (Some(task), false) => Some(format!("{task}   {}", stats.join(" · "))),
        (None, true) => None,
        (None, false) => Some(stats.join(" · ")),
    }
}

pub(crate) fn format_agent_picker_item_name(
    agent_nickname: Option<&str>,
    agent_role: Option<&str>,
    is_primary: bool,
) -> String {
    if is_primary {
        return "Main [default]".to_string();
    }

    let agent_nickname = agent_nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty());
    let agent_role = agent_role.map(str::trim).filter(|role| !role.is_empty());
    match (agent_nickname, agent_role) {
        (Some(agent_nickname), Some(agent_role)) => format!("{agent_nickname} [{agent_role}]"),
        (Some(agent_nickname), None) => agent_nickname.to_string(),
        (None, Some(agent_role)) => format!("[{agent_role}]"),
        (None, None) => "Agent".to_string(),
    }
}

pub(crate) fn format_agent_picker_entry_name(
    entry: &AgentPickerThreadEntry,
    is_primary: bool,
) -> String {
    if is_primary {
        return format_agent_picker_item_name(
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref(),
            /*is_primary*/ true,
        );
    }

    if let Some(thread_name) = entry.thread_name.as_deref().map(str::trim)
        && !thread_name.is_empty()
    {
        return thread_name.to_string();
    }

    format_agent_picker_item_name(
        entry.agent_nickname.as_deref(),
        entry.agent_role.as_deref(),
        /*is_primary*/ false,
    )
}

pub(crate) fn previous_agent_shortcut() -> crate::key_hint::KeyBinding {
    crate::key_hint::alt(KeyCode::Left)
}

pub(crate) fn next_agent_shortcut() -> crate::key_hint::KeyBinding {
    crate::key_hint::alt(KeyCode::Right)
}

/// Matches the canonical "previous agent" binding plus platform-specific fallbacks that keep agent
/// navigation working when enhanced key reporting is unavailable.
pub(crate) fn previous_agent_shortcut_matches(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    previous_agent_shortcut().is_press(key_event)
        || previous_agent_word_motion_fallback(key_event, allow_word_motion_fallback)
}

/// Matches the canonical "next agent" binding plus platform-specific fallbacks that keep agent
/// navigation working when enhanced key reporting is unavailable.
pub(crate) fn next_agent_shortcut_matches(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    next_agent_shortcut().is_press(key_event)
        || next_agent_word_motion_fallback(key_event, allow_word_motion_fallback)
}

#[cfg(target_os = "macos")]
fn previous_agent_word_motion_fallback(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    // Some terminals, especially on macOS, send Option+b/f as word-motion keys instead of
    // Option+arrow events unless enhanced keyboard reporting is enabled. Callers should only
    // enable this fallback when the composer is empty so draft editing retains the expected
    // word-wise motion behavior.
    allow_word_motion_fallback
        && matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Char('b'),
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        )
}

#[cfg(not(target_os = "macos"))]
fn previous_agent_word_motion_fallback(
    _key_event: KeyEvent,
    _allow_word_motion_fallback: bool,
) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn next_agent_word_motion_fallback(key_event: KeyEvent, allow_word_motion_fallback: bool) -> bool {
    // Some terminals, especially on macOS, send Option+b/f as word-motion keys instead of
    // Option+arrow events unless enhanced keyboard reporting is enabled. Callers should only
    // enable this fallback when the composer is empty so draft editing retains the expected
    // word-wise motion behavior.
    allow_word_motion_fallback
        && matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        )
}

#[cfg(not(target_os = "macos"))]
fn next_agent_word_motion_fallback(
    _key_event: KeyEvent,
    _allow_word_motion_fallback: bool,
) -> bool {
    false
}

pub(crate) fn spawn_request_summary(item: &ThreadItem) -> Option<SpawnRequestSummary> {
    match item {
        ThreadItem::CollabAgentToolCall {
            tool: CollabAgentTool::SpawnAgent,
            model: Some(model),
            reasoning_effort: Some(reasoning_effort),
            ..
        } => Some(SpawnRequestSummary {
            model: model.clone(),
            reasoning_effort: reasoning_effort.clone(),
        }),
        _ => None,
    }
}

pub(crate) fn tool_call_history_cell(
    item: &ThreadItem,
    cached_spawn_request: Option<&SpawnRequestSummary>,
    mut agent_metadata: impl FnMut(ThreadId) -> AgentMetadata,
) -> Option<PlainHistoryCell> {
    let ThreadItem::CollabAgentToolCall {
        tool,
        status,
        receiver_thread_ids,
        prompt,
        agents_states,
        ..
    } = item
    else {
        return None;
    };

    let first_receiver = receiver_thread_ids
        .first()
        .and_then(|id| parse_thread_id(id));
    let prompt = prompt.as_deref().unwrap_or_default();

    match tool {
        CollabAgentTool::SpawnAgent => {
            if matches!(status, CollabAgentToolCallStatus::InProgress) {
                return None;
            }
            let fallback_spawn_request = spawn_request_summary(item);
            let spawn_request = cached_spawn_request.or(fallback_spawn_request.as_ref());
            Some(spawn_end(
                first_receiver,
                prompt,
                spawn_request,
                &mut agent_metadata,
            ))
        }
        CollabAgentTool::SendInput => {
            if matches!(status, CollabAgentToolCallStatus::InProgress) {
                return None;
            }
            first_receiver.map(|receiver_thread_id| {
                interaction_end(receiver_thread_id, prompt, &mut agent_metadata)
            })
        }
        CollabAgentTool::ResumeAgent => first_receiver.map(|receiver_thread_id| {
            if matches!(status, CollabAgentToolCallStatus::InProgress) {
                resume_begin(receiver_thread_id, &mut agent_metadata)
            } else {
                let state = first_agent_state(receiver_thread_ids, agents_states);
                resume_end(
                    receiver_thread_id,
                    state,
                    "Agent resume failed",
                    &mut agent_metadata,
                )
            }
        }),
        CollabAgentTool::Wait => {
            // A waiting turn is already conveyed by the running status spinner
            // (`begin_collab_wait`), so an in-progress `wait_agent` should not
            // add a persistent transcript cell.
            if matches!(status, CollabAgentToolCallStatus::InProgress) {
                return None;
            }
            // On completion, only surface a cell when there are real agent
            // statuses to report. An empty status set previously produced a
            // useless "No agents completed yet" cell on every wait.
            let details =
                wait_complete_lines(receiver_thread_ids, agents_states, &mut agent_metadata);
            if details.is_empty() {
                return None;
            }
            Some(collab_event(title_text("Agents finished"), details))
        }
        CollabAgentTool::CloseAgent => {
            if matches!(status, CollabAgentToolCallStatus::InProgress) {
                return None;
            }
            first_receiver
                .map(|receiver_thread_id| close_end(receiver_thread_id, &mut agent_metadata))
        }
    }
}

fn spawn_end(
    new_thread_id: Option<ThreadId>,
    prompt: &str,
    spawn_request: Option<&SpawnRequestSummary>,
    agent_metadata: &mut impl FnMut(ThreadId) -> AgentMetadata,
) -> PlainHistoryCell {
    let title = match new_thread_id {
        Some(thread_id) => title_with_agent(
            "Spawned",
            agent_label(thread_id, &agent_metadata(thread_id)),
            spawn_request,
        ),
        None => title_text("Agent spawn failed"),
    };

    let mut details = Vec::new();
    if let Some(line) = prompt_line(prompt) {
        details.push(line);
    }
    collab_event(title, details)
}

fn interaction_end(
    receiver_thread_id: ThreadId,
    prompt: &str,
    agent_metadata: &mut impl FnMut(ThreadId) -> AgentMetadata,
) -> PlainHistoryCell {
    let title = title_with_agent(
        "Sent input to",
        agent_label(receiver_thread_id, &agent_metadata(receiver_thread_id)),
        /*spawn_request*/ None,
    );

    let mut details = Vec::new();
    if let Some(line) = prompt_line(prompt) {
        details.push(line);
    }
    collab_event(title, details)
}

fn close_end(
    receiver_thread_id: ThreadId,
    agent_metadata: &mut impl FnMut(ThreadId) -> AgentMetadata,
) -> PlainHistoryCell {
    collab_event(
        title_with_agent(
            "Closed",
            agent_label(receiver_thread_id, &agent_metadata(receiver_thread_id)),
            /*spawn_request*/ None,
        ),
        Vec::new(),
    )
}

fn resume_begin(
    receiver_thread_id: ThreadId,
    agent_metadata: &mut impl FnMut(ThreadId) -> AgentMetadata,
) -> PlainHistoryCell {
    collab_event(
        title_with_agent(
            "Resuming",
            agent_label(receiver_thread_id, &agent_metadata(receiver_thread_id)),
            /*spawn_request*/ None,
        ),
        Vec::new(),
    )
}

fn resume_end(
    receiver_thread_id: ThreadId,
    status: Option<&CollabAgentState>,
    fallback_error: &str,
    agent_metadata: &mut impl FnMut(ThreadId) -> AgentMetadata,
) -> PlainHistoryCell {
    collab_event(
        title_with_agent(
            "Resumed",
            agent_label(receiver_thread_id, &agent_metadata(receiver_thread_id)),
            /*spawn_request*/ None,
        ),
        vec![status_summary_line(status, fallback_error)],
    )
}

fn collab_event(title: Line<'static>, details: Vec<Line<'static>>) -> PlainHistoryCell {
    let mut lines: Vec<Line<'static>> = vec![title];
    if !details.is_empty() {
        lines.extend(prefix_lines(details, "  └ ".dim(), "    ".into()));
    }
    PlainHistoryCell::new(lines)
}

fn title_text(title: impl Into<String>) -> Line<'static> {
    title_spans_line(vec![Span::from(title.into()).bold()])
}

fn title_with_agent(
    prefix: &str,
    agent: AgentLabel<'_>,
    spawn_request: Option<&SpawnRequestSummary>,
) -> Line<'static> {
    let mut spans = vec![Span::from(format!("{prefix} ")).bold()];
    spans.extend(agent_label_spans(agent));
    spans.extend(spawn_request_spans(spawn_request));
    title_spans_line(spans)
}

fn title_spans_line(mut spans: Vec<Span<'static>>) -> Line<'static> {
    let mut title = Vec::with_capacity(spans.len() + 1);
    title.push(Span::from("• ").dim());
    title.append(&mut spans);
    title.into()
}

fn parse_thread_id(thread_id: &str) -> Option<ThreadId> {
    ThreadId::from_string(thread_id).ok()
}

fn agent_label(thread_id: ThreadId, metadata: &AgentMetadata) -> AgentLabel<'_> {
    AgentLabel {
        thread_id: Some(thread_id),
        nickname: metadata.agent_nickname.as_deref(),
        role: metadata.agent_role.as_deref(),
        tree_path: metadata.tree_path.as_deref(),
    }
}

fn agent_label_spans(agent: AgentLabel<'_>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let nickname = agent
        .nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty());
    let role = agent.role.map(str::trim).filter(|role| !role.is_empty());

    let tree_path = agent
        .tree_path
        .map(str::trim)
        .filter(|tree_path| !tree_path.is_empty());

    if let Some(nickname) = nickname {
        spans.push(Span::from(nickname.to_string()).fg(accent_color()).bold());
    } else if let Some(tree_path) = tree_path {
        // Prefer the readable hierarchical path over a raw id. `agent_tree_path` guarantees this
        // never contains a full UUID (worst case an 8-character short id).
        spans.push(Span::from(tree_path.to_string()).fg(accent_color()));
    } else if let Some(thread_id) = agent.thread_id {
        // Last resort for threads with no cached metadata at all: an 8-character short id, never
        // the full 36-character UUID.
        spans.push(Span::from(short_thread_id(thread_id)).fg(accent_color()));
    } else {
        spans.push(Span::from("agent").fg(accent_color()));
    }

    if let Some(role) = role {
        spans.push(Span::from(" ").dim());
        spans.push(Span::from(format!("[{role}]")));
    }

    spans
}

fn spawn_request_spans(spawn_request: Option<&SpawnRequestSummary>) -> Vec<Span<'static>> {
    let Some(spawn_request) = spawn_request else {
        return Vec::new();
    };

    let model = spawn_request.model.trim();
    if model.is_empty() && spawn_request.reasoning_effort == ReasoningEffortConfig::default() {
        return Vec::new();
    }

    let details = if model.is_empty() {
        format!("({})", spawn_request.reasoning_effort)
    } else {
        format!("({model} {})", spawn_request.reasoning_effort)
    };

    vec![Span::from(" ").dim(), Span::from(details).magenta()]
}

fn prompt_line(prompt: &str) -> Option<Line<'static>> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(Line::from(Span::from(truncate_text(
            trimmed,
            COLLAB_PROMPT_PREVIEW_GRAPHEMES,
        ))))
    }
}

fn wait_complete_lines(
    receiver_thread_ids: &[String],
    agents_states: &std::collections::HashMap<String, CollabAgentState>,
    agent_metadata: &mut impl FnMut(ThreadId) -> AgentMetadata,
) -> Vec<Line<'static>> {
    let mut seen = HashSet::new();
    let mut entries = receiver_thread_ids
        .iter()
        .filter_map(|thread_id| {
            let parsed_thread_id = parse_thread_id(thread_id)?;
            let status = agents_states.get(thread_id)?;
            seen.insert(parsed_thread_id);
            Some((parsed_thread_id, agent_metadata(parsed_thread_id), status))
        })
        .collect::<Vec<_>>();

    let mut extras = agents_states
        .iter()
        .filter_map(|(thread_id, status)| {
            let parsed_thread_id = parse_thread_id(thread_id)?;
            (!seen.contains(&parsed_thread_id))
                .then(|| (parsed_thread_id, agent_metadata(parsed_thread_id), status))
        })
        .collect::<Vec<_>>();
    extras.sort_by_key(|entry| entry.0.to_string());
    entries.extend(extras);

    // Empty => caller suppresses the cell entirely (no "No agents completed yet"
    // noise); the status spinner already conveyed the wait.
    entries
        .into_iter()
        .map(|(thread_id, metadata, status)| {
            let mut spans = agent_label_spans(agent_label(thread_id, &metadata));
            spans.push(Span::from(": ").dim());
            spans.extend(status_summary_spans(status));
            spans.into()
        })
        .collect()
}

fn first_agent_state<'a>(
    receiver_thread_ids: &[String],
    agents_states: &'a std::collections::HashMap<String, CollabAgentState>,
) -> Option<&'a CollabAgentState> {
    receiver_thread_ids
        .iter()
        .find_map(|thread_id| agents_states.get(thread_id))
        .or_else(|| {
            agents_states
                .iter()
                .min_by(|left, right| left.0.cmp(right.0))
                .map(|(_, status)| status)
        })
}

fn status_summary_line(status: Option<&CollabAgentState>, fallback_error: &str) -> Line<'static> {
    match status {
        Some(status) => status_summary_spans(status).into(),
        None => error_summary_spans(fallback_error).into(),
    }
}

fn status_summary_spans(status: &CollabAgentState) -> Vec<Span<'static>> {
    match status.status {
        CollabAgentStatus::PendingInit => vec![Span::from("Pending init").fg(accent_color())],
        CollabAgentStatus::Running => vec![Span::from("Running").fg(accent_color()).bold()],
        // Allow `.yellow()`
        #[allow(clippy::disallowed_methods)]
        CollabAgentStatus::Interrupted => vec![Span::from("Interrupted").yellow()],
        CollabAgentStatus::Completed => {
            let mut spans = vec![Span::from("Completed").green()];
            if let Some(message) = status.message.as_ref() {
                let message_preview = truncate_text(
                    &message.split_whitespace().collect::<Vec<_>>().join(" "),
                    COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES,
                );
                if !message_preview.is_empty() {
                    spans.push(Span::from(" - ").dim());
                    spans.push(Span::from(message_preview));
                }
            }
            spans
        }
        CollabAgentStatus::Errored => {
            error_summary_spans(status.message.as_deref().unwrap_or("Agent errored"))
        }
        CollabAgentStatus::Shutdown => vec![Span::from("Shutdown")],
        CollabAgentStatus::NotFound => vec![Span::from("Not found").red()],
    }
}

fn error_summary_spans(error: &str) -> Vec<Span<'static>> {
    let mut spans = vec![Span::from("Error").red()];
    let error_preview = truncate_text(
        &error.split_whitespace().collect::<Vec<_>>().join(" "),
        COLLAB_AGENT_ERROR_PREVIEW_GRAPHEMES,
    );
    if !error_preview.is_empty() {
        spans.push(Span::from(" - ").dim());
        spans.push(Span::from(error_preview));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::HistoryCell;
    #[cfg(target_os = "macos")]
    use crossterm::event::KeyEvent;
    #[cfg(target_os = "macos")]
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::style::Color;
    use ratatui::style::Modifier;
    use std::collections::HashMap;

    #[test]
    fn collab_events_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");
        let bob_id = ThreadId::from_string("00000000-0000-0000-0000-000000000003")
            .expect("valid bob thread id");

        let spawn = tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-spawn".to_string(),
                tool: CollabAgentTool::SpawnAgent,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![robie_id.to_string()],
                prompt: Some("Compute 11! and reply with just the integer result.".to_string()),
                model: Some("gpt-5".to_string()),
                reasoning_effort: Some(ReasoningEffortConfig::High),
                agents_states: HashMap::from([(
                    robie_id.to_string(),
                    agent_state(CollabAgentStatus::PendingInit, /*message*/ None),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| metadata_for(thread_id, robie_id, bob_id),
        )
        .expect("spawn item renders");

        let send = tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-send".to_string(),
                tool: CollabAgentTool::SendInput,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![robie_id.to_string()],
                prompt: Some("Please continue and return the answer only.".to_string()),
                model: None,
                reasoning_effort: None,
                agents_states: HashMap::from([(
                    robie_id.to_string(),
                    agent_state(CollabAgentStatus::Running, /*message*/ None),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| metadata_for(thread_id, robie_id, bob_id),
        )
        .expect("send-input item renders");

        // An in-progress wait is conveyed by the status spinner, not a cell.
        assert!(
            tool_call_history_cell(
                &ThreadItem::CollabAgentToolCall {
                    id: "call-wait".to_string(),
                    tool: CollabAgentTool::Wait,
                    status: CollabAgentToolCallStatus::InProgress,
                    sender_thread_id: sender_thread_id.to_string(),
                    receiver_thread_ids: vec![robie_id.to_string()],
                    prompt: None,
                    model: None,
                    reasoning_effort: None,
                    agents_states: HashMap::new(),
                },
                /*cached_spawn_request*/ None,
                |thread_id| metadata_for(thread_id, robie_id, bob_id),
            )
            .is_none(),
            "an in-progress wait should not render a persistent cell",
        );

        // A completed wait with no agent statuses is also suppressed (no more
        // "No agents completed yet" noise).
        assert!(
            tool_call_history_cell(
                &ThreadItem::CollabAgentToolCall {
                    id: "call-wait-empty".to_string(),
                    tool: CollabAgentTool::Wait,
                    status: CollabAgentToolCallStatus::Completed,
                    sender_thread_id: sender_thread_id.to_string(),
                    receiver_thread_ids: vec![robie_id.to_string()],
                    prompt: None,
                    model: None,
                    reasoning_effort: None,
                    agents_states: HashMap::new(),
                },
                /*cached_spawn_request*/ None,
                |thread_id| metadata_for(thread_id, robie_id, bob_id),
            )
            .is_none(),
            "a completed wait with no agent statuses should not render a cell",
        );

        let finished = tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-wait".to_string(),
                tool: CollabAgentTool::Wait,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![robie_id.to_string(), bob_id.to_string()],
                prompt: None,
                model: None,
                reasoning_effort: None,
                agents_states: HashMap::from([
                    (
                        robie_id.to_string(),
                        agent_state(CollabAgentStatus::Completed, Some("39916800")),
                    ),
                    (
                        bob_id.to_string(),
                        agent_state(CollabAgentStatus::Errored, Some("tool timeout")),
                    ),
                ]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| metadata_for(thread_id, robie_id, bob_id),
        )
        .expect("wait end item renders");

        let close = tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-close".to_string(),
                tool: CollabAgentTool::CloseAgent,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![robie_id.to_string()],
                prompt: None,
                model: None,
                reasoning_effort: None,
                agents_states: HashMap::from([(
                    robie_id.to_string(),
                    agent_state(CollabAgentStatus::Completed, Some("39916800")),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| metadata_for(thread_id, robie_id, bob_id),
        )
        .expect("close item renders");

        let snapshot = [spawn, send, finished, close]
            .iter()
            .map(cell_to_text)
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_snapshot!("collab_agent_transcript", snapshot);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn agent_shortcut_matches_option_arrow_word_motion_fallbacks_only_when_allowed() {
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ true,
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ true,
        ));
        assert!(!previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
        assert!(!next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn agent_shortcut_matches_option_arrows_only() {
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Left, crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Right, crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
        assert!(!previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
        assert!(!next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
    }

    #[test]
    fn title_styles_nickname_and_role() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");
        let cell = tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-spawn".to_string(),
                tool: CollabAgentTool::SpawnAgent,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![robie_id.to_string()],
                prompt: Some(String::new()),
                model: Some("gpt-5".to_string()),
                reasoning_effort: Some(ReasoningEffortConfig::High),
                agents_states: HashMap::from([(
                    robie_id.to_string(),
                    agent_state(CollabAgentStatus::PendingInit, /*message*/ None),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| metadata_for(thread_id, robie_id, ThreadId::new()),
        )
        .expect("spawn item renders");

        let lines = cell.display_lines(/*width*/ 200);
        let title = &lines[0];
        assert_eq!(title.spans[2].content.as_ref(), "Robie");
        assert_eq!(title.spans[2].style.fg, Some(accent_color()));
        assert!(title.spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(title.spans[4].content.as_ref(), "[explorer]");
        assert_eq!(title.spans[4].style.fg, None);
        assert!(!title.spans[4].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(title.spans[6].content.as_ref(), "(gpt-5 high)");
        assert_eq!(title.spans[6].style.fg, Some(Color::Magenta));
    }

    #[test]
    fn collab_resume_interrupted_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");

        let cell = tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-resume".to_string(),
                tool: CollabAgentTool::ResumeAgent,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![robie_id.to_string()],
                prompt: None,
                model: None,
                reasoning_effort: None,
                agents_states: HashMap::from([(
                    robie_id.to_string(),
                    agent_state(CollabAgentStatus::Interrupted, /*message*/ None),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| metadata_for(thread_id, robie_id, ThreadId::new()),
        )
        .expect("resume item renders");

        assert_snapshot!("collab_resume_interrupted", cell_to_text(&cell));
    }

    /// Builds a single-receiver completed-wait row for the given receiver
    /// metadata, exercising the agent-label rendering in the surviving cell.
    fn waiting_row(receiver: ThreadId, metadata: AgentMetadata) -> PlainHistoryCell {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-wait".to_string(),
                tool: CollabAgentTool::Wait,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![receiver.to_string()],
                prompt: None,
                model: None,
                reasoning_effort: None,
                agents_states: HashMap::from([(
                    receiver.to_string(),
                    agent_state(CollabAgentStatus::Completed, /*message*/ None),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| {
                if thread_id == receiver {
                    metadata.clone()
                } else {
                    AgentMetadata::default()
                }
            },
        )
        .expect("completed wait row renders")
    }

    #[test]
    fn nickname_less_row_renders_tree_path_instead_of_uuid() {
        let receiver = ThreadId::from_string("019f8894-89dc-70f2-ad8e-d74deba8ed9b")
            .expect("valid receiver thread id");
        let cell = waiting_row(
            receiver,
            AgentMetadata {
                agent_nickname: None,
                agent_role: None,
                tree_path: Some("root/backend_audit/db_check".to_string()),
            },
        );

        let text = cell_to_text(&cell);
        assert!(
            text.contains("root/backend_audit/db_check"),
            "expected hierarchical tree path, got: {text}"
        );
        assert!(
            !text.contains(&receiver.to_string()),
            "must not leak the full receiver UUID, got: {text}"
        );
    }

    #[test]
    fn nickname_less_row_without_tree_path_renders_short_id_not_uuid() {
        let receiver = ThreadId::from_string("019f8894-89dc-70f2-ad8e-d74deba8ed9b")
            .expect("valid receiver thread id");
        let cell = waiting_row(receiver, AgentMetadata::default());

        let text = cell_to_text(&cell);
        assert!(
            text.contains("019f8894"),
            "expected 8-char short id, got: {text}"
        );
        assert!(
            !text.contains(&receiver.to_string()),
            "must not leak the full receiver UUID, got: {text}"
        );
    }

    /// Builds a completed spawn row for the given child receiver and its cached render metadata,
    /// exercising exactly the agent-label rendering the live "Spawned" row uses.
    fn spawn_row(receiver: ThreadId, metadata: AgentMetadata) -> PlainHistoryCell {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        tool_call_history_cell(
            &ThreadItem::CollabAgentToolCall {
                id: "call-spawn".to_string(),
                tool: CollabAgentTool::SpawnAgent,
                status: CollabAgentToolCallStatus::Completed,
                sender_thread_id: sender_thread_id.to_string(),
                receiver_thread_ids: vec![receiver.to_string()],
                prompt: Some("Explore the repo".to_string()),
                model: Some("gpt-5".to_string()),
                reasoning_effort: Some(ReasoningEffortConfig::High),
                agents_states: HashMap::from([(
                    receiver.to_string(),
                    agent_state(CollabAgentStatus::PendingInit, /*message*/ None),
                )]),
            },
            /*cached_spawn_request*/ None,
            |thread_id| {
                if thread_id == receiver {
                    metadata.clone()
                } else {
                    AgentMetadata::default()
                }
            },
        )
        .expect("spawn row renders")
    }

    #[test]
    fn spawn_cell_renders_tree_path_instead_of_uuid() {
        let receiver = ThreadId::from_string("019f8894-89dc-70f2-ad8e-d74deba8ed9b")
            .expect("valid receiver thread id");
        let cell = spawn_row(
            receiver,
            AgentMetadata {
                agent_nickname: None,
                agent_role: None,
                tree_path: Some("root/backend_audit/db_check".to_string()),
            },
        );

        let text = cell_to_text(&cell);
        assert!(
            text.starts_with("• Spawned root/backend_audit/db_check"),
            "expected hierarchical tree path in the spawn row, got: {text}"
        );
        // The model/effort annotation from the spawn request is preserved.
        assert!(
            text.contains("(gpt-5 high)"),
            "expected the spawn request model annotation, got: {text}"
        );
        assert!(
            !text.contains("019f8894"),
            "spawn row must render the path, not a short/raw thread id, got: {text}"
        );
        assert!(
            !text.contains(&receiver.to_string()),
            "must not leak the full receiver UUID, got: {text}"
        );
    }

    #[test]
    fn spawn_cell_renders_distinct_paths_for_distinct_children() {
        // Two siblings spawned inside the same ~65s window share an 8-char UUIDv7 prefix, so the
        // short-id fallback would render an identical label for both. A resolved tree path keeps
        // them distinct.
        let first = ThreadId::from_string("019f8894-89dc-70f2-ad8e-d74deba8ed9b")
            .expect("valid first thread id");
        let second = ThreadId::from_string("019f8894-89dc-70f2-ad8e-000000000002")
            .expect("valid second thread id");
        assert_eq!(
            short_thread_id(first),
            short_thread_id(second),
            "sibling short ids are expected to collide, which is why the path matters"
        );

        let first_text = cell_to_text(&spawn_row(
            first,
            AgentMetadata {
                agent_nickname: None,
                agent_role: None,
                tree_path: Some("root/reviewer".to_string()),
            },
        ));
        let second_text = cell_to_text(&spawn_row(
            second,
            AgentMetadata {
                agent_nickname: None,
                agent_role: None,
                tree_path: Some("root/explorer".to_string()),
            },
        ));

        assert!(first_text.starts_with("• Spawned root/reviewer"));
        assert!(second_text.starts_with("• Spawned root/explorer"));
        assert_ne!(
            first_text, second_text,
            "distinct children must render distinct spawn rows"
        );
    }

    fn agent_state(status: CollabAgentStatus, message: Option<&str>) -> CollabAgentState {
        CollabAgentState {
            status,
            message: message.map(str::to_string),
            agent_path: None,
        }
    }

    fn metadata_for(thread_id: ThreadId, robie_id: ThreadId, bob_id: ThreadId) -> AgentMetadata {
        if thread_id == robie_id {
            AgentMetadata {
                agent_nickname: Some("Robie".to_string()),
                agent_role: Some("explorer".to_string()),
                tree_path: None,
            }
        } else if thread_id == bob_id {
            AgentMetadata {
                agent_nickname: Some("Bob".to_string()),
                agent_role: Some("worker".to_string()),
                tree_path: None,
            }
        } else {
            AgentMetadata::default()
        }
    }

    fn cell_to_text(cell: &PlainHistoryCell) -> String {
        cell.display_lines(/*width*/ 200)
            .iter()
            .map(line_to_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_to_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn agent_picker_row_status_prefers_live_status_but_respects_closed() {
        // No telemetry yet: pending when open, stopped when the picker marked it closed.
        assert_eq!(
            agent_picker_row_status(/*is_closed*/ false, None),
            CollabAgentStatus::PendingInit
        );
        assert_eq!(
            agent_picker_row_status(/*is_closed*/ true, None),
            CollabAgentStatus::Shutdown
        );
        // A live running thread the picker has since marked closed downgrades to stopped.
        assert_eq!(
            agent_picker_row_status(/*is_closed*/ true, Some(CollabAgentStatus::Running)),
            CollabAgentStatus::Shutdown
        );
        // Terminal telemetry is authoritative even when the entry is closed.
        assert_eq!(
            agent_picker_row_status(/*is_closed*/ true, Some(CollabAgentStatus::Completed)),
            CollabAgentStatus::Completed
        );
        assert_eq!(
            agent_picker_row_status(/*is_closed*/ false, Some(CollabAgentStatus::Running)),
            CollabAgentStatus::Running
        );
    }

    #[test]
    fn agent_picker_status_dot_spans_encode_status_shape_and_color() {
        let running_active = agent_picker_status_dot_spans(CollabAgentStatus::Running, true);
        assert_eq!(running_active[0].content.as_ref(), "●");
        assert_eq!(running_active[0].style.fg, Some(Color::Green));

        let running_idle = agent_picker_status_dot_spans(CollabAgentStatus::Running, false);
        assert_eq!(running_idle[0].content.as_ref(), "○");
        assert_eq!(running_idle[0].style.fg, Some(Color::Green));

        let completed = agent_picker_status_dot_spans(CollabAgentStatus::Completed, false);
        assert_eq!(completed[0].content.as_ref(), "✓");
        assert_eq!(completed[0].style.fg, Some(Color::Green));

        let interrupted = agent_picker_status_dot_spans(CollabAgentStatus::Interrupted, false);
        assert_eq!(interrupted[0].content.as_ref(), "✓");
        assert_eq!(interrupted[0].style.fg, Some(Color::Yellow));

        let errored = agent_picker_status_dot_spans(CollabAgentStatus::Errored, false);
        assert_eq!(errored[0].content.as_ref(), "✗");
        assert_eq!(errored[0].style.fg, Some(Color::Red));

        let shutdown = agent_picker_status_dot_spans(CollabAgentStatus::Shutdown, false);
        assert_eq!(shutdown[0].content.as_ref(), "■");
        assert!(shutdown[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn format_agent_picker_metrics_composes_task_elapsed_and_tokens() {
        assert_eq!(
            format_agent_picker_metrics(Some("audit the backend"), Some(125), 69_742),
            Some("audit the backend   2m 05s · ↓ 69.7K tokens".to_string())
        );
        assert_eq!(
            format_agent_picker_metrics(None, Some(5), 0),
            Some("5s".to_string())
        );
        assert_eq!(
            format_agent_picker_metrics(Some("just a task"), None, 0),
            Some("just a task".to_string())
        );
        assert_eq!(
            format_agent_picker_metrics(None, None, 0),
            None,
            "an empty row should have no description"
        );
    }

    #[test]
    fn agent_picker_rows_snapshot() {
        // Deterministic stand-in for the live picker rows: `<dot> <name>  <task> <elapsed> · tokens`
        // with Main first, one running/active row, one completed row, and one errored row.
        let rows = [
            (
                CollabAgentStatus::Running,
                /*is_active*/ true,
                "Main [default]",
                format_agent_picker_metrics(Some("triage the failing suite"), Some(125), 69_742),
            ),
            (
                CollabAgentStatus::Completed,
                /*is_active*/ false,
                "Robie [explorer]",
                format_agent_picker_metrics(Some("map the crate graph"), Some(42), 12_800),
            ),
            (
                CollabAgentStatus::Errored,
                /*is_active*/ false,
                "Bob [worker]",
                format_agent_picker_metrics(Some("run migrations"), Some(9), 1_024),
            ),
        ];

        let snapshot = rows
            .iter()
            .map(|(status, is_active, name, description)| {
                let mut spans = agent_picker_status_dot_spans(status.clone(), *is_active);
                spans.push(Span::from((*name).to_string()));
                if let Some(description) = description {
                    spans.push(Span::from("  "));
                    spans.push(Span::from(description.clone()));
                }
                line_to_text(&Line::from(spans))
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert_snapshot!("agent_picker_rows", snapshot);
    }
}
