use crate::legacy_core::config::Config;
use serde_json::Value as JsonValue;

#[derive(Clone, Debug)]
pub(crate) struct CommonConfigOption {
    pub(crate) id: &'static str,
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

pub(crate) fn common_config_options(config: &Config) -> Vec<CommonConfigOption> {
    vec![
        disabled_option(
            "update-checks",
            "Update checks",
            "Off for this internal app. Updates come from explicit internal releases.",
            "Managed by Codewith.",
        ),
        option(
            "auth-profile-auto-switch",
            "Auth profile auto-switch",
            "Switch to another configured profile after rate limits are exhausted.",
            "auth_profile_auto_switch.enabled",
            config.auth_profile_auto_switch.enabled,
        ),
        option(
            "switch-on-5h-limit",
            "Switch on 5h limit",
            "Allow auto-switching when the five-hour limit is exhausted.",
            "auth_profile_auto_switch.on_5h_limit",
            config.auth_profile_auto_switch.on_5h_limit,
        ),
        option(
            "switch-on-weekly-limit",
            "Switch on weekly limit",
            "Allow auto-switching when the weekly limit is exhausted.",
            "auth_profile_auto_switch.on_weekly_limit",
            config.auth_profile_auto_switch.on_weekly_limit,
        ),
        inverted_option(
            "paste-burst-detection",
            "Paste burst detection",
            "Detect fast pasted input before inserting it into the composer.",
            "disable_paste_burst",
            !config.disable_paste_burst,
        ),
        option(
            "session-recap",
            "Session recap",
            "Prepare a one-line summary while the terminal is unfocused.",
            "session_recap.enabled",
            config.session_recap.enabled,
        ),
        option(
            "hide-reasoning-summaries",
            "Hide reasoning summaries",
            "Hide agent reasoning events from the transcript.",
            "hide_agent_reasoning",
            config.hide_agent_reasoning,
        ),
        option(
            "show-raw-reasoning",
            "Show raw reasoning",
            "Show raw reasoning content when the model emits it.",
            "show_raw_agent_reasoning",
            config.show_raw_agent_reasoning,
        ),
        option(
            "environment-context",
            "Environment context",
            "Include the environment_context block in model-visible context.",
            "include_environment_context",
            config.include_environment_context,
        ),
        option(
            "permission-instructions",
            "Permission instructions",
            "Include current sandbox and approval instructions in model-visible context.",
            "include_permissions_instructions",
            config.include_permissions_instructions,
        ),
        option(
            "app-instructions",
            "App instructions",
            "Include app and tool-surface instructions in model-visible context.",
            "include_apps_instructions",
            config.include_apps_instructions,
        ),
        option(
            "collaboration-instructions",
            "Collaboration instructions",
            "Include collaboration-mode instructions in model-visible context.",
            "include_collaboration_mode_instructions",
            config.include_collaboration_mode_instructions,
        ),
        option(
            "skill-instructions",
            "Skill instructions",
            "Include installed skill instructions in model-visible context.",
            "skills.include_instructions",
            config.include_skill_instructions,
        ),
        inverted_option(
            "unstable-feature-warnings",
            "Unstable feature warnings",
            "Show warnings for enabled under-development features.",
            "suppress_unstable_features_warning",
            !config.suppress_unstable_features_warning,
        ),
        option(
            "analytics",
            "Analytics",
            "Allow analytics across product surfaces on this machine.",
            "analytics.enabled",
            config.analytics_enabled.unwrap_or(true),
        ),
        option(
            "feedback",
            "Feedback",
            "Allow feedback collection from the TUI.",
            "feedback.enabled",
            config.feedback_enabled,
        ),
    ]
}

fn option(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    enabled: bool,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        label,
        description,
        key_path: Some(key_path),
        enabled,
        disabled_reason: None,
        value_for_enabled: bool_value,
    }
}

fn inverted_option(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    enabled: bool,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        label,
        description,
        key_path: Some(key_path),
        enabled,
        disabled_reason: None,
        value_for_enabled: inverted_bool_value,
    }
}

fn disabled_option(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    disabled_reason: &'static str,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
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

        assert_eq!(paste_burst.enabled, true);
        assert_eq!(
            paste_burst.value_for_enabled(false),
            serde_json::json!(true)
        );
        assert_eq!(unstable_warnings.enabled, true);
        assert_eq!(
            unstable_warnings.value_for_enabled(false),
            serde_json::json!(true)
        );
    }
}
