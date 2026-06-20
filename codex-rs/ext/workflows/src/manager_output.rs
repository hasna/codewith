use serde_json::Value;
use serde_json::json;

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
    json!({
        "run": run_summary_json(snapshot),
        "steps": snapshot.steps.iter().map(step_json).collect::<Vec<_>>(),
        "verifiers": snapshot.verifiers.iter().map(verifier_json).collect::<Vec<_>>(),
        "events": snapshot.events.iter().map(event_json).collect::<Vec<_>>(),
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
