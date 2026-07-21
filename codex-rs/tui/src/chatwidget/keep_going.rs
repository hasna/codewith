//! Session-scoped keep-going / auto-resume for `/keep-going`.
//!
//! When enabled, a clean turn-end (the model returned a final message and the
//! session would otherwise stop and fire the "agent turn complete" notification)
//! automatically injects a neutral continuation prompt and starts the next turn.
//!
//! Keep-going is bounded (`config.keep_going.max_continuations` per user turn) and
//! opt-in (default OFF). It never bypasses approvals, the sandbox, or any refusal:
//! it only starts a normal turn after the previous one fully completed and the
//! session is idle, and every tool call in the continued turn still passes all
//! existing enforcement. It deliberately does not fire on errors, on
//! approval-denied ends, while a modal/approval is pending, in Plan mode, while a
//! goal is active, or while a usage-limit reset / self-heal / auth-switch owns the
//! turn.

use super::*;

const KEEP_GOING_ENABLED_HINT: &str =
    "After a clean turn-end Codewith will auto-continue until the work is done or the cap is hit.";
const KEEP_GOING_DISABLED_HINT: &str = "Turns will stop normally and wait for your next message.";
const KEEP_GOING_ON_NOTICE: &str = "Keep-going enabled.";
const KEEP_GOING_OFF_NOTICE: &str = "Keep-going disabled.";
const KEEP_GOING_STATUS_ON: &str = "Keep-going is enabled.";
const KEEP_GOING_STATUS_OFF: &str = "Keep-going is disabled.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeepGoingCommand {
    Toggle,
    Enable,
    Disable,
    Status,
}

impl KeepGoingCommand {
    pub(super) fn parse(args: &str) -> Result<Self, &'static str> {
        match args.trim().to_ascii_lowercase().as_str() {
            "" => Ok(Self::Toggle),
            "on" | "enable" | "enabled" => Ok(Self::Enable),
            "off" | "disable" | "disabled" | "stop" => Ok(Self::Disable),
            "status" => Ok(Self::Status),
            _ => Err("Usage: /keep-going [on|off|status]"),
        }
    }
}

impl ChatWidget {
    pub(super) fn apply_keep_going_command(&mut self, command: KeepGoingCommand) {
        match command {
            KeepGoingCommand::Toggle => {
                self.set_keep_going_and_notify(!self.keep_going_enabled);
            }
            KeepGoingCommand::Enable => {
                self.set_keep_going_and_notify(/*enabled*/ true);
            }
            KeepGoingCommand::Disable => {
                self.set_keep_going_and_notify(/*enabled*/ false);
            }
            KeepGoingCommand::Status => {
                self.add_info_message(
                    Self::keep_going_status_notice(self.keep_going_active()).to_string(),
                    Some(Self::keep_going_hint(self.keep_going_active()).to_string()),
                );
            }
        }
    }

    fn set_keep_going_and_notify(&mut self, enabled: bool) {
        self.keep_going_enabled = enabled;
        // Turning keep-going off must abandon any in-flight continuation budget so
        // a later re-enable starts from a clean cap.
        if !enabled {
            self.continuations_this_user_turn = 0;
        }
        self.add_info_message(
            Self::keep_going_change_notice(enabled).to_string(),
            Some(Self::keep_going_hint(enabled).to_string()),
        );
    }

    /// Whether keep-going is active for this session: the runtime `/keep-going`
    /// flag overrides the persisted `[keep_going].enabled` default.
    pub(super) fn keep_going_active(&self) -> bool {
        self.keep_going_enabled || self.config.keep_going.enabled
    }

    #[cfg(test)]
    pub(crate) fn keep_going_enabled_runtime(&self) -> bool {
        self.keep_going_enabled
    }

    #[cfg(test)]
    pub(crate) fn keep_going_continuations_this_user_turn(&self) -> u32 {
        self.continuations_this_user_turn
    }

    fn keep_going_change_notice(enabled: bool) -> &'static str {
        if enabled {
            KEEP_GOING_ON_NOTICE
        } else {
            KEEP_GOING_OFF_NOTICE
        }
    }

    fn keep_going_status_notice(enabled: bool) -> &'static str {
        if enabled {
            KEEP_GOING_STATUS_ON
        } else {
            KEEP_GOING_STATUS_OFF
        }
    }

    fn keep_going_hint(enabled: bool) -> &'static str {
        if enabled {
            KEEP_GOING_ENABLED_HINT
        } else {
            KEEP_GOING_DISABLED_HINT
        }
    }

    /// Reset the per-user-turn continuation budget. Called whenever a real user
    /// message is submitted so a fresh user turn gets the full cap.
    pub(super) fn reset_keep_going_continuations(&mut self) {
        self.continuations_this_user_turn = 0;
    }

    /// After a clean turn-end, decide whether keep-going should auto-continue and,
    /// if so, inject the continuation prompt and start the next turn.
    ///
    /// Returns `true` when a continuation was started (so the caller suppresses the
    /// "agent turn complete" notification), `false` otherwise.
    pub(super) fn maybe_run_keep_going_continuation(&mut self) -> bool {
        if !self.can_run_keep_going_continuation() {
            return false;
        }

        let prompt = self.config.keep_going.prompt.clone();
        if !self.submit_keep_going_turn(prompt) {
            return false;
        }
        self.continuations_this_user_turn = self.continuations_this_user_turn.saturating_add(1);
        let remaining = self
            .config
            .keep_going
            .max_continuations
            .saturating_sub(self.continuations_this_user_turn);
        self.add_info_message(
            format!(
                "Keep-going: auto-continuing ({}/{}).",
                self.continuations_this_user_turn, self.config.keep_going.max_continuations
            ),
            Some(if remaining == 0 {
                "Continuation cap reached after this turn. /keep-going off to stop.".to_string()
            } else {
                "/keep-going off to stop.".to_string()
            }),
        );
        true
    }

    /// All guardrails that must hold before keep-going may auto-continue.
    ///
    /// This is intentionally conservative: any doubt means fall through to the
    /// normal turn-complete notification instead of auto-continuing.
    fn can_run_keep_going_continuation(&self) -> bool {
        // Opt-in only.
        if !self.keep_going_active() {
            return false;
        }
        // Hard per-user-turn cap so keep-going can never loop forever.
        if self.continuations_this_user_turn >= self.config.keep_going.max_continuations {
            return false;
        }
        // Never double-drive with the goal loop.
        if self
            .current_goal_status
            .as_ref()
            .is_some_and(GoalStatusState::is_active)
        {
            return false;
        }
        // `try_start_turn_if_idle` refuses in Plan mode; mirror that here so the
        // synthetic continuation never starts a Plan-mode turn.
        if self.active_mode_kind() == ModeKind::Plan {
            return false;
        }
        // Only continue when nothing is waiting on the user.
        if !self.bottom_pane.no_modal_or_popup_active() {
            return false;
        }
        // The session must be genuinely idle (the turn we are completing has been
        // finished before this runs).
        if self.is_user_turn_pending_or_running() {
            return false;
        }
        // Do not fight the usage-limit reset / self-heal / auth-switch machinery
        // when it owns the failed turn or has a retry pending.
        if self.automatic_usage_limit_reset_owns_failed_turn()
            || self.manual_usage_limit_reset_is_active()
            || self.has_pending_usage_self_heal_retry()
        {
            return false;
        }
        // A pending rate-limit model-switch prompt is about to be shown for the
        // user to decide; do not preempt it with an auto-continuation.
        if matches!(
            self.rate_limit_switch_prompt,
            RateLimitSwitchPromptState::Pending
        ) {
            return false;
        }
        true
    }

    /// Start the next turn with the continuation prompt.
    ///
    /// This reuses the normal user-turn submission op (the client analog of the
    /// goal loop's `Session::try_start_turn_if_idle`): because the previous turn
    /// has fully completed and the session is idle, the op starts a fresh turn
    /// rather than steering into an active one. The prompt is not rendered as a
    /// user cell and is not appended to message-history recall, matching the goal
    /// loop's silent continuation.
    fn submit_keep_going_turn(&mut self, prompt: String) -> bool {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return false;
        }
        let effective_mode = self.effective_collaboration_mode();
        if effective_mode.model().trim().is_empty() {
            return false;
        }

        let items = vec![UserInput::Text {
            text: prompt,
            text_elements: Vec::new(),
        }];

        let additional_context = self
            .teaching_mode_enabled
            .then(teaching_mode::teaching_mode_additional_context);
        let collaboration_mode = if self.collaboration_modes_enabled() {
            self.active_collaboration_mask
                .as_ref()
                .map(|_| effective_mode.clone())
        } else {
            None
        };
        let personality = self
            .config
            .personality
            .filter(|_| self.config.features.enabled(Feature::Personality))
            .filter(|_| self.current_model_supports_personality());
        let service_tier = self.service_tier_update_for_core();
        let active_permission_profile = self.config.permissions.active_permission_profile();
        let op = AppCommand::user_turn(
            items,
            self.config.cwd.to_path_buf(),
            AskForApproval::from(self.config.permissions.approval_policy.value()),
            active_permission_profile,
            self.config.model_provider_id.clone(),
            effective_mode.model().to_string(),
            effective_mode.reasoning_effort(),
            /*summary*/ None,
            service_tier,
            /*final_output_json_schema*/ None,
            additional_context,
            collaboration_mode,
            personality,
        );

        if !self.submit_op(op) {
            return false;
        }
        self.input_queue.user_turn_pending_start = true;
        self.transcript.needs_final_message_separator = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_going_command_parses_aliases() {
        assert_eq!(KeepGoingCommand::parse(""), Ok(KeepGoingCommand::Toggle));
        assert_eq!(KeepGoingCommand::parse("on"), Ok(KeepGoingCommand::Enable));
        assert_eq!(
            KeepGoingCommand::parse("ENABLE"),
            Ok(KeepGoingCommand::Enable)
        );
        assert_eq!(KeepGoingCommand::parse("off"), Ok(KeepGoingCommand::Disable));
        assert_eq!(
            KeepGoingCommand::parse("stop"),
            Ok(KeepGoingCommand::Disable)
        );
        assert_eq!(
            KeepGoingCommand::parse("status"),
            Ok(KeepGoingCommand::Status)
        );
        assert!(KeepGoingCommand::parse("wat").is_err());
    }
}
