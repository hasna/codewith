use crate::active_session_registry::ActivePeer;
use crate::active_session_registry::ActivePeerKind;
use crate::active_session_registry::ActivePeerOwner;
use codex_core::ThreadManager;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::InterAgentCommunication;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveChannelEndpoint {
    pub(crate) id: String,
    pub(crate) kind: ActiveChannelEndpointKind,
    pub(crate) label: Option<String>,
    pub(crate) agent_path: Option<String>,
}

impl ActiveChannelEndpoint {
    pub(crate) fn from_peer(peer: &ActivePeer) -> Self {
        Self {
            id: peer.peer_id.clone(),
            kind: active_channel_endpoint_kind(peer.kind),
            label: peer.display_name.clone(),
            agent_path: peer.agent_path.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveChannelEndpointKind {
    CodewithSession,
    CodewithSpawnedAgent,
    #[allow(dead_code)] // Reserved for native Claude session bridge adapters.
    ClaudeCodeSession,
    #[allow(dead_code)] // Reserved for native Telegram bridge adapters.
    TelegramChat,
    BridgeAdapter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveChannelDeliveryMode {
    QueueOnly,
    TriggerTurn,
}

impl ActiveChannelDeliveryMode {
    pub(crate) fn trigger_turn(self) -> bool {
        match self {
            Self::QueueOnly => false,
            Self::TriggerTurn => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveChannelEnvelope {
    pub(crate) message_id: String,
    pub(crate) sender: ActiveChannelEndpoint,
    pub(crate) claimed_sender: Option<ActiveChannelEndpoint>,
    pub(crate) recipient: ActiveChannelEndpoint,
    pub(crate) content: String,
    pub(crate) delivery: ActiveChannelDeliveryMode,
}

impl ActiveChannelEnvelope {
    pub(crate) fn new(
        message_id: String,
        sender: ActiveChannelEndpoint,
        claimed_sender: Option<ActiveChannelEndpoint>,
        recipient: ActiveChannelEndpoint,
        content: String,
        delivery: ActiveChannelDeliveryMode,
    ) -> Self {
        Self {
            message_id,
            sender,
            claimed_sender,
            recipient,
            content,
            delivery,
        }
    }
}

#[allow(dead_code)] // Bridge transports will return this once they are wired in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActiveChannelDeliveryOutcome {
    Delivered { message_id: String },
    NotLoaded { recipient_id: String },
    Unsupported { recipient_id: String },
}

#[allow(dead_code)] // Bridge transports will surface typed failures through this boundary.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ActiveChannelAdapterError {
    #[error("active channel recipient is not loaded: {recipient_id}")]
    NotLoaded { recipient_id: String },
    #[error("active channel recipient is unsupported: {recipient_id}")]
    Unsupported { recipient_id: String },
    #[error("active channel adapter failed: {message}")]
    Failed { message: String },
}

#[derive(Clone)]
pub(crate) struct ActiveChannelRouter {
    thread_manager: Arc<ThreadManager>,
    pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
}

impl ActiveChannelRouter {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
    ) -> Self {
        Self {
            thread_manager,
            pending_thread_unloads,
        }
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "active-session delivery must be serialized against pending unloads through get_thread and direct delivery"
    )]
    pub(crate) async fn deliver(
        &self,
        envelope: &ActiveChannelEnvelope,
        recipient: &ActivePeer,
        communication: InterAgentCommunication,
    ) -> Result<ActiveChannelDeliveryOutcome, ActiveChannelAdapterError> {
        match &recipient.owner {
            ActivePeerOwner::LocalThread { thread_id } => {
                let pending_thread_unloads = self.pending_thread_unloads.lock().await;
                if pending_thread_unloads.contains(thread_id) {
                    return Ok(ActiveChannelDeliveryOutcome::NotLoaded {
                        recipient_id: recipient.peer_id.clone(),
                    });
                }
                let target_thread = match self.thread_manager.get_thread(*thread_id).await {
                    Ok(thread) => thread,
                    Err(CodexErr::ThreadNotFound(_)) => {
                        return Ok(ActiveChannelDeliveryOutcome::NotLoaded {
                            recipient_id: recipient.peer_id.clone(),
                        });
                    }
                    Err(err) => {
                        return Err(ActiveChannelAdapterError::Failed {
                            message: format!("failed to resolve local active peer: {err}"),
                        });
                    }
                };
                match target_thread
                    .deliver_inter_agent_communication_with_id(
                        envelope.message_id.clone(),
                        communication,
                    )
                    .await
                {
                    Ok(()) => Ok(ActiveChannelDeliveryOutcome::Delivered {
                        message_id: envelope.message_id.clone(),
                    }),
                    Err(CodexErr::ThreadNotFound(_) | CodexErr::InternalAgentDied) => {
                        Ok(ActiveChannelDeliveryOutcome::NotLoaded {
                            recipient_id: recipient.peer_id.clone(),
                        })
                    }
                    Err(err) => Err(ActiveChannelAdapterError::Failed {
                        message: format!("failed to deliver active session message: {err}"),
                    }),
                }
            }
            ActivePeerOwner::BridgeAdapter { .. } => {
                Ok(ActiveChannelDeliveryOutcome::Unsupported {
                    recipient_id: recipient.peer_id.clone(),
                })
            }
        }
    }
}

pub(crate) fn active_channel_communication(
    envelope: &ActiveChannelEnvelope,
) -> InterAgentCommunication {
    let author = envelope
        .sender
        .agent_path
        .as_deref()
        .and_then(|path| AgentPath::try_from(path).ok())
        .unwrap_or_else(AgentPath::root);
    let recipient = envelope
        .recipient
        .agent_path
        .as_deref()
        .and_then(|path| AgentPath::try_from(path).ok())
        .unwrap_or_else(AgentPath::root);
    InterAgentCommunication::new(
        author,
        recipient,
        Vec::new(),
        envelope.content.clone(),
        envelope.delivery.trigger_turn(),
    )
}

/// Sends native active-channel envelopes through one live bridge transport.
///
/// Implementations should only deliver to endpoints they can prove are active in their own
/// transport. They must not cold-resume sessions or persist an offline queue; inactive delivery
/// should return `NotLoaded` or `Unsupported` so the caller can surface an explicit result.
#[allow(dead_code)] // Implemented by future native bridge transports, not by the local mailbox path.
pub(crate) trait ActiveChannelAdapter {
    fn send(
        &self,
        envelope: ActiveChannelEnvelope,
    ) -> impl std::future::Future<
        Output = Result<ActiveChannelDeliveryOutcome, ActiveChannelAdapterError>,
    > + Send;
}

fn active_channel_endpoint_kind(kind: ActivePeerKind) -> ActiveChannelEndpointKind {
    match kind {
        ActivePeerKind::CodewithSession => ActiveChannelEndpointKind::CodewithSession,
        ActivePeerKind::SpawnedAgent => ActiveChannelEndpointKind::CodewithSpawnedAgent,
        ActivePeerKind::BridgeAdapter => ActiveChannelEndpointKind::BridgeAdapter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoAdapter;

    impl ActiveChannelAdapter for EchoAdapter {
        async fn send(
            &self,
            envelope: ActiveChannelEnvelope,
        ) -> Result<ActiveChannelDeliveryOutcome, ActiveChannelAdapterError> {
            Ok(ActiveChannelDeliveryOutcome::Delivered {
                message_id: envelope.message_id,
            })
        }
    }

    #[tokio::test]
    async fn adapter_boundary_sends_native_envelope_without_transport_sdk() {
        let adapter = EchoAdapter;
        let envelope = ActiveChannelEnvelope::new(
            "msg-1".to_string(),
            ActiveChannelEndpoint {
                id: "codewith:root".to_string(),
                kind: ActiveChannelEndpointKind::CodewithSession,
                label: Some("Codewith".to_string()),
                agent_path: Some("/root".to_string()),
            },
            None,
            ActiveChannelEndpoint {
                id: "claude:session-1".to_string(),
                kind: ActiveChannelEndpointKind::ClaudeCodeSession,
                label: Some("Claude Code".to_string()),
                agent_path: None,
            },
            "hello bridge".to_string(),
            ActiveChannelDeliveryMode::QueueOnly,
        );

        assert_eq!(
            adapter.send(envelope).await,
            Ok(ActiveChannelDeliveryOutcome::Delivered {
                message_id: "msg-1".to_string(),
            })
        );
    }

    #[test]
    fn boundary_models_future_bridge_endpoints_and_failures() {
        let telegram_endpoint = ActiveChannelEndpoint {
            id: "telegram:chat-1".to_string(),
            kind: ActiveChannelEndpointKind::TelegramChat,
            label: Some("Telegram".to_string()),
            agent_path: None,
        };

        assert_eq!(
            telegram_endpoint.kind,
            ActiveChannelEndpointKind::TelegramChat
        );
        assert_eq!(
            ActiveChannelDeliveryOutcome::NotLoaded {
                recipient_id: telegram_endpoint.id,
            },
            ActiveChannelDeliveryOutcome::NotLoaded {
                recipient_id: "telegram:chat-1".to_string(),
            }
        );
        assert_eq!(
            ActiveChannelAdapterError::Unsupported {
                recipient_id: "claude:session-1".to_string(),
            }
            .to_string(),
            "active channel recipient is unsupported: claude:session-1"
        );
    }

    #[test]
    fn bridge_peer_kind_maps_to_bridge_endpoint_kind() {
        assert_eq!(
            active_channel_endpoint_kind(ActivePeerKind::BridgeAdapter),
            ActiveChannelEndpointKind::BridgeAdapter
        );
    }
}
