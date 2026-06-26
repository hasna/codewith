//! Parser and normalized model for thread schedule slash command arguments.

use chrono::DateTime;
use chrono::Datelike;
use chrono::Days;
use chrono::Local;
use chrono::LocalResult;
use chrono::NaiveDate;
use chrono::NaiveTime;
use chrono::TimeZone;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoopSlashCommand {
    Default,
    Create(LoopCreateRequest),
    Manage(LoopManageCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoopCreateRequest {
    pub schedule: LoopSchedule,
    pub prompt: LoopPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoopPrompt {
    Inline(String),
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoopSchedule {
    Once(ScheduleTime),
    Dynamic,
    Interval(LoopInterval),
    Cron(LoopCronSchedule),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScheduleTime {
    Delay(LoopInterval),
    At(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoopInterval {
    pub amount: u32,
    pub unit: LoopIntervalUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopIntervalUnit {
    Minutes,
    Hours,
    Days,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoopCronSchedule {
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoopManageCommand {
    List,
    Pause { schedule_id: Option<String> },
    Resume { schedule_id: Option<String> },
    Delete { schedule_id: Option<String> },
    Edit { schedule_id: Option<String> },
    RunNow { schedule_id: Option<String> },
    Stats { schedule_id: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoopSlashParseError {
    pub message: String,
    pub hint: Option<String>,
}

pub(super) fn parse_loop_slash_args(args: &str) -> Result<LoopSlashCommand, LoopSlashParseError> {
    parse_thread_schedule_slash_args(args, ThreadScheduleSlashKind::Loop)
}

pub(super) fn parse_schedule_slash_args(
    args: &str,
) -> Result<LoopSlashCommand, LoopSlashParseError> {
    parse_thread_schedule_slash_args(args, ThreadScheduleSlashKind::Schedule)
}

#[derive(Clone, Copy)]
enum ThreadScheduleSlashKind {
    Loop,
    Schedule,
}

impl ThreadScheduleSlashKind {
    fn command(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::Schedule => "schedule",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::Schedule => "schedule",
        }
    }

    fn label_title(self) -> &'static str {
        match self {
            Self::Loop => "Loop",
            Self::Schedule => "Schedule",
        }
    }

    fn interval_schedule(self, interval: LoopInterval) -> LoopSchedule {
        match self {
            Self::Loop => LoopSchedule::Interval(interval),
            Self::Schedule => LoopSchedule::Once(ScheduleTime::Delay(interval)),
        }
    }
}

fn parse_thread_schedule_slash_args(
    args: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<LoopSlashCommand, LoopSlashParseError> {
    let args = args.trim();
    if args.is_empty() {
        return Ok(LoopSlashCommand::Default);
    }

    if let Some(manage) = parse_manage_command(args, kind)? {
        return Ok(LoopSlashCommand::Manage(manage));
    }

    if let Some((interval, prompt)) = parse_compact_interval(args, kind)? {
        return create_with_prompt(kind, kind.interval_schedule(interval), prompt);
    }

    if matches!(kind, ThreadScheduleSlashKind::Schedule)
        && let Some((interval, prompt)) = parse_in_interval(args, kind)?
    {
        return create_with_prompt(kind, kind.interval_schedule(interval), prompt);
    }

    if matches!(kind, ThreadScheduleSlashKind::Schedule)
        && let Some((time, prompt)) = parse_schedule_time(args)?
    {
        return create_with_prompt(kind, LoopSchedule::Once(time), prompt);
    }

    if let Some((interval, prompt)) = parse_every_interval(args, kind)? {
        if matches!(kind, ThreadScheduleSlashKind::Schedule) {
            return Err(recurring_schedule_error());
        }
        return create_with_prompt(kind, LoopSchedule::Interval(interval), prompt);
    }

    if let Some((cron, prompt)) = parse_cron_schedule(args)? {
        if matches!(kind, ThreadScheduleSlashKind::Schedule) {
            return Err(recurring_schedule_error());
        }
        return create_with_prompt(kind, LoopSchedule::Cron(cron), prompt);
    }

    if matches!(kind, ThreadScheduleSlashKind::Schedule) {
        return Err(LoopSlashParseError {
            message: "Schedule time is required.".to_string(),
            hint: Some(
                "Use `/schedule 5m check CI` or `/schedule in 2 hours check CI`.".to_string(),
            ),
        });
    }

    create_with_prompt(kind, LoopSchedule::Dynamic, args)
}

fn parse_manage_command(
    args: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<Option<LoopManageCommand>, LoopSlashParseError> {
    let (command, rest) = split_first_token(args);
    let rest = rest.trim();
    let schedule_id = || schedule_id_arg(rest);
    let command = match command.to_ascii_lowercase().as_str() {
        "list" | "ls" | "status" => {
            reject_extra_manage_args(command, rest, kind)?;
            LoopManageCommand::List
        }
        "pause" | "stop" => LoopManageCommand::Pause {
            schedule_id: schedule_id(),
        },
        "resume" | "start" => LoopManageCommand::Resume {
            schedule_id: schedule_id(),
        },
        "delete" | "remove" | "cancel" | "clear" => LoopManageCommand::Delete {
            schedule_id: schedule_id(),
        },
        "edit" => LoopManageCommand::Edit {
            schedule_id: schedule_id(),
        },
        "run" | "run-now" | "now" => LoopManageCommand::RunNow {
            schedule_id: schedule_id(),
        },
        "stats" | "stat" => LoopManageCommand::Stats {
            schedule_id: schedule_id(),
        },
        _ => return Ok(None),
    };
    Ok(Some(command))
}

fn schedule_id_arg(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let compact = rest.split_whitespace().collect::<String>();
    if compact != rest && looks_like_uuid(&compact) {
        Some(compact)
    } else {
        Some(rest.to_string())
    }
}

fn looks_like_uuid(value: &str) -> bool {
    value.len() == 36
        && value
            .char_indices()
            .all(|(idx, ch)| matches!(idx, 8 | 13 | 18 | 23) == (ch == '-'))
        && value
            .chars()
            .filter(|ch| *ch != '-')
            .all(|ch| ch.is_ascii_hexdigit())
}

fn reject_extra_manage_args(
    command: &str,
    rest: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<(), LoopSlashParseError> {
    if rest.is_empty() {
        return Ok(());
    }
    Err(LoopSlashParseError {
        message: format!(
            "`/{} {command}` does not take additional arguments.",
            kind.command()
        ),
        hint: Some(format!(
            "Use `/{}` to open the schedule manager.",
            kind.command()
        )),
    })
}

fn parse_compact_interval(
    args: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<Option<(LoopInterval, &str)>, LoopSlashParseError> {
    let (token, rest) = split_first_token(args);
    let Some(interval) = parse_interval_token(token, kind)? else {
        return Ok(None);
    };
    Ok(Some((interval, rest.trim_start())))
}

fn parse_every_interval(
    args: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<Option<(LoopInterval, &str)>, LoopSlashParseError> {
    let Some(rest) = args.strip_prefix("every ") else {
        return Ok(None);
    };
    let (amount_token, rest) = split_first_token(rest.trim_start());
    let amount = amount_token
        .parse::<u32>()
        .map_err(|_| LoopSlashParseError {
            message: format!(
                "Invalid {} interval amount: `{amount_token}`.",
                kind.label()
            ),
            hint: Some(format!(
                "Use a positive whole number, for example `/{} every 5 minutes check CI`.",
                kind.command()
            )),
        })?;
    let (unit_token, prompt) = split_first_token(rest.trim_start());
    if is_seconds_unit(unit_token) {
        validate_interval_amount(amount, kind)?;
        return Ok(Some((seconds_interval(amount), prompt.trim_start())));
    }
    let Some(unit) = parse_interval_unit(unit_token) else {
        return Err(LoopSlashParseError {
            message: format!("Invalid {} interval unit: `{unit_token}`.", kind.label()),
            hint: Some("Supported units are seconds, minutes, hours, and days.".to_string()),
        });
    };
    validate_interval_amount(amount, kind)?;
    Ok(Some((LoopInterval { amount, unit }, prompt.trim_start())))
}

fn parse_in_interval(
    args: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<Option<(LoopInterval, &str)>, LoopSlashParseError> {
    let Some(rest) = args.strip_prefix("in ") else {
        return Ok(None);
    };
    parse_amount_unit_interval(rest, kind)
}

fn parse_amount_unit_interval(
    args: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<Option<(LoopInterval, &str)>, LoopSlashParseError> {
    let (amount_token, rest) = split_first_token(args.trim_start());
    let amount = amount_token
        .parse::<u32>()
        .map_err(|_| LoopSlashParseError {
            message: format!(
                "Invalid {} interval amount: `{amount_token}`.",
                kind.label()
            ),
            hint: Some(format!(
                "Use a positive whole number, for example `/{} in 5 minutes check CI`.",
                kind.command()
            )),
        })?;
    let (unit_token, prompt) = split_first_token(rest.trim_start());
    if is_seconds_unit(unit_token) {
        validate_interval_amount(amount, kind)?;
        return Ok(Some((seconds_interval(amount), prompt.trim_start())));
    }
    let Some(unit) = parse_interval_unit(unit_token) else {
        return Err(LoopSlashParseError {
            message: format!("Invalid {} interval unit: `{unit_token}`.", kind.label()),
            hint: Some("Supported units are seconds, minutes, hours, and days.".to_string()),
        });
    };
    validate_interval_amount(amount, kind)?;
    Ok(Some((LoopInterval { amount, unit }, prompt.trim_start())))
}

fn parse_cron_schedule(
    args: &str,
) -> Result<Option<(LoopCronSchedule, &str)>, LoopSlashParseError> {
    let tokens = args.split_whitespace().take(5).collect::<Vec<_>>();
    if tokens.len() < 5 {
        return Ok(None);
    }
    let has_cron_special = tokens.iter().any(|token| {
        *token == "*"
            || token
                .chars()
                .any(|ch| matches!(ch, '/' | ',' | '-' | '?' | 'L' | 'W' | '#'))
    });
    let all_numeric_or_structural = tokens.iter().all(|token| looks_like_cron_field(token));
    if !has_cron_special && !all_numeric_or_structural {
        return Ok(None);
    }
    validate_cron_fields(&tokens)?;
    let expression = tokens.join(" ");
    let prompt = split_after_tokens(args, /*token_count*/ 5)
        .unwrap_or("")
        .trim_start();
    Ok(Some((LoopCronSchedule { expression }, prompt)))
}

fn parse_schedule_time(args: &str) -> Result<Option<(ScheduleTime, &str)>, LoopSlashParseError> {
    parse_schedule_time_with_now(args, Local::now())
}

fn parse_schedule_time_with_now(
    args: &str,
    now: DateTime<Local>,
) -> Result<Option<(ScheduleTime, &str)>, LoopSlashParseError> {
    let (first_token, rest) = split_first_token(args);
    if first_token.is_empty() {
        return Ok(None);
    }

    if let Some(timestamp) = parse_absolute_timestamp_token(first_token, now)? {
        return Ok(Some((ScheduleTime::At(timestamp), rest.trim_start())));
    }

    let first_lower = first_token.to_ascii_lowercase();
    if first_lower == "at" {
        let (time, prompt) = parse_time_prefix(rest).ok_or_else(missing_schedule_time_error)?;
        let timestamp = next_time_timestamp(time, now)?;
        return Ok(Some((ScheduleTime::At(timestamp), prompt.trim_start())));
    }

    if matches!(first_lower.as_str(), "today" | "tomorrow") {
        let date = if first_lower == "today" {
            now.date_naive()
        } else {
            now.date_naive()
                .checked_add_days(Days::new(1))
                .ok_or_else(invalid_schedule_time_error)?
        };
        let (time, prompt) =
            parse_optional_at_time_prefix(rest).ok_or_else(missing_schedule_time_error)?;
        let timestamp = future_local_timestamp(date, time, now)?;
        return Ok(Some((ScheduleTime::At(timestamp), prompt.trim_start())));
    }

    if looks_like_date_token(first_token) {
        let date = parse_date_token(first_token).ok_or_else(invalid_schedule_time_error)?;
        let (time, prompt) =
            parse_optional_at_time_prefix(rest).ok_or_else(missing_schedule_time_error)?;
        let timestamp = future_local_timestamp(date, time, now)?;
        return Ok(Some((ScheduleTime::At(timestamp), prompt.trim_start())));
    }

    if let Some(month) = month_number(first_token) {
        let Some((date, rest)) = parse_month_date_prefix(month, rest, now) else {
            return Err(invalid_schedule_time_error());
        };
        let (time, prompt) =
            parse_optional_at_time_prefix(rest).ok_or_else(missing_schedule_time_error)?;
        let timestamp = future_local_timestamp(date, time, now)?;
        return Ok(Some((ScheduleTime::At(timestamp), prompt.trim_start())));
    }

    Ok(None)
}

fn parse_absolute_timestamp_token(
    token: &str,
    now: DateTime<Local>,
) -> Result<Option<i64>, LoopSlashParseError> {
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(token) {
        return validate_future_timestamp(timestamp.timestamp(), now).map(Some);
    }
    if !token.contains('T') {
        return Ok(None);
    }
    let token = token.replace('T', " ");
    for pattern in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(datetime) = chrono::NaiveDateTime::parse_from_str(token.as_str(), pattern) {
            let timestamp = future_local_timestamp(datetime.date(), datetime.time(), now)?;
            return Ok(Some(timestamp));
        }
    }
    Err(invalid_schedule_time_error())
}

fn parse_optional_at_time_prefix(args: &str) -> Option<(NaiveTime, &str)> {
    let (first_token, rest) = split_first_token(args);
    if first_token.eq_ignore_ascii_case("at") {
        parse_time_prefix(rest)
    } else {
        parse_time_prefix(args)
    }
}

fn parse_time_prefix(args: &str) -> Option<(NaiveTime, &str)> {
    let (time_token, rest) = split_first_token(args);
    if time_token.is_empty() {
        return None;
    }
    let (period_token, after_period) = split_first_token(rest);
    if is_day_period(period_token) {
        return parse_time_token_with_period(time_token, Some(period_token))
            .map(|time| (time, after_period));
    }
    parse_time_token_with_period(time_token, /*period*/ None).map(|time| (time, rest))
}

fn parse_time_token_with_period(token: &str, period: Option<&str>) -> Option<NaiveTime> {
    let token = token.trim_end_matches([',', '.']);
    if token.is_empty() {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    let (time_token, period) = if let Some(stripped) = lower.strip_suffix("am") {
        (stripped, Some("am"))
    } else if let Some(stripped) = lower.strip_suffix("pm") {
        (stripped, Some("pm"))
    } else {
        (token, period)
    };
    let (hour, minute) = parse_hour_minute(time_token)?;
    match period.map(str::to_ascii_lowercase).as_deref() {
        Some("am") => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            NaiveTime::from_hms_opt(if hour == 12 { 0 } else { hour }, minute, 0)
        }
        Some("pm") => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            NaiveTime::from_hms_opt(if hour == 12 { 12 } else { hour + 12 }, minute, 0)
        }
        Some(_) => None,
        None => {
            if hour > 23 {
                return None;
            }
            NaiveTime::from_hms_opt(hour, minute, 0)
        }
    }
}

fn parse_hour_minute(token: &str) -> Option<(u32, u32)> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let mut parts = token.split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = match parts.next() {
        Some(minute) => minute.parse::<u32>().ok()?,
        None => 0,
    };
    if parts.next().is_some() || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

fn is_day_period(token: &str) -> bool {
    matches!(token.to_ascii_lowercase().as_str(), "am" | "pm")
}

fn looks_like_date_token(token: &str) -> bool {
    let token = token.trim_end_matches(',');
    let separator = if token.contains('-') { '-' } else { '/' };
    let parts = token.split(separator).collect::<Vec<_>>();
    match separator {
        '-' => {
            parts.len() == 3
                && parts[0].len() == 4
                && parts
                    .iter()
                    .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
        }
        '/' => {
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
        }
        _ => false,
    }
}

fn parse_date_token(token: &str) -> Option<NaiveDate> {
    let token = token.trim_end_matches(',');
    NaiveDate::parse_from_str(token, "%Y-%m-%d")
        .ok()
        .or_else(|| NaiveDate::parse_from_str(token, "%m/%d/%Y").ok())
}

fn parse_month_date_prefix(
    month: u32,
    args: &str,
    now: DateTime<Local>,
) -> Option<(NaiveDate, &str)> {
    let (day_token, rest) = split_first_token(args);
    let day = parse_day_token(day_token)?;
    let (next_token, after_next) = split_first_token(rest);
    let (year, rest) = if next_token.len() == 4 && next_token.chars().all(|ch| ch.is_ascii_digit())
    {
        (Some(next_token.parse::<i32>().ok()?), after_next)
    } else {
        (None, rest)
    };
    let date = match year {
        Some(year) => NaiveDate::from_ymd_opt(year, month, day)?,
        None => next_month_day(month, day, now)?,
    };
    Some((date, rest))
}

fn parse_day_token(token: &str) -> Option<u32> {
    let mut token = token.trim_end_matches(',');
    for suffix in ["st", concat!("n", "d"), "rd", "th"] {
        token = token.trim_end_matches(suffix);
    }
    token.parse::<u32>().ok()
}

fn next_month_day(month: u32, day: u32, now: DateTime<Local>) -> Option<NaiveDate> {
    let this_year = NaiveDate::from_ymd_opt(now.year(), month, day)?;
    if this_year >= now.date_naive() {
        Some(this_year)
    } else {
        NaiveDate::from_ymd_opt(now.year() + 1, month, day)
    }
}

fn month_number(token: &str) -> Option<u32> {
    match token.trim_end_matches(',').to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn next_time_timestamp(time: NaiveTime, now: DateTime<Local>) -> Result<i64, LoopSlashParseError> {
    if let Ok(timestamp) = future_local_timestamp(now.date_naive(), time, now) {
        return Ok(timestamp);
    }
    let tomorrow = now
        .date_naive()
        .checked_add_days(Days::new(1))
        .ok_or_else(invalid_schedule_time_error)?;
    future_local_timestamp(tomorrow, time, now)
}

fn future_local_timestamp(
    date: NaiveDate,
    time: NaiveTime,
    now: DateTime<Local>,
) -> Result<i64, LoopSlashParseError> {
    let naive = date.and_time(time);
    let mut candidates = match Local.from_local_datetime(&naive) {
        LocalResult::Single(datetime) => vec![datetime],
        LocalResult::Ambiguous(earlier, later) => vec![earlier, later],
        LocalResult::None => return Err(invalid_schedule_time_error()),
    };
    candidates.sort();
    let Some(datetime) = candidates.into_iter().find(|datetime| *datetime > now) else {
        return Err(LoopSlashParseError {
            message: "Schedule time must be in the future.".to_string(),
            hint: Some("Use a future date/time or a relative delay such as `5m`.".to_string()),
        });
    };
    Ok(datetime.timestamp())
}

fn validate_future_timestamp(
    timestamp: i64,
    now: DateTime<Local>,
) -> Result<i64, LoopSlashParseError> {
    if timestamp > now.timestamp() {
        Ok(timestamp)
    } else {
        Err(LoopSlashParseError {
            message: "Schedule time must be in the future.".to_string(),
            hint: Some("Use a future date/time or a relative delay such as `5m`.".to_string()),
        })
    }
}

fn missing_schedule_time_error() -> LoopSlashParseError {
    LoopSlashParseError {
        message: "Schedule time is required.".to_string(),
        hint: Some(
            "Use `/schedule 5m check CI` or `/schedule 2026-06-05 09:30 check CI`.".to_string(),
        ),
    }
}

fn invalid_schedule_time_error() -> LoopSlashParseError {
    LoopSlashParseError {
        message: "Invalid schedule time.".to_string(),
        hint: Some("Use a time like `2026-06-05 09:30`, `tomorrow at 9am`, or `5m`.".to_string()),
    }
}

fn create_with_prompt(
    kind: ThreadScheduleSlashKind,
    schedule: LoopSchedule,
    prompt: &str,
) -> Result<LoopSlashCommand, LoopSlashParseError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        if matches!(kind, ThreadScheduleSlashKind::Schedule) {
            return Err(LoopSlashParseError {
                message: "Schedule prompt is required.".to_string(),
                hint: Some("Use `/schedule 5m ask me something`.".to_string()),
            });
        }
        return Ok(LoopSlashCommand::Create(LoopCreateRequest {
            schedule,
            prompt: LoopPrompt::Default,
        }));
    }
    Ok(LoopSlashCommand::Create(LoopCreateRequest {
        schedule,
        prompt: LoopPrompt::Inline(prompt.to_string()),
    }))
}

fn parse_interval_token(
    token: &str,
    kind: ThreadScheduleSlashKind,
) -> Result<Option<LoopInterval>, LoopSlashParseError> {
    if token.len() < 2 {
        return Ok(None);
    }
    let (amount, unit) = token.split_at(token.len() - 1);
    let seconds_unit = unit == "s";
    let unit = if seconds_unit {
        LoopIntervalUnit::Minutes
    } else {
        let Some(unit) = parse_compact_interval_unit(unit) else {
            return Ok(None);
        };
        unit
    };
    let amount = amount.parse::<u32>().map_err(|_| LoopSlashParseError {
        message: format!("Invalid {} interval: `{token}`.", kind.label()),
        hint: Some(format!(
            "Use a compact interval such as `/{} 30s check CI`, `/{} 5m check CI`, `/{} 2h check CI`, or `/{} 1d check CI`.",
            kind.command(),
            kind.command(),
            kind.command(),
            kind.command()
        )),
    })?;
    if seconds_unit {
        validate_interval_amount(amount, kind)?;
        return Ok(Some(seconds_interval(amount)));
    }
    validate_interval_amount(amount, kind)?;
    Ok(Some(LoopInterval { amount, unit }))
}

fn parse_compact_interval_unit(unit: &str) -> Option<LoopIntervalUnit> {
    match unit {
        "m" => Some(LoopIntervalUnit::Minutes),
        "h" => Some(LoopIntervalUnit::Hours),
        "d" => Some(LoopIntervalUnit::Days),
        _ => None,
    }
}

fn parse_interval_unit(unit: &str) -> Option<LoopIntervalUnit> {
    match unit.to_ascii_lowercase().as_str() {
        "minute" | "minutes" | "min" | "mins" => Some(LoopIntervalUnit::Minutes),
        "hour" | "hours" | "hr" | "hrs" => Some(LoopIntervalUnit::Hours),
        "day" | "days" => Some(LoopIntervalUnit::Days),
        _ => None,
    }
}

fn is_seconds_unit(unit: &str) -> bool {
    matches!(
        unit.to_ascii_lowercase().as_str(),
        "second" | "seconds" | "sec" | "secs"
    )
}

fn seconds_interval(seconds: u32) -> LoopInterval {
    LoopInterval {
        amount: seconds.div_ceil(60).max(1),
        unit: LoopIntervalUnit::Minutes,
    }
}

fn validate_interval_amount(
    amount: u32,
    kind: ThreadScheduleSlashKind,
) -> Result<(), LoopSlashParseError> {
    if amount == 0 {
        return Err(LoopSlashParseError {
            message: format!("{} interval must be greater than zero.", kind.label_title()),
            hint: Some("Use an interval such as `5m` or `every 10 minutes`.".to_string()),
        });
    }
    Ok(())
}

fn validate_cron_fields(fields: &[&str]) -> Result<(), LoopSlashParseError> {
    let ranges = [
        CronRange { min: 0, max: 59 },
        CronRange { min: 0, max: 23 },
        CronRange { min: 1, max: 31 },
        CronRange { min: 1, max: 12 },
        CronRange { min: 0, max: 7 },
    ];
    for (field, range) in fields.iter().zip(ranges) {
        validate_cron_field(field, range)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct CronRange {
    min: u32,
    max: u32,
}

fn validate_cron_field(field: &str, range: CronRange) -> Result<(), LoopSlashParseError> {
    if field
        .chars()
        .any(|ch| ch.is_ascii_alphabetic() || matches!(ch, '?' | 'L' | 'W' | '#'))
    {
        return Err(unsupported_cron_syntax(field));
    }

    for part in field.split(',') {
        validate_cron_part(part, range)?;
    }
    Ok(())
}

fn validate_cron_part(part: &str, range: CronRange) -> Result<(), LoopSlashParseError> {
    if part.is_empty() {
        return Err(invalid_cron_field(part));
    }
    let (base, step) = match part.split_once('/') {
        Some((base, step)) => (base, Some(step)),
        None => (part, None),
    };
    if let Some(step) = step {
        let step = step.parse::<u32>().map_err(|_| invalid_cron_field(part))?;
        if step == 0 {
            return Err(invalid_cron_field(part));
        }
    }
    if base == "*" {
        return Ok(());
    }
    if let Some((start, end)) = base.split_once('-') {
        let start = parse_cron_number(start, range, part)?;
        let end = parse_cron_number(end, range, part)?;
        if start > end {
            return Err(invalid_cron_field(part));
        }
        return Ok(());
    }
    parse_cron_number(base, range, part).map(|_| ())
}

fn parse_cron_number(
    value: &str,
    range: CronRange,
    field: &str,
) -> Result<u32, LoopSlashParseError> {
    let number = value
        .parse::<u32>()
        .map_err(|_| invalid_cron_field(field))?;
    if number < range.min || number > range.max {
        return Err(invalid_cron_field(field));
    }
    Ok(number)
}

fn looks_like_cron_field(token: &str) -> bool {
    token == "*"
        || token.contains('/')
        || token.contains(',')
        || token.contains('-')
        || token.chars().all(|ch| ch.is_ascii_digit())
}

fn unsupported_cron_syntax(field: &str) -> LoopSlashParseError {
    LoopSlashParseError {
        message: format!("Unsupported cron syntax in field `{field}`."),
        hint: Some(
            "Use five-field numeric cron syntax; names and extended fields like L, W, and ? are not supported."
                .to_string(),
        ),
    }
}

fn invalid_cron_field(field: &str) -> LoopSlashParseError {
    LoopSlashParseError {
        message: format!("Invalid cron field: `{field}`."),
        hint: Some("Use a five-field cron expression such as `*/5 * * * *`.".to_string()),
    }
}

fn recurring_schedule_error() -> LoopSlashParseError {
    LoopSlashParseError {
        message: "Schedules are one-time events; recurring work belongs in /loop.".to_string(),
        hint: Some("Use `/loop every 5 minutes check CI` for recurring work.".to_string()),
    }
}

fn split_first_token(value: &str) -> (&str, &str) {
    let value = value.trim_start();
    match value.find(char::is_whitespace) {
        Some(index) => (&value[..index], &value[index..]),
        None => (value, ""),
    }
}

fn split_after_tokens(value: &str, token_count: usize) -> Option<&str> {
    let mut remaining = value;
    for _ in 0..token_count {
        remaining = remaining.trim_start();
        let end = remaining
            .find(char::is_whitespace)
            .unwrap_or(remaining.len());
        remaining = &remaining[end..];
    }
    Some(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_bare_loop_as_default() {
        assert_eq!(parse_loop_slash_args(""), Ok(LoopSlashCommand::Default));
        assert_eq!(parse_loop_slash_args("   "), Ok(LoopSlashCommand::Default));
    }

    #[test]
    fn parses_compact_interval_loop() {
        assert_eq!(
            parse_loop_slash_args("5m check whether CI is green"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Interval(LoopInterval {
                    amount: 5,
                    unit: LoopIntervalUnit::Minutes,
                }),
                prompt: LoopPrompt::Inline("check whether CI is green".to_string()),
            }))
        );
    }

    #[test]
    fn parses_every_interval_loop() {
        assert_eq!(
            parse_loop_slash_args("every 2 hours check review comments"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Interval(LoopInterval {
                    amount: 2,
                    unit: LoopIntervalUnit::Hours,
                }),
                prompt: LoopPrompt::Inline("check review comments".to_string()),
            }))
        );
    }

    #[test]
    fn rounds_second_intervals_up_to_the_one_minute_minimum() {
        assert_eq!(
            parse_loop_slash_args("30s check CI"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Interval(LoopInterval {
                    amount: 1,
                    unit: LoopIntervalUnit::Minutes,
                }),
                prompt: LoopPrompt::Inline("check CI".to_string()),
            }))
        );
        assert_eq!(
            parse_loop_slash_args("every 90 seconds check CI"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Interval(LoopInterval {
                    amount: 2,
                    unit: LoopIntervalUnit::Minutes,
                }),
                prompt: LoopPrompt::Inline("check CI".to_string()),
            }))
        );
    }

    #[test]
    fn parses_cron_loop() {
        assert_eq!(
            parse_loop_slash_args("*/15 9-17 * * 1-5 check the deploy"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Cron(LoopCronSchedule {
                    expression: "*/15 9-17 * * 1-5".to_string(),
                }),
                prompt: LoopPrompt::Inline("check the deploy".to_string()),
            }))
        );
    }

    #[test]
    fn parses_numeric_cron_loop() {
        assert_eq!(
            parse_loop_slash_args("0 9 1 1 1 run monthly maintenance"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Cron(LoopCronSchedule {
                    expression: "0 9 1 1 1".to_string(),
                }),
                prompt: LoopPrompt::Inline("run monthly maintenance".to_string()),
            }))
        );
    }

    #[test]
    fn parses_prompt_only_loop_as_dynamic() {
        assert_eq!(
            parse_loop_slash_args("check whether CI passed"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Dynamic,
                prompt: LoopPrompt::Inline("check whether CI passed".to_string()),
            }))
        );
    }

    #[test]
    fn loop_in_phrase_remains_prompt_text() {
        assert_eq!(
            parse_loop_slash_args("in 2 hours check whether CI passed"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Dynamic,
                prompt: LoopPrompt::Inline("in 2 hours check whether CI passed".to_string()),
            }))
        );
    }

    #[test]
    fn prompt_only_loop_can_start_with_a_number() {
        assert_eq!(
            parse_loop_slash_args("5 things to check before release"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Dynamic,
                prompt: LoopPrompt::Inline("5 things to check before release".to_string()),
            }))
        );
    }

    #[test]
    fn parses_manage_commands() {
        assert_eq!(
            parse_loop_slash_args("list"),
            Ok(LoopSlashCommand::Manage(LoopManageCommand::List))
        );
        assert_eq!(
            parse_loop_slash_args("pause sched-1"),
            Ok(LoopSlashCommand::Manage(LoopManageCommand::Pause {
                schedule_id: Some("sched-1".to_string()),
            }))
        );
        assert_eq!(
            parse_loop_slash_args("run-now sched-1"),
            Ok(LoopSlashCommand::Manage(LoopManageCommand::RunNow {
                schedule_id: Some("sched-1".to_string()),
            }))
        );
        assert_eq!(
            parse_loop_slash_args("stats sched-1"),
            Ok(LoopSlashCommand::Manage(LoopManageCommand::Stats {
                schedule_id: Some("sched-1".to_string()),
            }))
        );
        assert_eq!(
            parse_loop_slash_args("run 0f4a4ce9-66ac-478c-\n  8897-43c2fe8c31df"),
            Ok(LoopSlashCommand::Manage(LoopManageCommand::RunNow {
                schedule_id: Some("0f4a4ce9-66ac-478c-8897-43c2fe8c31df".to_string()),
            }))
        );
        assert_eq!(
            parse_loop_slash_args("resume 0f4a4ce9-66ac-478c-\n  8897-43c2fe8c31df"),
            Ok(LoopSlashCommand::Manage(LoopManageCommand::Resume {
                schedule_id: Some("0f4a4ce9-66ac-478c-8897-43c2fe8c31df".to_string()),
            }))
        );
    }

    #[test]
    fn parses_missing_scheduled_prompt_as_default_prompt() {
        assert_eq!(
            parse_loop_slash_args("5m"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Interval(LoopInterval {
                    amount: 5,
                    unit: LoopIntervalUnit::Minutes,
                }),
                prompt: LoopPrompt::Default,
            }))
        );
    }

    #[test]
    fn rejects_zero_interval() {
        let err = parse_loop_slash_args("0m check CI").expect_err("expected interval rejection");
        assert_eq!(err.message, "Loop interval must be greater than zero.");
    }

    #[test]
    fn schedule_parser_uses_schedule_error_wording() {
        let err = parse_schedule_slash_args("every 2 weeks check CI")
            .expect_err("expected interval unit rejection");
        assert_eq!(err.message, "Invalid schedule interval unit: `weeks`.");
    }

    #[test]
    fn parses_compact_schedule_as_one_time_delay() {
        assert_eq!(
            parse_schedule_slash_args("5m check whether CI is green"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Once(ScheduleTime::Delay(LoopInterval {
                    amount: 5,
                    unit: LoopIntervalUnit::Minutes,
                })),
                prompt: LoopPrompt::Inline("check whether CI is green".to_string()),
            }))
        );
    }

    #[test]
    fn parses_in_schedule_as_one_time_delay() {
        assert_eq!(
            parse_schedule_slash_args("in 2 hours check review comments"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Once(ScheduleTime::Delay(LoopInterval {
                    amount: 2,
                    unit: LoopIntervalUnit::Hours,
                })),
                prompt: LoopPrompt::Inline("check review comments".to_string()),
            }))
        );
    }

    #[test]
    fn parses_rfc3339_schedule_as_one_time_at() {
        let timestamp = DateTime::parse_from_rfc3339("2099-06-05T09:30:00Z")
            .expect("test timestamp should parse")
            .timestamp();
        assert_eq!(
            parse_schedule_slash_args("2099-06-05T09:30:00Z ask me something"),
            Ok(LoopSlashCommand::Create(LoopCreateRequest {
                schedule: LoopSchedule::Once(ScheduleTime::At(timestamp)),
                prompt: LoopPrompt::Inline("ask me something".to_string()),
            }))
        );
    }

    #[test]
    fn parses_local_date_time_schedule_as_one_time_at() {
        let now = Local
            .with_ymd_and_hms(2026, 6, 4, 10, 0, 0)
            .single()
            .expect("test time should exist locally");
        let expected = future_local_timestamp(
            NaiveDate::from_ymd_opt(2026, 6, 5).expect("test date"),
            NaiveTime::from_hms_opt(9, 30, 0).expect("test time"),
            now,
        )
        .expect("future local timestamp should parse");
        assert_eq!(
            parse_schedule_time_with_now("2026-06-05 09:30 ask me something", now),
            Ok(Some((ScheduleTime::At(expected), "ask me something",)))
        );
    }

    #[test]
    fn parses_friendly_schedule_time_variants() {
        let now = Local
            .with_ymd_and_hms(2026, 6, 4, 10, 0, 0)
            .single()
            .expect("test time should exist locally");
        let expected = future_local_timestamp(
            NaiveDate::from_ymd_opt(2026, 6, 5).expect("test date"),
            NaiveTime::from_hms_opt(9, 0, 0).expect("test time"),
            now,
        )
        .expect("future local timestamp should parse");
        assert_eq!(
            parse_schedule_time_with_now("tomorrow at 9am ask me something", now),
            Ok(Some((ScheduleTime::At(expected), "ask me something",)))
        );
        assert_eq!(
            parse_schedule_time_with_now("June 5 2026 at 9am ask me something", now),
            Ok(Some((ScheduleTime::At(expected), "ask me something",)))
        );
    }

    #[test]
    fn rejects_schedule_without_prompt() {
        let err = parse_schedule_slash_args("5m").expect_err("expected missing prompt rejection");
        assert_eq!(err.message, "Schedule prompt is required.");
    }

    #[test]
    fn rejects_recurring_schedule_slash_args() {
        let err = parse_schedule_slash_args("every 2 hours check CI")
            .expect_err("expected recurring schedule rejection");
        assert_eq!(
            err.message,
            "Schedules are one-time events; recurring work belongs in /loop."
        );
    }

    #[test]
    fn rejects_unsupported_cron_names() {
        let err =
            parse_loop_slash_args("0 9 * * MON check CI").expect_err("expected cron rejection");
        assert_eq!(err.message, "Unsupported cron syntax in field `MON`.");
    }

    #[test]
    fn rejects_out_of_range_cron_fields() {
        let err = parse_loop_slash_args("60 * * * * check CI")
            .expect_err("expected out-of-range cron rejection");
        assert_eq!(err.message, "Invalid cron field: `60`.");
    }
}
