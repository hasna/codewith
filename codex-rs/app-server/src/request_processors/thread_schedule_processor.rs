use super::thread_schedule_api::*;
use super::*;

const RUN_NOW_LEASE_SECONDS: u64 = 10 * 60;

#[derive(Clone)]
pub(crate) struct ThreadScheduleRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    thread_state_manager: ThreadStateManager,
    state_db: Option<StateDbHandle>,
    schedule_runtime: ThreadScheduleRuntime,
}

impl ThreadScheduleRequestProcessor {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        thread_state_manager: ThreadStateManager,
        state_db: Option<StateDbHandle>,
        schedule_runtime: ThreadScheduleRuntime,
    ) -> Self {
        Self {
            thread_manager,
            outgoing,
            config,
            thread_state_manager,
            state_db,
            schedule_runtime,
        }
    }

    pub(crate) async fn thread_schedule_create(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleCreateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_create_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_schedule_list(
        &self,
        params: ThreadScheduleListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_schedule_get(
        &self,
        params: ThreadScheduleGetParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_get_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_schedule_update(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleUpdateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_update_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_schedule_pause(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadSchedulePauseParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_status_inner(
            request_id,
            params.thread_id,
            params.schedule_id,
            codex_state::ThreadScheduleStatus::Paused,
            ScheduleStatusResponseKind::Pause,
        )
        .await
        .map(|()| None)
    }

    pub(crate) async fn thread_schedule_resume(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleResumeParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_status_inner(
            request_id,
            params.thread_id,
            params.schedule_id,
            codex_state::ThreadScheduleStatus::Active,
            ScheduleStatusResponseKind::Resume,
        )
        .await
        .map(|()| None)
    }

    pub(crate) async fn thread_schedule_delete(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_delete_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_schedule_run_now(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleRunNowParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_schedule_run_now_inner(request_id, params)
            .await
            .map(|()| None)
    }

    async fn thread_schedule_create_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleCreateParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let parent_schedule_id = normalize_parent_schedule_id(params.parent_schedule_id)?;
        let prompt_source = params
            .prompt_source
            .unwrap_or(ThreadSchedulePromptSource::Inline);
        let schedule = api_thread_schedule_spec_to_state(params.schedule)?;
        let timezone = normalize_timezone(params.timezone)?;
        let now = Utc::now();
        let explicit_next_run_at = params
            .next_run_at
            .map(|value| timestamp_to_datetime(value, "nextRunAt"))
            .transpose()?;
        let next_run_at = match explicit_next_run_at {
            Some(next_run_at) => Some(next_run_at),
            None if matches!(schedule, codex_state::ThreadScheduleSpec::Once) => {
                return Err(invalid_request(
                    "nextRunAt is required for one-time schedules",
                ));
            }
            None => thread_schedule_runtime::next_thread_schedule_run_at(
                &schedule,
                timezone.as_str(),
                now,
            )
            .map_err(|err| invalid_request(err.to_string()))?,
        };
        let expires_at = params
            .expires_at
            .map(|value| timestamp_to_datetime(value, "expiresAt"))
            .transpose()?
            .or_else(|| {
                if matches!(schedule, codex_state::ThreadScheduleSpec::Once) {
                    None
                } else {
                    thread_schedule_runtime::default_thread_schedule_expires_at(now)
                }
            });
        validate_schedule_expiry(next_run_at, expires_at)?;

        let (state_db, listener_command_tx) = self.prepare_schedule_mutation(thread_id).await?;
        self.ensure_thread_schedule_capacity(&state_db, thread_id)
            .await?;
        let auth_profile = self
            .auth_profile_for_schedule_create(&state_db, thread_id)
            .await;
        let (prompt, state_prompt_source) = match prompt_source {
            ThreadSchedulePromptSource::Inline => (
                validate_schedule_prompt(params.prompt.as_str())?,
                codex_state::ThreadSchedulePromptSource::Inline,
            ),
            ThreadSchedulePromptSource::Default => {
                crate::request_processors::thread_schedule_default_prompt::resolve_default_loop_prompt_for_thread(
                    &state_db,
                    thread_id,
                    self.config.cwd.as_path(),
                    self.config.codex_home.as_path(),
                )
                .await
                .map_err(|err| invalid_request(err.to_string()))?;
                (
                    crate::request_processors::thread_schedule_default_prompt::DEFAULT_LOOP_PROMPT_DISPLAY
                        .to_string(),
                    codex_state::ThreadSchedulePromptSource::Default,
                )
            }
        };
        let create_params = codex_state::ThreadScheduleCreateParams {
            thread_id,
            prompt,
            prompt_source: state_prompt_source,
            schedule,
            timezone,
            status: codex_state::ThreadScheduleStatus::Active,
            next_run_at,
            expires_at,
        };
        let schedule = match parent_schedule_id {
            Some(parent_schedule_id) => {
                state_db
                    .thread_schedules()
                    .create_nested_thread_schedule_for_auth_profile(
                        create_params,
                        parent_schedule_id,
                        auth_profile,
                    )
                    .await
            }
            None => {
                state_db
                    .thread_schedules()
                    .create_thread_schedule_for_auth_profile(create_params, auth_profile)
                    .await
            }
        }
        .map_err(|err| schedule_mutation_error("create", err))?;
        let schedule = api_thread_schedule_from_state(schedule);

        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadScheduleCreateResponse {
                    schedule: schedule.clone(),
                },
            )
            .await;
        self.emit_thread_schedule_updated_ordered(thread_id, schedule, listener_command_tx)
            .await;
        Ok(())
    }

    async fn auth_profile_for_schedule_create(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
    ) -> Option<String> {
        if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
            return thread.config_snapshot().await.selected_auth_profile;
        }
        if let Some(auth_profile) = self
            .auth_profile_from_thread_rollout(state_db, thread_id)
            .await
        {
            return auth_profile;
        }
        self.config.selected_auth_profile.clone()
    }

    async fn auth_profile_from_thread_rollout(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
    ) -> Option<Option<String>> {
        let rollout_path = match codex_rollout::find_thread_path_by_id_str(
            &self.config.codex_home,
            &thread_id.to_string(),
            Some(state_db),
        )
        .await
        {
            Ok(Some(path)) => path,
            Ok(None) => return None,
            Err(err) => {
                warn!("failed to locate rollout for schedule auth profile {thread_id}: {err}");
                return None;
            }
        };
        let initial_history =
            match codex_rollout::RolloutRecorder::get_rollout_history(&rollout_path).await {
                Ok(history) => history,
                Err(err) => {
                    warn!(
                        "failed to load rollout {} for schedule auth profile: {err}",
                        rollout_path.display()
                    );
                    return None;
                }
            };
        thread_schedule_runtime::schedule_resume_auth_profile(
            /*schedule_auth_profile*/ None,
            &initial_history,
        )
    }

    async fn thread_schedule_list_inner(
        &self,
        params: ThreadScheduleListParams,
    ) -> Result<ThreadScheduleListResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let limit = normalize_list_limit(params.limit)?;
        let offset = decode_cursor(params.cursor.as_deref())?;
        let schedules = state_db
            .thread_schedules()
            .list_thread_schedules(thread_id)
            .await
            .map_err(|err| internal_error(format!("failed to list thread schedules: {err}")))?;
        let next_offset = offset.saturating_add(limit);
        let mut page: Vec<_> = schedules.into_iter().skip(offset).take(limit + 1).collect();
        let has_more = page.len() > limit;
        if has_more {
            page.truncate(limit);
        }
        let data: Vec<ThreadSchedule> = page
            .into_iter()
            .map(api_thread_schedule_from_state)
            .collect();
        let next_cursor = if has_more {
            Some(next_offset.to_string())
        } else {
            None
        };
        Ok(ThreadScheduleListResponse { data, next_cursor })
    }

    async fn thread_schedule_get_inner(
        &self,
        params: ThreadScheduleGetParams,
    ) -> Result<ThreadScheduleGetResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let schedule_id = self
            .resolve_schedule_id_for_thread(&state_db, thread_id, params.schedule_id.as_str())
            .await?;
        let schedule = state_db
            .thread_schedules()
            .get_thread_schedule(schedule_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read thread schedule: {err}")))?
            .filter(|schedule| schedule.thread_id == thread_id);
        let stats = if schedule.is_some() {
            state_db
                .thread_schedules()
                .get_thread_schedule_stats(schedule_id.as_str())
                .await
                .map_err(|err| {
                    internal_error(format!("failed to read thread schedule stats: {err}"))
                })
                .map(api_thread_schedule_stats_from_state)?
        } else {
            ThreadScheduleStats::default()
        };
        Ok(ThreadScheduleGetResponse {
            schedule: schedule.map(api_thread_schedule_from_state),
            stats,
        })
    }

    async fn thread_schedule_update_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleUpdateParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let prompt = params
            .prompt
            .as_deref()
            .map(validate_schedule_prompt)
            .transpose()?;
        let prompt_source = prompt
            .as_ref()
            .map(|_| codex_state::ThreadSchedulePromptSource::Inline);
        let schedule = params
            .schedule
            .map(api_thread_schedule_spec_to_state)
            .transpose()?;
        let timezone = params.timezone.map(normalize_timezone_value).transpose()?;
        let status = params.status.map(api_thread_schedule_status_to_state);
        let next_run_at = params
            .next_run_at
            .map(|value| {
                value
                    .map(|timestamp| timestamp_to_datetime(timestamp, "nextRunAt"))
                    .transpose()
            })
            .transpose()?;
        let expires_at = params
            .expires_at
            .map(|value| {
                value
                    .map(|timestamp| timestamp_to_datetime(timestamp, "expiresAt"))
                    .transpose()
            })
            .transpose()?;
        let (state_db, listener_command_tx) = self.prepare_schedule_mutation(thread_id).await?;
        let schedule_id = self
            .resolve_schedule_id_for_thread(&state_db, thread_id, params.schedule_id.as_str())
            .await?;
        let existing = self
            .load_schedule_for_thread(&state_db, thread_id, schedule_id.as_str())
            .await?;
        let mut next_run_at = if next_run_at.is_none() && (schedule.is_some() || timezone.is_some())
        {
            let effective_schedule = schedule
                .clone()
                .unwrap_or_else(|| existing.schedule.clone());
            let effective_timezone = timezone
                .clone()
                .unwrap_or_else(|| existing.timezone.clone());
            if matches!(effective_schedule, codex_state::ThreadScheduleSpec::Once) {
                if existing.next_run_at.is_none() {
                    return Err(invalid_request(
                        "nextRunAt is required for one-time schedules",
                    ));
                }
                None
            } else {
                Some(
                    thread_schedule_runtime::next_thread_schedule_run_at(
                        &effective_schedule,
                        effective_timezone.as_str(),
                        Utc::now(),
                    )
                    .map_err(|err| invalid_request(err.to_string()))?,
                )
            }
        } else {
            next_run_at
        };
        let effective_schedule = schedule.as_ref().unwrap_or(&existing.schedule);
        let effective_status = status.unwrap_or(existing.status);
        let effective_timezone = timezone.as_ref().unwrap_or(&existing.timezone);
        let mut effective_next_run_at = next_run_at.unwrap_or(existing.next_run_at);
        if effective_status == codex_state::ThreadScheduleStatus::Active
            && effective_next_run_at.is_none()
        {
            if matches!(effective_schedule, codex_state::ThreadScheduleSpec::Once) {
                return Err(invalid_request(
                    "nextRunAt is required for one-time schedules",
                ));
            }
            let computed_next_run_at =
                next_active_recurring_run_at(effective_schedule, effective_timezone.as_str())?;
            next_run_at = Some(Some(computed_next_run_at));
            effective_next_run_at = Some(computed_next_run_at);
        }
        let effective_expires_at = expires_at.unwrap_or(existing.expires_at);
        if effective_status == codex_state::ThreadScheduleStatus::Active
            && let Some(expires_at) = effective_expires_at
            && expires_at <= Utc::now()
        {
            return Err(invalid_request(
                "schedule expiresAt must be in the future to resume",
            ));
        }
        validate_schedule_expiry(effective_next_run_at, effective_expires_at)?;
        let schedule = state_db
            .thread_schedules()
            .update_thread_schedule(
                schedule_id.as_str(),
                codex_state::ThreadScheduleUpdate {
                    prompt,
                    prompt_source,
                    schedule,
                    timezone,
                    status,
                    next_run_at,
                    expires_at,
                },
            )
            .await
            .map_err(|err| schedule_mutation_error("update", err))?
            .ok_or_else(|| invalid_request(format!("schedule not found: {schedule_id}")))?;
        let schedule = api_thread_schedule_from_state(schedule);

        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadScheduleUpdateResponse {
                    schedule: schedule.clone(),
                },
            )
            .await;
        self.emit_thread_schedule_updated_ordered(thread_id, schedule, listener_command_tx)
            .await;
        Ok(())
    }

    async fn thread_schedule_status_inner(
        &self,
        request_id: ConnectionRequestId,
        thread_id: String,
        schedule_id: String,
        status: codex_state::ThreadScheduleStatus,
        response_kind: ScheduleStatusResponseKind,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(thread_id.as_str())?;
        let (state_db, listener_command_tx) = self.prepare_schedule_mutation(thread_id).await?;
        let schedule_id = self
            .resolve_schedule_id_for_thread(&state_db, thread_id, schedule_id.as_str())
            .await?;
        let existing = self
            .load_schedule_for_thread(&state_db, thread_id, schedule_id.as_str())
            .await?;
        if status == codex_state::ThreadScheduleStatus::Active
            && let Some(expires_at) = existing.expires_at
            && expires_at <= Utc::now()
        {
            return Err(invalid_request(
                "schedule expiresAt must be in the future to resume",
            ));
        }
        let schedule = if status == codex_state::ThreadScheduleStatus::Active
            && existing.next_run_at.is_none()
        {
            if matches!(existing.schedule, codex_state::ThreadScheduleSpec::Once) {
                return Err(invalid_request(
                    "nextRunAt is required for one-time schedules",
                ));
            }
            let next_run_at =
                next_active_recurring_run_at(&existing.schedule, existing.timezone.as_str())?;
            validate_schedule_expiry(Some(next_run_at), existing.expires_at)?;
            state_db
                .thread_schedules()
                .resume_thread_schedule_at(schedule_id.as_str(), next_run_at)
                .await
                .map_err(|err| {
                    internal_error(format!("failed to update thread schedule status: {err}"))
                })?
        } else if status == codex_state::ThreadScheduleStatus::Active {
            validate_schedule_expiry(existing.next_run_at, existing.expires_at)?;
            state_db
                .thread_schedules()
                .resume_thread_schedule(schedule_id.as_str())
                .await
                .map_err(|err| {
                    internal_error(format!("failed to update thread schedule status: {err}"))
                })?
        } else {
            state_db
                .thread_schedules()
                .set_thread_schedule_status(schedule_id.as_str(), status)
                .await
                .map_err(|err| {
                    internal_error(format!("failed to update thread schedule status: {err}"))
                })?
        }
        .ok_or_else(|| invalid_request(format!("schedule not found: {schedule_id}")))?;
        let schedule = api_thread_schedule_from_state(schedule);

        self.outgoing
            .send_response(
                request_id.clone(),
                response_kind.response_payload(schedule.clone()),
            )
            .await;
        self.emit_thread_schedule_updated_ordered(thread_id, schedule, listener_command_tx)
            .await;
        Ok(())
    }

    async fn thread_schedule_delete_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleDeleteParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let (state_db, listener_command_tx) = self.prepare_schedule_mutation(thread_id).await?;
        let Some(schedule_id) = self
            .find_schedule_id_for_thread(&state_db, thread_id, params.schedule_id.as_str())
            .await?
        else {
            self.outgoing
                .send_response(
                    request_id.clone(),
                    ThreadScheduleDeleteResponse { deleted: false },
                )
                .await;
            return Ok(());
        };
        let deleted_schedule_ids = state_db
            .thread_schedules()
            .delete_thread_schedule_tree(schedule_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to delete thread schedule: {err}")))?;
        let deleted = !deleted_schedule_ids.is_empty();

        self.outgoing
            .send_response(request_id.clone(), ThreadScheduleDeleteResponse { deleted })
            .await;
        if deleted {
            for deleted_schedule_id in deleted_schedule_ids {
                self.emit_thread_schedule_deleted_ordered(
                    thread_id,
                    deleted_schedule_id,
                    listener_command_tx.clone(),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn thread_schedule_run_now_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadScheduleRunNowParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let (state_db, _listener_command_tx) = self.prepare_schedule_mutation(thread_id).await?;
        let schedule_id = self
            .resolve_schedule_id_for_thread(&state_db, thread_id, params.schedule_id.as_str())
            .await?;
        let lease_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let local_active_fresh_after = self.schedule_runtime.local_active_fresh_after(now);
        let claim = state_db
            .thread_schedules()
            .claim_thread_schedule_now_with_params(codex_state::ThreadScheduleNowClaimParams {
                schedule_id: schedule_id.as_str(),
                now,
                lease_id: lease_id.as_str(),
                lease_duration: Duration::from_secs(RUN_NOW_LEASE_SECONDS),
                local_active_owner_id: Some(self.schedule_runtime.local_active_owner_id()),
                local_active_fresh_after: Some(local_active_fresh_after),
            })
            .await
            .map_err(|err| internal_error(format!("failed to claim thread schedule: {err}")))?
            .ok_or_else(|| {
                invalid_request(format!(
                    "schedule is not active or is already running: {schedule_id}"
                ))
            })?;
        let run = api_thread_schedule_run_from_state(claim.run.clone());

        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadScheduleRunNowResponse { run: run.clone() },
            )
            .await;
        self.schedule_runtime.spawn_claim_execution(state_db, claim);
        Ok(())
    }

    fn ensure_enabled(&self) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::ScheduledTasks) {
            return Err(invalid_request("scheduled_tasks feature is disabled"));
        }
        Ok(())
    }

    async fn prepare_schedule_mutation(
        &self,
        thread_id: ThreadId,
    ) -> Result<
        (
            StateDbHandle,
            Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
        ),
        JSONRPCErrorError,
    > {
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        self.reconcile_materialized_thread(thread_id, &state_db)
            .await?;
        let listener_command_tx = {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        Ok((state_db, listener_command_tx))
    }

    async fn reconcile_materialized_thread(
        &self,
        thread_id: ThreadId,
        state_db: &StateDbHandle,
    ) -> Result<(), JSONRPCErrorError> {
        let running_thread = self.thread_manager.get_thread(thread_id).await.ok();
        let rollout_path = match running_thread.as_ref() {
            Some(thread) => {
                let rollout_path = thread.rollout_path().ok_or_else(|| {
                    invalid_request(format!(
                        "ephemeral thread does not support scheduled tasks: {thread_id}"
                    ))
                })?;
                thread
                    .try_ensure_rollout_materialized()
                    .await
                    .map_err(|err| {
                        internal_error(format!(
                            "failed to materialize thread rollout before scheduling: {err}"
                        ))
                    })?;
                thread.flush_rollout().await.map_err(|err| {
                    internal_error(format!(
                        "failed to flush thread rollout before scheduling: {err}"
                    ))
                })?;
                rollout_path
            }
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
        if let Some(thread) = running_thread.as_ref() {
            let existing_metadata = state_db
                .get_thread(thread_id)
                .await
                .map_err(|err| internal_error(format!("failed to read thread metadata: {err}")))?;
            self.upsert_running_thread_metadata(
                state_db,
                thread_id,
                thread,
                rollout_path.as_path(),
                existing_metadata,
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
        existing_metadata: Option<codex_state::ThreadMetadata>,
    ) -> Result<(), JSONRPCErrorError> {
        let config_snapshot = thread.config_snapshot().await;
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            rollout_path.to_path_buf(),
            Utc::now(),
            SessionSource::default(),
        );
        builder.thread_source = config_snapshot.thread_source;
        builder.model_provider = Some(config_snapshot.model_provider_id);
        builder.cwd = config_snapshot.cwd.to_path_buf();
        builder.approval_mode = config_snapshot.approval_policy;
        let session_metadata = builder.build(self.config.model_provider_id.as_str());
        let mut metadata = existing_metadata.unwrap_or_else(|| session_metadata.clone());
        metadata.rollout_path = session_metadata.rollout_path;
        metadata.thread_source = session_metadata.thread_source;
        metadata.model_provider = session_metadata.model_provider;
        metadata.cwd = session_metadata.cwd;
        metadata.approval_mode = session_metadata.approval_mode;
        match serde_json::to_string(&config_snapshot.permission_profile) {
            Ok(permission_profile) => {
                metadata.sandbox_policy = permission_profile;
            }
            Err(err) => {
                warn!(
                    thread_id = %thread_id,
                    "failed to serialize running thread permission profile for schedule metadata: {err}"
                );
            }
        }
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
                    "ephemeral thread does not support scheduled tasks: {thread_id}"
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
            .ok_or_else(|| internal_error("sqlite state db unavailable for scheduled tasks"))
    }

    async fn resolve_schedule_id_for_thread(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        schedule_id: &str,
    ) -> Result<String, JSONRPCErrorError> {
        self.find_schedule_id_for_thread(state_db, thread_id, schedule_id)
            .await?
            .ok_or_else(|| invalid_request(format!("schedule not found: {}", schedule_id.trim())))
    }

    async fn find_schedule_id_for_thread(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        schedule_id: &str,
    ) -> Result<Option<String>, JSONRPCErrorError> {
        let schedule_id = schedule_id.trim();
        if schedule_id.is_empty() {
            return Err(invalid_request("schedule id must not be empty"));
        }

        let schedules = state_db
            .thread_schedules()
            .list_thread_schedules(thread_id)
            .await
            .map_err(|err| internal_error(format!("failed to list thread schedules: {err}")))?;
        if let Some(schedule) = schedules
            .iter()
            .find(|schedule| schedule.schedule_id == schedule_id)
        {
            return Ok(Some(schedule.schedule_id.clone()));
        }

        let matches = schedules
            .iter()
            .filter(|schedule| schedule.schedule_id.starts_with(schedule_id))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [schedule] => Ok(Some(schedule.schedule_id.clone())),
            [] => Ok(None),
            _ => Err(invalid_request(format!(
                "schedule id prefix is ambiguous: {schedule_id}"
            ))),
        }
    }

    async fn load_schedule_for_thread(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
        schedule_id: &str,
    ) -> Result<codex_state::ThreadSchedule, JSONRPCErrorError> {
        let schedule = state_db
            .thread_schedules()
            .get_thread_schedule(schedule_id)
            .await
            .map_err(|err| internal_error(format!("failed to read thread schedule: {err}")))?;
        let Some(schedule) = schedule else {
            return Err(invalid_request(format!(
                "schedule not found: {schedule_id}"
            )));
        };
        if schedule.thread_id != thread_id {
            return Err(invalid_request(format!(
                "schedule not found: {schedule_id}"
            )));
        }
        Ok(schedule)
    }

    async fn ensure_thread_schedule_capacity(
        &self,
        state_db: &StateDbHandle,
        thread_id: ThreadId,
    ) -> Result<(), JSONRPCErrorError> {
        let schedules = state_db
            .thread_schedules()
            .list_thread_schedules(thread_id)
            .await
            .map_err(|err| internal_error(format!("failed to count thread schedules: {err}")))?;
        let active_count = schedules
            .iter()
            .filter(|schedule| schedule.status != codex_state::ThreadScheduleStatus::Expired)
            .count();
        if active_count >= MAX_SCHEDULE_LIMIT {
            return Err(invalid_request(format!(
                "a thread can have at most {MAX_SCHEDULE_LIMIT} active scheduled tasks"
            )));
        }
        Ok(())
    }

    async fn emit_thread_schedule_updated_ordered(
        &self,
        thread_id: ThreadId,
        schedule: ThreadSchedule,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = ThreadListenerCommand::EmitThreadScheduleUpdated {
                schedule: schedule.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue thread schedule update for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadScheduleUpdated(
                ThreadScheduleUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    schedule,
                },
            ))
            .await;
    }

    async fn emit_thread_schedule_deleted_ordered(
        &self,
        thread_id: ThreadId,
        schedule_id: String,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = ThreadListenerCommand::EmitThreadScheduleDeleted {
                schedule_id: schedule_id.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue thread schedule delete for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadScheduleDeleted(
                ThreadScheduleDeletedNotification {
                    thread_id: thread_id.to_string(),
                    schedule_id,
                },
            ))
            .await;
    }
}

fn next_active_recurring_run_at(
    schedule: &codex_state::ThreadScheduleSpec,
    timezone: &str,
) -> Result<DateTime<Utc>, JSONRPCErrorError> {
    thread_schedule_runtime::next_thread_schedule_run_at(schedule, timezone, Utc::now())
        .map_err(|err| invalid_request(err.to_string()))?
        .ok_or_else(|| invalid_request("nextRunAt is required for one-time schedules"))
}

fn schedule_mutation_error(action: &str, err: anyhow::Error) -> JSONRPCErrorError {
    let message = err.to_string();
    if message.starts_with("invalid nested loop:") {
        invalid_request(message)
    } else {
        internal_error(format!("failed to {action} thread schedule: {message}"))
    }
}

enum ScheduleStatusResponseKind {
    Pause,
    Resume,
}

impl ScheduleStatusResponseKind {
    fn response_payload(self, schedule: ThreadSchedule) -> ClientResponsePayload {
        match self {
            Self::Pause => ThreadSchedulePauseResponse { schedule }.into(),
            Self::Resume => ThreadScheduleResumeResponse { schedule }.into(),
        }
    }
}
