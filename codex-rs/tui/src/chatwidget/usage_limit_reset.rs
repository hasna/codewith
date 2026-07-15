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
use codex_model_provider_info::CHATGPT_CODEX_BASE_URL;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::provider_base_url_matches;

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

mod automatic;
mod manual;
impl ChatWidget {
    pub(crate) fn advance_rate_limit_reset_generation(&mut self) -> u64 {
        let Some(next_generation) = self.rate_limit_reset_generation.checked_add(1) else {
            tracing::error!("usage-limit reset generation exhausted");
            std::process::abort();
        };
        self.rate_limit_reset_generation = next_generation;
        self.rate_limit_reset_generation
    }

    fn uses_canonical_codex_backend_for_usage_reset(&self) -> bool {
        self.config.model_provider_id == OPENAI_PROVIDER_ID
            && self.config.model_provider.requires_openai_auth
            && self
                .runtime_model_provider_base_url
                .as_deref()
                .is_some_and(|base_url| provider_base_url_matches(base_url, CHATGPT_CODEX_BASE_URL))
    }

    pub(crate) fn on_rate_limit_account_identity(&mut self, fingerprint: String) {
        if self
            .rate_limit_reset_account_identity_fingerprint
            .as_ref()
            .is_some_and(|current| current != &fingerprint)
        {
            self.rate_limit_reset_credits = None;
            self.announced_rate_limit_reset_available_count = None;
        }
        self.rate_limit_reset_account_identity_fingerprint = Some(fingerprint);
    }

    pub(crate) fn begin_selected_auth_profile_credential_mutation(&mut self, profile: &str) {
        if self.config.selected_auth_profile.as_deref() == Some(profile) {
            self.selected_auth_profile_credential_mutation_in_flight = Some(profile.to_string());
        }
    }

    pub(crate) fn finish_selected_auth_profile_credential_mutation(
        &mut self,
        profile: &str,
        credentials_changed: bool,
    ) {
        if self
            .selected_auth_profile_credential_mutation_in_flight
            .as_deref()
            != Some(profile)
        {
            return;
        }
        self.selected_auth_profile_credential_mutation_in_flight = None;
        if credentials_changed && self.config.selected_auth_profile.as_deref() == Some(profile) {
            self.invalidate_rate_limit_reset_state_after_account_update();
        }
    }

    pub(crate) fn selected_auth_profile_credential_mutation_in_flight(&self) -> bool {
        self.selected_auth_profile_credential_mutation_in_flight
            .is_some()
    }

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
            Ok(response)
                if response.account_identity_fingerprint
                    != attempt.account_identity_fingerprint =>
            {
                self.add_error_message(
                    "Usage limit reset stopped because the authenticated account changed."
                        .to_string(),
                );
                self.finish_automatic_reset_without_resuming(&attempt);
                RateLimitResetCompletion::Ignore
            }
            Ok(response) => match response.outcome {
                ConsumeAccountRateLimitResetCreditOutcome::Reset
                | ConsumeAccountRateLimitResetCreditOutcome::AlreadyRedeemed => {
                    let mut attempt = attempt;
                    attempt.verification = RateLimitResetVerification::LimitsOnly;
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
                ConsumeAccountRateLimitResetCreditOutcome::AccountChanged => {
                    self.add_error_message(
                        "Usage limit reset stopped because the authenticated account changed."
                            .to_string(),
                    );
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
        if !attempt.automatic {
            attempt.verification = RateLimitResetVerification::ExactCreditRedemption;
        }
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
        if attempt.verification == RateLimitResetVerification::ExactCreditRedemption
            && !self.exact_credit_redemption_is_verified(&attempt.credit_id)
        {
            self.add_error_message(
                "Couldn't confirm exact usage limit reset redemption; no additional reset was attempted."
                    .to_string(),
            );
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

    fn invalidate_pending_automatic_reset(&mut self) {
        self.advance_rate_limit_reset_generation();
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
        self.advance_rate_limit_reset_generation();
        self.rate_limit_reset_credits = None;
        self.rate_limit_reset_account_identity_fingerprint = None;
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
}

impl ChatWidget {
    pub(crate) fn post_reset_refresh_requires_credit_details(
        &self,
        origin: &RateLimitRefreshOrigin,
    ) -> bool {
        let RateLimitRefreshOrigin::PostReset { generation } = origin else {
            return false;
        };
        self.pending_post_reset_refresh
            .as_ref()
            .is_some_and(|attempt| {
                attempt.generation == *generation
                    && attempt.verification == RateLimitResetVerification::ExactCreditRedemption
            })
    }

    pub(crate) fn rate_limit_reset_refresh_account_is_current(
        &self,
        origin: &RateLimitRefreshOrigin,
        fingerprint: &str,
    ) -> bool {
        let RateLimitRefreshOrigin::PostReset { generation } = origin else {
            return true;
        };
        self.pending_post_reset_refresh
            .as_ref()
            .is_some_and(|attempt| {
                attempt.generation == *generation
                    && attempt.account_identity_fingerprint == fingerprint
            })
    }

    fn exact_credit_redemption_is_verified(&self, credit_id: &str) -> bool {
        self.rate_limit_reset_credits
            .as_ref()
            .and_then(|summary| summary.credits.as_deref())
            .is_some_and(|credits| {
                credits.iter().any(|credit| {
                    credit.id == credit_id
                        && credit.reset_type == RateLimitResetType::CodexRateLimits
                        && credit.status == RateLimitResetCreditStatus::Redeemed
                })
            })
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
