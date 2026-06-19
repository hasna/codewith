use super::*;
use codex_app_server_protocol::AskForApproval;
use codex_config::types::ApprovalsReviewer;
use color_eyre::eyre::WrapErr;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn app_scoped_key_path_quotes_dotted_app_ids() {
    assert_eq!(
        app_scoped_key_path("plugin.linear", "enabled"),
        "apps.\"plugin.linear\".enabled"
    );
}

#[test]
fn model_provider_selection_edits_update_provider_model_and_effort() {
    assert_eq!(
        build_model_provider_selection_edits(
            Some("team.prod"),
            "openrouter",
            "openai/o4-mini",
            Some("medium"),
        ),
        vec![
            replace_config_value(
                "profiles.\"team.prod\".model_provider",
                serde_json::json!("openrouter"),
            ),
            replace_config_value(
                "profiles.\"team.prod\".model_gateway",
                serde_json::json!("openrouter"),
            ),
            replace_config_value(
                "profiles.\"team.prod\".model",
                serde_json::json!("openai/o4-mini"),
            ),
            replace_config_value(
                "profiles.\"team.prod\".model_reasoning_effort",
                serde_json::json!("medium"),
            ),
        ]
    );
}

#[test]
fn model_provider_selection_edits_clear_default_effort() {
    assert_eq!(
        build_model_provider_selection_edits(
            /*profile*/ None,
            "openrouter",
            "openai/o4-mini",
            /*effort*/ Option::<String>::None,
        ),
        vec![
            replace_config_value("model_provider", serde_json::json!("openrouter")),
            replace_config_value("model_gateway", serde_json::json!("openrouter")),
            replace_config_value("model", serde_json::json!("openai/o4-mini")),
            clear_config_value("model_reasoning_effort"),
        ]
    );
}

#[test]
fn trusted_project_edit_targets_project_trust_level() {
    assert_eq!(
        trusted_project_edit(Path::new("/workspace/team.project")),
        ConfigEdit {
            key_path: "projects.\"/workspace/team.project\".trust_level".to_string(),
            value: serde_json::json!("trusted"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn permission_profile_selection_edits_persist_profile_policy_and_reviewer() {
    assert_eq!(
        build_permission_profile_selection_edits(
            ":danger-full-access",
            Some(AskForApproval::Never),
            Some(ApprovalsReviewer::User),
        ),
        vec![
            replace_config_value(
                "default_permissions",
                serde_json::json!(":danger-full-access"),
            ),
            replace_config_value("approval_policy", serde_json::json!("never")),
            replace_config_value("approvals_reviewer", serde_json::json!("user")),
        ]
    );
}

#[test]
fn format_config_error_preserves_server_validation_message() {
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: features.fast_mode=true violates \
         managed requirements; allowed set [fast_mode=false]"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    assert_eq!(
        format_config_error(&err),
        "config/batchWrite failed in TUI: config/batchWrite failed: Invalid configuration: \
         features.fast_mode=true violates managed requirements; allowed set [fast_mode=false]"
    );
}
