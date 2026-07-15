use super::*;

impl ChatWidget {
    pub(crate) fn advance_rate_limit_reset_generation(&mut self) -> u64 {
        let Some(next_generation) = self.rate_limit_reset_generation.checked_add(1) else {
            tracing::error!("usage-limit reset generation exhausted");
            std::process::abort();
        };
        self.rate_limit_reset_generation = next_generation;
        self.rate_limit_reset_generation
    }

    pub(super) fn uses_canonical_codex_backend_for_usage_reset(&self) -> bool {
        self.config.model_provider_id == OPENAI_PROVIDER_ID
            && self.config.model_provider.requires_openai_auth
            && self
                .config
                .model_provider
                .base_url
                .as_deref()
                .is_none_or(|base_url| provider_base_url_matches(base_url, CHATGPT_CODEX_BASE_URL))
            && self
                .runtime_model_provider_base_url
                .as_deref()
                .is_none_or(|base_url| provider_base_url_matches(base_url, CHATGPT_CODEX_BASE_URL))
            && provider_base_url_matches(&self.config.chatgpt_base_url, CHATGPT_BACKEND_BASE_URL)
    }

    pub(crate) fn on_rate_limit_account_identity(&mut self, fingerprint: Option<String>) {
        if fingerprint.is_none()
            || self
                .rate_limit_reset_account_identity_fingerprint
                .as_ref()
                .zip(fingerprint.as_ref())
                .is_some_and(|(current, next)| current != next)
        {
            self.rate_limit_reset_credits = None;
            self.announced_rate_limit_reset_available_count = None;
        }
        self.rate_limit_reset_account_identity_fingerprint = fingerprint;
    }

    pub(crate) fn begin_selected_auth_profile_credential_mutation(&mut self, profile: &str) {
        *self
            .auth_profile_credential_mutations_in_flight
            .entry(profile.to_string())
            .or_default() += 1;
    }

    pub(crate) fn finish_selected_auth_profile_credential_mutation(
        &mut self,
        profile: &str,
        credentials_changed: bool,
    ) {
        let Some(count) = self
            .auth_profile_credential_mutations_in_flight
            .get_mut(profile)
        else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            self.auth_profile_credential_mutations_in_flight
                .remove(profile);
        }
        if credentials_changed && self.config.selected_auth_profile.as_deref() == Some(profile) {
            self.invalidate_rate_limit_reset_state_after_account_update();
        }
    }

    pub(crate) fn selected_auth_profile_credential_mutation_in_flight(&self) -> bool {
        self.config
            .selected_auth_profile
            .as_deref()
            .is_some_and(|profile| {
                self.auth_profile_credential_mutations_in_flight
                    .contains_key(profile)
            })
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

    pub(super) fn invalidate_pending_automatic_reset(&mut self) {
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
        self.manual_rate_limit_reset_authority = None;
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
        fingerprint: Option<&str>,
    ) -> bool {
        let RateLimitRefreshOrigin::PostReset { generation } = origin else {
            return true;
        };
        self.pending_post_reset_refresh
            .as_ref()
            .is_some_and(|attempt| {
                attempt.generation == *generation
                    && fingerprint.is_some_and(|fingerprint| {
                        attempt.account_identity_fingerprint == fingerprint
                    })
            })
    }

    pub(super) fn exact_credit_redemption_is_verified(&self, credit_id: &str) -> bool {
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
