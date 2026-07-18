use super::active_session_processor::api_active_session_peer;
use super::thread_from_stored_thread;
use super::thread_processor::normalize_cwd_filters_for_method;
use super::thread_processor::thread_store_list_error;
use crate::active_session_registry::ActivePeerDirectory;
use crate::active_session_registry::ActivePeerFreshness;
use crate::active_session_registry::LastSeenAt;
use crate::error_code::internal_error;
use codex_app_server_protocol::ActiveSessionPeer;
#[cfg(test)]
use codex_app_server_protocol::ActiveSessionPeerKind;
#[cfg(test)]
use codex_app_server_protocol::AuthProfileKind;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::GitInfo as ApiGitInfo;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::LocalSession;
use codex_app_server_protocol::LocalSessionGitInfo;
use codex_app_server_protocol::LocalSessionListParams;
use codex_app_server_protocol::LocalSessionListResponse;
use codex_app_server_protocol::LocalSessionPeer;
use codex_app_server_protocol::LocalSessionRedaction;
use codex_app_server_protocol::LocalSessionStatus;
#[cfg(test)]
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::SortDirection;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadActiveFlag;
use codex_app_server_protocol::ThreadSortKey;
use codex_app_server_protocol::ThreadStatus;
use codex_protocol::ThreadId;
use codex_thread_store::ListThreadsParams as StoreListThreadsParams;
use codex_thread_store::SortDirection as StoreSortDirection;
use codex_thread_store::ThreadSortKey as StoreThreadSortKey;
#[cfg(test)]
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use super::ThreadRequestProcessor;

const LOCAL_SESSION_LIST_DEFAULT_LIMIT: usize = 25;
const LOCAL_SESSION_LIST_MAX_LIMIT: usize = 100;

impl ThreadRequestProcessor {
    pub(crate) async fn local_session_list(
        &self,
        params: LocalSessionListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.local_session_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(super) async fn local_session_list_inner(
        &self,
        params: LocalSessionListParams,
    ) -> Result<LocalSessionListResponse, JSONRPCErrorError> {
        let LocalSessionListParams {
            cursor,
            limit,
            sort_key,
            sort_direction,
            cwd,
            statuses,
            search_term,
            archived,
            use_state_db_only,
        } = params;
        let cwd_filters = normalize_cwd_filters_for_method("localSession/list", cwd)?;
        let requested_page_size = limit
            .map(|value| value as usize)
            .unwrap_or(LOCAL_SESSION_LIST_DEFAULT_LIMIT)
            .clamp(1, LOCAL_SESSION_LIST_MAX_LIMIT);
        let store_sort_key = match sort_key.unwrap_or(ThreadSortKey::CreatedAt) {
            ThreadSortKey::CreatedAt => StoreThreadSortKey::CreatedAt,
            ThreadSortKey::UpdatedAt => StoreThreadSortKey::UpdatedAt,
        };
        let store_sort_direction = match sort_direction.unwrap_or(SortDirection::Desc) {
            SortDirection::Asc => StoreSortDirection::Asc,
            SortDirection::Desc => StoreSortDirection::Desc,
        };
        let status_filter = statuses.and_then(|statuses| {
            (!statuses.is_empty()).then(|| statuses.into_iter().collect::<HashSet<_>>())
        });
        let live_overlay = self.local_session_live_overlay().await?;
        let auth_profile_account_labels = self.auth_profile_account_labels();
        let pending_thread_unloads = self.pending_thread_unloads.lock().await.clone();
        let fallback_provider = self.config.model_provider_id.as_str();
        let mut cursor_obj = cursor;
        let mut last_cursor = cursor_obj.clone();
        let mut remaining = requested_page_size;
        let mut data = Vec::with_capacity(requested_page_size);
        let mut next_cursor = None;

        while remaining > 0 {
            let page = self
                .thread_store
                .list_threads(StoreListThreadsParams {
                    page_size: remaining.min(LOCAL_SESSION_LIST_MAX_LIMIT),
                    cursor: cursor_obj.clone(),
                    sort_key: store_sort_key,
                    sort_direction: store_sort_direction,
                    allowed_sources: Vec::new(),
                    model_providers: Some(Vec::new()),
                    cwd_filters: cwd_filters.clone(),
                    archived: archived.unwrap_or(false),
                    search_term: search_term.clone(),
                    use_state_db_only,
                })
                .await
                .map_err(thread_store_list_error)?;

            for stored_thread in page.items {
                let model = stored_thread.model.clone();
                let agent_path = stored_thread.agent_path.clone();
                let (thread, _) =
                    thread_from_stored_thread(stored_thread, fallback_provider, &self.config.cwd);
                let local_session = api_local_session(
                    thread,
                    model,
                    agent_path,
                    &live_overlay,
                    &auth_profile_account_labels,
                    &pending_thread_unloads,
                );
                if status_filter
                    .as_ref()
                    .is_none_or(|statuses| statuses.contains(&local_session.status))
                {
                    data.push(local_session);
                    if data.len() >= requested_page_size {
                        break;
                    }
                }
            }

            remaining = requested_page_size.saturating_sub(data.len());
            next_cursor = page.next_cursor;
            if remaining == 0 {
                break;
            }
            let Some(cursor_val) = next_cursor.clone() else {
                break;
            };
            if last_cursor.as_ref() == Some(&cursor_val) {
                next_cursor = None;
                break;
            }
            last_cursor = Some(cursor_val.clone());
            cursor_obj = Some(cursor_val);
        }

        Ok(LocalSessionListResponse { data, next_cursor })
    }

    fn auth_profile_account_labels(&self) -> HashMap<String, String> {
        match codex_login::list_auth_profiles(
            self.config.codex_home.as_path(),
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => profiles
                .into_iter()
                .filter_map(|profile| {
                    let account_label = profile.email.or(profile.account_id)?;
                    Some((profile.name, account_label))
                })
                .collect(),
            Err(err) => {
                tracing::debug!("failed to load auth profile labels for local sessions: {err}");
                HashMap::new()
            }
        }
    }

    async fn local_session_live_overlay(
        &self,
    ) -> Result<BTreeMap<String, LocalSessionLivePeer>, JSONRPCErrorError> {
        let now = LastSeenAt::from_unix_seconds(time::OffsetDateTime::now_utc().unix_timestamp());
        let freshness = ActivePeerFreshness::new(now, Duration::from_secs(0));
        let registry = ActivePeerDirectory::new(
            Arc::clone(&self.thread_manager),
            Arc::clone(&self.pending_thread_unloads),
        )
        .snapshot(now)
        .await
        .map_err(|err| internal_error(format!("failed to list local active sessions: {err}")))?;
        let active_peers = registry
            .list_active(freshness)
            .into_iter()
            .map(api_active_session_peer)
            .map(|peer| (peer.thread_id.clone(), peer))
            .collect::<BTreeMap<_, _>>();
        let statuses = self
            .thread_watch_manager
            .loaded_statuses_for_threads(
                self.thread_manager
                    .list_thread_ids()
                    .await
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            )
            .await;

        let mut overlay = BTreeMap::new();
        for (thread_id, status) in statuses {
            let active_peer = active_peers.get(&thread_id).cloned();
            overlay.insert(
                thread_id,
                LocalSessionLivePeer {
                    active_peer,
                    status,
                },
            );
        }
        for (thread_id, active_peer) in active_peers {
            overlay.entry(thread_id).or_insert(LocalSessionLivePeer {
                active_peer: Some(active_peer),
                status: ThreadStatus::Idle,
            });
        }
        Ok(overlay)
    }
}

#[derive(Clone)]
struct LocalSessionLivePeer {
    active_peer: Option<ActiveSessionPeer>,
    status: ThreadStatus,
}

fn api_local_session(
    thread: Thread,
    model: Option<String>,
    thread_agent_path: Option<String>,
    live_overlay: &BTreeMap<String, LocalSessionLivePeer>,
    auth_profile_account_labels: &HashMap<String, String>,
    pending_thread_unloads: &HashSet<ThreadId>,
) -> LocalSession {
    let Thread {
        id,
        session_id: _,
        preview: _,
        forked_from_id: _,
        parent_thread_id: _,
        ephemeral: _,
        model_provider,
        auth_profile,
        auth_profile_kind,
        created_at,
        updated_at,
        status: _,
        path,
        cwd,
        cli_version: _,
        source,
        thread_source,
        agent_nickname,
        agent_role,
        git_info,
        name,
        turns: _,
    } = thread;
    let live_peer = live_overlay.get(id.as_str());
    let active_peer = live_peer.and_then(|live_peer| live_peer.active_peer.as_ref());
    let auth_profile = active_peer
        .and_then(|peer| peer.auth_profile.clone())
        .or(auth_profile);
    let auth_profile_kind = active_peer
        .map(|peer| peer.auth_profile_kind)
        .unwrap_or(auth_profile_kind);
    let account_label = auth_profile
        .as_ref()
        .and_then(|profile| auth_profile_account_labels.get(profile).cloned());
    let status = local_session_status(id.as_str(), live_peer, pending_thread_unloads);
    let active_flags = local_session_active_flags(live_peer);
    let (git_info, mut redactions) = local_session_git_info(git_info);
    if active_peer.is_some() {
        redactions.push(LocalSessionRedaction::ProcessDetails);
    }
    let display_name = active_peer
        .and_then(|peer| peer.display_name.clone())
        .or(name)
        .or(agent_nickname)
        .or(agent_role);
    let agent_path = active_peer
        .and_then(|peer| peer.agent_path.clone())
        .or(thread_agent_path);

    LocalSession {
        thread_id: id,
        runtime_session_id: active_peer.map(|peer| peer.session_id.clone()),
        peer: active_peer.map(local_session_peer),
        status,
        active_flags,
        cwd,
        display_name,
        agent_path,
        model_provider,
        model,
        auth_profile,
        auth_profile_kind,
        account_label,
        source,
        thread_source,
        created_at,
        updated_at,
        path,
        git_info,
        redactions,
    }
}

fn local_session_status(
    thread_id: &str,
    live_peer: Option<&LocalSessionLivePeer>,
    pending_thread_unloads: &HashSet<ThreadId>,
) -> LocalSessionStatus {
    if ThreadId::from_string(thread_id)
        .ok()
        .is_some_and(|thread_id| pending_thread_unloads.contains(&thread_id))
    {
        return LocalSessionStatus::Closing;
    }

    let Some(live_peer) = live_peer else {
        return LocalSessionStatus::NotLoaded;
    };
    if live_peer.active_peer.is_none() {
        return LocalSessionStatus::LoadedWithoutActivePeer;
    }
    match &live_peer.status {
        ThreadStatus::NotLoaded => LocalSessionStatus::LoadedWithoutActivePeer,
        ThreadStatus::Idle => LocalSessionStatus::Idle,
        ThreadStatus::SystemError => LocalSessionStatus::SystemError,
        ThreadStatus::Active { .. } => LocalSessionStatus::Active,
    }
}

fn local_session_active_flags(live_peer: Option<&LocalSessionLivePeer>) -> Vec<ThreadActiveFlag> {
    match live_peer.map(|live_peer| &live_peer.status) {
        Some(ThreadStatus::Active { active_flags }) => active_flags.clone(),
        Some(ThreadStatus::Idle | ThreadStatus::SystemError | ThreadStatus::NotLoaded) | None => {
            Vec::new()
        }
    }
}

fn local_session_peer(peer: &ActiveSessionPeer) -> LocalSessionPeer {
    LocalSessionPeer {
        peer_id: peer.peer_id.clone(),
        kind: peer.kind,
        capabilities: peer.capabilities.clone(),
        last_seen_at: peer.last_seen_at,
    }
}

fn local_session_git_info(
    git_info: Option<ApiGitInfo>,
) -> (Option<LocalSessionGitInfo>, Vec<LocalSessionRedaction>) {
    let Some(git_info) = git_info else {
        return (None, Vec::new());
    };
    let redactions = git_info
        .origin_url
        .is_some()
        .then_some(LocalSessionRedaction::GitOriginUrl)
        .into_iter()
        .collect();
    let api_git_info = LocalSessionGitInfo {
        sha: git_info.sha,
        branch: git_info.branch,
    };
    (Some(api_git_info), redactions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn live_peer(
        status: ThreadStatus,
        active_peer: Option<ActiveSessionPeer>,
    ) -> LocalSessionLivePeer {
        LocalSessionLivePeer {
            active_peer,
            status,
        }
    }

    #[test]
    fn local_session_status_maps_durable_and_live_states() {
        let thread_id = ThreadId::new();
        let id = thread_id.to_string();
        let active_peer = ActiveSessionPeer {
            peer_id: id.clone(),
            kind: ActiveSessionPeerKind::CodewithSession,
            thread_id: id.clone(),
            session_id: "session-1".to_string(),
            cwd: AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
                .expect("temp dir is absolute"),
            display_name: None,
            agent_path: None,
            capabilities: Vec::new(),
            auth_profile: Some("work".to_string()),
            auth_profile_kind: AuthProfileKind::Named,
            last_seen_at: 123,
        };

        assert_eq!(
            local_session_status(&id, /*live_peer*/ None, &HashSet::new()),
            LocalSessionStatus::NotLoaded
        );
        assert_eq!(
            local_session_status(
                &id,
                Some(&live_peer(ThreadStatus::Idle, Some(active_peer.clone()))),
                &HashSet::new()
            ),
            LocalSessionStatus::Idle
        );
        assert_eq!(
            local_session_status(
                &id,
                Some(&live_peer(
                    ThreadStatus::Active {
                        active_flags: Vec::new(),
                    },
                    Some(active_peer.clone())
                )),
                &HashSet::new()
            ),
            LocalSessionStatus::Active
        );
        assert_eq!(
            local_session_status(
                &id,
                Some(&live_peer(ThreadStatus::SystemError, Some(active_peer))),
                &HashSet::new()
            ),
            LocalSessionStatus::SystemError
        );
        assert_eq!(
            local_session_status(
                &id,
                Some(&live_peer(ThreadStatus::Idle, /*active_peer*/ None)),
                &HashSet::new()
            ),
            LocalSessionStatus::LoadedWithoutActivePeer
        );

        let pending_thread_unloads = HashSet::from([thread_id]);
        assert_eq!(
            local_session_status(&id, /*live_peer*/ None, &pending_thread_unloads),
            LocalSessionStatus::Closing
        );
    }

    #[test]
    fn api_local_session_overlays_live_auth_profile_kind_and_account_label() {
        let thread_id = ThreadId::new();
        let id = thread_id.to_string();
        let cwd = AbsolutePathBuf::from_absolute_path_checked(std::env::temp_dir())
            .expect("temp dir is absolute");
        let active_peer = ActiveSessionPeer {
            peer_id: id.clone(),
            kind: ActiveSessionPeerKind::CodewithSession,
            thread_id: id.clone(),
            session_id: "session-1".to_string(),
            cwd: cwd.clone(),
            display_name: None,
            agent_path: None,
            capabilities: Vec::new(),
            auth_profile: Some("work".to_string()),
            auth_profile_kind: AuthProfileKind::Named,
            last_seen_at: 123,
        };
        let thread = Thread {
            id: id.clone(),
            session_id: id.clone(),
            forked_from_id: None,
            parent_thread_id: None,
            preview: String::new(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 1,
            updated_at: 2,
            status: ThreadStatus::NotLoaded,
            path: None,
            cwd,
            cli_version: "test".to_string(),
            source: SessionSource::Cli,
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            auth_profile: None,
            auth_profile_kind: AuthProfileKind::Unknown,
            name: None,
            turns: Vec::new(),
        };
        let live_overlay = BTreeMap::from([(id, live_peer(ThreadStatus::Idle, Some(active_peer)))]);
        let auth_profile_account_labels =
            HashMap::from([("work".to_string(), "work@example.com".to_string())]);

        let local_session = api_local_session(
            thread,
            /*model*/ None,
            /*thread_agent_path*/ None,
            &live_overlay,
            &auth_profile_account_labels,
            &HashSet::new(),
        );

        assert_eq!(local_session.auth_profile.as_deref(), Some("work"));
        assert_eq!(local_session.auth_profile_kind, AuthProfileKind::Named);
        assert_eq!(
            local_session.account_label.as_deref(),
            Some("work@example.com")
        );
    }

    #[test]
    fn local_session_git_info_redacts_origin_url() {
        let (git_info, redactions) = local_session_git_info(Some(ApiGitInfo {
            sha: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            origin_url: Some("https://example.test/private.git".to_string()),
        }));

        assert_eq!(
            git_info,
            Some(LocalSessionGitInfo {
                sha: Some("abc123".to_string()),
                branch: Some("main".to_string()),
            })
        );
        assert_eq!(redactions, vec![LocalSessionRedaction::GitOriginUrl]);
    }
}
