use crate::legacy_core::config::Config;
use crate::legacy_core::config::GoalAutoExecuteMode;
use crate::legacy_core::config::UsageSelfHealErrorClass;
use serde_json::Value as JsonValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommonConfigSection {
    AccountAutomation,
    SelfHealing,
    AiContext,
    InterfacePrivacy,
}

impl CommonConfigSection {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::AccountAutomation => "account-automation",
            Self::SelfHealing => "self-healing",
            Self::AiContext => "ai-context",
            Self::InterfacePrivacy => "interface-privacy",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AccountAutomation => "Account & automation",
            Self::SelfHealing => "Self-healing",
            Self::AiContext => "AI context",
            Self::InterfacePrivacy => "Interface & privacy",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::AccountAutomation => "Auth failover, usage resets, recaps, and update policy.",
            Self::SelfHealing => "Automatic recovery from usage and model availability errors.",
            Self::AiContext => "What Codewith shows the model.",
            Self::InterfacePrivacy => "Composer, transcript, privacy, and TUI behavior.",
        }
    }
}

const COMMON_CONFIG_SECTIONS: &[CommonConfigSection] = &[
    CommonConfigSection::AccountAutomation,
    CommonConfigSection::SelfHealing,
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
    enabled_value: JsonValue,
    disabled_value: JsonValue,
}

impl CommonConfigOption {
    pub(crate) fn value_for_enabled(&self, enabled: bool) -> JsonValue {
        if enabled {
            self.enabled_value.clone()
        } else {
            self.disabled_value.clone()
        }
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
            SelfHealing,
            "usage-self-heal",
            "Automatic recovery",
            "Retry selected recoverable errors automatically.",
            "usage_self_heal.enabled",
            config.usage_self_heal.enabled,
        ),
        error_class_option(
            SelfHealing,
            "retry-usage-limit",
            "Retry usage limits",
            "Retry after the usage limit reset time when available.",
            "usage_self_heal.retry_errors",
            &config.usage_self_heal.retry_errors,
            UsageSelfHealErrorClass::UsageLimit,
        ),
        error_class_option(
            SelfHealing,
            "retry-model-capacity",
            "Retry model capacity",
            "Retry the current model when it is temporarily at capacity.",
            "usage_self_heal.retry_errors",
            &config.usage_self_heal.retry_errors,
            UsageSelfHealErrorClass::ModelCapacity,
        ),
        error_class_option(
            SelfHealing,
            "switch-model-on-capacity",
            "Switch model on capacity",
            "Try another compatible model before retrying a capacity error.",
            "usage_self_heal.switch_model_errors",
            &config.usage_self_heal.switch_model_errors,
            UsageSelfHealErrorClass::ModelCapacity,
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
        enabled_value: bool_value(/*enabled*/ true),
        disabled_value: bool_value(/*enabled*/ false),
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
        enabled_value: inverted_bool_value(/*enabled*/ true),
        disabled_value: inverted_bool_value(/*enabled*/ false),
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
        enabled_value: goal_auto_execute_value(/*enabled*/ true),
        disabled_value: goal_auto_execute_value(/*enabled*/ false),
    }
}

fn error_class_option(
    section: CommonConfigSection,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    configured: &[UsageSelfHealErrorClass],
    error_class: UsageSelfHealErrorClass,
) -> CommonConfigOption {
    CommonConfigOption {
        id,
        section,
        label,
        description,
        key_path: Some(key_path),
        enabled: configured.contains(&error_class),
        disabled_reason: None,
        enabled_value: error_class_toggle_value(error_class, /*enabled*/ true),
        disabled_value: error_class_toggle_value(error_class, /*enabled*/ false),
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
        enabled_value: bool_value(/*enabled*/ true),
        disabled_value: bool_value(/*enabled*/ false),
    }
}

fn bool_value(enabled: bool) -> JsonValue {
    serde_json::json!(enabled)
}

/// Marker for a toggle that adds or removes a single member of a config list
/// instead of replacing the whole list.
///
/// Several options can share one list `key_path` (both retry error classes live
/// under `usage_self_heal.retry_errors`), and a popup builds every toggle from
/// one config snapshot. Emitting the whole array from that snapshot would make
/// the second toggle of a session resurrect the member the first one removed, so
/// options emit a membership delta that [`resolve_error_class_toggle`] expands
/// against the live config immediately before the write.
const ERROR_CLASS_TOGGLE_KEY: &str = "__codewith_error_class_toggle";

fn error_class_toggle_value(error_class: UsageSelfHealErrorClass, enabled: bool) -> JsonValue {
    let mut toggle = serde_json::Map::new();
    toggle.insert(
        ERROR_CLASS_TOGGLE_KEY.to_string(),
        serde_json::json!({
            "error_class": error_class,
            "enabled": enabled,
        }),
    );
    JsonValue::Object(toggle)
}

/// Expand an error-class membership delta into the array to persist for
/// `key_path`, using `configured` as the live list. Concrete values (anything
/// that is not a delta) pass through unchanged.
pub(crate) fn resolve_error_class_toggle(
    configured: &[UsageSelfHealErrorClass],
    value: JsonValue,
) -> Result<JsonValue, String> {
    let Some(toggle) = value.get(ERROR_CLASS_TOGGLE_KEY) else {
        return Ok(value);
    };
    let raw_error_class = toggle.get("error_class").cloned().unwrap_or_default();
    let error_class: UsageSelfHealErrorClass = serde_json::from_value(raw_error_class)
        .map_err(|err| format!("invalid error class toggle: {err}"))?;
    let enabled = toggle
        .get("enabled")
        .and_then(JsonValue::as_bool)
        .ok_or_else(|| "invalid error class toggle: missing `enabled`".to_string())?;
    let mut error_classes = configured
        .iter()
        .copied()
        .filter(|configured| *configured != error_class)
        .collect::<Vec<_>>();
    if enabled {
        error_classes.push(error_class);
    }
    serde_json::to_value(error_classes).map_err(|err| format!("invalid error class toggle: {err}"))
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
        let retry_usage_limit = options
            .iter()
            .find(|option| option.id == "retry-usage-limit")
            .expect("retry usage limit option");
        let retry_model_capacity = options
            .iter()
            .find(|option| option.id == "retry-model-capacity")
            .expect("retry model capacity option");
        let switch_model_capacity = options
            .iter()
            .find(|option| option.id == "switch-model-on-capacity")
            .expect("switch model capacity option");

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
        assert!(retry_model_capacity.enabled);
        assert!(!switch_model_capacity.enabled);

        // Both retry options share `usage_self_heal.retry_errors`, so they emit a
        // membership delta rather than a snapshot of the whole list. Turning them
        // off one after the other must remove both classes instead of letting the
        // second toggle resurrect the first.
        let mut retry_errors = serde_json::to_value(&config.usage_self_heal.retry_errors)
            .expect("retry errors serialize");
        assert_eq!(
            retry_errors,
            serde_json::json!(["usage_limit", "model_capacity"])
        );
        for option in [retry_usage_limit, retry_model_capacity] {
            let configured: Vec<UsageSelfHealErrorClass> =
                serde_json::from_value(retry_errors).expect("retry errors parse");
            retry_errors = resolve_error_class_toggle(
                &configured,
                option.value_for_enabled(/*enabled*/ false),
            )
            .expect("resolve error class toggle");
        }
        assert_eq!(retry_errors, serde_json::json!([]));

        assert_eq!(
            resolve_error_class_toggle(
                &config.usage_self_heal.switch_model_errors,
                switch_model_capacity.value_for_enabled(/*enabled*/ true),
            )
            .expect("resolve error class toggle"),
            serde_json::json!(["model_capacity"])
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
                CommonConfigSection::SelfHealing,
                CommonConfigSection::AiContext,
                CommonConfigSection::InterfacePrivacy,
            ]
        );

        let self_healing_option_ids =
            common_config_options_for_section(&config, CommonConfigSection::SelfHealing)
                .into_iter()
                .map(|option| option.id)
                .collect::<Vec<_>>();
        assert_eq!(
            self_healing_option_ids,
            vec![
                "usage-self-heal",
                "retry-usage-limit",
                "retry-model-capacity",
                "switch-model-on-capacity",
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
