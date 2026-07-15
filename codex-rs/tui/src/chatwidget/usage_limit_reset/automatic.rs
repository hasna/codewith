use super::*;

impl ChatWidget {
    pub(crate) fn request_usage_limit_auto_reset_check(
        &mut self,
    ) -> UsageLimitAutoResetCheckOutcome {
        if self.automatic_usage_limit_reset_owns_failed_turn() {
            return UsageLimitAutoResetCheckOutcome::AlreadyInProgress;
        }
        if self.manual_usage_limit_reset_is_active() {
            return UsageLimitAutoResetCheckOutcome::Unavailable;
        }
        if self.automatic_reset_opted_out_generation.is_some() {
            return UsageLimitAutoResetCheckOutcome::OptedOut;
        }
        if !self.config.usage_limit.auto_reset_enabled {
            return UsageLimitAutoResetCheckOutcome::Unavailable;
        }
        if !self.uses_canonical_codex_backend_for_usage_reset()
            || self.selected_auth_profile_credential_mutation_in_flight()
        {
            return UsageLimitAutoResetCheckOutcome::Unavailable;
        }
        let generation = self.advance_rate_limit_reset_generation();
        self.automatic_reset_opted_out_generation = None;
        self.pending_usage_limit_auto_reset_check = Some(generation);
        self.prepare_for_usage_limit_reset();
        self.app_event_tx.send(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::AutoResetCheck { generation },
            target: RateLimitRefreshTarget::Selected,
        });
        UsageLimitAutoResetCheckOutcome::Started
    }

    pub(crate) fn automatic_usage_limit_reset_owns_failed_turn(&self) -> bool {
        self.pending_usage_limit_auto_reset_check.is_some()
            || self
                .pending_rate_limit_reset_consumption
                .as_ref()
                .is_some_and(|attempt| attempt.automatic)
            || self
                .rate_limit_reset_in_flight
                .as_ref()
                .is_some_and(|attempt| attempt.automatic)
            || self
                .rate_limit_reset_retry
                .as_ref()
                .is_some_and(|attempt| attempt.automatic)
            || self
                .pending_post_reset_refresh
                .as_ref()
                .is_some_and(|attempt| attempt.automatic)
    }

    pub(crate) fn finish_usage_limit_auto_reset_check(
        &mut self,
        generation: u64,
        result: Result<(), String>,
    ) {
        if self.pending_usage_limit_auto_reset_check != Some(generation)
            || generation != self.rate_limit_reset_generation
        {
            return;
        }
        self.pending_usage_limit_auto_reset_check = None;
        if let Err(message) = result {
            self.add_error_message(format!("Couldn't check usage limit resets: {message}"));
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return;
        }
        if !self.config.usage_limit.auto_reset_enabled {
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return;
        }

        let Some(trigger_key) = self.weekly_usage_limit_auto_reset_key() else {
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return;
        };
        if self.usage_limit_auto_reset_key.as_deref() == Some(trigger_key.as_str()) {
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return;
        }
        let Some(credit_id) = self.rate_limit_reset_credits.as_ref().and_then(|summary| {
            available_reset_credits(summary, Utc::now().timestamp())
                .first()
                .map(|credit| credit.id.clone())
        }) else {
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return;
        };
        let Some(account_identity_fingerprint) =
            self.rate_limit_reset_account_identity_fingerprint.clone()
        else {
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return;
        };

        let attempt = RateLimitResetAttempt {
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            credit_id,
            auth_profile: self.config.selected_auth_profile.clone(),
            account_identity_fingerprint,
            generation,
            automatic: true,
            trigger_key: Some(trigger_key),
            retry_count: 0,
            verification: RateLimitResetVerification::LimitsOnly,
        };
        self.pending_rate_limit_reset_consumption = Some(attempt.clone());
        self.app_event_tx
            .send(AppEvent::ConsumeRateLimitResetCredit { attempt });
    }

    pub(in crate::chatwidget) fn set_usage_limit_auto_reset_enabled(&mut self, enabled: bool) {
        self.config.usage_limit.auto_reset_enabled = enabled;
        if enabled {
            return;
        }

        let post_was_dispatched = self
            .rate_limit_reset_in_flight
            .as_ref()
            .is_some_and(|attempt| attempt.automatic)
            || self
                .rate_limit_reset_retry
                .as_ref()
                .is_some_and(|attempt| attempt.automatic)
            || self
                .pending_post_reset_refresh
                .as_ref()
                .is_some_and(|attempt| attempt.automatic);
        if post_was_dispatched {
            self.automatic_reset_opted_out_generation = Some(self.rate_limit_reset_generation);
            self.prepare_for_usage_limit_reset();
            self.add_info_message(
                "Automatic usage reset disabled. Any reset request already sent will be verified, but the failed turn will not resume automatically."
                    .to_string(),
                /*hint*/ None,
            );
            if let Some(attempt) = self
                .rate_limit_reset_retry
                .take()
                .filter(|attempt| attempt.automatic)
            {
                if let Some(trigger_key) = attempt.trigger_key.clone() {
                    self.usage_limit_auto_reset_key = Some(trigger_key);
                }
                self.pending_post_reset_refresh = Some(attempt.clone());
                self.app_event_tx.send(AppEvent::RefreshRateLimits {
                    origin: RateLimitRefreshOrigin::PostReset {
                        generation: attempt.generation,
                    },
                    target: RateLimitRefreshTarget::Selected,
                });
            }
        } else if self.pending_usage_limit_auto_reset_check.is_some()
            || self
                .pending_rate_limit_reset_consumption
                .as_ref()
                .is_some_and(|attempt| attempt.automatic)
        {
            self.invalidate_pending_automatic_reset();
            self.fallback_auth_profile_switch_after_reset_unavailable();
        }
    }

    pub(in crate::chatwidget) fn usage_limit_reset_takes_precedence(&self) -> bool {
        if self.automatic_usage_limit_reset_owns_failed_turn()
            || self.manual_usage_limit_reset_is_active()
        {
            return true;
        }
        self.config.usage_limit.auto_reset_enabled
            && (self.pending_usage_limit_auto_reset_check.is_some()
                || self
                    .rate_limit_reset_credits
                    .as_ref()
                    .is_some_and(|summary| {
                        !available_reset_credits(summary, Utc::now().timestamp()).is_empty()
                    }))
    }

    pub(in crate::chatwidget) fn usage_limit_reset_takes_precedence_for_snapshot(
        &self,
        snapshot: &RateLimitSnapshot,
    ) -> bool {
        if self.automatic_usage_limit_reset_owns_failed_turn()
            || self.manual_usage_limit_reset_is_active()
        {
            return true;
        }
        weekly_exhausted_window(snapshot).is_some()
            && self.config.usage_limit.auto_reset_enabled
            && (self.pending_usage_limit_auto_reset_check.is_some()
                || self
                    .rate_limit_reset_credits
                    .as_ref()
                    .is_some_and(|summary| {
                        !available_reset_credits(summary, Utc::now().timestamp()).is_empty()
                    }))
    }

    pub(super) fn fallback_auth_profile_switch_after_reset_unavailable(&mut self) {
        if self.try_auth_profile_switch_after_reset_unavailable() {
            return;
        }
        let retry_delay = self.maybe_schedule_usage_self_heal_retry(
            UsageSelfHealErrorKind::UsageLimit,
            /*error_message*/ None,
        );
        if let Some(retry_delay) = retry_delay {
            self.add_info_message(
                format!(
                    "No usable reset or alternate profile was available; retrying this turn in {}.",
                    Self::usage_self_heal_delay_label(retry_delay)
                ),
                /*hint*/ None,
            );
        } else {
            self.maybe_send_next_queued_input();
        }
    }

    pub(in crate::chatwidget) fn try_auth_profile_switch_after_reset_unavailable(
        &mut self,
    ) -> bool {
        let exhausted = self
            .auth_profile_auto_switch_snapshots_by_limit_id
            .iter()
            .map(|(limit_id, snapshot)| (limit_id.clone(), snapshot.clone()))
            .collect::<Vec<_>>();
        for (limit_id, snapshot) in exhausted {
            self.maybe_auto_switch_auth_profile_for_rate_limit(&limit_id, &snapshot);
            if self.pending_auth_profile_auto_switch_trigger.is_some() {
                break;
            }
        }
        self.pending_auth_profile_auto_switch_trigger.is_some()
    }

    pub(super) fn finish_automatic_reset_without_resuming(
        &mut self,
        attempt: &RateLimitResetAttempt,
    ) {
        if !attempt.automatic {
            return;
        }
        if self.automatic_reset_was_opted_out(attempt) {
            self.maybe_send_next_queued_input();
        } else {
            self.fallback_auth_profile_switch_after_reset_unavailable();
        }
    }

    pub(super) fn automatic_reset_was_opted_out(&self, attempt: &RateLimitResetAttempt) -> bool {
        attempt.automatic && self.automatic_reset_opted_out_generation == Some(attempt.generation)
    }

    pub(super) fn weekly_usage_limit_auto_reset_key(&self) -> Option<String> {
        let snapshot = self
            .auth_profile_auto_switch_snapshots_by_limit_id
            .iter()
            .find(|(limit_id, _)| limit_id.eq_ignore_ascii_case("codex"))
            .map(|(_, snapshot)| snapshot)?;
        let window = weekly_exhausted_window(snapshot)?;
        Some(format!(
            "{:?}:codex:weekly:{:?}",
            self.config.selected_auth_profile, window.resets_at
        ))
    }
}
