use super::thread_monitor_api::*;
use super::*;

#[derive(Clone)]
pub(crate) struct ThreadMonitorRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    state_db: Option<StateDbHandle>,
    monitor_runtime: ThreadMonitorRuntime,
}

impl ThreadMonitorRequestProcessor {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        state_db: Option<StateDbHandle>,
        monitor_runtime: ThreadMonitorRuntime,
    ) -> Self {
        Self {
            thread_manager,
            outgoing,
            config,
            state_db,
            monitor_runtime,
        }
    }

    pub(crate) async fn thread_monitor_create(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorCreateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_monitor_create_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_monitor_list(
        &self,
        params: ThreadMonitorListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_monitor_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_monitor_read(
        &self,
        params: ThreadMonitorReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_monitor_read_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_monitor_stop(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorStopParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_monitor_stop_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_monitor_restart(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorRestartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_monitor_restart_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_monitor_delete(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_monitor_delete_inner(request_id, params)
            .await
            .map(|()| None)
    }

    async fn thread_monitor_create_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorCreateParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_monitor_request(params.thread_id.as_str())?;
        let name = validate_monitor_name(params.name.as_str())?;
        let prompt = validate_monitor_prompt(params.prompt.as_str())?;
        let command = validate_monitor_command(params.command.as_str())?;
        let cwd = validate_optional_monitor_path("monitor cwd", params.cwd)?;
        let routing = params
            .routing
            .map(api_thread_monitor_routing_to_state)
            .unwrap_or(codex_state::ThreadMonitorRouting::Stream);
        let output_file = validate_optional_monitor_path("monitor outputFile", params.output_file)?;
        let output_file = validate_monitor_output_file_for_routing(routing, output_file)?;
        let state_db = self.prepare_monitor_mutation(thread_id).await?;
        self.ensure_thread_monitor_capacity(&state_db, thread_id)
            .await?;
        let monitor = state_db
            .thread_monitors()
            .create_thread_monitor(codex_state::ThreadMonitorCreateParams {
                thread_id,
                name,
                prompt,
                command,
                cwd,
                routing,
                output_file,
                status: codex_state::ThreadMonitorStatus::Running,
            })
            .await
            .map_err(|err| internal_error(format!("failed to create thread monitor: {err}")))?;
        let monitor = api_thread_monitor_from_state(monitor);

        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadMonitorCreateResponse {
                    monitor: monitor.clone(),
                },
            )
            .await;
        self.emit_thread_monitor_updated(thread_id, monitor.clone())
            .await;
        self.monitor_runtime.start_monitor_now(monitor).await;
        Ok(())
    }

    async fn thread_monitor_list_inner(
        &self,
        params: ThreadMonitorListParams,
    ) -> Result<ThreadMonitorListResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_monitor_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let limit = normalize_monitor_list_limit(params.limit)?;
        let offset = decode_monitor_cursor(params.cursor.as_deref())?;
        let monitors = state_db
            .thread_monitors()
            .list_thread_monitors(thread_id)
            .await
            .map_err(|err| internal_error(format!("failed to list thread monitors: {err}")))?;
        let next_offset = offset.saturating_add(limit);
        let mut page: Vec<_> = monitors.into_iter().skip(offset).take(limit + 1).collect();
        let has_more = page.len() > limit;
        if has_more {
            page.truncate(limit);
        }
        let data = page
            .into_iter()
            .map(api_thread_monitor_from_state)
            .collect();
        let next_cursor = has_more.then(|| next_offset.to_string());
        Ok(ThreadMonitorListResponse { data, next_cursor })
    }

    async fn thread_monitor_read_inner(
        &self,
        params: ThreadMonitorReadParams,
    ) -> Result<ThreadMonitorReadResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_monitor_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let Some(monitor_id) = self
            .find_monitor_id_for_thread(&state_db, thread_id, params.monitor_id.as_str())
            .await?
        else {
            return Ok(ThreadMonitorReadResponse {
                monitor: None,
                events: Vec::new(),
                next_cursor: None,
            });
        };
        let limit = normalize_monitor_list_limit(params.limit)?;
        let offset = decode_monitor_cursor(params.cursor.as_deref())?;
        let monitor = self
            .load_monitor_for_thread(&state_db, thread_id, monitor_id.as_str())
            .await
            .ok();
        let next_offset = offset.saturating_add(limit);
        let mut events = state_db
            .thread_monitors()
            .list_thread_monitor_events(monitor_id.as_str(), offset, limit + 1)
            .await
            .map_err(|err| {
                internal_error(format!("failed to read thread monitor events: {err}"))
            })?;
        let has_more = events.len() > limit;
        if has_more {
            events.truncate(limit);
        }
        Ok(ThreadMonitorReadResponse {
            monitor: monitor.map(api_thread_monitor_from_state),
            events: events
                .into_iter()
                .map(api_thread_monitor_event_from_state)
                .collect(),
            next_cursor: has_more.then(|| next_offset.to_string()),
        })
    }

    async fn thread_monitor_stop_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorStopParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_monitor_request(params.thread_id.as_str())?;
        let state_db = self.prepare_monitor_mutation(thread_id).await?;
        let monitor_id = self
            .resolve_monitor_id_for_thread(&state_db, thread_id, params.monitor_id.as_str())
            .await?;
        let monitor = state_db
            .thread_monitors()
            .set_thread_monitor_status(
                monitor_id.as_str(),
                codex_state::ThreadMonitorStatus::Stopped,
                None,
            )
            .await
            .map_err(|err| internal_error(format!("failed to stop thread monitor: {err}")))?
            .ok_or_else(|| invalid_request(format!("monitor not found: {monitor_id}")))?;
        let monitor = api_thread_monitor_from_state(monitor);
        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadMonitorStopResponse {
                    monitor: monitor.clone(),
                },
            )
            .await;
        self.monitor_runtime
            .stop_monitor(monitor.monitor_id.as_str())
            .await;
        self.emit_thread_monitor_updated(thread_id, monitor).await;
        Ok(())
    }

    async fn thread_monitor_restart_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorRestartParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_monitor_request(params.thread_id.as_str())?;
        let state_db = self.prepare_monitor_mutation(thread_id).await?;
        let monitor_id = self
            .resolve_monitor_id_for_thread(&state_db, thread_id, params.monitor_id.as_str())
            .await?;
        let monitor = state_db
            .thread_monitors()
            .restart_thread_monitor(monitor_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to restart thread monitor: {err}")))?
            .ok_or_else(|| invalid_request(format!("monitor not found: {monitor_id}")))?;
        let monitor = api_thread_monitor_from_state(monitor);
        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadMonitorRestartResponse {
                    monitor: monitor.clone(),
                },
            )
            .await;
        self.monitor_runtime
            .stop_monitor(monitor.monitor_id.as_str())
            .await;
        self.emit_thread_monitor_updated(thread_id, monitor.clone())
            .await;
        self.monitor_runtime.start_monitor_now(monitor).await;
        Ok(())
    }

    async fn thread_monitor_delete_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadMonitorDeleteParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_monitor_request(params.thread_id.as_str())?;
        let state_db = self.prepare_monitor_mutation(thread_id).await?;
        let Some(monitor_id) = self
            .find_monitor_id_for_thread(&state_db, thread_id, params.monitor_id.as_str())
            .await?
        else {
            self.outgoing
                .send_response(
                    request_id.clone(),
                    ThreadMonitorDeleteResponse {
                        monitor_id: params.monitor_id,
                        deleted: false,
                    },
                )
                .await;
            return Ok(());
        };
        let deleted = state_db
            .thread_monitors()
            .delete_thread_monitor(monitor_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to delete thread monitor: {err}")))?;
        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadMonitorDeleteResponse {
                    monitor_id: monitor_id.clone(),
                    deleted,
                },
            )
            .await;
        if deleted {
            self.monitor_runtime.stop_monitor(monitor_id.as_str()).await;
            self.emit_thread_monitor_deleted(thread_id, monitor_id)
                .await;
        }
        Ok(())
    }

    fn ensure_enabled(&self) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::ScheduledTasks) {
            return Err(invalid_request("scheduled_tasks feature is disabled"));
        }
        Ok(())
    }

    async fn prepare_monitor_mutation(
        &self,
        thread_id: ThreadId,
    ) -> Result<StateDbHandle, JSONRPCErrorError> {
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        self.reconcile_materialized_thread(thread_id, &state_db)
            .await?;
        Ok(state_db)
    }

    async fn reconcile_materialized_thread(
        &self,
        thread_id: ThreadId,
        state_db: &StateDbHandle,
    ) -> Result<(), JSONRPCErrorError> {
        let running_thread = self.thread_manager.get_thread(thread_id).await.ok();
        let rollout_path = match running_thread.as_ref() {
            Some(thread) => thread.rollout_path().ok_or_else(|| {
                invalid_request(format!(
                    "ephemeral thread does not support monitors: {thread_id}"
                ))
            })?,
            None => codex_rollout::find_thread_path_by_id_str(
                &self.config.codex_home,
                &thread_id.to_string(),
                self.state_db.as_deref(),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to locate thread id {thread_id}: {err}"))
            })?
            .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?,
        };
        reconcile_rollout(
            Some(state_db),
            rollout_path.as_path(),
            self.config.model_provider_id.as_str(),
            /*builder*/ None,
            &[],
            /*archived_only*/ None,
            /*new_thread_memory_mode*/ None,
        )
        .await;
        if let Some(thread) = running_thread.as_ref()
            && state_db
                .get_thread(thread_id)
                .await
                .map_err(|err| internal_error(format!("failed to read thread metadata: {err}")))?
                .is_none()
        {
            self.upsert_running_thread_metadata(
                state_db,
                thread_id,
                thread,
                rollout_path.as_path(),
            )
            .await?;
        }
        Ok(())
    }

    async fn upsert_running_thread_metadata(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        thread: &Arc<CodexThread>,
        rollout_path: &Path,
    ) -> Result<(), JSONRPCErrorError> {
        let session_configured = thread.session_configured();
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            rollout_path.to_path_buf(),
            Utc::now(),
            SessionSource::default(),
        );
        builder.thread_source = session_configured.thread_source;
        builder.model_provider = Some(session_configured.model_provider_id);
        builder.cwd = session_configured.cwd.to_path_buf();
        builder.approval_mode = session_configured.approval_policy;
        let metadata = builder.build(self.config.model_provider_id.as_str());
        state_db
            .upsert_thread(&metadata)
            .await
            .map_err(|err| internal_error(format!("failed to materialize thread metadata: {err}")))
    }

    async fn state_db_for_materialized_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<StateDbHandle, JSONRPCErrorError> {
        if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
            if thread.rollout_path().is_none() {
                return Err(invalid_request(format!(
                    "ephemeral thread does not support monitors: {thread_id}"
                )));
            }
            if let Some(state_db) = thread.state_db() {
                return Ok(state_db);
            }
        } else {
            codex_rollout::find_thread_path_by_id_str(
                &self.config.codex_home,
                &thread_id.to_string(),
                self.state_db.as_deref(),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to locate thread id {thread_id}: {err}"))
            })?
            .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?;
        }

        self.state_db
            .clone()
            .ok_or_else(|| internal_error("sqlite state db unavailable for monitors"))
    }

    async fn ensure_thread_monitor_capacity(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
    ) -> Result<(), JSONRPCErrorError> {
        let monitors = state_db
            .thread_monitors()
            .list_thread_monitors(thread_id)
            .await
            .map_err(|err| internal_error(format!("failed to count thread monitors: {err}")))?;
        if monitors.len() >= MAX_MONITOR_LIMIT {
            return Err(invalid_request(format!(
                "a thread can have at most {MAX_MONITOR_LIMIT} monitors"
            )));
        }
        Ok(())
    }

    async fn resolve_monitor_id_for_thread(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        monitor_id: &str,
    ) -> Result<String, JSONRPCErrorError> {
        self.find_monitor_id_for_thread(state_db, thread_id, monitor_id)
            .await?
            .ok_or_else(|| invalid_request(format!("monitor not found: {}", monitor_id.trim())))
    }

    async fn find_monitor_id_for_thread(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        monitor_id: &str,
    ) -> Result<Option<String>, JSONRPCErrorError> {
        let monitor_id = monitor_id.trim();
        if monitor_id.is_empty() {
            return Err(invalid_request("monitor id must not be empty"));
        }

        let monitors = state_db
            .thread_monitors()
            .list_thread_monitors(thread_id)
            .await
            .map_err(|err| internal_error(format!("failed to list thread monitors: {err}")))?;
        if let Some(monitor) = monitors
            .iter()
            .find(|monitor| monitor.monitor_id == monitor_id)
        {
            return Ok(Some(monitor.monitor_id.clone()));
        }

        let matches = monitors
            .iter()
            .filter(|monitor| monitor.monitor_id.starts_with(monitor_id))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [monitor] => Ok(Some(monitor.monitor_id.clone())),
            [] => Ok(None),
            _ => Err(invalid_request(format!(
                "monitor id prefix is ambiguous: {monitor_id}"
            ))),
        }
    }

    async fn load_monitor_for_thread(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        monitor_id: &str,
    ) -> Result<codex_state::ThreadMonitor, JSONRPCErrorError> {
        let monitor = state_db
            .thread_monitors()
            .get_thread_monitor(monitor_id)
            .await
            .map_err(|err| internal_error(format!("failed to read thread monitor: {err}")))?;
        let Some(monitor) = monitor else {
            return Err(invalid_request(format!("monitor not found: {monitor_id}")));
        };
        if monitor.thread_id != thread_id {
            return Err(invalid_request(format!("monitor not found: {monitor_id}")));
        }
        Ok(monitor)
    }

    async fn emit_thread_monitor_updated(&self, thread_id: ThreadId, monitor: ThreadMonitor) {
        self.outgoing
            .send_server_notification(ServerNotification::ThreadMonitorUpdated(
                ThreadMonitorUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    monitor,
                },
            ))
            .await;
    }

    async fn emit_thread_monitor_deleted(&self, thread_id: ThreadId, monitor_id: String) {
        self.outgoing
            .send_server_notification(ServerNotification::ThreadMonitorDeleted(
                ThreadMonitorDeletedNotification {
                    thread_id: thread_id.to_string(),
                    monitor_id,
                },
            ))
            .await;
    }
}

fn parse_thread_id_for_monitor_request(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}
