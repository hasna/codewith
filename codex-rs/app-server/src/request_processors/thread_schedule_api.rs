use super::*;

const DEFAULT_SCHEDULE_LIMIT: usize = 50;
pub(super) const MAX_SCHEDULE_LIMIT: usize = 50;
const MAX_THREAD_SCHEDULE_PROMPT_CHARS: usize = 4_000;

pub(super) fn validate_schedule_prompt(prompt: &str) -> Result<String, JSONRPCErrorError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(invalid_request("schedule prompt must not be empty"));
    }
    if prompt.chars().count() > MAX_THREAD_SCHEDULE_PROMPT_CHARS {
        return Err(invalid_request(format!(
            "schedule prompt must be at most {MAX_THREAD_SCHEDULE_PROMPT_CHARS} characters"
        )));
    }
    Ok(prompt.to_string())
}

pub(super) fn normalize_timezone(timezone: Option<String>) -> Result<String, JSONRPCErrorError> {
    timezone.map_or_else(
        || {
            let timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
            normalize_timezone_value(timezone)
        },
        normalize_timezone_value,
    )
}

pub(super) fn normalize_timezone_value(timezone: String) -> Result<String, JSONRPCErrorError> {
    let timezone = timezone.trim();
    if timezone.is_empty() {
        return Err(invalid_request("schedule timezone must not be empty"));
    }
    thread_schedule_runtime::normalize_schedule_timezone(timezone)
        .map_err(|err| invalid_request(err.to_string()))
}

pub(super) fn timestamp_to_datetime(
    value: i64,
    field_name: &str,
) -> Result<DateTime<Utc>, JSONRPCErrorError> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| invalid_request(format!("{field_name} must be a valid Unix timestamp")))
}

pub(super) fn validate_schedule_expiry(
    next_run_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), JSONRPCErrorError> {
    if let (Some(next_run_at), Some(expires_at)) = (next_run_at, expires_at)
        && expires_at <= next_run_at
    {
        return Err(invalid_request(
            "schedule expiresAt must be later than nextRunAt",
        ));
    }
    Ok(())
}

pub(super) fn normalize_list_limit(limit: Option<u32>) -> Result<usize, JSONRPCErrorError> {
    let limit = limit.unwrap_or(DEFAULT_SCHEDULE_LIMIT as u32);
    if limit == 0 {
        return Err(invalid_request("schedule list limit must be positive"));
    }
    Ok((limit as usize).min(MAX_SCHEDULE_LIMIT))
}

pub(super) fn decode_cursor(cursor: Option<&str>) -> Result<usize, JSONRPCErrorError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    cursor
        .parse::<usize>()
        .map_err(|_| invalid_request("schedule list cursor is invalid"))
}

pub(super) fn parse_thread_id_for_request(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}

pub(super) fn api_thread_schedule_status_to_state(
    status: ThreadScheduleStatus,
) -> codex_state::ThreadScheduleStatus {
    match status {
        ThreadScheduleStatus::Active => codex_state::ThreadScheduleStatus::Active,
        ThreadScheduleStatus::Paused => codex_state::ThreadScheduleStatus::Paused,
        ThreadScheduleStatus::Expired => codex_state::ThreadScheduleStatus::Expired,
    }
}

fn thread_schedule_status_from_state(
    status: codex_state::ThreadScheduleStatus,
) -> ThreadScheduleStatus {
    match status {
        codex_state::ThreadScheduleStatus::Active => ThreadScheduleStatus::Active,
        codex_state::ThreadScheduleStatus::Paused => ThreadScheduleStatus::Paused,
        codex_state::ThreadScheduleStatus::Expired => ThreadScheduleStatus::Expired,
    }
}

fn thread_schedule_prompt_source_from_state(
    source: codex_state::ThreadSchedulePromptSource,
) -> ThreadSchedulePromptSource {
    match source {
        codex_state::ThreadSchedulePromptSource::Inline => ThreadSchedulePromptSource::Inline,
        codex_state::ThreadSchedulePromptSource::Default => ThreadSchedulePromptSource::Default,
    }
}

fn api_thread_schedule_interval_unit_to_state(
    unit: ThreadScheduleIntervalUnit,
) -> codex_state::ThreadScheduleIntervalUnit {
    match unit {
        ThreadScheduleIntervalUnit::Minutes => codex_state::ThreadScheduleIntervalUnit::Minutes,
        ThreadScheduleIntervalUnit::Hours => codex_state::ThreadScheduleIntervalUnit::Hours,
        ThreadScheduleIntervalUnit::Days => codex_state::ThreadScheduleIntervalUnit::Days,
    }
}

fn thread_schedule_interval_unit_from_state(
    unit: codex_state::ThreadScheduleIntervalUnit,
) -> ThreadScheduleIntervalUnit {
    match unit {
        codex_state::ThreadScheduleIntervalUnit::Minutes => ThreadScheduleIntervalUnit::Minutes,
        codex_state::ThreadScheduleIntervalUnit::Hours => ThreadScheduleIntervalUnit::Hours,
        codex_state::ThreadScheduleIntervalUnit::Days => ThreadScheduleIntervalUnit::Days,
    }
}

pub(super) fn api_thread_schedule_spec_to_state(
    schedule: ThreadScheduleSpec,
) -> Result<codex_state::ThreadScheduleSpec, JSONRPCErrorError> {
    match schedule {
        ThreadScheduleSpec::Once => Ok(codex_state::ThreadScheduleSpec::Once),
        ThreadScheduleSpec::Dynamic => Ok(codex_state::ThreadScheduleSpec::Dynamic),
        ThreadScheduleSpec::Interval { amount, unit } => {
            if amount <= 0 {
                return Err(invalid_request("interval amount must be positive"));
            }
            Ok(codex_state::ThreadScheduleSpec::Interval(
                codex_state::ThreadScheduleInterval {
                    amount,
                    unit: api_thread_schedule_interval_unit_to_state(unit),
                },
            ))
        }
        ThreadScheduleSpec::Cron { expression } => {
            let expression = validate_cron_expression(expression.as_str())?;
            Ok(codex_state::ThreadScheduleSpec::Cron { expression })
        }
    }
}

fn thread_schedule_spec_from_state(
    schedule: codex_state::ThreadScheduleSpec,
) -> ThreadScheduleSpec {
    match schedule {
        codex_state::ThreadScheduleSpec::Once => ThreadScheduleSpec::Once,
        codex_state::ThreadScheduleSpec::Dynamic => ThreadScheduleSpec::Dynamic,
        codex_state::ThreadScheduleSpec::Interval(interval) => ThreadScheduleSpec::Interval {
            amount: interval.amount,
            unit: thread_schedule_interval_unit_from_state(interval.unit),
        },
        codex_state::ThreadScheduleSpec::Cron { expression } => {
            ThreadScheduleSpec::Cron { expression }
        }
    }
}

fn validate_cron_expression(expression: &str) -> Result<String, JSONRPCErrorError> {
    let expression = expression.trim();
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(invalid_request(
            "cron schedule must contain exactly five fields",
        ));
    }
    let ranges = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
    for (field, (min, max)) in fields.iter().zip(ranges) {
        validate_cron_field(field, min, max)?;
    }
    Ok(fields.join(" "))
}

fn validate_cron_field(field: &str, min: u32, max: u32) -> Result<(), JSONRPCErrorError> {
    if field.is_empty()
        || !field
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '*' | '/' | '-' | ','))
    {
        return Err(invalid_request(
            "cron fields may only use digits, *, /, -, and ,",
        ));
    }
    for part in field.split(',') {
        validate_cron_part(part, min, max)?;
    }
    Ok(())
}

fn validate_cron_part(part: &str, min: u32, max: u32) -> Result<(), JSONRPCErrorError> {
    let (base, step) = part
        .split_once('/')
        .map_or((part, None), |(base, step)| (base, Some(step)));
    if let Some(step) = step {
        let step = parse_cron_field_value(step, min, max)?;
        if step == 0 {
            return Err(invalid_request("cron step must be positive"));
        }
    }
    if base == "*" {
        return Ok(());
    }
    if let Some((start, end)) = base.split_once('-') {
        let start = parse_cron_field_value(start, min, max)?;
        let end = parse_cron_field_value(end, min, max)?;
        if start > end {
            return Err(invalid_request("cron range start must be before end"));
        }
        return Ok(());
    }
    parse_cron_field_value(base, min, max).map(|_| ())
}

fn parse_cron_field_value(value: &str, min: u32, max: u32) -> Result<u32, JSONRPCErrorError> {
    let value = value
        .parse::<u32>()
        .map_err(|_| invalid_request("cron fields may only contain numeric ranges"))?;
    if value < min || value > max {
        return Err(invalid_request(format!(
            "cron field value {value} is outside {min}..={max}"
        )));
    }
    Ok(value)
}

pub(super) fn api_thread_schedule_from_state(
    schedule: codex_state::ThreadSchedule,
) -> ThreadSchedule {
    ThreadSchedule {
        thread_id: schedule.thread_id.to_string(),
        schedule_id: schedule.schedule_id,
        prompt: schedule.prompt,
        prompt_source: thread_schedule_prompt_source_from_state(schedule.prompt_source),
        schedule: thread_schedule_spec_from_state(schedule.schedule),
        timezone: schedule.timezone,
        status: thread_schedule_status_from_state(schedule.status),
        next_run_at: schedule.next_run_at.map(|datetime| datetime.timestamp()),
        last_run_at: schedule.last_run_at.map(|datetime| datetime.timestamp()),
        expires_at: schedule.expires_at.map(|datetime| datetime.timestamp()),
        failure_count: schedule.failure_count,
        lease_expires_at: schedule
            .lease_expires_at
            .map(|datetime| datetime.timestamp()),
        created_at: schedule.created_at.timestamp(),
        updated_at: schedule.updated_at.timestamp(),
    }
}

pub(super) fn api_thread_schedule_run_from_state(
    run: codex_state::ThreadScheduleRun,
) -> ThreadScheduleRun {
    ThreadScheduleRun {
        thread_id: run.thread_id.to_string(),
        schedule_id: run.schedule_id,
        run_id: run.run_id,
        status: thread_schedule_run_status_from_state(run.status),
        lease_id: run.lease_id,
        turn_id: run.turn_id,
        error: run.error,
        scheduled_for_at: run.scheduled_for.map(|datetime| datetime.timestamp()),
        started_at: run.started_at.timestamp(),
        completed_at: run.completed_at.map(|datetime| datetime.timestamp()),
    }
}

fn thread_schedule_run_status_from_state(
    status: codex_state::ThreadScheduleRunStatus,
) -> ThreadScheduleRunStatus {
    match status {
        codex_state::ThreadScheduleRunStatus::Leased => ThreadScheduleRunStatus::Leased,
        codex_state::ThreadScheduleRunStatus::Running => ThreadScheduleRunStatus::Running,
        codex_state::ThreadScheduleRunStatus::Completed => ThreadScheduleRunStatus::Completed,
        codex_state::ThreadScheduleRunStatus::Failed => ThreadScheduleRunStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn validates_schedule_prompt() {
        assert_eq!(
            "check the deploy",
            validate_schedule_prompt("  check the deploy  ").expect("prompt should be valid")
        );
        assert!(validate_schedule_prompt("   ").is_err());
        assert!(
            validate_schedule_prompt(&"x".repeat(MAX_THREAD_SCHEDULE_PROMPT_CHARS + 1)).is_err()
        );
    }

    #[test]
    fn validates_numeric_five_field_cron_expressions() {
        assert_eq!(
            "*/5 9-17 * * 1-5",
            validate_cron_expression("  */5 9-17 * * 1-5  ").expect("cron should be valid")
        );
        assert!(validate_cron_expression("*/5 9-17 * *").is_err());
        assert!(validate_cron_expression("0 25 * * *").is_err());
        assert!(validate_cron_expression("0 9 * jan *").is_err());
        assert!(validate_cron_expression("*/0 * * * *").is_err());
        assert!(validate_cron_expression("10-5 * * * *").is_err());
    }

    #[test]
    fn maps_api_schedule_spec_to_state() {
        assert_eq!(
            codex_state::ThreadScheduleSpec::Once,
            api_thread_schedule_spec_to_state(ThreadScheduleSpec::Once).expect("once should map")
        );
        assert_eq!(
            codex_state::ThreadScheduleSpec::Interval(codex_state::ThreadScheduleInterval {
                amount: 5,
                unit: codex_state::ThreadScheduleIntervalUnit::Minutes,
            }),
            api_thread_schedule_spec_to_state(ThreadScheduleSpec::Interval {
                amount: 5,
                unit: ThreadScheduleIntervalUnit::Minutes,
            })
            .expect("interval should map")
        );
        assert!(
            api_thread_schedule_spec_to_state(ThreadScheduleSpec::Interval {
                amount: 0,
                unit: ThreadScheduleIntervalUnit::Minutes,
            })
            .is_err()
        );
    }
}
