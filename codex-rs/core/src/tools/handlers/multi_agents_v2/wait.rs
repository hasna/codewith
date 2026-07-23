use super::*;
use crate::tools::handlers::multi_agents_spec::WaitAgentTimeoutOptions;
use crate::tools::handlers::multi_agents_spec::create_wait_agent_tool_v2;
use crate::turn_timing::now_unix_timestamp_ms;
use codex_protocol::protocol::CollabAgentStatusEntry;
use codex_tools::ToolSpec;
use std::time::Duration;
use tokio::time::Instant;
use tokio::time::timeout_at;

#[derive(Default)]
pub(crate) struct Handler {
    options: WaitAgentTimeoutOptions,
}

impl Handler {
    pub(crate) fn new(options: WaitAgentTimeoutOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("wait_agent")
    }

    fn spec(&self) -> ToolSpec {
        create_wait_agent_tool_v2(self.options)
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;
        let arguments = function_arguments(payload)?;
        let args: WaitArgs = parse_arguments(&arguments)?;
        let min_timeout_ms = turn.config.multi_agent_v2.min_wait_timeout_ms;
        let max_timeout_ms = turn.config.multi_agent_v2.max_wait_timeout_ms;
        let default_timeout_ms = turn.config.multi_agent_v2.default_wait_timeout_ms;
        let timeout_ms = match args.timeout_ms {
            Some(ms) if ms < min_timeout_ms => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "timeout_ms must be at least {min_timeout_ms}"
                )));
            }
            Some(ms) if ms > max_timeout_ms => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "timeout_ms must be at most {max_timeout_ms}"
                )));
            }
            Some(ms) => ms,
            None => default_timeout_ms,
        };

        let mut mailbox_rx = session.input_queue.subscribe_mailbox().await;

        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    started_at_ms: now_unix_timestamp_ms(),
                    sender_thread_id: session.thread_id,
                    receiver_thread_ids: Vec::new(),
                    receiver_agents: Vec::new(),
                    call_id: call_id.clone(),
                }
                .into(),
            )
            .await;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let timed_out = !wait_for_mailbox_change(&mut mailbox_rx, deadline).await;

        // Report an honest snapshot of the waiting agent's live sub-agents and
        // their current status. Previously this returned empty statuses, which
        // made the TUI print "No agents completed yet" on every wait.
        let (receiver_agents, statuses) = session
            .services
            .agent_control
            .collect_child_agent_statuses(session.thread_id)
            .await;
        let agent_statuses = build_wait_agent_statuses(&statuses, &receiver_agents);
        let result = WaitAgentResult::new(timed_out, agent_statuses.clone());

        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.thread_id,
                    call_id,
                    completed_at_ms: now_unix_timestamp_ms(),
                    agent_statuses,
                    statuses,
                }
                .into(),
            )
            .await;

        Ok(boxed_tool_output(result))
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    timeout_ms: Option<i64>,
}

/// Timeout copy that tells the model NOT to re-poll. Sub-agent completion is
/// push-based: a finishing child delivers its final answer to the parent's
/// mailbox and auto-resumes the parent if it is idle, so a wait timeout is not
/// a reason to call `wait_agent` again.
pub(crate) const WAIT_TIMEOUT_MESSAGE: &str = "Wait timed out before any mailbox update arrived. You do NOT need to keep waiting: when a spawned agent finishes, its final answer is delivered to your mailbox automatically and a new turn starts if you have ended yours. Do not call wait_agent again in a loop — continue other work or end your turn.";
pub(crate) const WAIT_COMPLETED_MESSAGE: &str = "A mailbox update arrived. See agent_statuses for the current state of your sub-agents; any final answers are also delivered to your mailbox as notifications.";

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WaitAgentResult {
    pub(crate) message: String,
    pub(crate) timed_out: bool,
    /// Current status of the waiting agent's live sub-agents when the wait
    /// returned. Empty when there are no live sub-agents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) agent_statuses: Vec<CollabAgentStatusEntry>,
}

impl WaitAgentResult {
    fn new(timed_out: bool, agent_statuses: Vec<CollabAgentStatusEntry>) -> Self {
        let message = if timed_out {
            WAIT_TIMEOUT_MESSAGE
        } else {
            WAIT_COMPLETED_MESSAGE
        };
        Self {
            message: message.to_string(),
            timed_out,
            agent_statuses,
        }
    }
}

impl ToolOutput for WaitAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "wait_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, /*success*/ None, "wait_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "wait_agent")
    }
}

async fn wait_for_mailbox_change(
    mailbox_rx: &mut tokio::sync::watch::Receiver<()>,
    deadline: Instant,
) -> bool {
    match timeout_at(deadline, mailbox_rx.changed()).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_out_result_tells_model_not_to_repoll() {
        let result = WaitAgentResult::new(/*timed_out*/ true, Vec::new());
        assert!(result.timed_out);
        assert!(
            result
                .message
                .contains("Do not call wait_agent again in a loop"),
            "timeout message must steer the model away from looping: {}",
            result.message
        );
        assert!(result.agent_statuses.is_empty());
    }

    #[test]
    fn completed_result_points_to_agent_statuses() {
        let result = WaitAgentResult::new(/*timed_out*/ false, Vec::new());
        assert!(!result.timed_out);
        assert!(result.message.contains("agent_statuses"));
    }
}
