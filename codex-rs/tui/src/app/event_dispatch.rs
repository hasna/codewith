//! AppEvent dispatch for the TUI app.
//!
//! This module contains the exhaustive `AppEvent` dispatcher and exit-mode handling. Large domain
//! actions are delegated to focused app submodules so the central match remains the routing layer.

use super::resize_reflow::trailing_run_start;
use super::*;
use crate::app_event::MiniMaxUsageRefreshOrigin;
use crate::config_update::format_config_error;
use crate::style::accent_color;
#[cfg(target_os = "windows")]
use codex_config::types::WindowsSandboxModeToml;
use codex_model_provider_info::model_gateway_for_provider;

const SHUTDOWN_FIRST_EXIT_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 2);

impl App {
    fn active_config_profile(&self) -> Option<&str> {
        match &self.config.config_layer_stack.get_active_user_layer()?.name {
            codex_app_server_protocol::ConfigLayerSource::User {
                profile: Some(profile),
                ..
            } => Some(profile.as_str()),
            _ => None,
        }
    }

    async fn select_model_provider_model(
        &mut self,
        app_server: &mut AppServerSession,
        provider_id: String,
        provider_info: ModelProviderInfo,
        models: Vec<ModelPreset>,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        let profile = self.active_config_profile().map(str::to_owned);
        let edits = crate::config_update::build_model_provider_selection_edits(
            profile.as_deref(),
            provider_id.as_str(),
            model.as_str(),
            effort.as_ref(),
        );
        match crate::config_update::write_config_batch(app_server.request_handle(), edits).await {
            Ok(_) => {
                self.config.model_provider_id = provider_id.clone();
                self.config.model_gateway_id = model_gateway_for_provider(&provider_id).to_string();
                self.config.model_provider = provider_info.clone();
                self.config.model = Some(model.clone());
                self.config.model_reasoning_effort = effort.clone();
                let model_catalog =
                    Arc::new(ModelCatalog::new_for_provider(provider_id.clone(), models));
                self.model_catalog = model_catalog.clone();
                let runtime_base_url =
                    super::resolve_runtime_model_provider_base_url(&provider_info).await;
                self.chat_widget.set_model_provider(
                    provider_id.clone(),
                    provider_info,
                    runtime_base_url,
                );
                self.chat_widget.set_model_catalog(model_catalog);
                self.chat_widget.set_model(&model);
                self.on_update_reasoning_effort(effort.clone());
                if self.active_thread_id.is_some() {
                    let op = AppCommand::override_turn_context_model_provider(
                        provider_id.clone(),
                        model.clone(),
                        effort.clone(),
                        Some(self.chat_widget.effective_collaboration_mode()),
                    );
                    if let Err(err) = self.submit_active_thread_op(app_server, op).await {
                        tracing::warn!(
                            error = %err,
                            "failed to apply model provider selection to active thread"
                        );
                        self.chat_widget.add_error_message(format!(
                            "Failed to update active thread provider: {err}"
                        ));
                    }
                }
                self.sync_active_thread_service_tier_to_cached_session()
                    .await;
                self.chat_widget.add_info_message(
                    format!("Provider changed to {provider_id}; current chat uses {model}."),
                    /*hint*/ None,
                );
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to save provider `{provider_id}`: {err}"));
            }
        }
    }

    pub(super) async fn handle_event(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        event: AppEvent,
    ) -> Result<AppRunControl> {
        match event {
            AppEvent::NewSession => {
                self.start_fresh_session_with_summary_hint(
                    tui, app_server, /*session_start_source*/ None,
                    /*initial_user_message*/ None,
                )
                .await;
            }
            AppEvent::StartupThreadStarted { result } => {
                self.handle_startup_thread_started(app_server, result)
                    .await?;
            }
            AppEvent::ClearUi => {
                self.clear_terminal_ui(tui, /*redraw_header*/ false)?;
                self.reset_app_ui_state_after_clear();

                self.start_fresh_session_with_summary_hint(
                    tui,
                    app_server,
                    Some(ThreadStartSource::Clear),
                    /*initial_user_message*/ None,
                )
                .await;
            }
            AppEvent::RawOutputModeChanged { enabled } => {
                self.apply_raw_output_mode(tui, enabled, /*notify*/ false);
            }
            AppEvent::ClearUiAndSubmitUserMessage { text } => {
                self.clear_terminal_ui(tui, /*redraw_header*/ false)?;
                self.reset_app_ui_state_after_clear();

                self.start_fresh_session_with_summary_hint(
                    tui,
                    app_server,
                    Some(ThreadStartSource::Clear),
                    crate::chatwidget::create_initial_user_message(
                        Some(text),
                        Vec::new(),
                        Vec::new(),
                    ),
                )
                .await;
            }
            AppEvent::OpenResumePicker => {
                let picker_app_server = match crate::start_app_server_for_picker(
                    &self.config,
                    &self.app_server_target,
                    self.state_db.clone(),
                    self.environment_manager.clone(),
                )
                .await
                {
                    Ok(app_server) => app_server,
                    Err(err) => {
                        self.chat_widget.add_error_message(format!(
                            "Failed to start TUI session picker: {err}"
                        ));
                        return Ok(AppRunControl::Continue);
                    }
                };
                match crate::resume_picker::run_resume_picker_from_existing_session_with_app_server(
                    tui,
                    &self.config,
                    /*show_all*/ false,
                    /*include_non_interactive*/ false,
                    picker_app_server,
                )
                .await?
                {
                    SessionSelection::Resume(target_session) => {
                        match self
                            .resume_target_session(tui, app_server, target_session)
                            .await?
                        {
                            AppRunControl::Continue => {}
                            AppRunControl::Exit(reason) => {
                                return Ok(AppRunControl::Exit(reason));
                            }
                        }
                    }
                    SessionSelection::Exit | SessionSelection::StartFresh => {
                        self.refresh_in_memory_config_from_disk_best_effort(
                            "closing the session picker",
                        )
                        .await;
                    }
                    SessionSelection::Fork(_) => {}
                }

                // Leaving alt-screen may blank the inline viewport; force a redraw either way.
                tui.frame_requester().schedule_frame();
            }
            AppEvent::ResumeSessionByIdOrName(id_or_name) => {
                match crate::lookup_session_target_with_app_server(app_server, &id_or_name).await? {
                    Some(target_session) => {
                        return self
                            .resume_target_session(tui, app_server, target_session)
                            .await;
                    }
                    None => {
                        self.chat_widget.add_error_message(format!(
                            "No saved chat found matching '{id_or_name}'."
                        ));
                    }
                }
            }
            AppEvent::ArchiveCurrentThread => {
                return Ok(self.archive_current_thread(app_server).await);
            }
            AppEvent::OpenInTmux {
                destination,
                replace_existing,
            } => match self.prepare_tmux_handoff_from_slash(destination, replace_existing) {
                Ok(_) => {
                    self.show_shutdown_feedback(tui)?;
                    return Ok(self
                        .handle_exit_mode(app_server, ExitMode::ShutdownFirst)
                        .await);
                }
                Err(message) => self.chat_widget.add_error_message(message),
            },
            AppEvent::ForkCurrentSession => {
                self.session_telemetry.counter(
                    "codex.thread.fork",
                    /*inc*/ 1,
                    &[("source", "slash_command")],
                );
                let summary = session_summary(
                    self.chat_widget.token_usage(),
                    self.chat_widget.thread_id(),
                    self.chat_widget.thread_name(),
                    self.chat_widget.rollout_path().as_deref(),
                );
                self.chat_widget
                    .add_plain_history_lines(vec!["/fork".magenta().into()]);
                if let Some(thread_id) = self.chat_widget.thread_id() {
                    if self.chat_widget.rollout_path().is_none() {
                        self.chat_widget.add_error_message(
                            "This session is still starting and cannot be forked yet. Send a message first, then try /fork again."
                                .to_string(),
                        );
                        tui.frame_requester().schedule_frame();
                        return Ok(AppRunControl::Continue);
                    }
                    self.refresh_in_memory_config_from_disk_best_effort("forking the thread")
                        .await;
                    match app_server.fork_thread(self.config.clone(), thread_id).await {
                        Ok(forked) => {
                            self.shutdown_current_thread(app_server).await;
                            match self
                                .replace_chat_widget_with_app_server_thread(
                                    tui, app_server, forked, /*initial_user_message*/ None,
                                )
                                .await
                            {
                                Ok(()) => {
                                    if let Some(summary) = summary {
                                        let mut lines: Vec<Line<'static>> = Vec::new();
                                        if let Some(usage_line) = summary.usage_line {
                                            lines.push(usage_line.into());
                                        }
                                        if let Some(command) = summary.resume_hint {
                                            let spans = vec![
                                                "To continue this session, run ".into(),
                                                command.fg(accent_color()),
                                            ];
                                            lines.push(spans.into());
                                        }
                                        self.chat_widget.add_plain_history_lines(lines);
                                    }
                                }
                                Err(err) => {
                                    self.chat_widget.add_error_message(format!(
                                        "Failed to attach to forked app-server thread: {err}"
                                    ));
                                }
                            }
                        }
                        Err(err) => {
                            self.chat_widget.add_error_message(format!(
                                "Failed to fork current session through the app server: {err}"
                            ));
                        }
                    }
                } else {
                    self.chat_widget.add_error_message(
                        "A thread must contain at least one turn before it can be forked."
                            .to_string(),
                    );
                }

                tui.frame_requester().schedule_frame();
            }
            AppEvent::BeginInitialHistoryReplayBuffer => {
                self.begin_initial_history_replay_buffer();
            }
            AppEvent::BeginThreadSwitchHistoryReplayBuffer => {
                self.begin_thread_switch_history_replay_buffer();
            }
            AppEvent::InsertHistoryCell(cell) => {
                let cell: Arc<dyn HistoryCell> = cell.into();
                if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                    t.insert_cell(cell.clone());
                    tui.frame_requester().schedule_frame();
                }
                self.transcript_cells.push(cell.clone());
                if self.initial_history_replay_buffer.as_ref().is_some() {
                    self.insert_history_cell_lines_with_initial_replay_buffer(
                        tui,
                        cell.as_ref(),
                        self.chat_widget
                            .history_wrap_width(tui.terminal.last_known_screen_size.width),
                    );
                } else {
                    self.insert_history_cell_lines(
                        tui,
                        cell.as_ref(),
                        self.chat_widget
                            .history_wrap_width(tui.terminal.last_known_screen_size.width),
                    );
                }
            }
            AppEvent::EndInitialHistoryReplayBuffer => {
                self.finish_initial_history_replay_buffer(tui);
            }
            AppEvent::ConsolidateAgentMessage {
                source,
                cwd,
                scrollback_reflow,
                deferred_history_cell,
            } => {
                self.handle_consolidate_agent_message(
                    tui,
                    source,
                    cwd,
                    scrollback_reflow,
                    deferred_history_cell,
                )?;
            }
            AppEvent::ConsolidateProposedPlan(source) => {
                if !self.terminal_resize_reflow_enabled() {
                    self.transcript_reflow.clear();
                    return Ok(AppRunControl::Continue);
                }
                let end = self.transcript_cells.len();
                let start = trailing_run_start::<history_cell::ProposedPlanStreamCell>(
                    &self.transcript_cells,
                );
                let consolidated: Arc<dyn HistoryCell> =
                    Arc::new(history_cell::new_proposed_plan(source, &self.config.cwd));

                if start < end {
                    self.transcript_cells
                        .splice(start..end, std::iter::once(consolidated.clone()));

                    if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                        t.consolidate_cells(start..end, consolidated.clone());
                        tui.frame_requester().schedule_frame();
                    }

                    self.finish_required_stream_reflow(tui)?;
                } else {
                    self.transcript_cells.push(consolidated.clone());
                    if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                        t.insert_cell(consolidated.clone());
                        tui.frame_requester().schedule_frame();
                    }
                    self.insert_history_cell_lines(
                        tui,
                        consolidated.as_ref(),
                        self.chat_widget
                            .history_wrap_width(tui.terminal.last_known_screen_size.width),
                    );

                    self.maybe_finish_stream_reflow(tui)?;
                }
            }
            AppEvent::ApplyThreadRollback { num_turns } => {
                if self.apply_non_pending_thread_rollback(num_turns) {
                    tui.frame_requester().schedule_frame();
                }
            }
            AppEvent::StartCommitAnimation => {
                if self
                    .commit_anim_running
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    let tx = self.app_event_tx.clone();
                    let running = self.commit_anim_running.clone();
                    thread::spawn(move || {
                        while running.load(Ordering::Relaxed) {
                            thread::sleep(COMMIT_ANIMATION_TICK);
                            tx.send(AppEvent::CommitTick);
                        }
                    });
                }
            }
            AppEvent::StopCommitAnimation => {
                self.commit_anim_running.store(false, Ordering::Release);
            }
            AppEvent::CommitTick => {
                self.chat_widget.on_commit_tick();
            }
            AppEvent::Exit(mode) => {
                if mode == ExitMode::ShutdownFirst {
                    self.show_shutdown_feedback(tui)?;
                }
                return Ok(self.handle_exit_mode(app_server, mode).await);
            }
            AppEvent::Logout => match app_server.logout_account().await {
                Ok(()) => {
                    self.show_shutdown_feedback(tui)?;
                    return Ok(self
                        .handle_exit_mode(app_server, ExitMode::ShutdownFirst)
                        .await);
                }
                Err(err) => {
                    tracing::error!("failed to logout: {err}");
                    self.chat_widget
                        .add_error_message(format!("Logout failed: {err}"));
                }
            },
            AppEvent::FatalExitRequest(message) => {
                return Ok(AppRunControl::Exit(ExitReason::Fatal(message)));
            }
            AppEvent::CodexOp(op) => {
                self.chat_widget.prepare_local_op_submission(&op);
                self.submit_active_thread_op(app_server, op).await?;
            }
            AppEvent::RestoreCancelledTurn(prompt) => {
                self.apply_cancelled_turn_edit(prompt);
            }
            AppEvent::AppendMessageHistoryEntry { thread_id, text } => {
                self.append_message_history_entry(thread_id, text);
            }
            AppEvent::SyncThreadGitBranch { thread_id, branch } => {
                if let Err(err) = app_server
                    .thread_metadata_update_branch(thread_id, branch)
                    .await
                {
                    tracing::warn!("failed to sync thread git branch from directive: {err}");
                }
            }
            AppEvent::LookupMessageHistoryEntry {
                thread_id,
                offset,
                log_id,
            } => {
                self.lookup_message_history_entry(thread_id, offset, log_id)
                    .await?;
            }
            AppEvent::ApproveRecentAutoReviewDenial { thread_id, id } => {
                self.chat_widget
                    .approve_recent_auto_review_denial(thread_id, id);
            }
            AppEvent::SubmitThreadOp { thread_id, op } => {
                self.submit_thread_op(app_server, thread_id, op).await?;
            }
            AppEvent::RequestSessionRecap {
                thread_id,
                prompt,
                automatic,
            } => {
                self.request_session_recap(app_server, thread_id, prompt, automatic);
            }
            AppEvent::MaybeStartAutomaticSessionRecap { thread_id, turn_id } => {
                self.maybe_start_automatic_session_recap(app_server, thread_id, turn_id)
                    .await;
            }
            AppEvent::SessionRecapFinished {
                thread_id,
                automatic,
                result,
            } => {
                self.handle_session_recap_finished(thread_id, automatic, result);
            }
            AppEvent::ThreadHistoryEntryResponse { thread_id, event } => {
                self.enqueue_thread_history_entry_response(thread_id, event)
                    .await?;
            }
            AppEvent::DiffResult(text) => {
                // Clear the in-progress state in the bottom pane
                self.chat_widget.on_diff_complete();
                // Enter alternate screen using TUI helper and build pager lines
                let _ = tui.enter_alt_screen();
                let pager_lines: Vec<ratatui::text::Line<'static>> = if text.trim().is_empty() {
                    vec!["No changes detected.".italic().into()]
                } else {
                    text.lines().map(ansi_escape_line).collect()
                };
                self.overlay = Some(Overlay::new_static_with_lines(
                    pager_lines,
                    "D I F F".to_string(),
                    self.keymap.pager.clone(),
                ));
                tui.frame_requester().schedule_frame();
            }
            AppEvent::OpenPullRequestOverview => {
                self.chat_widget.open_pull_request_overview();
            }
            AppEvent::PullRequestOverviewLoaded {
                request_id,
                overview,
            } => {
                self.chat_widget
                    .show_pull_request_overview(request_id, overview);
            }
            AppEvent::OpenAppLink {
                app_id,
                title,
                description,
                instructions,
                url,
                is_installed,
                is_enabled,
            } => {
                self.chat_widget
                    .open_app_link_view(crate::bottom_pane::AppLinkViewParams {
                        app_id,
                        title,
                        description,
                        instructions,
                        url,
                        is_installed,
                        is_enabled,
                        suggest_reason: None,
                        suggestion_type: None,
                        elicitation_target: None,
                    });
            }
            AppEvent::OpenUrlInBrowser { url } => {
                self.open_url_in_browser(url);
            }
            AppEvent::OpenDesktopThread { thread_id } => {
                self.open_desktop_thread(thread_id);
            }
            AppEvent::PetSelected { pet_id } => {
                self.handle_pet_selected(tui, pet_id);
            }
            AppEvent::PetDisabled => {
                self.handle_pet_disabled(tui).await;
            }
            AppEvent::PetPreviewRequested { pet_id } => {
                self.chat_widget.start_pet_picker_preview(pet_id);
            }
            AppEvent::PetPreviewLoaded { request_id, result } => {
                self.handle_pet_preview_loaded(tui, request_id, result);
            }
            AppEvent::PetSelectionLoaded {
                request_id,
                pet_id,
                result,
            } => {
                return self
                    .handle_pet_selection_loaded(tui, request_id, pet_id, result)
                    .await;
            }
            AppEvent::ConfiguredPetLoaded { pet_id, result } => {
                self.handle_configured_pet_loaded(tui, pet_id, result);
            }
            AppEvent::RefreshConnectors { force_refetch } => {
                self.chat_widget.refresh_connectors(force_refetch);
            }
            AppEvent::FetchConnectorsList { force_refetch } => {
                self.fetch_connectors_list(app_server, force_refetch);
            }
            AppEvent::PluginInstallAuthAdvance { refresh_connectors } => {
                if refresh_connectors {
                    self.chat_widget.refresh_connectors(/*force_refetch*/ true);
                }
                self.chat_widget.advance_plugin_install_auth_flow();
            }
            AppEvent::PluginInstallAuthAbandon => {
                self.chat_widget.abandon_plugin_install_auth_flow();
            }
            AppEvent::FetchPluginsList { cwd } => {
                self.fetch_plugins_list(app_server, cwd);
            }
            AppEvent::FetchHooksList { cwd } => {
                self.fetch_hooks_list(app_server, cwd);
            }
            AppEvent::OpenMarketplaceAddPrompt => {
                self.chat_widget.open_marketplace_add_prompt();
            }
            AppEvent::OpenMarketplaceAddLoading { source } => {
                self.chat_widget.open_marketplace_add_loading_popup(&source);
            }
            AppEvent::OpenMarketplaceRemoveConfirm {
                marketplace_name,
                marketplace_display_name,
            } => {
                self.chat_widget.open_marketplace_remove_confirmation(
                    marketplace_name,
                    marketplace_display_name,
                );
            }
            AppEvent::OpenMarketplaceRemoveLoading {
                marketplace_display_name,
            } => {
                self.chat_widget
                    .open_marketplace_remove_loading_popup(&marketplace_display_name);
            }
            AppEvent::OpenMarketplaceUpgradeLoading { marketplace_name } => {
                self.chat_widget
                    .open_marketplace_upgrade_loading_popup(marketplace_name.as_deref());
            }
            AppEvent::OpenPluginDetailLoading {
                plugin_display_name,
            } => {
                self.chat_widget
                    .open_plugin_detail_loading_popup(&plugin_display_name);
            }
            AppEvent::OpenPluginInstallLoading {
                plugin_display_name,
            } => {
                self.chat_widget
                    .open_plugin_install_loading_popup(&plugin_display_name);
            }
            AppEvent::OpenPluginUninstallLoading {
                plugin_display_name,
            } => {
                self.chat_widget
                    .open_plugin_uninstall_loading_popup(&plugin_display_name);
            }
            AppEvent::PluginsLoaded { cwd, result } => {
                self.chat_widget.on_plugins_loaded(cwd, result);
            }
            AppEvent::HooksLoaded { cwd, result } => {
                self.chat_widget.on_hooks_loaded(cwd, result);
            }
            AppEvent::FetchMarketplaceAdd { cwd, source } => {
                self.fetch_marketplace_add(app_server, cwd, source);
            }
            AppEvent::FetchMarketplaceUpgrade {
                cwd,
                marketplace_name,
            } => {
                self.fetch_marketplace_upgrade(app_server, cwd, marketplace_name);
            }
            AppEvent::MarketplaceAddLoaded {
                cwd,
                source,
                result,
            } => {
                let add_succeeded = result.is_ok();
                self.chat_widget
                    .on_marketplace_add_loaded(cwd.clone(), source, result);
                if add_succeeded && self.chat_widget.config_ref().cwd.as_path() == cwd.as_path() {
                    self.fetch_plugins_list(app_server, cwd);
                }
            }
            AppEvent::MarketplaceUpgradeLoaded { cwd, result } => {
                let marketplace_contents_changed =
                    matches!(&result, Ok(response) if !response.upgraded_roots.is_empty());
                if marketplace_contents_changed {
                    self.refresh_plugin_mentions_after_config_write();
                }
                self.chat_widget
                    .on_marketplace_upgrade_loaded(cwd.clone(), result);
                if self.chat_widget.config_ref().cwd.as_path() == cwd.as_path() {
                    self.fetch_plugins_list(app_server, cwd);
                }
            }
            AppEvent::FetchMarketplaceRemove {
                cwd,
                marketplace_name,
                marketplace_display_name,
            } => {
                self.fetch_marketplace_remove(
                    app_server,
                    cwd,
                    marketplace_name,
                    marketplace_display_name,
                );
            }
            AppEvent::MarketplaceRemoveLoaded {
                cwd,
                marketplace_name,
                marketplace_display_name,
                result,
            } => {
                let remove_succeeded = result.is_ok();
                self.chat_widget.on_marketplace_remove_loaded(
                    cwd.clone(),
                    marketplace_name,
                    marketplace_display_name,
                    result,
                );
                if remove_succeeded && self.chat_widget.config_ref().cwd.as_path() == cwd.as_path()
                {
                    self.refresh_plugin_mentions_after_config_write();
                    self.fetch_plugins_list(app_server, cwd);
                }
            }
            AppEvent::FetchPluginDetail { cwd, params } => {
                self.fetch_plugin_detail(app_server, cwd, params);
            }
            AppEvent::PluginDetailLoaded { cwd, result } => {
                self.chat_widget.on_plugin_detail_loaded(cwd, result);
            }
            AppEvent::FetchPluginInstall {
                cwd,
                marketplace_path,
                plugin_name,
                plugin_display_name,
            } => {
                self.fetch_plugin_install(
                    app_server,
                    cwd,
                    marketplace_path,
                    plugin_name,
                    plugin_display_name,
                );
            }
            AppEvent::FetchPluginUninstall {
                cwd,
                plugin_id,
                plugin_display_name,
            } => {
                self.fetch_plugin_uninstall(app_server, cwd, plugin_id, plugin_display_name);
            }
            AppEvent::SetPluginEnabled {
                cwd,
                plugin_id,
                enabled,
            } => {
                self.set_plugin_enabled(app_server, cwd, plugin_id, enabled);
            }
            AppEvent::PluginInstallLoaded {
                cwd,
                marketplace_path,
                plugin_name,
                plugin_display_name,
                result,
            } => {
                let install_succeeded = result.is_ok();
                if install_succeeded {
                    self.refresh_plugin_mentions_after_config_write();
                }
                let should_refresh_plugin_detail = self.chat_widget.on_plugin_install_loaded(
                    cwd.clone(),
                    marketplace_path.clone(),
                    plugin_name.clone(),
                    plugin_display_name,
                    result,
                );
                if install_succeeded && self.chat_widget.config_ref().cwd.as_path() == cwd.as_path()
                {
                    self.fetch_plugins_list(app_server, cwd.clone());
                    if should_refresh_plugin_detail {
                        self.fetch_plugin_detail(
                            app_server,
                            cwd,
                            PluginReadParams {
                                marketplace_path: Some(marketplace_path),
                                remote_marketplace_name: None,
                                plugin_name,
                            },
                        );
                    }
                }
            }
            AppEvent::PluginEnabledSet {
                cwd,
                plugin_id,
                enabled,
                result,
            } => {
                let queued_enabled = self
                    .pending_plugin_enabled_writes
                    .get_mut(&plugin_id)
                    .and_then(Option::take);
                let should_apply_result = if let Some(queued_enabled) = queued_enabled
                    && (result.is_err() || queued_enabled != enabled)
                {
                    self.spawn_plugin_enabled_write(
                        app_server,
                        cwd.clone(),
                        plugin_id.clone(),
                        queued_enabled,
                    );
                    false
                } else {
                    true
                };
                if should_apply_result {
                    self.pending_plugin_enabled_writes.remove(&plugin_id);
                    let update_succeeded = result.is_ok();
                    if update_succeeded {
                        self.refresh_plugin_mentions_after_config_write();
                    }
                    self.chat_widget
                        .on_plugin_enabled_set(cwd, plugin_id, enabled, result);
                }
            }
            AppEvent::FetchMcpInventory {
                detail,
                thread_id,
                target,
            } => {
                self.fetch_mcp_inventory(app_server, detail, thread_id, target);
            }
            AppEvent::McpInventoryLoaded {
                result,
                detail,
                thread_id,
                target,
            } => {
                self.handle_mcp_inventory_result(result, detail, thread_id, target);
            }
            AppEvent::OpenMcpManager { detail } => {
                self.chat_widget.open_mcp_manager(detail);
            }
            AppEvent::OpenMcpServerDetails { status } => {
                self.chat_widget.open_mcp_server_details(status);
            }
            AppEvent::OpenMcpAddServer => {
                self.chat_widget.open_mcp_add_server();
            }
            AppEvent::AddMcpServer { spec } => {
                if let Err(err) = self.add_mcp_server_from_spec(app_server, spec).await {
                    tracing::warn!("failed to add MCP server from TUI action: {err}");
                }
            }
            AppEvent::SetMcpServerEnabled { name, enabled } => {
                if let Err(err) = self.set_mcp_server_enabled(app_server, name, enabled).await {
                    tracing::warn!("failed to set MCP server enabled from TUI action: {err}");
                }
            }
            AppEvent::SetMcpToolEnabled {
                server,
                tool,
                enabled,
            } => {
                if let Err(err) = self
                    .set_mcp_tool_enabled(app_server, server, tool, enabled)
                    .await
                {
                    tracing::warn!("failed to set MCP tool enabled from TUI action: {err}");
                }
            }
            AppEvent::StartMcpServerOauthLogin { name } => {
                self.start_mcp_server_oauth_login(app_server, name);
            }
            AppEvent::ReloadMcpServers => {
                self.reload_mcp_servers(app_server);
            }
            AppEvent::OpenBackgroundTerminalManager => {
                self.chat_widget.open_background_terminal_manager();
            }
            AppEvent::OpenBackgroundTerminalStopConfirmation => {
                self.chat_widget
                    .open_background_terminal_stop_confirmation();
            }
            AppEvent::StopBackgroundTerminals => {
                self.chat_widget.stop_background_terminals();
            }
            AppEvent::PrintBackgroundTerminals => {
                self.chat_widget.add_ps_output();
            }
            AppEvent::McpServersReloaded { result } => match result {
                Ok(()) => {
                    self.chat_widget.add_info_message(
                        "MCP reload queued.".to_string(),
                        Some(
                            "Loaded threads pick up new or updated tools before the next turn. Reopening /mcp inventory now.".to_string(),
                        ),
                    );
                    self.chat_widget
                        .open_mcp_manager(McpServerStatusDetail::Full);
                }
                Err(err) => {
                    self.chat_widget
                        .add_error_message(format!("Failed to reload MCP servers: {err}"));
                }
            },
            AppEvent::ShowMcpSetupHelp => {
                self.chat_widget.add_info_message(
                    "Use /mcp add <name> <url-or-command...> to add MCP servers.".to_string(),
                    Some(
                        "Use --env-var KEY for secrets already in your shell, --bearer-env KEY for HTTP bearer tokens, or edit config.toml directly under [mcp_servers.<name>]. Codewith auto-refreshes MCP tools after managed config changes.".to_string(),
                    ),
                );
            }
            AppEvent::ShowMcpDiagnosticsHelp { name } => {
                self.chat_widget.add_info_message(
                    format!("Diagnose MCP server `{name}`."),
                    Some(
                        "Check command/cwd/env for stdio servers and URL/auth/headers for HTTP servers. Managed config changes auto-refresh; /mcp reload remains a diagnostic fallback.".to_string(),
                    ),
                );
            }
            AppEvent::McpServerOauthLoginStarted { name, result } => match result {
                Ok(url) => {
                    self.open_url_in_browser(url);
                }
                Err(err) => {
                    self.chat_widget.add_error_message(format!(
                        "Failed to start MCP OAuth login for {name}: {err}"
                    ));
                }
            },
            AppEvent::SkillsListLoaded { result } => {
                self.handle_skills_list_result(
                    result.map_err(|err| color_eyre::eyre::eyre!(err)),
                    "failed to load skills on startup",
                );
            }
            AppEvent::StartFileSearch(query) => {
                self.file_search.on_user_query(query);
            }
            AppEvent::FileSearchResult { query, matches } => {
                self.chat_widget.apply_file_search_result(query, matches);
            }
            AppEvent::RefreshRateLimits { origin, target } => {
                self.refresh_rate_limits(app_server, origin, target);
            }
            AppEvent::RefreshAuthProfileUsageHeartbeats => {
                for target in self.chat_widget.auth_profile_usage_refresh_targets() {
                    self.refresh_rate_limits(app_server, RateLimitRefreshOrigin::Heartbeat, target);
                }
            }
            AppEvent::RefreshMiniMaxUsage { origin } => {
                self.refresh_minimax_usage(origin);
            }
            AppEvent::OpenThreadGoalMenu { thread_id } => {
                self.open_thread_goal_menu(app_server, thread_id).await;
            }
            AppEvent::OpenMissionControlOverview => {
                self.open_mission_control_overview(app_server).await;
            }
            AppEvent::OpenMissionControlInteractionAnswer { interaction } => {
                self.chat_widget
                    .show_mission_control_answer_prompt(interaction);
            }
            AppEvent::RespondMissionControlInteraction {
                interaction_id,
                thread_id,
                terminal_status,
                response,
            } => {
                self.respond_mission_control_interaction(
                    app_server,
                    interaction_id,
                    thread_id,
                    terminal_status,
                    response,
                )
                .await;
            }
            AppEvent::ManageThreadWorkflow { thread_id, action } => {
                self.manage_thread_workflow(app_server, thread_id, action)
                    .await;
            }
            AppEvent::OpenThreadGoalPlanDetail { thread_id, plan } => {
                self.chat_widget.show_goal_plan_detail(thread_id, plan);
            }
            AppEvent::OpenThreadGoalEditor { thread_id } => {
                self.open_thread_goal_editor(app_server, thread_id).await;
            }
            AppEvent::SetThreadGoalObjective {
                thread_id,
                objective,
                mode,
            } => {
                self.set_thread_goal_objective(app_server, thread_id, objective, mode)
                    .await;
            }
            AppEvent::SetThreadGoalStatus { thread_id, status } => {
                self.set_thread_goal_status(app_server, thread_id, status)
                    .await;
            }
            AppEvent::ClearThreadGoal { thread_id } => {
                self.clear_thread_goal(app_server, thread_id).await;
            }
            AppEvent::ActivateThreadGoalPlanNode { thread_id, node_id } => {
                self.activate_thread_goal_plan_node(app_server, thread_id, node_id)
                    .await;
            }
            AppEvent::OpenThreadLoopManager { thread_id } => {
                self.open_thread_loop_manager(app_server, thread_id).await;
            }
            AppEvent::OpenThreadLoopEditor {
                thread_id,
                schedule_id,
            } => {
                self.open_thread_loop_editor(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::OpenThreadLoopScheduleActions {
                thread_id,
                schedule_id,
            } => {
                self.open_thread_loop_schedule_actions(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::OpenThreadLoopScheduleStats {
                thread_id,
                schedule_id,
            } => {
                self.open_thread_loop_schedule_stats(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::CreateThreadLoopSchedule {
                thread_id,
                prompt,
                prompt_source,
                schedule,
            } => {
                self.create_thread_loop_schedule(
                    app_server,
                    thread_id,
                    prompt,
                    prompt_source,
                    schedule,
                )
                .await;
            }
            AppEvent::UpdateThreadLoopSchedulePrompt {
                thread_id,
                schedule_id,
                prompt,
            } => {
                self.update_thread_loop_schedule_prompt(app_server, thread_id, schedule_id, prompt)
                    .await;
            }
            AppEvent::PauseThreadLoopSchedule {
                thread_id,
                schedule_id,
            } => {
                self.pause_thread_loop_schedule(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::ResumeThreadLoopSchedule {
                thread_id,
                schedule_id,
            } => {
                self.resume_thread_loop_schedule(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::DeleteThreadLoopSchedule {
                thread_id,
                schedule_id,
            } => {
                self.delete_thread_loop_schedule(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::RunThreadLoopScheduleNow {
                thread_id,
                schedule_id,
            } => {
                self.run_thread_loop_schedule_now(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::OpenThreadScheduleManager { thread_id } => {
                self.open_thread_schedule_manager(app_server, thread_id)
                    .await;
            }
            AppEvent::OpenThreadScheduleEditor {
                thread_id,
                schedule_id,
            } => {
                self.open_thread_schedule_editor(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::OpenThreadScheduleActions {
                thread_id,
                schedule_id,
            } => {
                self.open_thread_schedule_actions(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::OpenThreadScheduleStats {
                thread_id,
                schedule_id,
            } => {
                self.open_thread_schedule_stats(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::CreateThreadSchedule {
                thread_id,
                prompt,
                prompt_source,
                schedule,
                next_run_at,
            } => {
                self.create_thread_schedule(
                    app_server,
                    thread_id,
                    prompt,
                    prompt_source,
                    schedule,
                    next_run_at,
                )
                .await;
            }
            AppEvent::UpdateThreadSchedulePrompt {
                thread_id,
                schedule_id,
                prompt,
            } => {
                self.update_thread_schedule_prompt(app_server, thread_id, schedule_id, prompt)
                    .await;
            }
            AppEvent::PauseThreadSchedule {
                thread_id,
                schedule_id,
            } => {
                self.pause_thread_schedule(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::ResumeThreadSchedule {
                thread_id,
                schedule_id,
            } => {
                self.resume_thread_schedule(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::DeleteThreadSchedule {
                thread_id,
                schedule_id,
            } => {
                self.delete_thread_schedule(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::RunThreadScheduleNow {
                thread_id,
                schedule_id,
            } => {
                self.run_thread_schedule_now(app_server, thread_id, schedule_id)
                    .await;
            }
            AppEvent::OpenThreadMonitorManager { thread_id } => {
                self.open_thread_monitor_manager(app_server, thread_id)
                    .await;
            }
            AppEvent::OpenThreadMonitorActions {
                thread_id,
                monitor_id,
            } => {
                self.open_thread_monitor_actions(app_server, thread_id, monitor_id)
                    .await;
            }
            AppEvent::ReadThreadMonitor {
                thread_id,
                monitor_id,
            } => {
                self.read_thread_monitor(app_server, thread_id, monitor_id)
                    .await;
            }
            AppEvent::StopThreadMonitor {
                thread_id,
                monitor_id,
            } => {
                self.stop_thread_monitor(app_server, thread_id, monitor_id)
                    .await;
            }
            AppEvent::RestartThreadMonitor {
                thread_id,
                monitor_id,
            } => {
                self.restart_thread_monitor(app_server, thread_id, monitor_id)
                    .await;
            }
            AppEvent::DeleteThreadMonitor {
                thread_id,
                monitor_id,
            } => {
                self.delete_thread_monitor(app_server, thread_id, monitor_id)
                    .await;
            }
            AppEvent::OpenBackgroundAgentManager => {
                self.open_background_agent_manager(app_server).await;
            }
            AppEvent::OpenBackgroundAgentActions { agent_id } => {
                self.open_background_agent_actions(app_server, agent_id)
                    .await;
            }
            AppEvent::StartBackgroundAgent {
                prompt,
                worktree_id,
            } => {
                self.start_background_agent(app_server, prompt, worktree_id)
                    .await;
            }
            AppEvent::StartExternalAgentChildThread {
                runtime_id,
                runtime_display_name,
                task,
                mode,
            } => {
                self.start_external_agent_child_thread(
                    tui,
                    app_server,
                    runtime_id,
                    runtime_display_name,
                    task,
                    mode,
                )
                .await;
            }
            AppEvent::ReadBackgroundAgent { agent_id } => {
                self.read_background_agent(app_server, agent_id).await;
            }
            AppEvent::AttachBackgroundAgent { agent_id } => {
                self.attach_background_agent(tui, app_server, agent_id)
                    .await;
            }
            AppEvent::ShowBackgroundAgentLogs { agent_id } => {
                self.show_background_agent_logs(app_server, agent_id).await;
            }
            AppEvent::DetachBackgroundAgent { agent_id } => {
                self.detach_background_agent(app_server, agent_id).await;
            }
            AppEvent::StopBackgroundAgent { agent_id } => {
                self.stop_background_agent(app_server, agent_id).await;
            }
            AppEvent::DeleteBackgroundAgent { agent_id } => {
                self.delete_background_agent(app_server, agent_id).await;
            }
            AppEvent::ShowBackgroundAgentDiagnostics => {
                self.show_background_agent_diagnostics(app_server).await;
            }
            AppEvent::OpenWorktreeManager => {
                self.open_worktree_manager(app_server).await;
            }
            AppEvent::ReconcileWorktrees => {
                self.reconcile_worktrees(app_server).await;
            }
            AppEvent::CreateWorktree {
                name,
                branch,
                start_point,
            } => {
                self.create_worktree(app_server, name, branch, start_point)
                    .await;
            }
            AppEvent::OpenWorktreeActions {
                worktree_id,
                base_repo_path,
            } => {
                self.open_worktree_actions(app_server, worktree_id, base_repo_path)
                    .await;
            }
            AppEvent::ReadWorktree {
                worktree_id,
                base_repo_path,
            } => {
                self.read_worktree(app_server, worktree_id, base_repo_path)
                    .await;
            }
            AppEvent::UseWorktree {
                worktree_id,
                base_repo_path,
            } => {
                self.use_worktree(app_server, worktree_id, base_repo_path)
                    .await;
            }
            AppEvent::ReleaseWorktree {
                worktree_id,
                base_repo_path,
            } => {
                self.release_worktree(app_server, worktree_id, base_repo_path)
                    .await;
            }
            AppEvent::CleanupWorktree {
                worktree_id,
                base_repo_path,
                force_delete,
            } => {
                self.cleanup_worktree(app_server, worktree_id, base_repo_path, force_delete)
                    .await;
            }
            AppEvent::RefreshWorktreeMergeCandidate {
                worktree_id,
                base_repo_path,
                target_ref,
            } => {
                self.refresh_worktree_merge_candidate(
                    app_server,
                    worktree_id,
                    base_repo_path,
                    target_ref,
                )
                .await;
            }
            AppEvent::ListActiveSessions => {
                self.list_active_sessions(app_server).await;
            }
            AppEvent::SendActiveSessionMessage {
                target_peer_id,
                message,
                wake,
            } => {
                self.send_active_session_message(app_server, target_peer_id, message, wake)
                    .await;
            }
            AppEvent::PrefillComposer { text } => {
                self.chat_widget
                    .restore_user_message_to_composer(crate::chatwidget::UserMessage::from(text));
            }
            AppEvent::SendAddCreditsNudgeEmail { credit_type } => {
                if self
                    .chat_widget
                    .start_add_credits_nudge_email_request(credit_type)
                {
                    self.send_add_credits_nudge_email(app_server, credit_type);
                }
            }
            AppEvent::AddCreditsNudgeEmailFinished { result } => {
                self.chat_widget
                    .finish_add_credits_nudge_email_request(result);
            }
            AppEvent::RateLimitsLoaded {
                origin,
                target,
                auth_profile,
                result,
            } => {
                if self.apply_rate_limits_loaded(origin, target, auth_profile, result)
                    == RateLimitRefreshCompletion::ScheduleFrame
                {
                    tui.frame_requester().schedule_frame();
                }
            }
            AppEvent::MiniMaxUsageLoaded {
                origin,
                auth_profile,
                result,
            } => {
                self.apply_minimax_usage_loaded(origin, auth_profile, result);
            }
            AppEvent::UsageSelfHealRetry { retry_id } => {
                if self.chat_widget.on_usage_self_heal_retry(retry_id) {
                    tui.frame_requester().schedule_frame();
                }
            }
            AppEvent::ConnectorsLoaded { result, is_final } => {
                self.chat_widget.on_connectors_loaded(result, is_final);
            }
            AppEvent::UpdateReasoningEffort(effort) => {
                self.on_update_reasoning_effort(effort.clone());
                self.sync_active_thread_reasoning_setting(app_server, effort)
                    .await;
            }
            AppEvent::UpdateModel(model) => {
                self.chat_widget.set_model(&model);
                self.sync_active_thread_model_setting(app_server, model)
                    .await;
                self.sync_active_thread_service_tier_to_cached_session()
                    .await;
            }
            AppEvent::SelectModelProvider { provider_id } => {
                let Some(provider_info) = self.config.model_providers.get(&provider_id).cloned()
                else {
                    self.chat_widget
                        .add_error_message(format!("Unknown model provider: {provider_id}"));
                    return Ok(AppRunControl::Continue);
                };
                let models = match app_server
                    .list_models_for_provider(provider_id.clone())
                    .await
                {
                    Ok(models) => models,
                    Err(err) => {
                        self.chat_widget.add_error_message(format!(
                            "Failed to load models for provider `{provider_id}`: {err}"
                        ));
                        return Ok(AppRunControl::Continue);
                    }
                };
                let selected = models
                    .iter()
                    .find(|model| model.show_in_picker && model.is_default)
                    .or_else(|| models.iter().find(|model| model.show_in_picker))
                    .or_else(|| models.first())
                    .cloned();
                let Some(selected) = selected else {
                    self.chat_widget.add_error_message(format!(
                        "Provider `{provider_id}` did not return any models."
                    ));
                    return Ok(AppRunControl::Continue);
                };
                let effort = Some(selected.default_reasoning_effort);
                self.select_model_provider_model(
                    app_server,
                    provider_id,
                    provider_info,
                    models,
                    selected.model,
                    effort,
                )
                .await;
            }
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort,
            } => {
                let Some(provider_info) = self.config.model_providers.get(&provider_id).cloned()
                else {
                    self.chat_widget
                        .add_error_message(format!("Unknown model provider: {provider_id}"));
                    return Ok(AppRunControl::Continue);
                };
                let models = if self.model_catalog.provider_id() == Some(provider_id.as_str()) {
                    self.model_catalog.try_list_models().unwrap_or_default()
                } else {
                    match app_server
                        .list_models_for_provider(provider_id.clone())
                        .await
                    {
                        Ok(models) => models,
                        Err(err) => {
                            self.chat_widget.add_error_message(format!(
                                "Failed to load models for provider `{provider_id}`: {err}"
                            ));
                            return Ok(AppRunControl::Continue);
                        }
                    }
                };
                self.select_model_provider_model(
                    app_server,
                    provider_id,
                    provider_info,
                    models,
                    model,
                    effort,
                )
                .await;
            }
            AppEvent::SwitchAuthProfile {
                profile,
                reason,
                resume_queued_input,
            } => {
                self.submit_auth_profile_switch(profile, &reason, resume_queued_input)
                    .await;
            }
            AppEvent::OpenAuthProfileRenamePrompt { profile } => {
                self.chat_widget.open_auth_profile_rename_prompt(profile);
            }
            AppEvent::OpenAuthProfileSettings { profile } => {
                self.chat_widget.open_auth_profile_settings_popup(profile);
            }
            AppEvent::OpenAuthProfileDeleteConfirm { profile } => {
                self.chat_widget.open_auth_profile_delete_confirm(profile);
            }
            AppEvent::ReloginAuthProfile { profile } => {
                self.relogin_auth_profile(profile);
            }
            AppEvent::AuthProfileReloginFinished { profile, result } => {
                self.finish_auth_profile_relogin(profile, result);
            }
            AppEvent::OpenAuthProfileLoginPrompt => {
                self.chat_widget.open_auth_profile_login_prompt();
            }
            AppEvent::OpenAuthProfileNamePrompt {
                subscription_provider,
            } => {
                self.chat_widget
                    .open_auth_profile_name_prompt(subscription_provider);
            }
            AppEvent::LoginNewAuthProfile {
                profile,
                subscription_provider,
            } => {
                self.chat_widget
                    .start_auth_profile_login(profile, subscription_provider);
            }
            AppEvent::AuthProfileLoginCompleted {
                profile,
                success,
                error,
            } => {
                self.complete_auth_profile_login(profile, success, error);
            }
            AppEvent::RenameAuthProfile { old_name, new_name } => {
                self.rename_auth_profile(old_name, new_name);
            }
            AppEvent::DeleteAuthProfile { profile } => {
                self.delete_auth_profile(profile);
            }
            AppEvent::MoveAuthProfile { profile, direction } => {
                self.move_auth_profile(profile, direction);
            }
            AppEvent::UpdatePersonality(personality) => {
                self.on_update_personality(personality);
                self.sync_active_thread_personality_setting(app_server, personality)
                    .await;
            }
            AppEvent::UpdateConfigValue {
                key_path,
                value,
                label,
            } => {
                self.update_config_value_with_app_server(app_server, key_path, value, label)
                    .await;
            }
            AppEvent::OpenConfigMenu => {
                self.chat_widget.open_config_popup();
            }
            AppEvent::OpenConfigSection { section } => {
                self.chat_widget.open_config_section_popup(section);
            }
            AppEvent::OpenRealtimeAudioDeviceSelection { kind } => {
                self.chat_widget.open_realtime_audio_device_selection(kind);
            }
            AppEvent::RealtimeWebrtcOfferCreated { result } => {
                self.chat_widget.on_realtime_webrtc_offer_created(result);
            }
            AppEvent::RealtimeWebrtcEvent(event) => {
                self.chat_widget.on_realtime_webrtc_event(event);
            }
            AppEvent::RealtimeWebrtcLocalAudioLevel(peak) => {
                self.chat_widget.on_realtime_webrtc_local_audio_level(peak);
            }
            AppEvent::OpenReasoningPopup { model } => {
                self.chat_widget.open_reasoning_popup(model);
            }
            AppEvent::OpenPlanReasoningScopePrompt { model, effort } => {
                self.chat_widget
                    .open_plan_reasoning_scope_prompt(model, effort);
            }
            AppEvent::OpenAllModelsPopup { models } => {
                self.chat_widget.open_all_models_popup(models);
            }
            AppEvent::OpenFullAccessConfirmation {
                preset,
                return_to_permissions,
                profile_selection,
            } => {
                self.chat_widget.open_full_access_confirmation(
                    preset,
                    return_to_permissions,
                    profile_selection,
                );
            }
            AppEvent::OpenWorldWritableWarningConfirmation {
                preset,
                profile_selection,
                sample_paths,
                extra_count,
                failed_scan,
            } => {
                self.chat_widget.open_world_writable_warning_confirmation(
                    preset,
                    profile_selection,
                    sample_paths,
                    extra_count,
                    failed_scan,
                );
            }
            AppEvent::OpenFeedbackNote {
                category,
                include_logs,
            } => {
                self.chat_widget.open_feedback_note(category, include_logs);
            }
            AppEvent::OpenFeedbackConsent { category } => {
                self.chat_widget.open_feedback_consent(category);
            }
            AppEvent::SubmitFeedback {
                category,
                reason,
                turn_id,
                include_logs,
            } => {
                self.submit_feedback(app_server, category, reason, turn_id, include_logs);
            }
            AppEvent::FeedbackSubmitted {
                origin_thread_id,
                category,
                reason,
                include_logs,
                result,
            } => {
                self.handle_feedback_submitted(
                    origin_thread_id,
                    category,
                    reason,
                    include_logs,
                    result,
                )
                .await;
            }
            AppEvent::LaunchExternalEditor => {
                if self.chat_widget.external_editor_state() == ExternalEditorState::Active {
                    self.launch_external_editor(tui).await;
                }
            }
            AppEvent::OpenWindowsSandboxEnablePrompt {
                preset,
                profile_selection,
            } => {
                self.chat_widget
                    .open_windows_sandbox_enable_prompt(preset, profile_selection);
            }
            AppEvent::OpenWindowsSandboxFallbackPrompt {
                preset,
                profile_selection,
            } => {
                self.session_telemetry.counter(
                    "codex.windows_sandbox.fallback_prompt_shown",
                    /*inc*/ 1,
                    &[],
                );
                self.chat_widget.clear_windows_sandbox_setup_status();
                if let Some(started_at) = self.windows_sandbox.setup_started_at.take() {
                    self.session_telemetry.record_duration(
                        "codex.windows_sandbox.elevated_setup_duration_ms",
                        started_at.elapsed(),
                        &[("result", "failure")],
                    );
                }
                self.chat_widget
                    .open_windows_sandbox_fallback_prompt(preset, profile_selection);
            }
            AppEvent::BeginWindowsSandboxElevatedSetup {
                preset,
                profile_selection,
            } => {
                #[cfg(any(target_os = "windows", test))]
                if !self.chat_widget.windows_sandbox_mode_allowed(
                    codex_config::types::WindowsSandboxModeToml::Elevated,
                ) {
                    tracing::warn!(
                        "refusing to set up elevated Windows sandbox mode disallowed by requirements"
                    );
                    self.chat_widget.add_info_message(
                        "That Windows sandbox option is disallowed by requirements.".to_string(),
                        /*hint*/ None,
                    );
                    return Ok(AppRunControl::Continue);
                }
                #[cfg(target_os = "windows")]
                {
                    let setup_permissions = match self
                        .windows_setup_permissions(&preset, profile_selection.as_ref())
                        .await
                    {
                        Ok(setup_permissions) => setup_permissions,
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "failed to resolve permission profile for elevated Windows sandbox setup"
                            );
                            self.chat_widget.add_error_message(format!(
                                "Failed to prepare Windows sandbox for the selected permission profile: {err}"
                            ));
                            return Ok(AppRunControl::Continue);
                        }
                    };
                    let permission_profile = setup_permissions.permission_profile;
                    let workspace_roots = setup_permissions.workspace_roots;
                    let command_cwd = self.config.cwd.clone();
                    let env_map: std::collections::HashMap<String, String> =
                        std::env::vars().collect();
                    let codex_home = self.config.codex_home.clone();
                    let tx = self.app_event_tx.clone();

                    // If the elevated setup already ran on this machine, don't prompt for
                    // elevation again - just flip the config to use the elevated path.
                    if crate::legacy_core::windows_sandbox::sandbox_setup_is_complete(
                        codex_home.as_path(),
                    ) {
                        tx.send(AppEvent::EnableWindowsSandboxForAgentMode {
                            preset,
                            mode: WindowsSandboxEnableMode::Elevated,
                            profile_selection,
                        });
                        return Ok(AppRunControl::Continue);
                    }

                    self.chat_widget.show_windows_sandbox_setup_status();
                    self.windows_sandbox.setup_started_at = Some(Instant::now());
                    let session_telemetry = self.session_telemetry.clone();
                    tokio::task::spawn_blocking(move || {
                        let result = crate::legacy_core::windows_sandbox::run_elevated_setup(
                            &permission_profile,
                            workspace_roots.as_slice(),
                            command_cwd.as_path(),
                            &env_map,
                            codex_home.as_path(),
                        );
                        let event = match result {
                            Ok(()) => {
                                session_telemetry.counter(
                                    "codex.windows_sandbox.elevated_setup_success",
                                    /*inc*/ 1,
                                    &[],
                                );
                                AppEvent::EnableWindowsSandboxForAgentMode {
                                    preset: preset.clone(),
                                    mode: WindowsSandboxEnableMode::Elevated,
                                    profile_selection: profile_selection.clone(),
                                }
                            }
                            Err(err) => {
                                let mut code_tag: Option<String> = None;
                                let mut message_tag: Option<String> = None;
                                if let Some((code, message)) =
                                    crate::legacy_core::windows_sandbox::elevated_setup_failure_details(
                                        &err,
                                    )
                                {
                                    code_tag = Some(code);
                                    message_tag = Some(message);
                                }
                                let mut tags: Vec<(&str, &str)> = Vec::new();
                                if let Some(code) = code_tag.as_deref() {
                                    tags.push(("code", code));
                                }
                                if let Some(message) = message_tag.as_deref() {
                                    tags.push(("message", message));
                                }
                                session_telemetry.counter(
                                    crate::legacy_core::windows_sandbox::elevated_setup_failure_metric_name(
                                        &err,
                                    ),
                                    /*inc*/ 1,
                                    &tags,
                                );
                                tracing::error!(
                                    error = %err,
                                    "failed to run elevated Windows sandbox setup"
                                );
                                AppEvent::OpenWindowsSandboxFallbackPrompt {
                                    preset,
                                    profile_selection,
                                }
                            }
                        };
                        tx.send(event);
                    });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (preset, profile_selection);
                }
            }
            AppEvent::BeginWindowsSandboxLegacySetup {
                preset,
                profile_selection,
            } => {
                #[cfg(any(target_os = "windows", test))]
                if !self.chat_widget.windows_sandbox_mode_allowed(
                    codex_config::types::WindowsSandboxModeToml::Unelevated,
                ) {
                    tracing::warn!(
                        "refusing to set up unelevated Windows sandbox mode disallowed by requirements"
                    );
                    self.chat_widget.add_info_message(
                        "That Windows sandbox option is disallowed by requirements.".to_string(),
                        /*hint*/ None,
                    );
                    return Ok(AppRunControl::Continue);
                }
                #[cfg(target_os = "windows")]
                {
                    let setup_permissions = match self
                        .windows_setup_permissions(&preset, profile_selection.as_ref())
                        .await
                    {
                        Ok(setup_permissions) => setup_permissions,
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "failed to resolve permission profile for legacy Windows sandbox setup"
                            );
                            self.chat_widget.add_error_message(format!(
                                "Failed to prepare Windows sandbox for the selected permission profile: {err}"
                            ));
                            return Ok(AppRunControl::Continue);
                        }
                    };
                    let permission_profile = setup_permissions.permission_profile;
                    let workspace_roots = setup_permissions.workspace_roots;
                    let command_cwd = self.config.cwd.clone();
                    let env_map: std::collections::HashMap<String, String> =
                        std::env::vars().collect();
                    let codex_home = self.config.codex_home.clone();
                    let tx = self.app_event_tx.clone();
                    let session_telemetry = self.session_telemetry.clone();

                    self.chat_widget.show_windows_sandbox_setup_status();
                    tokio::task::spawn_blocking(move || {
                        if let Err(err) =
                            crate::legacy_core::windows_sandbox::run_legacy_setup_preflight(
                                &permission_profile,
                                workspace_roots.as_slice(),
                                command_cwd.as_path(),
                                &env_map,
                                codex_home.as_path(),
                            )
                        {
                            session_telemetry.counter(
                                "codex.windows_sandbox.legacy_setup_preflight_failed",
                                /*inc*/ 1,
                                &[],
                            );
                            tracing::warn!(
                                error = %err,
                                "failed to preflight non-admin Windows sandbox setup"
                            );
                        }
                        tx.send(AppEvent::EnableWindowsSandboxForAgentMode {
                            preset,
                            mode: WindowsSandboxEnableMode::Legacy,
                            profile_selection,
                        });
                    });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (preset, profile_selection);
                }
            }
            AppEvent::BeginWindowsSandboxGrantReadRoot { path } => {
                #[cfg(target_os = "windows")]
                {
                    self.chat_widget
                        .add_to_history(history_cell::new_info_event(
                            format!("Granting sandbox read access to {path} ..."),
                            /*hint*/ None,
                        ));

                    let permission_profile = self.config.permissions.effective_permission_profile();
                    let workspace_roots = self.config.effective_workspace_roots();
                    let command_cwd = self.config.cwd.clone();
                    let env_map: std::collections::HashMap<String, String> =
                        std::env::vars().collect();
                    let codex_home = self.config.codex_home.clone();
                    let tx = self.app_event_tx.clone();

                    tokio::task::spawn_blocking(move || {
                        let requested_path = PathBuf::from(path);
                        let event = match crate::legacy_core::grant_read_root_non_elevated(
                            &permission_profile,
                            workspace_roots.as_slice(),
                            command_cwd.as_path(),
                            &env_map,
                            codex_home.as_path(),
                            requested_path.as_path(),
                        ) {
                            Ok(canonical_path) => AppEvent::WindowsSandboxGrantReadRootCompleted {
                                path: canonical_path,
                                error: None,
                            },
                            Err(err) => AppEvent::WindowsSandboxGrantReadRootCompleted {
                                path: requested_path,
                                error: Some(err.to_string()),
                            },
                        };
                        tx.send(event);
                    });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = path;
                }
            }
            AppEvent::WindowsSandboxGrantReadRootCompleted { path, error } => match error {
                Some(err) => {
                    self.chat_widget
                        .add_to_history(history_cell::new_error_event(format!("Error: {err}")));
                }
                None => {
                    self.chat_widget
                        .add_to_history(history_cell::new_info_event(
                            format!("Sandbox read access granted for {}", path.display()),
                            /*hint*/ None,
                        ));
                }
            },
            AppEvent::EnableWindowsSandboxForAgentMode {
                preset,
                mode,
                profile_selection,
            } => {
                #[cfg(target_os = "windows")]
                {
                    self.chat_widget.clear_windows_sandbox_setup_status();
                    if let Some(started_at) = self.windows_sandbox.setup_started_at.take() {
                        self.session_telemetry.record_duration(
                            "codex.windows_sandbox.elevated_setup_duration_ms",
                            started_at.elapsed(),
                            &[("result", "success")],
                        );
                    }
                    let selected_mode = match mode {
                        WindowsSandboxEnableMode::Elevated => WindowsSandboxModeToml::Elevated,
                        WindowsSandboxEnableMode::Legacy => WindowsSandboxModeToml::Unelevated,
                    };
                    let elevated_enabled = selected_mode == WindowsSandboxModeToml::Elevated;
                    if !self.chat_widget.windows_sandbox_mode_allowed(selected_mode) {
                        tracing::warn!(
                            ?selected_mode,
                            "refusing to persist Windows sandbox mode disallowed by requirements"
                        );
                        self.chat_widget.add_info_message(
                            "That Windows sandbox option is disallowed by requirements."
                                .to_string(),
                            /*hint*/ None,
                        );
                        return Ok(AppRunControl::Continue);
                    }
                    let edits =
                        crate::config_update::build_windows_sandbox_mode_edits(elevated_enabled);
                    match crate::config_update::write_config_batch(
                        app_server.request_handle(),
                        edits,
                    )
                    .await
                    {
                        Ok(response) if response.status == WriteStatus::OkOverridden => {
                            self.sync_windows_sandbox_after_overridden_write(app_server, &response)
                                .await;
                        }
                        Ok(_) => {
                            if elevated_enabled {
                                self.config.set_windows_sandbox_enabled(/*value*/ false);
                                self.config
                                    .set_windows_elevated_sandbox_enabled(/*value*/ true);
                            } else {
                                self.config.set_windows_sandbox_enabled(/*value*/ true);
                                self.config
                                    .set_windows_elevated_sandbox_enabled(/*value*/ false);
                            }
                            self.chat_widget.set_windows_sandbox_mode(
                                self.config.permissions.windows_sandbox_mode,
                            );
                            let windows_sandbox_level =
                                WindowsSandboxLevel::from_config(&self.config);
                            if let Some((sample_paths, extra_count, failed_scan)) =
                                self.chat_widget.world_writable_warning_details()
                            {
                                self.app_event_tx.send(AppEvent::CodexOp(
                                    AppCommand::override_turn_context(
                                        /*cwd*/ None,
                                        /*approval_policy*/ None,
                                        /*approvals_reviewer*/ None,
                                        /*permission_profile*/ None,
                                        /*active_permission_profile*/ None,
                                        #[cfg(target_os = "windows")]
                                        Some(windows_sandbox_level),
                                        /*model*/ None,
                                        /*effort*/ None,
                                        /*summary*/ None,
                                        /*service_tier*/ None,
                                        /*collaboration_mode*/ None,
                                        /*personality*/ None,
                                    ),
                                ));
                                self.app_event_tx.send(
                                    AppEvent::OpenWorldWritableWarningConfirmation {
                                        preset: Some(preset.clone()),
                                        profile_selection: profile_selection.clone(),
                                        sample_paths,
                                        extra_count,
                                        failed_scan,
                                    },
                                );
                            } else if let Some(selection) = profile_selection {
                                self.app_event_tx.send(AppEvent::CodexOp(
                                    AppCommand::override_turn_context(
                                        /*cwd*/ None,
                                        /*approval_policy*/ None,
                                        /*approvals_reviewer*/ None,
                                        /*permission_profile*/ None,
                                        /*active_permission_profile*/ None,
                                        #[cfg(target_os = "windows")]
                                        Some(windows_sandbox_level),
                                        /*model*/ None,
                                        /*effort*/ None,
                                        /*summary*/ None,
                                        /*service_tier*/ None,
                                        /*collaboration_mode*/ None,
                                        /*personality*/ None,
                                    ),
                                ));
                                if self
                                    .apply_permission_profile_selection(app_server, selection)
                                    .await
                                {
                                    self.chat_widget.submit_initial_user_message_if_pending();
                                }
                                self.chat_widget.add_plain_history_lines(vec![
                                    Line::from(vec!["• ".dim(), "Sandbox ready".into()]),
                                    Line::from(vec![
                                        "  ".into(),
                                        "Codewith can now safely edit files and execute commands in your computer"
                                            .dark_gray(),
                                    ]),
                                ]);
                            } else {
                                self.app_event_tx.send(AppEvent::CodexOp(
                                    AppCommand::override_turn_context(
                                        /*cwd*/ None,
                                        Some(AskForApproval::from(preset.approval)),
                                        Some(self.config.approvals_reviewer),
                                        Some(preset.permission_profile.clone()),
                                        Some(preset.active_permission_profile.clone()),
                                        #[cfg(target_os = "windows")]
                                        Some(windows_sandbox_level),
                                        /*model*/ None,
                                        /*effort*/ None,
                                        /*summary*/ None,
                                        /*service_tier*/ None,
                                        /*collaboration_mode*/ None,
                                        /*personality*/ None,
                                    ),
                                ));
                                let selection = PermissionProfileSelection {
                                    profile_id: preset.active_permission_profile.id.clone(),
                                    approval_policy: Some(AskForApproval::from(preset.approval)),
                                    approvals_reviewer: Some(self.config.approvals_reviewer),
                                    display_label: preset.label.to_string(),
                                };
                                if !self
                                    .apply_permission_profile_selection(app_server, selection)
                                    .await
                                {
                                    return Ok(AppRunControl::Continue);
                                }
                                self.chat_widget.add_plain_history_lines(vec![
                                    Line::from(vec!["• ".dim(), "Sandbox ready".into()]),
                                    Line::from(vec![
                                        "  ".into(),
                                        "Codewith can now safely edit files and execute commands in your computer"
                                            .dark_gray(),
                                    ]),
                                ]);
                            }
                        }
                        Err(err) => {
                            tracing::error!(
                                error = %err,
                                "failed to enable Windows sandbox feature"
                            );
                            self.chat_widget.add_error_message(format!(
                                "Failed to enable the Windows sandbox feature: {err}"
                            ));
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (preset, mode, profile_selection);
                }
            }
            AppEvent::PersistModelSelection { model, effort } => {
                let profile = self.active_config_profile().map(str::to_owned);
                match crate::config_update::write_config_batch(
                    app_server.request_handle(),
                    crate::config_update::build_model_selection_edits(
                        profile.as_deref(),
                        model.as_str(),
                        effort.as_ref(),
                    ),
                )
                .await
                {
                    Ok(_) => {
                        self.config.model = Some(model.clone());
                        self.config.model_reasoning_effort = effort.clone();
                        self.refresh_status_line();
                        let effort_label = effort
                            .as_ref()
                            .map(std::string::ToString::to_string)
                            .unwrap_or_else(|| "default".to_string());
                        tracing::info!("Selected model: {model}, Selected effort: {effort_label}");
                        let mut message = format!("Model changed to {model}");
                        if let Some(label) = Self::reasoning_label_for(&model, effort.as_ref()) {
                            message.push(' ');
                            message.push_str(&label);
                        }
                        if let Some(profile) = &profile {
                            message.push_str(" for ");
                            message.push_str(profile);
                            message.push_str(" profile");
                        }
                        self.chat_widget.add_info_message(message, /*hint*/ None);
                    }
                    Err(err) => {
                        let error = format_config_error(&err);
                        tracing::error!(
                            error = %error,
                            "failed to persist model selection"
                        );
                        if let Some(profile) = &profile {
                            self.chat_widget.add_error_message(format!(
                                "Failed to save model for profile `{profile}`: {error}"
                            ));
                        } else {
                            self.chat_widget.add_error_message(format!(
                                "Failed to save default model: {error}"
                            ));
                        }
                    }
                }
            }
            AppEvent::PluginUninstallLoaded {
                cwd,
                plugin_id: _plugin_id,
                plugin_display_name,
                result,
            } => {
                let uninstall_succeeded = result.is_ok();
                if uninstall_succeeded {
                    self.refresh_plugin_mentions_after_config_write();
                }
                self.chat_widget.on_plugin_uninstall_loaded(
                    cwd.clone(),
                    plugin_display_name,
                    result,
                );
                if uninstall_succeeded
                    && self.chat_widget.config_ref().cwd.as_path() == cwd.as_path()
                {
                    self.fetch_plugins_list(app_server, cwd);
                }
            }
            AppEvent::RefreshPluginMentions => {
                self.refresh_plugin_mentions(app_server);
            }
            AppEvent::PluginMentionsLoaded { mut plugins } => {
                if !self.config.features.enabled(Feature::Plugins) {
                    plugins = None;
                }
                self.chat_widget.on_plugin_mentions_loaded(plugins);
            }
            AppEvent::PersistPersonalitySelection { personality } => {
                match crate::config_update::write_config_batch(
                    app_server.request_handle(),
                    vec![crate::config_update::replace_config_value(
                        "personality",
                        serde_json::json!(personality.to_string()),
                    )],
                )
                .await
                {
                    Ok(_) => {
                        let label = Self::personality_label(personality);
                        let message = format!("Personality set to {label}");
                        self.chat_widget.add_info_message(message, /*hint*/ None);
                    }
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "failed to persist personality selection"
                        );
                        self.chat_widget.add_error_message(format!(
                            "Failed to save default personality: {err}"
                        ));
                    }
                }
            }
            AppEvent::PersistServiceTierSelection { service_tier } => {
                self.refresh_status_line();
                self.config.service_tier = service_tier.clone();
                self.sync_active_thread_service_tier_to_cached_session()
                    .await;
                let profile = self.active_config_profile().map(str::to_owned);
                let edits = crate::config_update::build_service_tier_selection_edits(
                    profile.as_deref(),
                    service_tier.as_deref(),
                );
                match crate::config_update::write_config_batch(app_server.request_handle(), edits)
                    .await
                {
                    Ok(_) => {
                        let mut message = if let Some(service_tier) = service_tier {
                            format!("Service tier set to {service_tier}")
                        } else {
                            "Service tier cleared".to_string()
                        };
                        if let Some(profile) = &profile {
                            message.push_str(" for ");
                            message.push_str(profile);
                            message.push_str(" profile");
                        }
                        self.chat_widget.add_info_message(message, /*hint*/ None);
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "failed to persist service tier selection");
                        if let Some(profile) = &profile {
                            self.chat_widget.add_error_message(format!(
                                "Failed to save service tier for profile `{profile}`: {err}"
                            ));
                        } else {
                            self.chat_widget.add_error_message(format!(
                                "Failed to save default service tier: {err}"
                            ));
                        }
                    }
                }
            }
            AppEvent::PersistRealtimeAudioDeviceSelection { kind, name } => {
                let builder = match kind {
                    RealtimeAudioDeviceKind::Microphone => {
                        ConfigEditsBuilder::for_config(&self.config)
                            .set_realtime_microphone(name.as_deref())
                    }
                    RealtimeAudioDeviceKind::Speaker => {
                        ConfigEditsBuilder::for_config(&self.config)
                            .set_realtime_speaker(name.as_deref())
                    }
                };

                match builder.apply().await {
                    Ok(()) => {
                        match kind {
                            RealtimeAudioDeviceKind::Microphone => {
                                self.config.realtime_audio.microphone = name.clone();
                            }
                            RealtimeAudioDeviceKind::Speaker => {
                                self.config.realtime_audio.speaker = name.clone();
                            }
                        }
                        self.chat_widget
                            .set_realtime_audio_device(kind, name.clone());

                        if self.chat_widget.realtime_conversation_is_live() {
                            self.chat_widget.open_realtime_audio_restart_prompt(kind);
                        } else {
                            let selection = name.unwrap_or_else(|| "System default".to_string());
                            self.chat_widget.add_info_message(
                                format!("Realtime {} set to {selection}", kind.noun()),
                                /*hint*/ None,
                            );
                        }
                    }
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "failed to persist realtime audio selection"
                        );
                        self.chat_widget.add_error_message(format!(
                            "Failed to save realtime {}: {err}",
                            kind.noun()
                        ));
                    }
                }
            }
            AppEvent::RestartRealtimeAudioDevice { kind } => {
                self.chat_widget.restart_realtime_audio_device(kind);
            }
            AppEvent::SelectPermissionProfile(selection) => {
                if self
                    .apply_permission_profile_selection(app_server, selection)
                    .await
                {
                    self.chat_widget.submit_initial_user_message_if_pending();
                }
            }
            AppEvent::UpdateFeatureFlags { updates } => {
                self.update_feature_flags(app_server, updates).await;
            }
            AppEvent::UpdateMemorySettings {
                use_memories,
                generate_memories,
            } => {
                self.update_memory_settings_with_app_server(
                    app_server,
                    use_memories,
                    generate_memories,
                )
                .await;
            }
            AppEvent::ResetMemories => {
                self.reset_memories_with_app_server(app_server).await;
            }
            AppEvent::SkipNextWorldWritableScan => {
                self.windows_sandbox.skip_world_writable_scan_once = true;
            }
            AppEvent::UpdateFullAccessWarningAcknowledged(ack) => {
                self.chat_widget.set_full_access_warning_acknowledged(ack);
            }
            AppEvent::UpdateWorldWritableWarningAcknowledged(ack) => {
                self.chat_widget
                    .set_world_writable_warning_acknowledged(ack);
            }
            AppEvent::UpdateRateLimitSwitchPromptHidden(hidden) => {
                self.chat_widget.set_rate_limit_switch_prompt_hidden(hidden);
            }
            AppEvent::UpdatePlanModeReasoningEffort(effort) => {
                self.config.plan_mode_reasoning_effort = effort.clone();
                self.chat_widget.set_plan_mode_reasoning_effort(effort);
                self.sync_active_thread_plan_mode_reasoning_setting(app_server)
                    .await;
            }
            AppEvent::PersistFullAccessWarningAcknowledged => {
                if let Err(err) = ConfigEditsBuilder::for_config(&self.config)
                    .set_hide_full_access_warning(/*acknowledged*/ true)
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist full access warning acknowledgement"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save full access confirmation preference: {err}"
                    ));
                }
            }
            AppEvent::PersistWorldWritableWarningAcknowledged => {
                if let Err(err) = ConfigEditsBuilder::for_config(&self.config)
                    .set_hide_world_writable_warning(/*acknowledged*/ true)
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist world-writable warning acknowledgement"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save Agent mode warning preference: {err}"
                    ));
                }
            }
            AppEvent::PersistRateLimitSwitchPromptHidden => {
                if let Err(err) = ConfigEditsBuilder::for_config(&self.config)
                    .set_hide_rate_limit_model_nudge(/*acknowledged*/ true)
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist rate limit switch prompt preference"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save rate limit reminder preference: {err}"
                    ));
                }
            }
            AppEvent::PersistPlanModeReasoningEffort(effort) => {
                let key_path = "plan_mode_reasoning_effort";
                let edit = if let Some(effort) = effort {
                    crate::config_update::replace_config_value(
                        key_path,
                        serde_json::json!(effort.to_string()),
                    )
                } else {
                    crate::config_update::clear_config_value(key_path)
                };
                if let Err(err) = crate::config_update::write_config_batch(
                    app_server.request_handle(),
                    vec![edit],
                )
                .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist plan mode reasoning effort"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save Plan mode reasoning effort: {err}"
                    ));
                }
            }
            AppEvent::PersistModelMigrationPromptAcknowledged {
                from_model,
                to_model,
            } => {
                if let Err(err) = ConfigEditsBuilder::for_config(&self.config)
                    .record_model_migration_seen(from_model.as_str(), to_model.as_str())
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist model migration prompt acknowledgement"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save model migration prompt preference: {err}"
                    ));
                }
            }
            AppEvent::OpenApprovalsPopup => {
                self.chat_widget.open_approvals_popup();
            }
            AppEvent::OpenAgentPicker => {
                self.open_agent_picker(app_server).await;
            }
            AppEvent::OpenAgentRenamePrompt {
                thread_id,
                current_name,
                label,
            } => {
                self.chat_widget
                    .show_agent_rename_prompt(thread_id, current_name, label);
            }
            AppEvent::SelectAgentThread(thread_id) => {
                self.select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await?;
            }
            AppEvent::StartSide {
                parent_thread_id,
                user_message,
            } => {
                return self
                    .handle_start_side(tui, app_server, parent_thread_id, user_message)
                    .await;
            }
            AppEvent::OpenSkillsList => {
                self.chat_widget.open_skills_list();
            }
            AppEvent::OpenManageSkillsPopup => {
                self.chat_widget.open_manage_skills_popup();
            }
            AppEvent::SetSkillEnabled { path, enabled } => {
                match crate::config_update::write_skill_enabled(
                    app_server.request_handle(),
                    path.clone(),
                    enabled,
                )
                .await
                {
                    Ok(()) => {
                        self.chat_widget.update_skill_enabled(path, enabled);
                    }
                    Err(err) => {
                        let path_display = path.display();
                        self.chat_widget.add_error_message(format!(
                            "Failed to update skill config for {path_display}: {err}"
                        ));
                    }
                }
            }
            AppEvent::SetAppEnabled { id, enabled } => {
                let edits = if enabled {
                    vec![
                        crate::config_update::clear_config_value(
                            crate::config_update::app_scoped_key_path(&id, "enabled"),
                        ),
                        crate::config_update::clear_config_value(
                            crate::config_update::app_scoped_key_path(&id, "disabled_reason"),
                        ),
                    ]
                } else {
                    vec![
                        crate::config_update::replace_config_value(
                            crate::config_update::app_scoped_key_path(&id, "enabled"),
                            serde_json::json!(false),
                        ),
                        crate::config_update::replace_config_value(
                            crate::config_update::app_scoped_key_path(&id, "disabled_reason"),
                            serde_json::json!("user"),
                        ),
                    ]
                };
                match crate::config_update::write_config_batch(app_server.request_handle(), edits)
                    .await
                {
                    Ok(_) => {
                        self.chat_widget.update_connector_enabled(&id, enabled);
                    }
                    Err(err) => {
                        self.chat_widget.add_error_message(format!(
                            "Failed to update app config for {id}: {err}"
                        ));
                    }
                }
            }
            AppEvent::SetHookEnabled { key, enabled } => {
                self.set_hook_enabled(app_server, key, enabled);
            }
            AppEvent::TrustHook { key, current_hash } => {
                self.trust_hook(app_server, key, current_hash);
            }
            AppEvent::TrustHooks { updates } => {
                self.trust_hooks(app_server, updates);
            }
            AppEvent::HookEnabledSet {
                key,
                enabled,
                result,
            } => {
                let queued_enabled = self
                    .pending_hook_enabled_writes
                    .get_mut(&key)
                    .and_then(Option::take);
                let should_apply_result = if let Some(queued_enabled) = queued_enabled
                    && (result.is_err() || queued_enabled != enabled)
                {
                    self.spawn_hook_enabled_write(app_server, key.clone(), queued_enabled);
                    false
                } else {
                    true
                };
                if should_apply_result {
                    self.pending_hook_enabled_writes.remove(&key);
                    if let Err(err) = result {
                        self.chat_widget.add_error_message(err);
                    }
                }
            }
            AppEvent::HookTrusted { result } => {
                if let Err(err) = result {
                    self.chat_widget.add_error_message(err);
                }
            }
            AppEvent::OpenPermissionsPopup => {
                self.chat_widget.open_permissions_popup();
            }
            AppEvent::DispatchSlashCommand(command) => {
                self.chat_widget.run_slash_command(command);
            }
            AppEvent::ShowStatusReport => {
                self.chat_widget.show_status_report();
            }
            AppEvent::OpenReviewBranchPicker(cwd) => {
                self.chat_widget.show_review_branch_picker(&cwd).await;
            }
            AppEvent::OpenReviewCommitPicker(cwd) => {
                self.chat_widget.show_review_commit_picker(&cwd).await;
            }
            AppEvent::OpenReviewCustomPrompt => {
                self.chat_widget.show_review_custom_prompt();
            }
            AppEvent::SubmitUserMessageWithMode {
                text,
                collaboration_mode,
            } => {
                self.chat_widget
                    .submit_user_message_with_mode(text, collaboration_mode);
            }
            AppEvent::ManageSkillsClosed => {
                self.chat_widget.handle_manage_skills_closed();
            }
            AppEvent::FullScreenApprovalRequest(request) => match request {
                ApprovalRequest::ApplyPatch { cwd, changes, .. } => {
                    let _ = tui.enter_alt_screen();
                    let diff_summary = DiffSummary::new(changes, cwd);
                    self.overlay = Some(Overlay::new_static_with_renderables(
                        vec![diff_summary.into()],
                        "P A T C H".to_string(),
                        self.keymap.pager.clone(),
                    ));
                }
                ApprovalRequest::Exec { command, .. } => {
                    let _ = tui.enter_alt_screen();
                    let full_cmd = strip_bash_lc_and_escape(&command);
                    let full_cmd_lines = highlight_bash_to_lines(&full_cmd);
                    self.overlay = Some(Overlay::new_static_with_lines(
                        full_cmd_lines,
                        "E X E C".to_string(),
                        self.keymap.pager.clone(),
                    ));
                }
                ApprovalRequest::Permissions {
                    environment_id,
                    permissions,
                    reason,
                    ..
                } => {
                    let _ = tui.enter_alt_screen();
                    let mut lines = Vec::new();
                    if let Some(environment_id) = environment_id {
                        lines.push(Line::from(vec![
                            "Environment: ".into(),
                            environment_id.bold(),
                        ]));
                        lines.push(Line::from(""));
                    }
                    if let Some(reason) = reason {
                        lines.push(Line::from(vec!["Reason: ".into(), reason.italic()]));
                        lines.push(Line::from(""));
                    }
                    if let Some(rule_line) =
                        crate::bottom_pane::format_requested_permissions_rule(&permissions)
                    {
                        lines.push(Line::from(vec![
                            "Permission rule: ".into(),
                            rule_line.fg(accent_color()),
                        ]));
                    }
                    self.overlay = Some(Overlay::new_static_with_renderables(
                        vec![Box::new(Paragraph::new(lines).wrap(Wrap { trim: false }))],
                        "P E R M I S S I O N S".to_string(),
                        self.keymap.pager.clone(),
                    ));
                }
                ApprovalRequest::McpElicitation {
                    server_name,
                    message,
                    ..
                } => {
                    let _ = tui.enter_alt_screen();
                    let paragraph = Paragraph::new(vec![
                        Line::from(vec!["Server: ".into(), server_name.bold()]),
                        Line::from(""),
                        Line::from(message),
                    ])
                    .wrap(Wrap { trim: false });
                    self.overlay = Some(Overlay::new_static_with_renderables(
                        vec![Box::new(paragraph)],
                        "E L I C I T A T I O N".to_string(),
                        self.keymap.pager.clone(),
                    ));
                }
            },
            #[cfg(not(target_os = "linux"))]
            AppEvent::UpdateRecordingMeter { id, text } => {
                // Update in place to preserve the element id for subsequent frames.
                let updated = self.chat_widget.update_recording_meter_in_place(&id, &text);
                if updated
                    || self
                        .chat_widget
                        .stop_realtime_conversation_for_deleted_meter(&id)
                {
                    tui.frame_requester().schedule_frame();
                }
            }
            AppEvent::StatusLineSetup {
                items,
                use_theme_colors,
            } => {
                let ids = items.iter().map(ToString::to_string).collect::<Vec<_>>();
                let items_edit = crate::legacy_core::config::edit::status_line_items_edit(&ids);
                let colors_edit =
                    crate::legacy_core::config::edit::status_line_use_colors_edit(use_theme_colors);
                let apply_result = ConfigEditsBuilder::for_config(&self.config)
                    .with_edits([items_edit, colors_edit])
                    .apply()
                    .await;
                match apply_result {
                    Ok(()) => {
                        self.config.tui_status_line = Some(ids.clone());
                        self.config.tui_status_line_use_colors = use_theme_colors;
                        self.chat_widget.setup_status_line(items, use_theme_colors);
                    }
                    Err(err) => {
                        let error = format_config_error(&err);
                        tracing::error!(error = %error, "failed to persist status line settings; keeping previous selection");
                        self.chat_widget.add_error_message(format!(
                            "Failed to save status line settings: {error}"
                        ));
                    }
                }
            }
            AppEvent::StatusLineBranchUpdated { cwd, branch } => {
                self.chat_widget.set_status_line_branch(cwd, branch);
                self.refresh_status_line();
            }
            AppEvent::StatusLineGitSummaryUpdated { cwd, summary } => {
                self.chat_widget.set_status_line_git_summary(cwd, summary);
                self.refresh_status_line();
            }
            AppEvent::StatusLineSetupCancelled => {
                self.chat_widget.cancel_status_line_setup();
            }
            AppEvent::TerminalTitleSetup { items } => {
                let ids = items.iter().map(ToString::to_string).collect::<Vec<_>>();
                let edit = crate::legacy_core::config::edit::terminal_title_items_edit(&ids);
                let apply_result = ConfigEditsBuilder::for_config(&self.config)
                    .with_edits([edit])
                    .apply()
                    .await;
                match apply_result {
                    Ok(()) => {
                        self.config.tui_terminal_title = Some(ids.clone());
                        self.chat_widget.setup_terminal_title(items);
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "failed to persist terminal title items; keeping previous selection");
                        self.chat_widget.revert_terminal_title_setup_preview();
                        self.chat_widget.add_error_message(format!(
                            "Failed to save terminal title items: {err}"
                        ));
                    }
                }
            }
            AppEvent::TerminalTitleSetupPreview { items } => {
                self.chat_widget.preview_terminal_title(items);
            }
            AppEvent::TerminalTitleSetupCancelled => {
                self.chat_widget.cancel_terminal_title_setup();
            }
            AppEvent::SyntaxThemeSelected { name } => {
                let edit = crate::legacy_core::config::edit::syntax_theme_edit(&name);
                let apply_result = ConfigEditsBuilder::for_config(&self.config)
                    .with_edits([edit])
                    .apply()
                    .await;
                match apply_result {
                    Ok(()) => {
                        // Ensure the selected theme is active in the current
                        // session.  The preview callback covers arrow-key
                        // navigation, but if the user presses Enter without
                        // navigating, the runtime theme must still be applied.
                        if let Some(theme) = crate::render::highlight::resolve_theme_by_name(
                            &name,
                            Some(&self.config.codex_home),
                        ) {
                            crate::render::highlight::set_syntax_theme(theme);
                        }
                        self.sync_tui_theme_selection(name);
                        self.refresh_status_line();
                    }
                    Err(err) => {
                        self.restore_runtime_theme_from_config();
                        self.refresh_status_line();
                        tracing::error!(error = %err, "failed to persist theme selection");
                        self.chat_widget
                            .add_error_message(format!("Failed to save theme: {err}"));
                    }
                }
            }
            AppEvent::SyntaxThemePreviewed => {
                self.refresh_status_line();
            }
            AppEvent::OpenKeymapActionMenu { context, action } => {
                self.chat_widget
                    .open_keymap_action_menu(context, action, &self.keymap);
            }
            AppEvent::OpenKeymapReplaceBindingMenu { context, action } => {
                self.chat_widget
                    .open_keymap_replace_binding_menu(context, action, &self.keymap);
            }
            AppEvent::OpenKeymapCapture {
                context,
                action,
                intent,
            } => {
                self.chat_widget
                    .open_keymap_capture(context, action, intent, &self.keymap);
            }
            AppEvent::OpenKeymapDebug => {
                self.chat_widget.open_keymap_debug(&self.keymap);
            }
            AppEvent::KeymapCaptured {
                context,
                action,
                key,
                intent,
            } => {
                self.apply_keymap_capture(context, action, key, intent)
                    .await;
            }
            AppEvent::KeymapCleared { context, action } => {
                self.apply_keymap_clear(context, action).await;
            }
        }
        Ok(AppRunControl::Continue)
    }

    pub(super) async fn submit_auth_profile_switch(
        &mut self,
        profile: Option<String>,
        reason: &crate::app_event::AuthProfileSwitchReason,
        resume_queued_input: bool,
    ) {
        let op = match profile.as_deref() {
            Some(profile_name) => {
                match codex_login::load_auth_profile_metadata(&self.config.codex_home, profile_name)
                {
                    Ok(metadata) => match metadata.last_permissions {
                        Some(settings) => match self
                            .rebuild_config_for_auth_profile_permission_settings(&settings)
                            .await
                        {
                            Ok(config) => AppCommand::OverrideTurnContext {
                                cwd: None,
                                approval_policy: Some(AskForApproval::from(
                                    config.permissions.approval_policy.value(),
                                )),
                                approvals_reviewer: Some(config.approvals_reviewer),
                                permission_profile: Some(
                                    config.permissions.permission_profile().clone(),
                                ),
                                active_permission_profile: config
                                    .permissions
                                    .active_permission_profile(),
                                auth_profile: Some(profile.clone()),
                                windows_sandbox_level: None,
                                model_provider: None,
                                model: None,
                                effort: None,
                                summary: None,
                                service_tier: None,
                                collaboration_mode: None,
                                personality: None,
                            },
                            Err(err) => {
                                tracing::warn!(
                                    profile = profile_name,
                                    error = %err,
                                    "failed to apply saved auth profile permissions during profile switch"
                                );
                                self.chat_widget.add_error_message(format!(
                                "Saved permissions for auth profile `{profile_name}` could not be applied: {err}"
                            ));
                                AppCommand::override_turn_context_auth_profile(profile.clone())
                            }
                        },
                        None => AppCommand::override_turn_context_auth_profile(profile.clone()),
                    },
                    Err(err) => {
                        tracing::warn!(
                            profile = profile_name,
                            error = %err,
                            "failed to load auth profile metadata during profile switch"
                        );
                        AppCommand::override_turn_context_auth_profile(profile.clone())
                    }
                }
            }
            None => AppCommand::override_turn_context_auth_profile(profile.clone()),
        };
        let submitted = self.chat_widget.submit_op(op);
        if submitted {
            let queued_input_will_resume =
                resume_queued_input && self.chat_widget.has_queued_follow_up_messages();
            let message =
                auth_profile_switch_message(profile.as_deref(), reason, queued_input_will_resume);
            self.chat_widget.add_info_message(message, /*hint*/ None);
            self.refresh_status_line();
            if resume_queued_input {
                self.chat_widget.maybe_send_next_queued_input();
            }
        }
    }

    async fn apply_keymap_capture(
        &mut self,
        context: String,
        action: String,
        key: String,
        intent: crate::app_event::KeymapEditIntent,
    ) {
        let outcome = match crate::keymap_setup::keymap_with_edit(
            &self.config.tui_keymap,
            &self.keymap,
            &context,
            &action,
            &key,
            &intent,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                self.chat_widget.add_error_message(err);
                return;
            }
        };
        let (keymap_config, bindings, message) = match outcome {
            crate::keymap_setup::KeymapEditOutcome::Updated {
                keymap_config,
                bindings,
                message,
            } => (*keymap_config, bindings, message),
            crate::keymap_setup::KeymapEditOutcome::Unchanged { message } => {
                self.chat_widget.add_info_message(message, /*hint*/ None);
                return;
            }
        };

        let runtime_keymap = match RuntimeKeymap::from_config(&keymap_config) {
            Ok(runtime_keymap) => runtime_keymap,
            Err(err) => {
                let params = crate::keymap_setup::build_keymap_conflict_params(
                    context, action, key, intent, err,
                );
                self.chat_widget.show_selection_view(params);
                return;
            }
        };

        let edit =
            crate::legacy_core::config::edit::keymap_bindings_edit(&context, &action, &bindings);
        match ConfigEditsBuilder::for_config(&self.config)
            .with_edits([edit])
            .apply()
            .await
        {
            Ok(()) => {
                self.config.tui_keymap = keymap_config.clone();
                self.keymap = runtime_keymap.clone();
                self.chat_widget
                    .apply_keymap_update(keymap_config, &runtime_keymap);
                self.chat_widget
                    .return_to_keymap_picker(&context, &action, &runtime_keymap);
                self.chat_widget.add_info_message(message, /*hint*/ None);
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to persist keymap binding");
                self.chat_widget
                    .add_error_message(format!("Failed to save shortcut: {err}"));
            }
        }
    }

    fn refresh_plugin_mentions_after_config_write(&mut self) {
        self.chat_widget.refresh_plugin_mentions();
        self.chat_widget.submit_op(AppCommand::reload_user_config());
    }

    async fn apply_keymap_clear(&mut self, context: String, action: String) {
        let keymap_config = match crate::keymap_setup::keymap_without_custom_binding(
            &self.config.tui_keymap,
            &context,
            &action,
        ) {
            Ok(keymap_config) => keymap_config,
            Err(err) => {
                self.chat_widget.add_error_message(err);
                return;
            }
        };

        let runtime_keymap = match RuntimeKeymap::from_config(&keymap_config) {
            Ok(runtime_keymap) => runtime_keymap,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to refresh shortcuts: {err}"));
                return;
            }
        };

        let edit = crate::legacy_core::config::edit::keymap_binding_clear_edit(&context, &action);
        match ConfigEditsBuilder::for_config(&self.config)
            .with_edits([edit])
            .apply()
            .await
        {
            Ok(()) => {
                self.config.tui_keymap = keymap_config.clone();
                self.keymap = runtime_keymap.clone();
                self.chat_widget
                    .apply_keymap_update(keymap_config, &runtime_keymap);
                self.chat_widget
                    .return_to_keymap_picker(&context, &action, &runtime_keymap);
                self.chat_widget.add_info_message(
                    format!("Removed custom shortcut for `{context}.{action}`."),
                    /*hint*/ None,
                );
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to clear keymap binding");
                self.chat_widget
                    .add_error_message(format!("Failed to remove shortcut: {err}"));
            }
        }
    }

    pub(super) async fn handle_exit_mode(
        &mut self,
        app_server: &mut AppServerSession,
        mode: ExitMode,
    ) -> AppRunControl {
        match mode {
            ExitMode::ShutdownFirst => {
                // Mark the thread we are explicitly shutting down for exit so
                // its shutdown completion does not trigger agent failover.
                self.pending_shutdown_exit_thread_id =
                    self.active_thread_id.or(self.chat_widget.thread_id());
                if self.pending_shutdown_exit_thread_id.is_some() {
                    // This is a UI escape-hatch budget, not a protocol
                    // deadline. A healthy local thread/unsubscribe round trip
                    // should finish comfortably inside two seconds, while a
                    // longer wait makes Ctrl+C feel broken when the app-server
                    // is already wedged.
                    if tokio::time::timeout(
                        SHUTDOWN_FIRST_EXIT_TIMEOUT,
                        self.shutdown_current_thread(app_server),
                    )
                    .await
                    .is_err()
                    {
                        tracing::warn!("timed out waiting for app-server thread shutdown");
                    }
                }
                self.pending_shutdown_exit_thread_id = None;
                AppRunControl::Exit(ExitReason::UserRequested)
            }
            ExitMode::Immediate => {
                self.pending_shutdown_exit_thread_id = None;
                AppRunControl::Exit(ExitReason::UserRequested)
            }
        }
    }

    pub(super) async fn archive_current_thread(
        &mut self,
        app_server: &mut AppServerSession,
    ) -> AppRunControl {
        let Some(thread_id) = self.active_thread_id.or(self.chat_widget.thread_id()) else {
            self.chat_widget
                .add_error_message("A thread must start before it can be archived.".to_string());
            return AppRunControl::Continue;
        };
        if self.side_threads.contains_key(&thread_id) {
            self.chat_widget.add_error_message(
                "'/archive' is unavailable in side conversations. Press Ctrl+C to return to the main thread first."
                    .to_string(),
            );
            return AppRunControl::Continue;
        }

        match app_server.thread_archive(thread_id).await {
            Ok(()) => AppRunControl::Exit(ExitReason::UserRequested),
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to archive current thread: {err}"));
                AppRunControl::Continue
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RateLimitRefreshCompletion {
    None,
    ScheduleFrame,
}

impl App {
    pub(super) fn apply_rate_limits_loaded(
        &mut self,
        origin: RateLimitRefreshOrigin,
        target: RateLimitRefreshTarget,
        auth_profile: Option<String>,
        result: Result<Vec<RateLimitSnapshot>, String>,
    ) -> RateLimitRefreshCompletion {
        let is_current_profile = auth_profile == self.config.selected_auth_profile;
        let heartbeat_profile = matches!(origin, RateLimitRefreshOrigin::Heartbeat)
            .then(|| target.auth_profile_key(self.config.selected_auth_profile.as_deref()))
            .flatten();
        if target.targets_selected_profile() && !is_current_profile {
            tracing::debug!(
                request_auth_profile = ?auth_profile,
                current_auth_profile = ?self.config.selected_auth_profile,
                "discarding stale account/rateLimits/read result after auth profile change"
            );
            if let RateLimitRefreshOrigin::StatusCommand { request_id } = origin {
                self.chat_widget
                    .finish_status_rate_limit_refresh(request_id);
            }
            return RateLimitRefreshCompletion::None;
        }

        match result {
            Ok(snapshots) => {
                if matches!(origin, RateLimitRefreshOrigin::Heartbeat) {
                    self.chat_widget
                        .record_auth_profile_usage_heartbeat_success(heartbeat_profile);
                }
                if is_current_profile {
                    for snapshot in snapshots {
                        self.chat_widget.on_rate_limit_snapshot(Some(snapshot));
                    }
                } else {
                    self.chat_widget
                        .on_auth_profile_rate_limit_snapshots(auth_profile, snapshots);
                }
                match origin {
                    RateLimitRefreshOrigin::StartupPrefetch | RateLimitRefreshOrigin::Heartbeat => {
                        self.chat_widget.refresh_profile_popup_if_active();
                        RateLimitRefreshCompletion::ScheduleFrame
                    }
                    RateLimitRefreshOrigin::StatusCommand { request_id } => {
                        self.chat_widget
                            .finish_status_rate_limit_refresh(request_id);
                        RateLimitRefreshCompletion::None
                    }
                }
            }
            Err(err) => {
                if matches!(origin, RateLimitRefreshOrigin::Heartbeat) {
                    self.chat_widget
                        .record_auth_profile_usage_heartbeat_failure(heartbeat_profile);
                }
                if matches!(origin, RateLimitRefreshOrigin::StatusCommand { .. })
                    || target.targets_selected_profile()
                    || is_current_profile
                {
                    tracing::warn!("account/rateLimits/read failed during TUI refresh: {err}");
                } else {
                    tracing::debug!("account/rateLimits/read heartbeat failed: {err}");
                }
                if let RateLimitRefreshOrigin::StatusCommand { request_id } = origin {
                    self.chat_widget
                        .finish_status_rate_limit_refresh(request_id);
                }
                RateLimitRefreshCompletion::None
            }
        }
    }

    pub(super) fn apply_minimax_usage_loaded(
        &mut self,
        origin: MiniMaxUsageRefreshOrigin,
        auth_profile: Option<String>,
        result: Result<crate::minimax_usage::MiniMaxUsageSnapshot, String>,
    ) {
        let MiniMaxUsageRefreshOrigin::StatusCommand { request_id } = origin;
        if auth_profile != self.config.selected_auth_profile {
            tracing::debug!(
                request_auth_profile = ?auth_profile,
                current_auth_profile = ?self.config.selected_auth_profile,
                "discarding stale MiniMax usage result after auth profile change"
            );
            self.chat_widget.finish_status_minimax_usage_refresh(
                request_id,
                Err("MiniMax usage refresh was superseded by an auth profile change".to_string()),
            );
            return;
        }

        if let Err(err) = result.as_ref() {
            tracing::warn!("MiniMax usage refresh failed during TUI refresh: {err}");
        }
        self.chat_widget
            .finish_status_minimax_usage_refresh(request_id, result);
    }
}

pub(super) fn auth_profile_switch_message(
    profile: Option<&str>,
    reason: &crate::app_event::AuthProfileSwitchReason,
    queued_input_will_resume: bool,
) -> String {
    let label = profile
        .map(str::to_string)
        .unwrap_or_else(|| "default".to_string());
    match reason {
        crate::app_event::AuthProfileSwitchReason::Manual => {
            format!("Profile switch requested for {label}")
        }
        crate::app_event::AuthProfileSwitchReason::AutoRateLimit { window } => {
            let mut message = format!(
                "Auto-switching auth profile to {label} because the {window} limit is exhausted."
            );
            if queued_input_will_resume {
                message.push_str(" Your prompt will continue with that account.");
            }
            message
        }
    }
}
