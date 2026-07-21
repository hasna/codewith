//! Automatic retry scheduling for recoverable usage-limit and availability failures.

use super::*;
use chrono::DateTime;
use chrono::Local;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::TimeZone;
use chrono::Utc;

#[derive(Debug, Clone, Default)]
pub(super) struct UsageSelfHealState {
    last_submitted_turn: Option<UsageSelfHealSubmittedTurn>,
    pending_retry: Option<UsageSelfHealPendingRetry>,
    next_retry_id: u64,
    consecutive_retries: u64,
}

#[derive(Debug, Clone)]
struct UsageSelfHealSubmittedTurn {
    user_message: UserMessage,
    history_record: UserMessageHistoryRecord,
    shell_escape_policy: ShellEscapePolicy,
}

#[derive(Debug, Clone)]
struct UsageSelfHealPendingRetry {
    retry_id: u64,
    submitted: UsageSelfHealSubmittedTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageSelfHealErrorKind {
    UsageLimit,
    TransientAvailability,
}

impl ChatWidget {
    pub(super) fn record_usage_self_heal_submitted_turn(
        &mut self,
        user_message: &UserMessage,
        history_record: &UserMessageHistoryRecord,
        shell_escape_policy: ShellEscapePolicy,
    ) {
        if self.pending_usage_limit_auto_reset_check.is_none()
            && self.pending_rate_limit_reset_consumption.is_none()
            && self.rate_limit_reset_in_flight.is_none()
            && self.rate_limit_reset_retry.is_none()
            && self.pending_post_reset_refresh.is_none()
        {
            self.automatic_reset_opted_out_generation = None;
        }
        self.usage_self_heal.last_submitted_turn = Some(UsageSelfHealSubmittedTurn {
            user_message: user_message.clone(),
            history_record: history_record.clone(),
            shell_escape_policy,
        });
    }

    pub(super) fn clear_usage_self_heal_turn(&mut self) {
        self.usage_self_heal.last_submitted_turn = None;
        self.usage_self_heal.pending_retry = None;
        self.usage_self_heal.consecutive_retries = 0;
    }

    pub(super) fn prepare_for_usage_limit_reset(&mut self) {
        self.usage_self_heal.pending_retry = None;
    }

    pub(super) fn resume_after_usage_limit_reset(&mut self) -> bool {
        let Some(submitted) = self.usage_self_heal.last_submitted_turn.take() else {
            return false;
        };
        self.usage_self_heal.pending_retry = None;
        self.usage_self_heal.consecutive_retries = 0;
        self.input_queue.queued_user_messages.push_front(
            QueuedUserMessage::new_with_shell_escape_policy(
                submitted.user_message,
                QueuedInputAction::Plain,
                submitted.shell_escape_policy,
            ),
        );
        self.input_queue
            .queued_user_message_history_records
            .push_front(submitted.history_record);
        self.refresh_pending_input_preview();
        self.maybe_send_next_queued_input()
    }

    pub(super) fn maybe_schedule_usage_self_heal_retry(
        &mut self,
        kind: UsageSelfHealErrorKind,
        error_message: Option<&str>,
    ) -> Option<Duration> {
        let config = &self.config.usage_self_heal;
        if !config.enabled || config.max_retries == 0 {
            return None;
        }
        if kind == UsageSelfHealErrorKind::UsageLimit
            && self.pending_auth_profile_auto_switch_trigger.is_some()
        {
            return None;
        }
        if self.usage_self_heal.consecutive_retries >= config.max_retries {
            return None;
        }
        let submitted = self.usage_self_heal.last_submitted_turn.clone()?;
        let retry_number = self.usage_self_heal.consecutive_retries + 1;
        let delay = match kind {
            UsageSelfHealErrorKind::UsageLimit => self
                .usage_self_heal_reset_retry_delay(error_message)
                .unwrap_or_else(|| self.usage_self_heal_backoff_delay(retry_number)),
            UsageSelfHealErrorKind::TransientAvailability => {
                self.usage_self_heal_backoff_delay(retry_number)
            }
        };

        self.usage_self_heal.next_retry_id = self.usage_self_heal.next_retry_id.saturating_add(1);
        let retry_id = self.usage_self_heal.next_retry_id;
        self.usage_self_heal.consecutive_retries = retry_number;
        self.usage_self_heal.pending_retry = Some(UsageSelfHealPendingRetry {
            retry_id,
            submitted,
        });

        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            tx.send(AppEvent::UsageSelfHealRetry { retry_id });
        });

        Some(delay)
    }

    pub(crate) fn on_usage_self_heal_retry(&mut self, retry_id: u64) -> bool {
        if !self.config.usage_self_heal.enabled {
            return false;
        }
        let Some(pending) = self.usage_self_heal.pending_retry.take() else {
            return false;
        };
        if pending.retry_id != retry_id {
            self.usage_self_heal.pending_retry = Some(pending);
            return false;
        }

        self.usage_self_heal.last_submitted_turn = Some(pending.submitted.clone());
        self.input_queue.queued_user_messages.push_front(
            QueuedUserMessage::new_with_shell_escape_policy(
                pending.submitted.user_message,
                QueuedInputAction::Plain,
                pending.submitted.shell_escape_policy,
            ),
        );
        self.input_queue
            .queued_user_message_history_records
            .push_front(pending.submitted.history_record);
        self.refresh_pending_input_preview();
        self.maybe_send_next_queued_input()
    }

    #[cfg(test)]
    pub(crate) fn pending_usage_self_heal_retry_id(&self) -> Option<u64> {
        self.usage_self_heal
            .pending_retry
            .as_ref()
            .map(|retry| retry.retry_id)
    }

    /// Whether a usage self-heal retry is currently scheduled. Used to keep
    /// keep-going from preempting the self-heal machinery.
    pub(super) fn has_pending_usage_self_heal_retry(&self) -> bool {
        self.usage_self_heal.pending_retry.is_some()
    }

    pub(super) fn usage_self_heal_delay_label(delay: Duration) -> String {
        let seconds = delay.as_secs();
        if seconds < 60 {
            format!("{seconds}s")
        } else if seconds < 60 * 60 {
            let minutes = seconds.div_ceil(60);
            format!("{minutes}m")
        } else {
            let hours = seconds.div_ceil(60 * 60);
            format!("{hours}h")
        }
    }

    fn usage_self_heal_backoff_delay(&self, retry_number: u64) -> Duration {
        let config = &self.config.usage_self_heal;
        let exponent = retry_number.saturating_sub(1).min(16);
        let multiplier = 1u64 << exponent;
        let seconds = config
            .initial_backoff_secs
            .saturating_mul(multiplier)
            .min(config.max_backoff_secs)
            .max(1);
        Duration::from_secs(seconds)
    }

    fn usage_self_heal_reset_retry_delay(&self, error_message: Option<&str>) -> Option<Duration> {
        let config = &self.config.usage_self_heal;
        let now = Utc::now().timestamp();
        let reset_at = error_message
            .and_then(parse_usage_limit_reset_timestamp)
            .map(|dt| dt.timestamp())
            .or_else(|| {
                self.auth_profile_auto_switch_snapshots_by_limit_id
                    .values()
                    .flat_map(|snapshot| [snapshot.secondary.as_ref(), snapshot.primary.as_ref()])
                    .flatten()
                    .filter(|window| window.used_percent >= 100)
                    .filter_map(|window| window.resets_at)
                    .filter(|reset_at| *reset_at > now)
                    .min()
            })?;
        let delay_secs = reset_at
            .saturating_sub(now)
            .saturating_add(i64::try_from(config.reset_retry_buffer_secs).unwrap_or(i64::MAX));
        let delay_secs = u64::try_from(delay_secs).ok()?;
        (delay_secs <= config.max_reset_retry_delay_secs).then(|| Duration::from_secs(delay_secs))
    }
}

fn parse_usage_limit_reset_timestamp(message: &str) -> Option<DateTime<Utc>> {
    let marker = "try again at ";
    let start = message.to_ascii_lowercase().find(marker)? + marker.len();
    let candidate = message.get(start..)?.trim_start();
    let mut parts = Vec::new();
    for part in candidate.split_whitespace() {
        let trimmed = part
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '.' | ';' | ')' | ']' | '}'));
        if trimmed.is_empty() {
            continue;
        }
        parts.push(strip_ordinal_day_suffix(trimmed));
        if matches!(trimmed.to_ascii_uppercase().as_str(), "AM" | "PM") {
            break;
        }
    }
    let date_text = parts.join(" ");
    let naive = if let Ok(naive) = NaiveDateTime::parse_from_str(&date_text, "%b %d, %Y %I:%M %p") {
        naive
    } else if let Ok(naive) = NaiveDateTime::parse_from_str(&date_text, "%b %d %Y %I:%M %p") {
        naive
    } else if let Ok(time) = NaiveTime::parse_from_str(&date_text, "%I:%M %p") {
        let today = Local::now().date_naive();
        let candidate = today.and_time(time);
        let local_candidate = resolve_local_datetime(candidate)?;
        if local_candidate < Local::now() {
            today.succ_opt()?.and_time(time)
        } else {
            candidate
        }
    } else {
        return None;
    };
    resolve_local_datetime(naive).map(|dt| dt.with_timezone(&Utc))
}

fn resolve_local_datetime(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
}

fn strip_ordinal_day_suffix(value: &str) -> String {
    let (stem, comma) = value
        .strip_suffix(',')
        .map_or((value, false), |stem| (stem, true));
    let bytes = stem.as_bytes();
    if bytes.len() >= 3
        && bytes[bytes.len() - 3].is_ascii_digit()
        && matches!(
            (bytes[bytes.len() - 2], bytes[bytes.len() - 1]),
            (b's', b't') | (b'n', b'd') | (b'r', b'd') | (b't', b'h')
        )
    {
        let mut stripped = stem[..stem.len() - 2].to_string();
        if comma {
            stripped.push(',');
        }
        stripped
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use chrono::Timelike;

    #[test]
    fn parses_usage_limit_reset_timestamp_in_local_timezone() {
        let reset_at = parse_usage_limit_reset_timestamp(
            "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Jul 10th, 2026 4:53 PM.",
        )
        .expect("reset timestamp should parse")
        .with_timezone(&Local);

        assert_eq!(reset_at.year(), 2026);
        assert_eq!(reset_at.month(), 7);
        assert_eq!(reset_at.day(), 10);
        assert_eq!(reset_at.hour(), 16);
        assert_eq!(reset_at.minute(), 53);
    }

    #[test]
    fn returns_none_when_usage_limit_message_has_no_reset_timestamp() {
        assert!(parse_usage_limit_reset_timestamp("You've hit your usage limit.").is_none());
    }

    #[test]
    fn parses_same_day_usage_limit_reset_timestamp_in_local_timezone() {
        let reset_at =
            parse_usage_limit_reset_timestamp("Usage limit reached; try again at 4:53 PM")
                .expect("reset timestamp should parse")
                .with_timezone(&Local);

        assert_eq!(reset_at.hour(), 16);
        assert_eq!(reset_at.minute(), 53);
    }
}
