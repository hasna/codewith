//! Thread routing, buffering, and app-server operation submission for the TUI app.
//!
//! This module manages active thread channels, routes server requests and notifications into those
//! channels, submits thread-scoped operations through the app server, and replays buffered events
//! when the visible thread changes.

use super::loaded_threads::thread_spawn_parent_thread_id;
use super::*;
use crate::app::app_server_event_targets::server_notification_thread_target;
use crate::app::app_server_event_targets::server_request_thread_id;
use crate::app::app_server_requests::AppServerRequestResolution;
use crate::session_resume::read_session_model;
use codex_app_server_protocol::ThreadExternalAgentStartStatus;

/// Maximum number of `/`-joined segments shown in a rendered agent tree path before the middle is
/// collapsed to a single ellipsis segment (`root/…/<parent>/<leaf>`).
const AGENT_TREE_PATH_MAX_SEGMENTS: usize = 4;
/// Maximum graphemes shown per tree-path segment so a wait/spawn row cannot overflow its line.
const AGENT_TREE_PATH_SEGMENT_MAX_GRAPHEMES: usize = 24;
/// Segment inserted when the middle of a deep path is collapsed or when the chain to the root
/// cannot be confirmed (cycle or orphaned ancestor).
const AGENT_TREE_PATH_ELLIPSIS: &str = "…";
/// Synthetic top segment for a resolved agent tree path.
const AGENT_TREE_PATH_ROOT: &str = "root";

impl App {
    pub(super) async fn shutdown_current_thread(&mut self, app_server: &mut AppServerSession) {
        if let Some(thread_id) = self.chat_widget.thread_id() {
            // Clear any in-flight rollback guard when switching threads.
            self.backtrack.pending_rollback = None;
            if let Err(err) = app_server.thread_unsubscribe(thread_id).await {
                tracing::warn!("failed to unsubscribe thread {thread_id}: {err}");
            }
            self.abort_thread_event_listener(thread_id);
        }
    }

    pub(super) fn abort_thread_event_listener(&mut self, thread_id: ThreadId) {
        if let Some(handle) = self.thread_event_listener_tasks.remove(&thread_id) {
            handle.abort();
        }
    }

    pub(super) fn abort_all_thread_event_listeners(&mut self) {
        for handle in self
            .thread_event_listener_tasks
            .drain()
            .map(|(_, handle)| handle)
        {
            handle.abort();
        }
    }

    pub(super) fn ensure_thread_channel(&mut self, thread_id: ThreadId) -> &mut ThreadEventChannel {
        self.thread_event_channels
            .entry(thread_id)
            .or_insert_with(|| ThreadEventChannel::new(THREAD_EVENT_CHANNEL_CAPACITY))
    }

    pub(super) async fn set_thread_active(&mut self, thread_id: ThreadId, active: bool) {
        if let Some(channel) = self.thread_event_channels.get_mut(&thread_id) {
            let mut store = channel.store.lock().await;
            store.active = active;
        }
    }

    pub(super) async fn activate_thread_channel(&mut self, thread_id: ThreadId) {
        if self.active_thread_id.is_some() {
            return;
        }
        self.set_thread_active(thread_id, /*active*/ true).await;
        let receiver = if let Some(channel) = self.thread_event_channels.get_mut(&thread_id) {
            channel.receiver.take()
        } else {
            None
        };
        self.active_thread_id = Some(thread_id);
        self.active_thread_rx = receiver;
        self.refresh_pending_thread_approvals().await;
    }

    pub(super) async fn store_active_thread_receiver(&mut self) {
        let Some(active_id) = self.active_thread_id else {
            return;
        };
        let input_state = self.chat_widget.capture_thread_input_state();
        if let Some(channel) = self.thread_event_channels.get_mut(&active_id) {
            let receiver = self.active_thread_rx.take();
            let mut store = channel.store.lock().await;
            store.active = false;
            store.input_state = input_state;
            if let Some(receiver) = receiver {
                channel.receiver = Some(receiver);
            }
        }
    }

    pub(super) async fn activate_thread_for_replay(
        &mut self,
        thread_id: ThreadId,
    ) -> Option<(mpsc::Receiver<ThreadBufferedEvent>, ThreadEventSnapshot)> {
        let channel = self.thread_event_channels.get_mut(&thread_id)?;
        let receiver = channel.receiver.take()?;
        let mut store = channel.store.lock().await;
        store.active = true;
        let snapshot = store.snapshot();
        Some((receiver, snapshot))
    }

    pub(super) async fn clear_active_thread(&mut self) {
        if let Some(active_id) = self.active_thread_id.take() {
            self.set_thread_active(active_id, /*active*/ false).await;
        }
        self.active_thread_rx = None;
        self.refresh_pending_thread_approvals().await;
    }

    pub(super) async fn note_thread_outbound_op(&mut self, thread_id: ThreadId, op: &AppCommand) {
        let Some(channel) = self.thread_event_channels.get(&thread_id) else {
            return;
        };
        let mut store = channel.store.lock().await;
        store.note_outbound_op(op);
    }

    pub(super) async fn note_active_thread_outbound_op(&mut self, op: &AppCommand) {
        if !ThreadEventStore::op_can_change_pending_replay_state(op) {
            return;
        }
        let Some(thread_id) = self.active_thread_id else {
            return;
        };
        self.note_thread_outbound_op(thread_id, op).await;
    }

    pub(super) async fn active_turn_id_for_thread(&self, thread_id: ThreadId) -> Option<String> {
        let channel = self.thread_event_channels.get(&thread_id)?;
        let store = channel.store.lock().await;
        store.active_turn_id().map(ToOwned::to_owned)
    }

    pub(super) fn thread_label(&self, thread_id: ThreadId) -> String {
        let is_primary = self.primary_thread_id == Some(thread_id);
        let fallback_label = if is_primary {
            "Main [default]".to_string()
        } else {
            let thread_id = thread_id.to_string();
            let short_id: String = thread_id.chars().take(8).collect();
            format!("Agent ({short_id})")
        };
        if let Some(entry) = self.agent_navigation.get(&thread_id) {
            let label = format_agent_picker_entry_name(entry, is_primary);
            if label == "Agent" {
                let thread_id = thread_id.to_string();
                let short_id: String = thread_id.chars().take(8).collect();
                format!("{label} ({short_id})")
            } else {
                label
            }
        } else {
            fallback_label
        }
    }

    /// Resolves a readable hierarchical path (for example `root/backend_audit/db_check`) for a
    /// thread, guaranteeing the result never contains a full 36-character thread UUID.
    ///
    /// Resolution order:
    /// 1. an authoritative server-composed `agent_path` (leading `/` stripped, and the leaf segment
    ///    swapped for the thread's nickname when one is known);
    /// 2. otherwise a parent-chain walk up to the root, labeling each node by nickname -> `[role]`
    ///    -> 8-character short id, with a visited-set guard against cycles and an ellipsis prefix
    ///    when the chain to the root cannot be confirmed;
    /// 3. otherwise the single-line `thread_label`, which itself never contains a full UUID.
    pub(super) fn agent_tree_path(&self, thread_id: ThreadId) -> String {
        if let Some(agent_path) = self.agent_navigation.agent_path(&thread_id) {
            return self.format_authoritative_agent_path(agent_path, thread_id);
        }
        if let Some(tree_path) = self.walk_agent_tree_path(thread_id) {
            return tree_path;
        }
        self.thread_label(thread_id)
    }

    /// Formats a server-composed absolute `agent_path` for display.
    ///
    /// Strips the leading `/`, and when the path has more than just the `root` segment, swaps the
    /// leaf for the thread's nickname when one is available so the most specific segment stays
    /// human-readable.
    fn format_authoritative_agent_path(&self, agent_path: &str, thread_id: ThreadId) -> String {
        let mut segments: Vec<String> = agent_path
            .strip_prefix('/')
            .unwrap_or(agent_path)
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect();
        if segments.is_empty() {
            segments.push(AGENT_TREE_PATH_ROOT.to_string());
        }
        if segments.len() > 1
            && let Some(nickname) = self.agent_nickname(thread_id)
            && let Some(leaf) = segments.last_mut()
        {
            *leaf = nickname;
        }
        finalize_agent_tree_path(segments)
    }

    /// Reconstructs a tree path by walking cached parent edges from `thread_id` up to the root.
    ///
    /// Returns `None` when the thread has no known parent edge so the caller can fall back to
    /// `thread_label`. Terminates on a cycle, a self-parent, or an ancestor edge that points at a
    /// thread with no cached metadata, prefixing the path with an ellipsis in those cases because
    /// the chain to the root could not be confirmed.
    fn walk_agent_tree_path(&self, thread_id: ThreadId) -> Option<String> {
        self.agent_navigation.parent(&thread_id)?;

        let mut ancestors: Vec<ThreadId> = Vec::new();
        let mut visited: std::collections::HashSet<ThreadId> =
            std::collections::HashSet::from([thread_id]);
        let mut current = thread_id;
        let mut reached_root = false;

        while let Some(parent_thread_id) = self.agent_navigation.parent(&current) {
            if Some(parent_thread_id) == self.primary_thread_id {
                reached_root = true;
                break;
            }
            if !visited.insert(parent_thread_id) {
                // Cycle or self-parent: stop before revisiting a node.
                break;
            }
            if !self.agent_navigation.is_tracked(&parent_thread_id) {
                // Edge points at a thread we have never seen metadata for.
                break;
            }
            ancestors.push(parent_thread_id);
            current = parent_thread_id;
        }

        let mut segments: Vec<String> = Vec::with_capacity(ancestors.len() + 2);
        segments.push(if reached_root {
            AGENT_TREE_PATH_ROOT.to_string()
        } else {
            AGENT_TREE_PATH_ELLIPSIS.to_string()
        });
        for ancestor in ancestors.iter().rev() {
            segments.push(self.agent_node_label(*ancestor));
        }
        segments.push(self.agent_node_label(thread_id));

        Some(finalize_agent_tree_path(segments))
    }

    /// Returns the single-segment label for a node in a tree path: nickname, else `[role]`, else an
    /// 8-character short id. Never returns a full UUID.
    fn agent_node_label(&self, thread_id: ThreadId) -> String {
        if let Some(entry) = self.agent_navigation.get(&thread_id) {
            if let Some(nickname) = entry
                .agent_nickname
                .as_deref()
                .map(str::trim)
                .filter(|nickname| !nickname.is_empty())
            {
                return nickname.to_string();
            }
            if let Some(role) = entry
                .agent_role
                .as_deref()
                .map(str::trim)
                .filter(|role| !role.is_empty())
            {
                return format!("[{role}]");
            }
        }
        crate::multi_agents::short_thread_id(thread_id)
    }

    /// Returns the trimmed, non-empty nickname for a thread, if one is cached.
    fn agent_nickname(&self, thread_id: ThreadId) -> Option<String> {
        self.agent_navigation
            .get(&thread_id)
            .and_then(|entry| entry.agent_nickname.as_deref())
            .map(str::trim)
            .filter(|nickname| !nickname.is_empty())
            .map(str::to_string)
    }

    /// Returns the thread whose transcript is currently on screen.
    ///
    /// `active_thread_id` is the source of truth during steady state, but the widget can briefly
    /// lag behind thread bookkeeping during transitions. The footer label and adjacent-thread
    /// navigation both follow what the user is actually looking at, not whichever thread most
    /// recently began switching.
    pub(super) fn current_displayed_thread_id(&self) -> Option<ThreadId> {
        self.active_thread_id.or(self.chat_widget.thread_id())
    }

    pub(super) fn ignore_same_thread_resume(
        &mut self,
        target_session: &crate::resume_picker::SessionTarget,
    ) -> bool {
        if self.active_thread_id != Some(target_session.thread_id) {
            return false;
        };

        self.chat_widget.add_info_message(
            format!("Already viewing {}.", target_session.display_label()),
            /*hint*/ None,
        );
        true
    }

    /// Mirrors the visible thread into the contextual footer row.
    ///
    /// The footer sometimes shows ambient context instead of an instructional hint. In multi-agent
    /// sessions, that contextual row includes the currently viewed agent label. The label is
    /// intentionally hidden until there is more than one known thread so single-thread sessions do
    /// not spend footer space restating that the user is already on the main conversation.
    pub(super) fn sync_active_agent_label(&mut self) {
        let label = self
            .agent_navigation
            .active_agent_label(self.current_displayed_thread_id(), self.primary_thread_id);
        self.chat_widget.set_active_agent_label(label);
        self.sync_side_thread_ui();
    }

    pub(super) async fn thread_cwd(&self, thread_id: ThreadId) -> Option<AbsolutePathBuf> {
        let channel = self.thread_event_channels.get(&thread_id)?;
        let store = channel.store.lock().await;
        store.session.as_ref().map(|session| session.cwd.clone())
    }

    async fn thread_file_change_changes(
        &self,
        thread_id: ThreadId,
        turn_id: &str,
        item_id: &str,
    ) -> Option<Vec<codex_app_server_protocol::FileUpdateChange>> {
        let channel = self.thread_event_channels.get(&thread_id)?;
        let store = channel.store.lock().await;
        store.file_change_changes(turn_id, item_id)
    }

    pub(super) async fn interactive_request_for_thread_request(
        &self,
        thread_id: ThreadId,
        request: &ServerRequest,
    ) -> Option<ThreadInteractiveRequest> {
        let thread_label = Some(self.thread_label(thread_id));
        match request {
            ServerRequest::CommandExecutionRequestApproval { params, .. } => {
                let network_approval_context = params.network_approval_context.clone();
                let additional_permissions = params.additional_permissions.clone();
                let proposed_execpolicy_amendment = params.proposed_execpolicy_amendment.clone();
                let proposed_network_policy_amendments =
                    params.proposed_network_policy_amendments.clone();
                Some(ThreadInteractiveRequest::Approval(ApprovalRequest::Exec {
                    thread_id,
                    thread_label,
                    id: params
                        .approval_id
                        .clone()
                        .unwrap_or_else(|| params.item_id.clone()),
                    command: params
                        .command
                        .as_deref()
                        .map(split_command_string)
                        .unwrap_or_default(),
                    reason: params.reason.clone(),
                    available_decisions: params.available_decisions.clone().unwrap_or_else(|| {
                        default_exec_approval_decisions(
                            network_approval_context.as_ref(),
                            proposed_execpolicy_amendment.as_ref(),
                            proposed_network_policy_amendments.as_deref(),
                            additional_permissions.as_ref(),
                        )
                    }),
                    network_approval_context,
                    additional_permissions,
                }))
            }
            ServerRequest::FileChangeRequestApproval { params, .. } => Some(
                ThreadInteractiveRequest::Approval(ApprovalRequest::ApplyPatch {
                    thread_id,
                    thread_label,
                    id: params.item_id.clone(),
                    reason: params.reason.clone(),
                    cwd: self
                        .thread_cwd(thread_id)
                        .await
                        .unwrap_or_else(|| self.config.cwd.clone()),
                    changes: self
                        .thread_file_change_changes(thread_id, &params.turn_id, &params.item_id)
                        .await
                        .map(crate::app_server_approval_conversions::file_update_changes_to_display)
                        .unwrap_or_default(),
                }),
            ),
            ServerRequest::McpServerElicitationRequest { request_id, params } => {
                if let Some(params) = AppLinkViewParams::from_url_app_server_request(
                    thread_id,
                    &params.server_name,
                    request_id.clone(),
                    &params.request,
                ) {
                    Some(ThreadInteractiveRequest::AppLink(params))
                } else if let Some(request) =
                    McpServerElicitationFormRequest::from_app_server_request(
                        thread_id,
                        request_id.clone(),
                        params.clone(),
                    )
                {
                    Some(ThreadInteractiveRequest::McpServerElicitation(request))
                } else {
                    match &params.request {
                        codex_app_server_protocol::McpServerElicitationRequest::Form {
                            message,
                            ..
                        } => Some(ThreadInteractiveRequest::Approval(
                            ApprovalRequest::McpElicitation {
                                thread_id,
                                thread_label,
                                server_name: params.server_name.clone(),
                                request_id: request_id.clone(),
                                message: message.clone(),
                            },
                        )),
                        codex_app_server_protocol::McpServerElicitationRequest::Url { .. } => {
                            self.app_event_tx.resolve_elicitation(
                                thread_id,
                                params.server_name.clone(),
                                request_id.clone(),
                                codex_app_server_protocol::McpServerElicitationAction::Decline,
                                /*content*/ None,
                                /*meta*/ None,
                            );
                            None
                        }
                    }
                }
            }
            ServerRequest::PermissionsRequestApproval { params, .. } => Some(
                ThreadInteractiveRequest::Approval(ApprovalRequest::Permissions {
                    thread_id,
                    thread_label,
                    call_id: params.item_id.clone(),
                    environment_id: params.environment_id.clone(),
                    reason: params.reason.clone(),
                    permissions: params.permissions.clone().into(),
                }),
            ),
            _ => None,
        }
    }

    pub(super) fn push_thread_interactive_request(&mut self, request: ThreadInteractiveRequest) {
        match request {
            ThreadInteractiveRequest::AppLink(params) => {
                self.chat_widget.open_app_link_view(params);
            }
            ThreadInteractiveRequest::Approval(request) => {
                self.render_inactive_patch_preview(&request);
                self.chat_widget.push_approval_request(request);
            }
            ThreadInteractiveRequest::McpServerElicitation(request) => {
                self.chat_widget
                    .push_mcp_server_elicitation_request(request);
            }
        }
    }

    fn render_inactive_patch_preview(&mut self, request: &ApprovalRequest) {
        let ApprovalRequest::ApplyPatch {
            thread_label,
            cwd,
            changes,
            ..
        } = request
        else {
            return;
        };
        if thread_label.is_none() || changes.is_empty() {
            return;
        }
        self.chat_widget
            .add_to_history(history_cell::new_patch_event(changes.clone(), cwd));
    }

    pub(super) async fn pending_inactive_thread_requests(&self) -> Vec<(ThreadId, ServerRequest)> {
        let channels: Vec<(ThreadId, Arc<Mutex<ThreadEventStore>>)> = self
            .thread_event_channels
            .iter()
            .map(|(thread_id, channel)| (*thread_id, Arc::clone(&channel.store)))
            .collect();

        let mut requests = Vec::new();
        for (thread_id, store) in channels {
            if Some(thread_id) == self.active_thread_id {
                continue;
            }

            let store = store.lock().await;
            requests.extend(
                store
                    .pending_replay_requests()
                    .into_iter()
                    .map(|request| (thread_id, request)),
            );
        }
        requests
    }

    pub(super) async fn surface_pending_inactive_thread_interactive_requests(&mut self) {
        if self.active_side_parent_thread_id().is_some() {
            return;
        }

        let requests = self.pending_inactive_thread_requests().await;
        for (thread_id, request) in requests {
            if let Some(request) = self
                .interactive_request_for_thread_request(thread_id, &request)
                .await
            {
                self.push_thread_interactive_request(request);
            }
        }
    }

    pub(super) async fn submit_active_thread_op(
        &mut self,
        app_server: &mut AppServerSession,
        op: AppCommand,
    ) -> Result<()> {
        let Some(thread_id) = self.active_thread_id else {
            self.chat_widget
                .add_error_message("No active thread is available.".to_string());
            return Ok(());
        };

        self.submit_thread_op(app_server, thread_id, op).await
    }

    pub(super) async fn submit_thread_op(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        op: AppCommand,
    ) -> Result<()> {
        crate::session_log::log_outbound_op(&op);

        if self
            .try_resolve_app_server_request(app_server, thread_id, &op)
            .await?
        {
            return Ok(());
        }

        if self
            .try_submit_active_thread_op_via_app_server(app_server, thread_id, &op)
            .await?
        {
            if ThreadEventStore::op_can_change_pending_replay_state(&op) {
                self.note_thread_outbound_op(thread_id, &op).await;
                self.refresh_pending_thread_approvals().await;
                self.refresh_side_parent_status_from_store(thread_id).await;
            }
            return Ok(());
        }

        self.chat_widget
            .add_error_message(format!("Not available in TUI yet for thread {thread_id}."));
        Ok(())
    }

    /// Persist prompt text in the local cross-session message history.
    pub(super) fn append_message_history_entry(&self, thread_id: ThreadId, text: String) {
        let history_config = codex_message_history::HistoryConfig::new(
            self.chat_widget.config_ref().codex_home.clone(),
            &self.chat_widget.config_ref().history,
        );
        tokio::spawn(async move {
            if let Err(err) =
                codex_message_history::append_entry(&text, thread_id, &history_config).await
            {
                tracing::warn!(
                    thread_id = %thread_id,
                    error = %err,
                    "failed to append to message history"
                );
            }
        });
    }

    /// Fetch one local cross-session message history entry for the requesting thread.
    pub(super) async fn lookup_message_history_entry(
        &mut self,
        thread_id: ThreadId,
        offset: usize,
        log_id: u64,
    ) -> Result<()> {
        let history_config = codex_message_history::HistoryConfig::new(
            self.chat_widget.config_ref().codex_home.clone(),
            &self.chat_widget.config_ref().history,
        );
        let app_event_tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let entry_opt = tokio::task::spawn_blocking(move || {
                codex_message_history::lookup(log_id, offset, &history_config)
            })
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "history lookup task failed");
                None
            });

            app_event_tx.send(AppEvent::ThreadHistoryEntryResponse {
                thread_id,
                event: HistoryLookupResponse {
                    offset,
                    log_id,
                    entry: entry_opt.map(|entry| entry.text),
                },
            });
        });
        Ok(())
    }

    pub(super) async fn try_submit_active_thread_op_via_app_server(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        op: &AppCommand,
    ) -> Result<bool> {
        match op {
            AppCommand::Interrupt { .. } => {
                if let Some(turn_id) = self.active_turn_id_for_thread(thread_id).await {
                    app_server.turn_interrupt(thread_id, turn_id).await?;
                } else {
                    app_server.startup_interrupt(thread_id).await?;
                }
                Ok(true)
            }
            AppCommand::UserTurn {
                items,
                cwd,
                approval_policy,
                approvals_reviewer,
                active_permission_profile,
                model_provider,
                model,
                effort,
                summary,
                service_tier,
                final_output_json_schema,
                additional_context,
                collaboration_mode,
                personality,
            } => {
                let mut should_start_turn = true;
                if let Some(turn_id) = self.active_turn_id_for_thread(thread_id).await {
                    let mut steer_turn_id = turn_id;
                    let mut retried_after_turn_mismatch = false;
                    loop {
                        match app_server
                            .turn_steer(
                                thread_id,
                                steer_turn_id.clone(),
                                items.to_vec(),
                                additional_context.clone(),
                            )
                            .await
                        {
                            Ok(_) => return Ok(true),
                            Err(error) => {
                                if let Some(turn_error) =
                                    active_turn_not_steerable_turn_error(&error)
                                {
                                    if !self.chat_widget.enqueue_rejected_steer() {
                                        self.chat_widget.add_error_message(turn_error.message);
                                    }
                                    return Ok(true);
                                }
                                match active_turn_steer_race(&error) {
                                    Some(ActiveTurnSteerRace::Missing) => {
                                        if let Some(channel) =
                                            self.thread_event_channels.get(&thread_id)
                                        {
                                            let mut store = channel.store.lock().await;
                                            store.clear_active_turn_id();
                                        }
                                        should_start_turn = true;
                                        break;
                                    }
                                    Some(ActiveTurnSteerRace::ExpectedTurnMismatch {
                                        actual_turn_id,
                                    }) if !retried_after_turn_mismatch
                                        && actual_turn_id != steer_turn_id =>
                                    {
                                        // Review flows can swap the active turn before the TUI
                                        // processes the corresponding notification. Retry once with
                                        // the server-reported turn id so non-steerable review turns
                                        // still fall through to the existing queueing behavior.
                                        if let Some(channel) =
                                            self.thread_event_channels.get(&thread_id)
                                        {
                                            let mut store = channel.store.lock().await;
                                            store.active_turn_id = Some(actual_turn_id.clone());
                                        }
                                        steer_turn_id = actual_turn_id;
                                        retried_after_turn_mismatch = true;
                                    }
                                    Some(ActiveTurnSteerRace::ExpectedTurnMismatch {
                                        actual_turn_id,
                                    }) => {
                                        if let Some(channel) =
                                            self.thread_event_channels.get(&thread_id)
                                        {
                                            let mut store = channel.store.lock().await;
                                            store.active_turn_id = Some(actual_turn_id);
                                        }
                                        return Err(error.into());
                                    }
                                    None => return Err(error.into()),
                                }
                            }
                        }
                    }
                }
                if should_start_turn {
                    let config = self.chat_widget.config_ref();
                    let approvals_reviewer =
                        approvals_reviewer.unwrap_or(config.approvals_reviewer);
                    let permissions_override = Self::turn_permissions_override_from_config(
                        config,
                        active_permission_profile.as_ref(),
                        self.runtime_permission_profile_override
                            .as_ref()
                            .map(|profile| &profile.permission_profile),
                    );
                    app_server
                        .turn_start(
                            thread_id,
                            items.to_vec(),
                            cwd.clone(),
                            *approval_policy,
                            approvals_reviewer,
                            permissions_override,
                            config.permissions.user_visible_workspace_roots(),
                            Some(model_provider.clone()),
                            model.to_string(),
                            effort.clone(),
                            *summary,
                            service_tier.clone(),
                            additional_context.clone(),
                            collaboration_mode.clone(),
                            *personality,
                            final_output_json_schema.clone(),
                        )
                        .await?;
                }
                Ok(true)
            }
            AppCommand::ListSkills { cwds, force_reload } => {
                self.handle_skills_list_result(
                    app_server
                        .skills_list(codex_app_server_protocol::SkillsListParams {
                            cwds: cwds.clone(),
                            force_reload: *force_reload,
                        })
                        .await,
                    "failed to refresh skills",
                );
                Ok(true)
            }
            AppCommand::Compact => {
                app_server.thread_compact_start(thread_id).await?;
                Ok(true)
            }
            AppCommand::SetThreadName { name } => {
                app_server
                    .thread_set_name(thread_id, name.to_string())
                    .await?;
                self.agent_navigation
                    .set_thread_name(thread_id, Some(name.to_string()));
                self.sync_active_agent_label();
                Ok(true)
            }
            AppCommand::ThreadRollback { num_turns } => {
                let response = match app_server.thread_rollback(thread_id, *num_turns).await {
                    Ok(response) => response,
                    Err(err) => {
                        self.handle_backtrack_rollback_failed();
                        return Err(err);
                    }
                };
                self.handle_thread_rollback_response(thread_id, *num_turns, &response)
                    .await;
                Ok(true)
            }
            AppCommand::Review { target } => {
                app_server.review_start(thread_id, target.clone()).await?;
                Ok(true)
            }
            AppCommand::CleanBackgroundTerminals => {
                app_server
                    .thread_background_terminals_clean(thread_id)
                    .await?;
                Ok(true)
            }
            AppCommand::RealtimeConversationStart { transport, voice } => {
                app_server
                    .thread_realtime_start(thread_id, transport.clone(), voice.clone())
                    .await?;
                Ok(true)
            }
            AppCommand::RealtimeConversationAudio(frame) => {
                app_server
                    .thread_realtime_audio(thread_id, frame.clone())
                    .await?;
                Ok(true)
            }
            AppCommand::RealtimeConversationClose => {
                app_server.thread_realtime_stop(thread_id).await?;
                Ok(true)
            }
            AppCommand::RunUserShellCommand { command } => {
                app_server
                    .thread_shell_command(thread_id, command.to_string())
                    .await?;
                Ok(true)
            }
            AppCommand::StartExternalAgent {
                runtime_id,
                task,
                mode,
            } => {
                let response = app_server
                    .thread_external_agent_start(
                        thread_id,
                        runtime_id.to_string(),
                        task.to_string(),
                        *mode,
                    )
                    .await?;
                match response.status {
                    ThreadExternalAgentStartStatus::Started => {
                        self.chat_widget.add_info_message(
                            format!("External-agent `{runtime_id}` started."),
                            response
                                .run_id
                                .map(|run_id| format!("Run: {run_id}. Mode: {mode:?}.")),
                        );
                    }
                    ThreadExternalAgentStartStatus::Gated => {
                        self.chat_widget.add_error_message(format!(
                            "External-agent `{runtime_id}` gated: {}",
                            response.message
                        ));
                    }
                }
                Ok(true)
            }
            AppCommand::ReloadUserConfig => {
                app_server.reload_user_config().await?;
                Ok(true)
            }
            AppCommand::OverrideTurnContext { .. } => {
                self.sync_override_turn_context_settings(app_server, thread_id, op)
                    .await;
                Ok(true)
            }
            AppCommand::ApproveGuardianDeniedAction { event } => {
                app_server
                    .thread_approve_guardian_denied_action(thread_id, event)
                    .await?;
                Ok(true)
            }
            AppCommand::RespondExternalAgentPermission {
                run_id,
                request_id,
                decision,
            } => {
                app_server
                    .thread_external_agent_permission_respond(
                        thread_id,
                        run_id.to_string(),
                        request_id.to_string(),
                        *decision,
                    )
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn turn_permissions_override_from_config(
        config: &Config,
        active_permission_profile: Option<&ActivePermissionProfile>,
        runtime_permission_profile_override: Option<&PermissionProfile>,
    ) -> TurnPermissionsOverride {
        if let Some(active_permission_profile) = active_permission_profile {
            return TurnPermissionsOverride::ActiveProfile(active_permission_profile.clone());
        }

        let effective_permission_profile = config.permissions.effective_permission_profile();
        let runtime_permission_profile_override =
            runtime_permission_profile_override.map(|profile| {
                profile
                    .clone()
                    .materialize_project_roots_with_workspace_roots(
                        &config.effective_workspace_roots(),
                    )
            });
        if runtime_permission_profile_override
            .as_ref()
            .is_some_and(|profile| profile == &effective_permission_profile)
        {
            return TurnPermissionsOverride::LegacySandbox(effective_permission_profile);
        }

        TurnPermissionsOverride::Preserve
    }

    pub(super) fn handle_skills_list_result(
        &mut self,
        result: Result<SkillsListResponse>,
        failure_message: &str,
    ) {
        match result {
            Ok(response) => self.handle_skills_list_response(response),
            Err(err) => {
                tracing::warn!("{failure_message}: {err:#}");
                self.chat_widget
                    .add_error_message(format!("{failure_message}: {err:#}"));
            }
        }
    }

    pub(super) async fn try_resolve_app_server_request(
        &mut self,
        app_server: &AppServerSession,
        thread_id: ThreadId,
        op: &AppCommand,
    ) -> Result<bool> {
        let mut resolution = self
            .pending_app_server_requests
            .take_resolution(op)
            .map_err(|err| color_eyre::eyre::eyre!(err))?;
        if resolution.is_none() {
            resolution = self
                .recover_app_server_request_resolution(thread_id, op)
                .await
                .map_err(|err| color_eyre::eyre::eyre!(err))?;
        }
        let Some(resolution) = resolution else {
            self.warn_missing_app_server_request_resolution(thread_id, op);
            return Ok(false);
        };

        match app_server
            .resolve_server_request(resolution.request_id, resolution.result)
            .await
        {
            Ok(()) => {
                if ThreadEventStore::op_can_change_pending_replay_state(op) {
                    self.note_thread_outbound_op(thread_id, op).await;
                    self.refresh_pending_thread_approvals().await;
                    self.refresh_side_parent_status_from_store(thread_id).await;
                }
                Ok(true)
            }
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to resolve app-server request for thread {thread_id}: {err}"
                ));
                Ok(false)
            }
        }
    }

    fn warn_missing_app_server_request_resolution(&self, thread_id: ThreadId, op: &AppCommand) {
        let Some(lookup) = app_server_request_lookup(op) else {
            return;
        };
        tracing::warn!(
            thread_id = %thread_id,
            request_kind = lookup.kind,
            request_key = %lookup.key,
            turn_id = ?lookup.turn_id,
            "missing app-server request mapping for outbound interactive response"
        );
    }

    async fn recover_app_server_request_resolution(
        &mut self,
        thread_id: ThreadId,
        op: &AppCommand,
    ) -> std::result::Result<Option<AppServerRequestResolution>, String> {
        let request = {
            let Some(channel) = self.thread_event_channels.get(&thread_id) else {
                return Ok(None);
            };
            let store = channel.store.lock().await;
            store.pending_server_request_for_op(op)
        };
        let Some(request) = request else {
            return Ok(None);
        };

        let request_id = request.id().clone();
        if let Some(unsupported) = self
            .pending_app_server_requests
            .note_server_request(&request)
        {
            return Err(format!(
                "cannot recover unsupported app-server request {:?}: {}",
                unsupported.request_id, unsupported.message
            ));
        }

        let resolution = self.pending_app_server_requests.take_resolution(op)?;
        if resolution.is_some()
            && let Some(lookup) = app_server_request_lookup(op)
        {
            tracing::warn!(
                thread_id = %thread_id,
                request_kind = lookup.kind,
                request_key = %lookup.key,
                turn_id = ?lookup.turn_id,
                server_request_id = ?request_id,
                "recovered missing app-server request mapping from thread event buffer"
            );
        }
        Ok(resolution)
    }

    pub(super) async fn refresh_pending_thread_approvals(&mut self) {
        let side_parent_thread_id = self.active_side_parent_thread_id();
        let channels: Vec<(ThreadId, Arc<Mutex<ThreadEventStore>>)> = self
            .thread_event_channels
            .iter()
            .map(|(thread_id, channel)| (*thread_id, Arc::clone(&channel.store)))
            .collect();

        let mut pending_thread_ids = Vec::new();
        for (thread_id, store) in channels {
            if Some(thread_id) == self.active_thread_id || Some(thread_id) == side_parent_thread_id
            {
                continue;
            }

            let store = store.lock().await;
            if store.has_pending_thread_approvals() {
                pending_thread_ids.push(thread_id);
            }
        }

        pending_thread_ids.sort_by_key(ThreadId::to_string);

        let threads = pending_thread_ids
            .into_iter()
            .map(|thread_id| self.thread_label(thread_id))
            .collect();

        self.chat_widget.set_pending_thread_approvals(threads);
    }

    pub(super) async fn refresh_side_parent_status_from_store(&mut self, thread_id: ThreadId) {
        let Some(channel) = self.thread_event_channels.get(&thread_id) else {
            return;
        };
        let status = {
            let store = channel.store.lock().await;
            store.side_parent_pending_status()
        };
        if let Some(status) = status {
            self.set_side_parent_status(thread_id, Some(status));
        } else {
            self.clear_side_parent_action_status(thread_id);
        }
    }

    pub(super) async fn enqueue_thread_notification(
        &mut self,
        thread_id: ThreadId,
        notification: ServerNotification,
    ) -> Result<()> {
        if matches!(notification, ServerNotification::ThreadSettingsUpdated(_))
            && self.primary_thread_id.is_some()
            && self.primary_thread_id != Some(thread_id)
            && !self.thread_event_channels.contains_key(&thread_id)
        {
            return Ok(());
        }
        if let ServerNotification::ThreadSettingsUpdated(notification) = &notification {
            self.apply_thread_settings_to_cached_session(thread_id, &notification.thread_settings)
                .await;
        }
        let inferred_session = self
            .infer_session_for_thread_notification(thread_id, &notification)
            .await;
        let (sender, store) = {
            let channel = self.ensure_thread_channel(thread_id);
            (channel.sender.clone(), Arc::clone(&channel.store))
        };

        let (should_send, pending_status) = {
            let mut guard = store.lock().await;
            if guard.session.is_none()
                && let Some(session) = inferred_session
            {
                guard.session = Some(session);
            }
            guard.push_notification(notification.clone());
            (guard.active, guard.side_parent_pending_status())
        };
        let notification_status_change = SideParentStatusChange::for_notification(&notification);

        if should_send {
            match sender.try_send(ThreadBufferedEvent::Notification(notification)) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    tokio::spawn(async move {
                        if let Err(err) = sender.send(event).await {
                            tracing::warn!("thread {thread_id} event channel closed: {err}");
                        }
                    });
                }
                Err(TrySendError::Closed(_)) => {
                    tracing::warn!("thread {thread_id} event channel closed");
                }
            }
        }
        if let Some(status) = pending_status {
            self.set_side_parent_status(thread_id, Some(status));
        } else if let Some(change) = notification_status_change {
            self.apply_side_parent_status_change(thread_id, change);
        }
        self.refresh_pending_thread_approvals().await;
        Ok(())
    }

    /// Validates receiver thread ids referenced by a collab notification and, for spawn
    /// notifications, seeds hierarchical render metadata for the freshly spawned children.
    ///
    /// This intentionally avoids app-server reads on the active-thread rendering path. During large
    /// fan-outs the app-server can be saturated with spawn work, and blocking here would freeze the
    /// TUI event loop. Receiver-only notifications are not proof that a thread is a spawned
    /// descendant of the current primary thread, so they must not create `/agent` picker rows.
    pub(super) fn cache_collab_receiver_threads_for_notification(
        &mut self,
        notification: &ServerNotification,
    ) {
        let Some(receiver_thread_ids) = collab_receiver_thread_ids(notification) else {
            return;
        };

        for receiver_thread_id in receiver_thread_ids {
            if collab_receiver_is_not_found(notification, receiver_thread_id) {
                continue;
            }

            if ThreadId::from_string(receiver_thread_id).is_err() {
                tracing::warn!(
                    thread_id = receiver_thread_id,
                    "ignoring collab receiver with invalid thread id during picker lineage validation"
                );
                continue;
            }
        }

        // A spawn notification is the authoritative moment we learn that each receiver thread is a
        // freshly spawned child of the sender. `handle_thread_event_now` calls this immediately
        // before the sibling `handle_server_notification` renders the "Spawned" row, so seeding the
        // lineage edge and a hierarchical tree path here means the row shows `root/agent/sub_agent`
        // (or the child's nickname once its own `ThreadStarted` has landed) instead of a raw
        // 8-character short id. Without this the row falls back to a short id, which additionally
        // collides across siblings spawned within the same ~65s UUIDv7 timestamp window.
        if let Some((sender_thread_id, spawn_receiver_thread_ids)) =
            collab_spawn_lineage(notification)
            && let Ok(parent_thread_id) = ThreadId::from_string(sender_thread_id)
        {
            for receiver_thread_id in spawn_receiver_thread_ids {
                if collab_receiver_is_not_found(notification, receiver_thread_id) {
                    continue;
                }
                let Ok(child_thread_id) = ThreadId::from_string(receiver_thread_id) else {
                    continue;
                };
                self.seed_spawned_child_render_metadata(child_thread_id, parent_thread_id);
            }
        }
    }

    /// Records the authoritative spawn lineage edge for a freshly spawned child and refreshes the
    /// `ChatWidget` render metadata so the "Spawned" row renders a hierarchical tree path instead of
    /// a raw thread id.
    ///
    /// This deliberately records only the child -> parent edge and rendering metadata; it does not
    /// call [`Self::upsert_agent_picker_thread`], so a receiver reference never fabricates an
    /// `/agent` picker row before the child's own `ThreadStarted` confirms it. When that
    /// `ThreadStarted` later lands it upgrades the resolution to the authoritative agent path and
    /// nickname.
    fn seed_spawned_child_render_metadata(
        &mut self,
        child_thread_id: ThreadId,
        parent_thread_id: ThreadId,
    ) {
        self.agent_navigation
            .set_parent(child_thread_id, parent_thread_id);
        let tree_path = self.agent_tree_path(child_thread_id);
        let existing = self.agent_navigation.get(&child_thread_id);
        let agent_nickname = existing.and_then(|entry| entry.agent_nickname.clone());
        let agent_role = existing.and_then(|entry| entry.agent_role.clone());
        self.chat_widget.set_collab_agent_metadata(
            child_thread_id,
            agent_nickname,
            agent_role,
            Some(tree_path),
        );
    }

    pub(super) async fn infer_session_for_thread_notification(
        &mut self,
        thread_id: ThreadId,
        notification: &ServerNotification,
    ) -> Option<ThreadSessionState> {
        let ServerNotification::ThreadStarted(notification) = notification else {
            return None;
        };
        let mut session = self.primary_session_configured.clone()?;
        session.thread_id = thread_id;
        session.thread_name = notification.thread.name.clone();
        session.model_provider_id = notification.thread.model_provider.clone();
        session
            .set_cwd_retargeting_implicit_runtime_workspace_root(notification.thread.cwd.clone());
        let rollout_path = notification.thread.path.clone();
        if let Some(model) =
            read_session_model(self.state_db.as_deref(), thread_id, rollout_path.as_deref()).await
        {
            session.model = model;
        } else if rollout_path.is_some() {
            session.model.clear();
        }
        session.message_history = None;
        session.rollout_path = rollout_path;
        let parent_thread_id = notification
            .thread
            .parent_thread_id
            .as_deref()
            .and_then(|id| ThreadId::from_string(id).ok())
            .or_else(|| thread_spawn_parent_thread_id(&notification.thread.source));
        let should_register_picker_thread = if self.primary_thread_id == Some(thread_id) {
            true
        } else if let (Some(primary_thread_id), Some(parent_thread_id)) =
            (self.primary_thread_id, parent_thread_id)
        {
            parent_thread_id == primary_thread_id
                || self.agent_navigation.get(&parent_thread_id).is_some()
        } else {
            false
        };
        if should_register_picker_thread {
            self.upsert_agent_picker_thread(
                thread_id,
                notification.thread.agent_nickname.clone(),
                notification.thread.agent_role.clone(),
                parent_thread_id,
                thread_spawn_agent_path(&notification.thread.source),
                /*is_closed*/ false,
            );
            self.agent_navigation
                .set_thread_name(thread_id, notification.thread.name.clone());
            self.sync_active_agent_label();
        }
        Some(session)
    }

    pub(super) async fn enqueue_thread_request(
        &mut self,
        thread_id: ThreadId,
        request: ServerRequest,
    ) -> Result<()> {
        let inactive_interactive_request = if self.active_thread_id != Some(thread_id) {
            self.interactive_request_for_thread_request(thread_id, &request)
                .await
        } else {
            None
        };
        let (sender, store) = {
            let channel = self.ensure_thread_channel(thread_id);
            (channel.sender.clone(), Arc::clone(&channel.store))
        };

        let (should_send, pending_status) = {
            let mut guard = store.lock().await;
            guard.push_request(request.clone());
            (guard.active, guard.side_parent_pending_status())
        };
        let request_status = SideParentStatus::for_request(&request);

        if should_send {
            match sender.try_send(ThreadBufferedEvent::Request(request)) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    tokio::spawn(async move {
                        if let Err(err) = sender.send(event).await {
                            tracing::warn!("thread {thread_id} event channel closed: {err}");
                        }
                    });
                }
                Err(TrySendError::Closed(_)) => {
                    tracing::warn!("thread {thread_id} event channel closed");
                }
            }
        } else if self.active_side_parent_thread_id().is_none()
            && let Some(request) = inactive_interactive_request
        {
            self.push_thread_interactive_request(request);
        }
        if let Some(status) = pending_status.or(request_status) {
            self.set_side_parent_status(thread_id, Some(status));
        }
        self.refresh_pending_thread_approvals().await;
        Ok(())
    }

    pub(super) async fn enqueue_thread_history_entry_response(
        &mut self,
        thread_id: ThreadId,
        event: HistoryLookupResponse,
    ) -> Result<()> {
        let (sender, store) = {
            let channel = self.ensure_thread_channel(thread_id);
            (channel.sender.clone(), Arc::clone(&channel.store))
        };

        let should_send = {
            let mut guard = store.lock().await;
            guard
                .buffer
                .push_back(ThreadBufferedEvent::HistoryEntryResponse(event.clone()));
            if guard.buffer.len() > guard.capacity
                && let Some(removed) = guard.buffer.pop_front()
                && let ThreadBufferedEvent::Request(request) = &removed
            {
                guard
                    .pending_interactive_replay
                    .note_evicted_server_request(request);
            }
            guard.active
        };

        if should_send {
            match sender.try_send(ThreadBufferedEvent::HistoryEntryResponse(event)) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    tokio::spawn(async move {
                        if let Err(err) = sender.send(event).await {
                            tracing::warn!("thread {thread_id} event channel closed: {err}");
                        }
                    });
                }
                Err(TrySendError::Closed(_)) => {
                    tracing::warn!("thread {thread_id} event channel closed");
                }
            }
        }
        Ok(())
    }

    pub(super) async fn enqueue_primary_thread_session(
        &mut self,
        session: ThreadSessionState,
        turns: Vec<Turn>,
    ) -> Result<()> {
        let thread_id = session.thread_id;
        self.primary_thread_id = Some(thread_id);
        self.primary_session_configured = Some(session.clone());
        self.upsert_agent_picker_thread(
            thread_id, /*agent_nickname*/ None, /*agent_role*/ None,
            /*parent_thread_id*/ None, /*agent_path*/ None, /*is_closed*/ false,
        );
        let channel = self.ensure_thread_channel(thread_id);
        {
            let mut store = channel.store.lock().await;
            store.set_session(session.clone(), turns.clone());
        }
        self.activate_thread_channel(thread_id).await;
        self.chat_widget
            .set_initial_user_message_submit_suppressed(/*suppressed*/ true);
        self.chat_widget.handle_thread_session(session);
        let should_buffer_initial_replay =
            self.terminal_resize_reflow_enabled() && !turns.is_empty();
        if should_buffer_initial_replay {
            self.app_event_tx
                .send(AppEvent::BeginInitialHistoryReplayBuffer);
        }
        self.chat_widget
            .replay_thread_turns(turns, ReplayKind::ResumeInitialMessages);
        if should_buffer_initial_replay {
            self.app_event_tx
                .send(AppEvent::EndInitialHistoryReplayBuffer);
        }
        let pending = std::mem::take(&mut self.pending_primary_events);
        for pending_event in pending {
            match pending_event {
                ThreadBufferedEvent::Notification(notification) => {
                    let event_thread_id =
                        server_notification_thread_target(&notification).thread_id();
                    match self.resolve_pending_primary_flush_target(event_thread_id, thread_id) {
                        Some(target_thread_id) => {
                            self.enqueue_thread_notification(target_thread_id, notification)
                                .await?;
                        }
                        None => {
                            tracing::debug!(
                                ?event_thread_id,
                                %thread_id,
                                "dropping buffered notification for unrelated untracked thread while starting primary thread"
                            );
                        }
                    }
                }
                ThreadBufferedEvent::Request(request) => {
                    let event_thread_id = server_request_thread_id(&request);
                    match self.resolve_pending_primary_flush_target(event_thread_id, thread_id) {
                        Some(target_thread_id) => {
                            self.enqueue_thread_request(target_thread_id, request)
                                .await?;
                        }
                        None => {
                            tracing::debug!(
                                ?event_thread_id,
                                %thread_id,
                                "dropping buffered request for unrelated untracked thread while starting primary thread"
                            );
                        }
                    }
                }
                ThreadBufferedEvent::HistoryEntryResponse(event) => {
                    self.enqueue_thread_history_entry_response(thread_id, event)
                        .await?;
                }
                ThreadBufferedEvent::FeedbackSubmission(event) => {
                    self.enqueue_thread_feedback_event(thread_id, event).await;
                }
            }
        }
        self.chat_widget
            .set_initial_user_message_submit_suppressed(/*suppressed*/ false);
        self.chat_widget.submit_initial_user_message_if_pending();
        Ok(())
    }

    /// Whether `thread_id` is already tracked by the TUI (has an event channel
    /// or is a registered side thread). Used during the startup pre-primary
    /// window to distinguish known loaded/scheduled threads from the primary
    /// thread that is still starting.
    pub(super) fn is_tracked_thread(&self, thread_id: ThreadId) -> bool {
        self.thread_event_channels.contains_key(&thread_id)
            || self.side_threads.contains_key(&thread_id)
    }

    /// Resolve which thread a buffered pre-primary event should flush onto once
    /// the primary thread (`primary_thread_id`) has started.
    ///
    /// - Events that target the started primary thread flush onto the primary.
    /// - Events for another already-tracked thread flush onto that thread's own
    ///   channel so they are not misrouted onto the primary's stream.
    /// - Events for an unrelated, untracked thread return `None` and are dropped
    ///   by the caller; they never belonged to the primary being started.
    /// - Events with no resolvable thread target flush onto the primary, matching
    ///   the historical behavior (such events are not buffered here in practice).
    fn resolve_pending_primary_flush_target(
        &self,
        event_thread_id: Option<ThreadId>,
        primary_thread_id: ThreadId,
    ) -> Option<ThreadId> {
        match event_thread_id {
            None => Some(primary_thread_id),
            Some(event_thread_id) if event_thread_id == primary_thread_id => {
                Some(primary_thread_id)
            }
            Some(event_thread_id) if self.is_tracked_thread(event_thread_id) => {
                Some(event_thread_id)
            }
            Some(_) => None,
        }
    }

    pub(super) async fn enqueue_primary_thread_notification(
        &mut self,
        notification: ServerNotification,
    ) -> Result<()> {
        if let Some(thread_id) = self.primary_thread_id {
            return self
                .enqueue_thread_notification(thread_id, notification)
                .await;
        }
        self.pending_primary_events
            .push_back(ThreadBufferedEvent::Notification(notification));
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) async fn enqueue_primary_thread_request(
        &mut self,
        request: ServerRequest,
    ) -> Result<()> {
        if let Some(thread_id) = self.primary_thread_id {
            return self.enqueue_thread_request(thread_id, request).await;
        }
        self.pending_primary_events
            .push_back(ThreadBufferedEvent::Request(request));
        Ok(())
    }

    pub(super) async fn refresh_snapshot_session_if_needed(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        is_replay_only: bool,
        snapshot: &mut ThreadEventSnapshot,
    ) {
        if !self.should_refresh_snapshot_session(thread_id, is_replay_only, snapshot) {
            return;
        }

        match app_server
            .resume_thread(self.config.clone(), thread_id)
            .await
        {
            Ok(started) => {
                self.apply_refreshed_snapshot_thread(thread_id, started, snapshot)
                    .await
            }
            Err(err) => {
                tracing::warn!(
                    thread_id = %thread_id,
                    error = %err,
                    "failed to refresh inferred thread session before replay"
                );
            }
        }
    }

    pub(super) async fn refresh_snapshot_turns_from_thread_read(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        snapshot: &mut ThreadEventSnapshot,
    ) {
        if self.side_threads.contains_key(&thread_id) {
            return;
        }

        match app_server
            .thread_read(thread_id, /*include_turns*/ true)
            .await
        {
            Ok(thread) => {
                let session = self.session_state_for_thread_read(thread_id, &thread).await;
                let turns = thread.turns.clone();
                if let Some(channel) = self.thread_event_channels.get(&thread_id) {
                    let mut store = channel.store.lock().await;
                    store.set_session(session.clone(), turns.clone());
                    store.rebase_buffer_after_session_refresh();
                }
                snapshot.session = Some(session);
                snapshot.turns = turns;
                snapshot
                    .events
                    .retain(ThreadEventStore::event_survives_session_refresh);
                self.agent_navigation
                    .set_thread_name(thread_id, thread.name);
                self.sync_active_agent_label();
            }
            Err(err) if Self::can_fallback_from_include_turns_error(&err) => {
                match app_server
                    .thread_read(thread_id, /*include_turns*/ false)
                    .await
                {
                    Ok(thread) => {
                        let session = self.session_state_for_thread_read(thread_id, &thread).await;
                        if let Some(channel) = self.thread_event_channels.get(&thread_id) {
                            let mut store = channel.store.lock().await;
                            store.session = Some(session.clone());
                        }
                        snapshot.session = Some(session);
                        self.agent_navigation
                            .set_thread_name(thread_id, thread.name);
                        self.sync_active_agent_label();
                    }
                    Err(fallback_err) => {
                        tracing::warn!(
                            thread_id = %thread_id,
                            error = %fallback_err,
                            "failed to refresh thread metadata before replay"
                        );
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    thread_id = %thread_id,
                    error = %err,
                    "failed to hydrate thread turns before replay"
                );
            }
        }
    }

    pub(super) fn should_refresh_snapshot_session(
        &self,
        thread_id: ThreadId,
        is_replay_only: bool,
        snapshot: &ThreadEventSnapshot,
    ) -> bool {
        !is_replay_only
            && !self.side_threads.contains_key(&thread_id)
            && snapshot.session.as_ref().is_none_or(|session| {
                session.model.trim().is_empty() || session.rollout_path.is_none()
            })
    }

    pub(super) async fn apply_refreshed_snapshot_thread(
        &mut self,
        thread_id: ThreadId,
        started: AppServerStartedThread,
        snapshot: &mut ThreadEventSnapshot,
    ) {
        let AppServerStartedThread { session, turns } = started;
        if let Some(channel) = self.thread_event_channels.get(&thread_id) {
            let mut store = channel.store.lock().await;
            store.set_session(session.clone(), turns.clone());
            store.rebase_buffer_after_session_refresh();
        }
        snapshot.session = Some(session);
        snapshot.turns = turns;
        snapshot
            .events
            .retain(ThreadEventStore::event_survives_session_refresh);
    }

    /// Opens the `/session` picker after refreshing cached labels for known threads.
    ///
    /// The picker state is derived from long-lived thread channels plus best-effort metadata
    /// refreshes from the backend. Refresh failures are treated as "thread is only inspectable by
    /// historical id now" and converted into closed picker entries instead of deleting them, so
    /// the stable traversal order remains intact for review and keyboard navigation.
    pub(super) async fn drain_active_thread_events(&mut self, tui: &mut tui::Tui) -> Result<()> {
        let Some(mut rx) = self.active_thread_rx.take() else {
            return Ok(());
        };

        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(event) => self.handle_thread_event_now(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if !disconnected {
            self.active_thread_rx = Some(rx);
        } else {
            self.clear_active_thread().await;
        }

        if self.backtrack_render_pending {
            tui.frame_requester().schedule_frame();
        }
        Ok(())
    }

    /// Returns `(closed_thread_id, primary_thread_id)` when a non-primary active
    /// thread has died and we should fail over to the primary thread.
    ///
    /// A user-requested shutdown (`ExitMode::ShutdownFirst`) sets
    /// `pending_shutdown_exit_thread_id`; matching shutdown completions are ignored
    /// here so Ctrl+C-like exits don't accidentally resurrect the main thread.
    ///
    /// Failover is only eligible when all of these are true:
    /// 1. the event is `thread/closed`;
    /// 2. the active thread differs from the primary thread;
    /// 3. the active thread is not the pending shutdown-exit thread.
    pub(super) fn active_non_primary_shutdown_target(
        &self,
        notification: &ServerNotification,
    ) -> Option<(ThreadId, ThreadId)> {
        if !matches!(notification, ServerNotification::ThreadClosed(_)) {
            return None;
        }
        let active_thread_id = self.active_thread_id?;
        let primary_thread_id = self.primary_thread_id?;
        if self.pending_shutdown_exit_thread_id == Some(active_thread_id) {
            return None;
        }
        (active_thread_id != primary_thread_id).then_some((active_thread_id, primary_thread_id))
    }

    pub(super) fn replay_thread_snapshot(
        &mut self,
        snapshot: ThreadEventSnapshot,
        resume_restored_queue: bool,
    ) {
        self.refresh_mcp_startup_expected_servers_from_config();
        let should_buffer_replay = self.terminal_resize_reflow_enabled()
            && (!snapshot.turns.is_empty() || !snapshot.events.is_empty());
        if should_buffer_replay {
            self.app_event_tx
                .send(AppEvent::BeginThreadSwitchHistoryReplayBuffer);
        }
        let suppress_replay_notices =
            replay_filter::snapshot_has_pending_interactive_request(&snapshot);
        if let Some(session) = snapshot.session {
            if self.side_threads.contains_key(&session.thread_id) {
                self.chat_widget.handle_side_thread_session(session);
            } else if suppress_replay_notices {
                self.chat_widget.handle_thread_session_quiet(session);
            } else {
                self.chat_widget.handle_thread_session(session);
            }
        }
        self.chat_widget
            .set_queue_autosend_suppressed(/*suppressed*/ true);
        self.chat_widget
            .restore_thread_input_state(snapshot.input_state);
        if !snapshot.turns.is_empty() {
            self.chat_widget
                .replay_thread_turns(snapshot.turns, ReplayKind::ThreadSnapshot);
        }
        for event in snapshot.events {
            if suppress_replay_notices && replay_filter::event_is_notice(&event) {
                continue;
            }
            self.handle_thread_event_replay(event);
        }
        if should_buffer_replay {
            self.app_event_tx
                .send(AppEvent::EndInitialHistoryReplayBuffer);
        }
        self.chat_widget
            .set_queue_autosend_suppressed(/*suppressed*/ false);
        self.chat_widget
            .set_initial_user_message_submit_suppressed(/*suppressed*/ false);
        self.chat_widget.submit_initial_user_message_if_pending();
        if resume_restored_queue {
            self.chat_widget.maybe_send_next_queued_input();
        }
        self.refresh_status_line();
    }

    pub(super) fn should_wait_for_initial_session(session_selection: &SessionSelection) -> bool {
        matches!(
            session_selection,
            SessionSelection::StartFresh | SessionSelection::Exit
        )
    }

    pub(super) fn should_prompt_for_paused_goal_after_startup_resume(
        session_selection: &SessionSelection,
        initial_prompt: &Option<String>,
        initial_images: &[PathBuf],
    ) -> bool {
        matches!(session_selection, SessionSelection::Resume(_))
            && initial_prompt.is_none()
            && initial_images.is_empty()
    }

    pub(super) fn should_handle_active_thread_events(
        waiting_for_initial_session_configured: bool,
        has_active_thread_receiver: bool,
    ) -> bool {
        has_active_thread_receiver && !waiting_for_initial_session_configured
    }

    pub(super) fn should_stop_waiting_for_initial_session(
        waiting_for_initial_session_configured: bool,
        primary_thread_id: Option<ThreadId>,
    ) -> bool {
        waiting_for_initial_session_configured && primary_thread_id.is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_skills_list_response(&mut self, response: SkillsListResponse) {
        let cwd = self.chat_widget.config_ref().cwd.clone();
        let errors = errors_for_cwd(&cwd, &response);
        let errors = self.skill_load_warnings.newly_active_errors(&errors);
        emit_skill_load_warnings(&self.app_event_tx, &errors);
        self.chat_widget.handle_skills_list_response(response);
    }

    pub(super) async fn handle_thread_rollback_response(
        &mut self,
        thread_id: ThreadId,
        num_turns: u32,
        response: &ThreadRollbackResponse,
    ) {
        if let Some(channel) = self.thread_event_channels.get(&thread_id) {
            let mut store = channel.store.lock().await;
            store.apply_thread_rollback(response);
        }
        if self.active_thread_id == Some(thread_id)
            && let Some(mut rx) = self.active_thread_rx.take()
        {
            let mut disconnected = false;
            loop {
                match rx.try_recv() {
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if !disconnected {
                self.active_thread_rx = Some(rx);
            } else {
                self.clear_active_thread().await;
            }
        }
        self.handle_backtrack_rollback_succeeded(num_turns);
    }

    pub(super) fn handle_thread_event_now(&mut self, event: ThreadBufferedEvent) {
        let needs_refresh = matches!(
            &event,
            ThreadBufferedEvent::Notification(ServerNotification::TurnStarted(_))
                | ThreadBufferedEvent::Notification(ServerNotification::ThreadTokenUsageUpdated(_))
        );
        match event {
            ThreadBufferedEvent::Notification(notification) => {
                self.cache_collab_receiver_threads_for_notification(&notification);
                self.chat_widget
                    .handle_server_notification(notification, /*replay_kind*/ None);
            }
            ThreadBufferedEvent::Request(request) => {
                if self
                    .pending_app_server_requests
                    .contains_server_request(&request)
                {
                    self.chat_widget
                        .handle_server_request(request, /*replay_kind*/ None);
                }
            }
            ThreadBufferedEvent::HistoryEntryResponse(event) => {
                self.chat_widget.handle_history_entry_response(event);
            }
            ThreadBufferedEvent::FeedbackSubmission(event) => {
                self.handle_feedback_thread_event(event);
            }
        }
        if needs_refresh {
            self.refresh_status_line();
        }
    }

    pub(super) fn handle_thread_event_replay(&mut self, event: ThreadBufferedEvent) {
        match event {
            ThreadBufferedEvent::Notification(notification) => self
                .chat_widget
                .handle_server_notification(notification, Some(ReplayKind::ThreadSnapshot)),
            ThreadBufferedEvent::Request(request) => self
                .chat_widget
                .handle_server_request(request, Some(ReplayKind::ThreadSnapshot)),
            ThreadBufferedEvent::HistoryEntryResponse(event) => {
                self.chat_widget.handle_history_entry_response(event)
            }
            ThreadBufferedEvent::FeedbackSubmission(event) => {
                self.handle_feedback_thread_event(event);
            }
        }
    }

    /// Handles an event emitted by the currently active thread.
    ///
    /// This function enforces shutdown intent routing: unexpected non-primary
    /// thread shutdowns fail over to the primary thread, while user-requested
    /// app exits consume only the tracked shutdown completion and then proceed.
    pub(super) async fn handle_active_thread_event(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        event: ThreadBufferedEvent,
    ) -> Result<()> {
        // Capture this before any potential thread switch: we only want to clear
        // the exit marker when the currently active thread acknowledges shutdown.
        let pending_shutdown_exit_completed = matches!(
            &event,
            ThreadBufferedEvent::Notification(ServerNotification::ThreadClosed(_))
        ) && self.pending_shutdown_exit_thread_id
            == self.active_thread_id;

        // Processing order matters:
        //
        // 1. handle unexpected non-primary shutdown failover first;
        // 2. clear pending exit marker for matching shutdown;
        // 3. forward the event through normal handling.
        //
        // This preserves the mental model that user-requested exits do not trigger
        // failover, while true sub-agent deaths still do.
        if let ThreadBufferedEvent::Notification(notification) = &event
            && let Some((closed_thread_id, primary_thread_id)) =
                self.active_non_primary_shutdown_target(notification)
        {
            self.mark_agent_picker_thread_closed(closed_thread_id);
            if self.side_threads.contains_key(&closed_thread_id) {
                self.discard_closed_side_thread(closed_thread_id).await;
                self.select_agent_thread(tui, app_server, primary_thread_id)
                    .await?;
            } else {
                self.select_agent_thread_and_discard_side(tui, app_server, primary_thread_id)
                    .await?;
            }
            if self.active_thread_id == Some(primary_thread_id) {
                self.chat_widget.add_info_message(
                    format!(
                        "Agent thread {closed_thread_id} closed. Switched back to main thread."
                    ),
                    /*hint*/ None,
                );
            } else {
                self.clear_active_thread().await;
                self.chat_widget.add_error_message(format!(
                    "Agent thread {closed_thread_id} closed. Failed to switch back to main thread {primary_thread_id}.",
                ));
            }
            return Ok(());
        }

        if pending_shutdown_exit_completed {
            // Clear only after seeing the shutdown completion for the tracked
            // thread, so unrelated shutdowns cannot consume this marker.
            self.pending_shutdown_exit_thread_id = None;
        }
        self.handle_thread_event_now(event);
        if self.backtrack_render_pending {
            tui.frame_requester().schedule_frame();
        }
        Ok(())
    }
}

/// Applies the depth cap and per-segment grapheme truncation, then joins segments with `/`.
fn finalize_agent_tree_path(segments: Vec<String>) -> String {
    cap_agent_tree_path_depth(segments)
        .into_iter()
        .map(|segment| {
            crate::text_formatting::truncate_text(&segment, AGENT_TREE_PATH_SEGMENT_MAX_GRAPHEMES)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Collapses the middle of an over-deep path to a single ellipsis segment, keeping the root, the
/// immediate parent, and the leaf: `root/…/<parent>/<leaf>`.
fn cap_agent_tree_path_depth(segments: Vec<String>) -> Vec<String> {
    if segments.len() <= AGENT_TREE_PATH_MAX_SEGMENTS {
        return segments;
    }
    let len = segments.len();
    vec![
        segments[0].clone(),
        AGENT_TREE_PATH_ELLIPSIS.to_string(),
        segments[len - 2].clone(),
        segments[len - 1].clone(),
    ]
}

struct AppServerRequestLookup {
    kind: &'static str,
    key: String,
    turn_id: Option<String>,
}

fn app_server_request_lookup(op: &AppCommand) -> Option<AppServerRequestLookup> {
    match op {
        AppCommand::ExecApproval { id, turn_id, .. } => Some(AppServerRequestLookup {
            kind: "commandExecutionApproval",
            key: id.clone(),
            turn_id: turn_id.clone(),
        }),
        AppCommand::PatchApproval { id, .. } => Some(AppServerRequestLookup {
            kind: "fileChangeApproval",
            key: id.clone(),
            turn_id: None,
        }),
        AppCommand::RequestPermissionsResponse { id, .. } => Some(AppServerRequestLookup {
            kind: "permissionsApproval",
            key: id.clone(),
            turn_id: None,
        }),
        AppCommand::UserInputAnswer { id, .. } => Some(AppServerRequestLookup {
            kind: "requestUserInput",
            key: id.clone(),
            turn_id: Some(id.clone()),
        }),
        AppCommand::ResolveElicitation {
            server_name,
            request_id,
            ..
        } => Some(AppServerRequestLookup {
            kind: "mcpElicitation",
            key: format!("{server_name}:{request_id:?}"),
            turn_id: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::PermissionGrantScope as AppServerPermissionGrantScope;
    use codex_app_server_protocol::PermissionsRequestApprovalParams;
    use codex_app_server_protocol::PermissionsRequestApprovalResponse;
    use codex_app_server_protocol::RequestId as AppServerRequestId;
    use codex_protocol::models::ActivePermissionProfile;
    use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_WORKSPACE;
    use codex_protocol::request_permissions::PermissionGrantScope;
    use codex_protocol::request_permissions::RequestPermissionProfile as CoreRequestPermissionProfile;
    use codex_protocol::request_permissions::RequestPermissionsResponse;

    async fn config_with_workspace_profile() -> Config {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                default_permissions: Some(BUILT_IN_PERMISSION_PROFILE_WORKSPACE.to_string()),
                ..ConfigOverrides::default()
            })
            .build()
            .await
            .expect("config should build")
    }

    #[tokio::test]
    async fn turn_permissions_use_active_profile_when_available() {
        let config = config_with_workspace_profile().await;
        let active_permission_profile = config.permissions.active_permission_profile();

        assert_eq!(
            App::turn_permissions_override_from_config(
                &config,
                active_permission_profile.as_ref(),
                /*runtime_permission_profile_override*/ None,
            ),
            TurnPermissionsOverride::ActiveProfile(ActivePermissionProfile::new(
                BUILT_IN_PERMISSION_PROFILE_WORKSPACE
            ))
        );
    }

    #[tokio::test]
    async fn turn_permissions_preserve_server_snapshot_without_local_override() {
        let mut config = config_with_workspace_profile().await;
        config
            .permissions
            .set_permission_profile(PermissionProfile::read_only())
            .expect("read-only profile should be allowed");

        assert_eq!(
            App::turn_permissions_override_from_config(
                &config, /*active_permission_profile*/ None,
                /*runtime_permission_profile_override*/ None,
            ),
            TurnPermissionsOverride::Preserve
        );
    }

    #[tokio::test]
    async fn turn_permissions_send_legacy_sandbox_for_local_override() {
        let mut config = config_with_workspace_profile().await;
        let permission_profile = PermissionProfile::workspace_write();
        config
            .permissions
            .set_permission_profile(permission_profile.clone())
            .expect("workspace profile should be allowed");
        let effective_permission_profile = config.permissions.effective_permission_profile();

        assert_eq!(
            App::turn_permissions_override_from_config(
                &config,
                /*active_permission_profile*/ None,
                Some(&permission_profile),
            ),
            TurnPermissionsOverride::LegacySandbox(effective_permission_profile)
        );
    }

    #[tokio::test]
    async fn recovers_missing_app_server_request_mapping_from_thread_event_buffer() {
        let mut app = crate::app::test_support::make_test_app().await;
        let thread_id = ThreadId::new();
        let op = AppCommand::RequestPermissionsResponse {
            id: "permissions-1".to_string(),
            response: RequestPermissionsResponse {
                permissions: CoreRequestPermissionProfile::default(),
                scope: PermissionGrantScope::Turn,
                strict_auto_review: false,
            },
        };

        {
            let channel = app.ensure_thread_channel(thread_id);
            let mut store = channel.store.lock().await;
            store.push_request(ServerRequest::PermissionsRequestApproval {
                request_id: AppServerRequestId::Integer(11),
                params: PermissionsRequestApprovalParams {
                    thread_id: thread_id.to_string(),
                    turn_id: "turn-1".to_string(),
                    item_id: "permissions-1".to_string(),
                    environment_id: None,
                    started_at_ms: 0,
                    cwd: test_path_buf("/tmp/project").abs(),
                    reason: Some("needs permissions".to_string()),
                    permissions: codex_app_server_protocol::RequestPermissionProfile {
                        network: None,
                        file_system: None,
                    },
                },
            });
        }

        let resolution = app
            .recover_app_server_request_resolution(thread_id, &op)
            .await
            .expect("recovery should serialize response")
            .expect("pending request should recover a resolution");
        assert_eq!(resolution.request_id, AppServerRequestId::Integer(11));
        let response: PermissionsRequestApprovalResponse =
            serde_json::from_value(resolution.result)
                .expect("permissions resolution should decode");
        assert_eq!(response.scope, AppServerPermissionGrantScope::Turn);
        assert_eq!(response.strict_auto_review, None);
        assert_eq!(response.permissions.network, None);
        assert_eq!(response.permissions.file_system, None);

        app.note_thread_outbound_op(thread_id, &op).await;

        assert!(
            app.recover_app_server_request_resolution(thread_id, &op)
                .await
                .expect("second recovery should not fail")
                .is_none(),
            "answered request should no longer be recoverable"
        );
    }

    fn thread_id(last_hextet: &str) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{last_hextet:0>12}"))
            .expect("valid thread id")
    }

    /// Asserts that a rendered tree path never leaks any full 36-character thread UUID.
    fn assert_no_full_uuid(rendered: &str, thread_ids: &[ThreadId]) {
        for thread_id in thread_ids {
            let full = thread_id.to_string();
            assert_eq!(full.len(), 36, "thread ids should be 36-char UUIDs");
            assert!(
                !rendered.contains(&full),
                "tree path `{rendered}` must not contain the full UUID `{full}`"
            );
        }
    }

    #[tokio::test]
    async fn agent_tree_path_uses_authoritative_path_with_nickname_leaf_swap() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        let leaf = thread_id("000000000002");
        app.primary_thread_id = Some(root);
        app.agent_navigation
            .set_agent_path(leaf, "/root/backend_audit/db_check".to_string());
        app.agent_navigation.upsert(
            leaf,
            Some("Sleuth".to_string()),
            Some("worker".to_string()),
            /*is_closed*/ false,
        );

        let rendered = app.agent_tree_path(leaf);
        assert_eq!(rendered, "root/backend_audit/Sleuth");
        assert_no_full_uuid(&rendered, &[root, leaf]);
    }

    #[tokio::test]
    async fn agent_tree_path_authoritative_without_nickname_keeps_path_segments() {
        let mut app = crate::app::test_support::make_test_app().await;
        let leaf = thread_id("000000000002");
        app.agent_navigation
            .set_agent_path(leaf, "/root/backend_audit".to_string());
        app.agent_navigation.upsert(
            leaf, /*agent_nickname*/ None, /*agent_role*/ None, /*is_closed*/ false,
        );

        assert_eq!(app.agent_tree_path(leaf), "root/backend_audit");
    }

    #[tokio::test]
    async fn agent_tree_path_walks_parents_labeling_by_nickname_then_role() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        let parent = thread_id("000000000002");
        let leaf = thread_id("000000000003");
        app.primary_thread_id = Some(root);
        app.agent_navigation.upsert(
            parent,
            Some("Scout".to_string()),
            Some("explorer".to_string()),
            /*is_closed*/ false,
        );
        app.agent_navigation.upsert(
            leaf,
            /*agent_nickname*/ None,
            Some("worker".to_string()),
            /*is_closed*/ false,
        );
        app.agent_navigation.set_parent(parent, root);
        app.agent_navigation.set_parent(leaf, parent);

        let rendered = app.agent_tree_path(leaf);
        // Parent labeled by nickname, leaf labeled by `[role]` because it has no nickname.
        assert_eq!(rendered, "root/Scout/[worker]");
        assert_no_full_uuid(&rendered, &[root, parent, leaf]);
    }

    #[tokio::test]
    async fn agent_tree_path_walk_uses_short_id_without_nickname_or_role() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        // The exact UUID from the upstream bug report (codex#23588) that previously leaked in full.
        let leaf =
            ThreadId::from_string("019f8894-89dc-70f2-ad8e-d74deba8ed9b").expect("valid thread id");
        app.primary_thread_id = Some(root);
        app.agent_navigation.upsert(
            leaf, /*agent_nickname*/ None, /*agent_role*/ None, /*is_closed*/ false,
        );
        app.agent_navigation.set_parent(leaf, root);

        let rendered = app.agent_tree_path(leaf);
        assert_eq!(rendered, "root/019f8894");
        assert_no_full_uuid(&rendered, &[root, leaf]);
    }

    #[tokio::test]
    async fn agent_tree_path_collapses_deep_authoritative_paths() {
        let mut app = crate::app::test_support::make_test_app().await;
        let leaf = thread_id("000000000002");
        app.agent_navigation
            .set_agent_path(leaf, "/root/a/b/c/d/e".to_string());
        app.agent_navigation.upsert(
            leaf, /*agent_nickname*/ None, /*agent_role*/ None, /*is_closed*/ false,
        );

        // Six segments collapse to root/…/<parent>/<leaf>.
        assert_eq!(app.agent_tree_path(leaf), "root/…/d/e");
    }

    #[tokio::test]
    async fn agent_tree_path_stops_on_cycle_with_ellipsis_prefix() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        let a = thread_id("00000000000a");
        let b = thread_id("00000000000b");
        app.primary_thread_id = Some(root);
        app.agent_navigation.upsert(
            a,
            Some("Aa".to_string()),
            /*agent_role*/ None,
            /*is_closed*/ false,
        );
        app.agent_navigation.upsert(
            b,
            Some("Bb".to_string()),
            /*agent_role*/ None,
            /*is_closed*/ false,
        );
        // Two-node cycle that never reaches the root.
        app.agent_navigation.set_parent(a, b);
        app.agent_navigation.set_parent(b, a);

        let rendered = app.agent_tree_path(a);
        assert!(
            rendered.starts_with("…/"),
            "unconfirmed chain should be ellipsis-prefixed, got `{rendered}`"
        );
        assert_no_full_uuid(&rendered, &[root, a, b]);
    }

    #[tokio::test]
    async fn agent_tree_path_marks_orphaned_parent_with_ellipsis_prefix() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        let orphan = thread_id("00000000000f");
        let leaf = thread_id("000000000003");
        app.primary_thread_id = Some(root);
        app.agent_navigation.upsert(
            leaf,
            Some("Solo".to_string()),
            /*agent_role*/ None,
            /*is_closed*/ false,
        );
        // Parent edge points at a thread we never cached metadata for.
        app.agent_navigation.set_parent(leaf, orphan);

        let rendered = app.agent_tree_path(leaf);
        assert_eq!(rendered, "…/Solo");
        assert_no_full_uuid(&rendered, &[root, orphan, leaf]);
    }

    #[tokio::test]
    async fn agent_tree_path_primary_falls_back_to_thread_label() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        app.primary_thread_id = Some(root);

        assert_eq!(app.agent_tree_path(root), "Main [default]");
    }

    #[tokio::test]
    async fn agent_tree_path_unknown_thread_never_emits_full_uuid() {
        let mut app = crate::app::test_support::make_test_app().await;
        let root = thread_id("000000000001");
        let unknown =
            ThreadId::from_string("019f8894-89dc-70f2-ad8e-d74deba8ed9b").expect("valid thread id");
        app.primary_thread_id = Some(root);

        let rendered = app.agent_tree_path(unknown);
        assert_eq!(rendered, "Agent (019f8894)");
        assert_no_full_uuid(&rendered, &[root, unknown]);
    }
}
