use serde_json::Value;
use serde_json::json;

const MAX_MODEL_RUN_DETAILS: usize = 20;

pub(crate) fn workflow_json(workflow: codex_state::WorkflowSpecRecord) -> Value {
    json!({
        "threadId": workflow.source_thread_id.map(|thread_id| thread_id.to_string()),
        "workflowRecordId": workflow.workflow_record_id,
        "specWorkflowId": workflow.spec_workflow_id,
        "schemaVersion": workflow.schema_version,
        "displayName": workflow.display_name,
        "status": workflow.status.as_str(),
        "sourceYamlSha256": workflow.source_yaml_sha256,
        "agentCount": workflow.agent_count,
        "stepCount": workflow.step_count,
        "parallelGroupCount": workflow.parallel_group_count,
        "verifierCount": workflow.verifier_count,
        "runCommandVerifierCount": workflow.run_command_verifier_count,
        "modelRoutedStepCount": workflow.model_routed_step_count,
        "createdAt": workflow.created_at.timestamp(),
        "updatedAt": workflow.updated_at.timestamp(),
    })
}

pub(crate) fn run_summary_json(snapshot: &codex_state::WorkflowRunSnapshot) -> Value {
    let count_steps = |status| {
        snapshot
            .steps
            .iter()
            .filter(|step| step.status == status)
            .count() as i64
    };
    json!({
        "threadId": snapshot.run.source_thread_id.map(|thread_id| thread_id.to_string()),
        "runId": snapshot.run.run_id,
        "workflowRecordId": snapshot.run.workflow_record_id,
        "specWorkflowId": snapshot.run.spec_workflow_id,
        "schemaVersion": snapshot.run.schema_version,
        "sourceYamlSha256": snapshot.run.source_yaml_sha256,
        "status": snapshot.run.status.as_str(),
        "statusReason": snapshot.run.status_reason,
        "reasonCode": snapshot.run.reason_code,
        "generation": snapshot.run.generation,
        "pendingStepCount": count_steps(codex_state::WorkflowRunStepStatus::Pending),
        "readyStepCount": count_steps(codex_state::WorkflowRunStepStatus::Ready),
        "activeStepCount": count_steps(codex_state::WorkflowRunStepStatus::Active),
        "waitingVerifierStepCount": count_steps(codex_state::WorkflowRunStepStatus::WaitingVerifier),
        "blockedStepCount": count_steps(codex_state::WorkflowRunStepStatus::Blocked),
        "failedStepCount": count_steps(codex_state::WorkflowRunStepStatus::Failed),
        "succeededStepCount": count_steps(codex_state::WorkflowRunStepStatus::Succeeded),
        "skippedStepCount": count_steps(codex_state::WorkflowRunStepStatus::Skipped),
        "verifierCount": snapshot.verifiers.len() as i64,
        "eventCount": snapshot.events.len() as i64,
        "createdAt": snapshot.run.created_at.timestamp(),
        "updatedAt": snapshot.run.updated_at.timestamp(),
        "startedAt": snapshot.run.started_at.map(|timestamp| timestamp.timestamp()),
        "completedAt": snapshot.run.completed_at.map(|timestamp| timestamp.timestamp()),
    })
}

pub(crate) fn run_snapshot_json(snapshot: &codex_state::WorkflowRunSnapshot) -> Value {
    let steps = snapshot
        .steps
        .iter()
        .take(MAX_MODEL_RUN_DETAILS)
        .map(step_json)
        .collect::<Vec<_>>();
    let verifiers = snapshot
        .verifiers
        .iter()
        .take(MAX_MODEL_RUN_DETAILS)
        .map(verifier_json)
        .collect::<Vec<_>>();
    let event_count = snapshot.events.len();
    let events = snapshot
        .events
        .iter()
        .skip(event_count.saturating_sub(MAX_MODEL_RUN_DETAILS))
        .map(event_json)
        .collect::<Vec<_>>();

    json!({
        "run": run_summary_json(snapshot),
        "steps": steps,
        "omittedStepCount": snapshot.steps.len().saturating_sub(MAX_MODEL_RUN_DETAILS),
        "verifiers": verifiers,
        "omittedVerifierCount": snapshot.verifiers.len().saturating_sub(MAX_MODEL_RUN_DETAILS),
        "recentEvents": events,
        "omittedEventCount": event_count.saturating_sub(MAX_MODEL_RUN_DETAILS),
    })
}

pub(crate) fn goal_plan_projection_json(
    outcome: codex_state::WorkflowGoalPlanProjectionOutcome,
) -> Value {
    let usage = outcome.snapshot.usage_summary();
    json!({
        "projectionId": outcome.projection_id,
        "runId": outcome.run_id,
        "threadId": outcome.thread_id.to_string(),
        "planId": outcome.plan_id,
        "idempotencyKey": outcome.idempotency_key,
        "created": outcome.created,
        "status": outcome.snapshot.plan.status.as_str(),
        "nodeCount": usage.node_count,
        "readyNodeCount": usage.ready_node_count,
        "activeNodeCount": usage.active_node_count,
        "pendingNodeCount": usage.pending_node_count,
        "pausedNodeCount": usage.paused_node_count,
        "blockedNodeCount": usage.blocked_node_count,
        "completedNodeCount": usage.completed_node_count,
    })
}

fn step_json(step: &codex_state::WorkflowRunStep) -> Value {
    json!({
        "stepRunId": step.step_run_id,
        "stepId": step.step_id,
        "sequence": step.sequence,
        "title": truncate_text(step.title.as_str()),
        "agentId": step.agent_id,
        "status": step.status.as_str(),
        "statusReason": step.status_reason.as_deref().map(truncate_text),
        "reasonCode": step.reason_code,
        "approvalGate": step.approval_gate,
        "approvalState": step.approval_state,
        "dependsOn": step.depends_on,
        "backgroundAgentRunId": step.background_agent_run_id,
        "createdAt": step.created_at.timestamp(),
        "updatedAt": step.updated_at.timestamp(),
        "startedAt": step.started_at.map(|timestamp| timestamp.timestamp()),
        "completedAt": step.completed_at.map(|timestamp| timestamp.timestamp()),
    })
}

fn verifier_json(verifier: &codex_state::WorkflowRunStepVerifier) -> Value {
    json!({
        "verifierRunId": verifier.verifier_run_id,
        "stepId": verifier.step_id,
        "verifierId": verifier.verifier_id,
        "verifierType": verifier.verifier_type,
        "status": verifier.status.as_str(),
        "statusReason": verifier.status_reason.as_deref().map(truncate_text),
        "reasonCode": verifier.reason_code,
        "attemptCount": verifier.attempt_count,
        "maxAttempts": verifier.max_attempts,
        "createdAt": verifier.created_at.timestamp(),
        "updatedAt": verifier.updated_at.timestamp(),
        "completedAt": verifier.completed_at.map(|timestamp| timestamp.timestamp()),
    })
}

fn event_json(event: &codex_state::WorkflowRunEvent) -> Value {
    json!({
        "seq": event.seq,
        "eventType": event.event_type,
        "actorKind": event.actor_kind,
        "actorId": event.actor_id,
        "stepRunId": event.step_run_id,
        "verifierRunId": event.verifier_run_id,
        "visibility": event.visibility,
        "createdAt": event.created_at.timestamp(),
    })
}

fn truncate_text(value: &str) -> String {
    const MAX_CHARS: usize = 240;
    if value.chars().count() <= MAX_CHARS {
        return value.to_string();
    }
    value.chars().take(MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn run_snapshot_json_bounds_recent_events_for_model_context() {
        let timestamp = chrono::Utc
            .timestamp_opt(1_800_000_000, 0)
            .single()
            .expect("timestamp should be valid");
        let snapshot = codex_state::WorkflowRunSnapshot {
            run: codex_state::WorkflowRun {
                run_id: "run-1".to_string(),
                workflow_record_id: "workflow-1".to_string(),
                source_thread_id: None,
                idempotency_key: None,
                spec_workflow_id: "wf_context_bound".to_string(),
                schema_version: "1".to_string(),
                source_yaml_sha256: "sha".to_string(),
                status: codex_state::WorkflowRunStatus::Running,
                status_reason: None,
                reason_code: None,
                generation: 1,
                owner_id: None,
                lease_expires_at: None,
                heartbeat_at: None,
                last_event_seq: 25,
                agents_json: json!([]),
                execution_defaults_json: json!({}),
                limits_json: json!({}),
                approvals_json: json!({}),
                loops_json: None,
                monitor_links_json: None,
                artifacts_json: json!({}),
                cleanup_json: json!({}),
                created_at: timestamp,
                updated_at: timestamp,
                started_at: Some(timestamp),
                completed_at: None,
            },
            steps: Vec::new(),
            verifiers: Vec::new(),
            events: (1..=25)
                .map(|seq| codex_state::WorkflowRunEvent {
                    event_id: format!("event-{seq}"),
                    run_id: "run-1".to_string(),
                    seq,
                    event_type: format!("event-{seq}"),
                    actor_kind: "system".to_string(),
                    actor_id: None,
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "internal".to_string(),
                    event_payload_json: json!({"secret": "not serialized"}),
                    created_at: timestamp,
                })
                .collect(),
        };

        let value = run_snapshot_json(&snapshot);

        assert_eq!(5, value["omittedEventCount"]);
        let events = value["recentEvents"]
            .as_array()
            .expect("events should be array");
        assert_eq!(MAX_MODEL_RUN_DETAILS, events.len());
        assert_eq!(6, events[0]["seq"]);
        assert_eq!(25, events[19]["seq"]);
        assert!(!value.to_string().contains("not serialized"));
    }
}
