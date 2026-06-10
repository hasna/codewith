//! Built-in model tool handler for managing thread monitors.

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::monitor_control_spec::MANAGE_MONITOR_TOOL_NAME;
use crate::tools::handlers::monitor_control_spec::create_manage_monitor_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::ThreadId;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;

const MAX_THREAD_MONITOR_NAME_CHARS: usize = 120;
const MAX_THREAD_MONITOR_PROMPT_CHARS: usize = 4_000;
const MAX_THREAD_MONITOR_COMMAND_CHARS: usize = 8_000;
const MAX_THREAD_MONITOR_PATH_CHARS: usize = 1_000;
const MAX_THREAD_MONITORS: usize = 50;
const DEFAULT_MONITOR_EVENT_LIMIT: usize = 50;
const MAX_MONITOR_EVENT_LIMIT: usize = 200;

pub struct ManageMonitorHandler;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ManageMonitorArgs {
    action: MonitorAction,
    monitor_id: Option<String>,
    name: Option<String>,
    prompt: Option<String>,
    command: Option<String>,
    cwd: Option<String>,
    routing: Option<MonitorRoutingArg>,
    output_file: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum MonitorAction {
    Create,
    List,
    Read,
    Stop,
    Restart,
    #[serde(alias = "clear", alias = "remove")]
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MonitorRoutingArg {
    Stream,
    File,
    Both,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageMonitorResponse {
    action: MonitorAction,
    monitor_id: Option<String>,
    affected_monitor: Option<MonitorSnapshot>,
    monitors: Vec<MonitorSnapshot>,
    events: Vec<MonitorEventSnapshot>,
    deleted: Option<bool>,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorSnapshot {
    thread_id: String,
    monitor_id: String,
    name: String,
    prompt: String,
    command: String,
    cwd: Option<String>,
    routing: String,
    output_file: Option<String>,
    status: String,
    generation: i64,
    process_id: Option<i64>,
    last_event_at: Option<i64>,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorEventSnapshot {
    thread_id: String,
    monitor_id: String,
    event_id: String,
    stream: String,
    text: String,
    created_at: i64,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for ManageMonitorHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MANAGE_MONITOR_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_manage_monitor_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session, payload, ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "manage_monitor handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ManageMonitorArgs = parse_arguments(&arguments)?;
        let state_db = session.state_db().ok_or_else(|| {
            FunctionCallError::Fatal("sqlite state db is unavailable for this session".to_string())
        })?;
        let response = manage_monitor(state_db, session.thread_id(), args).await?;
        monitor_response(response).map(boxed_tool_output)
    }
}

impl CoreToolRuntime for ManageMonitorHandler {}

async fn manage_monitor(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageMonitorArgs,
) -> Result<ManageMonitorResponse, FunctionCallError> {
    match args.action {
        MonitorAction::Create => create_monitor(state_db, thread_id, args).await,
        MonitorAction::List => {
            let monitors = list_monitor_snapshots(&state_db, thread_id).await?;
            Ok(ManageMonitorResponse {
                action: MonitorAction::List,
                monitor_id: None,
                affected_monitor: None,
                monitors,
                events: Vec::new(),
                deleted: None,
                message: "Listed monitors for this thread.".to_string(),
            })
        }
        MonitorAction::Read => read_monitor(state_db, thread_id, args).await,
        MonitorAction::Stop => set_monitor_stopped(state_db, thread_id, args).await,
        MonitorAction::Restart => restart_monitor(state_db, thread_id, args).await,
        MonitorAction::Delete => delete_monitor(state_db, thread_id, args).await,
    }
}

async fn create_monitor(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageMonitorArgs,
) -> Result<ManageMonitorResponse, FunctionCallError> {
    ensure_monitor_capacity(&state_db, thread_id).await?;
    let name = validate_monitor_text(
        "name",
        args.name
            .as_deref()
            .ok_or_else(|| model_error("name is required when action is create"))?,
        MAX_THREAD_MONITOR_NAME_CHARS,
    )?;
    let prompt = validate_monitor_text(
        "prompt",
        args.prompt
            .as_deref()
            .ok_or_else(|| model_error("prompt is required when action is create"))?,
        MAX_THREAD_MONITOR_PROMPT_CHARS,
    )?;
    let command = validate_monitor_text(
        "command",
        args.command
            .as_deref()
            .ok_or_else(|| model_error("command is required when action is create"))?,
        MAX_THREAD_MONITOR_COMMAND_CHARS,
    )?;
    let cwd = validate_optional_monitor_path("cwd", args.cwd.as_deref())?;
    let routing = args
        .routing
        .map(monitor_routing_arg_to_state)
        .unwrap_or(codex_state::ThreadMonitorRouting::Stream);
    let output_file = validate_optional_monitor_path("output_file", args.output_file.as_deref())?;
    let output_file = validate_output_file_for_routing(routing, output_file)?;
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
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to create thread monitor: {err}"))
        })?;
    let snapshot = MonitorSnapshot::from(monitor);
    Ok(ManageMonitorResponse {
        action: MonitorAction::Create,
        monitor_id: Some(snapshot.monitor_id.clone()),
        affected_monitor: Some(snapshot.clone()),
        monitors: vec![snapshot],
        events: Vec::new(),
        deleted: None,
        message: "Created monitor. The app-server runtime will start the command shortly."
            .to_string(),
    })
}

async fn read_monitor(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageMonitorArgs,
) -> Result<ManageMonitorResponse, FunctionCallError> {
    let monitor_id = resolve_monitor_id(&state_db, thread_id, args.monitor_id.as_deref()).await?;
    let monitor = load_monitor_for_thread(&state_db, thread_id, monitor_id.as_str()).await?;
    let limit = args
        .limit
        .unwrap_or(DEFAULT_MONITOR_EVENT_LIMIT)
        .min(MAX_MONITOR_EVENT_LIMIT);
    let events = state_db
        .thread_monitors()
        .list_thread_monitor_events(monitor_id.as_str(), /*offset*/ 0, limit)
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to read monitor events: {err}")))?
        .into_iter()
        .map(MonitorEventSnapshot::from)
        .collect::<Vec<_>>();
    let snapshot = MonitorSnapshot::from(monitor);
    Ok(ManageMonitorResponse {
        action: MonitorAction::Read,
        monitor_id: Some(monitor_id),
        affected_monitor: Some(snapshot.clone()),
        monitors: vec![snapshot],
        events,
        deleted: None,
        message: "Read monitor output events.".to_string(),
    })
}

async fn set_monitor_stopped(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageMonitorArgs,
) -> Result<ManageMonitorResponse, FunctionCallError> {
    let monitor_id = resolve_monitor_id(&state_db, thread_id, args.monitor_id.as_deref()).await?;
    load_monitor_for_thread(&state_db, thread_id, monitor_id.as_str()).await?;
    let monitor = state_db
        .thread_monitors()
        .set_thread_monitor_status(
            monitor_id.as_str(),
            codex_state::ThreadMonitorStatus::Stopped,
            /*last_error*/ None,
        )
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to stop monitor: {err}")))?
        .ok_or_else(|| model_error(format!("monitor not found: {monitor_id}")))?;
    let snapshot = MonitorSnapshot::from(monitor);
    Ok(ManageMonitorResponse {
        action: MonitorAction::Stop,
        monitor_id: Some(monitor_id),
        affected_monitor: Some(snapshot.clone()),
        monitors: vec![snapshot],
        events: Vec::new(),
        deleted: None,
        message: "Stopped monitor.".to_string(),
    })
}

async fn restart_monitor(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageMonitorArgs,
) -> Result<ManageMonitorResponse, FunctionCallError> {
    let monitor_id = resolve_monitor_id(&state_db, thread_id, args.monitor_id.as_deref()).await?;
    load_monitor_for_thread(&state_db, thread_id, monitor_id.as_str()).await?;
    let monitor = state_db
        .thread_monitors()
        .restart_thread_monitor(monitor_id.as_str())
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to restart monitor: {err}")))?
        .ok_or_else(|| model_error(format!("monitor not found: {monitor_id}")))?;
    let snapshot = MonitorSnapshot::from(monitor);
    Ok(ManageMonitorResponse {
        action: MonitorAction::Restart,
        monitor_id: Some(monitor_id),
        affected_monitor: Some(snapshot.clone()),
        monitors: vec![snapshot],
        events: Vec::new(),
        deleted: None,
        message: "Restarted monitor. The app-server runtime will rerun the command shortly."
            .to_string(),
    })
}

async fn delete_monitor(
    state_db: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    args: ManageMonitorArgs,
) -> Result<ManageMonitorResponse, FunctionCallError> {
    let monitor_id = resolve_monitor_id(&state_db, thread_id, args.monitor_id.as_deref()).await?;
    load_monitor_for_thread(&state_db, thread_id, monitor_id.as_str()).await?;
    let deleted = state_db
        .thread_monitors()
        .delete_thread_monitor(monitor_id.as_str())
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to delete monitor: {err}")))?;
    Ok(ManageMonitorResponse {
        action: MonitorAction::Delete,
        monitor_id: Some(monitor_id),
        affected_monitor: None,
        monitors: Vec::new(),
        events: Vec::new(),
        deleted: Some(deleted),
        message: "Deleted monitor.".to_string(),
    })
}

async fn ensure_monitor_capacity(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<(), FunctionCallError> {
    let monitors = state_db
        .thread_monitors()
        .list_thread_monitors(thread_id)
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to count monitors: {err}")))?;
    if monitors.len() >= MAX_THREAD_MONITORS {
        return Err(model_error(format!(
            "a thread can have at most {MAX_THREAD_MONITORS} monitors"
        )));
    }
    Ok(())
}

async fn list_monitor_snapshots(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<Vec<MonitorSnapshot>, FunctionCallError> {
    state_db
        .thread_monitors()
        .list_thread_monitors(thread_id)
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to list monitors: {err}")))
        .map(|monitors| monitors.into_iter().map(MonitorSnapshot::from).collect())
}

async fn resolve_monitor_id(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    monitor_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let monitors = state_db
        .thread_monitors()
        .list_thread_monitors(thread_id)
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to list monitors: {err}")))?;
    match monitor_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(monitor_id) => resolve_monitor_id_from_list(&monitors, monitor_id),
        None => match monitors.as_slice() {
            [monitor] => Ok(monitor.monitor_id.clone()),
            [] => Err(model_error("no monitors exist in this thread")),
            _ => Err(model_error(
                "monitor_id is required because multiple monitors exist in this thread",
            )),
        },
    }
}

fn resolve_monitor_id_from_list(
    monitors: &[codex_state::ThreadMonitor],
    monitor_id: &str,
) -> Result<String, FunctionCallError> {
    if let Some(monitor) = monitors
        .iter()
        .find(|monitor| monitor.monitor_id == monitor_id)
    {
        return Ok(monitor.monitor_id.clone());
    }
    let matches = monitors
        .iter()
        .filter(|monitor| monitor.monitor_id.starts_with(monitor_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [monitor] => Ok(monitor.monitor_id.clone()),
        [] => Err(model_error(format!("monitor not found: {monitor_id}"))),
        _ => Err(model_error(format!(
            "monitor id prefix is ambiguous: {monitor_id}"
        ))),
    }
}

async fn load_monitor_for_thread(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    monitor_id: &str,
) -> Result<codex_state::ThreadMonitor, FunctionCallError> {
    let monitor = state_db
        .thread_monitors()
        .get_thread_monitor(monitor_id)
        .await
        .map_err(|err| FunctionCallError::Fatal(format!("failed to read monitor: {err}")))?;
    let Some(monitor) = monitor else {
        return Err(model_error(format!("monitor not found: {monitor_id}")));
    };
    if monitor.thread_id != thread_id {
        return Err(model_error(format!("monitor not found: {monitor_id}")));
    }
    Ok(monitor)
}

fn validate_monitor_text(
    field_name: &str,
    value: &str,
    max_chars: usize,
) -> Result<String, FunctionCallError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(model_error(format!("{field_name} must not be empty")));
    }
    if value.chars().count() > max_chars {
        return Err(model_error(format!(
            "{field_name} must be at most {max_chars} characters"
        )));
    }
    Ok(value.to_string())
}

fn validate_optional_monitor_path(
    field_name: &str,
    value: Option<&str>,
) -> Result<Option<String>, FunctionCallError> {
    value
        .map(|value| validate_monitor_text(field_name, value, MAX_THREAD_MONITOR_PATH_CHARS))
        .transpose()
}

fn validate_output_file_for_routing(
    routing: codex_state::ThreadMonitorRouting,
    output_file: Option<String>,
) -> Result<Option<String>, FunctionCallError> {
    if routing.writes_to_file() && output_file.is_none() {
        return Err(model_error(
            "output_file is required when routing is file or both",
        ));
    }
    if !routing.writes_to_file() && output_file.is_some() {
        return Err(model_error(
            "output_file is only valid when routing is file or both",
        ));
    }
    Ok(output_file)
}

fn monitor_routing_arg_to_state(routing: MonitorRoutingArg) -> codex_state::ThreadMonitorRouting {
    match routing {
        MonitorRoutingArg::Stream => codex_state::ThreadMonitorRouting::Stream,
        MonitorRoutingArg::File => codex_state::ThreadMonitorRouting::File,
        MonitorRoutingArg::Both => codex_state::ThreadMonitorRouting::Both,
    }
}

fn monitor_response(
    response: ManageMonitorResponse,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let content = serde_json::to_string_pretty(&response)
        .map_err(|err| FunctionCallError::Fatal(format!("failed to serialize response: {err}")))?;
    Ok(FunctionToolOutput::from_text(content, Some(true)))
}

fn model_error(message: impl Into<String>) -> FunctionCallError {
    FunctionCallError::RespondToModel(message.into())
}

impl From<codex_state::ThreadMonitor> for MonitorSnapshot {
    fn from(monitor: codex_state::ThreadMonitor) -> Self {
        Self {
            thread_id: monitor.thread_id.to_string(),
            monitor_id: monitor.monitor_id,
            name: monitor.name,
            prompt: monitor.prompt,
            command: monitor.command,
            cwd: monitor.cwd,
            routing: monitor.routing.as_str().to_string(),
            output_file: monitor.output_file,
            status: monitor.status.as_str().to_string(),
            generation: monitor.generation,
            process_id: monitor.process_id,
            last_event_at: monitor.last_event_at.map(|datetime| datetime.timestamp()),
            last_error: monitor.last_error,
            created_at: monitor.created_at.timestamp(),
            updated_at: monitor.updated_at.timestamp(),
        }
    }
}

impl From<codex_state::ThreadMonitorEvent> for MonitorEventSnapshot {
    fn from(event: codex_state::ThreadMonitorEvent) -> Self {
        Self {
            thread_id: event.thread_id.to_string(),
            monitor_id: event.monitor_id,
            event_id: event.event_id,
            stream: event.stream.as_str().to_string(),
            text: event.text,
            created_at: event.created_at.timestamp(),
        }
    }
}
