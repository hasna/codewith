//! Rate-limit warning, prompt, and notice surfaces for `ChatWidget`.

use super::*;
use crate::chatwidget::user_messages::QueueInsertionPosition;
use codex_app_server_protocol::CodexErrorInfo as AppServerCodexErrorInfo;
use codex_app_server_protocol::RateLimitWindow;
use codex_login::AuthProfile;
use codex_login::list_auth_profiles;

pub(super) const NUDGE_MODEL_SLUG: &str = "gpt-5.4-mini";
pub(super) const RATE_LIMIT_SWITCH_PROMPT_THRESHOLD: f64 = 90.0;

const RATE_LIMIT_WARNING_THRESHOLDS: [f64; 3] = [75.0, 90.0, 95.0];
const PRIMARY_LIMIT_FALLBACK_LABEL: &str = "usage";
const SECONDARY_LIMIT_FALLBACK_LABEL: &str = "secondary usage";
const FIVE_HOUR_LIMIT_LABEL: &str = "5h";
const WEEKLY_LIMIT_LABEL: &str = "weekly";

#[derive(Default)]
pub(super) struct RateLimitWarningState {
    pub(super) secondary_index: usize,
    pub(super) primary_index: usize,
}

impl RateLimitWarningState {
    pub(super) fn take_warnings(
        &mut self,
        secondary_used_percent: Option<f64>,
        secondary_window_minutes: Option<i64>,
        primary_used_percent: Option<f64>,
        primary_window_minutes: Option<i64>,
    ) -> Vec<String> {
        let reached_secondary_cap =
            matches!(secondary_used_percent, Some(percent) if percent == 100.0);
        let reached_primary_cap = matches!(primary_used_percent, Some(percent) if percent == 100.0);
        if reached_secondary_cap || reached_primary_cap {
            return Vec::new();
        }

        let mut warnings = Vec::new();

        if let Some(secondary_used_percent) = secondary_used_percent {
            let mut highest_secondary: Option<f64> = None;
            while self.secondary_index < RATE_LIMIT_WARNING_THRESHOLDS.len()
                && secondary_used_percent >= RATE_LIMIT_WARNING_THRESHOLDS[self.secondary_index]
            {
                highest_secondary = Some(RATE_LIMIT_WARNING_THRESHOLDS[self.secondary_index]);
                self.secondary_index += 1;
            }
            if let Some(threshold) = highest_secondary {
                let limit_label =
                    limit_label_for_window(secondary_window_minutes, /*is_secondary*/ true);
                let remaining_percent = 100.0 - threshold;
                warnings.push(format!(
                    "Heads up, you have less than {remaining_percent:.0}% of your {limit_label} limit left. Run /status for a breakdown."
                ));
            }
        }

        if let Some(primary_used_percent) = primary_used_percent {
            let mut highest_primary: Option<f64> = None;
            while self.primary_index < RATE_LIMIT_WARNING_THRESHOLDS.len()
                && primary_used_percent >= RATE_LIMIT_WARNING_THRESHOLDS[self.primary_index]
            {
                highest_primary = Some(RATE_LIMIT_WARNING_THRESHOLDS[self.primary_index]);
                self.primary_index += 1;
            }
            if let Some(threshold) = highest_primary {
                let limit_label =
                    limit_label_for_window(primary_window_minutes, /*is_secondary*/ false);
                let remaining_percent = 100.0 - threshold;
                warnings.push(format!(
                    "Heads up, you have less than {remaining_percent:.0}% of your {limit_label} limit left. Run /status for a breakdown."
                ));
            }
        }

        warnings
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AuthProfileAutoSwitchWindow {
    label: String,
    resets_at: Option<i64>,
}

fn exhausted_auto_switch_window(
    snapshot: &RateLimitSnapshot,
    config: &crate::legacy_core::config::AuthProfileAutoSwitchConfig,
    is_codex_limit: bool,
) -> Option<AuthProfileAutoSwitchWindow> {
    if !config.enabled {
        return None;
    }

    if !is_codex_limit {
        return None;
    }

    [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(exhausted_auto_switch_window_for_limit)
        .find(|window| auth_profile_auto_switch_window_enabled(window, config))
}

fn exhausted_auto_switch_window_for_snapshot(
    snapshot: &RateLimitSnapshot,
    is_codex_limit: bool,
) -> Option<AuthProfileAutoSwitchWindow> {
    if !is_codex_limit {
        return None;
    }

    [snapshot.secondary.as_ref(), snapshot.primary.as_ref()]
        .into_iter()
        .flatten()
        .find_map(exhausted_auto_switch_window_for_limit)
}

fn exhausted_auto_switch_window_for_limit(
    window: &RateLimitWindow,
) -> Option<AuthProfileAutoSwitchWindow> {
    if window.used_percent < 100 {
        return None;
    }
    let label = get_limits_duration(window.window_duration_mins?)?;
    matches!(label.as_str(), FIVE_HOUR_LIMIT_LABEL | WEEKLY_LIMIT_LABEL).then_some(
        AuthProfileAutoSwitchWindow {
            label,
            resets_at: window.resets_at,
        },
    )
}

fn auth_profile_auto_switch_window_enabled(
    window: &AuthProfileAutoSwitchWindow,
    config: &crate::legacy_core::config::AuthProfileAutoSwitchConfig,
) -> bool {
    match window.label.as_str() {
        FIVE_HOUR_LIMIT_LABEL => config.on_5h_limit,
        WEEKLY_LIMIT_LABEL => config.on_weekly_limit,
        _ => false,
    }
}

fn auto_switch_trigger_key(limit_id: &str, window: &AuthProfileAutoSwitchWindow) -> String {
    let resets_at = window
        .resets_at
        .map(|reset| reset.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!("{limit_id}:{}:{resets_at}", window.label)
}

fn next_auth_profile_for_auto_switch(
    current: Option<&str>,
    configured_profiles: &[String],
    saved_profiles: &[AuthProfile],
) -> Option<String> {
    let saved_names = saved_profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<HashSet<_>>();
    let ordered = if configured_profiles.is_empty() {
        saved_profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect::<Vec<_>>()
    } else {
        configured_profiles
            .iter()
            .filter(|profile| saved_names.contains(profile.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    let ordered = dedupe_profile_names(ordered);
    if ordered.is_empty() {
        return None;
    }

    let start = current
        .and_then(|current| ordered.iter().position(|profile| profile == current))
        .map(|index| index + 1)
        .unwrap_or(0);
    ordered
        .iter()
        .cycle()
        .skip(start)
        .take(ordered.len())
        .find(|profile| current != Some(profile.as_str()))
        .cloned()
}

fn dedupe_profile_names(profiles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .filter(|profile| seen.insert(profile.clone()))
        .collect()
}

pub(crate) fn limit_label_for_window(window_minutes: Option<i64>, is_secondary: bool) -> String {
    window_minutes
        .and_then(get_limits_duration)
        .unwrap_or_else(|| fallback_limit_label(is_secondary).to_string())
}

pub(crate) fn get_limits_duration(windows_minutes: i64) -> Option<String> {
    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_5_HOURS: i64 = 5 * MINUTES_PER_HOUR;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
    const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;
    const MINUTES_PER_YEAR: i64 = 365 * MINUTES_PER_DAY;

    let windows_minutes = windows_minutes.max(0);

    if is_approximate_window(windows_minutes, MINUTES_PER_5_HOURS) {
        Some("5h".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_DAY) {
        Some("daily".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_WEEK) {
        Some("weekly".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_MONTH) {
        Some("monthly".to_string())
    } else if is_approximate_window(windows_minutes, MINUTES_PER_YEAR) {
        Some("annual".to_string())
    } else {
        None
    }
}

pub(crate) fn fallback_limit_label(is_secondary: bool) -> &'static str {
    if is_secondary {
        SECONDARY_LIMIT_FALLBACK_LABEL
    } else {
        PRIMARY_LIMIT_FALLBACK_LABEL
    }
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes as f64;
    let expected_minutes = expected_minutes as f64;
    minutes >= expected_minutes * 0.95 && minutes <= expected_minutes * 1.05
}

#[derive(Default)]
pub(super) enum RateLimitSwitchPromptState {
    #[default]
    Idle,
    Pending,
    Shown,
}

#[derive(Debug)]
pub(super) enum RateLimitErrorKind {
    ServerOverloaded,
    UsageLimit,
    Generic,
}

pub(super) fn app_server_rate_limit_error_kind(
    info: &AppServerCodexErrorInfo,
) -> Option<RateLimitErrorKind> {
    match info {
        AppServerCodexErrorInfo::ServerOverloaded => Some(RateLimitErrorKind::ServerOverloaded),
        AppServerCodexErrorInfo::UsageLimitExceeded => Some(RateLimitErrorKind::UsageLimit),
        AppServerCodexErrorInfo::ResponseTooManyFailedAttempts {
            http_status_code: Some(429),
        } => Some(RateLimitErrorKind::Generic),
        _ => None,
    }
}

pub(super) fn is_app_server_cyber_policy_error(info: &AppServerCodexErrorInfo) -> bool {
    matches!(info, AppServerCodexErrorInfo::CyberPolicy)
}

impl ChatWidget {
    pub(crate) fn on_rate_limit_snapshot(&mut self, snapshot: Option<RateLimitSnapshot>) {
        if let Some(mut snapshot) = snapshot {
            let limit_id = snapshot
                .limit_id
                .clone()
                .unwrap_or_else(|| "codex".to_string());
            let limit_label = snapshot
                .limit_name
                .clone()
                .unwrap_or_else(|| limit_id.clone());
            if snapshot.credits.is_none() {
                snapshot.credits = self
                    .rate_limit_snapshots_by_limit_id
                    .get(&limit_id)
                    .and_then(|display| display.credits.as_ref())
                    .map(|credits| CreditsSnapshot {
                        has_credits: credits.has_credits,
                        unlimited: credits.unlimited,
                        balance: credits.balance.clone(),
                    });
            }

            self.plan_type = snapshot.plan_type.or(self.plan_type);

            let is_codex_limit = limit_id.eq_ignore_ascii_case("codex");
            if is_codex_limit
                && let Some(rate_limit_reached_type) = snapshot.rate_limit_reached_type
            {
                self.codex_rate_limit_reached_type = Some(rate_limit_reached_type);
            }
            let warnings = if is_codex_limit {
                self.rate_limit_warnings.take_warnings(
                    snapshot
                        .secondary
                        .as_ref()
                        .map(|window| f64::from(window.used_percent)),
                    snapshot
                        .secondary
                        .as_ref()
                        .and_then(|window| window.window_duration_mins),
                    snapshot
                        .primary
                        .as_ref()
                        .map(|window| f64::from(window.used_percent)),
                    snapshot
                        .primary
                        .as_ref()
                        .and_then(|window| window.window_duration_mins),
                )
            } else {
                vec![]
            };

            let high_usage = is_codex_limit
                && (snapshot
                    .secondary
                    .as_ref()
                    .map(|w| f64::from(w.used_percent) >= RATE_LIMIT_SWITCH_PROMPT_THRESHOLD)
                    .unwrap_or(false)
                    || snapshot
                        .primary
                        .as_ref()
                        .map(|w| f64::from(w.used_percent) >= RATE_LIMIT_SWITCH_PROMPT_THRESHOLD)
                        .unwrap_or(false));

            let has_workspace_credits = snapshot
                .credits
                .as_ref()
                .map(|credits| credits.has_credits)
                .unwrap_or(false);

            if high_usage
                && !has_workspace_credits
                && !self.rate_limit_switch_prompt_hidden()
                && self.current_model() != NUDGE_MODEL_SLUG
                && !matches!(
                    self.rate_limit_switch_prompt,
                    RateLimitSwitchPromptState::Shown
                )
            {
                self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Pending;
            }

            self.update_auth_profile_auto_switch_snapshot(&limit_id, &snapshot, is_codex_limit);
            self.maybe_auto_switch_auth_profile_for_rate_limit(&limit_id, &snapshot);

            let display =
                rate_limit_snapshot_display_for_limit(&snapshot, limit_label, Local::now());
            self.rate_limit_snapshots_by_limit_id
                .insert(limit_id, display);

            if !warnings.is_empty() {
                for warning in warnings {
                    self.add_to_history(history_cell::new_warning_event(warning));
                }
                self.request_redraw();
            }
        } else {
            self.rate_limit_snapshots_by_limit_id.clear();
            self.auth_profile_auto_switch_snapshots_by_limit_id.clear();
            self.codex_rate_limit_reached_type = None;
        }
        self.refresh_status_line();
    }

    fn update_auth_profile_auto_switch_snapshot(
        &mut self,
        limit_id: &str,
        snapshot: &RateLimitSnapshot,
        is_codex_limit: bool,
    ) {
        if exhausted_auto_switch_window_for_snapshot(snapshot, is_codex_limit).is_some() {
            self.auth_profile_auto_switch_snapshots_by_limit_id
                .insert(limit_id.to_string(), snapshot.clone());
        } else {
            self.auth_profile_auto_switch_snapshots_by_limit_id
                .remove(limit_id);
        }
    }

    fn maybe_auto_switch_auth_profile_for_rate_limit(
        &mut self,
        limit_id: &str,
        snapshot: &RateLimitSnapshot,
    ) {
        let Some(window) = exhausted_auto_switch_window(
            snapshot,
            &self.config.auth_profile_auto_switch,
            limit_id.eq_ignore_ascii_case("codex"),
        ) else {
            return;
        };
        if !self.is_session_configured() {
            return;
        }
        let Some((next_profile, trigger_key)) =
            self.auth_profile_auto_switch_target(limit_id, &window)
        else {
            return;
        };
        self.send_auth_profile_auto_switch(next_profile, trigger_key, window);
    }

    pub(super) fn maybe_auto_switch_auth_profile_before_user_turn(
        &mut self,
        user_message: &UserMessage,
        history_record: &UserMessageHistoryRecord,
        queue_position: QueueInsertionPosition,
    ) -> bool {
        let Some((limit_id, window)) = self
            .auth_profile_auto_switch_snapshots_by_limit_id
            .iter()
            .find_map(|(limit_id, snapshot)| {
                exhausted_auto_switch_window(
                    snapshot,
                    &self.config.auth_profile_auto_switch,
                    limit_id.eq_ignore_ascii_case("codex"),
                )
                .map(|window| (limit_id.clone(), window))
            })
        else {
            return false;
        };
        let trigger_key = auto_switch_trigger_key(&limit_id, &window);
        if self.pending_auth_profile_auto_switch_trigger.as_deref() == Some(trigger_key.as_str()) {
            self.queue_user_message_for_auth_profile_auto_switch(
                user_message,
                history_record,
                queue_position,
            );
            return true;
        }
        let Some((next_profile, trigger_key)) =
            self.auth_profile_auto_switch_target(&limit_id, &window)
        else {
            return false;
        };
        self.queue_user_message_for_auth_profile_auto_switch(
            user_message,
            history_record,
            queue_position,
        );
        self.send_auth_profile_auto_switch(next_profile, trigger_key, window);
        true
    }

    fn queue_user_message_for_auth_profile_auto_switch(
        &mut self,
        user_message: &UserMessage,
        history_record: &UserMessageHistoryRecord,
        queue_position: QueueInsertionPosition,
    ) {
        match queue_position {
            QueueInsertionPosition::Front => {
                self.input_queue
                    .queued_user_messages
                    .push_front(QueuedUserMessage::from(user_message.clone()));
                self.input_queue
                    .queued_user_message_history_records
                    .push_front(history_record.clone());
            }
            QueueInsertionPosition::Back => {
                self.input_queue
                    .queued_user_messages
                    .push_back(QueuedUserMessage::from(user_message.clone()));
                self.input_queue
                    .queued_user_message_history_records
                    .push_back(history_record.clone());
            }
        }
        self.refresh_pending_input_preview();
    }

    fn auth_profile_auto_switch_target(
        &mut self,
        limit_id: &str,
        window: &AuthProfileAutoSwitchWindow,
    ) -> Option<(String, String)> {
        let trigger_key = auto_switch_trigger_key(limit_id, window);
        if self.last_auth_profile_auto_switch_trigger.as_deref() == Some(trigger_key.as_str()) {
            return None;
        }

        let profiles = match list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => profiles,
            Err(err) => {
                self.add_error_message(format!("Failed to load auth profiles: {err}"));
                return None;
            }
        };
        let next_profile = next_auth_profile_for_auto_switch(
            self.config.selected_auth_profile.as_deref(),
            &self.config.auth_profile_auto_switch.profiles,
            &profiles,
        );
        if let Some(next_profile) = next_profile {
            return Some((next_profile, trigger_key));
        }

        self.add_info_message(
            "Auth profile auto-switch is enabled, but no alternate profile is available."
                .to_string(),
            /*hint*/ None,
        );
        None
    }

    fn send_auth_profile_auto_switch(
        &mut self,
        next_profile: String,
        trigger_key: String,
        window: AuthProfileAutoSwitchWindow,
    ) {
        self.pending_auth_profile_auto_switch_trigger = Some(trigger_key.clone());
        self.last_auth_profile_auto_switch_trigger = Some(trigger_key);
        self.app_event_tx.send(AppEvent::SwitchAuthProfile {
            profile: Some(next_profile),
            reason: crate::app_event::AuthProfileSwitchReason::AutoRateLimit {
                window: window.label,
            },
            resume_queued_input: true,
        });
    }

    pub(super) fn stop_rate_limit_poller(&mut self) {}

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn prefetch_rate_limits(&mut self) {
        self.stop_rate_limit_poller();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn should_prefetch_rate_limits(&self) -> bool {
        self.config.model_provider.requires_openai_auth && self.has_chatgpt_account
    }

    fn lower_cost_preset(&self) -> Option<ModelPreset> {
        let models = self.model_catalog.try_list_models().ok()?;
        models
            .iter()
            .find(|preset| preset.show_in_picker && preset.model == NUDGE_MODEL_SLUG)
            .cloned()
    }

    fn rate_limit_switch_prompt_hidden(&self) -> bool {
        self.config
            .notices
            .hide_rate_limit_model_nudge
            .unwrap_or(false)
    }

    pub(super) fn maybe_show_pending_rate_limit_prompt(&mut self) {
        if self.rate_limit_switch_prompt_hidden() {
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Idle;
            return;
        }
        if !matches!(
            self.rate_limit_switch_prompt,
            RateLimitSwitchPromptState::Pending
        ) {
            return;
        }
        if let Some(preset) = self.lower_cost_preset() {
            self.open_rate_limit_switch_prompt(preset);
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Shown;
        } else {
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Idle;
        }
    }

    fn open_rate_limit_switch_prompt(&mut self, preset: ModelPreset) {
        let switch_model = preset.model;
        let switch_model_for_events = switch_model.clone();
        let default_effort: ReasoningEffortConfig = preset.default_reasoning_effort;

        let switch_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::CodexOp(AppCommand::override_turn_context(
                /*cwd*/ None,
                /*approval_policy*/ None,
                /*approvals_reviewer*/ None,
                /*permission_profile*/ None,
                /*active_permission_profile*/ None,
                /*windows_sandbox_level*/ None,
                Some(switch_model_for_events.clone()),
                Some(Some(default_effort)),
                /*summary*/ None,
                /*service_tier*/ None,
                /*collaboration_mode*/ None,
                /*personality*/ None,
            )));
            tx.send(AppEvent::UpdateModel(switch_model_for_events.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(Some(default_effort)));
        })];

        let keep_actions: Vec<SelectionAction> = Vec::new();
        let never_actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::UpdateRateLimitSwitchPromptHidden(true));
            tx.send(AppEvent::PersistRateLimitSwitchPromptHidden);
        })];
        let description = if preset.description.is_empty() {
            Some("Uses fewer credits for upcoming turns.".to_string())
        } else {
            Some(preset.description)
        };

        let items = vec![
            SelectionItem {
                name: format!("Switch to {switch_model}"),
                description,
                selected_description: None,
                is_current: false,
                actions: switch_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Keep current model".to_string(),
                description: None,
                selected_description: None,
                is_current: false,
                actions: keep_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Keep current model (never show again)".to_string(),
                description: Some(
                    "Hide future rate limit reminders about switching models.".to_string(),
                ),
                selected_description: None,
                is_current: false,
                actions: never_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Approaching rate limits".to_string()),
            subtitle: Some(format!("Switch to {switch_model} for lower credit usage?")),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(super) fn open_workspace_owner_nudge_prompt(
        &mut self,
        credit_type: AddCreditsNudgeCreditType,
    ) {
        if self.add_credits_nudge_email_in_flight.is_some() {
            return;
        }

        let (title, prompt) = match credit_type {
            AddCreditsNudgeCreditType::Credits => (
                "You've reached your workspace credit limit",
                "Your workspace is out of credits. Ask your workspace owner to add more. Notify owner?",
            ),
            AddCreditsNudgeCreditType::UsageLimit => (
                "Usage limit reached",
                "Request a limit increase from your owner to continue using Codewith. Request increase?",
            ),
        };
        let send_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SendAddCreditsNudgeEmail { credit_type });
        })];
        let items = vec![
            SelectionItem {
                name: "Yes".to_string(),
                display_shortcut: Some(key_hint::plain(KeyCode::Char('y'))),
                actions: send_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "No".to_string(),
                display_shortcut: Some(key_hint::plain(KeyCode::Char('n'))),
                is_default: true,
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(title.to_string()),
            subtitle: Some(prompt.to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx: Some(1),
            ..Default::default()
        });
    }

    pub(crate) fn start_add_credits_nudge_email_request(
        &mut self,
        credit_type: AddCreditsNudgeCreditType,
    ) -> bool {
        self.add_credits_nudge_email_in_flight = Some(credit_type);
        true
    }

    pub(crate) fn finish_add_credits_nudge_email_request(
        &mut self,
        result: Result<AddCreditsNudgeEmailStatus, String>,
    ) {
        let credit_type = self
            .add_credits_nudge_email_in_flight
            .take()
            .unwrap_or(AddCreditsNudgeCreditType::Credits);
        let message = match (credit_type, result) {
            (AddCreditsNudgeCreditType::Credits, Ok(AddCreditsNudgeEmailStatus::Sent)) => {
                "Workspace owner notified."
            }
            (
                AddCreditsNudgeCreditType::Credits,
                Ok(AddCreditsNudgeEmailStatus::CooldownActive),
            ) => "Workspace owner was already notified recently.",
            (AddCreditsNudgeCreditType::Credits, Err(_)) => {
                "Could not notify your workspace owner. Please try again."
            }
            (AddCreditsNudgeCreditType::UsageLimit, Ok(AddCreditsNudgeEmailStatus::Sent)) => {
                "Limit increase requested."
            }
            (
                AddCreditsNudgeCreditType::UsageLimit,
                Ok(AddCreditsNudgeEmailStatus::CooldownActive),
            ) => "A limit increase was already requested recently.",
            (AddCreditsNudgeCreditType::UsageLimit, Err(_)) => {
                "Could not request a limit increase. Please try again."
            }
        };
        self.add_to_history(history_cell::new_info_event(
            message.to_string(),
            /*hint*/ None,
        ));
        self.request_redraw();
    }

    pub(crate) fn set_rate_limit_switch_prompt_hidden(&mut self, hidden: bool) {
        self.config.notices.hide_rate_limit_model_nudge = Some(hidden);
        if hidden {
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Idle;
        }
    }
}
