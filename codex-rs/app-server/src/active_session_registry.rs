use codex_protocol::ThreadId;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

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
pub(crate) enum ActivePeerKind {
    CodewithSession,
    SpawnedAgent,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivePeerRegistry {
    peers: BTreeMap<String, ActivePeer>,
}

impl ActivePeerRegistry {
    pub(crate) fn register(
        &mut self,
        registration: ActivePeerRegistration,
        last_seen_at: LastSeenAt,
    ) -> ActivePeer {
        let peer = ActivePeer::from_registration(registration, last_seen_at);
        self.peers.insert(peer.peer_id.clone(), peer.clone());
        peer
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
        let inactive_peer_ids = self
            .peers
            .iter()
            .filter_map(|(peer_id, peer)| (!peer.is_fresh(freshness)).then_some(peer_id.clone()))
            .collect::<Vec<_>>();
        inactive_peer_ids
            .into_iter()
            .filter_map(|peer_id| self.peers.remove(&peer_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn test_cwd() -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
            .expect("temp dir is absolute")
    }

    fn registration(peer_id: &str) -> ActivePeerRegistration {
        ActivePeerRegistration {
            peer_id: peer_id.to_string(),
            kind: ActivePeerKind::CodewithSession,
            thread_id: ThreadId::new(),
            session_id: format!("session-{peer_id}"),
            cwd: test_cwd(),
            display_name: Some(format!("Peer {peer_id}")),
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

    #[test]
    fn register_lists_active_peer() {
        let mut registry = ActivePeerRegistry::default();
        let registration = registration("thread-a");
        let expected =
            ActivePeer::from_registration(registration.clone(), LastSeenAt::from_unix_seconds(100));

        let registered = registry.register(registration, LastSeenAt::from_unix_seconds(100));

        assert_eq!(registered, expected.clone());
        assert_eq!(registry.list_active(freshness(110, 30)), vec![expected]);
    }

    #[test]
    fn register_existing_peer_updates_metadata_and_last_seen() {
        let mut registry = ActivePeerRegistry::default();
        registry.register(registration("thread-a"), LastSeenAt::from_unix_seconds(100));

        let mut updated_registration = registration("thread-a");
        updated_registration.display_name = Some("Renamed peer".to_string());
        updated_registration.agent_path = Some("agent/worker".to_string());
        let expected = ActivePeer::from_registration(
            updated_registration.clone(),
            LastSeenAt::from_unix_seconds(130),
        );

        let updated = registry.register(updated_registration, LastSeenAt::from_unix_seconds(130));

        assert_eq!(updated, expected.clone());
        assert_eq!(registry.list_active(freshness(130, 30)), vec![expected]);
    }

    #[test]
    fn heartbeat_updates_last_seen() {
        let mut registry = ActivePeerRegistry::default();
        let mut expected =
            registry.register(registration("thread-a"), LastSeenAt::from_unix_seconds(100));
        expected.last_seen_at = LastSeenAt::from_unix_seconds(150);

        let updated = registry
            .heartbeat("thread-a", LastSeenAt::from_unix_seconds(150))
            .expect("peer exists");

        assert_eq!(updated, expected.clone());
        assert_eq!(
            registry.get_active("thread-a", freshness(170, 30)),
            Ok(expected)
        );
    }

    #[test]
    fn stale_peer_is_not_returned_as_active() {
        let mut registry = ActivePeerRegistry::default();
        let registered =
            registry.register(registration("thread-a"), LastSeenAt::from_unix_seconds(100));

        assert_eq!(registry.list_active(freshness(131, 30)), Vec::new());
        assert_eq!(
            registry.get_active("thread-a", freshness(131, 30)),
            Err(ActivePeerLookupError::Inactive {
                peer_id: "thread-a".to_string(),
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
        let inactive =
            registry.register(registration("thread-a"), LastSeenAt::from_unix_seconds(100));
        let active =
            registry.register(registration("thread-b"), LastSeenAt::from_unix_seconds(130));

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
}
