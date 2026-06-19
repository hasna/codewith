use codex_core::ThreadConfigSnapshot;
use codex_core::ThreadManager;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const BACKGROUND_AGENT_SESSION_SOURCE: &str = "background_agent";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LastSeenAt(i64);

impl LastSeenAt {
    pub(crate) fn from_unix_seconds(seconds: i64) -> Self {
        Self(seconds)
    }

    pub(crate) fn unix_seconds(self) -> i64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActivePeerFreshness {
    pub(crate) now: LastSeenAt,
    pub(crate) stale_after: Duration,
}

impl ActivePeerFreshness {
    pub(crate) fn new(now: LastSeenAt, stale_after: Duration) -> Self {
        Self { now, stale_after }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivePeerRegistration {
    pub(crate) peer_id: String,
    pub(crate) kind: ActivePeerKind,
    pub(crate) owner: ActivePeerOwner,
    pub(crate) thread_id: ThreadId,
    pub(crate) session_id: String,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) display_name: Option<String>,
    pub(crate) agent_path: Option<String>,
    pub(crate) process: Option<ActivePeerProcess>,
    pub(crate) capabilities: ActivePeerCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivePeer {
    pub(crate) peer_id: String,
    pub(crate) kind: ActivePeerKind,
    pub(crate) owner: ActivePeerOwner,
    pub(crate) thread_id: ThreadId,
    pub(crate) session_id: String,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) display_name: Option<String>,
    pub(crate) agent_path: Option<String>,
    pub(crate) process: Option<ActivePeerProcess>,
    pub(crate) capabilities: ActivePeerCapabilities,
    pub(crate) last_seen_at: LastSeenAt,
}

impl ActivePeer {
    fn from_registration(registration: ActivePeerRegistration, last_seen_at: LastSeenAt) -> Self {
        Self {
            peer_id: registration.peer_id,
            kind: registration.kind,
            owner: registration.owner,
            thread_id: registration.thread_id,
            session_id: registration.session_id,
            cwd: registration.cwd,
            display_name: registration.display_name,
            agent_path: registration.agent_path,
            process: registration.process,
            capabilities: registration.capabilities,
            last_seen_at,
        }
    }

    fn is_fresh(&self, freshness: ActivePeerFreshness) -> bool {
        let age_seconds = freshness
            .now
            .unix_seconds()
            .saturating_sub(self.last_seen_at.unix_seconds());
        let stale_after_seconds =
            i64::try_from(freshness.stale_after.as_secs()).unwrap_or(i64::MAX);
        age_seconds <= stale_after_seconds
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActivePeerOwner {
    LocalThread {
        thread_id: ThreadId,
    },
    #[allow(dead_code)] // Reserved for cross-process active-channel bridge adapters.
    BridgeAdapter {
        adapter_id: String,
    },
}

impl ActivePeerOwner {
    fn validate_registration(
        &self,
        peer_id: &str,
        thread_id: ThreadId,
    ) -> Result<(), ActivePeerRegistrationError> {
        match self {
            Self::LocalThread {
                thread_id: owner_thread_id,
            } => {
                if *owner_thread_id != thread_id {
                    return Err(ActivePeerRegistrationError::OwnerThreadMismatch {
                        peer_id: peer_id.to_string(),
                        owner_thread_id: *owner_thread_id,
                        thread_id,
                    });
                }
                if peer_id != thread_id.to_string() {
                    return Err(ActivePeerRegistrationError::LocalPeerIdMismatch {
                        peer_id: peer_id.to_string(),
                        thread_id,
                    });
                }
                Ok(())
            }
            Self::BridgeAdapter { adapter_id } => {
                if adapter_id.trim().is_empty() {
                    return Err(ActivePeerRegistrationError::BridgeAdapterIdEmpty {
                        peer_id: peer_id.to_string(),
                    });
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivePeerKind {
    CodewithSession,
    SpawnedAgent,
    #[allow(dead_code)] // Reserved for cross-process active-channel bridge adapters.
    BridgeAdapter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivePeerProcess {
    pub(crate) pid: u32,
    pub(crate) executable: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivePeerCapabilities {
    values: BTreeSet<ActivePeerCapability>,
}

impl ActivePeerCapabilities {
    pub(crate) fn new(values: impl IntoIterator<Item = ActivePeerCapability>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub(crate) fn codewith_session() -> Self {
        Self::new([
            ActivePeerCapability::ReceiveMessage,
            ActivePeerCapability::QueueMessage,
            ActivePeerCapability::TriggerTurn,
        ])
    }

    pub(crate) fn contains(&self, capability: ActivePeerCapability) -> bool {
        self.values.contains(&capability)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ActivePeerCapability {
    ReceiveMessage,
    QueueMessage,
    TriggerTurn,
    ClaudeChannelBridge,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ActivePeerLookupError {
    #[error("active peer not found: {peer_id}")]
    Unknown { peer_id: String },
    #[error("active peer is inactive: {peer_id}")]
    Inactive {
        peer_id: String,
        last_seen_at: LastSeenAt,
        now: LastSeenAt,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ActivePeerRegistrationError {
    #[error(
        "local active peer `{peer_id}` is owned by thread {owner_thread_id} but registered for thread {thread_id}"
    )]
    OwnerThreadMismatch {
        peer_id: String,
        owner_thread_id: ThreadId,
        thread_id: ThreadId,
    },
    #[error("local active peer id `{peer_id}` must match owning thread id {thread_id}")]
    LocalPeerIdMismatch {
        peer_id: String,
        thread_id: ThreadId,
    },
    #[error("bridge active peer `{peer_id}` must declare a non-empty adapter id")]
    BridgeAdapterIdEmpty { peer_id: String },
}

/// Registry of peers that can receive active-session messages.
///
/// This registry tracks liveness and routing metadata only. It does not own the
/// target session mailbox, persist pending messages, or provide offline
/// delivery. A peer returned from this registry is only a candidate for active
/// delivery; callers must still submit to the live session/runtime that owns the
/// peer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivePeerRegistry {
    peers: BTreeMap<String, ActivePeer>,
}

#[derive(Clone)]
pub(crate) struct ActivePeerDirectory {
    thread_manager: Arc<ThreadManager>,
    pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
}

impl ActivePeerDirectory {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
    ) -> Self {
        Self {
            thread_manager,
            pending_thread_unloads,
        }
    }

    pub(crate) async fn snapshot(
        &self,
        last_seen_at: LastSeenAt,
    ) -> Result<ActivePeerRegistry, CodexErr> {
        let mut registry = ActivePeerRegistry::default();
        let pending_thread_unloads = self.pending_thread_unloads.lock().await.clone();
        for thread_id in self.thread_manager.list_thread_ids().await {
            if pending_thread_unloads.contains(&thread_id) {
                continue;
            }
            let thread = match self.thread_manager.get_thread(thread_id).await {
                Ok(thread) => thread,
                Err(CodexErr::ThreadNotFound(_)) => continue,
                Err(err) => return Err(err),
            };
            let config_snapshot = thread.config_snapshot().await;
            let session_id = thread.session_configured().session_id.to_string();
            let peer_id = thread_id.to_string();
            registry
                .register(
                    active_peer_registration(thread_id, session_id, &config_snapshot),
                    last_seen_at,
                )
                .map_err(|err| {
                    CodexErr::Fatal(format!("failed to register active peer {thread_id}: {err}"))
                })?;
            let _ = registry.heartbeat(peer_id.as_str(), last_seen_at);
        }
        Ok(registry)
    }
}

impl ActivePeerRegistry {
    pub(crate) fn register(
        &mut self,
        registration: ActivePeerRegistration,
        last_seen_at: LastSeenAt,
    ) -> Result<ActivePeer, ActivePeerRegistrationError> {
        registration
            .owner
            .validate_registration(registration.peer_id.as_str(), registration.thread_id)?;
        let peer = ActivePeer::from_registration(registration, last_seen_at);
        self.peers.insert(peer.peer_id.clone(), peer.clone());
        Ok(peer)
    }

    pub(crate) fn heartbeat(
        &mut self,
        peer_id: &str,
        last_seen_at: LastSeenAt,
    ) -> Result<ActivePeer, ActivePeerLookupError> {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return Err(ActivePeerLookupError::Unknown {
                peer_id: peer_id.to_string(),
            });
        };
        peer.last_seen_at = last_seen_at;
        Ok(peer.clone())
    }

    pub(crate) fn get_active(
        &self,
        peer_id: &str,
        freshness: ActivePeerFreshness,
    ) -> Result<ActivePeer, ActivePeerLookupError> {
        let Some(peer) = self.peers.get(peer_id) else {
            return Err(ActivePeerLookupError::Unknown {
                peer_id: peer_id.to_string(),
            });
        };
        if !peer.is_fresh(freshness) {
            return Err(ActivePeerLookupError::Inactive {
                peer_id: peer_id.to_string(),
                last_seen_at: peer.last_seen_at,
                now: freshness.now,
            });
        }
        Ok(peer.clone())
    }

    pub(crate) fn list_active(&self, freshness: ActivePeerFreshness) -> Vec<ActivePeer> {
        self.peers
            .values()
            .filter(|peer| peer.is_fresh(freshness))
            .cloned()
            .collect()
    }

    pub(crate) fn remove_inactive(&mut self, freshness: ActivePeerFreshness) -> Vec<ActivePeer> {
        let mut inactive_peers = Vec::new();
        self.peers.retain(|_, peer| {
            if peer.is_fresh(freshness) {
                true
            } else {
                inactive_peers.push(peer.clone());
                false
            }
        });
        inactive_peers
    }
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
    let display_name =
        active_peer_display_name(&config_snapshot.session_source, agent_path.as_deref());
    ActivePeerRegistration {
        peer_id: thread_id.to_string(),
        kind: active_peer_kind(&config_snapshot.session_source),
        owner: ActivePeerOwner::LocalThread { thread_id },
        thread_id,
        session_id,
        cwd: config_snapshot.cwd.clone(),
        display_name,
        agent_path,
        process: None,
        capabilities: ActivePeerCapabilities::codewith_session(),
    }
}

fn active_peer_display_name(
    session_source: &SessionSource,
    agent_path: Option<&str>,
) -> Option<String> {
    session_source
        .get_nickname()
        .or_else(|| session_source.get_agent_role())
        .or_else(|| agent_path.map(str::to_string))
        .or_else(|| {
            is_background_agent_session_source(session_source)
                .then(|| "background agent".to_string())
        })
}

fn active_peer_kind(session_source: &SessionSource) -> ActivePeerKind {
    match session_source {
        SessionSource::SubAgent(SubAgentSource::ThreadSpawn { .. }) => ActivePeerKind::SpawnedAgent,
        source if is_background_agent_session_source(source) => ActivePeerKind::SpawnedAgent,
        _ => ActivePeerKind::CodewithSession,
    }
}

fn is_background_agent_session_source(session_source: &SessionSource) -> bool {
    matches!(
        session_source,
        SessionSource::Custom(source)
            if matches!(source.as_str(), BACKGROUND_AGENT_SESSION_SOURCE | "background-agent")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn test_cwd() -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
            .expect("temp dir is absolute")
    }

    fn registration(name: &str) -> ActivePeerRegistration {
        let thread_id = ThreadId::new();
        let peer_id = thread_id.to_string();
        ActivePeerRegistration {
            peer_id,
            kind: ActivePeerKind::CodewithSession,
            owner: ActivePeerOwner::LocalThread { thread_id },
            thread_id,
            session_id: format!("session-{name}"),
            cwd: test_cwd(),
            display_name: Some(format!("Peer {name}")),
            agent_path: None,
            process: Some(ActivePeerProcess {
                pid: 1234,
                executable: Some(PathBuf::from("/usr/bin/codewith")),
            }),
            capabilities: ActivePeerCapabilities::codewith_session(),
        }
    }

    fn freshness(now: i64, stale_after_seconds: u64) -> ActivePeerFreshness {
        ActivePeerFreshness::new(
            LastSeenAt::from_unix_seconds(now),
            Duration::from_secs(stale_after_seconds),
        )
    }

    fn register_valid(
        registry: &mut ActivePeerRegistry,
        registration: ActivePeerRegistration,
        last_seen_at: LastSeenAt,
    ) -> ActivePeer {
        registry
            .register(registration, last_seen_at)
            .expect("registration should be valid")
    }

    #[test]
    fn register_lists_active_peer() {
        let mut registry = ActivePeerRegistry::default();
        let registration = registration("thread-a");
        let expected =
            ActivePeer::from_registration(registration.clone(), LastSeenAt::from_unix_seconds(100));

        let registered = register_valid(
            &mut registry,
            registration,
            LastSeenAt::from_unix_seconds(100),
        );

        assert_eq!(registered, expected);
        assert_eq!(registry.list_active(freshness(110, 30)), vec![expected]);
    }

    #[test]
    fn register_existing_peer_updates_metadata_and_last_seen() {
        let mut registry = ActivePeerRegistry::default();
        let registration = registration("thread-a");
        register_valid(
            &mut registry,
            registration.clone(),
            LastSeenAt::from_unix_seconds(100),
        );

        let mut updated_registration = registration;
        updated_registration.display_name = Some("Renamed peer".to_string());
        updated_registration.agent_path = Some("agent/worker".to_string());
        let expected = ActivePeer::from_registration(
            updated_registration.clone(),
            LastSeenAt::from_unix_seconds(130),
        );

        let updated = register_valid(
            &mut registry,
            updated_registration,
            LastSeenAt::from_unix_seconds(130),
        );

        assert_eq!(updated, expected);
        assert_eq!(registry.list_active(freshness(130, 30)), vec![expected]);
    }

    #[test]
    fn registration_preserves_local_thread_owner() {
        let mut registry = ActivePeerRegistry::default();
        let registration = registration("thread-a");
        let expected_owner = registration.owner.clone();

        let registered = register_valid(
            &mut registry,
            registration,
            LastSeenAt::from_unix_seconds(100),
        );

        assert_eq!(registered.owner, expected_owner);
    }

    #[test]
    fn register_rejects_local_owner_thread_mismatch() {
        let mut registry = ActivePeerRegistry::default();
        let mut registration = registration("thread-a");
        let owner_thread_id = ThreadId::new();
        registration.owner = ActivePeerOwner::LocalThread {
            thread_id: owner_thread_id,
        };

        let err = registry
            .register(registration.clone(), LastSeenAt::from_unix_seconds(100))
            .expect_err("mismatched owner should be rejected");

        assert_eq!(
            err,
            ActivePeerRegistrationError::OwnerThreadMismatch {
                peer_id: registration.peer_id,
                owner_thread_id,
                thread_id: registration.thread_id,
            }
        );
    }

    #[test]
    fn register_rejects_local_peer_id_mismatch() {
        let mut registry = ActivePeerRegistry::default();
        let mut registration = registration("thread-a");
        registration.peer_id = "not-the-thread-id".to_string();

        let err = registry
            .register(registration.clone(), LastSeenAt::from_unix_seconds(100))
            .expect_err("mismatched local peer id should be rejected");

        assert_eq!(
            err,
            ActivePeerRegistrationError::LocalPeerIdMismatch {
                peer_id: registration.peer_id,
                thread_id: registration.thread_id,
            }
        );
    }

    #[test]
    fn register_accepts_bridge_peer_with_distinct_peer_id() {
        let mut registry = ActivePeerRegistry::default();
        let mut registration = registration("bridge-peer");
        registration.peer_id = "claude:session-1".to_string();
        registration.kind = ActivePeerKind::BridgeAdapter;
        registration.owner = ActivePeerOwner::BridgeAdapter {
            adapter_id: "claude-code".to_string(),
        };
        let expected =
            ActivePeer::from_registration(registration.clone(), LastSeenAt::from_unix_seconds(100));

        let registered = register_valid(
            &mut registry,
            registration,
            LastSeenAt::from_unix_seconds(100),
        );

        assert_eq!(registered, expected);
        assert_eq!(
            registry.get_active("claude:session-1", freshness(100, 0)),
            Ok(expected)
        );
    }

    #[test]
    fn register_rejects_empty_bridge_adapter_id() {
        let mut registry = ActivePeerRegistry::default();
        let mut registration = registration("bridge-peer");
        registration.peer_id = "claude:session-1".to_string();
        registration.kind = ActivePeerKind::BridgeAdapter;
        registration.owner = ActivePeerOwner::BridgeAdapter {
            adapter_id: "  ".to_string(),
        };

        let err = registry
            .register(registration.clone(), LastSeenAt::from_unix_seconds(100))
            .expect_err("empty bridge adapter id should be rejected");

        assert_eq!(
            err,
            ActivePeerRegistrationError::BridgeAdapterIdEmpty {
                peer_id: registration.peer_id,
            }
        );
    }

    #[test]
    fn heartbeat_updates_last_seen() {
        let mut registry = ActivePeerRegistry::default();
        let registration = registration("thread-a");
        let peer_id = registration.peer_id.clone();
        let mut expected = register_valid(
            &mut registry,
            registration,
            LastSeenAt::from_unix_seconds(100),
        );
        expected.last_seen_at = LastSeenAt::from_unix_seconds(150);

        let updated = registry
            .heartbeat(peer_id.as_str(), LastSeenAt::from_unix_seconds(150))
            .expect("peer exists");

        assert_eq!(updated, expected);
        assert_eq!(
            registry.get_active(peer_id.as_str(), freshness(170, 30)),
            Ok(expected)
        );
    }

    #[test]
    fn stale_peer_is_not_returned_as_active() {
        let mut registry = ActivePeerRegistry::default();
        let registration = registration("thread-a");
        let peer_id = registration.peer_id.clone();
        let registered = register_valid(
            &mut registry,
            registration,
            LastSeenAt::from_unix_seconds(100),
        );

        assert_eq!(registry.list_active(freshness(131, 30)), Vec::new());
        assert_eq!(
            registry.get_active(peer_id.as_str(), freshness(131, 30)),
            Err(ActivePeerLookupError::Inactive {
                peer_id,
                last_seen_at: registered.last_seen_at,
                now: LastSeenAt::from_unix_seconds(131),
            })
        );
    }

    #[test]
    fn unknown_peer_returns_unknown_error() {
        let registry = ActivePeerRegistry::default();

        assert_eq!(
            registry.get_active("missing", freshness(100, 30)),
            Err(ActivePeerLookupError::Unknown {
                peer_id: "missing".to_string(),
            })
        );
    }

    #[test]
    fn remove_inactive_prunes_stale_peers() {
        let mut registry = ActivePeerRegistry::default();
        let inactive = register_valid(
            &mut registry,
            registration("thread-a"),
            LastSeenAt::from_unix_seconds(100),
        );
        let active = register_valid(
            &mut registry,
            registration("thread-b"),
            LastSeenAt::from_unix_seconds(130),
        );

        assert_eq!(registry.remove_inactive(freshness(131, 30)), vec![inactive]);
        assert_eq!(registry.list_active(freshness(131, 30)), vec![active]);
    }

    #[test]
    fn capabilities_track_adapter_support_without_transport_dependency() {
        let capabilities = ActivePeerCapabilities::new([
            ActivePeerCapability::ReceiveMessage,
            ActivePeerCapability::ClaudeChannelBridge,
        ]);

        assert!(capabilities.contains(ActivePeerCapability::ClaudeChannelBridge));
        assert!(!capabilities.contains(ActivePeerCapability::TriggerTurn));
    }

    #[test]
    fn thread_spawn_session_source_maps_to_spawned_agent_kind() {
        let source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: ThreadId::new(),
            depth: 1,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
        });

        assert_eq!(active_peer_kind(&source), ActivePeerKind::SpawnedAgent);
        assert_eq!(
            active_peer_kind(&SessionSource::Cli),
            ActivePeerKind::CodewithSession
        );
    }

    #[test]
    fn background_agent_session_source_maps_to_spawned_agent_peer() {
        let source = SessionSource::Custom(BACKGROUND_AGENT_SESSION_SOURCE.to_string());

        assert_eq!(active_peer_kind(&source), ActivePeerKind::SpawnedAgent);
        assert_eq!(
            active_peer_display_name(&source, /*agent_path*/ None),
            Some("background agent".to_string())
        );
    }
}
