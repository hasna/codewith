//! Aggregated live status for collaborator wait tool calls.

use super::*;
use std::collections::BTreeMap;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const WAIT_ACTIVITY_MAX_GRAPHEMES: usize = 80;
const WAIT_STATUS_MAX_AGENTS: usize = 3;

#[derive(Clone, Debug)]
struct WaitTarget {
    raw_thread_id: String,
    thread_id: Option<ThreadId>,
}

#[derive(Debug)]
struct ActiveWait {
    started_at: Instant,
    targets: Vec<WaitTarget>,
}

#[derive(Debug, Default)]
pub(super) struct CollabWaitStatus {
    active_waits: BTreeMap<String, ActiveWait>,
    agent_activities: HashMap<ThreadId, String>,
    previous_status: Option<StatusIndicatorState>,
}

impl CollabWaitStatus {
    pub(super) fn record_agent_activity(&mut self, thread_id: ThreadId, activity: &str) {
        let activity = compact_activity(activity);
        if activity.is_empty() {
            self.agent_activities.remove(&thread_id);
        } else {
            self.agent_activities.insert(thread_id, activity);
        }
    }

    pub(super) fn begin_wait(
        &mut self,
        call_id: String,
        receiver_thread_ids: &[String],
        started_at: Instant,
        previous_status: StatusIndicatorState,
    ) {
        if self.active_waits.is_empty() {
            self.previous_status = Some(previous_status);
        }
        let targets = receiver_thread_ids
            .iter()
            .filter_map(|raw_thread_id| {
                let raw_thread_id = raw_thread_id.trim();
                (!raw_thread_id.is_empty()).then(|| WaitTarget {
                    raw_thread_id: raw_thread_id.to_string(),
                    thread_id: ThreadId::from_string(raw_thread_id).ok(),
                })
            })
            .collect();
        self.active_waits.insert(
            call_id,
            ActiveWait {
                started_at,
                targets,
            },
        );
    }

    pub(super) fn record_base_status(&mut self, status: StatusIndicatorState) {
        if !self.active_waits.is_empty() {
            self.previous_status = Some(status);
        }
    }

    pub(super) fn finish_wait(&mut self, call_id: &str) -> (bool, Option<StatusIndicatorState>) {
        if self.active_waits.remove(call_id).is_none() {
            return (false, None);
        }
        if self.active_waits.is_empty() {
            return (true, self.previous_status.take());
        }
        (false, None)
    }

    pub(super) fn clear_active_waits(&mut self) {
        self.active_waits.clear();
        self.previous_status = None;
    }

    pub(super) fn has_active_waits(&self) -> bool {
        !self.active_waits.is_empty()
    }

    pub(super) fn status_indicator_state_at(
        &self,
        now: Instant,
        agent_metadata: &HashMap<ThreadId, AgentMetadata>,
    ) -> Option<StatusIndicatorState> {
        if self.active_waits.is_empty() {
            return None;
        }

        let mut agents = Vec::new();
        let mut agent_indices = HashMap::new();
        for wait in self.active_waits.values() {
            for target in &wait.targets {
                let elapsed = now.saturating_duration_since(wait.started_at);
                if let Some(index) = agent_indices.get(&target.raw_thread_id).copied() {
                    let (_, _, existing_elapsed): &mut (String, String, Duration) =
                        &mut agents[index];
                    *existing_elapsed = (*existing_elapsed).max(elapsed);
                    continue;
                }
                let metadata = target
                    .thread_id
                    .and_then(|thread_id| agent_metadata.get(&thread_id));
                let name = metadata
                    .and_then(|metadata| metadata.agent_nickname.as_deref())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&target.raw_thread_id)
                    .to_string();
                let activity = target
                    .thread_id
                    .and_then(|thread_id| self.agent_activities.get(&thread_id))
                    .cloned()
                    .or_else(|| {
                        metadata
                            .and_then(|metadata| metadata.agent_role.as_deref())
                            .map(compact_activity)
                            .filter(|role| !role.is_empty())
                    })
                    .unwrap_or_else(|| "Working".to_string());
                agent_indices.insert(target.raw_thread_id.clone(), agents.len());
                agents.push((name, activity, elapsed));
            }
        }

        let header = match agents.as_slice() {
            [(name, _, _)] => format!("Waiting for {name}"),
            [] => "Waiting for agents".to_string(),
            _ => format!("Waiting for {} agents", agents.len()),
        };
        let (details, details_max_lines) = match agents.as_slice() {
            [(_, activity, elapsed)] => (
                Some(format!(
                    "{activity} · waiting {}",
                    crate::status_indicator_widget::fmt_elapsed_compact(elapsed.as_secs())
                )),
                1,
            ),
            [] => {
                let elapsed = self
                    .active_waits
                    .values()
                    .map(|wait| now.saturating_duration_since(wait.started_at))
                    .max()
                    .unwrap_or_default();
                (
                    Some(format!(
                        "waiting {}",
                        crate::status_indicator_widget::fmt_elapsed_compact(elapsed.as_secs())
                    )),
                    1,
                )
            }
            _ => {
                let mut lines = agents
                    .iter()
                    .take(WAIT_STATUS_MAX_AGENTS)
                    .map(|(name, activity, elapsed)| {
                        format!(
                            "• {name} — {activity} · {}",
                            crate::status_indicator_widget::fmt_elapsed_compact(elapsed.as_secs())
                        )
                    })
                    .collect::<Vec<_>>();
                let remaining = agents.len().saturating_sub(WAIT_STATUS_MAX_AGENTS);
                if remaining > 0 {
                    lines.push(format!("+{remaining} more"));
                }
                (Some(lines.join("\n")), WAIT_STATUS_MAX_AGENTS + 1)
            }
        };

        Some(StatusIndicatorState {
            header,
            details,
            details_max_lines,
        })
    }
}

fn compact_activity(activity: &str) -> String {
    let activity = activity
        .chars()
        .filter(|ch| !is_unsafe_status_char(*ch))
        .collect::<String>();
    let activity = activity.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::text_formatting::truncate_text(&activity, WAIT_ACTIVITY_MAX_GRAPHEMES)
}

fn is_unsafe_status_char(ch: char) -> bool {
    (ch.is_control() && !ch.is_whitespace())
        || matches!(
            ch,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

fn started_at_from_unix_ms(
    started_at_ms: Option<i64>,
    received_at: Instant,
    received_at_unix_ms: Option<i64>,
) -> Instant {
    let Some(elapsed_ms) = started_at_ms
        .filter(|started_at_ms| *started_at_ms > 0)
        .zip(received_at_unix_ms)
        .and_then(|(started_at_ms, received_at_unix_ms)| {
            received_at_unix_ms
                .checked_sub(started_at_ms)
                .filter(|elapsed_ms| *elapsed_ms >= 0)
        })
        .and_then(|elapsed_ms| u64::try_from(elapsed_ms).ok())
    else {
        return received_at;
    };
    received_at
        .checked_sub(Duration::from_millis(elapsed_ms))
        .unwrap_or(received_at)
}

fn current_unix_time_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

impl ChatWidget {
    pub(super) fn record_collab_agent_activity(
        &mut self,
        receiver_thread_ids: &[String],
        activity: &str,
    ) {
        for thread_id in receiver_thread_ids {
            if let Ok(thread_id) = ThreadId::from_string(thread_id) {
                self.collab_wait_status
                    .record_agent_activity(thread_id, activity);
            }
        }
        self.refresh_collab_wait_status_at(Instant::now());
    }

    pub(super) fn begin_collab_wait(
        &mut self,
        call_id: String,
        receiver_thread_ids: &[String],
        started_at_ms: Option<i64>,
    ) {
        let received_at = Instant::now();
        self.collab_wait_status.begin_wait(
            call_id,
            receiver_thread_ids,
            started_at_from_unix_ms(started_at_ms, received_at, current_unix_time_ms()),
            self.status_state.current_status.clone(),
        );
        self.refresh_collab_wait_status_at(received_at);
    }

    pub(super) fn finish_collab_wait(&mut self, call_id: &str) {
        let (finished_last_wait, previous_status) = self.collab_wait_status.finish_wait(call_id);
        if !finished_last_wait {
            self.refresh_collab_wait_status_at(Instant::now());
            return;
        }
        if let Some(previous_status) = previous_status
            && self.bottom_pane.is_task_running()
        {
            self.set_status(
                previous_status.header,
                previous_status.details,
                StatusDetailsCapitalization::Preserve,
                previous_status.details_max_lines,
            );
        }
    }

    pub(super) fn refresh_collab_wait_status_at(&mut self, now: Instant) {
        let Some(status) = self
            .collab_wait_status
            .status_indicator_state_at(now, &self.collab_agent_metadata)
        else {
            return;
        };
        if !self.bottom_pane.is_task_running() {
            return;
        }
        self.bottom_pane.ensure_status_indicator();
        self.set_collab_wait_status(status);
        self.frame_requester
            .schedule_frame_in(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn thread_id(value: &str) -> ThreadId {
        ThreadId::from_string(value).expect("valid thread id")
    }

    fn status_snapshot(status: StatusIndicatorState) -> String {
        format!(
            "{}\n{}",
            status.header,
            status.details.unwrap_or_else(|| "<no details>".to_string())
        )
    }

    #[test]
    fn named_wait_status_bounds_activity_and_shows_wait_elapsed() {
        let now = Instant::now();
        let robie = thread_id("019cff70-2599-75e2-af72-b958ce5dc1cc");
        let mut waits = CollabWaitStatus::default();
        waits.record_agent_activity(
            robie,
            "  Inspect   the waiting-agent rendering path and describe every possible regression in exhaustive detail before reporting back  ",
        );
        waits.begin_wait(
            "wait-1".to_string(),
            &[robie.to_string()],
            now - Duration::from_secs(65),
            StatusIndicatorState::working(),
        );
        let metadata = HashMap::from([(
            robie,
            AgentMetadata {
                agent_nickname: Some("Robie".to_string()),
                agent_role: Some("explorer".to_string()),
            },
        )]);

        assert_snapshot!(
            "named_wait_status",
            status_snapshot(
                waits
                    .status_indicator_state_at(now, &metadata)
                    .expect("active status")
            )
        );
    }

    #[test]
    fn multiple_wait_status_aggregates_and_uses_uuid_fallback() {
        let now = Instant::now();
        let robie = thread_id("019cff70-2599-75e2-af72-b958ce5dc1cc");
        let unnamed = thread_id("019cff70-2599-75e2-af72-b96db334332d");
        let mut waits = CollabWaitStatus::default();
        waits.record_agent_activity(robie, "Inspect rendering");
        waits.begin_wait(
            "z-older-wait".to_string(),
            &[robie.to_string()],
            now - Duration::from_secs(9),
            StatusIndicatorState::working(),
        );
        waits.begin_wait(
            "a-newer-wait".to_string(),
            &[robie.to_string(), unnamed.to_string()],
            now - Duration::from_secs(4),
            StatusIndicatorState::working(),
        );
        let metadata = HashMap::from([(
            robie,
            AgentMetadata {
                agent_nickname: Some("Robie".to_string()),
                agent_role: Some("explorer".to_string()),
            },
        )]);

        assert_snapshot!(
            "multiple_wait_status",
            status_snapshot(
                waits
                    .status_indicator_state_at(now, &metadata)
                    .expect("active status")
            )
        );
    }

    #[test]
    fn concurrent_waits_finish_independently_and_restore_latest_base_status() {
        let now = Instant::now();
        let robie = thread_id("019cff70-2599-75e2-af72-b958ce5dc1cc");
        let ada = thread_id("019cff70-2599-75e2-af72-b96db334332d");
        let mut waits = CollabWaitStatus::default();
        let original_status = StatusIndicatorState::working();
        let latest_status = StatusIndicatorState {
            header: "Waiting for approval".to_string(),
            details: Some("Reviewing command".to_string()),
            details_max_lines: 1,
        };
        waits.begin_wait(
            "wait-1".to_string(),
            &[robie.to_string()],
            now,
            original_status,
        );
        waits.begin_wait(
            "wait-2".to_string(),
            &[ada.to_string()],
            now,
            StatusIndicatorState::working(),
        );
        waits.record_base_status(latest_status.clone());

        assert_eq!(waits.finish_wait("wait-1"), (false, None));
        assert!(waits.has_active_waits());
        assert_eq!(waits.finish_wait("wait-2"), (true, Some(latest_status)));
    }

    #[test]
    fn activity_strips_terminal_and_bidi_controls() {
        assert_eq!(
            compact_activity(
                "Inspect\u{1b}[31m unsafe\u{061c}\u{200e}\u{200f}\u{202e}\u{2066} status"
            ),
            "Inspect[31m unsafe status"
        );
    }

    #[test]
    fn role_fallback_is_sanitized_and_bounded() {
        let now = Instant::now();
        let robie = thread_id("019cff70-2599-75e2-af72-b958ce5dc1cc");
        let unsafe_role = format!(
            "\u{061c}\u{200e}\u{200f}\u{202e}\u{2066}\u{1b}{}",
            "reviewing detailed activity ".repeat(8)
        );
        let mut waits = CollabWaitStatus::default();
        waits.begin_wait(
            "wait-1".to_string(),
            &[robie.to_string()],
            now,
            StatusIndicatorState::working(),
        );
        let metadata = HashMap::from([(
            robie,
            AgentMetadata {
                agent_nickname: Some("Robie".to_string()),
                agent_role: Some(unsafe_role.clone()),
            },
        )]);

        assert_eq!(
            waits
                .status_indicator_state_at(now, &metadata)
                .expect("active status")
                .details,
            Some(format!("{} · waiting 0s", compact_activity(&unsafe_role)))
        );
    }

    #[test]
    fn targetless_wait_still_shows_elapsed_time() {
        let now = Instant::now();
        let mut waits = CollabWaitStatus::default();
        waits.begin_wait(
            "wait-1".to_string(),
            &[],
            now - Duration::from_secs(9),
            StatusIndicatorState::working(),
        );

        assert_eq!(
            waits
                .status_indicator_state_at(now, &HashMap::new())
                .expect("active status"),
            StatusIndicatorState {
                header: "Waiting for agents".to_string(),
                details: Some("waiting 9s".to_string()),
                details_max_lines: 1,
            }
        );
    }

    #[test]
    fn event_timestamp_preserves_elapsed_wait() {
        let received_at = Instant::now();
        assert_eq!(
            received_at.saturating_duration_since(started_at_from_unix_ms(
                Some(935_000),
                received_at,
                Some(1_000_000),
            )),
            Duration::from_secs(65)
        );
        assert_eq!(
            started_at_from_unix_ms(Some(1_000_001), received_at, Some(1_000_000)),
            received_at
        );
    }
}
