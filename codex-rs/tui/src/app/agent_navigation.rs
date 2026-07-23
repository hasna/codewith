//! Multi-agent picker navigation and labeling state for the TUI app.
//!
//! This module exists to keep the pure parts of multi-agent navigation out of [`crate::app::App`].
//! It owns the stable spawn-order cache used by the `/session` picker, keyboard next/previous
//! navigation, and the contextual footer label for the thread currently being watched.
//!
//! Responsibilities here are intentionally narrow:
//! - remember picker entries and their first-seen order
//! - answer traversal questions like "what is the next thread?"
//! - derive user-facing picker/footer text from cached thread metadata
//!
//! Responsibilities that stay in `App`:
//! - discovering threads from the backend
//! - deciding which thread is currently displayed
//! - mutating UI state such as switching threads or updating the footer widget
//!
//! The key invariant is that traversal follows first-seen spawn order rather than thread-id sort
//! order. Once a thread id is observed it keeps its place in the cycle even if the entry is later
//! updated or marked closed.

use crate::multi_agents::AgentPickerThreadEntry;
use crate::multi_agents::format_agent_picker_entry_name;
use crate::multi_agents::format_agent_picker_item_name;
use crate::multi_agents::next_agent_shortcut;
use crate::multi_agents::previous_agent_shortcut;
use codex_app_server_protocol::CollabAgentStatus;
use codex_protocol::ThreadId;
use ratatui::text::Span;
use std::collections::HashMap;
use std::time::Instant;

/// Small state container for multi-agent picker ordering and labeling.
///
/// `App` owns thread lifecycle and UI side effects. This type keeps the pure rules for stable
/// spawn-order traversal, picker copy, and active-agent labels together and separately testable.
///
/// The core invariant is that `order` records first-seen thread ids exactly once, while `threads`
/// stores the latest metadata for those ids. Mutation is intentionally funneled through `upsert`,
/// `mark_closed`, and `clear` so those two collections do not drift semantically even if they are
/// temporarily out of sync during teardown races.
#[derive(Debug, Default)]
pub(crate) struct AgentNavigationState {
    /// Latest picker metadata for each tracked thread id.
    threads: HashMap<ThreadId, AgentPickerThreadEntry>,
    /// Stable first-seen traversal order for picker rows and keyboard cycling.
    order: Vec<ThreadId>,
    /// Child -> parent edges used to reconstruct a hierarchical agent tree path when an
    /// authoritative `agent_path` is unavailable. Kept separate from `threads` so ancestry
    /// survives thread switches (which rebuild the `ChatWidget`) even when a parent is not itself a
    /// picker row.
    parents: HashMap<ThreadId, ThreadId>,
    /// Authoritative absolute agent paths (for example `/root/backend_audit/db_check`) captured
    /// from the server-composed `AgentPath`, keyed by thread id.
    paths: HashMap<ThreadId, String>,
    /// Live per-thread runtime telemetry (status, elapsed, tokens, task) rendered by the enriched
    /// agent picker. Kept in its own map so it survives `upsert` metadata refreshes and thread
    /// switches; only `clear` and `remove` drop it.
    live_metrics: HashMap<ThreadId, AgentLiveMetrics>,
}

/// Live runtime telemetry for a tracked agent thread.
///
/// These fields are folded from the app-server event stream and rendered in the enriched agent
/// picker rows (status dot, task, elapsed timer, token total). They are intentionally kept here in
/// [`AgentNavigationState`] rather than on the `ChatWidget`: switching into an agent's window
/// rebuilds the `ChatWidget` from scratch, which would otherwise reset the elapsed timer and token
/// total every time the user glanced at a different agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentLiveMetrics {
    /// Latest lifecycle status folded from turn/thread notifications.
    pub(crate) status: CollabAgentStatus,
    /// When the most recent turn started, used to render a live elapsed timer. `None` until the
    /// thread emits its first `TurnStarted`.
    pub(crate) started_at: Option<Instant>,
    /// Short description of the current task, derived from the latest turn's first user input.
    pub(crate) last_task_message: Option<String>,
    /// Cumulative total token usage most recently reported for the thread.
    pub(crate) token_total: i64,
}

impl Default for AgentLiveMetrics {
    fn default() -> Self {
        Self {
            status: CollabAgentStatus::PendingInit,
            started_at: None,
            last_task_message: None,
            token_total: 0,
        }
    }
}

/// Direction of keyboard traversal through the stable picker order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentNavigationDirection {
    /// Move toward the entry that was seen earlier in spawn order, wrapping at the front.
    Previous,
    /// Move toward the entry that was seen later in spawn order, wrapping at the end.
    Next,
}

impl AgentNavigationState {
    /// Returns the cached picker entry for a specific thread id.
    ///
    /// Callers use this when they already know which thread they care about and need the last
    /// metadata captured for picker or footer rendering. If a caller assumes every tracked thread
    /// must be present here, shutdown races can turn that assumption into a panic elsewhere, so
    /// this stays optional.
    pub(crate) fn get(&self, thread_id: &ThreadId) -> Option<&AgentPickerThreadEntry> {
        self.threads.get(thread_id)
    }

    /// Returns whether the picker cache currently knows about any threads.
    ///
    /// This is the cheapest way for `App` to decide whether opening the picker should show "No
    /// agents available yet." rather than constructing picker rows from an empty state.
    pub(crate) fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    /// Inserts or updates a picker entry while preserving first-seen traversal order.
    ///
    /// The key invariant of this module is enforced here: a thread id is appended to `order` only
    /// the first time it is seen. Later updates may change nickname, role, or closed state, but
    /// they must not move the thread in the cycle or keyboard navigation would feel unstable.
    pub(crate) fn upsert(
        &mut self,
        thread_id: ThreadId,
        agent_nickname: Option<String>,
        agent_role: Option<String>,
        is_closed: bool,
    ) {
        let thread_name = self
            .threads
            .get(&thread_id)
            .and_then(|entry| entry.thread_name.clone());
        if !self.threads.contains_key(&thread_id) {
            self.order.push(thread_id);
        }
        self.threads.insert(
            thread_id,
            AgentPickerThreadEntry {
                agent_nickname,
                agent_role,
                thread_name,
                is_closed,
            },
        );
    }

    pub(crate) fn set_thread_name(&mut self, thread_id: ThreadId, thread_name: Option<String>) {
        if let Some(entry) = self.threads.get_mut(&thread_id) {
            entry.thread_name = thread_name;
        }
    }

    /// Records an authoritative child -> parent edge, overwriting any previously cached parent.
    ///
    /// Used at spawn/read sites that carry a trustworthy `parent_thread_id`. The reverse edge is
    /// never stored, and a self-parent is ignored so the tree walk cannot spin on a degenerate edge.
    pub(crate) fn set_parent(&mut self, child_thread_id: ThreadId, parent_thread_id: ThreadId) {
        if child_thread_id == parent_thread_id {
            return;
        }
        self.parents.insert(child_thread_id, parent_thread_id);
    }

    /// Returns the cached parent for a thread, if any.
    pub(crate) fn parent(&self, child_thread_id: &ThreadId) -> Option<ThreadId> {
        self.parents.get(child_thread_id).copied()
    }

    /// Stores the authoritative absolute agent path for a thread.
    pub(crate) fn set_agent_path(&mut self, thread_id: ThreadId, agent_path: String) {
        self.paths.insert(thread_id, agent_path);
    }

    /// Returns the authoritative absolute agent path for a thread, if one was captured.
    pub(crate) fn agent_path(&self, thread_id: &ThreadId) -> Option<&str> {
        self.paths.get(thread_id).map(String::as_str)
    }

    /// Returns whether the navigation cache has a picker entry for a thread.
    ///
    /// Used by the tree-path walk to distinguish a known ancestor from an orphaned edge that points
    /// at a thread we have never seen metadata for.
    pub(crate) fn is_tracked(&self, thread_id: &ThreadId) -> bool {
        self.threads.contains_key(thread_id)
    }

    /// Returns the live runtime telemetry captured for a thread, if any turn/thread event has been
    /// folded for it yet.
    pub(crate) fn metrics(&self, thread_id: &ThreadId) -> Option<&AgentLiveMetrics> {
        self.live_metrics.get(thread_id)
    }

    /// Returns a mutable telemetry record for a thread, creating a default one on first sight.
    ///
    /// The default seeds `PendingInit`/no-elapsed/zero-tokens so a thread that has only just been
    /// observed renders as a hollow pending dot until the first real event arrives.
    fn metrics_mut(&mut self, thread_id: ThreadId) -> &mut AgentLiveMetrics {
        self.live_metrics.entry(thread_id).or_default()
    }

    /// Folds a `TurnStarted` event: the thread is now running and its elapsed timer restarts.
    ///
    /// A non-empty `task_message` (the turn's first user input) is captured so the picker row can
    /// describe what the agent is working on; an empty message leaves the previous task in place.
    pub(crate) fn note_turn_started(&mut self, thread_id: ThreadId, task_message: Option<String>) {
        let task_message = task_message
            .map(|task| task.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|task| !task.is_empty());
        let metrics = self.metrics_mut(thread_id);
        metrics.status = CollabAgentStatus::Running;
        metrics.started_at = Some(Instant::now());
        if task_message.is_some() {
            metrics.last_task_message = task_message;
        }
    }

    /// Folds a `TurnCompleted` event, honoring the terminal turn status mapped by the caller.
    pub(crate) fn note_turn_completed(&mut self, thread_id: ThreadId, status: CollabAgentStatus) {
        self.metrics_mut(thread_id).status = status;
    }

    /// Folds a non-retrying `Error` event: the thread is now errored.
    pub(crate) fn note_error(&mut self, thread_id: ThreadId) {
        self.metrics_mut(thread_id).status = CollabAgentStatus::Errored;
    }

    /// Records a status transition folded from a thread-lifecycle event (close / status change).
    pub(crate) fn note_status(&mut self, thread_id: ThreadId, status: CollabAgentStatus) {
        self.metrics_mut(thread_id).status = status;
    }

    /// Records the cumulative total token usage most recently reported for a thread.
    pub(crate) fn note_token_usage(&mut self, thread_id: ThreadId, token_total: i64) {
        self.metrics_mut(thread_id).token_total = token_total.max(0);
    }

    /// Marks a thread as closed without removing it from the traversal cache.
    ///
    /// Closed threads stay in the picker and in spawn order so users can still review them and so
    /// next/previous navigation does not reshuffle around disappearing entries. If a caller "cleans
    /// this up" by deleting the entry instead, wraparound navigation will silently change shape
    /// mid-session.
    pub(crate) fn mark_closed(&mut self, thread_id: ThreadId) {
        if let Some(entry) = self.threads.get_mut(&thread_id) {
            entry.is_closed = true;
        } else {
            self.upsert(
                thread_id, /*agent_nickname*/ None, /*agent_role*/ None,
                /*is_closed*/ true,
            );
        }
    }

    /// Drops all cached picker state.
    ///
    /// This is used when `App` tears down thread event state and needs the picker cache to return
    /// to a pristine single-session state.
    pub(crate) fn clear(&mut self) {
        self.threads.clear();
        self.order.clear();
        self.parents.clear();
        self.paths.clear();
        self.live_metrics.clear();
    }

    /// Removes a tracked thread entirely from picker metadata and traversal order.
    ///
    /// This is reserved for entries that were only discovered opportunistically and never became
    /// replayable local threads. Keeping those around after the backend confirms they are gone
    /// would leave ghost rows in `/agent`.
    pub(crate) fn remove(&mut self, thread_id: ThreadId) {
        self.threads.remove(&thread_id);
        self.order.retain(|candidate| *candidate != thread_id);
        self.live_metrics.remove(&thread_id);
    }

    /// Returns whether there is at least one tracked thread other than the primary one.
    ///
    /// `App` uses this to decide whether the picker should be available even when the collaboration
    /// feature flag is currently disabled, because already-existing sub-agent threads should remain
    /// inspectable.
    pub(crate) fn has_non_primary_thread(&self, primary_thread_id: Option<ThreadId>) -> bool {
        self.threads
            .keys()
            .any(|thread_id| Some(*thread_id) != primary_thread_id)
    }

    /// Returns live picker rows in the same order users cycle through them.
    ///
    /// The `order` vector is intentionally historical and may briefly contain thread ids that no
    /// longer have cached metadata, so this filters through the map instead of assuming both
    /// collections are perfectly synchronized.
    pub(crate) fn ordered_threads(&self) -> Vec<(ThreadId, &AgentPickerThreadEntry)> {
        self.order
            .iter()
            .filter_map(|thread_id| self.threads.get(thread_id).map(|entry| (*thread_id, entry)))
            .collect()
    }

    /// Returns tracked thread ids in the same stable order used by the picker.
    pub(crate) fn tracked_thread_ids(&self) -> Vec<ThreadId> {
        self.ordered_threads()
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect()
    }

    /// Returns the adjacent thread id for keyboard navigation in stable spawn order.
    ///
    /// The caller must pass the thread whose transcript is actually being shown to the user, not
    /// just whichever thread bookkeeping most recently marked active. If the wrong current thread
    /// is supplied, next/previous navigation will jump in a way that feels nondeterministic even
    /// though the cache itself is correct.
    pub(crate) fn adjacent_thread_id(
        &self,
        current_displayed_thread_id: Option<ThreadId>,
        direction: AgentNavigationDirection,
    ) -> Option<ThreadId> {
        let ordered_threads = self.ordered_threads();
        if ordered_threads.len() < 2 {
            return None;
        }

        let current_thread_id = current_displayed_thread_id?;
        let current_idx = ordered_threads
            .iter()
            .position(|(thread_id, _)| *thread_id == current_thread_id)?;
        let next_idx = match direction {
            AgentNavigationDirection::Next => (current_idx + 1) % ordered_threads.len(),
            AgentNavigationDirection::Previous => {
                if current_idx == 0 {
                    ordered_threads.len() - 1
                } else {
                    current_idx - 1
                }
            }
        };
        Some(ordered_threads[next_idx].0)
    }

    /// Derives the contextual footer label for the currently displayed thread.
    ///
    /// This intentionally returns `None` until there is more than one tracked thread so
    /// single-thread sessions do not waste footer space restating the obvious. When metadata for
    /// the displayed thread is missing, the label falls back to the same generic naming rules used
    /// by the picker.
    pub(crate) fn active_agent_label(
        &self,
        current_displayed_thread_id: Option<ThreadId>,
        primary_thread_id: Option<ThreadId>,
    ) -> Option<String> {
        if self.threads.len() <= 1 {
            return None;
        }

        let thread_id = current_displayed_thread_id?;
        let is_primary = primary_thread_id == Some(thread_id);
        Some(
            self.threads
                .get(&thread_id)
                .map(|entry| format_agent_picker_entry_name(entry, is_primary))
                .unwrap_or_else(|| {
                    format_agent_picker_item_name(
                        /*agent_nickname*/ None, /*agent_role*/ None, is_primary,
                    )
                }),
        )
    }

    /// Builds the `/session` picker subtitle from the same canonical bindings used by key handling.
    ///
    /// Keeping this text derived from the actual shortcut helpers prevents the picker copy from
    /// drifting if the bindings ever change on one platform.
    pub(crate) fn picker_subtitle() -> String {
        let previous: Span<'static> = previous_agent_shortcut().into();
        let next: Span<'static> = next_agent_shortcut().into();
        format!(
            "Switch into an agent's live window. {} previous, {} next.",
            previous.content, next.content
        )
    }

    #[cfg(test)]
    /// Returns only the ordered thread ids for focused tests of traversal invariants.
    ///
    /// This helper exists so tests can assert on ordering without embedding the full picker entry
    /// payload in every expectation.
    pub(crate) fn ordered_thread_ids(&self) -> Vec<ThreadId> {
        self.ordered_threads()
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn populated_state() -> (AgentNavigationState, ThreadId, ThreadId, ThreadId) {
        let mut state = AgentNavigationState::default();
        let main_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000101").expect("valid thread");
        let first_agent_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000102").expect("valid thread");
        let second_agent_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000103").expect("valid thread");

        state.upsert(
            main_thread_id,
            /*agent_nickname*/ None,
            /*agent_role*/ None,
            /*is_closed*/ false,
        );
        state.upsert(
            first_agent_id,
            Some("Robie".to_string()),
            Some("explorer".to_string()),
            /*is_closed*/ false,
        );
        state.upsert(
            second_agent_id,
            Some("Bob".to_string()),
            Some("worker".to_string()),
            /*is_closed*/ false,
        );

        (state, main_thread_id, first_agent_id, second_agent_id)
    }

    #[test]
    fn upsert_preserves_first_seen_order() {
        let (mut state, main_thread_id, first_agent_id, second_agent_id) = populated_state();

        state.upsert(
            first_agent_id,
            Some("Robie".to_string()),
            Some("worker".to_string()),
            /*is_closed*/ true,
        );

        assert_eq!(
            state.ordered_thread_ids(),
            vec![main_thread_id, first_agent_id, second_agent_id]
        );
    }

    #[test]
    fn adjacent_thread_id_wraps_in_spawn_order() {
        let (state, main_thread_id, first_agent_id, second_agent_id) = populated_state();

        assert_eq!(
            state.adjacent_thread_id(Some(second_agent_id), AgentNavigationDirection::Next),
            Some(main_thread_id)
        );
        assert_eq!(
            state.adjacent_thread_id(Some(second_agent_id), AgentNavigationDirection::Previous),
            Some(first_agent_id)
        );
        assert_eq!(
            state.adjacent_thread_id(Some(main_thread_id), AgentNavigationDirection::Previous),
            Some(second_agent_id)
        );
    }

    #[test]
    fn picker_subtitle_mentions_shortcuts() {
        let previous: Span<'static> = previous_agent_shortcut().into();
        let next: Span<'static> = next_agent_shortcut().into();
        let subtitle = AgentNavigationState::picker_subtitle();

        assert!(subtitle.contains(previous.content.as_ref()));
        assert!(subtitle.contains(next.content.as_ref()));
    }

    #[test]
    fn active_agent_label_tracks_current_thread() {
        let (state, main_thread_id, first_agent_id, _) = populated_state();

        assert_eq!(
            state.active_agent_label(Some(first_agent_id), Some(main_thread_id)),
            Some("Robie [explorer]".to_string())
        );
        assert_eq!(
            state.active_agent_label(Some(main_thread_id), Some(main_thread_id)),
            Some("Main [default]".to_string())
        );
    }

    #[test]
    fn active_agent_label_prefers_thread_name_for_non_primary_agent() {
        let (mut state, main_thread_id, first_agent_id, _) = populated_state();
        state.set_thread_name(first_agent_id, Some("Backend audit".to_string()));

        assert_eq!(
            state.active_agent_label(Some(first_agent_id), Some(main_thread_id)),
            Some("Backend audit".to_string())
        );
        assert_eq!(
            state.active_agent_label(Some(main_thread_id), Some(main_thread_id)),
            Some("Main [default]".to_string())
        );
    }

    #[test]
    fn note_turn_started_marks_running_and_records_task_and_start() {
        let (mut state, _, first_agent_id, _) = populated_state();
        assert_eq!(state.metrics(&first_agent_id), None);

        state.note_turn_started(
            first_agent_id,
            Some("  audit   the   backend  ".to_string()),
        );

        let metrics = state.metrics(&first_agent_id).expect("metrics recorded");
        assert_eq!(metrics.status, CollabAgentStatus::Running);
        assert!(metrics.started_at.is_some());
        // Whitespace is collapsed so the picker row stays on a single tidy line.
        assert_eq!(
            metrics.last_task_message.as_deref(),
            Some("audit the backend")
        );
    }

    #[test]
    fn note_turn_started_keeps_previous_task_when_message_is_blank() {
        let (mut state, _, first_agent_id, _) = populated_state();
        state.note_turn_started(first_agent_id, Some("first task".to_string()));
        state.note_turn_started(first_agent_id, Some("   ".to_string()));

        assert_eq!(
            state
                .metrics(&first_agent_id)
                .and_then(|metrics| metrics.last_task_message.as_deref()),
            Some("first task")
        );
    }

    #[test]
    fn note_turn_completed_and_error_transition_status() {
        let (mut state, _, first_agent_id, second_agent_id) = populated_state();

        state.note_turn_started(first_agent_id, None);
        state.note_turn_completed(first_agent_id, CollabAgentStatus::Completed);
        assert_eq!(
            state
                .metrics(&first_agent_id)
                .map(|metrics| metrics.status.clone()),
            Some(CollabAgentStatus::Completed)
        );

        state.note_error(second_agent_id);
        assert_eq!(
            state
                .metrics(&second_agent_id)
                .map(|metrics| metrics.status.clone()),
            Some(CollabAgentStatus::Errored)
        );
    }

    #[test]
    fn note_status_and_token_usage_are_recorded() {
        let (mut state, _, first_agent_id, second_agent_id) = populated_state();

        state.note_status(first_agent_id, CollabAgentStatus::Shutdown);
        state.note_token_usage(first_agent_id, 69_742);
        // Negative totals are clamped so a bad report never renders a negative token count.
        state.note_token_usage(second_agent_id, -5);

        let metrics = state.metrics(&first_agent_id).expect("metrics recorded");
        assert_eq!(metrics.status, CollabAgentStatus::Shutdown);
        assert_eq!(metrics.token_total, 69_742);
        assert_eq!(
            state
                .metrics(&second_agent_id)
                .map(|metrics| metrics.token_total),
            Some(0)
        );
    }

    #[test]
    fn metrics_survive_metadata_upsert() {
        let (mut state, _, first_agent_id, _) = populated_state();
        state.note_turn_started(first_agent_id, Some("task".to_string()));
        state.note_token_usage(first_agent_id, 1_234);

        // A picker-liveness refresh re-`upsert`s metadata; live telemetry must not be reset.
        state.upsert(
            first_agent_id,
            Some("Robie".to_string()),
            Some("worker".to_string()),
            /*is_closed*/ true,
        );

        let metrics = state.metrics(&first_agent_id).expect("metrics preserved");
        assert_eq!(metrics.status, CollabAgentStatus::Running);
        assert_eq!(metrics.token_total, 1_234);
        assert_eq!(metrics.last_task_message.as_deref(), Some("task"));
    }

    #[test]
    fn remove_and_clear_drop_metrics() {
        let (mut state, main_thread_id, first_agent_id, _) = populated_state();
        state.note_token_usage(first_agent_id, 10);
        state.note_token_usage(main_thread_id, 20);

        state.remove(first_agent_id);
        assert_eq!(state.metrics(&first_agent_id), None);
        assert!(state.metrics(&main_thread_id).is_some());

        state.clear();
        assert_eq!(state.metrics(&main_thread_id), None);
    }
}
