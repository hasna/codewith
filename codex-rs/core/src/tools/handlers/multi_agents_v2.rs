//! Implements the MultiAgentV2 collaboration tool surface.

use crate::agent::AgentStatus;
use crate::agent::agent_resolver::resolve_agent_target;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::multi_agents_common::*;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_model_provider_info::WireApi;
use codex_protocol::AgentPath;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::CollabAgentInteractionBeginEvent;
use codex_protocol::protocol::CollabAgentInteractionEndEvent;
use codex_protocol::protocol::CollabAgentSpawnBeginEvent;
use codex_protocol::protocol::CollabAgentSpawnEndEvent;
use codex_protocol::protocol::CollabCloseBeginEvent;
use codex_protocol::protocol::CollabCloseEndEvent;
use codex_protocol::protocol::CollabWaitingBeginEvent;
use codex_protocol::protocol::CollabWaitingEndEvent;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::user_input::UserInput;
use codex_tools::ToolName;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

pub(crate) use close_agent::Handler as CloseAgentHandler;
pub(crate) use followup_task::Handler as FollowupTaskHandler;
pub(crate) use list_agents::Handler as ListAgentsHandler;
pub(crate) use send_message::Handler as SendMessageHandler;
pub(crate) use spawn::Handler as SpawnAgentHandler;
pub(crate) use wait::Handler as WaitAgentHandler;

mod close_agent;
mod followup_task;
mod list_agents;
mod message_tool;
mod send_message;
mod spawn;
pub(crate) mod wait;

/// Builds the inter-agent communication for a v2 message tool, choosing the wire
/// representation from the *receiver's* provider wire protocol.
///
/// Responses-wire receivers keep the encrypted `AgentMessage` form, which the
/// Responses API renders with native inter-agent semantics. Chat-completions
/// providers have no `agent_message` item type, so an encrypted communication
/// would serialize to `ResponseItem::AgentMessage` and be silently discarded by
/// the chat-wire request builder — the receiving model would never see it.
/// For those receivers we send plain `content` instead, which stays a normal
/// assistant message on every wire (and, being plain, truncates rather than being
/// omitted when it exceeds the mailbox context bound).
///
/// When the receiver's wire is unknown, default to the encrypted form to preserve
/// prior behavior; the chat-completions builder also renders `AgentMessage` as a
/// backstop, so an unknown-wire chat receiver still receives the message.
pub(super) fn communication_from_tool_message(
    author: AgentPath,
    recipient: AgentPath,
    message: String,
    receiver_wire: Option<WireApi>,
) -> InterAgentCommunication {
    match receiver_wire {
        Some(WireApi::Chat) => InterAgentCommunication::new(
            author,
            recipient,
            Vec::new(),
            message,
            /*trigger_turn*/ true,
        ),
        _ => InterAgentCommunication::new_encrypted(
            author,
            recipient,
            Vec::new(),
            message,
            /*trigger_turn*/ true,
        ),
    }
}

#[cfg(test)]
mod communication_tests {
    use super::*;

    fn worker() -> AgentPath {
        AgentPath::try_from("/root/worker").expect("agent path")
    }

    #[test]
    fn chat_wire_receiver_gets_plain_content() {
        // Chat-wire providers have no `agent_message` item type, so the body must
        // ride in plain `content` (which survives every wire and truncates rather
        // than being omitted when large).
        let communication = communication_from_tool_message(
            AgentPath::root(),
            worker(),
            "hello".to_string(),
            Some(WireApi::Chat),
        );
        assert_eq!(communication.content, "hello");
        assert_eq!(communication.encrypted_content, None);
        assert!(communication.trigger_turn);
    }

    #[test]
    fn responses_and_unknown_wire_receivers_keep_encrypted_agent_message() {
        // Responses-wire receivers keep the existing AgentMessage form; an unknown
        // wire defaults to it too (the chat builder renders AgentMessage as a
        // backstop).
        for wire in [Some(WireApi::Responses), None] {
            let communication = communication_from_tool_message(
                AgentPath::root(),
                worker(),
                "hello".to_string(),
                wire,
            );
            assert!(communication.content.is_empty());
            assert_eq!(communication.encrypted_content.as_deref(), Some("hello"));
            assert!(communication.trigger_turn);
        }
    }
}
