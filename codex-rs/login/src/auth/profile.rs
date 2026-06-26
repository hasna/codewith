use std::fmt;
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
use codex_config::types::ApprovalsReviewer;
use codex_config::types::AuthCredentialsStoreMode;
use codex_protocol::account::PlanType as AccountPlanType;
use codex_protocol::protocol::AskForApproval;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use super::storage::AgentIdentityAuthRecord;
use super::storage::AuthDotJson;
use super::storage::create_auth_storage;

const AUTH_PROFILES_DIR: &str = "auth_profiles";
const ACTIVE_PROFILE_FILE: &str = ".active";
const PROFILE_ORDER_FILE: &str = ".order";
const PROFILE_METADATA_FILE: &str = "profile.json";
pub const CODEWITH_AUTH_PROFILE_ENV_VAR: &str = "CODEWITH_AUTH_PROFILE";
pub const CODEX_AUTH_PROFILE_ENV_VAR: &str = "CODEX_AUTH_PROFILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthProfile {
    pub name: String,
    pub subscription_provider: AuthProfileSubscriptionProvider,
    pub auth_mode: Option<AuthMode>,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub plan: Option<String>,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthProfileSubscriptionProvider {
    #[default]
    ChatGpt,
    ClaudeAi,
    Cursor,
    Grok,
}

impl AuthProfileSubscriptionProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::ChatGpt => "ChatGPT",
            Self::ClaudeAi => "Claude.ai",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok",
        }
    }
}

impl fmt::Display for AuthProfileSubscriptionProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProfileMetadata {
    pub subscription_provider: AuthProfileSubscriptionProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_permissions: Option<AuthProfilePermissionSettings>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProfilePermissionSettings {
    pub default_permissions: String,
    pub approval_policy: AskForApproval,
    #[serde(default)]
    pub approvals_reviewer: ApprovalsReviewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthProfileMoveDirection {
    Up,
    Down,
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

    #[error("auth profile `{name}` already exists")]
    ProfileAlreadyExists { name: String },

    #[error("not logged in; run `codewith login` first")]
    NoActiveAuth,

    #[error("auth profiles require persistent auth storage; ephemeral auth cannot be profiled")]
    EphemeralAuthStorage,

    #[error(
        "auth profile `{name}` is tied to {provider}; use ChatGPT login with a ChatGPT auth profile"
    )]
    NonChatGptProfile {
        name: String,
        provider: AuthProfileSubscriptionProvider,
    },

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
        let profile_dir = entry.path();
        let metadata = load_profile_metadata(&profile_dir)?;
        let profile_auth =
            if metadata.subscription_provider == AuthProfileSubscriptionProvider::ChatGpt {
                load_optional_profile_auth(codex_home, auth_credentials_store_mode, &name)?
            } else {
                None
            };
        let active = active_profile.as_deref() == Some(name.as_str())
            && match profile_auth.as_ref() {
                Some(auth) => active_auth.as_ref().is_some_and(|active| active == auth),
                None => true,
            };
        if metadata.subscription_provider != AuthProfileSubscriptionProvider::ChatGpt {
            profiles.push(profile_from_metadata(name, metadata, active));
            continue;
        }
        match profile_auth {
            Some(auth) => profiles.push(profile_from_auth(name, &auth, active, metadata)),
            None => profiles.push(profile_from_metadata(name, metadata, active)),
        }
    }
    sort_auth_profiles(codex_home, &mut profiles)?;
    Ok(profiles)
}

pub fn save_current_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthProfile, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;
    ensure_chatgpt_profile_target(codex_home, name)?;

    let auth = load_active_auth(codex_home, auth_credentials_store_mode)?
        .ok_or(AuthProfileError::NoActiveAuth)?;
    save_profile_auth(codex_home, auth_credentials_store_mode, name, &auth)?;
    write_profile_metadata(
        &auth_profile_dir(codex_home, name),
        &AuthProfileMetadata::default(),
    )?;
    write_active_profile(codex_home, name)?;
    Ok(profile_from_auth(
        name.to_string(),
        &auth,
        /*active*/ true,
        AuthProfileMetadata::default(),
    ))
}

pub fn save_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
    auth: &AuthDotJson,
) -> Result<AuthProfile, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;
    ensure_chatgpt_profile_target(codex_home, name)?;

    save_profile_auth(codex_home, auth_credentials_store_mode, name, auth)?;
    write_profile_metadata(
        &auth_profile_dir(codex_home, name),
        &AuthProfileMetadata::default(),
    )?;
    Ok(profile_from_auth(
        name.to_string(),
        auth,
        /*active*/ false,
        AuthProfileMetadata::default(),
    ))
}

pub fn load_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthDotJson, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;
    load_profile_auth(codex_home, auth_credentials_store_mode, name)
}

pub fn delete_auth_profile(
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

    delete_profile_storage_dir(profile_dir, auth_credentials_store_mode)?;
    remove_profile_from_order(codex_home, name)?;
    Ok(())
}

pub fn switch_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthProfile, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    let metadata = load_profile_metadata(&auth_profile_dir(codex_home, name))?;
    if metadata.subscription_provider != AuthProfileSubscriptionProvider::ChatGpt {
        return Err(AuthProfileError::NonChatGptProfile {
            name: name.to_string(),
            provider: metadata.subscription_provider,
        });
    }

    let auth = load_profile_auth(codex_home, auth_credentials_store_mode, name)?;
    let active_storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);
    active_storage.save(&auth)?;
    write_active_profile(codex_home, name)?;
    Ok(profile_from_auth(
        name.to_string(),
        &auth,
        /*active*/ true,
        metadata,
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

    delete_profile_storage_dir(profile_dir, auth_credentials_store_mode)?;

    if active_auth_profile(codex_home).ok().flatten().as_deref() == Some(name) {
        clear_active_auth_profile(codex_home)?;
    }
    remove_profile_from_order(codex_home, name)?;
    Ok(())
}

pub fn rename_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    old_name: &str,
    new_name: &str,
) -> Result<AuthProfile, AuthProfileError> {
    validate_auth_profile_name(old_name)?;
    validate_auth_profile_name(new_name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    if old_name == new_name {
        let active = active_auth_profile(codex_home).ok().flatten().as_deref() == Some(old_name);
        let metadata = load_profile_metadata(&auth_profile_dir(codex_home, old_name))?;
        let auth = load_optional_profile_auth(codex_home, auth_credentials_store_mode, old_name)?;
        return match auth {
            Some(auth) => Ok(profile_from_auth(
                old_name.to_string(),
                &auth,
                active,
                metadata,
            )),
            None if metadata.subscription_provider != AuthProfileSubscriptionProvider::ChatGpt => {
                Ok(profile_from_metadata(
                    old_name.to_string(),
                    metadata,
                    active,
                ))
            }
            None => Err(AuthProfileError::ProfileNotFound {
                name: old_name.to_string(),
            }),
        };
    }

    let old_profile_dir = auth_profile_dir(codex_home, old_name);
    if !old_profile_dir.is_dir() {
        return Err(AuthProfileError::ProfileNotFound {
            name: old_name.to_string(),
        });
    }

    let new_profile_dir = auth_profile_dir(codex_home, new_name);
    if new_profile_dir.exists() {
        return Err(AuthProfileError::ProfileAlreadyExists {
            name: new_name.to_string(),
        });
    }

    let metadata = load_profile_metadata(&old_profile_dir)?;
    let auth = load_optional_profile_auth(codex_home, auth_credentials_store_mode, old_name)?;
    if metadata.subscription_provider == AuthProfileSubscriptionProvider::ChatGpt && auth.is_none()
    {
        return Err(AuthProfileError::ProfileNotFound {
            name: old_name.to_string(),
        });
    }
    let active = active_auth_profile(codex_home).ok().flatten().as_deref() == Some(old_name);
    // Keyring-backed stores derive their key from the profile directory, so
    // migrate auth through the storage abstraction instead of only renaming the
    // dir when auth exists.
    if let Some(auth) = auth.as_ref() {
        save_profile_auth(codex_home, auth_credentials_store_mode, new_name, auth)?;
    } else {
        create_private_dir_all(&new_profile_dir)?;
    }
    write_profile_metadata(&auth_profile_dir(codex_home, new_name), &metadata)?;
    delete_profile_storage_dir(old_profile_dir, auth_credentials_store_mode)?;
    if active {
        write_active_profile(codex_home, new_name)?;
    }
    rename_profile_in_order(codex_home, old_name, new_name)?;

    match auth {
        Some(auth) => Ok(profile_from_auth(
            new_name.to_string(),
            &auth,
            active,
            metadata,
        )),
        None => Ok(profile_from_metadata(
            new_name.to_string(),
            metadata,
            active,
        )),
    }
}

pub fn move_auth_profile(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
    direction: AuthProfileMoveDirection,
) -> Result<bool, AuthProfileError> {
    validate_auth_profile_name(name)?;
    ensure_persistent_auth_storage(auth_credentials_store_mode)?;

    let profiles = list_auth_profiles(codex_home, auth_credentials_store_mode)?;
    let mut ordered_names: Vec<String> = profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    let Some(index) = ordered_names.iter().position(|profile| profile == name) else {
        return Err(AuthProfileError::ProfileNotFound {
            name: name.to_string(),
        });
    };
    let target_index = match direction {
        AuthProfileMoveDirection::Up => index.checked_sub(1),
        AuthProfileMoveDirection::Down => (index + 1 < ordered_names.len()).then_some(index + 1),
    };
    let Some(target_index) = target_index else {
        return Ok(false);
    };

    ordered_names.swap(index, target_index);
    write_auth_profile_order(codex_home, &ordered_names)?;
    Ok(true)
}

pub fn auth_profile_storage_dir(
    codex_home: &Path,
    name: &str,
) -> Result<PathBuf, AuthProfileError> {
    validate_auth_profile_name(name)?;
    Ok(auth_profile_dir(codex_home, name))
}

pub fn ensure_auth_profile_storage_dir(
    codex_home: &Path,
    name: &str,
) -> Result<PathBuf, AuthProfileError> {
    let profile_dir = auth_profile_storage_dir(codex_home, name)?;
    create_private_dir_all(&profile_dir)?;
    Ok(profile_dir)
}

pub fn load_auth_profile_metadata(
    codex_home: &Path,
    name: &str,
) -> Result<AuthProfileMetadata, AuthProfileError> {
    validate_auth_profile_name(name)?;
    let profile_dir = auth_profile_dir(codex_home, name);
    if !profile_dir.is_dir() {
        return Err(AuthProfileError::ProfileNotFound {
            name: name.to_string(),
        });
    }
    load_profile_metadata(&profile_dir)
}

pub fn save_auth_profile_metadata(
    codex_home: &Path,
    name: &str,
    metadata: AuthProfileMetadata,
) -> Result<(), AuthProfileError> {
    let profile_dir = ensure_auth_profile_storage_dir(codex_home, name)?;
    write_profile_metadata(&profile_dir, &metadata)?;
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
    let profile_dir = auth_profile_dir(codex_home, &active_profile);
    if load_profile_metadata(&profile_dir)?.subscription_provider
        != AuthProfileSubscriptionProvider::ChatGpt
    {
        return Ok(());
    }
    save_profile_auth(
        codex_home,
        auth_credentials_store_mode,
        &active_profile,
        auth,
    )
}

fn ensure_chatgpt_profile_target(codex_home: &Path, name: &str) -> Result<(), AuthProfileError> {
    let profile_dir = auth_profile_dir(codex_home, name);
    if !profile_dir.is_dir() {
        return Ok(());
    }
    let metadata = load_profile_metadata(&profile_dir)?;
    if metadata.subscription_provider == AuthProfileSubscriptionProvider::ChatGpt {
        Ok(())
    } else {
        Err(AuthProfileError::NonChatGptProfile {
            name: name.to_string(),
            provider: metadata.subscription_provider,
        })
    }
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

fn profile_order_file(codex_home: &Path) -> PathBuf {
    auth_profiles_dir(codex_home).join(PROFILE_ORDER_FILE)
}

fn profile_metadata_file(profile_dir: &Path) -> PathBuf {
    profile_dir.join(PROFILE_METADATA_FILE)
}

fn load_profile_metadata(profile_dir: &Path) -> Result<AuthProfileMetadata, AuthProfileError> {
    let raw = match fs::read_to_string(profile_metadata_file(profile_dir)) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(AuthProfileMetadata::default());
        }
        Err(err) => return Err(err.into()),
    };
    serde_json::from_str(&raw)
        .map_err(|err| std::io::Error::new(ErrorKind::InvalidData, err).into())
}

fn write_profile_metadata(
    profile_dir: &Path,
    metadata: &AuthProfileMetadata,
) -> Result<(), AuthProfileError> {
    let json = serde_json::to_string_pretty(&metadata)
        .map_err(|err| std::io::Error::new(ErrorKind::InvalidData, err))?;
    write_private_file(&profile_metadata_file(profile_dir), &json)?;
    Ok(())
}

fn sort_auth_profiles(
    codex_home: &Path,
    profiles: &mut [AuthProfile],
) -> Result<(), AuthProfileError> {
    let order = read_auth_profile_order(codex_home)?;
    profiles.sort_by(|left, right| {
        let left_order = order.iter().position(|name| name == &left.name);
        let right_order = order.iter().position(|name| name == &right.name);
        match (left_order, right_order) {
            (Some(left_order), Some(right_order)) => left_order.cmp(&right_order),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.name.cmp(&right.name),
        }
    });
    Ok(())
}

fn read_auth_profile_order(codex_home: &Path) -> Result<Vec<String>, AuthProfileError> {
    let raw = match fs::read_to_string(profile_order_file(codex_home)) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut names = Vec::new();
    for line in raw.lines() {
        let name = line.trim();
        if validate_auth_profile_name(name).is_ok() && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn write_auth_profile_order(codex_home: &Path, names: &[String]) -> Result<(), AuthProfileError> {
    let mut contents = names.join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }
    write_private_file(&profile_order_file(codex_home), &contents)?;
    Ok(())
}

fn rewrite_auth_profile_order_if_present(
    codex_home: &Path,
    rewrite: impl FnOnce(&mut Vec<String>),
) -> Result<(), AuthProfileError> {
    let path = profile_order_file(codex_home);
    if !path.exists() {
        return Ok(());
    }
    let mut order = read_auth_profile_order(codex_home)?;
    rewrite(&mut order);
    write_auth_profile_order(codex_home, &order)
}

fn remove_profile_from_order(codex_home: &Path, name: &str) -> Result<(), AuthProfileError> {
    rewrite_auth_profile_order_if_present(codex_home, |order| {
        order.retain(|profile| profile != name);
    })
}

fn rename_profile_in_order(
    codex_home: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<(), AuthProfileError> {
    rewrite_auth_profile_order_if_present(codex_home, |order| {
        for profile in order {
            if profile == old_name {
                *profile = new_name.to_string();
            }
        }
    })
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
    load_auth_with_fallback(
        codex_home.to_path_buf(),
        auth_credentials_store_mode,
        /*profile_name*/ None,
    )
}

fn load_auth_with_fallback(
    storage_home: PathBuf,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    profile_name: Option<&str>,
) -> Result<Option<AuthDotJson>, AuthProfileError> {
    let primary_storage = create_auth_storage(storage_home.clone(), auth_credentials_store_mode);
    match primary_storage.load() {
        Ok(Some(auth)) => Ok(Some(auth)),
        Ok(None) if auth_credentials_store_mode != AuthCredentialsStoreMode::File => {
            let fallback_storage =
                create_auth_storage(storage_home, AuthCredentialsStoreMode::File);
            fallback_storage.load().map_err(Into::into)
        }
        Ok(None) => Ok(None),
        Err(err) if auth_credentials_store_mode != AuthCredentialsStoreMode::File => {
            tracing::debug!(
                profile = profile_name.unwrap_or("<root>"),
                mode = ?auth_credentials_store_mode,
                error = %err,
                "failed to load auth from configured storage, falling back to file"
            );
            let fallback_storage =
                create_auth_storage(storage_home, AuthCredentialsStoreMode::File);
            fallback_storage.load().map_err(Into::into)
        }
        Err(err) => Err(err.into()),
    }
}

fn save_profile_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
    auth: &AuthDotJson,
) -> Result<(), AuthProfileError> {
    let profile_dir = ensure_auth_profile_storage_dir(codex_home, name)?;
    let storage = create_auth_storage(profile_dir, auth_credentials_store_mode);
    storage.save(auth)?;
    Ok(())
}

fn load_profile_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<AuthDotJson, AuthProfileError> {
    load_optional_profile_auth(codex_home, auth_credentials_store_mode, name)?.ok_or(
        AuthProfileError::ProfileNotFound {
            name: name.to_string(),
        },
    )
}

fn load_optional_profile_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    name: &str,
) -> Result<Option<AuthDotJson>, AuthProfileError> {
    let profile_dir = auth_profile_dir(codex_home, name);
    if !profile_dir.is_dir() {
        return Err(AuthProfileError::ProfileNotFound {
            name: name.to_string(),
        });
    }
    load_auth_with_fallback(profile_dir, auth_credentials_store_mode, Some(name))
}

fn delete_profile_storage_dir(
    profile_dir: PathBuf,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<(), AuthProfileError> {
    let storage = create_auth_storage(profile_dir.clone(), auth_credentials_store_mode);
    storage.delete()?;
    match fs::remove_dir_all(&profile_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn profile_from_auth(
    name: String,
    auth: &AuthDotJson,
    active: bool,
    metadata: AuthProfileMetadata,
) -> AuthProfile {
    let auth_mode = resolved_auth_mode(auth);
    let (email, account_id, plan) = auth_profile_metadata(auth_mode, auth);
    AuthProfile {
        name,
        subscription_provider: metadata.subscription_provider,
        auth_mode: Some(auth_mode),
        email,
        account_id,
        plan,
        active,
    }
}

fn profile_from_metadata(name: String, metadata: AuthProfileMetadata, active: bool) -> AuthProfile {
    AuthProfile {
        name,
        subscription_provider: metadata.subscription_provider,
        auth_mode: None,
        email: None,
        account_id: None,
        plan: None,
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
        AuthMode::ApiKey | AuthMode::PersonalAccessToken => (None, None, None),
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
