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
