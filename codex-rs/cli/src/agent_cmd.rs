use anyhow::Context;
use clap::Args;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_background_agent::daemon::BackgroundAgentDaemon;
use codex_background_agent::daemon::BackgroundAgentDaemonPaths;
use codex_background_agent::daemon::background_agent_daemon_state_dir;
use codex_background_agent::daemon::ensure_supported_platform as ensure_background_agent_supported_platform;
use codex_core::config::find_codex_home;
use codex_state::BackgroundAgentDesiredState;
use codex_state::BackgroundAgentExecutionSnapshotParams;
use codex_state::BackgroundAgentPendingInteractionStatus;
use codex_state::BackgroundAgentRun;
use codex_state::BackgroundAgentRunCreateParams;
use codex_state::BackgroundAgentRunStatus;
use codex_state::BackgroundAgentStatusSnapshotParams;
use codex_state::StateRuntime;
use codex_state::busy_retry::retry_on_busy;

#[derive(Debug, Clone)]
pub(crate) struct AgentStartRuntimeContext {
    pub(crate) cwd: PathBuf,
    pub(crate) workspace_roots: Vec<PathBuf>,
    pub(crate) auth_profile_ref: Option<String>,
    pub(crate) approval_policy: Option<Value>,
    pub(crate) permission_profile: Option<Value>,
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
            permission_profile: serde_json::to_value(
                config.permissions.effective_permission_profile(),
            )
            .ok(),
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
    Diagnostics,
}

#[derive(Debug, Args)]
pub(crate) struct AgentStartCommand {
    /// Prompt to run in the background.
    #[arg(required = true, trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Idempotency key for retrying the same start request.
    #[arg(long = "idempotency-key")]
    idempotency_key: Option<String>,

    /// Working directory for the run. Defaults to the current directory.
    #[arg(long = "cwd")]
    cwd: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct AgentListCommand {
    /// Maximum number of runs to return.
    #[arg(long = "limit", default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Args)]
pub(crate) struct AgentIdCommand {
    /// Background-agent run id.
    agent_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct AgentLogsCommand {
    /// Background-agent run id.
    agent_id: String,

    /// Return events after this sequence number.
    #[arg(long = "after-seq")]
    after_seq: Option<i64>,

    /// Maximum number of events to return.
    #[arg(long = "limit", default_value_t = 100)]
    limit: usize,
}

pub(crate) async fn run_agent_command(
    cli: AgentCli,
    runtime_context: Option<AgentStartRuntimeContext>,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    let state_db = state_runtime().await?;
    let output = match cli.subcommand {
        AgentSubcommand::Start(cmd) => {
            start_agent(
                state_db.as_ref(),
                cmd,
                runtime_context.as_ref(),
                auth_profile,
            )
            .await?
        }
        AgentSubcommand::List(cmd) => {
            let runs = state_db
                .list_background_agent_runs(Some(cmd.limit))
                .await
                .context("failed to list background agents")?;
            json!({ "data": runs.into_iter().map(run_json).collect::<Vec<_>>() })
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
                .list_background_agent_pending_interactions(cmd.agent_id.as_str(), None)
                .await
                .context("failed to list background agent pending interactions")?;
            json!({
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
            })
        }
        AgentSubcommand::Attach(cmd) => attach_agent(state_db.as_ref(), cmd).await?,
        AgentSubcommand::Logs(cmd) => {
            let events = state_db
                .list_background_agent_events_after(
                    cmd.agent_id.as_str(),
                    cmd.after_seq,
                    Some(cmd.limit),
                )
                .await
                .context("failed to list background agent events")?;
            json!({
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
            })
        }
        AgentSubcommand::Stop(cmd) => {
            let run = stop_agent(state_db.as_ref(), cmd.agent_id.as_str()).await?;
            json!({ "agent": run.map(run_json) })
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
            json!({ "deleted": deleted, "agent": run.map(run_json) })
        }
        AgentSubcommand::Diagnostics => diagnostics_json(state_db.as_ref()).await?,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
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
    limit: usize,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Attach(AgentLogsCommand {
                agent_id,
                after_seq,
                limit,
            }),
        },
        None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_logs(
    agent_id: String,
    after_seq: Option<i64>,
    limit: usize,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Logs(AgentLogsCommand {
                agent_id,
                after_seq,
                limit,
            }),
        },
        None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_stop(
    agent_id: String,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Stop(AgentIdCommand { agent_id }),
        },
        None,
        auth_profile,
    )
    .await
}

pub(crate) async fn run_background_agent_delete(
    agent_id: String,
    auth_profile: Option<&str>,
) -> anyhow::Result<()> {
    run_agent_command(
        AgentCli {
            subcommand: AgentSubcommand::Delete(AgentIdCommand { agent_id }),
        },
        None,
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

async fn attach_agent(state_db: &StateRuntime, cmd: AgentLogsCommand) -> anyhow::Result<Value> {
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
        .list_background_agent_events_after(cmd.agent_id.as_str(), cmd.after_seq, Some(cmd.limit))
        .await
        .context("failed to list background agent events")?;
    let pending_interactions = state_db
        .list_background_agent_pending_interactions(cmd.agent_id.as_str(), None)
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
    if let Some(idempotency_key) = cmd.idempotency_key.as_deref()
        && let Some(run) = retry_on_busy("load background agent idempotency key", || {
            state_db.get_background_agent_run_by_idempotency_key(idempotency_key)
        })
        .await
        .context("failed to load background agent idempotency key")?
    {
        let daemon = background_agent_daemon()?;
        let daemon_output = daemon.start().await?;
        return Ok(json!({ "agent": run_json(run), "created": false, "daemon": daemon_output }));
    }

    ensure_background_agent_supported_platform()?;

    let agent_id = new_agent_id();
    let cwd = resolve_agent_cwd(
        cmd.cwd,
        runtime_context.map(|context| context.cwd.as_path()),
    )?;
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
        version_fingerprint: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    let run = retry_on_busy("create background agent run", || {
        state_db.create_background_agent_run(&create_params)
    })
    .await
    .context("failed to create background agent")?;
    let start_event_payload = json!({
        "cwd": cwd.display().to_string(),
        "prompt": prompt,
        "promptSnapshotRef": prompt_snapshot_ref,
    });
    let event = retry_on_busy("append background agent start event", || {
        state_db.append_background_agent_event(
            agent_id.as_str(),
            "agent.started",
            &start_event_payload,
        )
    })
    .await
    .context("failed to append background agent start event")?;
    let snapshot_params = BackgroundAgentExecutionSnapshotParams {
        run_id: agent_id.clone(),
        snapshot_kind: "initial_execution_context".to_string(),
        payload_json: json!({
            "snapshotSource": "codewith agent start",
            "cwd": cwd.display().to_string(),
            "workspaceRoots": runtime_context.map(|context| {
                context
                    .workspace_roots
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
            }),
            "authProfileRef": auth_profile_ref,
            "approvalPolicy": runtime_context
                .and_then(|context| context.approval_policy.as_ref()),
            "permissionProfile": runtime_context
                .and_then(|context| context.permission_profile.as_ref()),
            "model": runtime_context.and_then(|context| context.model.as_deref()),
            "provider": runtime_context.and_then(|context| context.provider.as_deref()),
            "serviceTier": runtime_context
                .and_then(|context| context.service_tier.as_deref()),
            "recoveryPolicy": "abort_mid_turn_resume_at_safe_boundary",
        }),
        recovery_policy: "abort_mid_turn_resume_at_safe_boundary".to_string(),
        config_fingerprint: None,
    };
    retry_on_busy("create background agent execution snapshot", || {
        state_db.create_background_agent_execution_snapshot(&snapshot_params)
    })
    .await
    .context("failed to create background agent execution snapshot")?;
    let status_snapshot_params = BackgroundAgentStatusSnapshotParams {
        run_id: agent_id,
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
    let daemon = background_agent_daemon()?;
    let daemon_output = daemon.start().await?;
    Ok(json!({ "agent": run_json(run), "created": true, "daemon": daemon_output }))
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
    state_db
        .set_background_agent_desired_state(agent_id, BackgroundAgentDesiredState::Stopped)
        .await
        .context("failed to update background agent desired state")?;
    if !background_agent_status_is_terminal(run.status) {
        let terminalize_immediately = should_terminalize_unclaimed_agent_run(&run);
        let status = if terminalize_immediately {
            BackgroundAgentRunStatus::Cancelled
        } else {
            BackgroundAgentRunStatus::Stopping
        };
        let status_reason = if terminalize_immediately {
            "stop requested by codewith agent stop before worker claim"
        } else {
            "stop requested by codewith agent stop"
        };
        state_db
            .update_background_agent_run_status(agent_id, status, Some(status_reason))
            .await
            .context("failed to update background agent status")?;
        state_db
            .append_background_agent_event(
                agent_id,
                "agent.stopRequested",
                &json!({"reason": "cli_requested_stop"}),
            )
            .await
            .context("failed to append background agent stop event")?;
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
        .count_background_agent_pending_interactions(None)
        .await
        .context("failed to count background agent pending interactions")?;
    let max_active_runs_per_user = 8_i64;
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
