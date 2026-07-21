use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_protocol::ThreadId;
use codex_state::StateRuntime;
use codex_tools::JsonSchema;
use codex_tools::ToolExposure;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::manager_output::goal_plan_projection_json;
use crate::manager_output::run_snapshot_json;
use crate::manager_output::run_summary_json;
use crate::manager_output::workflow_json;

pub const MANAGE_WORKFLOW_TOOL_NAME: &str = "manage_workflow";

pub(crate) struct ManageWorkflowTool {
    enabled: Arc<AtomicBool>,
    runtime: ManageWorkflowRuntime,
}

enum ManageWorkflowRuntime {
    Available {
        state_db: Arc<StateRuntime>,
        thread_id: ThreadId,
    },
    Unavailable {
        reason: &'static str,
    },
}

impl ManageWorkflowTool {
    pub(crate) fn new(
        enabled: Arc<AtomicBool>,
        state_db: Arc<StateRuntime>,
        thread_id: ThreadId,
    ) -> Self {
        Self {
            enabled,
            runtime: ManageWorkflowRuntime::Available {
                state_db,
                thread_id,
            },
        }
    }

    pub(crate) fn unavailable(enabled: Arc<AtomicBool>, reason: &'static str) -> Self {
        Self {
            enabled,
            runtime: ManageWorkflowRuntime::Unavailable { reason },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct ManageWorkflowArgs {
    action: ManageWorkflowAction,
    workflow_record_id: Option<String>,
    run_id: Option<String>,
    step_id: Option<String>,
    yaml: Option<String>,
    idempotency_key: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManageWorkflowAction {
    List,
    Create,
    Read,
    Start,
    ListRuns,
    ReadRun,
    Pause,
    Resume,
    Cancel,
    ApproveStep,
    RejectStep,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolCall> for ManageWorkflowTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MANAGE_WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        let nullable_string = |description: &str| {
            JsonSchema::any_of(
                vec![
                    JsonSchema::string(/*description*/ None),
                    JsonSchema::null(/*description*/ None),
                ],
                Some(description.to_string()),
            )
        };
        let nullable_integer = |description: &str| {
            JsonSchema::any_of(
                vec![
                    JsonSchema::integer(/*description*/ None),
                    JsonSchema::null(/*description*/ None),
                ],
                Some(description.to_string()),
            )
        };
        let properties = BTreeMap::from([
            (
                "action".to_string(),
                JsonSchema::string_enum(
                    vec![
                        json!("list"),
                        json!("create"),
                        json!("read"),
                        json!("start"),
                        json!("list_runs"),
                        json!("read_run"),
                        json!("pause"),
                        json!("resume"),
                        json!("cancel"),
                        json!("approve_step"),
                        json!("reject_step"),
                    ],
                    Some(
                        "Workflow management action to perform for the current thread.".to_string(),
                    ),
                ),
            ),
            (
                "workflow_record_id".to_string(),
                nullable_string("Workflow record id for read/start actions."),
            ),
            (
                "run_id".to_string(),
                nullable_string(
                    "Workflow run id for read_run/pause/resume/cancel/approve_step/reject_step actions.",
                ),
            ),
            (
                "step_id".to_string(),
                nullable_string(
                    "Workflow step id for approve_step/reject_step actions. Only gated steps can be approved.",
                ),
            ),
            (
                "yaml".to_string(),
                nullable_string("Raw workflow YAML only for create. Outputs never echo YAML."),
            ),
            (
                "idempotency_key".to_string(),
                nullable_string("Optional start idempotency key; blank values are ignored."),
            ),
            (
                "cursor".to_string(),
                nullable_string("Optional list cursor returned by a previous list action."),
            ),
            (
                "limit".to_string(),
                nullable_integer("Optional page size. The state store clamps oversized values."),
            ),
            (
                "reason".to_string(),
                nullable_string("Optional pause/cancel reason. Persisted output is sanitized."),
            ),
        ]);

        ToolSpec::Function(ResponsesApiTool {
            name: MANAGE_WORKFLOW_TOOL_NAME.to_string(),
            description: "Manage Codewith workflows for the current thread: list/create/read workflow specs, start runs into durable task plans, list/read runs, pause/resume/cancel runs, and approve or reject gated steps that require explicit user consent before running. This direct model tool is thread-bound and returns sanitized metadata without raw YAML, verifier command definitions, or event payloads."
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["action".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    async fn handle(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err(FunctionCallError::RespondToModel(
                "workflow management is unavailable because the workflows feature is disabled"
                    .to_string(),
            ));
        }

        let args: ManageWorkflowArgs = serde_json::from_str(call.function_arguments()?).map_err(
            |_| {
                FunctionCallError::RespondToModel(format!(
                    "invalid {MANAGE_WORKFLOW_TOOL_NAME} arguments: expected a JSON object with a valid action"
                ))
            },
        )?;
        let value = match args.action {
            ManageWorkflowAction::List => self.list_workflows(args).await?,
            ManageWorkflowAction::Create => self.create_workflow(args).await?,
            ManageWorkflowAction::Read => self.read_workflow(args).await?,
            ManageWorkflowAction::Start => self.start_run(args).await?,
            ManageWorkflowAction::ListRuns => self.list_runs(args).await?,
            ManageWorkflowAction::ReadRun => self.read_run(args).await?,
            ManageWorkflowAction::Pause => self.pause_run(args).await?,
            ManageWorkflowAction::Resume => self.resume_run(args).await?,
            ManageWorkflowAction::Cancel => self.cancel_run(args).await?,
            ManageWorkflowAction::ApproveStep => {
                self.decide_step(args, codex_state::WorkflowRunStepApprovalDecision::Approve)
                    .await?
            }
            ManageWorkflowAction::RejectStep => {
                self.decide_step(args, codex_state::WorkflowRunStepApprovalDecision::Reject)
                    .await?
            }
        };
        Ok(Box::new(JsonToolOutput::new(value)))
    }
}

impl ManageWorkflowTool {
    fn runtime(&self) -> Result<(&StateRuntime, ThreadId), FunctionCallError> {
        match &self.runtime {
            ManageWorkflowRuntime::Available {
                state_db,
                thread_id,
            } => Ok((state_db.as_ref(), *thread_id)),
            ManageWorkflowRuntime::Unavailable { reason } => Err(respond(*reason)),
        }
    }

    async fn list_workflows(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, thread_id) = self.runtime()?;
        let cursor = parse_list_cursor(args.cursor.as_deref())?;
        let limit = args
            .limit
            .unwrap_or(codex_state::DEFAULT_THREAD_WORKFLOW_LIST_LIMIT);
        let page = state_db
            .workflows()
            .list_thread_workflow_specs_page(thread_id, cursor, limit)
            .await
            .map_err(|_| respond("failed to list workflows for current thread"))?;
        Ok(json!({
            "action": "list",
            "data": page.data.into_iter().map(workflow_json).collect::<Vec<_>>(),
            "nextCursor": page.next_cursor,
        }))
    }

    async fn create_workflow(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, thread_id) = self.runtime()?;
        let yaml = required_field(args.yaml, "yaml", "create")?;
        let workflow = state_db
            .workflows()
            .save_workflow_spec_yaml(codex_state::WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: yaml,
            })
            .await
            .map_err(workflow_create_error)?;
        Ok(json!({
            "action": "create",
            "workflow": workflow_json(workflow),
        }))
    }

    async fn read_workflow(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, thread_id) = self.runtime()?;
        let workflow_record_id =
            required_field(args.workflow_record_id, "workflow_record_id", "read")?;
        let workflow = state_db
            .workflows()
            .get_thread_workflow_spec(thread_id, workflow_record_id.as_str())
            .await
            .map_err(|_| respond("failed to read workflow for current thread"))?
            .map(workflow_json);
        Ok(json!({
            "action": "read",
            "workflow": workflow,
        }))
    }

    async fn start_run(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, thread_id) = self.runtime()?;
        let workflow_record_id =
            required_field(args.workflow_record_id, "workflow_record_id", "start")?;
        let idempotency_key = normalize_optional_string(args.idempotency_key);
        if state_db
            .workflows()
            .get_thread_workflow_spec(thread_id, workflow_record_id.as_str())
            .await
            .map_err(|_| respond("failed to read workflow for current thread"))?
            .is_none()
        {
            return Ok(json!({
                "action": "start",
                "run": null,
                "goalPlan": null,
                "error": "workflow not found for current thread",
            }));
        }
        let snapshot = state_db
            .workflows()
            .create_workflow_run(codex_state::WorkflowRunCreateParams {
                workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: idempotency_key.clone(),
            })
            .await
            .map_err(|_| respond("failed to start workflow run"))?;
        let run_id = snapshot.run.run_id.clone();
        let run = run_snapshot_json(&snapshot);
        let goal_plan = state_db
            .project_workflow_run_to_goal_plan(codex_state::WorkflowGoalPlanProjectionParams {
                workflow_run_id: run_id,
                thread_id,
                idempotency_key,
            })
            .await
            .map_err(|_| respond("failed to project workflow run into task plan"))?
            .map(goal_plan_projection_json);
        Ok(json!({
            "action": "start",
            "run": run,
            "goalPlan": goal_plan,
        }))
    }

    async fn list_runs(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, thread_id) = self.runtime()?;
        let cursor = parse_list_cursor(args.cursor.as_deref())?;
        let limit = args
            .limit
            .unwrap_or(codex_state::DEFAULT_THREAD_WORKFLOW_RUN_LIST_LIMIT);
        let page = state_db
            .workflows()
            .list_thread_workflow_runs_page(thread_id, cursor, limit)
            .await
            .map_err(|_| respond("failed to list workflow runs for current thread"))?;
        Ok(json!({
            "action": "list_runs",
            "data": page.data.iter().map(run_summary_json).collect::<Vec<_>>(),
            "nextCursor": page.next_cursor,
        }))
    }

    async fn read_run(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let run_id = required_field(args.run_id, "run_id", "read_run")?;
        let run = self.thread_run_snapshot(run_id.as_str()).await?;
        Ok(json!({
            "action": "read_run",
            "run": run.as_ref().map(run_snapshot_json),
        }))
    }

    async fn pause_run(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, _) = self.runtime()?;
        let run_id = required_field(args.run_id, "run_id", "pause")?;
        if self.thread_run_snapshot(run_id.as_str()).await?.is_none() {
            return Ok(not_found_run_response("pause"));
        }
        let snapshot = state_db
            .pause_workflow_run(codex_state::WorkflowRunPauseParams {
                run_id,
                reason: normalize_optional_string(args.reason)
                    .unwrap_or_else(|| "model requested pause".to_string()),
            })
            .await
            .map_err(|_| respond("failed to pause workflow run"))?;
        Ok(json!({
            "action": "pause",
            "run": snapshot.as_ref().map(run_snapshot_json),
        }))
    }

    async fn resume_run(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, _) = self.runtime()?;
        let run_id = required_field(args.run_id, "run_id", "resume")?;
        if self.thread_run_snapshot(run_id.as_str()).await?.is_none() {
            return Ok(not_found_run_response("resume"));
        }
        let snapshot = state_db
            .resume_workflow_run(codex_state::WorkflowRunResumeParams { run_id })
            .await
            .map_err(|_| respond("failed to resume workflow run"))?;
        Ok(json!({
            "action": "resume",
            "run": snapshot.as_ref().map(run_snapshot_json),
        }))
    }

    async fn cancel_run(&self, args: ManageWorkflowArgs) -> Result<Value, FunctionCallError> {
        let (state_db, _) = self.runtime()?;
        let run_id = required_field(args.run_id, "run_id", "cancel")?;
        if self.thread_run_snapshot(run_id.as_str()).await?.is_none() {
            return Ok(not_found_run_response("cancel"));
        }
        let snapshot = state_db
            .request_workflow_run_cancel(codex_state::WorkflowRunCancelParams {
                run_id,
                reason: normalize_optional_string(args.reason)
                    .unwrap_or_else(|| "model requested cancellation".to_string()),
            })
            .await
            .map_err(|_| respond("failed to cancel workflow run"))?;
        Ok(json!({
            "action": "cancel",
            "run": snapshot.as_ref().map(run_snapshot_json),
        }))
    }

    async fn decide_step(
        &self,
        args: ManageWorkflowArgs,
        decision: codex_state::WorkflowRunStepApprovalDecision,
    ) -> Result<Value, FunctionCallError> {
        let (state_db, _) = self.runtime()?;
        let action = match decision {
            codex_state::WorkflowRunStepApprovalDecision::Approve => "approve_step",
            codex_state::WorkflowRunStepApprovalDecision::Reject => "reject_step",
        };
        let run_id = required_field(args.run_id, "run_id", action)?;
        let step_id = required_field(args.step_id, "step_id", action)?;
        if self.thread_run_snapshot(run_id.as_str()).await?.is_none() {
            return Ok(json!({
                "action": action,
                "run": null,
                "error": "workflow run not found for current thread",
            }));
        }
        let outcome = state_db
            .workflows()
            .set_workflow_run_step_approval(codex_state::WorkflowRunStepApprovalParams {
                run_id,
                step_id,
                decision,
                reason: normalize_optional_string(args.reason),
                actor_id: None,
            })
            .await
            .map_err(|_| respond("failed to record workflow step approval"))?;
        let Some(outcome) = outcome else {
            return Ok(json!({
                "action": action,
                "run": null,
                "error": "workflow run step not found for current thread",
            }));
        };
        let decision_state = match decision {
            codex_state::WorkflowRunStepApprovalDecision::Approve => {
                codex_state::WORKFLOW_STEP_APPROVAL_APPROVED
            }
            codex_state::WorkflowRunStepApprovalDecision::Reject => {
                codex_state::WORKFLOW_STEP_APPROVAL_REJECTED
            }
        };
        let error = (!outcome.gate_present).then_some("workflow step does not require approval");
        Ok(json!({
            "action": action,
            "run": run_snapshot_json(&outcome.snapshot),
            "gatePresent": outcome.gate_present,
            "changed": outcome.changed,
            "decision": decision_state,
            "error": error,
        }))
    }

    async fn thread_run_snapshot(
        &self,
        run_id: &str,
    ) -> Result<Option<codex_state::WorkflowRunSnapshot>, FunctionCallError> {
        let (state_db, thread_id) = self.runtime()?;
        let snapshot = state_db
            .workflows()
            .get_workflow_run_snapshot(run_id)
            .await
            .map_err(|_| respond("failed to read workflow run"))?
            .filter(|snapshot| snapshot.run.source_thread_id == Some(thread_id));
        Ok(snapshot)
    }
}

fn not_found_run_response(action: &str) -> Value {
    json!({
        "action": action,
        "run": null,
        "error": "workflow run not found for current thread",
    })
}

fn parse_list_cursor(cursor: Option<&str>) -> Result<Option<u32>, FunctionCallError> {
    cursor
        .map(|cursor| {
            cursor
                .parse::<u32>()
                .map_err(|_| respond("workflow list cursor is invalid"))
        })
        .transpose()
}

fn required_field(
    value: Option<String>,
    field_name: &str,
    action: &str,
) -> Result<String, FunctionCallError> {
    normalize_optional_string(value).ok_or_else(|| {
        respond(format!(
            "{field_name} is required for manage_workflow action {action}"
        ))
    })
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn workflow_create_error(err: anyhow::Error) -> FunctionCallError {
    if let Some(err) = err.downcast_ref::<codex_workflows::WorkflowSpecError>() {
        return respond(sanitized_workflow_error(err));
    }
    respond("failed to create workflow")
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

fn respond(message: impl Into<String>) -> FunctionCallError {
    FunctionCallError::RespondToModel(message.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use codex_extension_api::ConversationHistory;
    use codex_extension_api::FunctionCallError;
    use codex_extension_api::NoopTurnItemEmitter;
    use codex_extension_api::ToolExecutor;
    use codex_extension_api::ToolPayload;
    use codex_extension_api::ToolSpec;
    use codex_prompts::DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::ResponseInputItem;
    use codex_tools::ToolExposure;
    use codex_utils_output_truncation::TruncationPolicy;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;

    use super::MANAGE_WORKFLOW_TOOL_NAME;
    use super::ManageWorkflowTool;

    #[tokio::test]
    async fn manage_workflow_lifecycle_returns_sanitized_state() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(
            tempdir.path().to_path_buf(),
            "test-provider".to_string(),
        )
        .await
        .expect("state runtime should initialize");
        let thread_id = codex_protocol::ThreadId::new();
        state_db
            .upsert_thread(
                &codex_state::ThreadMetadataBuilder::new(
                    thread_id,
                    state_db.codex_home().join("rollout.jsonl"),
                    chrono::Utc::now(),
                    codex_protocol::protocol::SessionSource::Cli,
                )
                .build("test-provider"),
            )
            .await
            .expect("thread metadata should insert");
        let tool =
            ManageWorkflowTool::new(Arc::new(AtomicBool::new(true)), state_db.clone(), thread_id);

        let create = call_tool(
            &tool,
            json!({
                "action": "create",
                "yaml": DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML,
            }),
        )
        .await;
        assert_eq!(create["action"], "create");
        let workflow_record_id = create["workflow"]["workflowRecordId"]
            .as_str()
            .expect("workflow id")
            .to_string();
        let serialized = create.to_string();
        assert!(!serialized.contains("source_prompt"));

        let start = call_tool(
            &tool,
            json!({
                "action": "start",
                "workflow_record_id": workflow_record_id,
                "idempotency_key": " run-1 ",
            }),
        )
        .await;
        assert_eq!(start["action"], "start");
        assert_eq!(start["run"]["run"]["status"], "pending");
        assert_eq!(
            start["goalPlan"]["nodeCount"],
            start["run"]["run"]["pendingStepCount"]
        );
        let run_id = start["run"]["run"]["runId"]
            .as_str()
            .expect("run id")
            .to_string();
        let serialized = start.to_string();
        assert!(!serialized.contains("definition_json"));
        assert!(!serialized.contains("eventPayload"));
        assert!(!serialized.contains("source_prompt"));

        let pause = call_tool(&tool, json!({ "action": "pause", "run_id": run_id })).await;
        assert_eq!(pause["run"]["run"]["status"], "paused");

        let resume = call_tool(
            &tool,
            json!({
                "action": "resume",
                "run_id": pause["run"]["run"]["runId"].as_str().expect("run id"),
            }),
        )
        .await;
        assert_eq!(resume["run"]["run"]["status"], "waiting");

        let cancel = call_tool(
            &tool,
            json!({
                "action": "cancel",
                "run_id": resume["run"]["run"]["runId"].as_str().expect("run id"),
                "reason": "commands:\\n  - should-not-leak",
            }),
        )
        .await;
        assert_eq!(cancel["run"]["run"]["status"], "cancel_requested");
        assert_eq!(
            cancel["run"]["run"]["statusReason"],
            "user requested workflow cancellation"
        );
        assert!(!cancel.to_string().contains("should-not-leak"));
    }

    #[tokio::test]
    async fn manage_workflow_step_approval_records_gate_decision() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(
            tempdir.path().to_path_buf(),
            "test-provider".to_string(),
        )
        .await
        .expect("state runtime should initialize");
        let thread_id = codex_protocol::ThreadId::new();
        state_db
            .upsert_thread(
                &codex_state::ThreadMetadataBuilder::new(
                    thread_id,
                    state_db.codex_home().join("rollout.jsonl"),
                    chrono::Utc::now(),
                    codex_protocol::protocol::SessionSource::Cli,
                )
                .build("test-provider"),
            )
            .await
            .expect("thread metadata should insert");
        let tool =
            ManageWorkflowTool::new(Arc::new(AtomicBool::new(true)), state_db.clone(), thread_id);

        let create = call_tool(
            &tool,
            json!({
                "action": "create",
                "yaml": DENTAL_LEAD_SAAS_WORKFLOW_EXAMPLE_YAML,
            }),
        )
        .await;
        let workflow_record_id = create["workflow"]["workflowRecordId"]
            .as_str()
            .expect("workflow id")
            .to_string();
        let start = call_tool(
            &tool,
            json!({
                "action": "start",
                "workflow_record_id": workflow_record_id,
            }),
        )
        .await;
        let run_id = start["run"]["run"]["runId"]
            .as_str()
            .expect("run id")
            .to_string();

        // Approving the dental example's gated launch step records an approved
        // decision without leaking the raw reason.
        let approve = call_tool(
            &tool,
            json!({
                "action": "approve_step",
                "run_id": run_id,
                "step_id": "launch_gate",
                "reason": "commands:\\n  - should-not-leak",
            }),
        )
        .await;
        assert_eq!(approve["action"], "approve_step");
        assert_eq!(approve["gatePresent"], true);
        assert_eq!(approve["changed"], true);
        assert_eq!(approve["decision"], "approved");
        assert_eq!(approve["error"], Value::Null);
        assert!(!approve.to_string().contains("should-not-leak"));

        // Re-approving the same gate is idempotent.
        let repeat = call_tool(
            &tool,
            json!({
                "action": "approve_step",
                "run_id": run_id,
                "step_id": "launch_gate",
            }),
        )
        .await;
        assert_eq!(repeat["changed"], false);

        // Unknown steps are reported without leaking other threads' state.
        let missing = call_tool(
            &tool,
            json!({
                "action": "reject_step",
                "run_id": run_id,
                "step_id": "does_not_exist",
            }),
        )
        .await;
        assert_eq!(missing["run"], Value::Null);
        assert_eq!(
            missing["error"],
            "workflow run step not found for current thread"
        );
    }

    #[tokio::test]
    async fn manage_workflow_is_model_only_and_uses_single_action_schema() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(
            tempdir.path().to_path_buf(),
            "test-provider".to_string(),
        )
        .await
        .expect("state runtime should initialize");
        let tool = ManageWorkflowTool::new(
            Arc::new(AtomicBool::new(true)),
            state_db,
            codex_protocol::ThreadId::new(),
        );

        assert_eq!(tool.exposure(), ToolExposure::DirectModelOnly);
        let ToolSpec::Function(spec) = tool.spec() else {
            panic!("workflow manager should be a function tool");
        };
        assert_eq!(spec.name, MANAGE_WORKFLOW_TOOL_NAME);
        assert!(!spec.strict);
        assert_eq!(spec.parameters.required, Some(vec!["action".to_string()]));
        assert_eq!(spec.parameters.additional_properties, Some(false.into()));
    }

    #[tokio::test]
    async fn unavailable_manage_workflow_reports_model_facing_reason() {
        let tool = ManageWorkflowTool::unavailable(
            Arc::new(AtomicBool::new(true)),
            "workflow management requires a saved thread",
        );
        let payload = ToolPayload::Function {
            arguments: json!({ "action": "list" }).to_string(),
        };

        let result = tool
            .handle(codex_tools::ToolCall {
                turn_id: "turn".to_string(),
                call_id: "call-workflow".to_string(),
                tool_name: codex_tools::ToolName::plain(MANAGE_WORKFLOW_TOOL_NAME),
                model: "test-model".to_string(),
                truncation_policy: TruncationPolicy::Bytes(1024 * 64),
                conversation_history: ConversationHistory::default(),
                turn_item_emitter: Arc::new(NoopTurnItemEmitter),
                payload,
            })
            .await;

        match result {
            Err(err) => assert_eq!(
                err,
                FunctionCallError::RespondToModel(
                    "workflow management requires a saved thread".to_string()
                )
            ),
            Ok(_) => panic!("unavailable workflow manager should return a model-facing error"),
        }
    }

    async fn call_tool(tool: &ManageWorkflowTool, args: Value) -> Value {
        let payload = ToolPayload::Function {
            arguments: args.to_string(),
        };
        let output = tool
            .handle(codex_tools::ToolCall {
                turn_id: "turn".to_string(),
                call_id: "call-workflow".to_string(),
                tool_name: codex_tools::ToolName::plain(MANAGE_WORKFLOW_TOOL_NAME),
                model: "test-model".to_string(),
                truncation_policy: TruncationPolicy::Bytes(1024 * 64),
                conversation_history: ConversationHistory::default(),
                turn_item_emitter: Arc::new(NoopTurnItemEmitter),
                payload: payload.clone(),
            })
            .await
            .expect("workflow manager should return output");
        let ResponseInputItem::FunctionCallOutput { output, .. } =
            output.to_response_item("call-workflow", &payload)
        else {
            panic!("expected function call output");
        };
        let FunctionCallOutputBody::Text(text) = output.body else {
            panic!("expected text output");
        };
        serde_json::from_str(&text).expect("output should be json")
    }
}
