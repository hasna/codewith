//! Session-scoped teaching mode for `/teach`.

use super::*;
use std::collections::HashMap;

use codex_app_server_protocol::AdditionalContextEntry;
use codex_app_server_protocol::AdditionalContextKind;

const TEACHING_MODE_ENABLED_HINT: &str =
    "Future replies may include compact Teaching note callouts when they help.";
const TEACHING_MODE_DISABLED_HINT: &str = "Future replies will use normal concise behavior.";
const TEACHING_MODE_ON_NOTICE: &str = "Teaching mode enabled.";
const TEACHING_MODE_OFF_NOTICE: &str = "Teaching mode disabled.";
const TEACHING_MODE_STATUS_ON: &str = "Teaching mode is enabled.";
const TEACHING_MODE_STATUS_OFF: &str = "Teaching mode is disabled.";
pub(super) const TEACHING_MODE_CONTEXT_KEY: &str = "teaching_mode";

const TEACHING_MODE_INSTRUCTIONS: &str = "\
Teaching mode is enabled for this session.

Teach while you work, but do not spam every step. Add a concise teaching callout only when it would help the user understand an important choice, concept, or tradeoff.

Audience: a junior biocoder. Use plain English for someone technical who may not know this codebase deeply. Be clear without being patronizing.

When useful, format the callout as a compact Markdown blockquote:
> **Teaching note**
> What it means: ...
> Why this approach: ...
> Watch out for: ...

Do not reveal hidden chain-of-thought, private reasoning, or internal thoughts. Share only user-facing rationale and educational context.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TeachingModeCommand {
    Toggle,
    Enable,
    Disable,
    Status,
}

impl TeachingModeCommand {
    pub(super) fn parse(args: &str) -> Result<Self, &'static str> {
        match args.trim().to_ascii_lowercase().as_str() {
            "" => Ok(Self::Toggle),
            "on" | "enable" | "enabled" => Ok(Self::Enable),
            "off" | "disable" | "disabled" => Ok(Self::Disable),
            "status" => Ok(Self::Status),
            _ => Err("Usage: /teach [on|off|status]"),
        }
    }
}

pub(super) fn teaching_mode_additional_context() -> HashMap<String, AdditionalContextEntry> {
    HashMap::from([(
        TEACHING_MODE_CONTEXT_KEY.to_string(),
        AdditionalContextEntry {
            value: TEACHING_MODE_INSTRUCTIONS.to_string(),
            kind: AdditionalContextKind::Application,
        },
    )])
}

impl ChatWidget {
    pub(super) fn apply_teaching_mode_command(&mut self, command: TeachingModeCommand) {
        match command {
            TeachingModeCommand::Toggle => {
                self.set_teaching_mode_and_notify(!self.teaching_mode_enabled);
            }
            TeachingModeCommand::Enable => {
                self.set_teaching_mode_and_notify(/*enabled*/ true);
            }
            TeachingModeCommand::Disable => {
                self.set_teaching_mode_and_notify(/*enabled*/ false);
            }
            TeachingModeCommand::Status => {
                self.add_info_message(
                    Self::teaching_mode_status_notice(self.teaching_mode_enabled).to_string(),
                    Some(Self::teaching_mode_hint(self.teaching_mode_enabled).to_string()),
                );
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn teaching_mode_enabled(&self) -> bool {
        self.teaching_mode_enabled
    }

    fn set_teaching_mode_and_notify(&mut self, enabled: bool) {
        self.teaching_mode_enabled = enabled;
        if let Some(thread_id) = self.thread_id {
            if enabled {
                self.teaching_mode_by_thread.insert(thread_id, true);
            } else {
                self.teaching_mode_by_thread.remove(&thread_id);
            }
        }
        self.add_info_message(
            Self::teaching_mode_change_notice(enabled).to_string(),
            Some(Self::teaching_mode_hint(enabled).to_string()),
        );
    }

    fn teaching_mode_change_notice(enabled: bool) -> &'static str {
        if enabled {
            TEACHING_MODE_ON_NOTICE
        } else {
            TEACHING_MODE_OFF_NOTICE
        }
    }

    fn teaching_mode_status_notice(enabled: bool) -> &'static str {
        if enabled {
            TEACHING_MODE_STATUS_ON
        } else {
            TEACHING_MODE_STATUS_OFF
        }
    }

    fn teaching_mode_hint(enabled: bool) -> &'static str {
        if enabled {
            TEACHING_MODE_ENABLED_HINT
        } else {
            TEACHING_MODE_DISABLED_HINT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teaching_mode_context_mentions_callout_and_private_reasoning_guardrail() {
        let context = teaching_mode_additional_context();
        let entry = context
            .get(TEACHING_MODE_CONTEXT_KEY)
            .expect("missing teaching context");
        let text = &entry.value;

        assert!(text.contains("> **Teaching note**"));
        assert!(text.contains("What it means:"));
        assert!(text.contains("Why this approach:"));
        assert!(text.contains("Watch out for:"));
        assert!(text.contains("Do not reveal hidden chain-of-thought"));
        assert!(!text.contains("show your reasoning"));
        assert_eq!(entry.kind, AdditionalContextKind::Application);
    }
}
