use super::*;
use crate::active_session_bridge::ActiveChannelDeliveryMode;
use crate::active_session_bridge::ActiveChannelEndpoint;
use crate::active_session_bridge::ActiveChannelEnvelope;
use crate::active_session_registry::ActivePeer;
use crate::active_session_registry::ActivePeerCapabilities;
use crate::active_session_registry::ActivePeerCapability as RegistryCapability;
use crate::active_session_registry::ActivePeerFreshness;
use crate::active_session_registry::ActivePeerKind as RegistryPeerKind;
use crate::active_session_registry::ActivePeerLookupError;
use crate::active_session_registry::ActivePeerRegistration;
use crate::active_session_registry::ActivePeerRegistry;
use crate::active_session_registry::LastSeenAt;
use codex_protocol::protocol::SubAgentSource;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct ActiveSessionRequestProcessor {
    thread_manager: Arc<ThreadManager>,
}

impl ActiveSessionRequestProcessor {
    pub(crate) fn new(thread_manager: Arc<ThreadManager>) -> Self {
        Self { thread_manager }
    }

    pub(crate) async fn active_session_list(
        &self,
        params: ActiveSessionListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.active_session_list_response_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn active_session_send(
        &self,
        params: ActiveSessionSendParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.active_session_send_response_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    async fn active_session_list_response_inner(
        &self,
        params: ActiveSessionListParams,
    ) -> Result<ActiveSessionListResponse, JSONRPCErrorError> {
        let ActiveSessionListParams { cursor, limit } = params;
        let freshness = freshness_now();
        let mut registry = self.active_peer_registry(freshness.now).await?;
        let _ = registry.remove_inactive(freshness);
        let mut data = registry
            .list_active(freshness)
            .into_iter()
            .map(api_active_session_peer)
            .collect::<Vec<_>>();
        data.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));

        if data.is_empty() {
            return Ok(ActiveSessionListResponse {
                data,
                next_cursor: None,
            });
        }

        let total = data.len();
        let start = match cursor {
            Some(cursor) => match data.binary_search_by(|peer| peer.peer_id.cmp(&cursor)) {
                Ok(idx) => idx + 1,
                Err(idx) => idx,
            },
            None => 0,
        };
        let effective_limit = limit.unwrap_or(total as u32).max(1) as usize;
        let end = start.saturating_add(effective_limit).min(total);
        let page = data[start..end].to_vec();
        let next_cursor = page
            .last()
            .filter(|_| end < total)
            .map(|peer| peer.peer_id.clone());

        Ok(ActiveSessionListResponse {
            data: page,
            next_cursor,
        })
    }

    async fn active_session_send_response_inner(
        &self,
        params: ActiveSessionSendParams,
    ) -> Result<ActiveSessionSendResponse, JSONRPCErrorError> {
        let ActiveSessionSendParams {
            target_thread_id,
            message,
            sender_thread_id,
            sender_label,
            delivery,
        } = params;
        if message.trim().is_empty() {
            return Err(invalid_params(
                "activeSession/send message must not be empty",
            ));
        }

        let message_id = uuid::Uuid::now_v7().to_string();
        let target = ThreadId::from_string(target_thread_id.as_str())
            .map_err(|err| invalid_request(format!("invalid targetThreadId: {err}")))?;
        let sender = match sender_thread_id.as_deref() {
            Some(sender_thread_id) => Some(
                ThreadId::from_string(sender_thread_id)
                    .map_err(|err| invalid_request(format!("invalid senderThreadId: {err}")))?,
            ),
            None => None,
        };
        let freshness = freshness_now();
        let registry = self.active_peer_registry(freshness.now).await?;
        let target_peer = match registry.get_active(target_thread_id.as_str(), freshness) {
            Ok(peer) => peer,
            Err(ActivePeerLookupError::Unknown { .. } | ActivePeerLookupError::Inactive { .. }) => {
                return Ok(ActiveSessionSendResponse {
                    status: ActiveSessionSendStatus::NotLoaded,
                    message_id,
                    target_thread_id,
                    sender_thread_id,
                    reason: Some(
                        "target thread is not currently loaded; inactive delivery is deferred"
                            .to_string(),
                    ),
                });
            }
        };
        let sender_peer = match sender {
            Some(sender) => {
                let sender_thread_id = sender.to_string();
                match registry.get_active(sender_thread_id.as_str(), freshness) {
                    Ok(peer) => Some(peer),
                    Err(
                        ActivePeerLookupError::Unknown { .. }
                        | ActivePeerLookupError::Inactive { .. },
                    ) => {
                        return Ok(ActiveSessionSendResponse {
                            status: ActiveSessionSendStatus::NotLoaded,
                            message_id,
                            target_thread_id,
                            sender_thread_id: Some(sender_thread_id),
                            reason: Some(
                                "sender thread is not currently loaded; inactive delivery is deferred"
                                    .to_string(),
                            ),
                        });
                    }
                }
            }
            None => None,
        };
        let target_thread = self
            .thread_manager
            .get_thread(target)
            .await
            .map_err(active_session_get_thread_error)?;
        let envelope = active_channel_envelope(
            message_id.as_str(),
            sender_label,
            sender_peer.as_ref(),
            &target_peer,
            message,
            delivery
                .unwrap_or(ActiveSessionMessageDelivery::QueueOnly)
                .into(),
        );
        let communication = active_session_communication(&envelope);
        target_thread
            .submit(Op::InterAgentCommunication { communication })
            .await
            .map_err(active_session_submit_error)?;

        Ok(ActiveSessionSendResponse {
            status: ActiveSessionSendStatus::Delivered,
            message_id,
            target_thread_id,
            sender_thread_id,
            reason: None,
        })
    }

    async fn active_peer_registry(
        &self,
        last_seen_at: LastSeenAt,
    ) -> Result<ActivePeerRegistry, JSONRPCErrorError> {
        let mut registry = ActivePeerRegistry::default();
        for thread_id in self.thread_manager.list_thread_ids().await {
            let thread = self
                .thread_manager
                .get_thread(thread_id)
                .await
                .map_err(active_session_get_thread_error)?;
            let config_snapshot = thread.config_snapshot().await;
            let session_id = thread.session_configured().session_id.to_string();
            let peer_id = thread_id.to_string();
            registry.register(
                active_peer_registration(thread_id, session_id, &config_snapshot),
                last_seen_at,
            );
            let _ = registry.heartbeat(peer_id.as_str(), last_seen_at);
        }
        Ok(registry)
    }
}

fn freshness_now() -> ActivePeerFreshness {
    let now = LastSeenAt::from_unix_seconds(time::OffsetDateTime::now_utc().unix_timestamp());
    ActivePeerFreshness::new(now, Duration::from_secs(0))
}

fn active_peer_registration(
    thread_id: ThreadId,
    session_id: String,
    config_snapshot: &ThreadConfigSnapshot,
) -> ActivePeerRegistration {
    let agent_path = config_snapshot
        .session_source
        .get_agent_path()
        .map(String::from);
    let agent_nickname = config_snapshot.session_source.get_nickname();
    let agent_role = config_snapshot.session_source.get_agent_role();
    let display_name = agent_nickname
        .or_else(|| agent_role.clone())
        .or_else(|| agent_path.clone());
    ActivePeerRegistration {
        peer_id: thread_id.to_string(),
        kind: active_peer_kind(&config_snapshot.session_source),
        thread_id,
        session_id,
        cwd: config_snapshot.cwd.clone(),
        display_name,
        agent_path,
        process: None,
        capabilities: ActivePeerCapabilities::codewith_session(),
    }
}

fn active_peer_kind(session_source: &SessionSource) -> RegistryPeerKind {
    match session_source {
        SessionSource::SubAgent(SubAgentSource::ThreadSpawn { .. }) => {
            RegistryPeerKind::SpawnedAgent
        }
        _ => RegistryPeerKind::CodewithSession,
    }
}

fn api_active_session_peer(peer: ActivePeer) -> ActiveSessionPeer {
    ActiveSessionPeer {
        peer_id: peer.peer_id,
        kind: api_peer_kind(peer.kind),
        thread_id: peer.thread_id.to_string(),
        session_id: peer.session_id,
        cwd: peer.cwd,
        display_name: peer.display_name,
        agent_path: peer.agent_path,
        capabilities: api_capabilities(peer.capabilities),
        last_seen_at: peer.last_seen_at.unix_seconds(),
    }
}

fn api_peer_kind(kind: RegistryPeerKind) -> ActiveSessionPeerKind {
    match kind {
        RegistryPeerKind::CodewithSession => ActiveSessionPeerKind::CodewithSession,
        RegistryPeerKind::SpawnedAgent => ActiveSessionPeerKind::SpawnedAgent,
    }
}

fn api_capabilities(capabilities: ActivePeerCapabilities) -> Vec<ActiveSessionCapability> {
    [
        (
            RegistryCapability::ReceiveMessage,
            ActiveSessionCapability::ReceiveMessage,
        ),
        (
            RegistryCapability::QueueMessage,
            ActiveSessionCapability::QueueMessage,
        ),
        (
            RegistryCapability::TriggerTurn,
            ActiveSessionCapability::TriggerTurn,
        ),
        (
            RegistryCapability::ClaudeChannelBridge,
            ActiveSessionCapability::ClaudeChannelBridge,
        ),
    ]
    .into_iter()
    .filter_map(|(registry, api)| capabilities.contains(registry).then_some(api))
    .collect()
}

fn active_channel_envelope(
    message_id: &str,
    sender_label: Option<String>,
    sender_peer: Option<&ActivePeer>,
    target_peer: &ActivePeer,
    message: String,
    delivery: ActiveChannelDeliveryMode,
) -> ActiveChannelEnvelope {
    let sender = sender_label
        .or_else(|| sender_peer.and_then(|peer| peer.display_name.clone()))
        .or_else(|| sender_peer.map(|peer| peer.peer_id.clone()))
        .unwrap_or_else(|| "external active session".to_string());
    ActiveChannelEnvelope::new(
        message_id.to_string(),
        sender_peer
            .map(ActiveChannelEndpoint::from_peer)
            .unwrap_or_else(external_active_channel_endpoint),
        ActiveChannelEndpoint::from_peer(target_peer),
        format!("Active session message {message_id} from {sender}:\n\n{message}"),
        delivery,
    )
}

fn external_active_channel_endpoint() -> ActiveChannelEndpoint {
    ActiveChannelEndpoint {
        id: "external-active-session".to_string(),
        kind: crate::active_session_bridge::ActiveChannelEndpointKind::BridgeAdapter,
        label: Some("external active session".to_string()),
        agent_path: None,
    }
}

fn active_session_communication(envelope: &ActiveChannelEnvelope) -> InterAgentCommunication {
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

impl From<ActiveSessionMessageDelivery> for ActiveChannelDeliveryMode {
    fn from(value: ActiveSessionMessageDelivery) -> Self {
        match value {
            ActiveSessionMessageDelivery::QueueOnly => Self::QueueOnly,
            ActiveSessionMessageDelivery::TriggerTurn => Self::TriggerTurn,
        }
    }
}

fn active_session_get_thread_error(err: CodexErr) -> JSONRPCErrorError {
    match err {
        CodexErr::ThreadNotFound(thread_id) => {
            invalid_request(format!("thread not found: {thread_id}"))
        }
        err => internal_error(format!("failed to resolve active thread: {err}")),
    }
}

fn active_session_submit_error(err: CodexErr) -> JSONRPCErrorError {
    match err {
        CodexErr::ThreadNotFound(thread_id) => {
            invalid_request(format!("thread not found: {thread_id}"))
        }
        err => internal_error(format!("failed to deliver active session message: {err}")),
    }
}
