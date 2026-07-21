use super::*;

impl ChatWidget {
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
}
