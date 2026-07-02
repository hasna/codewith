use codex_protocol::ThreadId;
use codex_protocol::protocol::CodexErrorInfo;
use serde_json::json;

const GOAL_STATUS_WAIT_POLICY: &str = "record-and-wait-for-coordinator";

pub(crate) async fn record_goal_status_wait(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal: &codex_state::ThreadGoal,
    turn_id: Option<&str>,
    reason: &str,
) -> Result<(), String> {
    record_goal_status_wait_with_context(
        state_db,
        thread_id,
        goal,
        turn_id,
        GoalStatusWaitContext::Reason { reason },
    )
    .await
}

pub(crate) async fn record_goal_turn_error_status_wait(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal: &codex_state::ThreadGoal,
    turn_id: &str,
    error: &CodexErrorInfo,
) -> Result<(), String> {
    record_goal_status_wait_with_context(
        state_db,
        thread_id,
        goal,
        Some(turn_id),
        GoalStatusWaitContext::TurnError { error },
    )
    .await
}

async fn record_goal_status_wait_with_context(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal: &codex_state::ThreadGoal,
    turn_id: Option<&str>,
    context: GoalStatusWaitContext<'_>,
) -> Result<(), String> {
    let Some(kind) = pending_interaction_kind_for_goal_status(goal.status) else {
        return Ok(());
    };
    let occurred_at_ms = goal.updated_at.timestamp_millis();
    let worker_request_id = format!("goal:{}:{}:{occurred_at_ms}", goal.goal_id, kind.as_str());
    let reason = context.reason();
    let mut request_payload_json = json!({
        "type": "goalStatusWait",
        "reason": reason,
        "threadId": thread_id.to_string(),
        "goalId": goal.goal_id.as_str(),
        "status": goal.status.as_str(),
        "objective": goal.objective.as_str(),
        "tokenBudget": goal.token_budget,
        "tokensUsed": goal.tokens_used,
        "timeUsedSeconds": goal.time_used_seconds,
    });
    if let GoalStatusWaitContext::TurnError { error } = context {
        request_payload_json["terminalError"] = turn_error_payload(error);
    }
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
            request_payload_json,
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

enum GoalStatusWaitContext<'a> {
    Reason { reason: &'a str },
    TurnError { error: &'a CodexErrorInfo },
}

impl<'a> GoalStatusWaitContext<'a> {
    fn reason(&self) -> &'a str {
        match self {
            Self::Reason { reason } => reason,
            Self::TurnError { .. } => "turn-error",
        }
    }
}

fn turn_error_payload(error: &CodexErrorInfo) -> serde_json::Value {
    json!({
        "codexErrorInfo": error,
        "code": codex_error_code(error),
        "action": codex_error_action(error),
    })
}

fn codex_error_code(error: &CodexErrorInfo) -> &'static str {
    match error {
        CodexErrorInfo::ContextWindowExceeded => "context_window_exceeded",
        CodexErrorInfo::UsageLimitExceeded => "usage_limit_exceeded",
        CodexErrorInfo::ServerOverloaded => "server_overloaded",
        CodexErrorInfo::CyberPolicy => "cyber_policy",
        CodexErrorInfo::HttpConnectionFailed { .. } => "http_connection_failed",
        CodexErrorInfo::ResponseStreamConnectionFailed { .. } => {
            "response_stream_connection_failed"
        }
        CodexErrorInfo::InternalServerError => "internal_server_error",
        CodexErrorInfo::Unauthorized => "unauthorized",
        CodexErrorInfo::BadRequest => "bad_request",
        CodexErrorInfo::SandboxError => "sandbox_error",
        CodexErrorInfo::ResponseStreamDisconnected { .. } => "response_stream_disconnected",
        CodexErrorInfo::ResponseTooManyFailedAttempts { .. } => "response_too_many_failed_attempts",
        CodexErrorInfo::ActiveTurnNotSteerable { .. } => "active_turn_not_steerable",
        CodexErrorInfo::ThreadRollbackFailed => "thread_rollback_failed",
        CodexErrorInfo::Other => "other",
    }
}

fn codex_error_action(error: &CodexErrorInfo) -> String {
    match error {
        CodexErrorInfo::ContextWindowExceeded => {
            "The turn exceeded the model context window. Reduce prompt/history size or run a cleanup/compaction before retrying the loop.".to_string()
        }
        CodexErrorInfo::UsageLimitExceeded => {
            "The auth profile is usage-limited. Switch to a healthy profile or wait for the quota window before retrying the loop.".to_string()
        }
        CodexErrorInfo::ServerOverloaded => {
            "The provider reported overload. Retry later or route the loop to another healthy provider/profile.".to_string()
        }
        CodexErrorInfo::CyberPolicy => {
            "The provider blocked the request on policy. Reduce or reframe the blocked task before retrying the loop.".to_string()
        }
        CodexErrorInfo::HttpConnectionFailed { http_status_code } => match http_status_code {
            Some(status) => format!(
                "The provider HTTP request failed with status {status}. Check provider availability, network access, and auth before retrying the loop."
            ),
            None => {
                "The provider HTTP request failed before returning a status. Check network access and provider availability before retrying the loop.".to_string()
            }
        },
        CodexErrorInfo::ResponseStreamConnectionFailed { http_status_code } => {
            match http_status_code {
                Some(status) => format!(
                    "The response stream failed to connect with status {status}. Check provider availability and retry the loop."
                ),
                None => {
                    "The response stream failed to connect. Check provider availability and retry the loop.".to_string()
                }
            }
        }
        CodexErrorInfo::InternalServerError => {
            "The provider returned an internal server error. Retry later or route the loop to another healthy provider/profile.".to_string()
        }
        CodexErrorInfo::Unauthorized => {
            "Authentication failed. Refresh or repair the Codewith auth profile before retrying the loop.".to_string()
        }
        CodexErrorInfo::BadRequest => {
            "The provider rejected the request. Inspect the turn error and request shape before retrying the loop.".to_string()
        }
        CodexErrorInfo::SandboxError => {
            "The sandbox failed while running the turn. Inspect command sandbox settings and retry after fixing the sandbox error.".to_string()
        }
        CodexErrorInfo::ResponseStreamDisconnected { http_status_code } => {
            match http_status_code {
                Some(status) => format!(
                    "The response stream disconnected with status {status}. Retry the loop or route it to another healthy provider/profile."
                ),
                None => {
                    "The response stream disconnected mid-turn. Retry the loop or route it to another healthy provider/profile.".to_string()
                }
            }
        }
        CodexErrorInfo::ResponseTooManyFailedAttempts { http_status_code } => {
            match http_status_code {
                Some(status) => format!(
                    "The turn exhausted provider retry attempts after status {status}. Check provider health and retry once the upstream issue clears."
                ),
                None => {
                    "The turn exhausted provider retry attempts. Check provider health and retry once the upstream issue clears.".to_string()
                }
            }
        }
        CodexErrorInfo::ActiveTurnNotSteerable { .. } => {
            "The turn could not accept steering. Start a fresh turn instead of steering the active turn.".to_string()
        }
        CodexErrorInfo::ThreadRollbackFailed => {
            "Thread rollback failed. Inspect thread history/state before retrying the loop.".to_string()
        }
        CodexErrorInfo::Other => {
            "The host classified the turn failure as other. Inspect the turn error event or Codewith stderr before retrying the loop.".to_string()
        }
    }
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
        | codex_state::ThreadGoalStatus::Deferred
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
