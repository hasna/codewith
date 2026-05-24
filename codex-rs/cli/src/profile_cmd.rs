use codex_app_server_protocol::AuthMode;
use codex_core::config::Config;
use codex_login::AuthProfile;
use codex_login::AuthProfileError;
use codex_utils_cli::CliConfigOverrides;

pub async fn run_profile_list(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    match codex_login::list_auth_profiles(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
    ) {
        Ok(profiles) => {
            print_profiles(&profiles);
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

    println!("  {:<24} {:<28} {:<18} PLAN", "NAME", "ACCOUNT", "MODE");
    for profile in profiles {
        let marker = if profile.active { "*" } else { " " };
        let account = profile
            .email
            .as_deref()
            .or(profile.account_id.as_deref())
            .unwrap_or("-");
        let plan = profile.plan.as_deref().unwrap_or("-");
        println!(
            "{marker} {:<24} {:<28} {:<18} {plan}",
            profile.name,
            account,
            auth_mode_label(profile.auth_mode)
        );
    }
}

fn auth_mode_label(auth_mode: AuthMode) -> &'static str {
    match auth_mode {
        AuthMode::ApiKey => "api_key",
        AuthMode::Chatgpt => "chatgpt",
        AuthMode::ChatgptAuthTokens => "chatgpt_auth_tokens",
        AuthMode::AgentIdentity => "agent_identity",
    }
}

fn exit_with_profile_error(action: &str, err: AuthProfileError) -> ! {
    eprintln!("Error: failed to {action}: {err}");
    std::process::exit(1);
}
