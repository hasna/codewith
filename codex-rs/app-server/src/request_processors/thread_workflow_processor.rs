use super::thread_goal_processor::api_thread_goal_plan_from_state;
use super::*;

#[derive(Clone)]
pub(crate) struct ThreadWorkflowRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    config: Arc<Config>,
    state_db: Option<StateDbHandle>,
}

impl ThreadWorkflowRequestProcessor {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        config: Arc<Config>,
        state_db: Option<StateDbHandle>,
    ) -> Self {
        Self {
            thread_manager,
            config,
            state_db,
        }
    }

    pub(crate) async fn thread_workflow_create(
        &self,
        params: ThreadWorkflowCreateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_create_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_get(
        &self,
        params: ThreadWorkflowGetParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_get_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_list(
        &self,
        params: ThreadWorkflowListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_run_list(
        &self,
        params: ThreadWorkflowRunListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_run_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_run_get(
        &self,
        params: ThreadWorkflowRunGetParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_run_get_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_run_start(
        &self,
        params: ThreadWorkflowRunStartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_run_start_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_run_pause(
        &self,
        params: ThreadWorkflowRunPauseParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_run_pause_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_run_resume(
        &self,
        params: ThreadWorkflowRunResumeParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_run_resume_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_workflow_run_cancel(
        &self,
        params: ThreadWorkflowRunCancelParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_workflow_run_cancel_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    async fn thread_workflow_create_inner(
        &self,
        params: ThreadWorkflowCreateParams,
    ) -> Result<ThreadWorkflowCreateResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let workflow = state_db
            .workflows()
            .save_workflow_spec_yaml(codex_state::WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: params.yaml,
            })
            .await
            .map_err(workflow_store_error)?;
        Ok(ThreadWorkflowCreateResponse {
            workflow: api_thread_workflow_from_state(thread_id, workflow),
        })
    }

    async fn thread_workflow_get_inner(
        &self,
        params: ThreadWorkflowGetParams,
    ) -> Result<ThreadWorkflowGetResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let workflow = state_db
            .workflows()
            .get_thread_workflow_spec(thread_id, params.workflow_record_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read thread workflow: {err}")))?
            .map(|workflow| api_thread_workflow_from_state(thread_id, workflow));
        Ok(ThreadWorkflowGetResponse { workflow })
    }

    async fn thread_workflow_list_inner(
        &self,
        params: ThreadWorkflowListParams,
    ) -> Result<ThreadWorkflowListResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let cursor = parse_workflow_list_cursor(params.cursor.as_deref())?;
        let limit = params
            .limit
            .unwrap_or(codex_state::DEFAULT_THREAD_WORKFLOW_LIST_LIMIT);
        let page = state_db
            .workflows()
            .list_thread_workflow_specs_page(thread_id, cursor, limit)
            .await
            .map_err(|err| internal_error(format!("failed to list thread workflows: {err}")))?;
        let data = page
            .data
            .into_iter()
            .map(|workflow| api_thread_workflow_from_state(thread_id, workflow))
            .collect();
        Ok(ThreadWorkflowListResponse {
            data,
            next_cursor: page.next_cursor,
        })
    }

    async fn thread_workflow_run_list_inner(
        &self,
        params: ThreadWorkflowRunListParams,
    ) -> Result<ThreadWorkflowRunListResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let cursor = parse_workflow_list_cursor(params.cursor.as_deref())?;
        let limit = params
            .limit
            .unwrap_or(codex_state::DEFAULT_THREAD_WORKFLOW_RUN_LIST_LIMIT);
        let page = state_db
            .workflows()
            .list_thread_workflow_runs_page(thread_id, cursor, limit)
            .await
            .map_err(|err| internal_error(format!("failed to list thread workflow runs: {err}")))?;
        let data = page
            .data
            .into_iter()
            .map(|snapshot| api_thread_workflow_run_from_state(&snapshot))
            .collect();
        Ok(ThreadWorkflowRunListResponse {
            data,
            next_cursor: page.next_cursor,
        })
    }

    async fn thread_workflow_run_get_inner(
        &self,
        params: ThreadWorkflowRunGetParams,
    ) -> Result<ThreadWorkflowRunGetResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let run = state_db
            .workflows()
            .get_workflow_run_snapshot(params.run_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read thread workflow run: {err}")))?
            .filter(|snapshot| snapshot.run.source_thread_id == Some(thread_id))
            .map(api_thread_workflow_run_snapshot_from_state);
        Ok(ThreadWorkflowRunGetResponse { run })
    }

    async fn thread_workflow_run_start_inner(
        &self,
        params: ThreadWorkflowRunStartParams,
    ) -> Result<ThreadWorkflowRunStartResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let idempotency_key = params.idempotency_key.and_then(normalize_optional_string);
        let snapshot = state_db
            .workflows()
            .create_workflow_run(codex_state::WorkflowRunCreateParams {
                workflow_record_id: params.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: idempotency_key.clone(),
            })
            .await
            .map_err(|err| {
                invalid_request(format!("failed to start thread workflow run: {err}"))
            })?;
        let goal_plan = state_db
            .project_workflow_run_to_goal_plan(codex_state::WorkflowGoalPlanProjectionParams {
                workflow_run_id: snapshot.run.run_id.clone(),
                thread_id,
                idempotency_key,
            })
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to project workflow run into task plan: {err}"
                ))
            })?
            .map(|outcome| api_thread_goal_plan_from_state(outcome.snapshot));
        Ok(ThreadWorkflowRunStartResponse {
            run: api_thread_workflow_run_snapshot_from_state(snapshot),
            goal_plan,
        })
    }

    async fn thread_workflow_run_pause_inner(
        &self,
        params: ThreadWorkflowRunPauseParams,
    ) -> Result<ThreadWorkflowRunPauseResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        if !workflow_run_belongs_to_thread(&state_db, thread_id, params.run_id.as_str()).await? {
            return Ok(ThreadWorkflowRunPauseResponse { run: None });
        }
        let run = state_db
            .workflows()
            .pause_workflow_run(codex_state::WorkflowRunPauseParams {
                run_id: params.run_id,
                reason: params
                    .reason
                    .and_then(normalize_optional_string)
                    .unwrap_or_else(|| "user requested pause".to_string()),
            })
            .await
            .map_err(|err| internal_error(format!("failed to pause thread workflow run: {err}")))?
            .map(api_thread_workflow_run_snapshot_from_state);
        Ok(ThreadWorkflowRunPauseResponse { run })
    }

    async fn thread_workflow_run_resume_inner(
        &self,
        params: ThreadWorkflowRunResumeParams,
    ) -> Result<ThreadWorkflowRunResumeResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        if !workflow_run_belongs_to_thread(&state_db, thread_id, params.run_id.as_str()).await? {
            return Ok(ThreadWorkflowRunResumeResponse { run: None });
        }
        let run = state_db
            .workflows()
            .resume_workflow_run(codex_state::WorkflowRunResumeParams {
                run_id: params.run_id,
            })
            .await
            .map_err(|err| internal_error(format!("failed to resume thread workflow run: {err}")))?
            .map(api_thread_workflow_run_snapshot_from_state);
        Ok(ThreadWorkflowRunResumeResponse { run })
    }

    async fn thread_workflow_run_cancel_inner(
        &self,
        params: ThreadWorkflowRunCancelParams,
    ) -> Result<ThreadWorkflowRunCancelResponse, JSONRPCErrorError> {
        self.ensure_enabled()?;
        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        if !workflow_run_belongs_to_thread(&state_db, thread_id, params.run_id.as_str()).await? {
            return Ok(ThreadWorkflowRunCancelResponse { run: None });
        }
        let run = state_db
            .request_workflow_run_cancel(codex_state::WorkflowRunCancelParams {
                run_id: params.run_id,
                reason: params
                    .reason
                    .and_then(normalize_optional_string)
                    .unwrap_or_else(|| "user requested cancellation".to_string()),
            })
            .await
            .map_err(|err| internal_error(format!("failed to cancel thread workflow run: {err}")))?
            .map(api_thread_workflow_run_snapshot_from_state);
        Ok(ThreadWorkflowRunCancelResponse { run })
    }

    fn ensure_enabled(&self) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Workflows) {
            return Err(invalid_request("workflows feature is disabled"));
        }
        Ok(())
    }

    async fn state_db_for_materialized_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<StateDbHandle, JSONRPCErrorError> {
        if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
            if thread.rollout_path().is_none() {
                return Err(invalid_request(format!(
                    "ephemeral thread does not support workflows: {thread_id}"
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
            .ok_or_else(|| internal_error("sqlite state db unavailable for thread workflows"))
    }
}

fn api_thread_workflow_from_state(
    fallback_thread_id: ThreadId,
    workflow: codex_state::WorkflowSpecRecord,
) -> ThreadWorkflow {
    ThreadWorkflow {
        thread_id: workflow
            .source_thread_id
            .unwrap_or(fallback_thread_id)
            .to_string(),
        workflow_record_id: workflow.workflow_record_id,
        spec_workflow_id: workflow.spec_workflow_id,
        schema_version: workflow.schema_version,
        display_name: workflow.display_name,
        status: api_thread_workflow_status_from_state(workflow.status),
        source_yaml_sha256: workflow.source_yaml_sha256,
        agent_count: workflow.agent_count,
        step_count: workflow.step_count,
        parallel_group_count: workflow.parallel_group_count,
        verifier_count: workflow.verifier_count,
        run_command_verifier_count: workflow.run_command_verifier_count,
        model_routed_step_count: workflow.model_routed_step_count,
        created_at: workflow.created_at.timestamp(),
        updated_at: workflow.updated_at.timestamp(),
    }
}

fn api_thread_workflow_status_from_state(
    status: codex_state::WorkflowSpecStatus,
) -> ThreadWorkflowStatus {
    match status {
        codex_state::WorkflowSpecStatus::Draft => ThreadWorkflowStatus::Draft,
        codex_state::WorkflowSpecStatus::NeedsClarification => {
            ThreadWorkflowStatus::NeedsClarification
        }
        codex_state::WorkflowSpecStatus::Blocked => ThreadWorkflowStatus::Blocked,
    }
}

fn api_thread_workflow_run_snapshot_from_state(
    snapshot: codex_state::WorkflowRunSnapshot,
) -> ThreadWorkflowRunSnapshot {
    let run = api_thread_workflow_run_from_state(&snapshot);
    let steps = snapshot
        .steps
        .into_iter()
        .map(api_thread_workflow_run_step_from_state)
        .collect();
    let verifiers = snapshot
        .verifiers
        .into_iter()
        .map(api_thread_workflow_run_step_verifier_from_state)
        .collect();
    let events = snapshot
        .events
        .into_iter()
        .map(api_thread_workflow_run_event_from_state)
        .collect();
    ThreadWorkflowRunSnapshot {
        run,
        steps,
        verifiers,
        events,
    }
}

fn api_thread_workflow_run_from_state(
    snapshot: &codex_state::WorkflowRunSnapshot,
) -> ThreadWorkflowRun {
    let count_steps = |status| {
        snapshot
            .steps
            .iter()
            .filter(|step| step.status == status)
            .count() as i64
    };
    ThreadWorkflowRun {
        thread_id: snapshot
            .run
            .source_thread_id
            .map(|thread_id| thread_id.to_string()),
        run_id: snapshot.run.run_id.clone(),
        workflow_record_id: snapshot.run.workflow_record_id.clone(),
        spec_workflow_id: snapshot.run.spec_workflow_id.clone(),
        schema_version: snapshot.run.schema_version.clone(),
        source_yaml_sha256: snapshot.run.source_yaml_sha256.clone(),
        status: api_thread_workflow_run_status_from_state(&snapshot.run.status),
        status_reason: snapshot.run.status_reason.clone(),
        reason_code: snapshot.run.reason_code.clone(),
        generation: snapshot.run.generation,
        pending_step_count: count_steps(codex_state::WorkflowRunStepStatus::Pending),
        ready_step_count: count_steps(codex_state::WorkflowRunStepStatus::Ready),
        active_step_count: count_steps(codex_state::WorkflowRunStepStatus::Active),
        waiting_verifier_step_count: count_steps(
            codex_state::WorkflowRunStepStatus::WaitingVerifier,
        ),
        blocked_step_count: count_steps(codex_state::WorkflowRunStepStatus::Blocked),
        failed_step_count: count_steps(codex_state::WorkflowRunStepStatus::Failed),
        succeeded_step_count: count_steps(codex_state::WorkflowRunStepStatus::Succeeded),
        skipped_step_count: count_steps(codex_state::WorkflowRunStepStatus::Skipped),
        verifier_count: snapshot.verifiers.len() as i64,
        event_count: snapshot.events.len() as i64,
        created_at: snapshot.run.created_at.timestamp(),
        updated_at: snapshot.run.updated_at.timestamp(),
        started_at: snapshot
            .run
            .started_at
            .map(|timestamp| timestamp.timestamp()),
        completed_at: snapshot
            .run
            .completed_at
            .map(|timestamp| timestamp.timestamp()),
    }
}

fn api_thread_workflow_run_step_from_state(
    step: codex_state::WorkflowRunStep,
) -> ThreadWorkflowRunStep {
    ThreadWorkflowRunStep {
        step_run_id: step.step_run_id,
        step_id: step.step_id,
        sequence: step.sequence,
        title: step.title,
        agent_id: step.agent_id,
        status: api_thread_workflow_run_step_status_from_state(&step.status),
        status_reason: step.status_reason,
        reason_code: step.reason_code,
        depends_on: step.depends_on,
        background_agent_run_id: step.background_agent_run_id,
        created_at: step.created_at.timestamp(),
        updated_at: step.updated_at.timestamp(),
        started_at: step.started_at.map(|timestamp| timestamp.timestamp()),
        completed_at: step.completed_at.map(|timestamp| timestamp.timestamp()),
    }
}

fn api_thread_workflow_run_step_verifier_from_state(
    verifier: codex_state::WorkflowRunStepVerifier,
) -> ThreadWorkflowRunStepVerifier {
    ThreadWorkflowRunStepVerifier {
        verifier_run_id: verifier.verifier_run_id,
        step_id: verifier.step_id,
        verifier_id: verifier.verifier_id,
        verifier_type: verifier.verifier_type,
        status: api_thread_workflow_run_step_verifier_status_from_state(&verifier.status),
        status_reason: verifier.status_reason,
        reason_code: verifier.reason_code,
        attempt_count: verifier.attempt_count,
        max_attempts: verifier.max_attempts,
        created_at: verifier.created_at.timestamp(),
        updated_at: verifier.updated_at.timestamp(),
        completed_at: verifier.completed_at.map(|timestamp| timestamp.timestamp()),
    }
}

fn api_thread_workflow_run_event_from_state(
    event: codex_state::WorkflowRunEvent,
) -> ThreadWorkflowRunEvent {
    ThreadWorkflowRunEvent {
        seq: event.seq,
        event_type: event.event_type,
        actor_kind: event.actor_kind,
        actor_id: event.actor_id,
        step_run_id: event.step_run_id,
        verifier_run_id: event.verifier_run_id,
        visibility: event.visibility,
        created_at: event.created_at.timestamp(),
    }
}

fn api_thread_workflow_run_status_from_state(
    status: &codex_state::WorkflowRunStatus,
) -> ThreadWorkflowRunStatus {
    match status {
        codex_state::WorkflowRunStatus::Pending => ThreadWorkflowRunStatus::Pending,
        codex_state::WorkflowRunStatus::Running => ThreadWorkflowRunStatus::Running,
        codex_state::WorkflowRunStatus::Waiting => ThreadWorkflowRunStatus::Waiting,
        codex_state::WorkflowRunStatus::Blocked => ThreadWorkflowRunStatus::Blocked,
        codex_state::WorkflowRunStatus::Paused => ThreadWorkflowRunStatus::Paused,
        codex_state::WorkflowRunStatus::CancelRequested => ThreadWorkflowRunStatus::CancelRequested,
        codex_state::WorkflowRunStatus::Cancelled => ThreadWorkflowRunStatus::Cancelled,
        codex_state::WorkflowRunStatus::Failed => ThreadWorkflowRunStatus::Failed,
        codex_state::WorkflowRunStatus::Completed => ThreadWorkflowRunStatus::Completed,
        codex_state::WorkflowRunStatus::Other(_) => ThreadWorkflowRunStatus::Other,
    }
}

fn api_thread_workflow_run_step_status_from_state(
    status: &codex_state::WorkflowRunStepStatus,
) -> ThreadWorkflowRunStepStatus {
    match status {
        codex_state::WorkflowRunStepStatus::Pending => ThreadWorkflowRunStepStatus::Pending,
        codex_state::WorkflowRunStepStatus::Ready => ThreadWorkflowRunStepStatus::Ready,
        codex_state::WorkflowRunStepStatus::Active => ThreadWorkflowRunStepStatus::Active,
        codex_state::WorkflowRunStepStatus::WaitingVerifier => {
            ThreadWorkflowRunStepStatus::WaitingVerifier
        }
        codex_state::WorkflowRunStepStatus::Blocked => ThreadWorkflowRunStepStatus::Blocked,
        codex_state::WorkflowRunStepStatus::Skipped => ThreadWorkflowRunStepStatus::Skipped,
        codex_state::WorkflowRunStepStatus::Cancelled => ThreadWorkflowRunStepStatus::Cancelled,
        codex_state::WorkflowRunStepStatus::Failed => ThreadWorkflowRunStepStatus::Failed,
        codex_state::WorkflowRunStepStatus::Succeeded => ThreadWorkflowRunStepStatus::Succeeded,
        codex_state::WorkflowRunStepStatus::Other(_) => ThreadWorkflowRunStepStatus::Other,
    }
}

fn api_thread_workflow_run_step_verifier_status_from_state(
    status: &codex_state::WorkflowRunStepVerifierStatus,
) -> ThreadWorkflowRunStepVerifierStatus {
    match status {
        codex_state::WorkflowRunStepVerifierStatus::Pending => {
            ThreadWorkflowRunStepVerifierStatus::Pending
        }
        codex_state::WorkflowRunStepVerifierStatus::Running => {
            ThreadWorkflowRunStepVerifierStatus::Running
        }
        codex_state::WorkflowRunStepVerifierStatus::Blocked => {
            ThreadWorkflowRunStepVerifierStatus::Blocked
        }
        codex_state::WorkflowRunStepVerifierStatus::Passed => {
            ThreadWorkflowRunStepVerifierStatus::Passed
        }
        codex_state::WorkflowRunStepVerifierStatus::Failed => {
            ThreadWorkflowRunStepVerifierStatus::Failed
        }
        codex_state::WorkflowRunStepVerifierStatus::Skipped => {
            ThreadWorkflowRunStepVerifierStatus::Skipped
        }
        codex_state::WorkflowRunStepVerifierStatus::Other(_) => {
            ThreadWorkflowRunStepVerifierStatus::Other
        }
    }
}

async fn workflow_run_belongs_to_thread(
    state_db: &StateDbHandle,
    thread_id: ThreadId,
    run_id: &str,
) -> Result<bool, JSONRPCErrorError> {
    state_db
        .workflows()
        .get_workflow_run_snapshot(run_id)
        .await
        .map_err(|err| internal_error(format!("failed to read thread workflow run: {err}")))
        .map(|snapshot| {
            snapshot.is_some_and(|snapshot| snapshot.run.source_thread_id == Some(thread_id))
        })
}

fn workflow_store_error(err: anyhow::Error) -> JSONRPCErrorError {
    if let Some(err) = err.downcast_ref::<codex_workflows::WorkflowSpecError>() {
        return invalid_request(sanitized_workflow_error(err));
    }
    internal_error(format!("failed to save thread workflow: {err}"))
}

fn sanitized_workflow_error(err: &codex_workflows::WorkflowSpecError) -> &'static str {
    match err {
        codex_workflows::WorkflowSpecError::EmptyDocument => "workflow YAML is empty",
        codex_workflows::WorkflowSpecError::DocumentTooLarge { .. } => {
            "workflow YAML exceeds the validation byte limit"
        }
        codex_workflows::WorkflowSpecError::MarkdownFence => {
            "workflow YAML must be a raw YAML document without Markdown fences"
        }
        codex_workflows::WorkflowSpecError::ParseYaml(_) => "workflow YAML could not be parsed",
        codex_workflows::WorkflowSpecError::UnsupportedYamlFeature { .. } => {
            "workflow YAML uses an unsupported YAML feature"
        }
        codex_workflows::WorkflowSpecError::Invalid(_) => {
            "workflow YAML does not satisfy the workflow spec invariants"
        }
    }
}

fn parse_thread_id_for_request(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}

fn parse_workflow_list_cursor(cursor: Option<&str>) -> Result<Option<u32>, JSONRPCErrorError> {
    cursor
        .map(|cursor| {
            cursor
                .parse::<u32>()
                .map_err(|_| invalid_request("workflow list cursor is invalid"))
        })
        .transpose()
}

fn normalize_optional_string(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
