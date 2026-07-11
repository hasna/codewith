use super::*;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditResponse;
use codex_app_server_protocol::RateLimitResetCreditsSummary;
use codex_app_server_protocol::RateLimitSnapshot;
use codex_app_server_protocol::RateLimitWindow;

const WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;

impl ChatWidget {
    pub(crate) fn on_rate_limit_reset_credits(
        &mut self,
        reset_credits: Option<RateLimitResetCreditsSummary>,
    ) {
        let available_count = reset_credits
            .as_ref()
            .map(|credits| credits.available_count.max(0));
        self.rate_limit_reset_available_count = available_count;

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
            Some(_) => {
                self.announced_rate_limit_reset_available_count = None;
            }
            None => {}
        }

        self.refresh_usage_panel_if_active();
    }

    pub(crate) fn open_rate_limit_reset_confirm(&mut self) {
        let available_count = self.rate_limit_reset_available_count.unwrap_or(0);
        if available_count <= 0 {
            self.add_info_message(
                "No usage limit resets are available right now.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let auth_profile = Some(self.config.selected_auth_profile.clone());
        let use_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::ConsumeRateLimitResetCredit {
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                credit_id: None,
                auth_profile: auth_profile.clone(),
                automatic: false,
            });
        })];
        let items = vec![
            SelectionItem {
                name: "Use a reset".to_string(),
                description: Some(format!("Consume one available {}.", reset_label(1))),
                display_shortcut: Some(key_hint::plain(KeyCode::Char('y'))),
                actions: use_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Cancel".to_string(),
                display_shortcut: Some(key_hint::plain(KeyCode::Char('n'))),
                is_default: true,
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Usage limit resets".to_string()),
            subtitle: Some(format!(
                "You have {available_count} {} available.",
                reset_label(available_count)
            )),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx: Some(1),
            ..Default::default()
        });
    }

    pub(crate) fn start_rate_limit_reset_consumption(&mut self, automatic: bool) -> bool {
        if self.rate_limit_reset_in_flight {
            return false;
        }
        self.rate_limit_reset_in_flight = true;
        tracing::info!(automatic, "attempting usage-limit reset credit consumption");
        if automatic {
            self.add_info_message(
                "Weekly usage limit exhausted and usage limit auto-reset is enabled; \
                 attempting one available reset."
                    .to_string(),
                /*hint*/ None,
            );
        } else {
            self.add_info_message(
                "Attempting to use one usage limit reset.".to_string(),
                /*hint*/ None,
            );
        }
        true
    }

    pub(crate) fn finish_rate_limit_reset_consumption(
        &mut self,
        response: Result<ConsumeAccountRateLimitResetCreditResponse, String>,
        automatic: bool,
    ) -> bool {
        self.rate_limit_reset_in_flight = false;
        match response {
            Ok(response) => match response.outcome {
                ConsumeAccountRateLimitResetCreditOutcome::Reset
                | ConsumeAccountRateLimitResetCreditOutcome::AlreadyRedeemed => {
                    tracing::info!(
                        automatic,
                        outcome = ?response.outcome,
                        "usage-limit reset credit accepted"
                    );
                    self.rate_limit_reset_available_count = self
                        .rate_limit_reset_available_count
                        .map(|count| count.saturating_sub(1));
                    if let Some(trigger_key) = self.weekly_usage_limit_auto_reset_key() {
                        self.usage_limit_auto_reset_key = Some(trigger_key);
                    }
                    self.add_info_message(
                        "Usage limit reset accepted. Refreshing current limits.".to_string(),
                        /*hint*/ None,
                    );
                    self.refresh_usage_panel_if_active();
                    true
                }
                ConsumeAccountRateLimitResetCreditOutcome::NothingToReset => {
                    tracing::info!(
                        automatic,
                        "usage-limit reset skipped because no window needed a reset"
                    );
                    self.add_info_message(
                        "Your usage does not need a reset right now.".to_string(),
                        /*hint*/ None,
                    );
                    self.refresh_usage_panel_if_active();
                    false
                }
                ConsumeAccountRateLimitResetCreditOutcome::NoCredit => {
                    tracing::warn!(
                        automatic,
                        "usage-limit reset failed because no reset credits are available"
                    );
                    self.rate_limit_reset_available_count = Some(0);
                    self.announced_rate_limit_reset_available_count = None;
                    self.add_error_message("No usage limit resets are available.".to_string());
                    self.refresh_usage_panel_if_active();
                    false
                }
                ConsumeAccountRateLimitResetCreditOutcome::Unknown => {
                    tracing::warn!(automatic, "usage-limit reset returned an unknown outcome");
                    self.add_error_message(
                        "Usage reset returned an unknown result. Check /usage and try again."
                            .to_string(),
                    );
                    false
                }
            },
            Err(message) => {
                tracing::warn!(automatic, error = %message, "usage-limit reset request failed");
                self.add_error_message(format!("Couldn't reset usage: {message}"));
                false
            }
        }
    }

    pub(crate) fn maybe_start_usage_limit_auto_reset(
        &mut self,
        reset_credits: Option<RateLimitResetCreditsSummary>,
    ) {
        if !self.config.usage_limit.auto_reset_enabled || self.rate_limit_reset_in_flight {
            return;
        }
        if reset_credits
            .as_ref()
            .map(|credits| credits.available_count > 0)
            != Some(true)
        {
            return;
        }
        let Some(trigger_key) = self.weekly_usage_limit_auto_reset_key() else {
            return;
        };
        if self.usage_limit_auto_reset_key.as_deref() == Some(trigger_key.as_str()) {
            return;
        }

        self.usage_limit_auto_reset_key = Some(trigger_key);
        self.app_event_tx
            .send(AppEvent::ConsumeRateLimitResetCredit {
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                credit_id: None,
                auth_profile: Some(self.config.selected_auth_profile.clone()),
                automatic: true,
            });
    }

    fn weekly_usage_limit_auto_reset_key(&self) -> Option<String> {
        let (limit_id, window) = self
            .auth_profile_auto_switch_snapshots_by_limit_id
            .iter()
            .find_map(|(limit_id, snapshot)| {
                weekly_exhausted_window(snapshot).map(|window| (limit_id, window))
            })?;
        Some(format!(
            "{:?}:{limit_id}:weekly:{:?}",
            self.config.selected_auth_profile, window.resets_at
        ))
    }
}

fn weekly_exhausted_window(snapshot: &RateLimitSnapshot) -> Option<&RateLimitWindow> {
    [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
        .find(|window| {
            window.window_duration_mins == Some(WEEKLY_WINDOW_MINUTES)
                && window.used_percent >= 100
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

    #[test]
    fn weekly_exhausted_window_requires_weekly_duration_and_full_usage() {
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
    }
}
