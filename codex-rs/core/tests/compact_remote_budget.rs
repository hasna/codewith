#![allow(clippy::expect_used)]

use anyhow::Result;
use codex_features::Feature;
use codex_login::CodexAuth;
use codex_protocol::config_types::ModelProviderAuthInfo;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use tempfile::TempDir;
use wiremock::ResponseTemplate;

const MAX_REMOTE_COMPACTION_REQUESTS: usize = 4;

struct ProviderAuthCommandFixture {
    tempdir: TempDir,
    command: String,
    args: Vec<String>,
}

impl ProviderAuthCommandFixture {
    fn new(tokens: &[&str]) -> std::io::Result<Self> {
        let tempdir = tempfile::tempdir()?;
        let tokens_file = tempdir.path().join("tokens.txt");
        let mut token_file_contents = tokens.join("\n");
        token_file_contents.push('\n');
        std::fs::write(tokens_file, token_file_contents)?;

        #[cfg(unix)]
        let (command, args) = {
            use std::os::unix::fs::PermissionsExt;

            let script_path = tempdir.path().join("print-token.sh");
            std::fs::write(
                &script_path,
                r#"#!/bin/sh
first_line=$(sed -n '1p' tokens.txt)
printf '%s\n' "$first_line"
tail -n +2 tokens.txt > tokens.next
mv tokens.next tokens.txt
"#,
            )?;
            let mut permissions = std::fs::metadata(&script_path)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions)?;
            ("./print-token.sh".to_string(), Vec::new())
        };

        #[cfg(windows)]
        let (command, args) = {
            let script_path = tempdir.path().join("print-token.cmd");
            std::fs::write(
                &script_path,
                r#"@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "first_line="
<tokens.txt set /p first_line=
if not defined first_line exit /b 1

echo(%first_line%
more +1 tokens.txt > tokens.next
move /y tokens.next tokens.txt >nul
"#,
            )?;
            (
                "cmd.exe".to_string(),
                vec![
                    "/D".to_string(),
                    "/Q".to_string(),
                    "/C".to_string(),
                    ".\\print-token.cmd".to_string(),
                ],
            )
        };

        Ok(Self {
            tempdir,
            command,
            args,
        })
    }

    fn auth(&self) -> ModelProviderAuthInfo {
        ModelProviderAuthInfo {
            command: self.command.clone(),
            args: self.args.clone(),
            timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
            refresh_interval_ms: 60_000,
            cwd: AbsolutePathBuf::try_from(self.tempdir.path())
                .expect("tempdir path should be absolute"),
        }
    }
}

fn long_injected_history(item_count: usize) -> Vec<ResponseItem> {
    (0..item_count)
        .map(|index| ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("INJECTED_HISTORY_{index:03}"),
            }],
            phase: None,
        })
        .collect()
}

fn semantic_injected_history(item_count: usize) -> Vec<ResponseItem> {
    (0..item_count)
        .map(|index| ResponseItem::Message {
            id: None,
            role: if index % 2 == 0 {
                "user".to_string()
            } else {
                "assistant".to_string()
            },
            content: vec![if index % 2 == 0 {
                ContentItem::InputText {
                    text: format!("SEMANTIC_HISTORY_{index:03}"),
                }
            } else {
                ContentItem::OutputText {
                    text: format!("SEMANTIC_HISTORY_{index:03}"),
                }
            }],
            phase: None,
        })
        .collect()
}

fn context_window_exceeded_response() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(serde_json::json!({
        "error": {
            "code": "context_length_exceeded",
            "message": "Your input exceeds the context window of this model. Please adjust your input and try again."
        }
    }))
}

async fn wait_for_turn_complete(codex: &codex_core::CodexThread) {
    wait_for_event(codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
}

async fn submit_follow_up(codex: &codex_core::CodexThread, text: &str) -> Result<()> {
    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: text.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_turn_complete(codex).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantic_history_overflow_fails_without_mutating_history() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(|config| {
                let _ = config.features.enable(Feature::RemoteCompactionV2);
                config.model_provider.request_max_retries = Some(0);
                config.model_provider.stream_max_retries = Some(0);
            }),
    )
    .await?;
    let codex = harness.test().codex.clone();
    codex
        .inject_response_items(semantic_injected_history(8))
        .await?;

    let compact_mock = responses::mount_v2_compaction_response_sequence_up_to(
        harness.server(),
        vec![
            context_window_exceeded_response(),
            responses::sse_response(responses::sse(vec![
                serde_json::json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "compaction",
                        "encrypted_content": "UNSAFE_SUFFIX_ONLY_SUMMARY",
                    }
                }),
                responses::ev_completed("resp-unsafe-suffix-only"),
            ])),
        ],
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;

    assert!(error_message.to_lowercase().contains("context window"));
    assert_eq!(compact_mock.requests().len(), 1);

    let follow_up_mock = responses::mount_sse_once(
        harness.server(),
        responses::sse(vec![
            responses::ev_assistant_message("m-after-semantic-overflow", "HISTORY_PRESERVED_REPLY"),
            responses::ev_completed("resp-after-semantic-overflow"),
        ]),
    )
    .await;
    submit_follow_up(&codex, "after semantic overflow").await?;
    let follow_up_body = follow_up_mock.single_request().body_json().to_string();
    assert!(follow_up_body.contains("SEMANTIC_HISTORY_000"));
    assert!(follow_up_body.contains("SEMANTIC_HISTORY_007"));
    assert!(!follow_up_body.contains("UNSAFE_SUFFIX_ONLY_SUMMARY"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn irreducible_semantic_item_overflow_fails_without_mutating_history() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(|config| {
                let _ = config.features.enable(Feature::RemoteCompactionV2);
                config.model_provider.request_max_retries = Some(0);
                config.model_provider.stream_max_retries = Some(0);
            }),
    )
    .await?;
    let codex = harness.test().codex.clone();
    let irreducible_sentinel = format!("IRREDUCIBLE_SEMANTIC_ITEM_{}", "x".repeat(128_000));
    codex
        .inject_response_items(vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: irreducible_sentinel.clone(),
            }],
            phase: None,
        }])
        .await?;

    let compact_mock = responses::mount_v2_compaction_response_sequence_up_to(
        harness.server(),
        vec![
            context_window_exceeded_response(),
            responses::sse_response(responses::sse(vec![
                serde_json::json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "compaction",
                        "encrypted_content": "UNSAFE_IRREDUCIBLE_SUMMARY",
                    }
                }),
                responses::ev_completed("resp-unsafe-irreducible"),
            ])),
        ],
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;

    assert!(error_message.to_lowercase().contains("context window"));
    assert_eq!(compact_mock.requests().len(), 1);

    let follow_up_mock = responses::mount_sse_once(
        harness.server(),
        responses::sse(vec![
            responses::ev_assistant_message("m-after-irreducible", "HISTORY_PRESERVED_REPLY"),
            responses::ev_completed("resp-after-irreducible"),
        ]),
    )
    .await;
    submit_follow_up(&codex, "after irreducible overflow").await?;
    let follow_up_body = follow_up_mock.single_request().body_json().to_string();
    assert!(follow_up_body.contains(&irreducible_sentinel));
    assert!(!follow_up_body.contains("UNSAFE_IRREDUCIBLE_SUMMARY"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_base_and_tool_overhead_fails_without_mutating_history() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let oversized_base = format!("OVERSIZED_BASE_INSTRUCTIONS_{}", "b".repeat(128_000));
    let oversized_tool_description = format!("OVERSIZED_TOOL_DESCRIPTION_{}", "t".repeat(128_000));
    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config({
            let oversized_base = oversized_base.clone();
            move |config| {
                let _ = config.features.enable(Feature::RemoteCompactionV2);
                config.base_instructions = Some(oversized_base);
                config.model_provider.request_max_retries = Some(0);
                config.model_provider.stream_max_retries = Some(0);
            }
        });
    let mut test = builder.build(&server).await?;
    let new_thread = test
        .thread_manager
        .start_thread_with_tools(
            test.config.clone(),
            vec![DynamicToolSpec {
                namespace: Some("codex_app".to_string()),
                name: "oversized_invariant_tool".to_string(),
                description: oversized_tool_description,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": {"type": "string"},
                    },
                    "additionalProperties": false,
                }),
                defer_loading: false,
            }],
        )
        .await?;
    test.codex = new_thread.thread;
    test.session_configured = new_thread.session_configured;
    let codex = test.codex.clone();
    codex
        .inject_response_items(semantic_injected_history(4))
        .await?;

    let compact_mock = responses::mount_v2_compaction_response_sequence_up_to(
        &server,
        vec![
            context_window_exceeded_response(),
            responses::sse_response(responses::sse(vec![
                serde_json::json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "compaction",
                        "encrypted_content": "UNSAFE_OVERHEAD_SUMMARY",
                    }
                }),
                responses::ev_completed("resp-unsafe-overhead"),
            ])),
        ],
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;

    assert!(error_message.to_lowercase().contains("context window"));
    let compact_request = compact_mock.single_request();
    let compact_body = compact_request.body_json().to_string();
    assert!(
        compact_request
            .instructions_text()
            .contains("OVERSIZED_BASE_INSTRUCTIONS")
    );
    assert!(compact_body.contains("OVERSIZED_TOOL_DESCRIPTION"));

    let follow_up_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_assistant_message("m-after-overhead", "HISTORY_PRESERVED_REPLY"),
            responses::ev_completed("resp-after-overhead"),
        ]),
    )
    .await;
    submit_follow_up(&codex, "after invariant overhead overflow").await?;
    let follow_up_body = follow_up_mock.single_request().body_json().to_string();
    assert!(follow_up_body.contains("SEMANTIC_HISTORY_000"));
    assert!(follow_up_body.contains("SEMANTIC_HISTORY_003"));
    assert!(!follow_up_body.contains("UNSAFE_OVERHEAD_SUMMARY"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_mixed_stream_and_overflow_retries_share_request_budget() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(|config| {
                let _ = config.features.enable(Feature::RemoteCompactionV2);
                config.model_provider.stream_max_retries = Some(2);
            }),
    )
    .await?;
    let codex = harness.test().codex.clone();
    codex
        .inject_response_items(long_injected_history(MAX_REMOTE_COMPACTION_REQUESTS * 8))
        .await?;

    let responses_mock = responses::mount_v2_compaction_response_sequence_up_to(
        harness.server(),
        vec![
            ResponseTemplate::new(500).set_body_string("compact open failed"),
            responses::sse_response(responses::sse(vec![serde_json::json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "compaction",
                    "encrypted_content": "INCOMPLETE_COMPACT_SUMMARY",
                }
            })])),
            context_window_exceeded_response(),
            context_window_exceeded_response(),
            context_window_exceeded_response(),
            context_window_exceeded_response(),
        ],
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;

    assert!(
        error_message.to_lowercase().contains("context window"),
        "unexpected compaction error: {error_message}"
    );
    assert_eq!(
        responses_mock.requests().len(),
        MAX_REMOTE_COMPACTION_REQUESTS - 1
    );

    let follow_up_mock = responses::mount_sse_once(
        harness.server(),
        responses::sse(vec![
            responses::ev_assistant_message("m-after-mixed-v2", "HISTORY_PRESERVED_REPLY"),
            responses::ev_completed("resp-after-mixed-v2"),
        ]),
    )
    .await;
    submit_follow_up(&codex, "after mixed v2 compaction failure").await?;
    assert!(
        follow_up_mock
            .single_request()
            .body_json()
            .to_string()
            .contains("INJECTED_HISTORY_000")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_auth_recovery_provider_and_overflow_retries_share_request_budget() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let auth_fixture = ProviderAuthCommandFixture::new(&["test-token"; 16])?;
    let provider_auth = auth_fixture.auth();
    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(move |config| {
                let _ = config.features.enable(Feature::RemoteCompactionV2);
                config.model_provider.auth = Some(provider_auth);
                config.model_provider.request_max_retries = Some(2);
                config.model_provider.stream_max_retries = Some(0);
            }),
    )
    .await?;
    let codex = harness.test().codex.clone();
    codex
        .inject_response_items(long_injected_history(MAX_REMOTE_COMPACTION_REQUESTS * 8))
        .await?;

    let mut response_sequence = vec![
        ResponseTemplate::new(401).set_body_string("expired provider auth"),
        ResponseTemplate::new(500).set_body_string("first provider retry"),
        ResponseTemplate::new(500).set_body_string("second provider retry"),
    ];
    response_sequence.extend(
        (0..MAX_REMOTE_COMPACTION_REQUESTS * 2).map(|_| context_window_exceeded_response()),
    );
    let responses_mock =
        responses::mount_v2_compaction_response_sequence_up_to(harness.server(), response_sequence)
            .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;

    assert!(
        error_message.to_lowercase().contains("context window"),
        "unexpected compaction error: {error_message}"
    );
    assert_eq!(
        responses_mock.requests().len(),
        MAX_REMOTE_COMPACTION_REQUESTS
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_mixed_stream_and_overflow_failure_preserves_history() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(|config| {
                let _ = config.features.enable(Feature::RemoteCompactionV2);
                config.model_provider.request_max_retries = Some(0);
                config.model_provider.stream_max_retries = Some(2);
            }),
    )
    .await?;
    let codex = harness.test().codex.clone();
    codex
        .inject_response_items(long_injected_history(/*item_count*/ 12))
        .await?;

    let responses_mock = responses::mount_response_sequence(
        harness.server(),
        vec![
            ResponseTemplate::new(500).set_body_string("compact open failed"),
            context_window_exceeded_response(),
            responses::sse_response(responses::sse(vec![
                responses::ev_assistant_message("m-after-mixed-v2", "AFTER_RECOVERY_REPLY"),
                responses::ev_completed("resp-after-mixed-v2"),
            ])),
        ],
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;
    submit_follow_up(&codex, "after mixed v2 recovery").await?;

    let requests = responses_mock.requests();
    assert_eq!(requests.len(), MAX_REMOTE_COMPACTION_REQUESTS - 1);
    assert!(error_message.to_lowercase().contains("context window"));
    let follow_up_body = requests[2].body_json().to_string();
    assert!(follow_up_body.contains("INJECTED_HISTORY_000"));
    assert!(!follow_up_body.contains("MIXED_V2_RECOVERY_SUMMARY"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_transport_and_overflow_retries_share_request_budget() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(|config| config.model_provider.request_max_retries = Some(2)),
    )
    .await?;
    let codex = harness.test().codex.clone();
    codex
        .inject_response_items(long_injected_history(MAX_REMOTE_COMPACTION_REQUESTS * 8))
        .await?;

    let compact_mock = responses::mount_compact_response_sequence_up_to(
        harness.server(),
        vec![
            ResponseTemplate::new(500).set_body_string("first compact attempt failed"),
            ResponseTemplate::new(500).set_body_string("second compact attempt failed"),
            context_window_exceeded_response(),
            context_window_exceeded_response(),
            context_window_exceeded_response(),
            context_window_exceeded_response(),
        ],
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;

    assert!(error_message.to_lowercase().contains("context window"));
    assert_eq!(
        compact_mock.requests().len(),
        MAX_REMOTE_COMPACTION_REQUESTS - 1
    );

    let follow_up_mock = responses::mount_sse_once(
        harness.server(),
        responses::sse(vec![
            responses::ev_assistant_message("m-after-mixed-v1", "HISTORY_PRESERVED_REPLY"),
            responses::ev_completed("resp-after-mixed-v1"),
        ]),
    )
    .await;
    submit_follow_up(&codex, "after mixed v1 compaction failure").await?;
    assert!(
        follow_up_mock
            .single_request()
            .body_json()
            .to_string()
            .contains("INJECTED_HISTORY_000")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_transport_and_overflow_failure_preserves_history() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = TestCodexHarness::with_builder(
        test_codex()
            .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
            .with_config(|config| config.model_provider.request_max_retries = Some(2)),
    )
    .await?;
    let codex = harness.test().codex.clone();
    codex
        .inject_response_items(long_injected_history(/*item_count*/ 12))
        .await?;

    let compact_mock = responses::mount_compact_response_sequence(
        harness.server(),
        vec![
            ResponseTemplate::new(500).set_body_string("compact transport failed"),
            context_window_exceeded_response(),
        ],
    )
    .await;
    let follow_up_mock = responses::mount_sse_once(
        harness.server(),
        responses::sse(vec![
            responses::ev_assistant_message("m-after-mixed-v1", "AFTER_RECOVERY_REPLY"),
            responses::ev_completed("resp-after-mixed-v1"),
        ]),
    )
    .await;

    codex.submit(Op::Compact).await?;
    let error_message = wait_for_event_match(&codex, |event| match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        _ => None,
    })
    .await;
    wait_for_turn_complete(&codex).await;
    submit_follow_up(&codex, "after mixed v1 recovery").await?;

    let compact_requests = compact_mock.requests();
    assert_eq!(compact_requests.len(), 2);
    assert!(error_message.to_lowercase().contains("context window"));
    let follow_up_body = follow_up_mock.single_request().body_json().to_string();
    assert!(follow_up_body.contains("INJECTED_HISTORY_000"));
    assert!(!follow_up_body.contains("MIXED_V1_RECOVERY_SUMMARY"));

    Ok(())
}
