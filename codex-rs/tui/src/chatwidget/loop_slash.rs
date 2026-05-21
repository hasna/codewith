//! Parser and normalized model for `/loop` slash command arguments.

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
    Dynamic,
    Interval(LoopInterval),
    Cron(LoopCronSchedule),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoopSlashParseError {
    pub message: String,
    pub hint: Option<String>,
}

pub(super) fn parse_loop_slash_args(args: &str) -> Result<LoopSlashCommand, LoopSlashParseError> {
    let args = args.trim();
    if args.is_empty() {
        return Ok(LoopSlashCommand::Default);
    }

    if let Some(manage) = parse_manage_command(args)? {
        return Ok(LoopSlashCommand::Manage(manage));
    }

    if let Some((interval, prompt)) = parse_compact_interval(args)? {
        return create_with_prompt(LoopSchedule::Interval(interval), prompt);
    }

    if let Some((interval, prompt)) = parse_every_interval(args)? {
        return create_with_prompt(LoopSchedule::Interval(interval), prompt);
    }

    if let Some((cron, prompt)) = parse_cron_schedule(args)? {
        return create_with_prompt(LoopSchedule::Cron(cron), prompt);
    }

    create_with_prompt(LoopSchedule::Dynamic, args)
}

fn parse_manage_command(args: &str) -> Result<Option<LoopManageCommand>, LoopSlashParseError> {
    let (command, rest) = split_first_token(args);
    let rest = rest.trim();
    let schedule_id = || {
        if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        }
    };
    let command = match command.to_ascii_lowercase().as_str() {
        "list" | "ls" | "status" => {
            reject_extra_manage_args(command, rest)?;
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
        _ => return Ok(None),
    };
    Ok(Some(command))
}

fn reject_extra_manage_args(command: &str, rest: &str) -> Result<(), LoopSlashParseError> {
    if rest.is_empty() {
        return Ok(());
    }
    Err(LoopSlashParseError {
        message: format!("`/loop {command}` does not take additional arguments."),
        hint: Some("Use `/loop` to open the schedule manager.".to_string()),
    })
}

fn parse_compact_interval(args: &str) -> Result<Option<(LoopInterval, &str)>, LoopSlashParseError> {
    let (token, rest) = split_first_token(args);
    let Some(interval) = parse_interval_token(token)? else {
        return Ok(None);
    };
    Ok(Some((interval, rest.trim_start())))
}

fn parse_every_interval(args: &str) -> Result<Option<(LoopInterval, &str)>, LoopSlashParseError> {
    let Some(rest) = args.strip_prefix("every ") else {
        return Ok(None);
    };
    let (amount_token, rest) = split_first_token(rest.trim_start());
    let amount = amount_token
        .parse::<u32>()
        .map_err(|_| LoopSlashParseError {
            message: format!("Invalid loop interval amount: `{amount_token}`."),
            hint: Some(
                "Use a positive whole number, for example `/loop every 5 minutes check CI`."
                    .to_string(),
            ),
        })?;
    let (unit_token, prompt) = split_first_token(rest.trim_start());
    if is_seconds_unit(unit_token) {
        validate_interval_amount(amount)?;
        return Ok(Some((seconds_interval(amount), prompt.trim_start())));
    }
    let Some(unit) = parse_interval_unit(unit_token) else {
        return Err(LoopSlashParseError {
            message: format!("Invalid loop interval unit: `{unit_token}`."),
            hint: Some("Supported units are seconds, minutes, hours, and days.".to_string()),
        });
    };
    validate_interval_amount(amount)?;
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
    let prompt = split_after_tokens(args, 5).unwrap_or("").trim_start();
    Ok(Some((LoopCronSchedule { expression }, prompt)))
}

fn create_with_prompt(
    schedule: LoopSchedule,
    prompt: &str,
) -> Result<LoopSlashCommand, LoopSlashParseError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
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

fn parse_interval_token(token: &str) -> Result<Option<LoopInterval>, LoopSlashParseError> {
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
        message: format!("Invalid loop interval: `{token}`."),
        hint: Some("Use a compact interval such as `30s`, `5m`, `2h`, or `1d`.".to_string()),
    })?;
    if seconds_unit {
        validate_interval_amount(amount)?;
        return Ok(Some(seconds_interval(amount)));
    }
    validate_interval_amount(amount)?;
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

fn validate_interval_amount(amount: u32) -> Result<(), LoopSlashParseError> {
    if amount == 0 {
        return Err(LoopSlashParseError {
            message: "Loop interval must be greater than zero.".to_string(),
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
