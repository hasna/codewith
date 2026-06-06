use super::*;

const DEFAULT_MONITOR_LIMIT: usize = 50;
pub(super) const MAX_MONITOR_LIMIT: usize = 50;
const MAX_THREAD_MONITOR_NAME_CHARS: usize = 120;
const MAX_THREAD_MONITOR_PROMPT_CHARS: usize = 4_000;
const MAX_THREAD_MONITOR_COMMAND_CHARS: usize = 8_000;
const MAX_THREAD_MONITOR_PATH_CHARS: usize = 1_000;

pub(super) fn validate_monitor_name(name: &str) -> Result<String, JSONRPCErrorError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid_request("monitor name must not be empty"));
    }
    if name.chars().count() > MAX_THREAD_MONITOR_NAME_CHARS {
        return Err(invalid_request(format!(
            "monitor name must be at most {MAX_THREAD_MONITOR_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_string())
}

pub(super) fn validate_monitor_prompt(prompt: &str) -> Result<String, JSONRPCErrorError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(invalid_request("monitor prompt must not be empty"));
    }
    if prompt.chars().count() > MAX_THREAD_MONITOR_PROMPT_CHARS {
        return Err(invalid_request(format!(
            "monitor prompt must be at most {MAX_THREAD_MONITOR_PROMPT_CHARS} characters"
        )));
    }
    Ok(prompt.to_string())
}

pub(super) fn validate_monitor_command(command: &str) -> Result<String, JSONRPCErrorError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(invalid_request("monitor command must not be empty"));
    }
    if command.chars().count() > MAX_THREAD_MONITOR_COMMAND_CHARS {
        return Err(invalid_request(format!(
            "monitor command must be at most {MAX_THREAD_MONITOR_COMMAND_CHARS} characters"
        )));
    }
    Ok(command.to_string())
}

pub(super) fn validate_optional_monitor_path(
    field_name: &str,
    value: Option<String>,
) -> Result<Option<String>, JSONRPCErrorError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Err(invalid_request(format!("{field_name} must not be empty")));
            }
            if value.chars().count() > MAX_THREAD_MONITOR_PATH_CHARS {
                return Err(invalid_request(format!(
                    "{field_name} must be at most {MAX_THREAD_MONITOR_PATH_CHARS} characters"
                )));
            }
            Ok(value.to_string())
        })
        .transpose()
}

pub(super) fn validate_monitor_output_file_for_routing(
    routing: codex_state::ThreadMonitorRouting,
    output_file: Option<String>,
) -> Result<Option<String>, JSONRPCErrorError> {
    if routing.writes_to_file() && output_file.is_none() {
        return Err(invalid_request(
            "monitor outputFile is required when routing is file or both",
        ));
    }
    if !routing.writes_to_file() && output_file.is_some() {
        return Err(invalid_request(
            "monitor outputFile is only valid when routing is file or both",
        ));
    }
    Ok(output_file)
}

pub(super) fn normalize_monitor_list_limit(limit: Option<u32>) -> Result<usize, JSONRPCErrorError> {
    let limit = limit.unwrap_or(DEFAULT_MONITOR_LIMIT as u32);
    if limit == 0 {
        return Err(invalid_request("monitor list limit must be positive"));
    }
    Ok((limit as usize).min(MAX_MONITOR_LIMIT))
}

pub(super) fn decode_monitor_cursor(cursor: Option<&str>) -> Result<usize, JSONRPCErrorError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    cursor
        .parse::<usize>()
        .map_err(|_| invalid_request("monitor list cursor is invalid"))
}

pub(super) fn api_thread_monitor_routing_to_state(
    routing: ThreadMonitorRouting,
) -> codex_state::ThreadMonitorRouting {
    match routing {
        ThreadMonitorRouting::Stream => codex_state::ThreadMonitorRouting::Stream,
        ThreadMonitorRouting::File => codex_state::ThreadMonitorRouting::File,
        ThreadMonitorRouting::Both => codex_state::ThreadMonitorRouting::Both,
    }
}

fn thread_monitor_routing_from_state(
    routing: codex_state::ThreadMonitorRouting,
) -> ThreadMonitorRouting {
    match routing {
        codex_state::ThreadMonitorRouting::Stream => ThreadMonitorRouting::Stream,
        codex_state::ThreadMonitorRouting::File => ThreadMonitorRouting::File,
        codex_state::ThreadMonitorRouting::Both => ThreadMonitorRouting::Both,
    }
}

pub(super) fn thread_monitor_status_from_state(
    status: codex_state::ThreadMonitorStatus,
) -> ThreadMonitorStatus {
    match status {
        codex_state::ThreadMonitorStatus::Running => ThreadMonitorStatus::Running,
        codex_state::ThreadMonitorStatus::Stopped => ThreadMonitorStatus::Stopped,
        codex_state::ThreadMonitorStatus::Failed => ThreadMonitorStatus::Failed,
    }
}

pub(super) fn thread_monitor_event_stream_from_state(
    stream: codex_state::ThreadMonitorEventStream,
) -> ThreadMonitorEventStream {
    match stream {
        codex_state::ThreadMonitorEventStream::Stdout => ThreadMonitorEventStream::Stdout,
        codex_state::ThreadMonitorEventStream::Stderr => ThreadMonitorEventStream::Stderr,
        codex_state::ThreadMonitorEventStream::System => ThreadMonitorEventStream::System,
    }
}

pub(super) fn api_thread_monitor_from_state(monitor: codex_state::ThreadMonitor) -> ThreadMonitor {
    ThreadMonitor {
        thread_id: monitor.thread_id.to_string(),
        monitor_id: monitor.monitor_id,
        name: monitor.name,
        prompt: monitor.prompt,
        command: monitor.command,
        cwd: monitor.cwd,
        routing: thread_monitor_routing_from_state(monitor.routing),
        output_file: monitor.output_file,
        status: thread_monitor_status_from_state(monitor.status),
        generation: monitor.generation,
        process_id: monitor.process_id,
        last_event_at: monitor.last_event_at.map(|datetime| datetime.timestamp()),
        last_error: monitor.last_error,
        created_at: monitor.created_at.timestamp(),
        updated_at: monitor.updated_at.timestamp(),
    }
}

pub(super) fn api_thread_monitor_event_from_state(
    event: codex_state::ThreadMonitorEvent,
) -> ThreadMonitorEvent {
    ThreadMonitorEvent {
        thread_id: event.thread_id.to_string(),
        monitor_id: event.monitor_id,
        event_id: event.event_id,
        stream: thread_monitor_event_stream_from_state(event.stream),
        text: event.text,
        created_at: event.created_at.timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn validates_monitor_fields() {
        assert_eq!(
            "CI watcher",
            validate_monitor_name("  CI watcher  ").expect("name should be valid")
        );
        assert!(validate_monitor_name(" ").is_err());
        assert!(validate_monitor_name(&"x".repeat(MAX_THREAD_MONITOR_NAME_CHARS + 1)).is_err());

        assert_eq!(
            "tail a file",
            validate_monitor_prompt(" tail a file ").expect("prompt should be valid")
        );
        assert!(validate_monitor_prompt(" ").is_err());

        assert_eq!(
            "tail -f log",
            validate_monitor_command(" tail -f log ").expect("command should be valid")
        );
        assert!(validate_monitor_command(" ").is_err());
    }

    #[test]
    fn requires_output_file_for_file_routing() {
        assert!(validate_monitor_output_file_for_routing(
            codex_state::ThreadMonitorRouting::File,
            None,
        )
        .is_err());
        assert!(
            validate_monitor_output_file_for_routing(
                codex_state::ThreadMonitorRouting::Stream,
                Some("monitor.log".to_string()),
            )
            .is_err()
        );
        assert_eq!(
            Some("monitor.log".to_string()),
            validate_monitor_output_file_for_routing(
                codex_state::ThreadMonitorRouting::Both,
                Some("monitor.log".to_string()),
            )
            .expect("output file should be valid")
        );
    }
}
