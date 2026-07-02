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
        personal_access_token: None,
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
fn lists_metadata_only_auth_profiles() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    save_auth_profile_metadata(
        codex_home.path(),
        "work",
        AuthProfileMetadata {
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
            last_permissions: None,
        },
    )?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| (profile.name.as_str(), profile.active))
            .collect::<Vec<_>>(),
        vec![("work", false)]
    );
    assert_eq!(
        profiles.first().map(|profile| profile.auth_mode),
        Some(None)
    );

    Ok(())
}

#[test]
fn removing_active_auth_profile_clears_active_marker() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&auth_with_key("sk-work"))?;
    save_current_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;

    remove_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;

    assert_eq!(active_auth_profile(codex_home.path())?, None);
    assert!(matches!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work"),
        Err(AuthProfileError::ProfileNotFound { name }) if name == "work"
    ));
    Ok(())
}

#[test]
fn rename_auth_profile_preserves_storage_and_active_marker() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let work_auth = auth_with_key("sk-work");
    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&work_auth)?;
    save_current_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;

    let renamed = rename_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        "client",
    )?;

    assert_eq!(
        renamed,
        AuthProfile {
            name: "client".to_string(),
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
            auth_mode: Some(AuthMode::ApiKey),
            email: None,
            account_id: None,
            plan: None,
            active: true,
        }
    );
    assert_eq!(
        active_auth_profile(codex_home.path())?.as_deref(),
        Some("client")
    );
    assert_eq!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "client")?,
        work_auth
    );
    assert!(matches!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work"),
        Err(AuthProfileError::ProfileNotFound { name }) if name == "work"
    ));

    Ok(())
}

#[test]
fn rename_inactive_auth_profile_keeps_other_active_marker() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let work_auth = auth_with_key("sk-work");
    let personal_auth = auth_with_key("sk-personal");
    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&personal_auth)?;
    save_current_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &work_auth,
    )?;

    let renamed = rename_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        "client",
    )?;

    assert_eq!(
        renamed,
        AuthProfile {
            name: "client".to_string(),
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
            auth_mode: Some(AuthMode::ApiKey),
            email: None,
            account_id: None,
            plan: None,
            active: false,
        }
    );
    assert_eq!(
        active_auth_profile(codex_home.path())?.as_deref(),
        Some("personal")
    );
    assert_eq!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "client")?,
        work_auth
    );

    Ok(())
}

#[test]
fn rename_auth_profile_rejects_existing_target() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &auth_with_key("sk-work"),
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        &auth_with_key("sk-personal"),
    )?;

    assert!(matches!(
        rename_auth_profile(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            "work",
            "personal",
        ),
        Err(AuthProfileError::ProfileAlreadyExists { name }) if name == "personal"
    ));
    let work_auth = load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "work")?;
    let personal_auth = load_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
    )?;
    assert_eq!(work_auth.openai_api_key.as_deref(), Some("sk-work"));
    assert_eq!(personal_auth.openai_api_key.as_deref(), Some("sk-personal"));

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
fn auth_profile_metadata_round_trips_and_renames() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &auth_with_key("sk-work"),
    )?;

    assert_eq!(
        load_auth_profile_metadata(codex_home.path(), "work")?,
        AuthProfileMetadata {
            subscription_provider: AuthProfileSubscriptionProvider::ChatGpt,
            last_permissions: None,
        }
    );

    let metadata = AuthProfileMetadata {
        subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
        last_permissions: Some(AuthProfilePermissionSettings {
            default_permissions:
                codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS.to_string(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            approvals_reviewer: codex_config::types::ApprovalsReviewer::User,
        }),
    };
    save_auth_profile_metadata(codex_home.path(), "work", metadata.clone())?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| (profile.name.as_str(), profile.subscription_provider))
            .collect::<Vec<_>>(),
        vec![("work", AuthProfileSubscriptionProvider::ClaudeAi)]
    );

    let renamed = rename_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        "client",
    )?;

    assert_eq!(
        renamed.subscription_provider,
        AuthProfileSubscriptionProvider::ClaudeAi
    );
    assert_eq!(
        load_auth_profile_metadata(codex_home.path(), "client")?,
        metadata
    );

    Ok(())
}

#[test]
fn external_subscription_profiles_do_not_require_auth_json() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let root_auth = auth_with_key("sk-root");
    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&root_auth)?;

    let metadata = AuthProfileMetadata {
        subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
        last_permissions: None,
    };
    save_auth_profile_metadata(codex_home.path(), "claude", metadata.clone())?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles,
        vec![AuthProfile {
            name: "claude".to_string(),
            subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
            auth_mode: None,
            email: None,
            account_id: None,
            plan: None,
            active: false,
        }]
    );

    assert!(matches!(
        switch_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "claude"),
        Err(AuthProfileError::NonChatGptProfile {
            name,
            provider: AuthProfileSubscriptionProvider::ClaudeAi,
        }) if name == "claude"
    ));
    assert_eq!(active_auth_profile(codex_home.path())?, None);
    assert_eq!(active_storage.load()?, Some(root_auth));

    let renamed = rename_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "claude",
        "claude-work",
    )?;
    assert_eq!(renamed.name, "claude-work");
    assert_eq!(
        renamed.subscription_provider,
        AuthProfileSubscriptionProvider::ClaudeAi
    );
    assert_eq!(renamed.auth_mode, None);
    assert_eq!(active_auth_profile(codex_home.path())?, None);
    assert_eq!(
        load_auth_profile_metadata(codex_home.path(), "claude-work")?,
        metadata
    );

    Ok(())
}

#[test]
fn external_subscription_profiles_ignore_stray_openai_auth() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let metadata = AuthProfileMetadata {
        subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
        last_permissions: None,
    };
    save_auth_profile_metadata(codex_home.path(), "claude", metadata)?;
    save_profile_auth(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "claude",
        &auth_with_key("sk-stray"),
    )?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;

    assert_eq!(
        profiles,
        vec![AuthProfile {
            name: "claude".to_string(),
            subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
            auth_mode: None,
            email: None,
            account_id: None,
            plan: None,
            active: false,
        }]
    );

    Ok(())
}

#[test]
fn mirror_active_auth_profile_skips_external_profiles() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let original_auth = auth_with_key("sk-original");
    let refreshed_auth = auth_with_key("sk-refreshed");
    let active_storage = create_auth_storage(
        codex_home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
    );
    active_storage.save(&original_auth)?;
    save_auth_profile_metadata(
        codex_home.path(),
        "claude",
        AuthProfileMetadata {
            subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
            last_permissions: None,
        },
    )?;
    write_active_profile(codex_home.path(), "claude")?;

    mirror_active_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        &refreshed_auth,
    )?;

    assert_eq!(active_storage.load()?, Some(original_auth));
    assert!(matches!(
        load_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "claude"),
        Err(AuthProfileError::ProfileNotFound { name }) if name == "claude"
    ));

    Ok(())
}

#[test]
fn chatgpt_auth_cannot_be_saved_into_external_profile() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    save_auth_profile_metadata(
        codex_home.path(),
        "claude",
        AuthProfileMetadata {
            subscription_provider: AuthProfileSubscriptionProvider::ClaudeAi,
            last_permissions: None,
        },
    )?;

    assert!(matches!(
        save_auth_profile(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            "claude",
            &auth_with_key("sk-chatgpt"),
        ),
        Err(AuthProfileError::NonChatGptProfile {
            name,
            provider: AuthProfileSubscriptionProvider::ClaudeAi,
        }) if name == "claude"
    ));

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

#[test]
fn moving_auth_profiles_persists_manual_order() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &auth_with_key("sk-work"),
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        &auth_with_key("sk-personal"),
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "client",
        &auth_with_key("sk-client"),
    )?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        vec!["client", "personal", "work"]
    );

    assert!(move_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        AuthProfileMoveDirection::Down,
    )?);
    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        vec!["client", "work", "personal"]
    );

    assert!(!move_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        AuthProfileMoveDirection::Down,
    )?);
    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        vec!["client", "work", "personal"]
    );

    Ok(())
}

#[test]
fn rename_and_remove_auth_profiles_update_manual_order() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        &auth_with_key("sk-work"),
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        &auth_with_key("sk-personal"),
    )?;
    save_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "client",
        &auth_with_key("sk-client"),
    )?;
    move_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "personal",
        AuthProfileMoveDirection::Down,
    )?;

    rename_auth_profile(
        codex_home.path(),
        AuthCredentialsStoreMode::File,
        "work",
        "team",
    )?;
    remove_auth_profile(codex_home.path(), AuthCredentialsStoreMode::File, "client")?;

    let profiles = list_auth_profiles(codex_home.path(), AuthCredentialsStoreMode::File)?;
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        vec!["team", "personal"]
    );

    Ok(())
}
