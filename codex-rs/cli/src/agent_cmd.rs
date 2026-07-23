use anyhow::Context;
use clap::Args;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_background_agent::BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION;
use codex_background_agent::DEFAULT_MAX_ACTIVE_BACKGROUND_AGENT_RUNS;
use codex_background_agent::daemon::BackgroundAgentDaemon;
use codex_background_agent::daemon::BackgroundAgentDaemonPaths;
use codex_background_agent::daemon::background_agent_daemon_state_dir;
use codex_background_agent::daemon::ensure_supported_platform as ensure_background_agent_supported_platform;
use codex_core::config::find_codex_home;
use codex_protocol::models::PermissionProfile;
use codex_state::BackgroundAgentDesiredState;
use codex_state::BackgroundAgentExecutionSnapshotParams;
use codex_state::BackgroundAgentPendingInteractionStatus;
use codex_state::BackgroundAgentRun;
use codex_state::BackgroundAgentRunCreateParams;
use codex_state::BackgroundAgentRunStatus;
use codex_state::BackgroundAgentStatusSnapshotParams;
use codex_state::StateRuntime;
use codex_state::busy_retry::retry_on_busy;
use codex_utils_absolute_path::AbsolutePathBuf;

const COMPACT_FIELD_PREVIEW_CHARS: usize = 160;
const COMPACT_PAYLOAD_PREVIEW_CHARS: usize = 240;
const DEFAULT_AGENT_LIST_LIMIT: usize = 20;
const DEFAULT_AGENT_LIST_JSON_LIMIT: usize = 50;
const DEFAULT_AGENT_EVENTS_LIMIT: usize = 20;
const DEFAULT_AGENT_EVENTS_JSON_LIMIT: usize = 100;

#[derive(Debug, Clone)]
pub(crate) struct AgentStartRuntimeContext {
    pub(crate) cwd: PathBuf,
    pub(crate) workspace_roots: Vec<PathBuf>,
    pub(crate) auth_profile_ref: Option<String>,
    pub(crate) approval_policy: Option<Value>,
    pub(crate) permission_profile: PermissionProfile,
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) service_tier: Option<String>,
}

impl AgentStartRuntimeContext {
    pub(crate) fn from_config(config: &codex_core::config::Config) -> Self {
        Self {
            cwd: config.cwd.as_path().to_path_buf(),
            workspace_roots: config
                .workspace_roots
                .iter()
                .map(|root| root.as_path().to_path_buf())
                .collect(),
            auth_profile_ref: config.selected_auth_profile.clone(),
            approval_policy: serde_json::to_value(config.permissions.approval_policy.value()).ok(),
            permission_profile: config.permissions.permission_profile().clone(),
            model: config.model.clone(),
            provider: Some(config.model_provider_id.clone()),
            service_tier: config.service_tier.clone(),
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct AgentCli {
    #[command(subcommand)]
    pub(crate) subcommand: AgentSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum AgentSubcommand {
    /// Enqueue a durable background-agent run.
    Start(AgentStartCommand),

    /// List durable background-agent runs.
    List(AgentListCommand),

    /// Read one durable background-agent run.
    Read(AgentIdCommand),

    /// Attach to one durable background-agent run and deliver pending interactions.
    Attach(AgentLogsCommand),

    /// Print durable background-agent events for a run.
    Logs(AgentLogsCommand),

    /// Request a background-agent run stop.
    Stop(AgentIdCommand),

    /// Mark a background-agent run for deletion.
    Delete(AgentIdCommand),

    /// Print durable background-agent admission and status diagnostics.
    Diagnostics(AgentDiagnosticsCommand),
}

/// Enqueue a durable background-agent run.
///
/// This command has no local `--auth-profile` flag: it inherits the top-level
/// `codewith --auth-profile <name>` selector (or the `CODEWITH_AUTH_PROFILE`/`CODEX_AUTH_PROFILE`
/// environment variables), so the durable agent runs under that profile.
#[derive(Debug, Args)]
pub(crate) struct AgentStartCommand {
    /// Prompt to run in the background.
    #[arg(required = true, num_args = 1..)]
    prompt: Vec<String>,

    /// Idempotency key for retrying the same start request.
    #[arg(long = "idempotency-key")]
    idempotency_key: Option<String>,

    /// Working directory for the run. Defaults to the current directory.
    #[arg(long = "cwd")]
    cwd: Option<PathBuf>,

    /// Output the full background-agent record as JSON.
    #[arg(long = "json")]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentListCommand {
    /// Maximum number of runs to return.
    #[arg(long = "limit")]
    limit: Option<usize>,

    /// Output full background-agent records as JSON.
    #[arg(long = "json")]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentIdCommand {
    /// Background-agent run id.
    agent_id: String,

    /// Output the full background-agent record as JSON.
    #[arg(long = "json")]
    json: bool,

    /// Include compact payload previews in human output.
    #[arg(long = "verbose")]
    verbose: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentLogsCommand {
    /// Background-agent run id.
    agent_id: String,

    /// Return events after this sequence number.
    #[arg(long = "after-seq")]
    after_seq: Option<i64>,

    /// Maximum number of events to return.
    #[arg(long = "limit")]
    limit: Option<usize>,

    /// Output full background-agent event payloads as JSON.
    #[arg(long = "json")]
    json: bool,

    /// Include compact payload previews in human output.
    #[arg(long = "verbose")]
    verbose: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentDiagnosticsCommand {
    /// Output full diagnostics as JSON.
    #[arg(long = "json")]
    json: bool,
}

#[derive(Debug, Clone, Copy)]
enum AgentPrintMode {
    Json,
    Start,
    List,
    Read { verbose: bool },
    Events { verbose: bool },
    Mutation { action: &'static str },
    Diagnostics,
}

pub(crate) async fn run_agent_command(
    cli: AgentCli,
    runtime_context: Option<AgentStartRuntimeContext>,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    let state_db = state_runtime().await?;
    let (output, print_mode) = match cli.subcommand {
        AgentSubcommand::Start(cmd) => {
            let json = cmd.json;
            let output = start_agent(
                state_db.as_ref(),
                cmd,
                runtime_context.as_ref(),
                auth_profile,
            )
            .await?;
            (
                output,
                if json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Start
                },
            )
        }
        AgentSubcommand::List(cmd) => {
            let limit = cmd.limit.unwrap_or(if cmd.json {
                DEFAULT_AGENT_LIST_JSON_LIMIT
            } else {
                DEFAULT_AGENT_LIST_LIMIT
            });
            let runs = state_db
                .list_background_agent_runs(Some(limit))
                .await
                .context("failed to list background agents")?;
            let output = json!({ "data": runs.into_iter().map(run_json).collect::<Vec<_>>() });
            (
                output,
                if cmd.json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::List
                },
            )
        }
        AgentSubcommand::Read(cmd) => {
            let agent = state_db
                .get_background_agent_run(cmd.agent_id.as_str())
                .await
                .context("failed to read background agent")?;
            let status_snapshot = state_db
                .get_background_agent_status_snapshot(cmd.agent_id.as_str())
                .await
                .context("failed to read background agent status snapshot")?;
            let execution_snapshot = state_db
                .get_latest_background_agent_execution_snapshot(cmd.agent_id.as_str())
                .await
                .context("failed to read background agent execution snapshot")?;
            let pending_interactions = state_db
                .list_background_agent_pending_interactions(
                    cmd.agent_id.as_str(),
                    /*status*/ None,
                )
                .await
                .context("failed to list background agent pending interactions")?;
            let output = json!({
                "agent": agent.map(run_json),
                "statusSnapshot": status_snapshot.map(|snapshot| json!({
                    "seq": snapshot.seq,
                    "status": snapshot.status.as_str(),
                    "desiredState": snapshot.desired_state.as_str(),
                    "summary": snapshot.summary,
                    "pendingInteractionCount": snapshot.pending_interaction_count,
                    "lastEventSeq": snapshot.last_event_seq,
                    "payload": snapshot.payload_json,
                    "updatedAt": snapshot.updated_at.timestamp(),
                })),
                "executionSnapshot": execution_snapshot.map(|snapshot| json!({
                    "seq": snapshot.seq,
                    "snapshotKind": snapshot.snapshot_kind,
                    "payload": snapshot.payload_json,
                    "recoveryPolicy": snapshot.recovery_policy,
                    "configFingerprint": snapshot.config_fingerprint,
                    "createdAt": snapshot.created_at.timestamp(),
                })),
                "pendingInteractions": pending_interactions
                    .into_iter()
                    .map(|interaction| json!({
                        "interactionId": interaction.id,
                        "workerRequestId": interaction.worker_request_id,
                        "kind": interaction.kind.as_str(),
                        "status": interaction.status.as_str(),
                        "requestPayload": interaction.request_payload_json,
                        "responsePayload": interaction.response_payload_json,
                        "timeoutAt": interaction.timeout_at.map(|value| value.timestamp()),
                    }))
                .collect::<Vec<_>>(),
            });
            (
                output,
                if cmd.json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Read {
                        verbose: cmd.verbose,
                    }
                },
            )
        }
        AgentSubcommand::Attach(cmd) => {
            let json = cmd.json;
            let verbose = cmd.verbose;
            let output = attach_agent(state_db.as_ref(), cmd).await?;
            (
                output,
                if json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Events { verbose }
                },
            )
        }
        AgentSubcommand::Logs(cmd) => {
            let limit = cmd.limit.unwrap_or(if cmd.json {
                DEFAULT_AGENT_EVENTS_JSON_LIMIT
            } else {
                DEFAULT_AGENT_EVENTS_LIMIT
            });
            let events = state_db
                .list_background_agent_events_after(
                    cmd.agent_id.as_str(),
                    cmd.after_seq,
                    Some(limit),
                )
                .await
                .context("failed to list background agent events")?;
            let output = json!({
                "data": events
                    .into_iter()
                    .map(|event| json!({
                        "agentId": event.run_id,
                        "seq": event.seq,
                        "eventType": event.event_type,
                        "payload": event.payload_json,
                        "createdAt": event.created_at.timestamp(),
                    }))
                    .collect::<Vec<_>>()
            });
            (
                output,
                if cmd.json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Events {
                        verbose: cmd.verbose,
                    }
                },
            )
        }
        AgentSubcommand::Stop(cmd) => {
            let run = stop_agent(state_db.as_ref(), cmd.agent_id.as_str()).await?;
            let output = json!({ "agent": run.map(run_json) });
            (
                output,
                if cmd.json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Mutation { action: "stop" }
                },
            )
        }
        AgentSubcommand::Delete(cmd) => {
            let existing_run = state_db
                .get_background_agent_run(cmd.agent_id.as_str())
                .await
                .context("failed to read background agent before delete")?;
            let deleted = state_db
                .request_background_agent_delete(cmd.agent_id.as_str())
                .await
                .context("failed to request background agent delete")?;
            if deleted {
                if existing_run.as_ref().is_some_and(|run| {
                    !background_agent_status_is_terminal(run.status)
                        && should_terminalize_unclaimed_agent_run(run)
                }) {
                    state_db
                        .update_background_agent_run_status(
                            cmd.agent_id.as_str(),
                            BackgroundAgentRunStatus::Cancelled,
                            Some("delete requested by codewith agent delete before worker claim"),
                        )
                        .await
                        .context("failed to update background agent status after delete")?;
                }
                state_db
                    .append_background_agent_event(
                        cmd.agent_id.as_str(),
                        "agent.deleteRequested",
                        &json!({"reason": "cli_requested_delete"}),
                    )
                    .await
                    .context("failed to append background agent delete event")?;
            }
            let run = state_db
                .get_background_agent_run(cmd.agent_id.as_str())
                .await
                .context("failed to read background agent after delete")?;
            let output = json!({ "deleted": deleted, "agent": run.map(run_json) });
            (
                output,
                if cmd.json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Mutation { action: "delete" }
                },
            )
        }
        AgentSubcommand::Diagnostics(cmd) => {
            let output = diagnostics_json(state_db.as_ref()).await?;
            (
                output,
                if cmd.json {
                    AgentPrintMode::Json
                } else {
                    AgentPrintMode::Diagnostics
                },
            )
        }
    };
    print_agent_output(&output, print_mode)?;
    Ok(())
}

pub(crate) async fn run_background_agent_start(
    prompt: String,
    cwd: Option<PathBuf>,
    runtime_context: Option<AgentStartRuntimeContext>,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Start(AgentStartCommand {
                prompt: vec![prompt],
                idempotency_key: None,
                cwd,
                json: false,
            }),
        },
        runtime_context,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_attach(
    agent_id: String,
    after_seq: Option<i64>,
    limit: Option<usize>,
    json: bool,
    verbose: bool,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Attach(AgentLogsCommand {
                agent_id,
                after_seq,
                limit,
                json,
                verbose,
            }),
        },
        /*runtime_context*/ None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_logs(
    agent_id: String,
    after_seq: Option<i64>,
    limit: Option<usize>,
    json: bool,
    verbose: bool,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Logs(AgentLogsCommand {
                agent_id,
                after_seq,
                limit,
                json,
                verbose,
            }),
        },
        /*runtime_context*/ None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_stop(
    agent_id: String,
    json: bool,
    verbose: bool,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Stop(AgentIdCommand {
                agent_id,
                json,
                verbose,
            }),
        },
        /*runtime_context*/ None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_delete(
    agent_id: String,
    json: bool,
    verbose: bool,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Delete(AgentIdCommand {
                agent_id,
                json,
                verbose,
            }),
        },
        /*runtime_context*/ None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_daemon_status() -> anyhow::Result<()> {
    let output = background_agent_daemon()?.status().await?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) async fn run_background_agent_daemon_stop() -> anyhow::Result<()> {
    let output = background_agent_daemon()?.stop().await?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_agent_output(output: &Value, mode: AgentPrintMode) -> anyhow::Result<()> {
    match mode {
        AgentPrintMode::Json => print_json(output),
        AgentPrintMode::Start => {
            print_agent_start(output);
            Ok(())
        }
        AgentPrintMode::List => {
            print_agent_list(output);
            Ok(())
        }
        AgentPrintMode::Read { verbose } => {
            print_agent_read(output, verbose);
            Ok(())
        }
        AgentPrintMode::Events { verbose } => {
            print_agent_events(output, verbose);
            Ok(())
        }
        AgentPrintMode::Mutation { action } => {
            print_agent_mutation(output, action);
            Ok(())
        }
        AgentPrintMode::Diagnostics => {
            print_agent_diagnostics(output);
            Ok(())
        }
    }
}

fn print_json(output: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(output)?);
    Ok(())
}

fn print_agent_start(output: &Value) {
    let Some(agent) = output.get("agent") else {
        println!("No background agent was returned.");
        return;
    };
    let id = value_str(agent, "agentId").unwrap_or("<unknown>");
    let status = value_str(agent, "status").unwrap_or("unknown");
    let created = output
        .get("created")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if created {
        println!("Started background agent {id} ({status}).");
    } else {
        println!("Found existing background agent {id} ({status}).");
    }
    if let Some(reason) = value_str(agent, "statusReason") {
        println!(
            "Reason: {}",
            compact_text_preview(reason, COMPACT_FIELD_PREVIEW_CHARS)
        );
    }
    println!(
        "Use `codewith agent read {id}` for status, `codewith agent logs {id}` for events, or `codewith agent read {id} --json` for the full record."
    );
}

fn print_agent_list(output: &Value) {
    let rows = output
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if rows.is_empty() {
        println!("No background agents found.");
        println!("Use `codewith agent start <prompt>` to enqueue one.");
        return;
    }

    println!(
        "{:<48}  {:<14}  {:<8}  {:<10}  {:<12}  SUMMARY",
        "AGENT_ID", "STATUS", "DESIRED", "SOURCE", "UPDATED_AT"
    );
    for agent in rows {
        let id = value_str(agent, "agentId").unwrap_or("<unknown>");
        let status = value_str(agent, "status").unwrap_or("unknown");
        let desired = value_str(agent, "desiredState").unwrap_or("unknown");
        let source = value_str(agent, "source").unwrap_or("unknown");
        let updated_at = value_i64(agent, "updatedAt")
            .map(|timestamp| timestamp.to_string())
            .unwrap_or_else(|| "-".to_string());
        let summary = value_str(agent, "statusReason")
            .map(|reason| compact_text_preview(reason, COMPACT_FIELD_PREVIEW_CHARS))
            .unwrap_or_default();
        println!("{id:<48}  {status:<14}  {desired:<8}  {source:<10}  {updated_at:<12}  {summary}");
    }
    println!(
        "Shown {} agent(s). Use `codewith agent read <id>` for details, `codewith agent logs <id>` for events, `--limit N` to page, or `--json` for full records.",
        rows.len()
    );
}

fn print_agent_read(output: &Value, verbose: bool) {
    let Some(agent) = output.get("agent").filter(|value| !value.is_null()) else {
        println!("Background agent not found.");
        return;
    };
    let id = value_str(agent, "agentId").unwrap_or("<unknown>");
    println!("Background agent {id}");
    println!(
        "  status: {}",
        value_str(agent, "status").unwrap_or("unknown")
    );
    println!(
        "  desired: {}",
        value_str(agent, "desiredState").unwrap_or("unknown")
    );
    if let Some(reason) = value_str(agent, "statusReason") {
        println!(
            "  reason: {}",
            compact_text_preview(reason, COMPACT_FIELD_PREVIEW_CHARS)
        );
    }
    if let Some(thread_id) = value_str(agent, "threadId") {
        println!("  thread: {thread_id}");
    }
    println!(
        "  events: last_seq={} last_snapshot_seq={}",
        value_i64(agent, "lastEventSeq").unwrap_or_default(),
        value_i64(agent, "lastSnapshotSeq").unwrap_or_default()
    );

    if let Some(snapshot) = output
        .get("statusSnapshot")
        .filter(|value| !value.is_null())
    {
        println!(
            "  snapshot: status={} pending={} last_event_seq={}",
            value_str(snapshot, "status").unwrap_or("unknown"),
            value_i64(snapshot, "pendingInteractionCount").unwrap_or_default(),
            value_i64(snapshot, "lastEventSeq").unwrap_or_default()
        );
        if let Some(summary) = value_str(snapshot, "summary") {
            println!(
                "  summary: {}",
                compact_text_preview(summary, COMPACT_FIELD_PREVIEW_CHARS)
            );
        }
        if verbose && let Some(payload) = snapshot.get("payload") {
            println!(
                "  status_payload: {}",
                compact_json_preview(payload, COMPACT_PAYLOAD_PREVIEW_CHARS)
            );
        }
    }

    if let Some(snapshot) = output
        .get("executionSnapshot")
        .filter(|value| !value.is_null())
    {
        println!(
            "  execution_snapshot: kind={} seq={}",
            value_str(snapshot, "snapshotKind").unwrap_or("unknown"),
            value_i64(snapshot, "seq").unwrap_or_default()
        );
        if verbose && let Some(payload) = snapshot.get("payload") {
            println!(
                "  execution_payload: {}",
                compact_json_preview(payload, COMPACT_PAYLOAD_PREVIEW_CHARS)
            );
        }
    }

    let pending = output
        .get("pendingInteractions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    println!("  pending_interactions: {}", pending.len());
    if verbose {
        for interaction in pending {
            println!(
                "    {} {} {}",
                value_str(interaction, "interactionId").unwrap_or("<unknown>"),
                value_str(interaction, "kind").unwrap_or("unknown"),
                value_str(interaction, "status").unwrap_or("unknown")
            );
            if let Some(payload) = interaction.get("requestPayload") {
                println!(
                    "      request: {}",
                    compact_json_preview(payload, COMPACT_PAYLOAD_PREVIEW_CHARS)
                );
            }
        }
    }
    println!(
        "Use `codewith agent read {id} --verbose` for payload previews or `--json` for the full record."
    );
}

fn print_agent_events(output: &Value, verbose: bool) {
    if let Some(agent) = output.get("agent").filter(|value| !value.is_null()) {
        let id = value_str(agent, "agentId").unwrap_or("<unknown>");
        println!(
            "Background agent {id}: {}",
            value_str(agent, "status").unwrap_or("unknown")
        );
    }

    let events = output
        .get("events")
        .or_else(|| output.get("data"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if events.is_empty() {
        println!("No background-agent events found.");
    } else {
        println!(
            "{:<8}  {:<28}  {:<12}  SUMMARY",
            "SEQ", "EVENT", "CREATED_AT"
        );
        for event in events {
            let seq = value_i64(event, "seq").unwrap_or_default();
            let event_type = value_str(event, "eventType").unwrap_or("unknown");
            let created_at = value_i64(event, "createdAt")
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_else(|| "-".to_string());
            let summary = if verbose {
                event
                    .get("payload")
                    .map(|payload| compact_json_preview(payload, COMPACT_PAYLOAD_PREVIEW_CHARS))
                    .unwrap_or_default()
            } else {
                payload_event_summary(event.get("payload"))
            };
            println!("{seq:<8}  {event_type:<28}  {created_at:<12}  {summary}");
        }
    }

    let pending = output
        .get("pendingInteractions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !pending.is_empty() {
        println!("Pending interactions: {}", pending.len());
    }
    println!(
        "Use `--verbose` for payload previews, `--json` for full payloads, `--after-seq N` to continue, or `--limit N` to page."
    );
}

fn print_agent_mutation(output: &Value, action: &'static str) {
    let Some(agent) = output.get("agent").filter(|value| !value.is_null()) else {
        println!("No background agent matched the {action} request.");
        return;
    };
    let id = value_str(agent, "agentId").unwrap_or("<unknown>");
    let status = value_str(agent, "status").unwrap_or("unknown");
    println!("Requested {action} for background agent {id} ({status}).");
    if let Some(reason) = value_str(agent, "statusReason") {
        println!(
            "Reason: {}",
            compact_text_preview(reason, COMPACT_FIELD_PREVIEW_CHARS)
        );
    }
    println!("Use `codewith agent read {id}` for status or `--json` for the full record.");
}

fn print_agent_diagnostics(output: &Value) {
    println!("Background-agent diagnostics");
    println!(
        "  active: {} / {}",
        value_i64(output, "activeRunCount").unwrap_or_default(),
        value_i64(output, "maxActiveRunsPerUser").unwrap_or_default()
    );
    println!(
        "  available_slots: {}",
        value_i64(output, "availableActiveRunSlots").unwrap_or_default()
    );
    println!(
        "  pending_interactions: {}",
        value_i64(output, "pendingInteractionCount").unwrap_or_default()
    );
    println!(
        "  admission_allowed: {}",
        output
            .get("admissionAllowed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    );
    if let Some(counts) = output.get("runsByStatus").and_then(Value::as_array) {
        let summary = counts
            .iter()
            .filter_map(|entry| {
                let status = value_str(entry, "status")?;
                let count = value_i64(entry, "count")?;
                Some(format!("{status}={count}"))
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !summary.is_empty() {
            println!("  runs_by_status: {summary}");
        }
    }
    if let Some(daemon) = output.get("daemon") {
        println!(
            "  daemon: {}",
            compact_json_preview(daemon, COMPACT_PAYLOAD_PREVIEW_CHARS)
        );
    }
    println!("Use `codewith agent diagnostics --json` for the full diagnostic object.");
}

fn payload_event_summary(payload: Option<&Value>) -> String {
    let Some(payload) = payload else {
        return String::new();
    };
    for key in ["summary", "phase", "status", "reason", "message"] {
        if let Some(value) = value_str(payload, key) {
            return compact_text_preview(value, COMPACT_FIELD_PREVIEW_CHARS);
        }
    }
    "(payload hidden; use --verbose)".to_string()
}

fn compact_json_preview(value: &Value, max_chars: usize) -> String {
    let rendered = serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string());
    compact_text_preview(&rendered, max_chars)
}

fn compact_text_preview(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_end(&normalized, max_chars)
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn value_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn value_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

async fn attach_agent(state_db: &StateRuntime, cmd: AgentLogsCommand) -> anyhow::Result<Value> {
    let limit = cmd.limit.unwrap_or(if cmd.json {
        DEFAULT_AGENT_EVENTS_JSON_LIMIT
    } else {
        DEFAULT_AGENT_EVENTS_LIMIT
    });
    state_db
        .expire_background_agent_pending_interactions()
        .await
        .context("failed to expire stale background agent pending interactions")?;
    let pending_before_delivery = state_db
        .list_background_agent_pending_interactions(
            cmd.agent_id.as_str(),
            Some(BackgroundAgentPendingInteractionStatus::Pending),
        )
        .await
        .context("failed to list pending background agent interactions before attach")?;
    for interaction in pending_before_delivery {
        state_db
            .mark_background_agent_pending_interaction_delivered(interaction.id.as_str())
            .await
            .with_context(|| {
                format!(
                    "failed to mark background agent interaction {} delivered",
                    interaction.id
                )
            })?;
    }

    let agent = state_db
        .get_background_agent_run(cmd.agent_id.as_str())
        .await
        .context("failed to read background agent")?;
    let status_snapshot = state_db
        .get_background_agent_status_snapshot(cmd.agent_id.as_str())
        .await
        .context("failed to read background agent status snapshot")?;
    let execution_snapshot = state_db
        .get_latest_background_agent_execution_snapshot(cmd.agent_id.as_str())
        .await
        .context("failed to read background agent execution snapshot")?;
    let events = state_db
        .list_background_agent_events_after(cmd.agent_id.as_str(), cmd.after_seq, Some(limit))
        .await
        .context("failed to list background agent events")?;
    let pending_interactions = state_db
        .list_background_agent_pending_interactions(cmd.agent_id.as_str(), /*status*/ None)
        .await
        .context("failed to list background agent pending interactions")?;
    Ok(json!({
        "agent": agent.map(run_json),
        "statusSnapshot": status_snapshot.map(|snapshot| json!({
            "seq": snapshot.seq,
            "status": snapshot.status.as_str(),
            "desiredState": snapshot.desired_state.as_str(),
            "summary": snapshot.summary,
            "pendingInteractionCount": snapshot.pending_interaction_count,
            "lastEventSeq": snapshot.last_event_seq,
            "payload": snapshot.payload_json,
            "updatedAt": snapshot.updated_at.timestamp(),
        })),
        "executionSnapshot": execution_snapshot.map(|snapshot| json!({
            "seq": snapshot.seq,
            "snapshotKind": snapshot.snapshot_kind,
            "payload": snapshot.payload_json,
            "recoveryPolicy": snapshot.recovery_policy,
            "configFingerprint": snapshot.config_fingerprint,
            "createdAt": snapshot.created_at.timestamp(),
        })),
        "events": events
            .into_iter()
            .map(|event| json!({
                "agentId": event.run_id,
                "seq": event.seq,
                "eventType": event.event_type,
                "payload": event.payload_json,
                "createdAt": event.created_at.timestamp(),
            }))
            .collect::<Vec<_>>(),
        "pendingInteractions": pending_interactions
            .into_iter()
            .map(|interaction| json!({
                "interactionId": interaction.id,
                "workerRequestId": interaction.worker_request_id,
                "kind": interaction.kind.as_str(),
                "status": interaction.status.as_str(),
                "requestPayload": interaction.request_payload_json,
                "responsePayload": interaction.response_payload_json,
                "timeoutAt": interaction.timeout_at.map(|value| value.timestamp()),
            }))
            .collect::<Vec<_>>(),
    }))
}

async fn start_agent(
    state_db: &StateRuntime,
    cmd: AgentStartCommand,
    runtime_context: Option<&AgentStartRuntimeContext>,
    auth_profile: Option<&str>,
) -> anyhow::Result<Value> {
    let prompt = cmd.prompt.join(" ");
    let prompt = prompt.trim();
    if prompt.is_empty() {
        anyhow::bail!("agent prompt must not be empty");
    }

    ensure_background_agent_supported_platform()?;

    let agent_id = new_agent_id();
    let explicit_cwd = cmd.cwd.is_some();
    let cwd = resolve_agent_cwd(
        cmd.cwd,
        runtime_context.map(|context| context.cwd.as_path()),
    )?;
    let workspace_roots = agent_start_snapshot_workspace_roots(runtime_context, &cwd, explicit_cwd);
    let permission_profile =
        agent_start_snapshot_permission_profile(runtime_context, &cwd, explicit_cwd)?;
    let auth_profile_ref = runtime_context
        .and_then(|context| context.auth_profile_ref.as_deref())
        .or(auth_profile)
        .map(str::to_string);
    let prompt_snapshot_ref = format!("inline:{agent_id}:prompt");
    // The state DB is shared across many concurrent processes; every write
    // below retries transient SQLITE_BUSY / SQLITE_BUSY_SNAPSHOT contention
    // with backoff instead of failing the whole `agent start` invocation.
    let create_params = BackgroundAgentRunCreateParams {
        id: agent_id.clone(),
        idempotency_key: cmd.idempotency_key,
        request_id: None,
        source: "cli".to_string(),
        prompt_snapshot_ref: prompt_snapshot_ref.clone(),
        input_snapshot_ref: None,
        thread_id: None,
        thread_store_kind: "background-agent".to_string(),
        thread_store_id: None,
        rollout_path: None,
        parent_thread_id: None,
        parent_agent_run_id: None,
        spawn_linkage_json: None,
        auth_profile_ref: auth_profile_ref.clone(),
        status_reason: Some("queued by codewith agent start".to_string()),
        config_fingerprint: None,
        version_fingerprint: Some(BACKGROUND_AGENT_ADMISSION_SCHEMA_VERSION.to_string()),
    };
    retry_on_busy("reconcile stale background agents before admission", || {
        state_db.orphan_stale_background_agent_runs(Duration::from_secs(30))
    })
    .await
    .context("failed to reconcile stale background agents before admission")?;
    let (run, created) = retry_on_busy("admit background agent run", || {
        state_db
            .admit_background_agent_run(&create_params, DEFAULT_MAX_ACTIVE_BACKGROUND_AGENT_RUNS)
    })
    .await
    .context("failed to admit background agent")?;
    let admitted_agent_id = run.id.clone();
    let start_event_payload = json!({
        "cwd": cwd.display().to_string(),
        "prompt": prompt,
        "promptSnapshotRef": run.prompt_snapshot_ref,
    });
    let existing_start_event = if created {
        None
    } else {
        retry_on_busy("load background agent start event", || {
            state_db.list_background_agent_events_after(
                admitted_agent_id.as_str(),
                /*after_seq*/ None,
                Some(100),
            )
        })
        .await
        .context("failed to load background agent start event")?
        .into_iter()
        .find(|event| {
            matches!(
                event.event_type.as_str(),
                "agent.started" | "agent.startRecovered"
            )
        })
    };
    let event = match existing_start_event {
        Some(event) => event,
        None => {
            let event_type = if created {
                "agent.started"
            } else {
                "agent.startRecovered"
            };
            retry_on_busy("append background agent start event", || {
                state_db.append_background_agent_event(
                    admitted_agent_id.as_str(),
                    event_type,
                    &start_event_payload,
                )
            })
            .await
            .context("failed to append background agent start event")?
        }
    };
    let snapshot_params = BackgroundAgentExecutionSnapshotParams {
        run_id: admitted_agent_id.clone(),
        snapshot_kind: "initial_execution_context".to_string(),
        payload_json: json!({
            "snapshotSource": "codewith agent start",
            "cwd": cwd.display().to_string(),
            "workspaceRoots": workspace_roots,
            "authProfileRef": auth_profile_ref,
            "approvalPolicy": runtime_context
                .and_then(|context| context.approval_policy.as_ref()),
            "permissionProfile": permission_profile,
            "model": runtime_context.and_then(|context| context.model.as_deref()),
            "provider": runtime_context.and_then(|context| context.provider.as_deref()),
            "serviceTier": runtime_context
                .and_then(|context| context.service_tier.as_deref()),
            "recoveryPolicy": "abort_mid_turn_resume_at_safe_boundary",
        }),
        recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
        config_fingerprint: None,
    };
    let execution_snapshot_exists =
        retry_on_busy("load background agent execution snapshot", || {
            state_db.get_latest_background_agent_execution_snapshot(admitted_agent_id.as_str())
        })
        .await
        .context("failed to load background agent execution snapshot")?
        .is_some();
    if !execution_snapshot_exists {
        retry_on_busy("create background agent execution snapshot", || {
            state_db.create_background_agent_execution_snapshot(&snapshot_params)
        })
        .await
        .context("failed to create background agent execution snapshot")?;
    }
    let status_snapshot_exists = retry_on_busy("load background agent status snapshot", || {
        state_db.get_background_agent_status_snapshot(admitted_agent_id.as_str())
    })
    .await
    .context("failed to load background agent status snapshot")?
    .is_some();
    if !status_snapshot_exists {
        let status_snapshot_params = BackgroundAgentStatusSnapshotParams {
            run_id: admitted_agent_id.clone(),
            seq: event.seq,
            status: BackgroundAgentRunStatus::Queued,
            desired_state: BackgroundAgentDesiredState::Running,
            summary: Some("Queued".to_string()),
            pending_interaction_count: 0,
            last_event_seq: event.seq,
            payload_json: json!({"phase": "queued"}),
        };
        retry_on_busy("create background agent status snapshot", || {
            state_db.upsert_background_agent_status_snapshot(&status_snapshot_params)
        })
        .await
        .context("failed to create background agent status snapshot")?;
    }
    let daemon = background_agent_daemon()?;
    let daemon_output = daemon.start().await?;
    let run = retry_on_busy("reload admitted background agent", || {
        state_db.get_background_agent_run(admitted_agent_id.as_str())
    })
    .await?
    .unwrap_or(run);
    Ok(json!({ "agent": run_json(run), "created": created, "daemon": daemon_output }))
}

fn agent_start_snapshot_workspace_roots(
    runtime_context: Option<&AgentStartRuntimeContext>,
    cwd: &Path,
    explicit_cwd: bool,
) -> Option<Vec<String>> {
    if explicit_cwd {
        return Some(vec![cwd.display().to_string()]);
    }
    runtime_context.map(|context| {
        context
            .workspace_roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
    })
}

fn agent_start_snapshot_permission_profile(
    runtime_context: Option<&AgentStartRuntimeContext>,
    cwd: &Path,
    explicit_cwd: bool,
) -> anyhow::Result<Option<Value>> {
    let Some(context) = runtime_context else {
        return Ok(None);
    };
    let permission_profile = if explicit_cwd {
        let cwd = AbsolutePathBuf::from_absolute_path_checked(cwd)
            .with_context(|| format!("invalid background agent cwd: {}", cwd.display()))?;
        context
            .permission_profile
            .clone()
            .materialize_project_roots_with_workspace_roots(std::slice::from_ref(&cwd))
    } else {
        context.permission_profile.clone()
    };
    Ok(Some(serde_json::to_value(permission_profile)?))
}

async fn stop_agent(
    state_db: &StateRuntime,
    agent_id: &str,
) -> anyhow::Result<Option<BackgroundAgentRun>> {
    let Some(run) = state_db
        .get_background_agent_run(agent_id)
        .await
        .context("failed to read background agent before stop")?
    else {
        return Ok(None);
    };
    if !background_agent_status_is_terminal(run.status) {
        let mut observed = run;
        let mut stopped = false;
        let stop_diagnostics = json!({"reason": "cli_requested_stop"});
        for _ in 0..2 {
            let status_reason = if should_terminalize_unclaimed_agent_run(&observed) {
                "stop requested by codewith agent stop before worker claim"
            } else {
                "stop requested by codewith agent stop"
            };
            stopped = retry_on_busy("request fenced background agent stop", || {
                state_db.request_background_agent_stop_for_generation(
                    agent_id,
                    observed.supervisor_id.as_deref(),
                    observed.generation,
                    status_reason,
                    &stop_diagnostics,
                )
            })
            .await
            .context("failed to request background agent stop")?;
            if stopped {
                break;
            }
            let Some(latest) = state_db
                .get_background_agent_run(agent_id)
                .await
                .context("failed to reload background agent during stop")?
            else {
                return Ok(None);
            };
            if background_agent_status_is_terminal(latest.status) {
                stopped = true;
                break;
            }
            observed = latest;
        }
        if !stopped {
            anyhow::bail!("background agent ownership changed during stop request");
        }
    }
    state_db
        .get_background_agent_run(agent_id)
        .await
        .context("failed to read background agent after stop")
}

async fn diagnostics_json(state_db: &StateRuntime) -> anyhow::Result<Value> {
    let counts = state_db
        .count_background_agent_runs_by_status()
        .await
        .context("failed to count background agent runs")?;
    let pending_interaction_count = state_db
        .count_background_agent_pending_interactions(/*status*/ None)
        .await
        .context("failed to count background agent pending interactions")?;
    let max_active_runs_per_user = DEFAULT_MAX_ACTIVE_BACKGROUND_AGENT_RUNS;
    let active_run_count = counts
        .iter()
        .filter(|(status, _)| {
            matches!(
                status,
                BackgroundAgentRunStatus::Queued
                    | BackgroundAgentRunStatus::Starting
                    | BackgroundAgentRunStatus::Running
                    | BackgroundAgentRunStatus::WaitingOnApproval
                    | BackgroundAgentRunStatus::WaitingOnUser
                    | BackgroundAgentRunStatus::Stopping
                    | BackgroundAgentRunStatus::Orphaned
            )
        })
        .map(|(_, count)| *count)
        .sum::<i64>();
    let daemon_status = background_agent_daemon()?.status().await?;
    Ok(json!({
        "stateStoreAvailable": true,
        "daemon": daemon_status,
        "activeRunCount": active_run_count,
        "availableActiveRunSlots": max_active_runs_per_user.saturating_sub(active_run_count),
        "maxActiveRunsPerUser": max_active_runs_per_user,
        "admissionAllowed": active_run_count < max_active_runs_per_user,
        "pendingInteractionCount": pending_interaction_count,
        "runsByStatus": counts
            .into_iter()
            .map(|(status, count)| json!({"status": status.as_str(), "count": count}))
            .collect::<Vec<_>>(),
    }))
}

async fn state_runtime() -> anyhow::Result<std::sync::Arc<StateRuntime>> {
    let codex_home = find_codex_home().context("failed to resolve CODEWITH_HOME")?;
    StateRuntime::init(codex_home.to_path_buf(), "cli".to_string())
        .await
        .context("failed to initialize state runtime")
}

fn background_agent_daemon() -> anyhow::Result<BackgroundAgentDaemon> {
    let codex_home = find_codex_home().context("failed to resolve CODEWITH_HOME")?;
    let codex_bin = std::env::current_exe().context("failed to resolve current Codewith binary")?;
    Ok(BackgroundAgentDaemon::new(BackgroundAgentDaemonPaths::new(
        codex_bin,
        background_agent_daemon_state_dir(codex_home.as_path()),
    )))
}

fn resolve_agent_cwd(cwd: Option<PathBuf>, default_cwd: Option<&Path>) -> anyhow::Result<PathBuf> {
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => default_cwd
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir().context("failed to read current directory")?),
    };
    if cwd.is_absolute() {
        return Ok(cwd);
    }
    Ok(std::env::current_dir()
        .context("failed to read current directory")?
        .join(cwd))
}

fn should_terminalize_unclaimed_agent_run(run: &BackgroundAgentRun) -> bool {
    run.supervisor_id.is_none()
        || matches!(
            run.status,
            BackgroundAgentRunStatus::Queued | BackgroundAgentRunStatus::Orphaned
        )
}

fn background_agent_status_is_terminal(status: BackgroundAgentRunStatus) -> bool {
    matches!(
        status,
        BackgroundAgentRunStatus::Completed
            | BackgroundAgentRunStatus::Failed
            | BackgroundAgentRunStatus::Cancelled
    )
}

fn run_json(run: BackgroundAgentRun) -> Value {
    json!({
        "agentId": run.id,
        "idempotencyKey": run.idempotency_key,
        "source": run.source,
        "promptSnapshotRef": run.prompt_snapshot_ref,
        "threadId": run.thread_id,
        "threadStoreKind": run.thread_store_kind,
        "threadStoreId": run.thread_store_id,
        "rolloutPath": run.rollout_path,
        "parentThreadId": run.parent_thread_id,
        "parentAgentRunId": run.parent_agent_run_id,
        "authProfileRef": run.auth_profile_ref,
        "desiredState": run.desired_state.as_str(),
        "status": run.status.as_str(),
        "statusReason": run.status_reason,
        "configFingerprint": run.config_fingerprint,
        "versionFingerprint": run.version_fingerprint,
        "retentionState": run.retention_state.as_str(),
        "supervisorId": run.supervisor_id,
        "generation": run.generation,
        "pid": run.pid,
        "pgid": run.pgid,
        "jobId": run.job_id,
        "heartbeatAt": run.heartbeat_at.map(|value| value.timestamp()),
        "crashReason": run.crash_reason,
        "exitCode": run.exit_code,
        "exitSignal": run.exit_signal,
        "lastEventSeq": run.last_event_seq,
        "lastSnapshotSeq": run.last_snapshot_seq,
        "createdAt": run.created_at.timestamp(),
        "updatedAt": run.updated_at.timestamp(),
        "startedAt": run.started_at.map(|value| value.timestamp()),
        "completedAt": run.completed_at.map(|value| value.timestamp()),
    })
}

fn new_agent_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("cli-{nanos}-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use pretty_assertions::assert_eq;

    #[derive(Debug, Parser)]
    struct TestAgentCli {
        #[command(subcommand)]
        subcommand: AgentSubcommand,
    }

    #[test]
    fn payload_summary_prefers_short_human_fields() {
        let payload = json!({
            "summary": "Finished indexing",
            "message": "This longer message should not be used",
        });

        assert_eq!(payload_event_summary(Some(&payload)), "Finished indexing");
    }

    #[test]
    fn payload_summary_hides_unallowlisted_payload_fields() {
        let payload = json!({
            "prompt": "implement a private feature",
            "cwd": "/private/workspace",
        });

        assert_eq!(
            payload_event_summary(Some(&payload)),
            "(payload hidden; use --verbose)"
        );
    }

    #[test]
    fn compact_json_preview_truncates_large_payloads() {
        let payload = json!({
            "message": "x".repeat(400),
        });

        let preview = compact_json_preview(&payload, 80);

        assert!(preview.ends_with("..."));
        assert!(preview.len() <= 83);
    }

    #[test]
    fn start_parses_cwd_before_or_after_prompt() {
        let before =
            TestAgentCli::try_parse_from(["agent", "start", "--cwd", "/home/hasna", "print OK"])
                .expect("parse --cwd before prompt");
        let AgentSubcommand::Start(before) = before.subcommand else {
            panic!("expected start command");
        };
        assert_eq!(before.cwd, Some(PathBuf::from("/home/hasna")));
        assert_eq!(before.prompt, vec!["print OK".to_string()]);

        let after =
            TestAgentCli::try_parse_from(["agent", "start", "print OK", "--cwd", "/home/hasna"])
                .expect("parse --cwd after prompt");
        let AgentSubcommand::Start(after) = after.subcommand else {
            panic!("expected start command");
        };
        assert_eq!(after.cwd, Some(PathBuf::from("/home/hasna")));
        assert_eq!(after.prompt, vec!["print OK".to_string()]);
    }

    #[test]
    fn start_accepts_literal_flag_like_prompt_after_separator() {
        let parsed = TestAgentCli::try_parse_from(["agent", "start", "--", "--cwd", "/tmp"])
            .expect("parse literal prompt after separator");
        let AgentSubcommand::Start(parsed) = parsed.subcommand else {
            panic!("expected start command");
        };
        assert_eq!(parsed.cwd, None);
        assert_eq!(parsed.prompt, vec!["--cwd".to_string(), "/tmp".to_string()]);
    }

    #[test]
    fn explicit_cwd_snapshot_uses_requested_cwd_as_only_workspace_root() {
        let runtime_context = AgentStartRuntimeContext {
            cwd: PathBuf::from("/launcher"),
            workspace_roots: vec![PathBuf::from("/launcher"), PathBuf::from("/launcher/huge")],
            auth_profile_ref: Some("profile-a".to_string()),
            approval_policy: Some(json!("never")),
            permission_profile: PermissionProfile::workspace_write(),
            model: Some("model-a".to_string()),
            provider: Some("provider-a".to_string()),
            service_tier: None,
        };
        let requested_cwd = PathBuf::from("/target");
        let expected_cwd =
            AbsolutePathBuf::from_absolute_path_checked(&requested_cwd).expect("absolute test cwd");
        let expected_permission_profile = PermissionProfile::workspace_write()
            .materialize_project_roots_with_workspace_roots(std::slice::from_ref(&expected_cwd));

        assert_eq!(
            agent_start_snapshot_workspace_roots(
                Some(&runtime_context),
                requested_cwd.as_path(),
                /*explicit_cwd*/ true,
            ),
            Some(vec!["/target".to_string()])
        );
        assert_eq!(
            agent_start_snapshot_permission_profile(
                Some(&runtime_context),
                requested_cwd.as_path(),
                /*explicit_cwd*/ true,
            )
            .expect("serialize permission profile"),
            Some(serde_json::to_value(expected_permission_profile).expect("expected profile json"))
        );
    }

    #[test]
    fn explicit_cwd_snapshot_preserves_read_only_permission_profile() {
        let runtime_context = AgentStartRuntimeContext {
            cwd: PathBuf::from("/launcher"),
            workspace_roots: vec![PathBuf::from("/launcher")],
            auth_profile_ref: None,
            approval_policy: None,
            permission_profile: PermissionProfile::read_only(),
            model: None,
            provider: None,
            service_tier: None,
        };

        assert_eq!(
            agent_start_snapshot_permission_profile(
                Some(&runtime_context),
                Path::new("/target"),
                /*explicit_cwd*/ true,
            )
            .expect("serialize permission profile"),
            Some(
                serde_json::to_value(PermissionProfile::read_only())
                    .expect("expected profile json")
            )
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_context_from_config_keeps_workspace_permissions_symbolic_for_cwd_snapshot()
    -> anyhow::Result<()> {
        let codex_home = tempfile::TempDir::new()?;
        let launcher = tempfile::TempDir::new()?;
        let launcher_extra = tempfile::TempDir::new()?;
        let target = tempfile::TempDir::new()?;
        let config = codex_core::config::ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(codex_core::config::ConfigOverrides {
                cwd: Some(launcher.path().to_path_buf()),
                default_permissions: Some(":workspace".to_string()),
                additional_writable_roots: vec![launcher_extra.path().to_path_buf()],
                ..Default::default()
            })
            .build()
            .await?;
        let runtime_context = AgentStartRuntimeContext::from_config(&config);

        let permission_profile = agent_start_snapshot_permission_profile(
            Some(&runtime_context),
            target.path(),
            /*explicit_cwd*/ true,
        )?
        .expect("permission profile should be snapshotted");
        let permission_profile_json = serde_json::to_string(&permission_profile)?;

        assert!(
            permission_profile_json.contains(target.path().to_string_lossy().as_ref()),
            "expected target cwd in background agent permission profile: {permission_profile_json}",
        );
        assert!(
            !permission_profile_json.contains(launcher.path().to_string_lossy().as_ref()),
            "launcher cwd leaked into background agent permission profile: {permission_profile_json}",
        );
        assert!(
            !permission_profile_json.contains(launcher_extra.path().to_string_lossy().as_ref()),
            "launcher extra root leaked into background agent permission profile: {permission_profile_json}",
        );
        Ok(())
    }
}
