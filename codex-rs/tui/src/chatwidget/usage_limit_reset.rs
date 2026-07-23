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

const CHATGPT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api";
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
mod completion;
mod manual;
mod state;
impl ChatWidget {
    pub(crate) fn start_rate_limit_reset_consumption(
        &mut self,
        attempt: &RateLimitResetAttempt,
    ) -> bool {
        if !attempt.automatic && self.automatic_usage_limit_reset_owns_failed_turn() {
            return false;
        }
        let expected_attempt = if attempt.automatic {
            if attempt.retry_count == 0 {
                self.pending_rate_limit_reset_consumption.as_ref() == Some(attempt)
            } else {
                self.rate_limit_reset_retry.as_ref() == Some(attempt)
            }
        } else if attempt.retry_count == 0 {
            self.bottom_pane
                .has_view_id(RATE_LIMIT_RESET_CONFIRM_VIEW_ID)
                && self
                    .manual_rate_limit_reset_authority
                    .as_ref()
                    .is_some_and(|authority| {
                        authority.generation == attempt.generation
                            && authority.auth_profile == attempt.auth_profile
                            && authority.account_identity_fingerprint
                                == attempt.account_identity_fingerprint
                    })
                && self
                    .rate_limit_reset_credits
                    .as_ref()
                    .is_some_and(|summary| {
                        available_reset_credits(summary, Utc::now().timestamp())
                            .into_iter()
                            .any(|credit| credit.id == attempt.credit_id)
                    })
        } else {
            self.rate_limit_reset_retry.as_ref() == Some(attempt)
        };
        if attempt.generation != self.rate_limit_reset_generation
            || attempt.auth_profile != self.config.selected_auth_profile
            || self.rate_limit_reset_in_flight.is_some()
            || self.pending_post_reset_refresh.is_some()
            || attempt.credit_id.is_empty()
            || !expected_attempt
        {
            if !attempt.automatic {
                self.cancel_manual_rate_limit_reset_selection(attempt.generation);
            }
            return false;
        }
        let automatic_boundary_is_valid = !attempt.automatic
            || (attempt.trigger_key.as_deref().is_some_and(|trigger_key| {
                self.weekly_usage_limit_auto_reset_key().as_deref() == Some(trigger_key)
            }) && self
                .rate_limit_reset_credits
                .as_ref()
                .is_some_and(|summary| {
                    available_reset_credits(summary, Utc::now().timestamp())
                        .into_iter()
                        .any(|credit| credit.id == attempt.credit_id)
                }));
        let final_boundary_is_valid = self.uses_canonical_codex_backend_for_usage_reset()
            && !self.selected_auth_profile_credential_mutation_in_flight()
            && self
                .rate_limit_reset_account_identity_fingerprint
                .as_deref()
                == Some(attempt.account_identity_fingerprint.as_str())
            && automatic_boundary_is_valid;
        if !final_boundary_is_valid {
            if attempt.automatic {
                self.invalidate_pending_automatic_reset();
                self.fallback_auth_profile_switch_after_reset_unavailable();
            } else {
                self.cancel_manual_rate_limit_reset_selection(attempt.generation);
                self.add_error_message(
                    "Usage limit reset selection expired before it could be used.".to_string(),
                );
            }
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
        self.manual_rate_limit_reset_authority = None;
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

    pub(crate) fn cancel_manual_rate_limit_reset_selection(&mut self, generation: u64) {
        if generation != self.rate_limit_reset_generation {
            return;
        }
        if self
            .manual_rate_limit_reset_authority
            .as_ref()
            .is_some_and(|authority| authority.generation == generation)
        {
            self.manual_rate_limit_reset_authority = None;
        }
        if self
            .pending_rate_limit_reset_consumption
            .as_ref()
            .is_some_and(|attempt| !attempt.automatic && attempt.generation == generation)
        {
            self.pending_rate_limit_reset_consumption = None;
        }
        self.bottom_pane
            .dismiss_active_view_if_id(RATE_LIMIT_RESET_CONFIRM_VIEW_ID);
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
            window.window_duration_mins == Some(WEEKLY_WINDOW_MINUTES) && window.used_percent == 100
        })
}

pub(super) fn reset_label(count: i64) -> &'static str {
    if count == 1 {
        "usage limit reset"
    } else {
        "usage limit resets"
    }
}

/// Builds the menu description for one banked reset, folding in when the reset
/// expires so the picker shows each credit's remaining window instead of hiding
/// it. `now` is a Unix timestamp (seconds) so the relative countdown stays
/// deterministic and testable.
pub(super) fn reset_credit_description(credit: &RateLimitResetCredit, now: i64) -> Option<String> {
    let expiry = reset_credit_expiry_clause(credit.expires_at, now);
    match (credit.description.clone(), expiry) {
        (Some(description), Some(expiry)) => Some(format!("{description} {expiry}")),
        (Some(description), None) => Some(description),
        (None, expiry) => expiry,
    }
}

/// Human-readable expiry sentence for a banked reset, or `None` when the credit
/// carries no expiry. Matches the capitalized, period-terminated style of the
/// other picker descriptions (for example "Expires in 3d 4h.").
fn reset_credit_expiry_clause(expires_at: Option<i64>, now: i64) -> Option<String> {
    let expires_at = expires_at?;
    let remaining = expires_at.saturating_sub(now);
    if remaining <= 0 {
        return Some("Expired.".to_string());
    }
    Some(format!(
        "Expires in {}.",
        format_remaining_duration(remaining)
    ))
}

/// Renders a positive duration (seconds) as a compact "3d 4h" / "4h 5m" / "6m"
/// countdown, mirroring the day+hour granularity used elsewhere in the TUI.
fn format_remaining_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let minutes = seconds / 60;
    if minutes < 1 {
        return "under a minute".to_string();
    }
    let hours = minutes / 60;
    let days = hours / 24;
    if days >= 1 {
        let remaining_hours = hours % 24;
        return format!("{days}d {remaining_hours}h");
    }
    let remaining_minutes = minutes % 60;
    if hours >= 1 {
        return format!("{hours}h {remaining_minutes}m");
    }
    format!("{minutes}m")
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

    fn described_credit(
        expires_at: Option<i64>,
        description: Option<&str>,
    ) -> RateLimitResetCredit {
        let mut credit = reset_credit("credit", expires_at, RateLimitResetCreditStatus::Available);
        credit.description = description.map(str::to_string);
        credit
    }

    #[test]
    fn reset_credit_description_appends_relative_expiry() {
        let now = 1_000_000;
        let three_days_four_hours = now + 3 * 86_400 + 4 * 3_600 + 59 * 60;

        // Description present: expiry is folded in instead of being hidden.
        assert_eq!(
            reset_credit_description(
                &described_credit(Some(three_days_four_hours), Some("Earned reset credit.")),
                now,
            )
            .as_deref(),
            Some("Earned reset credit. Expires in 3d 4h."),
        );

        // No description: the expiry sentence stands alone.
        assert_eq!(
            reset_credit_description(
                &described_credit(Some(now + 5 * 3_600 + 12 * 60), None),
                now
            )
            .as_deref(),
            Some("Expires in 5h 12m."),
        );

        // Sub-hour windows fall back to minutes.
        assert_eq!(
            reset_credit_description(&described_credit(Some(now + 30 * 60), None), now).as_deref(),
            Some("Expires in 30m."),
        );
    }

    #[test]
    fn reset_credit_description_handles_expired_and_missing_expiry() {
        let now = 1_000_000;

        // Already expired.
        assert_eq!(
            reset_credit_description(&described_credit(Some(now - 1), Some("Earned.")), now)
                .as_deref(),
            Some("Earned. Expired."),
        );

        // No expiry, but a description: description is preserved unchanged.
        assert_eq!(
            reset_credit_description(&described_credit(None, Some("Earned reset credit.")), now)
                .as_deref(),
            Some("Earned reset credit."),
        );

        // No expiry and no description: nothing to show.
        assert_eq!(
            reset_credit_description(&described_credit(None, None), now),
            None,
        );

        // A far-future sentinel does not overflow the countdown math.
        assert!(
            reset_credit_description(&described_credit(Some(i64::MAX), None), now)
                .as_deref()
                .is_some_and(|text| text.starts_with("Expires in "))
        );
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
        snapshot.primary.as_mut().expect("primary").used_percent = 101;
        assert!(weekly_exhausted_window(&snapshot).is_none());
        snapshot.limit_id = Some("other".to_string());
        assert!(weekly_exhausted_window(&snapshot).is_none());
    }
}
