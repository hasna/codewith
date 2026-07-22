//! Discovers subagent threads that belong to a primary thread by walking spawn-tree edges.
//!
//! When the TUI resumes or switches to an existing thread, it needs to populate
//! `AgentNavigationState` and `ChatWidget` metadata for every subagent that was spawned during
//! that thread's lifetime. The app server exposes a flat list of currently loaded threads via
//! `thread/loaded/list`, but the TUI must figure out which of those are descendants of the
//! primary thread.
//!
//! This module provides the pure, synchronous tree-walk that turns that flat list into the filtered
//! set of descendants. It intentionally has no async, no I/O, and no side effects so it can be
//! unit-tested in isolation.
//!
//! The walk starts from `primary_thread_id` and repeatedly follows
//! `SessionSource::SubAgent(ThreadSpawn { parent_thread_id, .. })` edges until no new children are
//! found. The primary thread itself is never included in the output.

use codex_app_server_protocol::Thread;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SubAgentSource;
use std::collections::HashMap;
use std::collections::HashSet;

/// A subagent thread discovered by the spawn-tree walk, carrying just enough metadata for the
/// TUI to register it in the navigation cache and rendering metadata map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSubagentThread {
    pub(crate) thread_id: ThreadId,
    pub(crate) agent_nickname: Option<String>,
    pub(crate) agent_role: Option<String>,
    pub(crate) thread_name: Option<String>,
    /// Parent thread id, used to reconstruct the agent tree path.
    pub(crate) parent_thread_id: Option<ThreadId>,
    /// Authoritative absolute agent path (for example `/root/backend_audit/db_check`), when the
    /// server composed one for this spawned thread.
    pub(crate) agent_path: Option<String>,
}

/// Walks the spawn tree rooted at `primary_thread_id` and returns every descendant subagent.
///
/// The walk is breadth-first over `SessionSource::SubAgent(ThreadSpawn { parent_thread_id })` edges.
/// Threads whose `source` is not a `ThreadSpawn`, or whose `parent_thread_id` does not chain back
/// to `primary_thread_id`, are excluded. The primary thread itself is never included.
///
/// Results are sorted by stringified thread id for deterministic output in tests and in the
/// navigation cache. Callers should not rely on this ordering for anything semantic; it exists
/// purely to make snapshot assertions stable.
///
/// If two threads claim the same parent, both are included. Cycles in the parent chain are not
/// possible because `ThreadId`s are server-assigned UUIDs and the server enforces acyclicity, but
/// the `included` set guards against re-visiting regardless.
pub(crate) fn find_loaded_subagent_threads_for_primary(
    threads: Vec<Thread>,
    primary_thread_id: ThreadId,
) -> Vec<LoadedSubagentThread> {
    let mut threads_by_id = HashMap::new();
    for thread in threads {
        let Ok(thread_id) = ThreadId::from_string(&thread.id) else {
            continue;
        };
        threads_by_id.insert(thread_id, thread);
    }

    let mut included = HashSet::new();
    let mut pending = vec![primary_thread_id];
    while let Some(parent_thread_id) = pending.pop() {
        for (thread_id, thread) in &threads_by_id {
            if included.contains(thread_id) {
                continue;
            }

            let Some(source_parent_thread_id) = thread_spawn_parent_thread_id(&thread.source)
            else {
                continue;
            };

            if source_parent_thread_id != parent_thread_id {
                continue;
            }

            included.insert(*thread_id);
            pending.push(*thread_id);
        }
    }

    let mut loaded_threads: Vec<LoadedSubagentThread> = included
        .into_iter()
        .filter_map(|thread_id| {
            threads_by_id
                .remove(&thread_id)
                .map(|thread| LoadedSubagentThread {
                    thread_id,
                    parent_thread_id: thread
                        .parent_thread_id
                        .as_deref()
                        .and_then(|id| ThreadId::from_string(id).ok())
                        .or_else(|| thread_spawn_parent_thread_id(&thread.source)),
                    agent_path: thread_spawn_agent_path(&thread.source),
                    agent_nickname: thread.agent_nickname,
                    agent_role: thread.agent_role,
                    thread_name: thread.name,
                })
        })
        .collect();
    loaded_threads.sort_by_key(|thread| thread.thread_id.to_string());
    loaded_threads
}

pub(crate) fn thread_spawn_parent_thread_id(
    source: &codex_app_server_protocol::SessionSource,
) -> Option<ThreadId> {
    let codex_app_server_protocol::SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        ..
    }) = source
    else {
        return None;
    };
    Some(*parent_thread_id)
}

/// Extracts the server-composed absolute agent path from a thread's session source, when present.
pub(crate) fn thread_spawn_agent_path(
    source: &codex_app_server_protocol::SessionSource,
) -> Option<String> {
    let codex_app_server_protocol::SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        agent_path,
        ..
    }) = source
    else {
        return None;
    };
    agent_path
        .as_ref()
        .map(|agent_path| agent_path.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::LoadedSubagentThread;
    use super::find_loaded_subagent_threads_for_primary;
    use codex_app_server_protocol::AuthProfileKind;
    use codex_app_server_protocol::SessionSource;
    use codex_app_server_protocol::Thread;
    use codex_app_server_protocol::ThreadStatus;
    use codex_protocol::ThreadId;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;

    fn test_thread(thread_id: ThreadId, source: SessionSource) -> Thread {
        Thread {
            id: thread_id.to_string(),
            session_id: thread_id.to_string(),
            forked_from_id: None,
            parent_thread_id: None,
            preview: String::new(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 0,
            updated_at: 0,
            status: ThreadStatus::Idle,
            path: None,
            cwd: test_path_buf("/tmp").abs(),
            cli_version: "0.0.0".to_string(),
            source,
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            auth_profile: None,
            auth_profile_kind: AuthProfileKind::Unknown,
            name: None,
            turns: Vec::new(),
        }
    }

    fn thread_spawn_source(
        parent_thread_id: ThreadId,
        depth: i32,
        agent_nickname: &str,
        agent_role: &str,
    ) -> SessionSource {
        serde_json::from_value(serde_json::json!({
            "subAgent": {
                "thread_spawn": {
                    "parent_thread_id": parent_thread_id.to_string(),
                    "depth": depth,
                    "agent_nickname": agent_nickname,
                    "agent_role": agent_role,
                }
            }
        }))
        .expect("valid subagent source")
    }

    #[test]
    fn finds_loaded_subagent_tree_for_primary_thread() {
        let primary_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid thread");
        let child_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid thread");
        let grandchild_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000003").expect("valid thread");
        let unrelated_parent_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000004").expect("valid thread");
        let unrelated_child_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000005").expect("valid thread");

        let mut child = test_thread(
            child_thread_id,
            thread_spawn_source(primary_thread_id, /*depth*/ 1, "Scout", "explorer"),
        );
        child.agent_nickname = Some("Scout".to_string());
        child.agent_role = Some("explorer".to_string());

        let mut grandchild = test_thread(
            grandchild_thread_id,
            thread_spawn_source(child_thread_id, /*depth*/ 2, "Atlas", "worker"),
        );
        grandchild.agent_nickname = Some("Atlas".to_string());
        grandchild.agent_role = Some("worker".to_string());

        let unrelated_child = test_thread(
            unrelated_child_id,
            thread_spawn_source(unrelated_parent_id, /*depth*/ 1, "Other", "researcher"),
        );

        let loaded = find_loaded_subagent_threads_for_primary(
            vec![
                test_thread(primary_thread_id, SessionSource::Cli),
                child,
                grandchild,
                unrelated_child,
            ],
            primary_thread_id,
        );

        assert_eq!(
            loaded,
            vec![
                LoadedSubagentThread {
                    thread_id: child_thread_id,
                    parent_thread_id: Some(primary_thread_id),
                    agent_path: None,
                    agent_nickname: Some("Scout".to_string()),
                    agent_role: Some("explorer".to_string()),
                    thread_name: None,
                },
                LoadedSubagentThread {
                    thread_id: grandchild_thread_id,
                    parent_thread_id: Some(child_thread_id),
                    agent_path: None,
                    agent_nickname: Some("Atlas".to_string()),
                    agent_role: Some("worker".to_string()),
                    thread_name: None,
                },
            ]
        );
    }

    #[test]
    fn captures_authoritative_agent_path_and_parent_for_loaded_subagent() {
        let primary_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid thread");
        let child_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid thread");

        let source: SessionSource = serde_json::from_value(serde_json::json!({
            "subAgent": {
                "thread_spawn": {
                    "parent_thread_id": primary_thread_id.to_string(),
                    "depth": 1,
                    "agent_path": "/root/backend_audit",
                    "agent_nickname": "Scout",
                    "agent_role": "explorer",
                }
            }
        }))
        .expect("valid subagent source");
        let mut child = test_thread(child_thread_id, source);
        child.agent_nickname = Some("Scout".to_string());
        child.agent_role = Some("explorer".to_string());

        let loaded = find_loaded_subagent_threads_for_primary(
            vec![test_thread(primary_thread_id, SessionSource::Cli), child],
            primary_thread_id,
        );

        assert_eq!(
            loaded,
            vec![LoadedSubagentThread {
                thread_id: child_thread_id,
                parent_thread_id: Some(primary_thread_id),
                agent_path: Some("/root/backend_audit".to_string()),
                agent_nickname: Some("Scout".to_string()),
                agent_role: Some("explorer".to_string()),
                thread_name: None,
            }]
        );
    }
}
