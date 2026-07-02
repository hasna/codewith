use std::sync::Arc;

use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use codex_core::config::AuthProfileAutoSwitchStrategy;
use codex_login::AuthDotJson;
use codex_login::CodexAuth;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use wiremock::ResponseTemplate;

fn api_key_auth(api_key: &str) -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(api_key.to_string()),
        tokens: None,
        last_refresh: None,
        agent_identity: None,
        personal_access_token: None,
    }
}

fn usage_limit_response() -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("x-codex-primary-used-percent", "100.0")
        .insert_header("x-codex-primary-window-minutes", "300")
        .set_body_json(json!({
            "error": {
                "type": "usage_limit_reached",
                "message": "limit reached",
                "resets_at": 1704067242,
                "plan_type": "pro"
            }
        }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_profile_auto_switch_retries_until_profiles_are_exhausted_or_one_succeeds()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let codex_home = Arc::new(TempDir::new()?);

    for profile in ["account001", "account002", "account003"] {
        codex_login::save_auth_profile(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            profile,
            &api_key_auth(&format!("test-key-{profile}")),
        )?;
    }

    let request_log = mount_response_sequence(
        &server,
        vec![
            usage_limit_response(),
            usage_limit_response(),
            sse_response(sse(vec![
                ev_response_created("resp-success"),
                ev_completed("resp-success"),
            ])),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_home(Arc::clone(&codex_home))
        .with_auth(CodexAuth::from_api_key("test-key-root"))
        .with_config(|config| {
            config.selected_auth_profile = Some("account001".to_string());
            config.auth_profile_auto_switch.enabled = true;
            config.auth_profile_auto_switch.strategy = AuthProfileAutoSwitchStrategy::Ordered;
            config.auth_profile_auto_switch.profiles = vec![
                "account001".to_string(),
                "account002".to_string(),
                "account003".to_string(),
            ];
        });
    let test = builder.build(&server).await?;

    test.codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "run a small loop task".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(3, request_log.requests().len());
    assert_eq!(
        Some("account003".to_string()),
        test.codex.config_snapshot().await.selected_auth_profile
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_profile_auto_switch_stops_after_all_profiles_are_exhausted() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let codex_home = Arc::new(TempDir::new()?);

    for profile in ["account001", "account002", "account003"] {
        codex_login::save_auth_profile(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            profile,
            &api_key_auth(&format!("test-key-{profile}")),
        )?;
    }

    let request_log = mount_response_sequence(
        &server,
        vec![
            usage_limit_response(),
            usage_limit_response(),
            usage_limit_response(),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_home(Arc::clone(&codex_home))
        .with_auth(CodexAuth::from_api_key("test-key-root"))
        .with_config(|config| {
            config.selected_auth_profile = Some("account001".to_string());
            config.auth_profile_auto_switch.enabled = true;
            config.auth_profile_auto_switch.strategy = AuthProfileAutoSwitchStrategy::Ordered;
            config.auth_profile_auto_switch.profiles = vec![
                "account001".to_string(),
                "account002".to_string(),
                "account003".to_string(),
            ];
        });
    let test = builder.build(&server).await?;

    test.codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "run a small loop task".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;

    assert_eq!(3, request_log.requests().len());
    assert!(error.message.to_ascii_lowercase().contains("usage limit"));
    assert_eq!(
        Some("account003".to_string()),
        test.codex.config_snapshot().await.selected_auth_profile
    );

    Ok(())
}
