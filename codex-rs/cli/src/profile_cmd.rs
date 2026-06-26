use codex_app_server_protocol::AuthMode;
use codex_core::config::Config;
use codex_login::AuthProfile;
use codex_login::AuthProfileError;
use codex_login::AuthProfileSubscriptionProvider;
use codex_utils_cli::CliConfigOverrides;
use serde_json::Value;
use serde_json::json;

pub async fn run_profile_list(cli_config_overrides: CliConfigOverrides, json_output: bool) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    match codex_login::list_auth_profiles(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
    ) {
        Ok(profiles) => {
            if json_output {
                if let Err(err) =
                    print_profiles_json(&profiles, config.selected_auth_profile.as_deref())
                {
                    eprintln!("Error serializing auth profiles: {err}");
                    std::process::exit(1);
                }
            } else {
                print_profiles(&profiles);
            }
            std::process::exit(0);
        }
        Err(err) => exit_with_profile_error("list auth profiles", err),
    }
}

pub async fn run_profile_save(cli_config_overrides: CliConfigOverrides, name: String) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    match codex_login::save_current_auth_profile(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        &name,
    ) {
        Ok(profile) => {
            eprintln!("Saved auth profile `{}`", profile.name);
            std::process::exit(0);
        }
        Err(err) => exit_with_profile_error("save auth profile", err),
    }
}

pub async fn run_profile_switch(cli_config_overrides: CliConfigOverrides, name: String) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    match codex_login::switch_auth_profile(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        &name,
    ) {
        Ok(profile) => {
            eprintln!("Switched to auth profile `{}`", profile.name);
            std::process::exit(0);
        }
        Err(err) => exit_with_profile_error("switch auth profile", err),
    }
}

pub async fn run_profile_remove(cli_config_overrides: CliConfigOverrides, name: String) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    match codex_login::remove_auth_profile(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        &name,
    ) {
        Ok(()) => {
            eprintln!("Removed auth profile `{name}`");
            std::process::exit(0);
        }
        Err(err) => exit_with_profile_error("remove auth profile", err),
    }
}

async fn load_config_or_exit(cli_config_overrides: CliConfigOverrides) -> Config {
    let cli_overrides = match cli_config_overrides.parse_overrides() {
        Ok(overrides) => overrides,
        Err(err) => {
            eprintln!("Error parsing -c overrides: {err}");
            std::process::exit(1);
        }
    };

    match Config::load_with_cli_overrides(cli_overrides).await {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Error loading configuration: {err}");
            std::process::exit(1);
        }
    }
}

fn print_profiles(profiles: &[AuthProfile]) {
    if profiles.is_empty() {
        println!("No auth profiles saved.");
        return;
    }

    println!(
        "  {:<24} {:<28} {:<14} {:<18} PLAN",
        "NAME", "ACCOUNT", "PROVIDER", "MODE"
    );
    for profile in profiles {
        let marker = if profile.active { "*" } else { " " };
        let account = profile
            .email
            .as_deref()
            .or(profile.account_id.as_deref())
            .unwrap_or("-");
        let plan = profile.plan.as_deref().unwrap_or("-");
        println!(
            "{marker} {:<24} {:<28} {:<14} {:<18} {plan}",
            profile.name,
            account,
            profile.subscription_provider,
            auth_mode_display_label(profile.auth_mode)
        );
    }
}

fn print_profiles_json(
    profiles: &[AuthProfile],
    selected_auth_profile: Option<&str>,
) -> Result<(), serde_json::Error> {
    println!(
        "{}",
        serde_json::to_string_pretty(&profiles_json_value(profiles, selected_auth_profile))?
    );
    Ok(())
}

fn profiles_json_value(profiles: &[AuthProfile], selected_auth_profile: Option<&str>) -> Value {
    let current_profile = json!({
        "name": selected_auth_profile,
        "profileKind": if selected_auth_profile.is_some() { "named" } else { "default" },
        "available": selected_auth_profile.is_none_or(|selected| {
            profiles.iter().any(|profile| profile.name == selected)
        }),
    });
    let data = profiles
        .iter()
        .map(|profile| {
            let account_label = profile
                .email
                .as_deref()
                .or(profile.account_id.as_deref())
                .map(str::to_string);
            let auth_mode = profile.auth_mode.map(auth_mode_wire_label);
            let unusable_reason = profile_unusable_reason(profile);
            json!({
                "name": profile.name.as_str(),
                "profileKind": "named",
                "active": profile.active,
                "selected": selected_auth_profile == Some(profile.name.as_str()),
                "subscriptionProvider": profile.subscription_provider,
                "authMode": auth_mode,
                "accountLabel": account_label,
                "email": profile.email.as_deref(),
                "accountId": profile.account_id.as_deref(),
                "plan": profile.plan.as_deref(),
                "usable": unusable_reason.is_none(),
                "unusableReason": unusable_reason,
            })
        })
        .collect::<Vec<_>>();
    json!({ "currentProfile": current_profile, "data": data })
}

fn profile_unusable_reason(profile: &AuthProfile) -> Option<&'static str> {
    if profile.subscription_provider != AuthProfileSubscriptionProvider::ChatGpt {
        Some("unsupported_subscription_provider")
    } else if profile.auth_mode.is_none() {
        Some("missing_auth")
    } else {
        None
    }
}

fn auth_mode_display_label(auth_mode: Option<AuthMode>) -> &'static str {
    match auth_mode {
        Some(auth_mode) => auth_mode_wire_label(auth_mode),
        None => "-",
    }
}

fn auth_mode_wire_label(auth_mode: AuthMode) -> &'static str {
    match auth_mode {
        AuthMode::ApiKey => "api_key",
        AuthMode::Chatgpt => "chatgpt",
        AuthMode::ChatgptAuthTokens => "chatgpt_auth_tokens",
        AuthMode::AgentIdentity => "agent_identity",
        AuthMode::PersonalAccessToken => "personal_access_token",
    }
}

fn exit_with_profile_error(action: &str, err: AuthProfileError) -> ! {
    eprintln!("Error: failed to {action}: {err}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_login::AuthProfileSubscriptionProvider;
    use pretty_assertions::assert_eq;

    #[test]
    fn profiles_json_value_marks_selection_usability_and_account_label() {
        let value = profiles_json_value(
            &[
                AuthProfile {
                    name: "work".to_string(),
                    subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
                    auth_mode: Some(AuthMode::Chatgpt),
                    email: Some("work@example.com".to_string()),
                    account_id: Some("acct_123".to_string()),
                    plan: Some("team".to_string()),
                    active: true,
                },
                AuthProfile {
                    name: "claude".to_string(),
                    subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
                    auth_mode: None,
                    email: None,
                    account_id: None,
                    plan: None,
                    active: false,
                },
            ],
            Some("work"),
        );

        assert_eq!(
            value,
            json!({
                "currentProfile": {
                    "name": "work",
                    "profileKind": "named",
                    "available": true,
                },
                "data": [
                    {
                        "name": "work",
                        "profileKind": "named",
                        "active": true,
                        "selected": true,
                        "subscriptionProvider": "chat-gpt",
                        "authMode": "chatgpt",
                        "accountLabel": "work@example.com",
                        "email": "work@example.com",
                        "accountId": "acct_123",
                        "plan": "team",
                        "usable": true,
                        "unusableReason": null,
                    },
                    {
                        "name": "claude",
                        "profileKind": "named",
                        "active": false,
                        "selected": false,
                        "subscriptionProvider": "claude-ai",
                        "authMode": null,
                        "accountLabel": null,
                        "email": null,
                        "accountId": null,
                        "plan": null,
                        "usable": false,
                        "unusableReason": "unsupported_subscription_provider",
                    }
                ]
            })
        );
    }

    #[test]
    fn profiles_json_value_reports_default_current_profile() {
        let value = profiles_json_value(&[], None);

        assert_eq!(
            value,
            json!({
                "currentProfile": {
                    "name": null,
                    "profileKind": "default",
                    "available": true,
                },
                "data": []
            })
        );
    }
}
