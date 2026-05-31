use super::*;
use crate::auth::storage::AuthDotJson;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

fn auth_with_key(key: &str) -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(key.to_string()),
        tokens: None,
        last_refresh: Some(Utc::now()),
        agent_identity: None,
    }
}

#[test]
fn validates_auth_profile_names() {
    assert!(validate_auth_profile_name("work").is_ok());
    assert!(validate_auth_profile_name("work.dev_1").is_ok());

    assert!(matches!(
        validate_auth_profile_name(""),
        Err(AuthProfileError::EmptyProfileName)
    ));
    assert!(matches!(
        validate_auth_profile_name(".hidden"),
        Err(AuthProfileError::InvalidProfileName { .. })
    ));
    assert!(matches!(
        validate_auth_profile_name("nested/work"),
        Err(AuthProfileError::InvalidProfileName { .. })
    ));
}

#[test]
fn ensure_auth_profile_storage_dir_creates_private_profile_dir() -> anyhow::Result<()> {
    let codex_home = tempdir()?;

    let profile_dir = ensure_auth_profile_storage_dir(codex_home.path(), "work")?;

    assert_eq!(
        profile_dir,
        codex_home.path().join("auth_profiles").join("work")
    );
    assert!(profile_dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&profile_dir)?.permissions().mode() & 0o777,
            0o700
        );
    }

    Ok(())
}

#[test]
fn saves_lists_switches_and_removes_auth_profiles() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let work_auth = auth_with_key("sk-work");
    let personal_auth = auth_with_key("sk-personal");

    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&work_auth)?;
    let saved_work =
        save_current_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;
    assert_eq!(saved_work.name, "work");
    assert!(saved_work.active);

    active_storage.save(&personal_auth)?;
    save_current_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
    )?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| (profile.name.as_str(), profile.active))
            .collect::<Vec<_>>(),
        vec![("personal", true), ("work", false)]
    );

    switch_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;
    assert_eq!(active_storage.load()?, Some(work_auth));

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| (profile.name.as_str(), profile.active))
            .collect::<Vec<_>>(),
        vec![("personal", false), ("work", true)]
    );

    remove_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
    )?;
    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        vec!["work"]
    );

    Ok(())
}

#[test]
fn mirror_active_auth_profile_updates_selected_profile() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let original_auth = auth_with_key("sk-original");
    let refreshed_auth = auth_with_key("sk-refreshed");

    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&original_auth)?;
    save_current_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;

    active_storage.save(&refreshed_auth)?;
    mirror_active_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        &refreshed_auth,
    )?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| (profile.name.as_str(), profile.active))
            .collect::<Vec<_>>(),
        vec![("work", true)]
    );
    switch_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;
    assert_eq!(active_storage.load()?, Some(refreshed_auth));

    Ok(())
}

#[test]
fn profile_scoped_storage_does_not_touch_root_auth_or_active_marker() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let root_auth = auth_with_key("sk-root");
    let work_auth = auth_with_key("sk-work");

    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&root_auth)?;

    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &work_auth,
    )?;

    assert_eq!(active_storage.load()?, Some(root_auth));
    assert_eq!(active_auth_profile(codex_home.path())?, None);
    assert_eq!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?,
        work_auth
    );

    Ok(())
}

#[test]
fn profile_scoped_delete_only_removes_target_profile() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let root_auth = auth_with_key("sk-root");
    let work_auth = auth_with_key("sk-work");
    let personal_auth = auth_with_key("sk-personal");

    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&root_auth)?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &work_auth,
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        &personal_auth,
    )?;

    delete_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;

    assert_eq!(active_storage.load()?, Some(root_auth));
    assert!(matches!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work"),
        Err(AuthProfileError::ProfileNotFound { name }) if name == "work"
    ));
    assert_eq!(
        load_auth_profile(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            "personal"
        )?,
        personal_auth
    );

    Ok(())
}
