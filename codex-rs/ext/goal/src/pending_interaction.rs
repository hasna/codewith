use codex_protocol::ThreadId;
use serde_json::json;

const GOAL_STATUS_WAIT_POLICY: &str = "record-and-wait-for-coordinator";

pub(crate) async fn record_goal_status_wait(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal: &codex_state::ThreadGoal,
    turn_id: Option<&str>,
    reason: &str,
) -> Result<(), String> {
    let Some(kind) = pending_interaction_kind_for_goal_status(goal.status) else {
        return Ok(());
    };
    let occurred_at_ms = goal.updated_at.timestamp_millis();
    let worker_request_id = format!("goal:{}:{}:{occurred_at_ms}", goal.goal_id, kind.as_str());
    state_db
        .create_thread_pending_interaction_if_absent(&codex_state::PendingInteractionCreateParams {
            interaction_id: worker_request_id.clone(),
            thread_id,
            source_kind: codex_state::PendingInteractionSourceKind::Goal,
            source_id: Some(goal.goal_id.clone()),
            turn_id: turn_id.map(str::to_string),
            worker_request_id: Some(worker_request_id),
            server_request_id_json: None,
            kind,
            request_payload_json: json!({
                "type": "goalStatusWait",
                "reason": reason,
                "threadId": thread_id.to_string(),
                "goalId": goal.goal_id.as_str(),
                "status": goal.status.as_str(),
                "objective": goal.objective.as_str(),
                "tokenBudget": goal.token_budget,
                "tokensUsed": goal.tokens_used,
                "timeUsedSeconds": goal.time_used_seconds,
            }),
            request_payload_preview: format!(
                "{}: {}",
                goal.status.as_str(),
                truncate_goal_preview(goal.objective.as_str())
            ),
            request_redactions_json: json!([]),
            no_client_policy: GOAL_STATUS_WAIT_POLICY.to_string(),
            timeout_at: None,
        })
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

pub(crate) async fn clear_goal_status_waits(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal_id: &str,
    reason: &str,
) -> Result<(), String> {
    state_db
        .respond_thread_pending_interactions_for_source(
            codex_state::PendingInteractionRespondForSourceParams {
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Goal,
                source_id: goal_id.to_string(),
                kinds: vec![
                    codex_state::PendingInteractionKind::Blocked,
                    codex_state::PendingInteractionKind::UsageLimit,
                ],
                response_payload_json: json!({
                    "type": "terminal",
                    "reason": reason,
                }),
                response_payload_preview: reason.to_string(),
                response_redactions_json: json!([]),
                terminal_status: codex_state::PendingInteractionStatus::NoLongerWaiting,
            },
        )
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn pending_interaction_kind_for_goal_status(
    status: codex_state::ThreadGoalStatus,
) -> Option<codex_state::PendingInteractionKind> {
    match status {
        codex_state::ThreadGoalStatus::Blocked => {
            Some(codex_state::PendingInteractionKind::Blocked)
        }
        codex_state::ThreadGoalStatus::UsageLimited => {
            Some(codex_state::PendingInteractionKind::UsageLimit)
        }
        codex_state::ThreadGoalStatus::Active
        | codex_state::ThreadGoalStatus::Paused
        | codex_state::ThreadGoalStatus::BudgetLimited
        | codex_state::ThreadGoalStatus::Complete
        | codex_state::ThreadGoalStatus::Cancelled => None,
    }
}

fn truncate_goal_preview(value: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 160;
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(MAX_PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}
