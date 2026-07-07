use super::*;
use crate::active_session_bridge::ActiveChannelDeliveryMode;
use crate::active_session_bridge::ActiveChannelDeliveryOutcome;
use crate::active_session_bridge::ActiveChannelEndpoint;
use crate::active_session_bridge::ActiveChannelEnvelope;
use crate::active_session_bridge::ActiveChannelRouter;
use crate::active_session_bridge::active_channel_communication;
use crate::active_session_registry::ActivePeer;
use crate::active_session_registry::ActivePeerCapabilities;
use crate::active_session_registry::ActivePeerCapability as RegistryCapability;
use crate::active_session_registry::ActivePeerDirectory;
use crate::active_session_registry::ActivePeerFreshness;
use crate::active_session_registry::ActivePeerKind as RegistryPeerKind;
use crate::active_session_registry::ActivePeerLookupError;
use crate::active_session_registry::ActivePeerRegistry;
use crate::active_session_registry::LastSeenAt;
use codex_app_server_protocol::AuthProfileKind;
use std::time::Duration;

const TARGET_NOT_LOADED_REASON: &str =
    "target thread is not currently loaded; no offline delivery was attempted";
const SENDER_NOT_LOADED_REASON: &str =
    "sender thread is not currently loaded; no offline delivery was attempted";
const TARGET_UNSUPPORTED_REASON: &str =
    "target peer is active but this app-server cannot deliver to its owner yet";
const MAX_ACTIVE_SESSION_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_ACTIVE_SESSION_SENDER_LABEL_BYTES: usize = 256;
const MAX_ACTIVE_SESSION_DESCRIPTOR_COMPONENT_BYTES: usize = 256;

#[derive(Clone)]
pub(crate) struct ActiveSessionRequestProcessor {
    active_peer_directory: ActivePeerDirectory,
    active_channel_router: ActiveChannelRouter,
}

impl ActiveSessionRequestProcessor {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
    ) -> Self {
        Self {
            active_peer_directory: ActivePeerDirectory::new(
                Arc::clone(&thread_manager),
                Arc::clone(&pending_thread_unloads),
            ),
            active_channel_router: ActiveChannelRouter::new(thread_manager, pending_thread_unloads),
        }
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
            target_peer_id,
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
        validate_max_bytes(
            "activeSession/send message",
            message.as_str(),
            MAX_ACTIVE_SESSION_MESSAGE_BYTES,
        )?;
        if let Some(sender_label) = sender_label.as_deref() {
            validate_max_bytes(
                "activeSession/send senderLabel",
                sender_label,
                MAX_ACTIVE_SESSION_SENDER_LABEL_BYTES,
            )?;
        }

        let message_id = uuid::Uuid::now_v7().to_string();
        let target_thread_id = optional_target_thread_id(target_thread_id)?;
        let target_lookup = active_session_target_lookup(target_peer_id, target_thread_id)?;
        let sender_thread_id = match sender_thread_id {
            Some(sender_thread_id) => {
                let sender = ThreadId::from_string(sender_thread_id.as_str())
                    .map_err(|err| invalid_request(format!("invalid senderThreadId: {err}")))?;
                Some(sender.to_string())
            }
            None => None,
        };
        let freshness = freshness_now();
        let registry = self.active_peer_registry(freshness.now).await?;
        let target_peer = match registry.get_active(target_lookup.peer_id.as_str(), freshness) {
            Ok(peer) => peer,
            Err(ActivePeerLookupError::Unknown { .. } | ActivePeerLookupError::Inactive { .. }) => {
                return Ok(active_session_not_loaded_response(
                    message_id,
                    target_lookup.peer_id,
                    target_lookup.thread_id,
                    sender_thread_id,
                    target_lookup.not_loaded_reason,
                ));
            }
        };
        let target_peer_id = target_peer.peer_id.clone();
        let target_thread_id = Some(target_peer.thread_id.to_string());
        let sender_peer = match sender_thread_id.as_ref() {
            Some(sender_thread_id) => {
                match registry.get_active(sender_thread_id.as_str(), freshness) {
                    Ok(peer) => Some(peer),
                    Err(
                        ActivePeerLookupError::Unknown { .. }
                        | ActivePeerLookupError::Inactive { .. },
                    ) => {
                        return Ok(active_session_not_loaded_response(
                            message_id,
                            target_peer_id,
                            target_thread_id,
                            Some(sender_thread_id.clone()),
                            SENDER_NOT_LOADED_REASON,
                        ));
                    }
                }
            }
            None => None,
        };
        let envelope = active_channel_envelope(
            message_id.as_str(),
            sender_label.as_deref(),
            sender_peer.as_ref(),
            &target_peer,
            message.as_str(),
            delivery
                .unwrap_or(ActiveSessionMessageDelivery::QueueOnly)
                .into(),
        );
        let communication = active_channel_communication(&envelope);
        let delivery_outcome = self
            .active_channel_router
            .deliver(&envelope, &target_peer, communication)
            .await
            .map_err(active_session_delivery_error)?;
        match delivery_outcome {
            ActiveChannelDeliveryOutcome::Delivered { .. } => {}
            ActiveChannelDeliveryOutcome::NotLoaded { .. } => {
                return Ok(active_session_send_response(
                    ActiveSessionSendStatus::NotLoaded,
                    message_id,
                    target_peer_id,
                    target_thread_id,
                    sender_thread_id,
                    Some(TARGET_NOT_LOADED_REASON),
                ));
            }
            ActiveChannelDeliveryOutcome::Unsupported { .. } => {
                return Ok(active_session_send_response(
                    ActiveSessionSendStatus::Unsupported,
                    message_id,
                    target_peer_id,
                    target_thread_id,
                    sender_thread_id,
                    Some(TARGET_UNSUPPORTED_REASON),
                ));
            }
        }

        // Delivered means the message was enqueued on the target session's live
        // in-memory submission channel. It does not mean the target has drained
        // the mailbox, started a turn, or persisted the communication.
        Ok(ActiveSessionSendResponse {
            status: ActiveSessionSendStatus::Delivered,
            message_id,
            target_peer_id,
            target_thread_id,
            sender_thread_id,
            reason: None,
        })
    }

    async fn active_peer_registry(
        &self,
        last_seen_at: LastSeenAt,
    ) -> Result<ActivePeerRegistry, JSONRPCErrorError> {
        self.active_peer_directory
            .snapshot(last_seen_at)
            .await
            .map_err(active_session_get_thread_error)
    }
}

fn freshness_now() -> ActivePeerFreshness {
    let now = LastSeenAt::from_unix_seconds(time::OffsetDateTime::now_utc().unix_timestamp());
    ActivePeerFreshness::new(now, Duration::from_secs(0))
}

pub(super) fn api_active_session_peer(peer: ActivePeer) -> ActiveSessionPeer {
    let auth_profile_kind = if peer.auth_profile.is_some() {
        AuthProfileKind::Named
    } else {
        AuthProfileKind::Default
    };
    ActiveSessionPeer {
        peer_id: peer.peer_id,
        kind: api_peer_kind(peer.kind),
        thread_id: peer.thread_id.to_string(),
        session_id: peer.session_id,
        cwd: peer.cwd,
        display_name: peer.display_name,
        agent_path: peer.agent_path,
        auth_profile: peer.auth_profile,
        auth_profile_kind,
        capabilities: api_capabilities(peer.capabilities),
        last_seen_at: peer.last_seen_at.unix_seconds(),
    }
}

fn api_peer_kind(kind: RegistryPeerKind) -> ActiveSessionPeerKind {
    match kind {
        RegistryPeerKind::CodewithSession => ActiveSessionPeerKind::CodewithSession,
        RegistryPeerKind::SpawnedAgent => ActiveSessionPeerKind::SpawnedAgent,
        RegistryPeerKind::BridgeAdapter => ActiveSessionPeerKind::BridgeAdapter,
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

fn validate_max_bytes(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), JSONRPCErrorError> {
    if value.len() > max_bytes {
        return Err(invalid_params(format!(
            "{field} must not exceed {max_bytes} bytes"
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct ActiveSessionTargetLookup {
    peer_id: String,
    thread_id: Option<String>,
    not_loaded_reason: &'static str,
}

fn optional_target_thread_id(
    target_thread_id: Option<String>,
) -> Result<Option<String>, JSONRPCErrorError> {
    let Some(target_thread_id) = target_thread_id else {
        return Ok(None);
    };
    if target_thread_id.trim().is_empty() {
        return Ok(None);
    }
    ThreadId::from_string(target_thread_id.as_str())
        .map(|thread_id| Some(thread_id.to_string()))
        .map_err(|err| invalid_request(format!("invalid targetThreadId: {err}")))
}

fn active_session_target_lookup(
    target_peer_id: Option<String>,
    target_thread_id: Option<String>,
) -> Result<ActiveSessionTargetLookup, JSONRPCErrorError> {
    if let Some(target_peer_id) = target_peer_id {
        if target_peer_id.trim().is_empty() {
            return Err(invalid_params(
                "activeSession/send targetPeerId must not be empty",
            ));
        }
        let target_peer_id = match ThreadId::from_string(target_peer_id.as_str()) {
            Ok(thread_id) => thread_id.to_string(),
            Err(_) => target_peer_id,
        };
        let target_thread_id = target_thread_id.or_else(|| {
            ThreadId::from_string(target_peer_id.as_str())
                .ok()
                .map(|thread_id| thread_id.to_string())
        });
        return Ok(ActiveSessionTargetLookup {
            peer_id: target_peer_id,
            thread_id: target_thread_id,
            not_loaded_reason: "target peer is not currently loaded; no offline delivery was attempted",
        });
    }

    let Some(target_thread_id) = target_thread_id else {
        return Err(invalid_params(
            "activeSession/send requires targetThreadId or targetPeerId",
        ));
    };
    Ok(ActiveSessionTargetLookup {
        peer_id: target_thread_id.clone(),
        thread_id: Some(target_thread_id),
        not_loaded_reason: TARGET_NOT_LOADED_REASON,
    })
}

fn active_channel_envelope(
    message_id: &str,
    sender_label: Option<&str>,
    sender_peer: Option<&ActivePeer>,
    target_peer: &ActivePeer,
    message: &str,
    delivery: ActiveChannelDeliveryMode,
) -> ActiveChannelEnvelope {
    let sender = untrusted_sender_descriptor(sender_label, sender_peer);
    let claimed_sender = sender_peer.map(ActiveChannelEndpoint::from_peer);
    ActiveChannelEnvelope::new(
        message_id.to_string(),
        external_active_channel_endpoint(),
        claimed_sender,
        ActiveChannelEndpoint::from_peer(target_peer),
        format!("Active session message {message_id} from {sender}:\n\n{message}"),
        delivery,
    )
}

fn untrusted_sender_descriptor(
    sender_label: Option<&str>,
    sender_peer: Option<&ActivePeer>,
) -> String {
    match (sender_peer, sender_label) {
        (Some(peer), Some(label)) => {
            let peer_label = peer
                .display_name
                .as_deref()
                .unwrap_or(peer.peer_id.as_str());
            let peer_label = bounded_descriptor_component(peer_label);
            let label = bounded_descriptor_component(label);
            format!("unverified app-server client claiming {peer_label} with label {label:?}")
        }
        (Some(peer), None) => {
            let peer_label = peer
                .display_name
                .as_deref()
                .unwrap_or(peer.peer_id.as_str());
            let peer_label = bounded_descriptor_component(peer_label);
            format!("unverified app-server client claiming {peer_label}")
        }
        (None, Some(label)) => {
            let label = bounded_descriptor_component(label);
            format!("unverified app-server client {label:?}")
        }
        (None, None) => "external active-session client".to_string(),
    }
}

fn bounded_descriptor_component(value: &str) -> String {
    if value.len() <= MAX_ACTIVE_SESSION_DESCRIPTOR_COMPONENT_BYTES {
        return value.to_string();
    }

    let max_without_suffix = MAX_ACTIVE_SESSION_DESCRIPTOR_COMPONENT_BYTES.saturating_sub(3);
    let mut truncated = String::new();
    for ch in value.chars() {
        if truncated.len() + ch.len_utf8() > max_without_suffix {
            break;
        }
        truncated.push(ch);
    }
    truncated.push_str("...");
    truncated
}

fn external_active_channel_endpoint() -> ActiveChannelEndpoint {
    ActiveChannelEndpoint {
        id: "external-active-session".to_string(),
        kind: crate::active_session_bridge::ActiveChannelEndpointKind::BridgeAdapter,
        label: Some("external active session".to_string()),
        agent_path: None,
    }
}

impl From<ActiveSessionMessageDelivery> for ActiveChannelDeliveryMode {
    fn from(value: ActiveSessionMessageDelivery) -> Self {
        match value {
            ActiveSessionMessageDelivery::QueueOnly => Self::QueueOnly,
            ActiveSessionMessageDelivery::TriggerTurn => Self::TriggerTurn,
        }
    }
}

fn active_session_not_loaded_response(
    message_id: String,
    target_peer_id: String,
    target_thread_id: Option<String>,
    sender_thread_id: Option<String>,
    reason: &'static str,
) -> ActiveSessionSendResponse {
    active_session_send_response(
        ActiveSessionSendStatus::NotLoaded,
        message_id,
        target_peer_id,
        target_thread_id,
        sender_thread_id,
        Some(reason),
    )
}

fn active_session_send_response(
    status: ActiveSessionSendStatus,
    message_id: String,
    target_peer_id: String,
    target_thread_id: Option<String>,
    sender_thread_id: Option<String>,
    reason: Option<&'static str>,
) -> ActiveSessionSendResponse {
    ActiveSessionSendResponse {
        status,
        message_id,
        target_peer_id,
        target_thread_id,
        sender_thread_id,
        reason: reason.map(str::to_string),
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

fn active_session_delivery_error(
    err: crate::active_session_bridge::ActiveChannelAdapterError,
) -> JSONRPCErrorError {
    internal_error(format!("failed to deliver active session message: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_session_registry::ActivePeerKind;
    use crate::active_session_registry::ActivePeerOwner;
    use codex_protocol::AgentPath;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;

    #[test]
    fn target_lookup_accepts_peer_id_without_thread_id() {
        let lookup = active_session_target_lookup(
            Some("claude:session-1".to_string()),
            /*target_thread_id*/ None,
        )
        .expect("targetPeerId should be enough");

        assert_eq!(lookup.peer_id, "claude:session-1",);
        assert_eq!(lookup.thread_id, None);
        assert_eq!(
            lookup.not_loaded_reason,
            "target peer is not currently loaded; no offline delivery was attempted"
        );
    }

    #[test]
    fn target_lookup_requires_one_target_identifier() {
        let err = active_session_target_lookup(
            /*target_peer_id*/ None, /*target_thread_id*/ None,
        )
        .expect_err("missing target should be rejected");

        assert_eq!(
            err.message,
            "activeSession/send requires targetThreadId or targetPeerId"
        );
    }

    #[test]
    fn optional_target_thread_id_normalizes_uuid_text() {
        let thread_id = ThreadId::new().to_string();
        let normalized = optional_target_thread_id(Some(thread_id.to_ascii_uppercase()))
            .expect("valid uuid")
            .expect("thread id should be present");

        assert_eq!(normalized, thread_id);
    }

    #[test]
    fn api_active_session_peer_sets_auth_profile_kind() {
        let named_api_peer =
            api_active_session_peer(test_active_peer(ThreadId::new(), Some("work".to_string())));

        assert_eq!(named_api_peer.auth_profile.as_deref(), Some("work"));
        assert_eq!(named_api_peer.auth_profile_kind, AuthProfileKind::Named);

        let default_api_peer = api_active_session_peer(test_active_peer(ThreadId::new(), None));

        assert_eq!(default_api_peer.auth_profile, None);
        assert_eq!(default_api_peer.auth_profile_kind, AuthProfileKind::Default);
    }

    #[test]
    fn untrusted_sender_descriptor_bounds_peer_display_name() {
        let thread_id = ThreadId::new();
        let peer = ActivePeer {
            peer_id: thread_id.to_string(),
            kind: ActivePeerKind::CodewithSession,
            owner: ActivePeerOwner::LocalThread { thread_id },
            thread_id,
            session_id: "session".to_string(),
            cwd: AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
                .expect("temp dir is absolute"),
            display_name: Some("x".repeat(MAX_ACTIVE_SESSION_DESCRIPTOR_COMPONENT_BYTES + 100)),
            agent_path: None,
            auth_profile: None,
            process: None,
            capabilities: ActivePeerCapabilities::codewith_session(),
            last_seen_at: LastSeenAt::from_unix_seconds(/*seconds*/ 100),
        };

        let descriptor = untrusted_sender_descriptor(/*sender_label*/ None, Some(&peer));

        assert!(descriptor.ends_with("..."));
        assert!(
            descriptor.len()
                <= "unverified app-server client claiming ".len()
                    + MAX_ACTIVE_SESSION_DESCRIPTOR_COMPONENT_BYTES
        );
    }

    #[test]
    fn active_channel_envelope_records_claimed_sender_without_trusting_author() {
        let sender_thread_id = ThreadId::new();
        let target_thread_id = ThreadId::new();
        let sender_peer = ActivePeer {
            peer_id: sender_thread_id.to_string(),
            kind: ActivePeerKind::CodewithSession,
            owner: ActivePeerOwner::LocalThread {
                thread_id: sender_thread_id,
            },
            thread_id: sender_thread_id,
            session_id: "sender-session".to_string(),
            cwd: AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
                .expect("temp dir is absolute"),
            display_name: Some("Sender".to_string()),
            agent_path: Some("/claimed".to_string()),
            auth_profile: None,
            process: None,
            capabilities: ActivePeerCapabilities::codewith_session(),
            last_seen_at: LastSeenAt::from_unix_seconds(/*seconds*/ 100),
        };
        let target_peer = ActivePeer {
            peer_id: target_thread_id.to_string(),
            kind: ActivePeerKind::CodewithSession,
            owner: ActivePeerOwner::LocalThread {
                thread_id: target_thread_id,
            },
            thread_id: target_thread_id,
            session_id: "target-session".to_string(),
            cwd: AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
                .expect("temp dir is absolute"),
            display_name: Some("Target".to_string()),
            agent_path: Some("/target".to_string()),
            auth_profile: None,
            process: None,
            capabilities: ActivePeerCapabilities::codewith_session(),
            last_seen_at: LastSeenAt::from_unix_seconds(/*seconds*/ 100),
        };

        let envelope = active_channel_envelope(
            "msg-1",
            /*sender_label*/ None,
            Some(&sender_peer),
            &target_peer,
            "hello",
            ActiveChannelDeliveryMode::QueueOnly,
        );
        let communication = active_channel_communication(&envelope);

        assert_eq!(envelope.sender.id, "external-active-session");
        assert_eq!(
            envelope
                .claimed_sender
                .as_ref()
                .expect("claimed sender should be preserved")
                .id,
            sender_thread_id.to_string()
        );
        assert_eq!(communication.author, AgentPath::root());
        assert!(envelope.content.contains("claiming Sender"));
    }

    fn test_active_peer(thread_id: ThreadId, auth_profile: Option<String>) -> ActivePeer {
        ActivePeer {
            peer_id: thread_id.to_string(),
            kind: ActivePeerKind::CodewithSession,
            owner: ActivePeerOwner::LocalThread { thread_id },
            thread_id,
            session_id: "session".to_string(),
            cwd: AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
                .expect("temp dir is absolute"),
            display_name: None,
            agent_path: None,
            auth_profile,
            process: None,
            capabilities: ActivePeerCapabilities::codewith_session(),
            last_seen_at: LastSeenAt::from_unix_seconds(100),
        }
    }
}
