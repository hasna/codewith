//! Rate-limit warning, prompt, and notice surfaces for `ChatWidget`.

use super::*;
use crate::chatwidget::auth_profile_popups::AUTH_PROFILE_USAGE_HEARTBEAT_FAILURE_BACKOFF;
use crate::chatwidget::usage_profile_broker::AutoSwitchTrigger;
use crate::chatwidget::usage_profile_broker::UsageProfileAutoSwitchWindow;
use crate::chatwidget::usage_profile_broker::auth_profile_auto_switch_target as broker_auth_profile_auto_switch_target;
use crate::chatwidget::usage_profile_broker::auto_switch_trigger_key;
use crate::chatwidget::usage_profile_broker::earliest_exhausted_reset_at;
use crate::chatwidget::usage_profile_broker::exhausted_auto_switch_window;
use crate::chatwidget::usage_profile_broker::exhausted_auto_switch_window_for_snapshot;
use crate::chatwidget::usage_profile_broker::fallback_limit_label as broker_fallback_limit_label;
use crate::chatwidget::usage_profile_broker::get_limits_duration as broker_get_limits_duration;
use crate::chatwidget::usage_profile_broker::limit_label_for_window as broker_limit_label_for_window;
use crate::chatwidget::usage_self_heal::parse_usage_limit_reset_timestamp;
use crate::chatwidget::user_messages::QueueInsertionPosition;
use crate::legacy_core::usage_profile_health::FIVE_HOUR_LIMIT_LABEL;
use crate::legacy_core::usage_profile_health::UsageProfileCooldownKey;
use crate::legacy_core::usage_profile_health::WEEKLY_LIMIT_LABEL;
use crate::legacy_core::usage_profile_health::auth_profile_auto_switch_label_enabled;
use crate::legacy_core::usage_profile_health::cooldown_duration_for_reset;
use codex_app_server_protocol::CodexErrorInfo as AppServerCodexErrorInfo;
use codex_login::list_auth_profiles;
use tokio::time::MissedTickBehavior;

pub(super) const NUDGE_MODEL_SLUG: &str = "gpt-5.4-mini";
pub(super) const RATE_LIMIT_SWITCH_PROMPT_THRESHOLD: f64 = 90.0;

const RATE_LIMIT_WARNING_THRESHOLDS: [f64; 3] = [75.0, 90.0, 95.0];

/// Longest reset horizon still attributable to the rolling 5-hour window. The server
/// advertises the reset of the window that actually blocked the request, and a weekly cap
/// always resets days out, so anything inside this horizon is the 5h window. The slack
/// absorbs server-side rounding and clock skew.
const FIVE_HOUR_RESET_HORIZON_SECS: i64 = 5 * 60 * 60 + 30 * 60;

/// Classify the window that blocked a turn from the reset instant the server advertised.
fn usage_limit_window_label_for_reset(resets_at: i64, now_unix_secs: i64) -> &'static str {
    if resets_at.saturating_sub(now_unix_secs) <= FIVE_HOUR_RESET_HORIZON_SECS {
        FIVE_HOUR_LIMIT_LABEL
    } else {
        WEEKLY_LIMIT_LABEL
    }
}

#[derive(Default)]
pub(super) struct RateLimitWarningState {
    pub(super) secondary_index: usize,
    pub(super) primary_index: usize,
}

pub(super) struct RateLimitWarningContext<'a> {
    pub(super) observed_at: &'a chrono::DateTime<Local>,
    pub(super) profile: Option<&'a str>,
}

impl RateLimitWarningState {
    pub(super) fn take_warnings(
        &mut self,
        secondary_used_percent: Option<f64>,
        secondary_window_minutes: Option<i64>,
        primary_used_percent: Option<f64>,
        primary_window_minutes: Option<i64>,
        context: &RateLimitWarningContext<'_>,
    ) -> Vec<String> {
        let reached_secondary_cap =
            matches!(secondary_used_percent, Some(percent) if percent == 100.0);
        let reached_primary_cap = matches!(primary_used_percent, Some(percent) if percent == 100.0);
        if reached_secondary_cap || reached_primary_cap {
            return Vec::new();
        }

        let mut warnings = Vec::new();

        if let Some(secondary_used_percent) = secondary_used_percent {
            let mut crossed_threshold = false;
            while self.secondary_index < RATE_LIMIT_WARNING_THRESHOLDS.len()
                && secondary_used_percent >= RATE_LIMIT_WARNING_THRESHOLDS[self.secondary_index]
            {
                crossed_threshold = true;
                self.secondary_index += 1;
            }
            if crossed_threshold {
                let limit_label =
                    limit_label_for_window(secondary_window_minutes, /*is_secondary*/ true);
                warnings.push(rate_limit_warning_message(
                    secondary_used_percent,
                    &limit_label,
                    context,
                ));
            }
        }

        if let Some(primary_used_percent) = primary_used_percent {
            let mut crossed_threshold = false;
            while self.primary_index < RATE_LIMIT_WARNING_THRESHOLDS.len()
                && primary_used_percent >= RATE_LIMIT_WARNING_THRESHOLDS[self.primary_index]
            {
                crossed_threshold = true;
                self.primary_index += 1;
            }
            if crossed_threshold {
                let limit_label =
                    limit_label_for_window(primary_window_minutes, /*is_secondary*/ false);
                warnings.push(rate_limit_warning_message(
                    primary_used_percent,
                    &limit_label,
                    context,
                ));
            }
        }

        warnings
    }
}

fn rate_limit_warning_message(
    used_percent: f64,
    limit_label: &str,
    context: &RateLimitWarningContext<'_>,
) -> String {
    let observed_at = context.observed_at.format("%-I:%M %p %b %-d");
    let profile = context.profile.unwrap_or("default");
    let remaining_percent = (100.0 - used_percent).clamp(0.0, 100.0);
    format!(
        "As of {observed_at}, profile {profile} has {remaining_percent:.0}% of its {limit_label} limit remaining."
    )
}

pub(crate) fn limit_label_for_window(window_minutes: Option<i64>, is_secondary: bool) -> String {
    broker_limit_label_for_window(window_minutes, is_secondary)
}

pub(crate) fn get_limits_duration(windows_minutes: i64) -> Option<String> {
    broker_get_limits_duration(windows_minutes)
}

pub(crate) fn fallback_limit_label(is_secondary: bool) -> &'static str {
    broker_fallback_limit_label(is_secondary)
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

#[derive(Clone, Copy)]
pub(super) enum RateLimitSnapshotSource {
    AccountUsage,
    RollingUpdate,
}

impl ChatWidget {
    pub(crate) fn on_rate_limit_snapshot(&mut self, snapshot: Option<RateLimitSnapshot>) {
        self.on_rate_limit_snapshot_from(
            snapshot,
            RateLimitSnapshotSource::AccountUsage,
            Local::now(),
        );
    }

    pub(crate) fn on_auth_profile_rate_limit_snapshots(
        &mut self,
        profile: Option<String>,
        snapshots: Vec<RateLimitSnapshot>,
    ) {
        let captured_at = Local::now();
        let mut displays = BTreeMap::new();
        for snapshot in &snapshots {
            let limit_id = snapshot
                .limit_id
                .clone()
                .unwrap_or_else(|| "codex".to_string());
            let limit_label = snapshot
                .limit_name
                .clone()
                .unwrap_or_else(|| limit_id.clone());
            displays.insert(
                limit_id,
                rate_limit_snapshot_display_for_limit(snapshot, limit_label, captured_at),
            );
        }

        self.update_auth_profile_usage_exhaustion(&profile, &snapshots);

        if displays.is_empty() {
            self.auth_profile_rate_limit_snapshots_by_profile
                .remove(&profile);
        } else {
            self.auth_profile_rate_limit_snapshots_by_profile
                .insert(profile, displays);
        }
    }

    /// Record whether `profile`'s codex usage is exhausted with a future reset so we can
    /// suppress usage heartbeats for it until it resets (see
    /// `should_request_auth_profile_usage_heartbeat`).
    fn update_auth_profile_usage_exhaustion(
        &mut self,
        profile: &Option<String>,
        snapshots: &[RateLimitSnapshot],
    ) {
        let now = chrono::Utc::now().timestamp();
        let reset_at = snapshots
            .iter()
            .filter_map(|snapshot| {
                let is_codex_limit = snapshot
                    .limit_id
                    .as_deref()
                    .is_none_or(|limit_id| limit_id.eq_ignore_ascii_case("codex"));
                earliest_exhausted_reset_at(snapshot, is_codex_limit, now)
            })
            .min();
        match reset_at {
            Some(reset_at) => {
                self.auth_profile_usage_exhausted_reset_at_by_profile
                    .insert(profile.clone(), reset_at);
            }
            None => {
                self.auth_profile_usage_exhausted_reset_at_by_profile
                    .remove(profile);
            }
        }
    }

    pub(crate) fn on_rolling_rate_limit_snapshot(&mut self, snapshot: RateLimitSnapshot) {
        // Rolling app-server notifications are sparse. Preserve metadata learned from the full read.
        self.on_rate_limit_snapshot_from(
            Some(snapshot),
            RateLimitSnapshotSource::RollingUpdate,
            Local::now(),
        );
    }

    pub(crate) fn begin_authoritative_selected_rate_limit_refresh(&mut self) {
        self.auth_profile_auto_switch_snapshots_by_limit_id.clear();
        self.codex_rate_limit_reached_type = None;
    }

    pub(super) fn on_rate_limit_snapshot_from(
        &mut self,
        snapshot: Option<RateLimitSnapshot>,
        source: RateLimitSnapshotSource,
        observed_at: chrono::DateTime<Local>,
    ) {
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
            let preserved_individual_limit =
                if matches!(source, RateLimitSnapshotSource::RollingUpdate)
                    && snapshot.individual_limit.is_none()
                {
                    self.rate_limit_snapshots_by_limit_id
                        .get(&limit_id)
                        .and_then(|display| display.individual_limit.clone())
                } else {
                    None
                };
            self.plan_type = snapshot.plan_type.or(self.plan_type);

            let is_codex_limit = limit_id.eq_ignore_ascii_case("codex");
            if is_codex_limit
                && let Some(rate_limit_reached_type) = snapshot.rate_limit_reached_type
            {
                self.codex_rate_limit_reached_type = Some(rate_limit_reached_type);
            }
            // Rolling notifications do not identify their profile, so only an authoritative
            // account-usage read can emit or consume profile-attributed warning thresholds.
            let warnings =
                if is_codex_limit && matches!(source, RateLimitSnapshotSource::AccountUsage) {
                    let selected_auth_profile = self.config.selected_auth_profile.clone();
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
                        &RateLimitWarningContext {
                            observed_at: &observed_at,
                            profile: selected_auth_profile.as_deref(),
                        },
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

            // Exhaustion bookkeeping and auth-profile auto-switching must only be driven by
            // authoritative, account-verified rate-limit reads. `event_dispatch` confirms an
            // `AccountUsage` snapshot belongs to the currently-selected account (via
            // `is_current_profile` + `account_identity_fingerprint`) before applying it.
            //
            // Rolling `account/rateLimits/updated` notifications (`RollingUpdate`) carry no
            // account identity (see `AccountRateLimitsUpdatedNotification`) and are emitted per
            // turn. Under multi-agent spawning a sibling turn running on a *different* (exhausted)
            // account emits a "100%" snapshot that would otherwise be misattributed to the current
            // profile — triggering a false-positive auto-switch and suppressing the corrective
            // usage heartbeat. Cascaded across profiles this rotates through every configured
            // account even though only one is genuinely exhausted, so rolling updates stay
            // display-only here.
            if matches!(source, RateLimitSnapshotSource::AccountUsage) {
                self.update_auth_profile_auto_switch_snapshot(&limit_id, &snapshot, is_codex_limit);
                if is_codex_limit {
                    let selected_profile = self.config.selected_auth_profile.clone();
                    self.update_auth_profile_usage_exhaustion(
                        &selected_profile,
                        std::slice::from_ref(&snapshot),
                    );
                }
                if !self.usage_limit_reset_takes_precedence_for_snapshot(&snapshot) {
                    self.maybe_auto_switch_auth_profile_for_rate_limit(&limit_id, &snapshot);
                }
            }

            let mut display =
                rate_limit_snapshot_display_for_limit(&snapshot, limit_label, observed_at);
            if display.individual_limit.is_none() {
                display.individual_limit = preserved_individual_limit;
            }
            self.rate_limit_snapshots_by_limit_id
                .insert(limit_id, display);
            self.auth_profile_rate_limit_snapshots_by_profile.insert(
                self.config.selected_auth_profile.clone(),
                self.rate_limit_snapshots_by_limit_id.clone(),
            );

            if !warnings.is_empty() {
                for warning in warnings {
                    self.add_to_history(history_cell::new_warning_event(warning));
                }
                self.request_redraw();
            }
        } else {
            self.rate_limit_snapshots_by_limit_id.clear();
            self.auth_profile_rate_limit_snapshots_by_profile
                .remove(&self.config.selected_auth_profile);
            self.auth_profile_usage_exhausted_reset_at_by_profile
                .remove(&self.config.selected_auth_profile);
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

    pub(super) fn maybe_auto_switch_auth_profile_for_rate_limit(
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
            self.auth_profile_auto_switch_target(&AutoSwitchTrigger {
                limit_id,
                window: &window,
                scope: None,
            })
        else {
            return;
        };
        self.send_auth_profile_auto_switch(next_profile, trigger_key, window);
    }

    /// Auto-switch away from the CURRENT profile in response to its own, authoritative
    /// usage-limit / 429 turn failure.
    ///
    /// A usage-limit *turn error* is emitted by the app server rejecting *this* profile's
    /// own turn, so — unlike a rolling `account/rateLimits/updated` snapshot, which #374
    /// made display-only because a sibling agent spawned on a *different* account can emit
    /// an identity-less 100%-usage snapshot that would be misattributed to the current
    /// profile — it authoritatively identifies the current profile as exhausted. Switching
    /// on it therefore cannot revive the cross-agent false-cascade #374 fixed: a foreign
    /// snapshot never produces a usage-limit turn error on this widget's own profile.
    ///
    /// Prefers an authoritative exhausted snapshot already cached for the current profile
    /// (which carries the exact limiting window). When none has been observed yet — the
    /// common case, because the limit is usually crossed mid-turn, before the ~60s
    /// authoritative heartbeat re-reads it — falls back to a window synthesized from the
    /// operator's enabled auto-switch config so the failed turn still moves to another
    /// configured profile instead of stalling until the (possibly days-away) reset.
    pub(in crate::chatwidget) fn try_auth_profile_switch_for_usage_limit(
        &mut self,
        is_usage_limit: bool,
        error_message: Option<&str>,
    ) -> bool {
        // Cached, account-verified exhaustion (populated only by `AccountUsage` reads,
        // never by a rolling notification) takes priority and carries the exact window.
        if self.try_auth_profile_switch_after_reset_unavailable() {
            return true;
        }
        // Only a hard usage-limit block should synthesize a switch without a corroborating
        // cached snapshot; a transient overload/429 must not rotate profiles.
        if !is_usage_limit || !self.is_session_configured() {
            return false;
        }
        let Some(window) = self.synthetic_usage_limit_auto_switch_window(error_message) else {
            return false;
        };
        let trigger_scope = self.synthetic_auto_switch_trigger_scope();
        let Some((next_profile, trigger_key)) =
            self.auth_profile_auto_switch_target(&AutoSwitchTrigger {
                limit_id: "codex",
                window: &window,
                scope: Some(trigger_scope.as_str()),
            })
        else {
            return false;
        };
        // Every synthetic switch opens a new exhaustion epoch, so the *next* genuine
        // exhaustion can never collapse onto this trigger key even when its reset instant
        // is unknown (see `synthetic_auto_switch_trigger_scope`).
        self.synthetic_auth_profile_auto_switch_epoch = self
            .synthetic_auth_profile_auto_switch_epoch
            .saturating_add(1);
        self.send_auth_profile_auto_switch(next_profile, trigger_key, window);
        true
    }

    /// Disambiguator appended to a synthetic trigger key so that repeated genuine
    /// exhaustions stay distinct.
    ///
    /// A synthetic trigger has no authoritative snapshot behind it, so its reset instant is
    /// frequently unknown and renders as the literal `unknown` in the trigger key. Without a
    /// scope, a second genuine exhaustion produces a byte-identical key, the
    /// `last_auth_profile_auto_switch_trigger` guard rejects it, and the session is stranded
    /// on an exhausted profile while untried profiles remain. The exhausted profile plus a
    /// monotonic per-switch epoch makes each exhaustion its own trigger, while repeats of
    /// the *same* exhaustion (same profile, no switch emitted in between) still collapse
    /// onto one key and are deduped.
    fn synthetic_auto_switch_trigger_scope(&self) -> String {
        let profile = self
            .config
            .selected_auth_profile
            .as_deref()
            .unwrap_or("<default>");
        format!(
            "synthetic#{}:{profile}",
            self.synthetic_auth_profile_auto_switch_epoch
        )
    }

    /// Trigger window used to auto-switch on a genuine usage-limit turn error when no
    /// authoritative exhausted snapshot has driven the switch.
    ///
    /// The label is derived from the window that actually blocked the turn, never from
    /// which auto-switch flags happen to be enabled (both default to `true`, so preferring
    /// weekly would mislabel every 5-hour exhaustion — the common case — and would also
    /// make `HighestAvailable` rank candidates by weekly headroom instead of 5h headroom).
    /// In order:
    /// 1. an authoritative, account-verified exhausted window cached for this profile,
    /// 2. the reset instant advertised in the error text — a reset inside the rolling
    ///    5-hour horizon can only be the 5h window, since a weekly cap resets days out,
    /// 3. otherwise 5h, the overwhelmingly more common exhaustion, falling back to weekly
    ///    when the operator opted out of 5h switching.
    ///
    /// Returns `None` when auto-switch is disabled, when both windows are opted out, or
    /// when the window that actually blocked the turn is one the operator opted out of, so
    /// a disabled window is never switched on.
    fn synthetic_usage_limit_auto_switch_window(
        &self,
        error_message: Option<&str>,
    ) -> Option<UsageProfileAutoSwitchWindow> {
        let config = &self.config.auth_profile_auto_switch;
        if !config.enabled {
            return None;
        }
        let parsed_resets_at = error_message
            .and_then(parse_usage_limit_reset_timestamp)
            .map(|reset_at| reset_at.timestamp());

        if let Some(observed) = self.authoritative_exhausted_codex_window() {
            let resets_at = parsed_resets_at.or(observed.resets_at);
            return auth_profile_auto_switch_label_enabled(&observed.label, config).then_some(
                UsageProfileAutoSwitchWindow {
                    label: observed.label,
                    resets_at,
                },
            );
        }

        let label = match parsed_resets_at {
            Some(resets_at) => {
                let label =
                    usage_limit_window_label_for_reset(resets_at, chrono::Utc::now().timestamp());
                if !auth_profile_auto_switch_label_enabled(label, config) {
                    return None;
                }
                label
            }
            // The blocking window is genuinely unknown: prefer 5h (by far the more common
            // exhaustion) and only fall back to weekly when 5h switching is opted out.
            None if config.on_5h_limit => FIVE_HOUR_LIMIT_LABEL,
            None if config.on_weekly_limit => WEEKLY_LIMIT_LABEL,
            None => return None,
        };
        Some(UsageProfileAutoSwitchWindow {
            label: label.to_string(),
            resets_at: parsed_resets_at,
        })
    }

    /// Exhausted codex window observed by an account-verified `AccountUsage` read for the
    /// current profile, if one has been cached. Rolling notifications never populate this
    /// map (see `on_rate_limit_snapshot_from`), so it cannot be poisoned by a sibling
    /// agent's identity-less snapshot.
    fn authoritative_exhausted_codex_window(&self) -> Option<UsageProfileAutoSwitchWindow> {
        self.auth_profile_auto_switch_snapshots_by_limit_id
            .iter()
            .find_map(|(limit_id, snapshot)| {
                exhausted_auto_switch_window_for_snapshot(
                    snapshot,
                    limit_id.eq_ignore_ascii_case("codex"),
                )
            })
    }

    pub(super) fn maybe_auto_switch_auth_profile_before_user_turn(
        &mut self,
        user_message: &UserMessage,
        history_record: &UserMessageHistoryRecord,
        shell_escape_policy: ShellEscapePolicy,
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
        if limit_id.eq_ignore_ascii_case("codex")
            && window.label.eq_ignore_ascii_case("weekly")
            && self.usage_limit_reset_takes_precedence()
        {
            return false;
        }
        let trigger = AutoSwitchTrigger {
            limit_id: &limit_id,
            window: &window,
            scope: None,
        };
        let trigger_key = auto_switch_trigger_key(&trigger);
        if self.pending_auth_profile_auto_switch_trigger.as_deref() == Some(trigger_key.as_str()) {
            self.queue_user_message_at_position(
                user_message,
                history_record,
                shell_escape_policy,
                queue_position,
            );
            return true;
        }
        let Some((next_profile, trigger_key)) = self.auth_profile_auto_switch_target(&trigger)
        else {
            return false;
        };
        self.queue_user_message_at_position(
            user_message,
            history_record,
            shell_escape_policy,
            queue_position,
        );
        self.send_auth_profile_auto_switch(next_profile, trigger_key, window);
        true
    }

    pub(super) fn queue_user_message_at_position(
        &mut self,
        user_message: &UserMessage,
        history_record: &UserMessageHistoryRecord,
        shell_escape_policy: ShellEscapePolicy,
        queue_position: QueueInsertionPosition,
    ) {
        match queue_position {
            QueueInsertionPosition::Front => {
                self.input_queue.queued_user_messages.push_front(
                    QueuedUserMessage::new_with_shell_escape_policy(
                        user_message.clone(),
                        QueuedInputAction::Plain,
                        shell_escape_policy,
                    ),
                );
                self.input_queue
                    .queued_user_message_history_records
                    .push_front(history_record.clone());
            }
            QueueInsertionPosition::Back => {
                self.input_queue.queued_user_messages.push_back(
                    QueuedUserMessage::new_with_shell_escape_policy(
                        user_message.clone(),
                        QueuedInputAction::Plain,
                        shell_escape_policy,
                    ),
                );
                self.input_queue
                    .queued_user_message_history_records
                    .push_back(history_record.clone());
            }
        }
        self.refresh_pending_input_preview();
    }

    /// Named profiles whose most recent usage heartbeat failed within the failure backoff.
    /// Used to avoid auto-switching onto a profile we could not confirm is available.
    fn recently_failed_auth_profile_usage_heartbeats(&self) -> std::collections::HashSet<String> {
        self.auth_profile_usage_heartbeat_failed_at_by_profile
            .iter()
            .filter(|(_, failed_at)| {
                failed_at.elapsed() < AUTH_PROFILE_USAGE_HEARTBEAT_FAILURE_BACKOFF
            })
            .filter_map(|(profile, _)| profile.clone())
            .collect()
    }

    fn auth_profile_auto_switch_target(
        &mut self,
        trigger: &AutoSwitchTrigger<'_>,
    ) -> Option<(String, String)> {
        let AutoSwitchTrigger {
            limit_id, window, ..
        } = *trigger;
        let trigger_key = auto_switch_trigger_key(trigger);
        if self.last_auth_profile_auto_switch_trigger.as_deref() == Some(trigger_key.as_str()) {
            return None;
        }
        let cooldown_key = self.auth_profile_auto_switch_cooldown_key(limit_id, window);
        if self.auth_profile_auto_switch_cooldown_active(&cooldown_key) {
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
        let recently_failed = self.recently_failed_auth_profile_usage_heartbeats();
        if let Some(target) = broker_auth_profile_auto_switch_target(
            &self.config.auth_profile_auto_switch,
            self.config.selected_auth_profile.as_deref(),
            &profiles,
            &self.auth_profile_rate_limit_snapshots_by_profile,
            &recently_failed,
            trigger,
        ) {
            self.mark_auth_profile_auto_switch_cooldown(cooldown_key);
            return Some((target.profile, target.trigger_key));
        }

        self.add_info_message(
            "Auth profile auto-switch is enabled, but no alternate profile with available usage is known."
                .to_string(),
            /*hint*/ None,
        );
        None
    }

    fn auth_profile_auto_switch_cooldown_key(
        &self,
        limit_id: &str,
        window: &UsageProfileAutoSwitchWindow,
    ) -> UsageProfileCooldownKey {
        UsageProfileCooldownKey {
            profile: self.config.selected_auth_profile.clone(),
            limit_id: limit_id.to_string(),
            window_label: window.label.clone(),
            resets_at: window.resets_at,
        }
    }

    fn auth_profile_auto_switch_cooldown_active(&mut self, key: &UsageProfileCooldownKey) -> bool {
        let now = Instant::now();
        self.auth_profile_auto_switch_cooldowns
            .retain(|_, expires_at| *expires_at > now);
        self.auth_profile_auto_switch_cooldowns.contains_key(key)
    }

    fn mark_auth_profile_auto_switch_cooldown(&mut self, key: UsageProfileCooldownKey) {
        let fallback = Duration::from_secs(
            self.config
                .auth_profile_auto_switch
                .heartbeat_interval_secs
                .max(60),
        );
        let cooldown = cooldown_duration_for_reset(
            key.resets_at,
            chrono::Utc::now().timestamp(),
            self.config.usage_self_heal.reset_retry_buffer_secs,
            fallback,
        );
        self.auth_profile_auto_switch_cooldowns
            .insert(key, Instant::now() + cooldown);
    }

    fn send_auth_profile_auto_switch(
        &mut self,
        next_profile: String,
        trigger_key: String,
        window: UsageProfileAutoSwitchWindow,
    ) {
        self.pending_auth_profile_auto_switch_trigger = Some(trigger_key.clone());
        self.last_auth_profile_auto_switch_trigger = Some(trigger_key);
        self.app_event_tx.send(AppEvent::SwitchAuthProfile {
            profile: Some(next_profile),
            reason: crate::app_event::AuthProfileSwitchReason::AutoRateLimit {
                window: window.label,
            },
            resume_queued_input: true,
            reset_generation: self.rate_limit_reset_generation,
        });
    }

    pub(super) fn stop_rate_limit_poller(&mut self) {
        if let Some(handle) = self.rate_limit_poller.take() {
            handle.abort();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn prefetch_rate_limits(&mut self) {
        self.stop_rate_limit_poller();

        let app_event_tx = self.app_event_tx.clone();
        let heartbeat_interval =
            Duration::from_secs(self.config.auth_profile_auto_switch.heartbeat_interval_secs);
        self.rate_limit_poller = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                app_event_tx.send(AppEvent::RefreshAuthProfileUsageHeartbeats);
            }
        }));
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
                Some(Some(default_effort.clone())),
                /*summary*/ None,
                /*service_tier*/ None,
                /*collaboration_mode*/ None,
                /*session_prompt*/ None,
                /*personality*/ None,
            )));
            tx.send(AppEvent::UpdateModel(switch_model_for_events.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(Some(
                default_effort.clone(),
            )));
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
