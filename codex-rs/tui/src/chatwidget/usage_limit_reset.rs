use super::*;
use chrono::Utc;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditResponse;
use codex_app_server_protocol::RateLimitResetCredit;
use codex_app_server_protocol::RateLimitResetCreditStatus;
use codex_app_server_protocol::RateLimitResetCreditsSummary;
use codex_app_server_protocol::RateLimitResetType;
use codex_app_server_protocol::RateLimitSnapshot;
use codex_app_server_protocol::RateLimitWindow;

const WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;
const MAX_AMBIGUOUS_RETRIES: u8 = 1;
const RATE_LIMIT_RESET_CONFIRM_VIEW_ID: &str = "usage-limit-reset-confirmation";

#[derive(Debug)]
pub(crate) enum RateLimitResetCompletion {
    Ignore,
    Retry(RateLimitResetAttempt),
    Verify(RateLimitResetAttempt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageLimitAutoResetCheckOutcome {
    Started,
    AlreadyInProgress,
    OptedOut,
    Unavailable,
}

impl ChatWidget {
    pub(crate) fn on_rate_limit_reset_credits(
        &mut self,
        reset_credits: Option<RateLimitResetCreditsSummary>,
    ) {
        let available_count = reset_credits
            .as_ref()
            .map(|credits| credits.available_count.max(0));
        self.rate_limit_reset_credits = reset_credits;

        match available_count {
            Some(count) if count > 0 => {
                if self.announced_rate_limit_reset_available_count != Some(count) {
                    self.announced_rate_limit_reset_available_count = Some(count);
                    self.add_info_message(
                        format!(
                            "You have {count} {} available. Run /usage to use one.",
                            reset_label(count)
                        ),
                        /*hint*/ None,
                    );
                }
            }
            Some(_) => self.announced_rate_limit_reset_available_count = None,
            None => {}
        }

        self.refresh_usage_panel_if_active();
    }

    pub(crate) fn start_rate_limit_reset_picker(&mut self) {
        if self.automatic_usage_limit_reset_owns_failed_turn() {
            self.add_error_message(
                "A usage-limit reset is already recovering the failed turn.".to_string(),
            );
            return;
        }
        let generation = self.rate_limit_reset_generation;
        if self.pending_rate_limit_reset_picker == Some(generation) {
            return;
        }
        self.pending_rate_limit_reset_picker = Some(generation);
        self.add_info_message(
            "Refreshing available usage limit resets…".to_string(),
            /*hint*/ None,
        );
        self.app_event_tx.send(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::ResetPicker { generation },
            target: RateLimitRefreshTarget::Selected,
        });
    }

    pub(crate) fn finish_rate_limit_reset_picker(
        &mut self,
        generation: u64,
        result: Result<(), String>,
    ) {
        if self.pending_rate_limit_reset_picker != Some(generation)
            || generation != self.rate_limit_reset_generation
        {
            return;
        }
        self.pending_rate_limit_reset_picker = None;
        if let Err(message) = result {
            self.add_error_message(format!("Couldn't refresh usage limit resets: {message}"));
            return;
        }
        self.open_rate_limit_reset_confirm();
    }

    pub(crate) fn open_rate_limit_reset_confirm(&mut self) {
        if self.automatic_usage_limit_reset_owns_failed_turn() {
            self.add_error_message(
                "A usage-limit reset is already recovering the failed turn.".to_string(),
            );
            return;
        }
        let Some(summary) = self.rate_limit_reset_credits.as_ref() else {
            self.add_info_message(
                "Usage limit reset details are unavailable right now.".to_string(),
                /*hint*/ None,
            );
            return;
        };
        let credits = available_reset_credits(summary, Utc::now().timestamp());
        if credits.is_empty() {
            self.add_info_message(
                "No usable usage limit resets are available right now.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let auth_profile = self.config.selected_auth_profile.clone();
        let generation = self.rate_limit_reset_generation;
        let mut items = Vec::with_capacity(credits.len() + 1);
        for credit in credits {
            let credit_id = credit.id.clone();
            let name = credit
                .title
                .clone()
                .unwrap_or_else(|| "Use a reset".to_string());
            let description = credit.description.clone().or_else(|| {
                credit
                    .expires_at
                    .map(|expires_at| format!("Expires at Unix time {expires_at}."))
            });
            let auth_profile = auth_profile.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::ConsumeRateLimitResetCredit {
                    attempt: RateLimitResetAttempt {
                        idempotency_key: uuid::Uuid::new_v4().to_string(),
                        credit_id: credit_id.clone(),
                        auth_profile: auth_profile.clone(),
                        generation,
                        automatic: false,
                        trigger_key: None,
                        retry_count: 0,
                    },
                });
            })];
            items.push(SelectionItem {
                name,
                description,
                actions,
                dismiss_on_select: false,
                ..Default::default()
            });
        }
        items.push(SelectionItem {
            name: "Cancel".to_string(),
            display_shortcut: Some(key_hint::plain(KeyCode::Char('n'))),
            is_default: true,
            dismiss_on_select: true,
            ..Default::default()
        });
        let selected = items.len().saturating_sub(1);

        self.bottom_pane.show_selection_view(SelectionViewParams {
            view_id: Some(RATE_LIMIT_RESET_CONFIRM_VIEW_ID),
            title: Some("Usage limit resets".to_string()),
            subtitle: Some(format!(
                "Choose one of {} exact {}.",
                selected,
                reset_label(i64::try_from(selected).unwrap_or(i64::MAX))
            )),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx: Some(selected),
            ..Default::default()
        });
    }

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
        if !self.config.model_provider.is_openai()
            || !self.config.model_provider.requires_openai_auth
        {
            return UsageLimitAutoResetCheckOutcome::Unavailable;
        }
        self.rate_limit_reset_generation = self.rate_limit_reset_generation.saturating_add(1);
        let generation = self.rate_limit_reset_generation;
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

    pub(crate) fn manual_usage_limit_reset_is_active(&self) -> bool {
        self.bottom_pane
            .has_view_id(RATE_LIMIT_RESET_CONFIRM_VIEW_ID)
            || self.pending_rate_limit_reset_picker.is_some()
            || self
                .pending_rate_limit_reset_consumption
                .as_ref()
                .is_some_and(|attempt| !attempt.automatic)
            || self
                .rate_limit_reset_in_flight
                .as_ref()
                .is_some_and(|attempt| !attempt.automatic)
            || self
                .rate_limit_reset_retry
                .as_ref()
                .is_some_and(|attempt| !attempt.automatic)
            || self
                .pending_post_reset_refresh
                .as_ref()
                .is_some_and(|attempt| !attempt.automatic)
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

        let attempt = RateLimitResetAttempt {
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            credit_id,
            auth_profile: self.config.selected_auth_profile.clone(),
            generation,
            automatic: true,
            trigger_key: Some(trigger_key),
            retry_count: 0,
        };
        self.pending_rate_limit_reset_consumption = Some(attempt.clone());
        self.app_event_tx
            .send(AppEvent::ConsumeRateLimitResetCredit { attempt });
    }

    pub(crate) fn start_rate_limit_reset_consumption(
        &mut self,
        attempt: &RateLimitResetAttempt,
    ) -> bool {
        if !attempt.automatic && self.automatic_usage_limit_reset_owns_failed_turn() {
            return false;
        }
        let expected_automatic_attempt = if attempt.automatic {
            if attempt.retry_count == 0 {
                self.pending_rate_limit_reset_consumption.as_ref()
            } else {
                self.rate_limit_reset_retry.as_ref()
            }
        } else {
            Some(attempt)
        };
        if attempt.generation != self.rate_limit_reset_generation
            || attempt.auth_profile != self.config.selected_auth_profile
            || self.rate_limit_reset_in_flight.is_some()
            || self.pending_post_reset_refresh.is_some()
            || attempt.credit_id.is_empty()
            || expected_automatic_attempt != Some(attempt)
        {
            return false;
        }
        if attempt.automatic && !self.config.usage_limit.auto_reset_enabled {
            self.invalidate_pending_automatic_reset();
            self.fallback_auth_profile_switch_after_reset_unavailable();
            return false;
        }
        if attempt.automatic {
            self.prepare_for_usage_limit_reset();
        }
        self.pending_rate_limit_reset_consumption = None;
        self.rate_limit_reset_retry = None;
        self.rate_limit_reset_in_flight = Some(attempt.clone());
        if !attempt.automatic {
            self.bottom_pane
                .dismiss_active_view_if_id(RATE_LIMIT_RESET_CONFIRM_VIEW_ID);
        }
        tracing::info!(
            automatic = attempt.automatic,
            "attempting usage-limit reset credit consumption"
        );
        self.add_info_message(
            if attempt.automatic {
                "Weekly usage limit exhausted; attempting one exact banked reset.".to_string()
            } else {
                "Attempting to use the selected usage limit reset.".to_string()
            },
            /*hint*/ None,
        );
        true
    }

    pub(crate) fn finish_rate_limit_reset_consumption(
        &mut self,
        attempt: RateLimitResetAttempt,
        response: Result<ConsumeAccountRateLimitResetCreditResponse, String>,
    ) -> RateLimitResetCompletion {
        if self.rate_limit_reset_in_flight.as_ref() != Some(&attempt)
            || attempt.generation != self.rate_limit_reset_generation
            || attempt.auth_profile != self.config.selected_auth_profile
        {
            return RateLimitResetCompletion::Ignore;
        }
        self.rate_limit_reset_in_flight = None;

        match response {
            Ok(response) => match response.outcome {
                ConsumeAccountRateLimitResetCreditOutcome::Reset
                | ConsumeAccountRateLimitResetCreditOutcome::AlreadyRedeemed => {
                    if let Some(trigger_key) = attempt.trigger_key.clone() {
                        self.usage_limit_auto_reset_key = Some(trigger_key);
                    }
                    if attempt.automatic {
                        self.prepare_for_usage_limit_reset();
                    }
                    self.pending_post_reset_refresh = Some(attempt.clone());
                    self.add_info_message(
                        if self.automatic_reset_was_opted_out(&attempt) {
                            "Usage limit reset accepted after automatic reset was disabled. Verifying current limits without resuming the failed turn."
                                .to_string()
                        } else {
                            "Usage limit reset accepted. Verifying current limits…".to_string()
                        },
                        /*hint*/ None,
                    );
                    RateLimitResetCompletion::Verify(attempt)
                }
                ConsumeAccountRateLimitResetCreditOutcome::NothingToReset => {
                    self.add_info_message(
                        "Your usage does not need a reset right now.".to_string(),
                        /*hint*/ None,
                    );
                    self.finish_automatic_reset_without_resuming(&attempt);
                    RateLimitResetCompletion::Ignore
                }
                ConsumeAccountRateLimitResetCreditOutcome::NoCredit => {
                    self.rate_limit_reset_credits = Some(RateLimitResetCreditsSummary {
                        available_count: 0,
                        credits: Some(Vec::new()),
                    });
                    self.announced_rate_limit_reset_available_count = None;
                    self.add_error_message("No usage limit resets are available.".to_string());
                    self.finish_automatic_reset_without_resuming(&attempt);
                    RateLimitResetCompletion::Ignore
                }
                ConsumeAccountRateLimitResetCreditOutcome::Unknown => {
                    self.retry_or_report_ambiguous(attempt, "the server returned an unknown result")
                }
            },
            Err(message) => self.retry_or_report_ambiguous(attempt, &message),
        }
    }

    fn retry_or_report_ambiguous(
        &mut self,
        mut attempt: RateLimitResetAttempt,
        message: &str,
    ) -> RateLimitResetCompletion {
        if self.automatic_reset_was_opted_out(&attempt) {
            self.rate_limit_reset_retry = None;
            if let Some(trigger_key) = attempt.trigger_key.clone() {
                self.usage_limit_auto_reset_key = Some(trigger_key);
            }
            self.prepare_for_usage_limit_reset();
            self.pending_post_reset_refresh = Some(attempt.clone());
            self.add_error_message(format!(
                "Couldn't confirm the usage reset after automatic reset was disabled: {message}. Checking current limits without another reset request."
            ));
            return RateLimitResetCompletion::Verify(attempt);
        }
        if attempt.retry_count < MAX_AMBIGUOUS_RETRIES {
            attempt.retry_count += 1;
            self.rate_limit_reset_retry = Some(attempt.clone());
            self.add_info_message(
                "Reset result was ambiguous; retrying with the same request key.".to_string(),
                /*hint*/ None,
            );
            return RateLimitResetCompletion::Retry(attempt);
        }
        self.rate_limit_reset_retry = None;
        if let Some(trigger_key) = attempt.trigger_key.clone() {
            self.usage_limit_auto_reset_key = Some(trigger_key);
        }
        if attempt.automatic {
            self.prepare_for_usage_limit_reset();
        }
        self.pending_post_reset_refresh = Some(attempt.clone());
        self.add_error_message(format!(
            "Couldn't confirm the usage reset: {message}. Checking current limits before continuing."
        ));
        RateLimitResetCompletion::Verify(attempt)
    }

    pub(crate) fn finish_post_reset_refresh(
        &mut self,
        generation: u64,
        result: Result<(), String>,
    ) {
        let Some(attempt) = self.pending_post_reset_refresh.take() else {
            return;
        };
        if attempt.generation != generation
            || generation != self.rate_limit_reset_generation
            || attempt.auth_profile != self.config.selected_auth_profile
        {
            self.pending_post_reset_refresh = Some(attempt);
            return;
        }
        if attempt.automatic {
            self.prepare_for_usage_limit_reset();
        }
        let opted_out = self.automatic_reset_was_opted_out(&attempt);
        if let Err(message) = result {
            self.add_error_message(format!("Couldn't verify the usage reset: {message}"));
            self.finish_automatic_reset_without_resuming(&attempt);
            return;
        }
        if self.weekly_usage_limit_auto_reset_key().is_some() {
            self.add_error_message(
                "The weekly usage limit is still exhausted after the reset.".to_string(),
            );
            self.finish_automatic_reset_without_resuming(&attempt);
            return;
        }
        self.add_info_message(
            if opted_out {
                "Usage limit reset verified. Automatic reset remains disabled for the failed turn."
                    .to_string()
            } else {
                "Usage limit reset verified.".to_string()
            },
            /*hint*/ None,
        );
        if attempt.automatic && !opted_out {
            if let Some(trigger_key) = attempt.trigger_key {
                self.usage_limit_auto_reset_key = Some(trigger_key);
            }
            self.resume_after_usage_limit_reset();
        } else if attempt.automatic {
            self.maybe_send_next_queued_input();
        }
    }

    pub(super) fn set_usage_limit_auto_reset_enabled(&mut self, enabled: bool) {
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

    fn invalidate_pending_automatic_reset(&mut self) {
        self.rate_limit_reset_generation = self.rate_limit_reset_generation.saturating_add(1);
        self.pending_usage_limit_auto_reset_check = None;
        if self
            .pending_rate_limit_reset_consumption
            .as_ref()
            .is_some_and(|attempt| attempt.automatic)
        {
            self.pending_rate_limit_reset_consumption = None;
        }
        if self
            .rate_limit_reset_retry
            .as_ref()
            .is_some_and(|attempt| attempt.automatic)
        {
            self.rate_limit_reset_retry = None;
        }
        self.automatic_reset_opted_out_generation = None;
    }

    pub(crate) fn is_rate_limit_reset_generation_current(&self, generation: u64) -> bool {
        generation == self.rate_limit_reset_generation
    }

    pub(crate) fn is_rate_limit_reset_refresh_current(
        &self,
        origin: &RateLimitRefreshOrigin,
    ) -> bool {
        match origin {
            RateLimitRefreshOrigin::ResetPicker { generation } => {
                self.pending_rate_limit_reset_picker == Some(*generation)
                    && self.is_rate_limit_reset_generation_current(*generation)
            }
            RateLimitRefreshOrigin::AutoResetCheck { generation } => {
                self.pending_usage_limit_auto_reset_check == Some(*generation)
                    && self.is_rate_limit_reset_generation_current(*generation)
            }
            RateLimitRefreshOrigin::PostReset { generation } => {
                self.pending_post_reset_refresh
                    .as_ref()
                    .is_some_and(|attempt| attempt.generation == *generation)
                    && self.is_rate_limit_reset_generation_current(*generation)
            }
            RateLimitRefreshOrigin::StartupPrefetch
            | RateLimitRefreshOrigin::Heartbeat
            | RateLimitRefreshOrigin::UsagePanel { .. }
            | RateLimitRefreshOrigin::StatusCommand { .. } => true,
        }
    }

    pub(crate) fn invalidate_rate_limit_reset_state_after_account_update(&mut self) {
        let automatic_reset_owned_failed_turn = self.automatic_usage_limit_reset_owns_failed_turn();
        self.rate_limit_reset_generation = self.rate_limit_reset_generation.saturating_add(1);
        self.rate_limit_reset_credits = None;
        self.announced_rate_limit_reset_available_count = None;
        self.pending_rate_limit_reset_consumption = None;
        self.rate_limit_reset_in_flight = None;
        self.rate_limit_reset_retry = None;
        self.pending_rate_limit_reset_picker = None;
        self.pending_usage_limit_auto_reset_check = None;
        self.pending_post_reset_refresh = None;
        self.automatic_reset_opted_out_generation = None;
        self.usage_limit_auto_reset_key = None;
        self.auth_profile_auto_switch_snapshots_by_limit_id.clear();
        self.prepare_for_usage_limit_reset();
        if automatic_reset_owned_failed_turn {
            self.fallback_auth_profile_switch_after_reset_unavailable();
        }
    }

    pub(super) fn usage_limit_reset_takes_precedence(&self) -> bool {
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

    pub(super) fn usage_limit_reset_takes_precedence_for_snapshot(
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

    fn fallback_auth_profile_switch_after_reset_unavailable(&mut self) {
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

    pub(super) fn try_auth_profile_switch_after_reset_unavailable(&mut self) -> bool {
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

    fn finish_automatic_reset_without_resuming(&mut self, attempt: &RateLimitResetAttempt) {
        if !attempt.automatic {
            return;
        }
        if self.automatic_reset_was_opted_out(attempt) {
            self.maybe_send_next_queued_input();
        } else {
            self.fallback_auth_profile_switch_after_reset_unavailable();
        }
    }

    fn automatic_reset_was_opted_out(&self, attempt: &RateLimitResetAttempt) -> bool {
        attempt.automatic && self.automatic_reset_opted_out_generation == Some(attempt.generation)
    }

    fn weekly_usage_limit_auto_reset_key(&self) -> Option<String> {
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

fn available_reset_credits(
    summary: &RateLimitResetCreditsSummary,
    now: i64,
) -> Vec<&RateLimitResetCredit> {
    if summary.available_count <= 0 {
        return Vec::new();
    }
    let mut credits = summary
        .credits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|credit| {
            !credit.id.is_empty()
                && credit.reset_type == RateLimitResetType::CodexRateLimits
                && credit.status == RateLimitResetCreditStatus::Available
                && credit.expires_at.is_none_or(|expires_at| expires_at > now)
        })
        .collect::<Vec<_>>();
    credits.sort_by(|left, right| {
        left.expires_at
            .unwrap_or(i64::MAX)
            .cmp(&right.expires_at.unwrap_or(i64::MAX))
            .then_with(|| left.id.cmp(&right.id))
    });
    credits
}

fn weekly_exhausted_window(snapshot: &RateLimitSnapshot) -> Option<&RateLimitWindow> {
    if !snapshot
        .limit_id
        .as_deref()
        .is_some_and(|limit_id| limit_id.eq_ignore_ascii_case("codex"))
    {
        return None;
    }
    [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
        .find(|window| {
            window.window_duration_mins == Some(WEEKLY_WINDOW_MINUTES) && window.used_percent >= 100
        })
}

pub(super) fn reset_label(count: i64) -> &'static str {
    if count == 1 {
        "usage limit reset"
    } else {
        "usage limit resets"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn available_reset_credits_are_selected_by_expiry_then_id() {
        let summary = RateLimitResetCreditsSummary {
            available_count: 5,
            credits: Some(vec![
                reset_credit("later", Some(30), RateLimitResetCreditStatus::Available),
                reset_credit("unavailable", Some(5), RateLimitResetCreditStatus::Redeemed),
                reset_credit("expired", Some(9), RateLimitResetCreditStatus::Available),
                reset_credit("earlier-b", Some(20), RateLimitResetCreditStatus::Available),
                reset_credit("earlier-a", Some(20), RateLimitResetCreditStatus::Available),
            ]),
        };

        assert_eq!(
            available_reset_credits(&summary, 10)
                .into_iter()
                .map(|credit| credit.id.as_str())
                .collect::<Vec<_>>(),
            vec!["earlier-a", "earlier-b", "later"],
        );
    }

    #[test]
    fn count_only_unknown_redeeming_and_wrong_type_fail_closed() {
        let count_only = RateLimitResetCreditsSummary {
            available_count: 4,
            credits: None,
        };
        assert!(available_reset_credits(&count_only, 0).is_empty());

        let mut credits = vec![
            reset_credit("unknown", None, RateLimitResetCreditStatus::Unknown),
            reset_credit("redeeming", None, RateLimitResetCreditStatus::Redeeming),
        ];
        credits[0].reset_type = RateLimitResetType::Unknown;
        let summary = RateLimitResetCreditsSummary {
            available_count: 2,
            credits: Some(credits),
        };
        assert!(available_reset_credits(&summary, 0).is_empty());
    }

    fn reset_credit(
        id: &str,
        expires_at: Option<i64>,
        status: RateLimitResetCreditStatus,
    ) -> RateLimitResetCredit {
        RateLimitResetCredit {
            id: id.to_string(),
            reset_type: RateLimitResetType::CodexRateLimits,
            status,
            granted_at: 0,
            expires_at,
            title: None,
            description: None,
        }
    }

    #[test]
    fn weekly_exhausted_window_requires_codex_weekly_duration_and_full_usage() {
        let mut snapshot = RateLimitSnapshot {
            limit_id: Some("codex".to_string()),
            limit_name: Some("Codex".to_string()),
            primary: Some(RateLimitWindow {
                used_percent: 99,
                window_duration_mins: Some(WEEKLY_WINDOW_MINUTES),
                resets_at: Some(10),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: 100,
                window_duration_mins: Some(300),
                resets_at: Some(20),
            }),
            credits: None,
            individual_limit: None,
            plan_type: None,
            rate_limit_reached_type: None,
        };

        assert!(weekly_exhausted_window(&snapshot).is_none());
        snapshot.primary.as_mut().expect("primary").used_percent = 100;
        assert_eq!(
            weekly_exhausted_window(&snapshot).and_then(|window| window.resets_at),
            Some(10)
        );
        snapshot.limit_id = Some("other".to_string());
        assert!(weekly_exhausted_window(&snapshot).is_none());
    }
}
