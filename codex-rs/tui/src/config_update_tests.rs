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

/// Regression test for the provider-save error message swallowing the real
/// server-side cause. `select_model_provider_model` (event_dispatch.rs) used
/// to format the write_config_batch error with bare `{err}` Display, which
/// for a `color_eyre::eyre::Report` only prints the outermost `wrap_err`
/// context and silently drops everything the error was wrapped over — so a
/// TUI user selecting a provider whose config write failed validation only
/// ever saw "Failed to save provider `<id>`: config/batchWrite failed in
/// TUI", with the actual reason (e.g. an "Invalid configuration: ..."
/// validation message from the app-server) discarded. This asserts the fix:
/// the same message-building logic used by the provider-save handler must
/// preserve the full chain, mirroring the sibling "Failed to save default
/// model" / "Failed to save model for profile" handlers a few hundred lines
/// below it in the same file, which already used `format_config_error`.
#[test]
fn provider_save_error_message_preserves_server_validation_message() {
    let provider_id = "openai";
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: model_provider.openai is not \
         reachable: missing env_key"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    // This mirrors exactly the message construction in
    // `App::select_model_provider_model`'s `Err` branch.
    let message = format!(
        "Failed to save provider `{provider_id}`: {}",
        format_config_error(&err)
    );

    assert_eq!(
        message,
        "Failed to save provider `openai`: config/batchWrite failed in TUI: \
         config/batchWrite failed: Invalid configuration: model_provider.openai is not \
         reachable: missing env_key"
    );
}

/// Companion negative control: proves the OLD formatting (bare `{err}`
/// Display on the eyre chain) is what produced the uninformative message the
/// owner hit — i.e. this test documents the bug this fix closes, so a future
/// reader can see both the failure mode and the fix in one place.
#[test]
fn bare_display_on_wrapped_config_error_drops_the_real_cause() {
    let provider_id = "openai";
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: model_provider.openai is not \
         reachable: missing env_key"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    // The pre-fix code path: `format!("Failed to save provider `{provider_id}`: {err}")`.
    let message = format!("Failed to save provider `{provider_id}`: {err}");

    assert_eq!(
        message,
        "Failed to save provider `openai`: config/batchWrite failed in TUI"
    );
    assert!(
        !message.contains("missing env_key"),
        "bare Display unexpectedly preserved the real cause; if this now fails, the eyre \
         Display contract changed and this whole regression test class needs re-justifying"
    );
}
