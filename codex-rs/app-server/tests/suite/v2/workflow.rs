use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::create_fake_rollout;
use app_test_support::create_fake_rollout_with_cwd;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use app_test_support::write_mock_provider_models_cache;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadWorkflowCreateResponse;
use codex_app_server_protocol::ThreadWorkflowDeleteResponse;
use codex_app_server_protocol::ThreadWorkflowGetResponse;
use codex_app_server_protocol::ThreadWorkflowListResponse;
use codex_app_server_protocol::ThreadWorkflowRunCancelResponse;
use codex_app_server_protocol::ThreadWorkflowRunGetResponse;
use codex_app_server_protocol::ThreadWorkflowRunListResponse;
use codex_app_server_protocol::ThreadWorkflowRunPauseResponse;
use codex_app_server_protocol::ThreadWorkflowRunResumeResponse;
use codex_app_server_protocol::ThreadWorkflowRunStartResponse;
use codex_app_server_protocol::ThreadWorkflowRunStatus;
use codex_app_server_protocol::ThreadWorkflowStatus;
use codex_protocol::ThreadId;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const ACTIVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
const RAW_SENTINEL: &str = "RAW_WORKFLOW_SECRET_SHOULD_NOT_LEAK";
const COMMAND_SENTINEL: &str = "WORKFLOW_COMMAND_SHOULD_NOT_RUN_OR_LEAK";

#[derive(Debug, Clone, Copy)]
enum WorkflowsFeature {
    Enabled,
    Disabled,
}

impl WorkflowsFeature {
    fn as_toml(self) -> &'static str {
        match self {
            Self::Enabled => "true",
            Self::Disabled => "false",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ExperimentalApiCapability {
    Enabled,
    Disabled,
}

impl ExperimentalApiCapability {
    fn enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[tokio::test]
async fn workflow_create_requires_experimental_api_capability() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let thread_id = create_materialized_thread(codex_home.path(), "workflow experimental gate")?;
    let marker = codex_home.path().join("experimental-gate-command-ran");

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Disabled).await?;

    let request_id = send_workflow_create(
        &mut mcp,
        thread_id.as_str(),
        &invalid_fenced_yaml(RAW_SENTINEL, &marker),
    )
    .await?;
    let error = read_error(&mut mcp, request_id).await?;
    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "thread/workflow/create requires experimentalApi capability"
    );
    assert_does_not_leak(&serde_json::to_string(&error)?)?;
    assert!(!marker.exists());
    assert_no_thread_workflows(codex_home.path(), thread_id.as_str()).await?;

    Ok(())
}

#[tokio::test]
async fn workflow_create_rejects_disabled_feature_before_parsing() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Disabled)?;
    let thread_id = create_materialized_thread(codex_home.path(), "workflow feature gate")?;
    let marker = codex_home.path().join("feature-gate-command-ran");

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let request_id = send_workflow_create(
        &mut mcp,
        thread_id.as_str(),
        &invalid_fenced_yaml(RAW_SENTINEL, &marker),
    )
    .await?;
    let error = read_error(&mut mcp, request_id).await?;
    assert_eq!(error.error.code, -32600);
    assert_eq!(error.error.message, "workflows feature is disabled");
    assert_does_not_leak(&serde_json::to_string(&error)?)?;
    assert!(!marker.exists());
    assert_no_thread_workflows(codex_home.path(), thread_id.as_str()).await?;

    Ok(())
}

#[tokio::test]
async fn workflow_create_rejects_ephemeral_thread_before_parsing() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let marker = codex_home.path().join("ephemeral-gate-command-ran");

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ephemeral: Some(true),
            ..Default::default()
        })
        .await?;
    let start_resp = read_response(&mut mcp, start_id).await?;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;

    let request_id = send_workflow_create(
        &mut mcp,
        thread.id.as_str(),
        &invalid_fenced_yaml(RAW_SENTINEL, &marker),
    )
    .await?;
    let error = read_error(&mut mcp, request_id).await?;
    assert_eq!(error.error.code, -32600);
    assert!(
        error
            .error
            .message
            .contains("ephemeral thread does not support workflows"),
        "unexpected workflow/create error: {}",
        error.error.message
    );
    assert_does_not_leak(&serde_json::to_string(&error)?)?;
    assert!(!marker.exists());

    Ok(())
}

#[tokio::test]
async fn workflow_create_get_and_list_return_sanitized_metadata_without_side_effects() -> Result<()>
{
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let thread_id = create_materialized_thread(codex_home.path(), "workflow metadata")?;
    let marker = codex_home.path().join("workflow-verifier-command-ran");
    let yaml = valid_workflow_yaml(&marker, "wf_app_server_metadata_smoke");

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let request_id = send_workflow_create(&mut mcp, thread_id.as_str(), yaml.as_str()).await?;
    let create_resp = read_response(&mut mcp, request_id).await?;
    let create_json = serde_json::to_string(&create_resp.result)?;
    assert_does_not_leak(&create_json)?;
    let ThreadWorkflowCreateResponse { workflow } =
        to_response::<ThreadWorkflowCreateResponse>(create_resp)?;

    assert_eq!(workflow.thread_id, thread_id);
    assert_eq!(workflow.spec_workflow_id, "wf_app_server_metadata_smoke");
    assert_eq!(workflow.schema_version, "workflow.codex.codewith/v0");
    assert_eq!(workflow.display_name, "Workflow Metadata Smoke");
    assert_eq!(workflow.status, ThreadWorkflowStatus::Draft);
    assert_eq!(workflow.source_yaml_sha256.len(), 64);
    assert_eq!(workflow.agent_count, 3);
    assert_eq!(workflow.step_count, 3);
    assert_eq!(workflow.parallel_group_count, 1);
    assert_eq!(workflow.verifier_count, 3);
    assert_eq!(workflow.run_command_verifier_count, 1);
    assert_eq!(workflow.model_routed_step_count, 3);
    assert!(!marker.exists());

    let get_id = mcp
        .send_raw_request(
            "thread/workflow/get",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": workflow.workflow_record_id.as_str(),
            })),
        )
        .await?;
    let get_resp = read_response(&mut mcp, get_id).await?;
    let get_json = serde_json::to_string(&get_resp.result)?;
    assert_does_not_leak(&get_json)?;
    let ThreadWorkflowGetResponse { workflow: loaded } =
        to_response::<ThreadWorkflowGetResponse>(get_resp)?;
    assert_eq!(Some(workflow.clone()), loaded);

    let list_id = mcp
        .send_raw_request(
            "thread/workflow/list",
            Some(json!({
                "threadId": thread_id.as_str(),
                "limit": 1,
            })),
        )
        .await?;
    let list_resp = read_response(&mut mcp, list_id).await?;
    let list_json = serde_json::to_string(&list_resp.result)?;
    assert_does_not_leak(&list_json)?;
    let list = to_response::<ThreadWorkflowListResponse>(list_resp)?;
    assert_eq!(vec![workflow.clone()], list.data);
    assert_eq!(None, list.next_cursor);

    let bad_cursor_id = mcp
        .send_raw_request(
            "thread/workflow/list",
            Some(json!({
                "threadId": workflow.thread_id.as_str(),
                "cursor": "not-a-cursor",
            })),
        )
        .await?;
    let bad_cursor = read_error(&mut mcp, bad_cursor_id).await?;
    assert_eq!(bad_cursor.error.code, -32600);
    assert_eq!(bad_cursor.error.message, "workflow list cursor is invalid");

    assert_no_execution_side_effects(codex_home.path(), parse_thread_id(thread_id.as_str())?)
        .await?;

    Ok(())
}

#[tokio::test]
async fn workflow_delete_removes_spec_and_reports_missing() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let thread_id = create_materialized_thread(codex_home.path(), "workflow delete")?;
    let marker = codex_home.path().join("workflow-delete-command-ran");
    let yaml = valid_workflow_yaml(&marker, "wf_app_server_delete_smoke");

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let create_id = send_workflow_create(&mut mcp, thread_id.as_str(), yaml.as_str()).await?;
    let create_resp = read_response(&mut mcp, create_id).await?;
    let ThreadWorkflowCreateResponse { workflow } =
        to_response::<ThreadWorkflowCreateResponse>(create_resp)?;

    // Deleting an unknown record is not an error; it simply reports deleted=false.
    let missing_id = mcp
        .send_raw_request(
            "thread/workflow/delete",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": "workflow_does_not_exist",
            })),
        )
        .await?;
    let missing_resp = read_response(&mut mcp, missing_id).await?;
    assert_does_not_leak(&serde_json::to_string(&missing_resp.result)?)?;
    let missing = to_response::<ThreadWorkflowDeleteResponse>(missing_resp)?;
    assert!(!missing.deleted);

    // Deleting the saved spec succeeds and does not leak raw YAML/prompt content.
    let delete_id = mcp
        .send_raw_request(
            "thread/workflow/delete",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": workflow.workflow_record_id.as_str(),
            })),
        )
        .await?;
    let delete_resp = read_response(&mut mcp, delete_id).await?;
    assert_does_not_leak(&serde_json::to_string(&delete_resp.result)?)?;
    let deleted = to_response::<ThreadWorkflowDeleteResponse>(delete_resp)?;
    assert!(deleted.deleted);

    // A follow-up get returns None now that the spec is gone.
    let get_id = mcp
        .send_raw_request(
            "thread/workflow/get",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": workflow.workflow_record_id.as_str(),
            })),
        )
        .await?;
    let get_resp = read_response(&mut mcp, get_id).await?;
    let ThreadWorkflowGetResponse { workflow: loaded } =
        to_response::<ThreadWorkflowGetResponse>(get_resp)?;
    assert_eq!(None, loaded);

    // Deleting again reports deleted=false rather than erroring.
    let repeat_id = mcp
        .send_raw_request(
            "thread/workflow/delete",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": workflow.workflow_record_id.as_str(),
            })),
        )
        .await?;
    let repeat_resp = read_response(&mut mcp, repeat_id).await?;
    let repeat = to_response::<ThreadWorkflowDeleteResponse>(repeat_resp)?;
    assert!(!repeat.deleted);
    assert!(!marker.exists());

    Ok(())
}

#[tokio::test]
async fn workflow_run_lifecycle_projects_tasks_and_returns_sanitized_state() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let thread_id = create_materialized_thread(codex_home.path(), "workflow run lifecycle")?;
    let marker = codex_home.path().join("workflow-run-command-ran");
    let yaml = valid_workflow_yaml(&marker, "wf_app_server_run_lifecycle");

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let create_id = send_workflow_create(&mut mcp, thread_id.as_str(), yaml.as_str()).await?;
    let create_resp = read_response(&mut mcp, create_id).await?;
    let ThreadWorkflowCreateResponse { workflow } =
        to_response::<ThreadWorkflowCreateResponse>(create_resp)?;

    let start_id = mcp
        .send_raw_request(
            "thread/workflow/run/start",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": workflow.workflow_record_id.as_str(),
                "idempotencyKey": "run-lifecycle",
            })),
        )
        .await?;
    let start_resp = read_response(&mut mcp, start_id).await?;
    let start_json = serde_json::to_string(&start_resp.result)?;
    assert_does_not_leak(&start_json)?;
    let started = to_response::<ThreadWorkflowRunStartResponse>(start_resp)?;
    assert_eq!(ThreadWorkflowRunStatus::Pending, started.run.run.status);
    assert_eq!(
        workflow.workflow_record_id,
        started.run.run.workflow_record_id
    );
    assert_eq!(3, started.run.steps.len());
    assert_eq!(3, started.run.verifiers.len());
    assert_eq!(
        Some(3),
        started.goal_plan.as_ref().map(|plan| plan.nodes.len())
    );
    assert!(!marker.exists());
    assert_no_execution_side_effects(codex_home.path(), parse_thread_id(thread_id.as_str())?)
        .await?;

    let list_id = mcp
        .send_raw_request(
            "thread/workflow/run/list",
            Some(json!({
                "threadId": thread_id.as_str(),
                "limit": 5,
            })),
        )
        .await?;
    let list_resp = read_response(&mut mcp, list_id).await?;
    assert_does_not_leak(&serde_json::to_string(&list_resp.result)?)?;
    let list = to_response::<ThreadWorkflowRunListResponse>(list_resp)?;
    assert_eq!(1, list.data.len());
    assert_eq!(started.run.run.run_id, list.data[0].run_id);
    assert!(
        matches!(
            list.data[0].status,
            ThreadWorkflowRunStatus::Pending
                | ThreadWorkflowRunStatus::Running
                | ThreadWorkflowRunStatus::Waiting
        ),
        "activation should keep the workflow in a live nonterminal state before pause"
    );

    let get_id = mcp
        .send_raw_request(
            "thread/workflow/run/get",
            Some(json!({
                "threadId": thread_id.as_str(),
                "runId": started.run.run.run_id.as_str(),
            })),
        )
        .await?;
    let get_resp = read_response(&mut mcp, get_id).await?;
    assert_does_not_leak(&serde_json::to_string(&get_resp.result)?)?;
    let get = to_response::<ThreadWorkflowRunGetResponse>(get_resp)?;
    assert_eq!(
        Some(started.run.run.run_id.clone()),
        get.run.map(|run| run.run.run_id)
    );

    let pause_id = mcp
        .send_raw_request(
            "thread/workflow/run/pause",
            Some(json!({
                "threadId": thread_id.as_str(),
                "runId": started.run.run.run_id.as_str(),
                "reason": RAW_SENTINEL,
            })),
        )
        .await?;
    let pause_resp = read_response(&mut mcp, pause_id).await?;
    assert_does_not_leak(&serde_json::to_string(&pause_resp.result)?)?;
    let paused = to_response::<ThreadWorkflowRunPauseResponse>(pause_resp)?;
    assert_eq!(
        Some(ThreadWorkflowRunStatus::Paused),
        paused.run.as_ref().map(|run| run.run.status)
    );

    let resume_id = mcp
        .send_raw_request(
            "thread/workflow/run/resume",
            Some(json!({
                "threadId": thread_id.as_str(),
                "runId": started.run.run.run_id.as_str(),
            })),
        )
        .await?;
    let resume_resp = read_response(&mut mcp, resume_id).await?;
    assert_does_not_leak(&serde_json::to_string(&resume_resp.result)?)?;
    let resumed = to_response::<ThreadWorkflowRunResumeResponse>(resume_resp)?;
    assert_eq!(
        Some(ThreadWorkflowRunStatus::Waiting),
        resumed.run.as_ref().map(|run| run.run.status)
    );

    let cancel_id = mcp
        .send_raw_request(
            "thread/workflow/run/cancel",
            Some(json!({
                "threadId": thread_id.as_str(),
                "runId": started.run.run.run_id.as_str(),
                "reason": RAW_SENTINEL,
            })),
        )
        .await?;
    let cancel_resp = read_response(&mut mcp, cancel_id).await?;
    assert_does_not_leak(&serde_json::to_string(&cancel_resp.result)?)?;
    let cancelled = to_response::<ThreadWorkflowRunCancelResponse>(cancel_resp)?;
    assert_eq!(
        Some(ThreadWorkflowRunStatus::CancelRequested),
        cancelled.run.as_ref().map(|run| run.run.status)
    );
    assert!(!marker.exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_start_activates_one_real_worker_and_one_verifier() -> Result<()> {
    let codex_home = TempDir::new()?;
    let source_repo = TempDir::new()?;
    init_git_repo(source_repo.path())?;
    let source_head = git_output(source_repo.path(), &["rev-parse", "HEAD"])?;
    let source_status = git_output(source_repo.path(), &["status", "--short"])?;
    let server =
        create_mock_responses_server_sequence(vec![create_final_assistant_message_sse_response(
            "workflow worker done",
        )?])
        .await;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let thread_id = create_materialized_thread_with_cwd(
        codex_home.path(),
        "workflow actual worker",
        source_repo.path(),
    )?;
    let yaml = actual_worker_workflow_yaml();
    codex_workflows::parse_workflow_yaml(&yaml)?;

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let create_id = send_workflow_create(&mut mcp, thread_id.as_str(), yaml.as_str()).await?;
    let create_resp = read_response(&mut mcp, create_id).await?;
    let ThreadWorkflowCreateResponse { workflow } =
        to_response::<ThreadWorkflowCreateResponse>(create_resp)?;
    let start_id = mcp
        .send_raw_request(
            "thread/workflow/run/start",
            Some(json!({
                "threadId": thread_id.as_str(),
                "workflowRecordId": workflow.workflow_record_id.as_str(),
                "idempotencyKey": "actual-worker-activation",
            })),
        )
        .await?;
    let start_resp = read_response(&mut mcp, start_id).await?;
    let started = to_response::<ThreadWorkflowRunStartResponse>(start_resp)?;
    let run_id = started.run.run.run_id;
    let runtime = open_state_runtime(codex_home.path()).await?;
    let snapshot = timeout(ACTIVATION_TIMEOUT, async {
        loop {
            let snapshot = runtime
                .workflows()
                .get_workflow_run_snapshot(run_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("workflow run {run_id} disappeared"))?;
            if snapshot.run.status.is_terminal() {
                return Result::<_, anyhow::Error>::Ok(snapshot);
            }
            sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await??;

    assert_eq!(codex_state::WorkflowRunStatus::Completed, snapshot.run.status);
    assert_eq!(1, snapshot.steps.len());
    assert_eq!(
        codex_state::WorkflowRunStepStatus::Succeeded,
        snapshot.steps[0].status
    );
    assert_eq!(1, snapshot.verifiers.len());
    assert_eq!(
        codex_state::WorkflowRunStepVerifierStatus::Passed,
        snapshot.verifiers[0].status
    );
    for event_type in ["branch_admitted", "verifier_started", "verifier_passed"] {
        assert_eq!(
            1,
            snapshot
                .events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            "workflow should record exactly one {event_type} event"
        );
    }

    let step = &snapshot.steps[0];
    let agent_run_id = step
        .background_agent_run_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("workflow step has no background AgentRun"))?;
    let managed_worktree_id = step
        .branch_admission_json
        .as_ref()
        .and_then(|value| value.pointer("/data/managedWorktreeId"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("workflow branch admission has no managedWorktreeId"))?;
    let agent_run = runtime
        .get_background_agent_run(agent_run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workflow AgentRun {agent_run_id} was not persisted"))?;
    assert_eq!(
        codex_state::BackgroundAgentRunStatus::Completed,
        agent_run.status
    );
    assert_eq!(
        Some(managed_worktree_id),
        agent_run.worktree_lease_id.as_deref()
    );
    assert_eq!(
        Some(step.agent_id.as_str()),
        agent_run
            .spawn_linkage_json
            .as_ref()
            .and_then(|value| value.get("agentId"))
            .and_then(serde_json::Value::as_str)
    );
    assert_eq!(1, runtime.list_background_agent_runs(Some(10)).await?.len());

    let managed_worktree = runtime
        .managed_worktrees()
        .get_managed_worktree(managed_worktree_id)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!("workflow managed worktree {managed_worktree_id} was not persisted")
        })?;
    assert_eq!(
        codex_state::ManagedWorktreeMode::IsolatedWorktree,
        managed_worktree.mode
    );
    assert_eq!(
        std::fs::canonicalize(source_repo.path())?,
        std::fs::canonicalize(&managed_worktree.base_repo_path)?
    );
    assert_ne!(
        std::fs::canonicalize(source_repo.path())?,
        std::fs::canonicalize(&managed_worktree.worktree_path)?
    );
    assert_eq!(
        Some(agent_run_id),
        managed_worktree.owner_agent_run_id.as_deref()
    );

    assert_eq!(
        source_head,
        git_output(source_repo.path(), &["rev-parse", "HEAD"])?
    );
    assert_eq!(
        source_status,
        git_output(source_repo.path(), &["status", "--short"])?
    );

    Ok(())
}

#[tokio::test]
async fn workflow_create_returns_sanitized_validation_error() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), WorkflowsFeature::Enabled)?;
    let thread_id = create_materialized_thread(codex_home.path(), "workflow validation")?;
    let marker = codex_home.path().join("invalid-workflow-command-ran");
    let yaml = valid_workflow_yaml(&marker, "wf_app_server_invalid")
        .replace("workflow.codex.codewith/v0", RAW_SENTINEL);

    let mut mcp = TestAppServer::new_without_managed_config(codex_home.path()).await?;
    initialize(&mut mcp, ExperimentalApiCapability::Enabled).await?;

    let request_id = send_workflow_create(&mut mcp, thread_id.as_str(), yaml.as_str()).await?;
    let error = read_error(&mut mcp, request_id).await?;
    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "workflow YAML does not satisfy the workflow spec invariants"
    );
    assert_does_not_leak(&serde_json::to_string(&error)?)?;
    assert!(!marker.exists());
    assert_no_thread_workflows(codex_home.path(), thread_id.as_str()).await?;

    Ok(())
}

async fn initialize(
    mcp: &mut TestAppServer,
    experimental_api: ExperimentalApiCapability,
) -> Result<()> {
    let init = timeout(
        DEFAULT_TIMEOUT,
        mcp.initialize_with_capabilities(
            default_client_info(),
            Some(InitializeCapabilities {
                experimental_api: experimental_api.enabled(),
                request_attestation: false,
                opt_out_notification_methods: None,
            }),
        ),
    )
    .await??;
    let JSONRPCMessage::Response(_) = init else {
        anyhow::bail!("expected initialize response, got {init:?}");
    };
    Ok(())
}

async fn send_workflow_create(mcp: &mut TestAppServer, thread_id: &str, yaml: &str) -> Result<i64> {
    mcp.send_raw_request(
        "thread/workflow/create",
        Some(json!({
            "threadId": thread_id,
            "yaml": yaml,
        })),
    )
    .await
}

async fn read_response(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCResponse> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await?
}

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCError> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}

async fn assert_no_thread_workflows(codex_home: &Path, thread_id: &str) -> Result<()> {
    let runtime = open_state_runtime(codex_home).await?;
    let page = runtime
        .workflows()
        .list_thread_workflow_specs_page(
            parse_thread_id(thread_id)?,
            /*cursor*/ None,
            codex_state::DEFAULT_THREAD_WORKFLOW_LIST_LIMIT,
        )
        .await?;
    assert_eq!(Vec::<codex_state::WorkflowSpecRecord>::new(), page.data);
    assert_eq!(None, page.next_cursor);
    Ok(())
}

async fn assert_no_execution_side_effects(codex_home: &Path, thread_id: ThreadId) -> Result<()> {
    let runtime = open_state_runtime(codex_home).await?;
    assert_eq!(
        None,
        runtime.thread_goals().get_thread_goal(thread_id).await?
    );
    assert_eq!(
        Vec::<codex_state::ThreadSchedule>::new(),
        runtime
            .thread_schedules()
            .list_thread_schedules(thread_id)
            .await?
    );
    assert_eq!(
        Vec::<codex_state::ThreadMonitor>::new(),
        runtime
            .thread_monitors()
            .list_thread_monitors(thread_id)
            .await?
    );
    assert_eq!(
        Vec::<codex_state::BackgroundAgentRun>::new(),
        runtime.list_background_agent_runs(Some(10)).await?
    );
    assert_eq!(
        Vec::<codex_state::ManagedWorktree>::new(),
        runtime
            .managed_worktrees()
            .list_managed_worktrees_page(
                /*base_repo_path*/ None, /*include_deleted*/ true, /*cursor*/ None,
                /*limit*/ 10,
            )
            .await?
            .data
    );
    Ok(())
}

async fn open_state_runtime(codex_home: &Path) -> Result<std::sync::Arc<StateRuntime>> {
    StateRuntime::init(codex_home.to_path_buf(), "mock_provider".to_string()).await
}

fn parse_thread_id(thread_id: &str) -> Result<ThreadId> {
    ThreadId::from_string(thread_id).map_err(|err| anyhow::anyhow!("invalid test thread id: {err}"))
}

fn create_materialized_thread(codex_home: &Path, preview: &str) -> Result<String> {
    create_fake_rollout(
        codex_home,
        "2026-01-02T03-04-05",
        "2026-01-02T03:04:05Z",
        preview,
        Some("mock_provider"),
        /*git_info*/ None,
    )
}

fn create_materialized_thread_with_cwd(
    codex_home: &Path,
    preview: &str,
    cwd: &Path,
) -> Result<String> {
    create_fake_rollout_with_cwd(
        codex_home,
        "2026-01-02T03-04-05",
        "2026-01-02T03:04:05Z",
        preview,
        Some("mock_provider"),
        /*git_info*/ None,
        cwd,
    )
}

fn create_config_toml(
    codex_home: &Path,
    server_uri: &str,
    workflows_feature: WorkflowsFeature,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"
suppress_unstable_features_warning = true

[features]
workflows = {}

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#,
            workflows_feature.as_toml()
        ),
    )?;
    write_mock_provider_models_cache(codex_home)
}

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: DEFAULT_CLIENT_NAME.to_string(),
        title: None,
        version: "0.1.0".to_string(),
    }
}

fn assert_does_not_leak(serialized: &str) -> Result<()> {
    for forbidden in [
        RAW_SENTINEL,
        COMMAND_SENTINEL,
        "sourcePrompt",
        "source_prompt",
        "\"yaml\"",
        "\"sourceYaml\"",
        "touch ",
    ] {
        if serialized.contains(forbidden) {
            anyhow::bail!("workflow API response leaked forbidden content `{forbidden}`");
        }
    }
    Ok(())
}

fn invalid_fenced_yaml(raw_sentinel: &str, marker: &Path) -> String {
    format!(
        "```yaml\nsource_prompt: \"{raw_sentinel}\"\ncommands:\n  - {}\n```",
        yaml_single_quoted(&format!("touch {} # {COMMAND_SENTINEL}", marker.display()))
    )
}

fn actual_worker_workflow_yaml() -> String {
    r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "wf_app_server_actual_worker"
display_name: "Actual Worker Activation"
source_prompt: "Run one real adversarial worker and verify its isolated checkout."
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "mock_provider"
  model: "mock-model"
  reasoning: "high"
  approval_policy: "never"
  permission_profile: "read-only"
limits:
  max_parallel_steps: 1
  max_agents: 1
  max_worktrees: 1
  max_runtime_seconds: 120
  max_step_runtime_seconds: 60
  max_tokens: 10000
  max_tool_calls: 20
approvals:
  required_before: []
agents:
  - id: "adversarial_worker"
    display_name: "Adversary-Hypatia"
    role: "Adversarially verify actual workflow worker activation."
    model:
      model_gateway: "hasna"
      provider: "mock_provider"
      model: "mock-model"
      reasoning: "high"
      approval_policy: "never"
      permission_profile: "read-only"
steps:
  - id: "adversarial_actual_worker"
    title: "Run the actual adversarial worker"
    agent: "adversarial_worker"
    model:
      model_gateway: "hasna"
      provider: "mock_provider"
      model: "mock-model"
      reasoning: "high"
      approval_policy: "never"
      permission_profile: "read-only"
    workspace:
      mode: "isolated_worktree"
    depends_on: []
    outputs: []
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "worktree_git_status"
          type: "run_commands"
          cwd: "."
          sandbox: "read-only"
          network: "disabled"
          timeout_seconds: 5
          output_limit_bytes: 1024
          commands:
            - "git status --short"
          expected_exit_code: 0
artifacts:
  retention: "preserve_evidence"
  required: []
cleanup:
  on_cancel: []
  on_complete: []
"#
    .to_string()
}

fn valid_workflow_yaml(marker: &Path, workflow_id: &str) -> String {
    let command = yaml_single_quoted(&format!("touch {} # {COMMAND_SENTINEL}", marker.display()));
    format!(
        r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "{workflow_id}"
display_name: "Workflow Metadata Smoke"
source_prompt: "Build a serious workflow without leaking {RAW_SENTINEL}"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 2
  max_agents: 3
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 1200
  max_tokens: 100000
  max_tool_calls: 100
approvals:
  required_before: []
agents:
  - id: "architect"
    display_name: "Architect-Archimedes"
    role: "Own the architecture and implementation map."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "adversarial_security"
    display_name: "Adversary-Hypatia"
    role: "Adversarially attack the security and leakage assumptions."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "adversarial_testing"
    display_name: "Adversary-Euclid"
    role: "Adversarially attack the deterministic test evidence."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "design"
    title: "Design the workflow architecture"
    agent: "architect"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on: []
    outputs:
      - "design.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "design_artifact"
          type: "artifact_contains"
          artifact: "design.md"
          must_contain:
            - "architecture"
            - "tests"
  - id: "adversarial_security_review"
    title: "Adversarial security review"
    agent: "adversarial_security"
    parallel_group: "adversarial_review"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on:
      - "design"
    outputs:
      - "security-review.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "security_artifact"
          type: "artifact_contains"
          artifact: "security-review.md"
          must_contain:
            - "leak"
            - "feature gate"
  - id: "adversarial_testing_review"
    title: "Adversarial deterministic testing review"
    agent: "adversarial_testing"
    parallel_group: "adversarial_review"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on:
      - "design"
    outputs:
      - "testing-review.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "command_check"
          type: "run_commands"
          cwd: "."
          sandbox: "read-only"
          network: "disabled"
          timeout_seconds: 1
          output_limit_bytes: 1024
          commands:
            - {command}
artifacts:
  retention: "preserve_evidence"
  required: []
cleanup:
  on_cancel: []
  on_complete: []
"#
    )
}

fn yaml_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn init_git_repo(repo_path: &Path) -> Result<()> {
    git(repo_path, &["init"])?;
    git(repo_path, &["config", "core.autocrlf", "false"])?;
    git(repo_path, &["config", "user.email", "codewith@example.com"])?;
    git(repo_path, &["config", "user.name", "Codewith Test"])?;
    std::fs::write(repo_path.join("README.md"), "workflow worktree test\n")?;
    git(repo_path, &["add", "README.md"])?;
    git(repo_path, &["commit", "-m", "initial"])?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    git_output(cwd, args).map(|_| ())
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
