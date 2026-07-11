use crate::legacy_core::config::Config;
use crate::legacy_core::config::GoalAutoExecuteMode;
use serde_json::Value as JsonValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommonConfigSection {
    AccountAutomation,
    AiContext,
    InterfacePrivacy,
}

impl CommonConfigSection {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::AccountAutomation => "account-automation",
            Self::AiContext => "ai-context",
            Self::InterfacePrivacy => "interface-privacy",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AccountAutomation => "Account & automation",
            Self::AiContext => "AI context",
            Self::InterfacePrivacy => "Interface & privacy",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::AccountAutomation => "Auth failover, recaps, and update policy.",
            Self::AiContext => "What Codewith shows the model.",
            Self::InterfacePrivacy => "Composer, transcript, privacy, and TUI behavior.",
        }
    }
}

const COMMON_CONFIG_SECTIONS: &[CommonConfigSection] = &[
    CommonConfigSection::AccountAutomation,
    CommonConfigSection::AiContext,
    CommonConfigSection::InterfacePrivacy,
];

#[derive(Clone, Debug)]
pub(crate) struct CommonConfigOption {
    pub(crate) id: &'static str,
    pub(crate) section: CommonConfigSection,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) key_path: Option<&'static str>,
    pub(crate) enabled: bool,
    pub(crate) disabled_reason: Option<&'static str>,
    value_for_enabled: fn(bool) -> JsonValue,
}

impl CommonConfigOption {
    pub(crate) fn value_for_enabled(&self, enabled: bool) -> JsonValue {
        (self.value_for_enabled)(enabled)
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled_reason.is_some()
    }
}

pub(crate) fn common_config_sections() -> &'static [CommonConfigSection] {
    COMMON_CONFIG_SECTIONS
}

pub(crate) fn common_config_options(config: &Config) -> Vec<CommonConfigOption> {
    use CommonConfigSection::*;

    vec![
        disabled_option(
            AccountAutomation,
            "update-checks",
            "Update checks",
            "Off for this internal app. Updates come from explicit internal releases.",
            "Managed by Codewith.",
        ),
        option(
            AccountAutomation,
            "auth-profile-auto-switch",
            "Auth profile auto-switch",
            "Switch to another configured profile after rate limits are exhausted.",
            "auth_profile_auto_switch.enabled",
            config.auth_profile_auto_switch.enabled,
        ),
        option(
            AccountAutomation,
            "switch-on-5h-limit",
            "Switch on 5h limit",
            "Allow auto-switching when the five-hour limit is exhausted.",
            "auth_profile_auto_switch.on_5h_limit",
            config.auth_profile_auto_switch.on_5h_limit,
        ),
        option(
            AccountAutomation,
            "switch-on-weekly-limit",
            "Switch on weekly limit",
            "Allow auto-switching when the weekly limit is exhausted.",
            "auth_profile_auto_switch.on_weekly_limit",
            config.auth_profile_auto_switch.on_weekly_limit,
        ),
        option(
            AccountAutomation,
            "usage-limit-auto-reset",
            "Usage limit auto-reset",
            "Use an available reset after Codewith confirms the weekly limit is exhausted.",
            "usage_limit.auto_reset_enabled",
            config.usage_limit.auto_reset_enabled,
        ),
        option(
            AccountAutomation,
            "session-recap",
            "Session recap",
            "Prepare a one-line summary while the terminal is unfocused.",
            "session_recap.enabled",
            config.session_recap.enabled,
        ),
        goal_auto_execute_option(
            AccountAutomation,
            "automated-goal-plans",
            "Automated goal plans",
            "Advance goal plans when exactly one next goal is ready.",
            "goals.auto_execute",
            config.goals.auto_execute != GoalAutoExecuteMode::Off,
        ),
        option(
            AiContext,
            "environment-context",
            "Environment context",
            "Include the environment_context block in model-visible context.",
            "include_environment_context",
            config.include_environment_context,
        ),
        option(
            AiContext,
            "permission-instructions",
            "Permission instructions",
            "Include current sandbox and approval instructions in model-visible context.",
            "include_permissions_instructions",
            config.include_permissions_instructions,
        ),
        option(
            AiContext,
            "app-instructions",
            "App instructions",
            "Include app and tool-surface instructions in model-visible context.",
            "include_apps_instructions",
            config.include_apps_instructions,
        ),
        option(
            AiContext,
            "collaboration-instructions",
            "Collaboration instructions",
            "Include collaboration-mode instructions in model-visible context.",
            "include_collaboration_mode_instructions",
            config.include_collaboration_mode_instructions,
        ),
        option(
            AiContext,
            "skill-instructions",
            "Skill instructions",
            "Include installed skill instructions in model-visible context.",
            "skills.include_instructions",
            config.include_skill_instructions,
        ),
        inverted_option(
            InterfacePrivacy,
            "paste-burst-detection",
            "Paste burst detection",
            "Detect fast pasted input before inserting it into the composer.",
            "disable_paste_burst",
            !config.disable_paste_burst,
        ),
        option(
            InterfacePrivacy,
            "hide-reasoning-summaries",
            "Hide reasoning summaries",
            "Hide agent reasoning events from the transcript.",
            "hide_agent_reasoning",
            config.hide_agent_reasoning,
        ),
        option(
            InterfacePrivacy,
            "show-raw-reasoning",
            "Show raw reasoning",
            "Show raw reasoning content when the model emits it.",
            "show_raw_agent_reasoning",
            config.show_raw_agent_reasoning,
        ),
        option(
            InterfacePrivacy,
            "animations",
            "Animations",
            "Show spinners, shimmer, and other TUI motion.",
            "tui.animations",
            config.animations,
        ),
        option(
            InterfacePrivacy,
            "tooltips",
            "Tooltips",
            "Show first-run tips and contextual TUI hints.",
            "tui.show_tooltips",
            config.show_tooltips,
        ),
        option(
            InterfacePrivacy,
            "analytics",
            "Analytics",
            "Allow analytics across product surfaces on this machine.",
            "analytics.enabled",
            config.analytics_enabled.unwrap_or(true),
        ),
        option(
            InterfacePrivacy,
            "feedback",
            "Feedback",
            "Allow feedback collection from the TUI.",
            "feedback.enabled",
            config.feedback_enabled,
        ),
        inverted_option(
            InterfacePrivacy,
            "unstable-feature-warnings",
            "Unstable feature warnings",
            "Show warnings for enabled under-development features.",
            "suppress_unstable_features_warning",
            !config.suppress_unstable_features_warning,
        ),
    ]
}

pub(crate) fn common_config_options_for_section(
    config: &Config,
    section: CommonConfigSection,
) -> Vec<CommonConfigOption> {
    common_config_options(config)
        .into_iter()
        .filter(|option| option.section == section)
        .collect()
}

fn option(
    section: CommonConfigSection,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    enabled: bool,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        section,
        label,
        description,
        key_path: Some(key_path),
        enabled,
        disabled_reason: None,
        value_for_enabled: bool_value,
    }
}

fn inverted_option(
    section: CommonConfigSection,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    enabled: bool,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        section,
        label,
        description,
        key_path: Some(key_path),
        enabled,
        disabled_reason: None,
        value_for_enabled: inverted_bool_value,
    }
}

fn goal_auto_execute_option(
    section: CommonConfigSection,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    enabled: bool,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        section,
        label,
        description,
        key_path: Some(key_path),
        enabled,
        disabled_reason: None,
        value_for_enabled: goal_auto_execute_value,
    }
}

fn disabled_option(
    section: CommonConfigSection,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    disabled_reason: &'static str,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        section,
        label,
        description,
        key_path: None,
        enabled: false,
        disabled_reason: Some(disabled_reason),
        value_for_enabled: bool_value,
    }
}

fn bool_value(enabled: bool) -> JsonValue {
    serde_json::json!(enabled)
}

fn inverted_bool_value(enabled: bool) -> JsonValue {
    serde_json::json!(!enabled)
}

fn goal_auto_execute_value(enabled: bool) -> JsonValue {
    serde_json::json!(if enabled { "ready-only" } else { "off" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn common_config_options_expose_logical_enabled_values() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let mut config = ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .build()
            .await
            .expect("config");
        config.disable_paste_burst = false;
        config.suppress_unstable_features_warning = false;

        let options = common_config_options(&config);
        let paste_burst = options
            .iter()
            .find(|option| option.id == "paste-burst-detection")
            .expect("paste burst option");
        let unstable_warnings = options
            .iter()
            .find(|option| option.id == "unstable-feature-warnings")
            .expect("unstable warnings option");
        let automated_goal_plans = options
            .iter()
            .find(|option| option.id == "automated-goal-plans")
            .expect("automated goal plans option");

        assert_eq!(paste_burst.enabled, true);
        assert_eq!(
            paste_burst.value_for_enabled(/*enabled*/ false),
            serde_json::json!(true)
        );
        assert_eq!(unstable_warnings.enabled, true);
        assert_eq!(
            unstable_warnings.value_for_enabled(/*enabled*/ false),
            serde_json::json!(true)
        );
        assert_eq!(automated_goal_plans.enabled, false);
        assert_eq!(
            automated_goal_plans.value_for_enabled(/*enabled*/ true),
            serde_json::json!("ready-only")
        );
        assert_eq!(
            automated_goal_plans.value_for_enabled(/*enabled*/ false),
            serde_json::json!("off")
        );
    }

    #[tokio::test]
    async fn common_config_options_are_split_into_sections() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .build()
            .await
            .expect("config");

        assert_eq!(
            common_config_sections(),
            &[
                CommonConfigSection::AccountAutomation,
                CommonConfigSection::AiContext,
                CommonConfigSection::InterfacePrivacy,
            ]
        );

        let interface_option_ids =
            common_config_options_for_section(&config, CommonConfigSection::InterfacePrivacy)
                .into_iter()
                .map(|option| option.id)
                .collect::<Vec<_>>();

        assert_eq!(
            interface_option_ids,
            vec![
                "paste-burst-detection",
                "hide-reasoning-summaries",
                "show-raw-reasoning",
                "animations",
                "tooltips",
                "analytics",
                "feedback",
                "unstable-feature-warnings",
            ]
        );
    }
}
