//! Automatic session recap scheduling for the TUI app.

use super::*;
use std::collections::HashSet;

const MIN_COMPLETED_TURNS_FOR_RECAP: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionRecapKey {
    pub(super) thread_id: ThreadId,
    pub(super) turn_id: String,
}

#[derive(Debug)]
pub(super) struct SessionRecapScheduler {
    terminal_focused: bool,
    latest_completed_key: Option<SessionRecapKey>,
    pending_key: Option<SessionRecapKey>,
    pending_timer: Option<JoinHandle<()>>,
    in_flight_key: Option<SessionRecapKey>,
    last_attempted_key: Option<SessionRecapKey>,
    last_recapped_key: Option<SessionRecapKey>,
}

impl Default for SessionRecapScheduler {
    fn default() -> Self {
        Self {
            terminal_focused: true,
            latest_completed_key: None,
            pending_key: None,
            pending_timer: None,
            in_flight_key: None,
            last_attempted_key: None,
            last_recapped_key: None,
        }
    }
}

impl SessionRecapScheduler {
    fn set_terminal_focused(&mut self, focused: bool) {
        self.terminal_focused = focused;
        if focused {
            self.cancel_pending_timer();
        }
    }

    fn note_completed_turn(&mut self, key: SessionRecapKey) {
        if self.latest_completed_key.as_ref() == Some(&key) {
            return;
        }
        self.latest_completed_key = Some(key);
    }

    fn schedule_if_needed(
        &mut self,
        config: &crate::legacy_core::config::SessionRecapConfig,
        app_event_tx: AppEventSender,
    ) {
        if self.terminal_focused || !config.enabled {
            return;
        }

        let Some(key) = self.latest_completed_key.clone() else {
            return;
        };
        if self.pending_key.as_ref() == Some(&key)
            || self.in_flight_key.as_ref() == Some(&key)
            || self.last_attempted_key.as_ref() == Some(&key)
            || self.last_recapped_key.as_ref() == Some(&key)
        {
            return;
        }

        self.cancel_pending_timer();
        self.pending_key = Some(key.clone());
        let delay = Duration::from_secs(config.idle_minutes.saturating_mul(60));
        self.pending_timer = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            app_event_tx.send(AppEvent::MaybeStartAutomaticSessionRecap {
                thread_id: key.thread_id,
                turn_id: key.turn_id,
            });
        }));
    }

    fn mark_automatic_started(
        &mut self,
        config: &crate::legacy_core::config::SessionRecapConfig,
        key: SessionRecapKey,
        current_thread_id: Option<ThreadId>,
        active_turn_id: Option<&str>,
        completed_turn_count: usize,
    ) -> bool {
        self.pending_key = None;
        self.pending_timer = None;

        if self.terminal_focused
            || !config.enabled
            || current_thread_id != Some(key.thread_id)
            || active_turn_id.is_some()
            || completed_turn_count < MIN_COMPLETED_TURNS_FOR_RECAP
            || self.latest_completed_key.as_ref() != Some(&key)
            || self.in_flight_key.as_ref() == Some(&key)
            || self.last_attempted_key.as_ref() == Some(&key)
            || self.last_recapped_key.as_ref() == Some(&key)
        {
            return false;
        }

        self.last_attempted_key = Some(key.clone());
        self.in_flight_key = Some(key);
        true
    }

    fn mark_finished(&mut self, thread_id: ThreadId, automatic: bool, succeeded: bool) {
        if !automatic {
            if succeeded
                && let Some(latest_key) = self.latest_completed_key.clone()
                && latest_key.thread_id == thread_id
            {
                self.last_recapped_key = Some(latest_key);
            }
            return;
        }

        let Some(key) = self.in_flight_key.take() else {
            return;
        };
        if succeeded {
            self.last_recapped_key = Some(key);
        }
    }

    fn cancel_pending_timer(&mut self) {
        if let Some(handle) = self.pending_timer.take() {
            handle.abort();
        }
        self.pending_key = None;
    }
}

impl App {
    pub(super) fn handle_terminal_focus_lost(&mut self) {
        self.session_recap.set_terminal_focused(/*focused*/ false);
        self.schedule_automatic_session_recap();
    }

    pub(super) fn handle_terminal_focus_gained(&mut self) {
        self.session_recap.set_terminal_focused(/*focused*/ true);
    }

    pub(super) fn note_session_recap_turn_completed(&mut self, thread_id: ThreadId, turn: &Turn) {
        if !matches!(turn.status, TurnStatus::Completed) {
            return;
        }
        self.session_recap.note_completed_turn(SessionRecapKey {
            thread_id,
            turn_id: turn.id.clone(),
        });
        self.schedule_automatic_session_recap();
    }

    pub(super) async fn maybe_start_automatic_session_recap(
        &mut self,
        app_server: &AppServerSession,
        thread_id: ThreadId,
        turn_id: String,
    ) {
        let key = SessionRecapKey { thread_id, turn_id };
        let active_turn_id = self.active_turn_id_for_thread(thread_id).await;
        let completed_turn_count = self.completed_turn_count_for_thread(thread_id).await;
        if self.session_recap.mark_automatic_started(
            &self.config.session_recap,
            key,
            self.current_displayed_thread_id(),
            active_turn_id.as_deref(),
            completed_turn_count,
        ) {
            self.request_session_recap(
                app_server, thread_id, /*prompt*/ None, /*automatic*/ true,
            );
        }
    }

    pub(super) fn mark_session_recap_finished(
        &mut self,
        thread_id: ThreadId,
        automatic: bool,
        succeeded: bool,
    ) {
        self.session_recap
            .mark_finished(thread_id, automatic, succeeded);
    }

    fn schedule_automatic_session_recap(&mut self) {
        self.session_recap
            .schedule_if_needed(&self.config.session_recap, self.app_event_tx.clone());
    }

    async fn completed_turn_count_for_thread(&self, thread_id: ThreadId) -> usize {
        let Some(channel) = self.thread_event_channels.get(&thread_id) else {
            return 0;
        };
        let store = channel.store.lock().await;
        let mut completed_turn_ids = HashSet::new();
        for turn in &store.turns {
            if matches!(turn.status, TurnStatus::Completed) {
                completed_turn_ids.insert(turn.id.clone());
            }
        }
        for event in &store.buffer {
            if let ThreadBufferedEvent::Notification(ServerNotification::TurnCompleted(
                notification,
            )) = event
                && matches!(notification.turn.status, TurnStatus::Completed)
            {
                completed_turn_ids.insert(notification.turn.id.clone());
            }
        }
        completed_turn_ids.len()
    }
}
