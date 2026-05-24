use std::fs;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use codex_protocol::account::PlanType as AccountPlanType;
use thiserror::Error;

use super::storage::AgentIdentityAuthRecord;
use super::storage::AuthDotJson;
use super::storage::create_auth_storage;

const AUTH_PROFILES_DIR: &str = "auth_profiles";
const ACTIVE_PROFILE_FILE: &str = ".active";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthProfile {
    pub name: String,
    pub auth_mode: AuthMode,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub plan: Option<String>,
    pub active: bool,
}

#[derive(Debug, Error)]
pub enum AuthProfileError {
    #[error("auth profile name cannot be empty")]
    EmptyProfileName,

    #[error(
        "invalid auth profile name `{name}`; use letters, numbers, dots, dashes, or underscores, and start with a letter or number"
    )]
    InvalidProfileName { name: String },

    #[error("auth profile `{name}` does not exist")]
    ProfileNotFound { name: String },

    #[error("not logged in; run `codex login` first")]
    NoActiveAuth,

    #[error("auth profiles require persistent auth storage; ephemeral auth cannot be profiled")]
    EphemeralAuthStorage,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn validate_auth_profile_name(name: &str) -> Result<(), AuthProfileError> {
    if name.is_empty() {
        return Err(AuthProfileError::EmptyProfileName);
    }

    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(AuthProfileError::EmptyProfileName);
    };
    if !first.is_ascii_alphanumeric() {
        return Err(AuthProfileError::InvalidProfileName {
            name: name.to_string(),
        });
    }

    if !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')) {
        return Err(AuthProfileError::InvalidProfileName {
            name: name.to_string(),
        });
    }

    Ok(())
}

pub fn active_auth_profile(codex_home: &Path) -> Result<Option<String>, AuthProfileError> {
    let path = active_profile_file(codex_home);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let name = raw.trim();
    if name.is_empty() {
        return Ok(None);
    }
    validate_auth_profile_name(name)?;
    Ok(Some(name.to_string()))
}

pub fn clear_active_auth_profile(codex_home: &Path) -> Result<bool, AuthProfileError> {
    let path = active_profile_file(codex_home);
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub fn list_auth_profiles(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<Vec<AuthProfile>, AuthProfileError> {
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    let profiles_dir = auth_profiles_dir(codex_home);
    let active_profile = active_auth_profile(codex_home).unwrap_or(None);
    let active_auth = load_active_auth(codex_home, auth_credentials_store_mode)?;
    let entries = match fs::read_dir(&profiles_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };

    let mut profiles = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if validate_auth_profile_name(&name).is_err() {
            continue;
        }
        let storage = create_auth_storage(entry.path(), auth_credentials_store_mode);
        let Some(auth) = storage.load()? else {
            continue;
        };
        let active = active_profile.as_deref() == Some(name.as_str())
            && active_auth.as_ref().is_some_and(|active| active == &auth);
        profiles.push(profile_from_auth(name, &auth, active));
    }
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

pub fn save_current_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthProfile, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    let auth = load_active_auth(codex_home, auth_credentials_store_mode)?
        .ok_or(AuthProfileError::NoActiveAuth)?;
    save_profile_auth(codex_home, auth_credentials_store_mode, name, &auth)?;
    write_active_profile(codex_home, name)?;
    Ok(profile_from_auth(
        name.to_string(),
        &auth,
        /*active*/ true,
    ))
}

pub fn switch_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthProfile, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    let auth = load_profile_auth(codex_home, auth_credentials_store_mode, name)?;
    let active_storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);
    active_storage.save(&auth)?;
    write_active_profile(codex_home, name)?;
    Ok(profile_from_auth(
        name.to_string(),
        &auth,
        /*active*/ true,
    ))
}

pub fn remove_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<(), AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    let profile_dir = auth_profile_dir(codex_home, name);
    if !profile_dir.is_dir() {
        return Err(AuthProfileError::ProfileNotFound {
            name: name.to_string(),
        });
    }

    let storage = create_auth_storage(profile_dir.clone(), auth_credentials_store_mode);
    storage.delete()?;
    match fs::remove_dir_all(&profile_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    if active_auth_profile(codex_home).ok().flatten().as_deref() == Some(name) {
        clear_active_auth_profile(codex_home)?;
    }
    Ok(())
}

pub(crate) fn mirror_active_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    auth: &AuthDotJson,
) -> Result<(), AuthProfileError> {
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;
    let Some(active_profile) = active_auth_profile(codex_home)? else {
        return Ok(());
    };
    save_profile_auth(
        codex_home,
        auth_credentials_store_mode,
        &active_profile,
        auth,
    )
}

fn ensure_persistent_auth_storage(
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<(), AuthProfileError> {
    if auth_credentials_store_mode == AuthCredentialsStoreMode::Ephemeral {
        Err(AuthProfileError::EphemeralAuthStorage)
    } else {
        Ok(())
    }
}

fn auth_profiles_dir(codex_home: &Path) -> PathBuf {
    codex_home.join(AUTH_PROFILES_DIR)
}

fn auth_profile_dir(codex_home: &Path, name: &str) -> PathBuf {
    auth_profiles_dir(codex_home).join(name)
}

fn active_profile_file(codex_home: &Path) -> PathBuf {
    auth_profiles_dir(codex_home).join(ACTIVE_PROFILE_FILE)
}

fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn write_active_profile(codex_home: &Path, name: &str) -> Result<(), AuthProfileError> {
    write_private_file(&active_profile_file(codex_home), name)?;
    Ok(())
}

fn load_active_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<Option<AuthDotJson>, AuthProfileError> {
    let storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);
    Ok(storage.load()?)
}

fn save_profile_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
    auth: &AuthDotJson,
) -> Result<(), AuthProfileError> {
    let profile_dir = auth_profile_dir(codex_home, name);
    create_private_dir_all(&profile_dir)?;
    let storage = create_auth_storage(profile_dir, auth_credentials_store_mode);
    storage.save(auth)?;
    Ok(())
}

fn load_profile_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthDotJson, AuthProfileError> {
    let profile_dir = auth_profile_dir(codex_home, name);
    if !profile_dir.is_dir() {
        return Err(AuthProfileError::ProfileNotFound {
            name: name.to_string(),
        });
    }
    let storage = create_auth_storage(profile_dir, auth_credentials_store_mode);
    storage.load()?.ok_or(AuthProfileError::ProfileNotFound {
        name: name.to_string(),
    })
}

fn profile_from_auth(name: String, auth: &AuthDotJson, active: bool) -> AuthProfile {
    let auth_mode = resolved_auth_mode(auth);
    let (email, account_id, plan) = auth_profile_metadata(auth_mode, auth);
    AuthProfile {
        name,
        auth_mode,
        email,
        account_id,
        plan,
        active,
    }
}

fn resolved_auth_mode(auth: &AuthDotJson) -> AuthMode {
    if let Some(mode) = auth.auth_mode {
        return mode;
    }
    if auth.openai_api_key.is_some() {
        AuthMode::ApiKey
    } else {
        AuthMode::Chatgpt
    }
}

fn auth_profile_metadata(
    auth_mode: AuthMode,
    auth: &AuthDotJson,
) -> (Option<String>, Option<String>, Option<String>) {
    match auth_mode {
        AuthMode::ApiKey => (None, None, None),
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
            let Some(tokens) = auth.tokens.as_ref() else {
                return (None, None, None);
            };
            (
                tokens.id_token.email.clone(),
                tokens
                    .id_token
                    .chatgpt_account_id
                    .clone()
                    .or_else(|| tokens.account_id.clone()),
                tokens.id_token.get_chatgpt_plan_type(),
            )
        }
        AuthMode::AgentIdentity => {
            let Some(agent_identity) = auth.agent_identity.as_deref() else {
                return (None, None, None);
            };
            match AgentIdentityAuthRecord::from_agent_identity_jwt(agent_identity) {
                Ok(record) => (
                    Some(record.email),
                    Some(record.account_id),
                    Some(account_plan_type_label(record.plan_type).to_string()),
                ),
                Err(_) => (None, None, None),
            }
        }
    }
}

fn account_plan_type_label(plan_type: AccountPlanType) -> &'static str {
    match plan_type {
        AccountPlanType::Free => "Free",
        AccountPlanType::Go => "Go",
        AccountPlanType::Plus => "Plus",
        AccountPlanType::Pro => "Pro",
        AccountPlanType::ProLite => "Pro Lite",
        AccountPlanType::Team => "Team",
        AccountPlanType::SelfServeBusinessUsageBased => "Self Serve Business Usage Based",
        AccountPlanType::Business => "Business",
        AccountPlanType::EnterpriseCbpUsageBased => "Enterprise CBP Usage Based",
        AccountPlanType::Enterprise => "Enterprise",
        AccountPlanType::Edu => "Edu",
        AccountPlanType::Unknown => "Unknown",
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
