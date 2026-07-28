use crate::config::AuthProfileAutoSwitchConfig;
use crate::config::AuthProfileAutoSwitchStrategy;
use crate::function_tool::FunctionCallError;
use crate::session::session::SessionSettingsUpdate;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::auth_profile_control_spec::MANAGE_AUTH_PROFILES_TOOL_NAME;
use crate::tools::handlers::auth_profile_control_spec::create_manage_auth_profiles_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_app_server_protocol::AuthMode;
use codex_extension_api::ToolWorktreeMutationSignal;
use codex_login::AuthProfile;
use codex_login::AuthProfileSubscriptionProvider;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;

pub struct ManageAuthProfilesHandler;

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ManageAuthProfilesAction {
    List,
    Current,
    Switch,
    SetAutoSwitch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ManageAuthProfilesArgs {
    action: ManageAuthProfilesAction,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    auto_switch_enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageAuthProfilesResponse {
    action: ManageAuthProfilesAction,
    current_profile: Option<String>,
    switched_to: Option<String>,
    profiles: Vec<AuthProfileSummary>,
    auto_switch: AuthProfileAutoSwitchState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthProfileSummary {
    name: Option<String>,
    display_name: String,
    subscription_provider: AuthProfileSubscriptionProvider,
    auth_mode: Option<AuthMode>,
    plan: Option<String>,
    current: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthProfileAutoSwitchState {
    enabled: bool,
    on_5h_limit: bool,
    on_weekly_limit: bool,
    strategy: &'static str,
    profiles: Vec<String>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for ManageAuthProfilesHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MANAGE_AUTH_PROFILES_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_manage_auth_profiles_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "manage_auth_profiles handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ManageAuthProfilesArgs = parse_arguments(&arguments)?;
        let mut current_profile = session.selected_auth_profile().await;
        let mut switched_to = None;

        match args.action {
            ManageAuthProfilesAction::Switch => {
                let profile = normalize_requested_profile(args.profile)?;
                session
                    .update_settings(SessionSettingsUpdate {
                        auth_profile: Some(profile.clone()),
                        ..Default::default()
                    })
                    .await
                    .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
                let turn_context = session
                    .new_default_turn_with_sub_id(turn.sub_id.clone())
                    .await;
                session
                    .refresh_token_context_window_for_profile_switch(&turn_context)
                    .await;
                current_profile = profile.clone();
                switched_to = profile;
            }
            ManageAuthProfilesAction::SetAutoSwitch => {
                let enabled = args.auto_switch_enabled.ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "auto_switch_enabled is required when action is set_auto_switch"
                            .to_string(),
                    )
                })?;
                session
                    .update_settings(SessionSettingsUpdate {
                        auth_profile_auto_switch_enabled: Some(enabled),
                        ..Default::default()
                    })
                    .await
                    .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
            }
            ManageAuthProfilesAction::List | ManageAuthProfilesAction::Current => {}
        }

        let config = session.get_config().await;
        let profiles = codex_login::list_auth_profiles(
            &config.codex_home,
            config.cli_auth_credentials_store_mode,
        )
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
        let response = ManageAuthProfilesResponse {
            action: args.action,
            current_profile: current_profile.clone(),
            switched_to,
            profiles: summarize_profiles(profiles, current_profile.as_deref()),
            auto_switch: summarize_auto_switch(&config.auth_profile_auto_switch),
        };
        let response = serde_json::to_string_pretty(&response)
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            response,
            Some(true),
        )))
    }
}

impl CoreToolRuntime for ManageAuthProfilesHandler {
    fn worktree_mutation_signal(&self, _invocation: &ToolInvocation) -> ToolWorktreeMutationSignal {
        ToolWorktreeMutationSignal::NoWorktreeMutation
    }
}

fn normalize_requested_profile(
    profile: Option<String>,
) -> Result<Option<String>, FunctionCallError> {
    let Some(profile) = profile else {
        return Ok(None);
    };
    let profile = profile.trim();
    if profile.is_empty() {
        return Ok(None);
    }
    codex_login::validate_auth_profile_name(profile)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
    Ok(Some(profile.to_string()))
}

fn summarize_profiles(
    profiles: Vec<AuthProfile>,
    current_profile: Option<&str>,
) -> Vec<AuthProfileSummary> {
    let mut summaries = vec![AuthProfileSummary {
        name: None,
        display_name: "Default".to_string(),
        subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
        auth_mode: None,
        plan: None,
        current: current_profile.is_none(),
    }];

    summaries.extend(profiles.into_iter().map(|profile| AuthProfileSummary {
        current: current_profile == Some(profile.name.as_str()),
        name: Some(profile.name.clone()),
        display_name: profile.name,
        subscription_provider: profile.subscription_provider,
        auth_mode: profile.auth_mode,
        plan: profile.plan,
    }));

    summaries
}

fn summarize_auto_switch(config: &AuthProfileAutoSwitchConfig) -> AuthProfileAutoSwitchState {
    AuthProfileAutoSwitchState {
        enabled: config.enabled,
        on_5h_limit: config.on_5h_limit,
        on_weekly_limit: config.on_weekly_limit,
        strategy: auth_profile_auto_switch_strategy_name(config.strategy),
        profiles: config.profiles.clone(),
    }
}

fn auth_profile_auto_switch_strategy_name(strategy: AuthProfileAutoSwitchStrategy) -> &'static str {
    match strategy {
        AuthProfileAutoSwitchStrategy::HighestAvailable => "highest_available",
        AuthProfileAutoSwitchStrategy::Ordered => "ordered",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn normalize_requested_profile_treats_missing_or_blank_as_default() {
        assert_eq!(normalize_requested_profile(/*profile*/ None).unwrap(), None);
        assert_eq!(
            normalize_requested_profile(Some("  ".to_string())).unwrap(),
            None
        );
    }

    #[test]
    fn normalize_requested_profile_validates_names() {
        assert!(normalize_requested_profile(Some("work.dev_1".to_string())).is_ok());
        assert!(normalize_requested_profile(Some("../work".to_string())).is_err());
    }

    #[test]
    fn summarize_profiles_includes_subscription_provider() {
        let summaries = summarize_profiles(
            vec![AuthProfile {
                name: "claude-work".to_string(),
                subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
                auth_mode: None,
                email: Some("private@example.com".to_string()),
                account_id: Some("account-private-123".to_string()),
                plan: None,
                active: false,
            }],
            Some("claude-work"),
        );

        let response = serde_json::to_value(&summaries).expect("serialize summaries");
        assert_eq!(
            response,
            serde_json::json!([
                {
                    "name": null,
                    "displayName": "Default",
                    "subscriptionProvider": "chat-gpt",
                    "authMode": null,
                    "plan": null,
                    "current": false
                },
                {
                    "name": "claude-work",
                    "displayName": "claude-work",
                    "subscriptionProvider": "claude-ai",
                    "authMode": null,
                    "plan": null,
                    "current": true
                }
            ])
        );
        let serialized = response.to_string();
        assert!(!serialized.contains("private@example.com"));
        assert!(!serialized.contains("account-private-123"));
    }

    #[test]
    fn summarize_auto_switch_serializes_session_state() {
        let state = summarize_auto_switch(&AuthProfileAutoSwitchConfig {
            enabled: true,
            profiles: vec!["work".to_string(), "spare".to_string()],
            on_5h_limit: true,
            on_weekly_limit: false,
            strategy: AuthProfileAutoSwitchStrategy::Ordered,
            heartbeat_interval_secs: 60,
            heartbeat_freshness_secs: 120,
        });

        let response = serde_json::to_value(state).expect("serialize auto switch state");
        assert_eq!(
            response,
            serde_json::json!({
                "enabled": true,
                "on5hLimit": true,
                "onWeeklyLimit": false,
                "strategy": "ordered",
                "profiles": ["work", "spare"]
            })
        );
    }
}
