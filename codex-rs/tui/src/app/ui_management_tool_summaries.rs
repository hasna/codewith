use codex_app_server_protocol::AgentEvent;
use codex_app_server_protocol::AgentEventsListResponse;
use codex_app_server_protocol::AgentExecutionSnapshot;
use codex_app_server_protocol::AgentPendingInteraction;
use codex_app_server_protocol::AgentReadResponse;
use codex_app_server_protocol::AgentRun;
use codex_app_server_protocol::AgentStatusSnapshot;
use codex_app_server_protocol::ThreadMonitor;
use codex_app_server_protocol::ThreadMonitorEvent;
use codex_app_server_protocol::ThreadMonitorReadResponse;
use codex_app_server_protocol::ThreadSchedule;
use serde_json::Value as JsonValue;
use serde_json::json;

pub(super) const COMPACT_LIST_LIMIT: usize = 20;
const COMPACT_TEXT_PREVIEW_CHARS: usize = 160;
const COMPACT_EVENT_PREVIEW_CHARS: usize = 240;

pub(super) fn compact_agent_value(agent: &AgentRun) -> JsonValue {
    json!({
        "agentId": &agent.agent_id,
        "status": &agent.status,
        "desiredState": &agent.desired_state,
        "source": &agent.source,
        "retentionState": &agent.retention_state,
        "generation": agent.generation,
        "pid": agent.pid,
        "lastEventSeq": agent.last_event_seq,
        "updatedAt": agent.updated_at,
        "threadId": agent.thread_id.as_deref(),
        "parentThreadId": agent.parent_thread_id.as_deref(),
        "statusReasonPreview": agent.status_reason.as_deref().map(|reason| {
            compact_text_preview(reason, COMPACT_TEXT_PREVIEW_CHARS)
        }),
    })
}

pub(super) fn compact_agent_read_response(
    response: &AgentReadResponse,
    limit: Option<usize>,
) -> JsonValue {
    let limit = limit.unwrap_or(COMPACT_LIST_LIMIT);
    json!({
        "agent": response.agent.as_ref().map(compact_agent_value),
        "statusSnapshot": response
            .status_snapshot
            .as_ref()
            .map(compact_agent_status_snapshot),
        "executionSnapshot": response
            .execution_snapshot
            .as_ref()
            .map(compact_agent_execution_snapshot),
        "pendingInteractionCount": response.pending_interactions.len(),
        "pendingInteractions": response
            .pending_interactions
            .iter()
            .take(limit)
            .map(compact_agent_pending_interaction)
            .collect::<Vec<_>>(),
        "hint": "Default agent read output is compact. Pass verbose=true for full snapshots and interaction payloads, or action=logs for recent event payload previews.",
    })
}

pub(super) fn compact_agent_logs_response(
    response: &AgentEventsListResponse,
    limit: Option<usize>,
) -> JsonValue {
    let total = response.data.len();
    let limit = limit.unwrap_or(COMPACT_LIST_LIMIT);
    json!({
        "count": total,
        "events": response
            .data
            .iter()
            .take(limit)
            .map(compact_agent_event)
            .collect::<Vec<_>>(),
        "nextCursor": response.next_cursor.as_deref(),
        "hint": "Default agent logs output is compact. Pass verbose=true for full event payloads, or pass limit to adjust the page size.",
    })
}

fn compact_agent_status_snapshot(snapshot: &AgentStatusSnapshot) -> JsonValue {
    json!({
        "agentId": &snapshot.agent_id,
        "seq": snapshot.seq,
        "status": &snapshot.status,
        "desiredState": &snapshot.desired_state,
        "summaryPreview": snapshot.summary.as_deref().map(|summary| {
            compact_text_preview(summary, COMPACT_TEXT_PREVIEW_CHARS)
        }),
        "pendingInteractionCount": snapshot.pending_interaction_count,
        "lastEventSeq": snapshot.last_event_seq,
        "payloadPreview": compact_json_preview(&snapshot.payload, COMPACT_EVENT_PREVIEW_CHARS),
        "updatedAt": snapshot.updated_at,
    })
}

fn compact_agent_execution_snapshot(snapshot: &AgentExecutionSnapshot) -> JsonValue {
    json!({
        "snapshotId": &snapshot.snapshot_id,
        "agentId": &snapshot.agent_id,
        "seq": snapshot.seq,
        "snapshotKind": &snapshot.snapshot_kind,
        "recoveryPolicy": &snapshot.recovery_policy,
        "payloadPreview": compact_json_preview(&snapshot.payload, COMPACT_EVENT_PREVIEW_CHARS),
        "createdAt": snapshot.created_at,
    })
}

fn compact_agent_pending_interaction(interaction: &AgentPendingInteraction) -> JsonValue {
    json!({
        "interactionId": &interaction.interaction_id,
        "agentId": &interaction.agent_id,
        "kind": &interaction.kind,
        "status": &interaction.status,
        "requestPayloadPreview": compact_json_preview(
            &interaction.request_payload,
            COMPACT_EVENT_PREVIEW_CHARS,
        ),
        "responsePayloadPreview": interaction.response_payload.as_ref().map(|payload| {
            compact_json_preview(payload, COMPACT_EVENT_PREVIEW_CHARS)
        }),
        "timeoutAt": interaction.timeout_at,
        "createdAt": interaction.created_at,
        "updatedAt": interaction.updated_at,
    })
}

fn compact_agent_event(event: &AgentEvent) -> JsonValue {
    json!({
        "eventId": &event.event_id,
        "agentId": &event.agent_id,
        "seq": event.seq,
        "eventType": &event.event_type,
        "payloadPreview": compact_json_preview(&event.payload, COMPACT_EVENT_PREVIEW_CHARS),
        "createdAt": event.created_at,
    })
}

pub(super) fn compact_schedule_value(schedule: &ThreadSchedule) -> JsonValue {
    json!({
        "scheduleId": &schedule.schedule_id,
        "status": &schedule.status,
        "schedule": &schedule.schedule,
        "timezone": &schedule.timezone,
        "nextRunAt": schedule.next_run_at,
        "lastRunAt": schedule.last_run_at,
        "expiresAt": schedule.expires_at,
        "failureCount": schedule.failure_count,
        "promptPreview": compact_text_preview(&schedule.prompt, COMPACT_TEXT_PREVIEW_CHARS),
        "promptChars": schedule.prompt.chars().count(),
    })
}

pub(super) fn compact_monitor_read_response(
    response: &ThreadMonitorReadResponse,
    limit: Option<usize>,
) -> JsonValue {
    let total = response.events.len();
    let limit = limit.unwrap_or(COMPACT_LIST_LIMIT);
    json!({
        "monitor": response.monitor.as_ref().map(compact_monitor_value),
        "eventCount": total,
        "events": response
            .events
            .iter()
            .take(limit)
            .map(compact_monitor_event_value)
            .collect::<Vec<_>>(),
        "nextCursor": response.next_cursor.as_deref(),
        "hint": "Default monitor read output is compact. Pass verbose=true for full command text, paths, errors, and event text.",
    })
}

pub(super) fn compact_monitor_value(monitor: &ThreadMonitor) -> JsonValue {
    json!({
        "monitorId": &monitor.monitor_id,
        "name": &monitor.name,
        "status": &monitor.status,
        "routing": &monitor.routing,
        "generation": monitor.generation,
        "processId": monitor.process_id,
        "lastEventAt": monitor.last_event_at,
        "promptPreview": compact_text_preview(&monitor.prompt, COMPACT_TEXT_PREVIEW_CHARS),
        "promptChars": monitor.prompt.chars().count(),
        "commandPreview": compact_text_preview(&monitor.command, COMPACT_TEXT_PREVIEW_CHARS),
        "commandChars": monitor.command.chars().count(),
        "cwdConfigured": monitor.cwd.is_some(),
        "outputFileConfigured": monitor.output_file.is_some(),
        "lastErrorPreview": monitor.last_error.as_deref().map(|error| {
            compact_text_preview(error, COMPACT_TEXT_PREVIEW_CHARS)
        }),
    })
}

fn compact_monitor_event_value(event: &ThreadMonitorEvent) -> JsonValue {
    json!({
        "monitorId": &event.monitor_id,
        "eventId": &event.event_id,
        "stream": &event.stream,
        "createdAt": event.created_at,
        "textPreview": compact_text_preview(&event.text, COMPACT_EVENT_PREVIEW_CHARS),
        "textChars": event.text.chars().count(),
    })
}

fn compact_json_preview(value: &JsonValue, max_chars: usize) -> String {
    compact_text_preview(&value.to_string(), max_chars)
}

fn compact_text_preview(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_end(&normalized, max_chars)
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::AgentDesiredState;
    use codex_app_server_protocol::AgentEvent;
    use codex_app_server_protocol::AgentEventsListResponse;
    use codex_app_server_protocol::AgentPendingInteraction;
    use codex_app_server_protocol::AgentPendingInteractionKind;
    use codex_app_server_protocol::AgentPendingInteractionStatus;
    use codex_app_server_protocol::AgentRetentionState;
    use codex_app_server_protocol::AgentRunStatus;
    use codex_app_server_protocol::ThreadMonitorEventStream;
    use codex_app_server_protocol::ThreadMonitorReadResponse;
    use pretty_assertions::assert_eq;

    #[test]
    fn compact_agent_logs_truncate_payloads_and_honor_limit() {
        let response = AgentEventsListResponse {
            data: vec![
                AgentEvent {
                    event_id: "event-1".to_string(),
                    agent_id: "agent-1".to_string(),
                    seq: 1,
                    event_type: "agent.message".to_string(),
                    payload: json!({ "message": "x".repeat(400) }),
                    created_at: 10,
                },
                AgentEvent {
                    event_id: "event-2".to_string(),
                    agent_id: "agent-1".to_string(),
                    seq: 2,
                    event_type: "agent.message".to_string(),
                    payload: json!({ "message": "second" }),
                    created_at: 11,
                },
            ],
            next_cursor: Some("cursor-2".to_string()),
        };

        let compact = compact_agent_logs_response(&response, Some(1));

        assert_eq!(compact["count"], 2);
        assert_eq!(compact["events"].as_array().expect("events").len(), 1);
        assert_eq!(compact["events"][0]["eventType"], "agent.message");
        assert!(
            compact["events"][0]["payloadPreview"]
                .as_str()
                .expect("payload preview")
                .ends_with("...")
        );
    }

    #[test]
    fn compact_agent_read_honors_pending_interaction_limit() {
        let response = AgentReadResponse {
            agent: None,
            status_snapshot: None,
            execution_snapshot: None,
            pending_interactions: vec![
                AgentPendingInteraction {
                    interaction_id: "interaction-1".to_string(),
                    agent_id: "agent-1".to_string(),
                    worker_request_id: None,
                    kind: AgentPendingInteractionKind::UserInput,
                    status: AgentPendingInteractionStatus::Pending,
                    request_payload: json!({ "question": "one" }),
                    response_payload: None,
                    no_client_policy: "persist".to_string(),
                    timeout_at: None,
                    created_at: 1,
                    delivered_at: None,
                    responded_at: None,
                    updated_at: 1,
                },
                AgentPendingInteraction {
                    interaction_id: "interaction-2".to_string(),
                    agent_id: "agent-1".to_string(),
                    worker_request_id: None,
                    kind: AgentPendingInteractionKind::Approval,
                    status: AgentPendingInteractionStatus::Delivered,
                    request_payload: json!({ "command": "two" }),
                    response_payload: None,
                    no_client_policy: "persist".to_string(),
                    timeout_at: None,
                    created_at: 2,
                    delivered_at: Some(2),
                    responded_at: None,
                    updated_at: 2,
                },
            ],
        };

        let compact = compact_agent_read_response(&response, Some(1));

        assert_eq!(compact["pendingInteractionCount"], 2);
        assert_eq!(
            compact["pendingInteractions"]
                .as_array()
                .expect("pending interactions")
                .len(),
            1
        );
        assert_eq!(
            compact["pendingInteractions"][0]["interactionId"],
            "interaction-1"
        );
    }

    #[test]
    fn compact_agent_run_exposes_metadata_not_snapshots() {
        let run = AgentRun {
            agent_id: "agent-1".to_string(),
            idempotency_key: None,
            request_id: None,
            source: "cli".to_string(),
            prompt_snapshot_ref: "inline:agent-1:prompt".to_string(),
            input_snapshot_ref: None,
            thread_id: Some("thread-1".to_string()),
            thread_store_kind: "background-agent".to_string(),
            thread_store_id: None,
            rollout_path: None,
            parent_thread_id: Some("parent-thread".to_string()),
            parent_agent_run_id: None,
            spawn_linkage: None,
            worktree_lease_id: None,
            auth_profile_ref: None,
            desired_state: AgentDesiredState::Running,
            status: AgentRunStatus::Running,
            status_reason: Some("working on a very long request".to_string()),
            config_fingerprint: None,
            version_fingerprint: None,
            retention_state: AgentRetentionState::Active,
            archive_after: None,
            delete_after: None,
            archived_at: None,
            deleted_at: None,
            supervisor_id: None,
            generation: 1,
            pid: Some(123),
            pgid: None,
            job_id: None,
            heartbeat_at: Some(20),
            crash_reason: None,
            exit_code: None,
            exit_signal: None,
            last_event_seq: 7,
            last_snapshot_seq: 6,
            created_at: 1,
            updated_at: 2,
            started_at: Some(1),
            completed_at: None,
        };

        let compact = compact_agent_value(&run);

        assert_eq!(compact["agentId"], "agent-1");
        assert_eq!(compact["status"], "running");
        assert_eq!(compact["lastEventSeq"], 7);
        assert!(compact["promptSnapshotRef"].is_null());
    }

    #[test]
    fn compact_monitor_read_truncates_event_text() {
        let response = ThreadMonitorReadResponse {
            monitor: None,
            events: vec![codex_app_server_protocol::ThreadMonitorEvent {
                thread_id: "thread-1".to_string(),
                monitor_id: "monitor-1".to_string(),
                event_id: "event-1".to_string(),
                stream: ThreadMonitorEventStream::Stdout,
                text: "line ".repeat(100),
                created_at: 10,
            }],
            next_cursor: None,
        };

        let compact = compact_monitor_read_response(&response, None);

        assert_eq!(compact["eventCount"], 1);
        assert!(
            compact["events"][0]["textPreview"]
                .as_str()
                .expect("event preview")
                .ends_with("...")
        );
        assert_eq!(compact["events"][0]["textChars"], 500);
    }
}
