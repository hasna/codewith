use super::*;

impl ChatWidget {
    pub(crate) fn start_rate_limit_reset_picker(&mut self) {
        if !self.uses_canonical_codex_backend_for_usage_reset() {
            self.add_error_message(
                "Usage-limit resets are only available for the canonical OpenAI provider."
                    .to_string(),
            );
            return;
        }
        if self.automatic_usage_limit_reset_owns_failed_turn() {
            self.add_error_message(
                "A usage-limit reset is already recovering the failed turn.".to_string(),
            );
            return;
        }
        if self.manual_usage_limit_reset_is_active() {
            return;
        }
        if self.selected_auth_profile_credential_mutation_in_flight() {
            self.add_error_message(
                "Usage-limit reset is unavailable while the selected profile is logging in."
                    .to_string(),
            );
            return;
        }
        let generation = self.advance_rate_limit_reset_generation();
        self.manual_rate_limit_reset_authority = None;
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
        if !self.uses_canonical_codex_backend_for_usage_reset() {
            self.add_error_message(
                "Usage-limit resets are only available for the canonical OpenAI provider."
                    .to_string(),
            );
            return;
        }
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
        let now = Utc::now().timestamp();
        let credits = available_reset_credits(summary, now);
        if credits.is_empty() {
            self.add_info_message(
                "No usable usage limit resets are available right now.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let auth_profile = self.config.selected_auth_profile.clone();
        let Some(account_identity_fingerprint) =
            self.rate_limit_reset_account_identity_fingerprint.clone()
        else {
            self.add_error_message(
                "Usage limit reset account identity is unavailable; refresh and try again."
                    .to_string(),
            );
            return;
        };
        let generation = self.rate_limit_reset_generation;
        self.manual_rate_limit_reset_authority = Some(ManualRateLimitResetAuthority {
            generation,
            auth_profile: auth_profile.clone(),
            account_identity_fingerprint: account_identity_fingerprint.clone(),
        });
        let mut items = Vec::with_capacity(credits.len() + 1);
        for credit in credits {
            let credit_id = credit.id.clone();
            let name = credit
                .title
                .clone()
                .unwrap_or_else(|| "Use a reset".to_string());
            let description = reset_credit_description(credit, now);
            let auth_profile = auth_profile.clone();
            let account_identity_fingerprint = account_identity_fingerprint.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::ConsumeRateLimitResetCredit {
                    attempt: RateLimitResetAttempt {
                        idempotency_key: uuid::Uuid::new_v4().to_string(),
                        credit_id: credit_id.clone(),
                        auth_profile: auth_profile.clone(),
                        account_identity_fingerprint: account_identity_fingerprint.clone(),
                        generation,
                        automatic: false,
                        trigger_key: None,
                        retry_count: 0,
                        verification: RateLimitResetVerification::LimitsOnly,
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
        let cancel_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::CancelRateLimitResetCreditSelection { generation });
        })];
        items.push(SelectionItem {
            name: "Cancel".to_string(),
            display_shortcut: Some(key_hint::plain(KeyCode::Char('n'))),
            is_default: true,
            actions: cancel_actions,
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
            footer_hint: Some(tree_navigation_hint_line(
                &self.bottom_pane.list_keymap(),
                TreeNavigationHint {
                    accept_label: "selects",
                    include_move_right: false,
                    include_space_toggle: false,
                    cancel_label: "goes back",
                },
            )),
            items,
            initial_selected_idx: Some(selected),
            tree_navigation_enabled: true,
            on_cancel: Some(Box::new(move |tx| {
                tx.send(AppEvent::CancelRateLimitResetCreditSelection { generation });
            })),
            ..Default::default()
        });
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
}
