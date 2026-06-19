//! Automatic retry scheduling for recoverable usage-limit and availability failures.

use super::*;
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
    ) {
        self.usage_self_heal.last_submitted_turn = Some(UsageSelfHealSubmittedTurn {
            user_message: user_message.clone(),
            history_record: history_record.clone(),
        });
    }

    pub(super) fn clear_usage_self_heal_turn(&mut self) {
        self.usage_self_heal.last_submitted_turn = None;
        self.usage_self_heal.pending_retry = None;
        self.usage_self_heal.consecutive_retries = 0;
    }

    pub(super) fn maybe_schedule_usage_self_heal_retry(
        &mut self,
        kind: UsageSelfHealErrorKind,
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
                .usage_self_heal_reset_retry_delay()
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
        self.input_queue
            .queued_user_messages
            .push_front(QueuedUserMessage::new(
                pending.submitted.user_message,
                QueuedInputAction::Plain,
            ));
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

    fn usage_self_heal_reset_retry_delay(&self) -> Option<Duration> {
        let config = &self.config.usage_self_heal;
        let now = Utc::now().timestamp();
        let reset_at = self
            .auth_profile_auto_switch_snapshots_by_limit_id
            .values()
            .flat_map(|snapshot| [snapshot.secondary.as_ref(), snapshot.primary.as_ref()])
            .flatten()
            .filter(|window| window.used_percent >= 100)
            .filter_map(|window| window.resets_at)
            .filter(|reset_at| *reset_at > now)
            .min()?;
        let delay_secs = reset_at
            .saturating_sub(now)
            .saturating_add(i64::try_from(config.reset_retry_buffer_secs).unwrap_or(i64::MAX));
        let delay_secs = u64::try_from(delay_secs).ok()?;
        (delay_secs <= config.max_reset_retry_delay_secs).then(|| Duration::from_secs(delay_secs))
    }
}
