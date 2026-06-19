#![allow(dead_code)]

// Staged executor entry point: workflow scheduling wires this module in a later slice,
// while this slice keeps the verifier policy and state-recording behavior test-covered.

use std::path::Component;
use std::path::Path;
use std::sync::Arc;

use codex_protocol::error::CodexErr;
use codex_protocol::error::SandboxErr;
use codex_protocol::exec_output::ExecToolCallOutput;
use codex_protocol::models::SandboxPermissions;
use codex_protocol::permissions::FileSystemSandboxKind;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_state::StateRuntime;
use codex_state::WorkflowRunVerifierClaimParams;
use codex_state::WorkflowRunVerifierClaimSelection;
use codex_state::WorkflowRunVerifierOutcomeStatus;
use codex_state::WorkflowRunVerifierRecordResultOutcome;
use codex_state::WorkflowRunVerifierRecordResultParams;
use codex_state::WorkflowRunVerifierResultSummary;
use codex_tools::ToolName;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::exec_env::create_env;
use crate::exec_policy::ExecApprovalRequest;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::runtimes::shell::ShellRequest;
use crate::tools::runtimes::shell::ShellRuntime;
use crate::tools::runtimes::shell::ShellRuntimeBackend;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;

const WORKFLOW_VERIFIER_TOOL_NAME: &str = "workflow_verifier";
const DEFAULT_ALLOWED_COMMANDS: &[&str] = &[
    "bun", "cargo", "deno", "false", "git", "go", "just", "node", "npm", "pnpm", "python",
    "python3", "rg", "ruff", "test", "true", "tsc", "uv", "yarn",
];
const MAX_VERIFIER_COMMANDS: usize = 8;
const MAX_VERIFIER_TIMEOUT_SECONDS: u64 = 30 * 60;
const MAX_VERIFIER_TOTAL_TIMEOUT_SECONDS: u64 = 30 * 60;
const MAX_VERIFIER_OUTPUT_LIMIT_BYTES: u64 = 1024 * 1024;
const MAX_VERIFIER_RETRY_ATTEMPTS: u32 = 5;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkflowStateEnvelope {
    schema_version: String,
    redaction_version: u32,
    kind: String,
    data: RunCommandsVerifierDefinition,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCommandsVerifierDefinition {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    artifact: Option<String>,
    #[serde(default)]
    must_contain: Vec<String>,
    cwd: Option<String>,
    sandbox: Option<String>,
    network: Option<String>,
    timeout_seconds: Option<u64>,
    output_limit_bytes: Option<u64>,
    #[serde(default)]
    commands: Vec<String>,
    expected_stdout: Option<String>,
    expected_exit_code: Option<i32>,
    retry_policy: Option<WorkflowVerifierRetryPolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowVerifierRetryPolicy {
    max_attempts: u32,
}

pub(crate) async fn execute_next_workflow_run_verifier(
    state: &StateRuntime,
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    run_id: String,
    owner_id: String,
    generation: i64,
) -> anyhow::Result<Option<WorkflowRunVerifierRecordResultOutcome>> {
    let Some(claim) = state
        .claim_workflow_run_verifier(WorkflowRunVerifierClaimParams {
            run_id: run_id.clone(),
            owner_id: owner_id.clone(),
            generation,
            selection: WorkflowRunVerifierClaimSelection::NextRunCommands,
        })
        .await?
    else {
        return Ok(None);
    };

    let definition = parse_run_commands_definition(&claim.verifier.definition_json);
    let result = match definition {
        Ok(definition) => execute_run_commands_definition(session, turn, &definition)
            .await
            .unwrap_or_else(|summary| summary),
        Err(_) => VerifierExecutionSummary::policy_failure(/*command_count*/ 0),
    };

    state
        .record_workflow_run_verifier_result(WorkflowRunVerifierRecordResultParams {
            run_id,
            owner_id,
            generation,
            verifier_run_id: claim.verifier.verifier_run_id,
            outcome: result.outcome,
            summary: result.summary,
        })
        .await
}

async fn execute_run_commands_definition(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    definition: &RunCommandsVerifierDefinition,
) -> Result<VerifierExecutionSummary, VerifierExecutionSummary> {
    validate_run_commands_definition(definition, turn.as_ref())?;
    let command_count = i64::try_from(definition.commands.len()).unwrap_or(i64::MAX);
    let timeout_ms = definition
        .timeout_seconds
        .unwrap_or_default()
        .saturating_mul(1000);
    let output_limit_bytes = definition.output_limit_bytes.unwrap_or_default();
    let expected_exit_code = definition.expected_exit_code.unwrap_or(0);
    let cwd = match verifier_cwd(&turn, definition.cwd.as_deref()) {
        Ok(cwd) => cwd,
        Err(_) => {
            return Err(VerifierExecutionSummary::failed(
                WorkflowRunVerifierResultSummary {
                    command_count,
                    expected_exit_code: Some(expected_exit_code),
                    observed_exit_code: None,
                    timed_out: false,
                    duration_ms: 0,
                    output_bytes: 0,
                    output_truncated: false,
                },
            ));
        }
    };

    let mut total_duration_ms = 0_i64;
    let mut total_output_bytes = 0_i64;
    let mut output_truncated = false;
    let mut last_exit_code = None;
    for command in &definition.commands {
        let command_argv = parse_verifier_command(command).map_err(|_| {
            VerifierExecutionSummary::failed(WorkflowRunVerifierResultSummary {
                command_count,
                expected_exit_code: Some(expected_exit_code),
                observed_exit_code: None,
                timed_out: false,
                duration_ms: total_duration_ms,
                output_bytes: total_output_bytes,
                output_truncated,
            })
        })?;
        let output = match run_verifier_command(
            Arc::clone(&session),
            Arc::clone(&turn),
            command,
            command_argv,
            cwd.clone(),
            timeout_ms,
        )
        .await
        {
            Ok(output) => output,
            Err(err) => {
                return Err(VerifierExecutionSummary::failed(
                    failed_summary_for_tool_error(
                        &err,
                        command_count,
                        expected_exit_code,
                        total_duration_ms,
                        total_output_bytes,
                        output_truncated,
                    ),
                ));
            }
        };

        let output_bytes = i64::try_from(output.aggregated_output.text.len()).unwrap_or(i64::MAX);
        total_output_bytes = total_output_bytes.saturating_add(output_bytes);
        total_duration_ms = total_duration_ms
            .saturating_add(i64::try_from(output.duration.as_millis()).unwrap_or(i64::MAX));
        output_truncated |= output.aggregated_output.truncated_after_lines.is_some()
            || u64::try_from(output_bytes).unwrap_or(u64::MAX) > output_limit_bytes;
        last_exit_code = Some(output.exit_code);

        let stdout_matches = definition
            .expected_stdout
            .as_ref()
            .is_none_or(|expected| output.aggregated_output.text.contains(expected));
        if output.timed_out
            || output.exit_code != expected_exit_code
            || !stdout_matches
            || output_truncated
        {
            return Err(VerifierExecutionSummary::failed(
                WorkflowRunVerifierResultSummary {
                    command_count,
                    expected_exit_code: Some(expected_exit_code),
                    observed_exit_code: Some(output.exit_code),
                    timed_out: output.timed_out,
                    duration_ms: total_duration_ms,
                    output_bytes: total_output_bytes,
                    output_truncated,
                },
            ));
        }
    }

    Ok(VerifierExecutionSummary {
        outcome: WorkflowRunVerifierOutcomeStatus::Passed,
        summary: WorkflowRunVerifierResultSummary {
            command_count,
            expected_exit_code: Some(expected_exit_code),
            observed_exit_code: last_exit_code,
            timed_out: false,
            duration_ms: total_duration_ms,
            output_bytes: total_output_bytes,
            output_truncated,
        },
    })
}

fn failed_summary_for_tool_error(
    err: &ToolError,
    command_count: i64,
    expected_exit_code: i32,
    total_duration_ms: i64,
    total_output_bytes: i64,
    output_truncated: bool,
) -> WorkflowRunVerifierResultSummary {
    let Some(output) = (match err {
        ToolError::Codex(CodexErr::Sandbox(SandboxErr::Denied { output, .. }))
        | ToolError::Codex(CodexErr::Sandbox(SandboxErr::Timeout { output })) => {
            Some(output.as_ref())
        }
        ToolError::Rejected(_) | ToolError::Codex(_) => None,
    }) else {
        return WorkflowRunVerifierResultSummary {
            command_count,
            expected_exit_code: Some(expected_exit_code),
            observed_exit_code: None,
            timed_out: false,
            duration_ms: total_duration_ms,
            output_bytes: total_output_bytes,
            output_truncated,
        };
    };

    let output_bytes = i64::try_from(output.aggregated_output.text.len()).unwrap_or(i64::MAX);
    WorkflowRunVerifierResultSummary {
        command_count,
        expected_exit_code: Some(expected_exit_code),
        observed_exit_code: Some(output.exit_code),
        timed_out: output.timed_out,
        duration_ms: total_duration_ms
            .saturating_add(i64::try_from(output.duration.as_millis()).unwrap_or(i64::MAX)),
        output_bytes: total_output_bytes.saturating_add(output_bytes),
        output_truncated: output_truncated
            || output.aggregated_output.truncated_after_lines.is_some(),
    }
}

async fn run_verifier_command(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    command: &str,
    command_argv: Vec<String>,
    cwd: AbsolutePathBuf,
    timeout_ms: u64,
) -> Result<ExecToolCallOutput, ToolError> {
    let effective_turn_settings = session.effective_turn_settings(turn.as_ref()).await;
    let exec_approval_requirement = session
        .services
        .exec_policy
        .create_exec_approval_requirement_for_command(ExecApprovalRequest {
            command: &command_argv,
            approval_policy: effective_turn_settings.approval_policy,
            permission_profile: effective_turn_settings.permission_profile.clone(),
            windows_sandbox_level: turn.windows_sandbox_level,
            sandbox_permissions: SandboxPermissions::UseDefault,
            prefix_rule: None,
        })
        .await;
    let req = ShellRequest {
        command: command_argv,
        shell_type: None,
        hook_command: command.to_string(),
        cwd,
        timeout_ms: Some(timeout_ms),
        cancellation_token: CancellationToken::new(),
        env: create_env(&turn.shell_environment_policy, Some(session.thread_id)),
        explicit_env_overrides: turn.shell_environment_policy.r#set.clone(),
        network: turn.network.clone(),
        sandbox_permissions: SandboxPermissions::UseDefault,
        additional_permissions: None,
        #[cfg(unix)]
        additional_permissions_preapproved: false,
        justification: Some("workflow deterministic verifier".to_string()),
        exec_approval_requirement,
    };
    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = ShellRuntime::for_shell_command(ShellRuntimeBackend::WorkflowVerifierDirect);
    let tool_ctx = ToolCtx {
        session: Arc::clone(&session),
        turn: Arc::clone(&turn),
        call_id: format!("workflow-verifier-{}", uuid::Uuid::new_v4()),
        tool_name: ToolName::plain(WORKFLOW_VERIFIER_TOOL_NAME),
    };
    orchestrator
        .run(
            &mut runtime,
            &req,
            &tool_ctx,
            turn.as_ref(),
            effective_turn_settings.approval_policy,
            &effective_turn_settings.permission_profile,
        )
        .await
        .map(|result| result.output)
}

fn parse_run_commands_definition(
    definition_json: &serde_json::Value,
) -> anyhow::Result<RunCommandsVerifierDefinition> {
    let envelope: WorkflowStateEnvelope =
        serde_json::from_value(definition_json.clone()).map_err(|err| {
            anyhow::anyhow!("workflow verifier definition is not a workflow state envelope: {err}")
        })?;
    if envelope.schema_version != "workflow.run_state/v0" {
        anyhow::bail!("workflow verifier definition has unsupported schema version");
    }
    if envelope.redaction_version != 1 {
        anyhow::bail!("workflow verifier definition has unsupported redaction version");
    }
    if envelope.kind != "workflow_run_step_verifier_definition" {
        anyhow::bail!("workflow verifier definition has unsupported envelope kind");
    }
    if envelope.data.kind != "run_commands" {
        anyhow::bail!("workflow verifier is not run_commands");
    }
    Ok(envelope.data)
}

fn validate_run_commands_definition(
    definition: &RunCommandsVerifierDefinition,
    turn: &TurnContext,
) -> Result<(), VerifierExecutionSummary> {
    let command_count = i64::try_from(definition.commands.len()).unwrap_or(i64::MAX);
    let failure = || {
        VerifierExecutionSummary::failed(WorkflowRunVerifierResultSummary {
            command_count,
            expected_exit_code: definition.expected_exit_code,
            observed_exit_code: None,
            timed_out: false,
            duration_ms: 0,
            output_bytes: 0,
            output_truncated: false,
        })
    };
    if definition.commands.is_empty() || definition.commands.len() > MAX_VERIFIER_COMMANDS {
        return Err(failure());
    }
    let timeout_seconds = definition.timeout_seconds.unwrap_or_default();
    if timeout_seconds == 0 || timeout_seconds > MAX_VERIFIER_TIMEOUT_SECONDS {
        return Err(failure());
    }
    let command_count_u64 = u64::try_from(definition.commands.len()).unwrap_or(u64::MAX);
    if timeout_seconds.saturating_mul(command_count_u64) > MAX_VERIFIER_TOTAL_TIMEOUT_SECONDS {
        return Err(failure());
    }
    if definition.output_limit_bytes.unwrap_or_default() == 0
        || definition.output_limit_bytes.unwrap_or_default() > MAX_VERIFIER_OUTPUT_LIMIT_BYTES
    {
        return Err(failure());
    }
    if !verifier_sandbox_is_supported(definition.sandbox.as_deref(), turn) {
        return Err(failure());
    }
    if !verifier_network_is_supported(definition.network.as_deref(), turn) {
        return Err(failure());
    }
    if verifier_cwd(turn, definition.cwd.as_deref()).is_err() {
        return Err(failure());
    }
    if definition.id.trim().is_empty() {
        return Err(failure());
    }
    if let Some(retry_policy) = &definition.retry_policy
        && (retry_policy.max_attempts == 0
            || retry_policy.max_attempts > MAX_VERIFIER_RETRY_ATTEMPTS)
    {
        return Err(failure());
    }
    for command in &definition.commands {
        if parse_verifier_command(command).is_err() {
            return Err(failure());
        }
    }
    Ok(())
}

fn parse_verifier_command(command: &str) -> anyhow::Result<Vec<String>> {
    let command = command.trim();
    if command.is_empty() {
        anyhow::bail!("workflow verifier command must not be empty");
    }
    for blocked in [
        "\0", "\n", "\r", ";", "&&", "||", "|", "`", "$(", "<", ">", "&",
    ] {
        if command.contains(blocked) {
            anyhow::bail!("workflow verifier command contains shell control syntax");
        }
    }
    let words = shlex::split(command).ok_or_else(|| {
        anyhow::anyhow!("workflow verifier command could not be parsed with shell quoting")
    })?;
    let Some(program) = words.first() else {
        anyhow::bail!("workflow verifier command must include a program");
    };
    if program.contains('/') || program.contains('\\') {
        anyhow::bail!("workflow verifier command program must be a bare allowlisted command");
    }
    if !DEFAULT_ALLOWED_COMMANDS.contains(&program.as_str()) {
        anyhow::bail!("workflow verifier command program is not allowlisted");
    }
    Ok(words)
}

fn verifier_sandbox_is_supported(sandbox: Option<&str>, turn: &TurnContext) -> bool {
    match sandbox.unwrap_or_default() {
        "default" | "turn_default" => true,
        "workspace-write" | "workspace_write" => {
            let policy = turn.file_system_sandbox_policy();
            !matches!(policy.kind, FileSystemSandboxKind::Restricted)
                || policy.entries.iter().any(|entry| entry.access.can_write())
        }
        "read-only" | "read_only" => {
            let policy = turn.file_system_sandbox_policy();
            matches!(policy.kind, FileSystemSandboxKind::Restricted)
                && !policy.entries.iter().any(|entry| entry.access.can_write())
        }
        _ => false,
    }
}

fn verifier_network_is_supported(network: Option<&str>, turn: &TurnContext) -> bool {
    match network.unwrap_or_default() {
        "default" | "turn_default" => true,
        "disabled" | "restricted" => {
            turn.network_sandbox_policy() == NetworkSandboxPolicy::Restricted
        }
        _ => false,
    }
}

fn verifier_cwd(turn: &TurnContext, cwd: Option<&str>) -> anyhow::Result<AbsolutePathBuf> {
    let cwd = cwd.unwrap_or(".");
    let path = Path::new(cwd);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("workflow verifier cwd must stay inside the turn workspace");
    }
    let base = turn.environments.primary().map_or_else(
        || {
            #[allow(deprecated)]
            turn.cwd.clone()
        },
        |environment| environment.cwd.clone(),
    );
    let canonical_base = base.canonicalize()?;
    let canonical_cwd = base.join(cwd).canonicalize()?;
    if !canonical_cwd
        .as_path()
        .starts_with(canonical_base.as_path())
    {
        anyhow::bail!("workflow verifier cwd must stay inside the turn workspace");
    }
    Ok(canonical_cwd)
}

#[derive(Debug)]
struct VerifierExecutionSummary {
    outcome: WorkflowRunVerifierOutcomeStatus,
    summary: WorkflowRunVerifierResultSummary,
}

impl VerifierExecutionSummary {
    fn failed(summary: WorkflowRunVerifierResultSummary) -> Self {
        Self {
            outcome: WorkflowRunVerifierOutcomeStatus::Failed,
            summary,
        }
    }

    fn policy_failure(command_count: i64) -> Self {
        Self::failed(WorkflowRunVerifierResultSummary {
            command_count,
            expected_exit_code: None,
            observed_exit_code: None,
            timed_out: false,
            duration_ms: 0,
            output_bytes: 0,
            output_truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::RunCommandsVerifierDefinition;
    use super::VerifierExecutionSummary;
    use super::WorkflowVerifierRetryPolicy;
    use super::execute_next_workflow_run_verifier;
    use super::execute_run_commands_definition;
    use super::parse_run_commands_definition;
    use super::parse_verifier_command;
    use crate::session::SessionSettingsUpdate;
    use crate::session::tests::make_session_and_context;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::protocol::AskForApproval;
    use codex_state::BackgroundAgentRunStatus;
    use codex_state::StateRuntime;
    use codex_state::WorkflowRunAdvanceParams;
    use codex_state::WorkflowRunBranchAdmissionParams;
    use codex_state::WorkflowRunBranchReconcileParams;
    use codex_state::WorkflowRunClaimParams;
    use codex_state::WorkflowRunCreateParams;
    use codex_state::WorkflowRunStatus;
    use codex_state::WorkflowRunStepStatus;
    use codex_state::WorkflowRunStepVerifierStatus;
    use codex_state::WorkflowRunVerifierOutcomeStatus;
    use codex_state::WorkflowSpecCreateParams;
    use tempfile::TempDir;

    fn run_commands_definition(command: &str) -> RunCommandsVerifierDefinition {
        RunCommandsVerifierDefinition {
            id: "tests".to_string(),
            kind: "run_commands".to_string(),
            artifact: None,
            must_contain: Vec::new(),
            cwd: Some(".".to_string()),
            sandbox: Some("default".to_string()),
            network: Some("default".to_string()),
            timeout_seconds: Some(30),
            output_limit_bytes: Some(2048),
            commands: vec![command.to_string()],
            expected_stdout: None,
            expected_exit_code: Some(0),
            retry_policy: Some(WorkflowVerifierRetryPolicy { max_attempts: 1 }),
        }
    }

    async fn execute_test_definition(
        definition: &RunCommandsVerifierDefinition,
    ) -> Result<VerifierExecutionSummary, VerifierExecutionSummary> {
        let (session, _turn) = make_session_and_context().await;
        session
            .update_settings(SessionSettingsUpdate {
                approval_policy: Some(AskForApproval::Never),
                permission_profile: Some(PermissionProfile::Disabled),
                ..Default::default()
            })
            .await
            .expect("verifier test session should allow non-interactive execution");
        let turn = session.new_default_turn().await;
        execute_run_commands_definition(Arc::new(session), turn, definition).await
    }

    fn e2e_workflow_yaml(workflow_id: &str, command: &str, expected_stdout: &str) -> String {
        format!(
            r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "{workflow_id}"
display_name: "Verifier Entrypoint Test"
source_prompt: "verify deterministic command execution without leaking output"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 1
  max_agents: 3
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 120
  max_tokens: 6000
  max_tool_calls: 50
approvals:
  required_before: []
agents:
  - id: "builder"
    display_name: "Builder-Archimedes"
    role: "Complete the implementation branch."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "adversarial_security"
    display_name: "Adversary-Hypatia"
    role: "Attack verifier output leakage."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "adversarial_testing"
    display_name: "Adversary-Euclid"
    role: "Attack verifier result persistence."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "implementation"
    title: "Implement and verify"
    agent: "builder"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on: []
    outputs:
      - "implementation.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "command_check"
          type: "run_commands"
          cwd: "."
          sandbox: "default"
          network: "default"
          timeout_seconds: 30
          output_limit_bytes: 2048
          commands:
            - "{command}"
          expected_stdout: "{expected_stdout}"
          expected_exit_code: 0
artifacts:
  retention: "until_workflow_complete"
  required: []
cleanup:
  on_cancel: []
  on_complete: []
"#
        )
    }

    #[test]
    fn parses_run_commands_definition_without_raw_execution() {
        let definition = parse_run_commands_definition(&json!({
            "schemaVersion": "workflow.run_state/v0",
            "redactionVersion": 1,
            "kind": "workflow_run_step_verifier_definition",
            "data": {
                "id": "tests",
                "type": "run_commands",
                "cwd": ".",
                "sandbox": "default",
                "network": "default",
                "timeout_seconds": 30,
                "output_limit_bytes": 2048,
                "commands": ["just test-fast -p codex-state"],
                "expected_exit_code": 0,
                "retry_policy": {
                    "max_attempts": 2
                }
            }
        }))
        .expect("definition should parse");

        assert_eq!(definition.commands, vec!["just test-fast -p codex-state"]);
    }

    #[test]
    fn rejects_shell_control_in_verifier_commands() {
        assert!(parse_verifier_command("just test-fast -p codex-state").is_ok());
        assert!(parse_verifier_command("true & cat .env").is_err());
        assert!(parse_verifier_command("echo ok; touch /tmp/secret").is_err());
        assert!(parse_verifier_command("just test-fast -p codex-state && cat .env").is_err());
        assert!(parse_verifier_command("sh -c 'echo bypass'").is_err());
    }

    #[test]
    fn rejects_path_qualified_allowlisted_commands() {
        assert!(parse_verifier_command("/tmp/evil/just test").is_err());
        assert!(parse_verifier_command("./just test").is_err());
        assert!(parse_verifier_command("../just test").is_err());
        assert!(parse_verifier_command("just test").is_ok());
    }

    #[test]
    fn rejects_unknown_verifier_definition_fields() {
        let err = parse_run_commands_definition(&json!({
            "schemaVersion": "workflow.run_state/v0",
            "redactionVersion": 1,
            "kind": "workflow_run_step_verifier_definition",
            "data": {
                "id": "tests",
                "type": "run_commands",
                "cwd": ".",
                "sandbox": "default",
                "network": "default",
                "timeout_seconds": 30,
                "output_limit_bytes": 2048,
                "commands": ["just test-fast -p codex-state"],
                "expected_exit_code": 0,
                "extra_policy": "silently ignored"
            }
        }))
        .expect_err("unknown fields should fail closed");

        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn executes_passing_and_failing_verifier_commands() {
        let passed = execute_test_definition(&run_commands_definition("true"))
            .await
            .expect("true verifier should pass");
        assert_eq!(WorkflowRunVerifierOutcomeStatus::Passed, passed.outcome);
        assert_eq!(Some(0), passed.summary.observed_exit_code);

        let failed = execute_test_definition(&run_commands_definition("false"))
            .await
            .expect_err("false verifier should fail");
        assert_eq!(WorkflowRunVerifierOutcomeStatus::Failed, failed.outcome);
        assert_eq!(Some(0), failed.summary.expected_exit_code);
        assert_eq!(Some(1), failed.summary.observed_exit_code);
    }

    #[tokio::test]
    async fn execute_next_workflow_run_verifier_claims_executes_and_persists_result() {
        let codex_home = TempDir::new().expect("temp home should create");
        let state = StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".into())
            .await
            .expect("state db should initialize");
        let sentinel = "12345";
        let spec = state
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: None,
                source_yaml: e2e_workflow_yaml(
                    "wf_core_verifier_e2e",
                    "python3 -c 'print(12345)'",
                    "12345",
                ),
            })
            .await
            .expect("workflow spec should save");
        let run = state
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: None,
                idempotency_key: Some("wf-core-verifier-e2e-run".to_string()),
            })
            .await
            .expect("workflow run should create");
        let claim = state
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                lease_duration_ms: Some(60_000),
            })
            .await
            .expect("workflow claim should succeed")
            .expect("workflow should claim");
        state
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("workflow advance should succeed")
            .expect("workflow should advance");
        let admitted = state
            .admit_workflow_run_branches(WorkflowRunBranchAdmissionParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation: claim.generation,
                auth_profile_ref: None,
                config_fingerprint: None,
                version_fingerprint: None,
                parent_agent_run_id: None,
                max_active_background_agent_runs: Some(10),
            })
            .await
            .expect("branch admission should succeed")
            .expect("workflow should admit");
        assert_eq!(1, admitted.admitted.len());
        state
            .update_background_agent_run_status(
                admitted.admitted[0].background_agent_run_id.as_str(),
                BackgroundAgentRunStatus::Completed,
                Some("branch complete"),
            )
            .await
            .expect("background run status should update");
        let reconciled = state
            .reconcile_workflow_run_branches(WorkflowRunBranchReconcileParams {
                run_id: run.run.run_id.clone(),
                owner_id: "verifier-owner".to_string(),
                generation: claim.generation,
            })
            .await
            .expect("branch reconcile should succeed")
            .expect("workflow should reconcile");
        assert_eq!(
            WorkflowRunStepStatus::WaitingVerifier,
            reconciled.snapshot.steps[0].status
        );
        assert_eq!(
            WorkflowRunStepVerifierStatus::Blocked,
            reconciled.snapshot.verifiers[0].status
        );

        let (session, _turn) = make_session_and_context().await;
        session
            .update_settings(SessionSettingsUpdate {
                approval_policy: Some(AskForApproval::Never),
                permission_profile: Some(PermissionProfile::Disabled),
                ..Default::default()
            })
            .await
            .expect("verifier test session should allow non-interactive execution");
        let turn = session.new_default_turn().await;
        let session = Arc::new(session);

        let stale = execute_next_workflow_run_verifier(
            state.as_ref(),
            Arc::clone(&session),
            Arc::clone(&turn),
            run.run.run_id.clone(),
            "verifier-owner".to_string(),
            claim.generation + 1,
        )
        .await
        .expect("stale verifier execution should not error");
        assert_eq!(None, stale);

        let recorded = execute_next_workflow_run_verifier(
            state.as_ref(),
            Arc::clone(&session),
            Arc::clone(&turn),
            run.run.run_id.clone(),
            "verifier-owner".to_string(),
            claim.generation,
        )
        .await
        .expect("verifier execution should not error")
        .expect("verifier result should record");
        assert_eq!(WorkflowRunStatus::Completed, recorded.snapshot.run.status);
        assert_eq!(
            WorkflowRunStepStatus::Succeeded,
            recorded.snapshot.steps[0].status
        );
        assert_eq!(
            WorkflowRunStepVerifierStatus::Passed,
            recorded.snapshot.verifiers[0].status
        );
        let result_json = recorded.snapshot.verifiers[0]
            .last_result_json
            .as_ref()
            .expect("verifier result should persist")
            .to_string();
        assert!(!result_json.contains(sentinel));
        let event_payloads = recorded
            .snapshot
            .events
            .iter()
            .map(|event| event.event_payload_json.to_string())
            .collect::<String>();
        assert!(!event_payloads.contains(sentinel));

        let terminal = execute_next_workflow_run_verifier(
            state.as_ref(),
            session,
            turn,
            run.run.run_id,
            "verifier-owner".to_string(),
            claim.generation,
        )
        .await
        .expect("terminal verifier execution should not error");
        assert_eq!(None, terminal);
    }

    #[tokio::test]
    async fn verifier_execution_fails_on_stdout_mismatch_and_output_cap() {
        let mut mismatch = run_commands_definition("python3 -c 'print(\"actual\")'");
        mismatch.expected_stdout = Some("missing".to_string());
        let mismatch_result = execute_test_definition(&mismatch)
            .await
            .expect_err("stdout mismatch should fail");
        assert_eq!(
            WorkflowRunVerifierOutcomeStatus::Failed,
            mismatch_result.outcome
        );
        assert_eq!(Some(0), mismatch_result.summary.observed_exit_code);

        let mut capped = run_commands_definition("python3 -c 'print(\"abcdef\")'");
        capped.output_limit_bytes = Some(1);
        let capped_result = execute_test_definition(&capped)
            .await
            .expect_err("output cap should fail");
        assert_eq!(
            WorkflowRunVerifierOutcomeStatus::Failed,
            capped_result.outcome
        );
        assert!(capped_result.summary.output_truncated);
    }

    #[tokio::test]
    async fn verifier_execution_fails_on_timeout_and_cwd_escape() {
        let mut timeout = run_commands_definition("python3 -c 'while True: pass'");
        timeout.timeout_seconds = Some(1);
        let timeout_result = execute_test_definition(&timeout)
            .await
            .expect_err("timeout should fail");
        assert_eq!(
            WorkflowRunVerifierOutcomeStatus::Failed,
            timeout_result.outcome
        );
        assert!(timeout_result.summary.timed_out);

        let mut cwd_escape = run_commands_definition("true");
        cwd_escape.cwd = Some("..".to_string());
        let cwd_result = execute_test_definition(&cwd_escape)
            .await
            .expect_err("cwd escape should fail policy");
        assert_eq!(WorkflowRunVerifierOutcomeStatus::Failed, cwd_result.outcome);
        assert_eq!(None, cwd_result.summary.observed_exit_code);
    }
}
