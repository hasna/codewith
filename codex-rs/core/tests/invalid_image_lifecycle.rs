#![cfg(not(debug_assertions))]

use std::sync::Arc;
use std::sync::Mutex;

use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnErrorInput;
use codex_extension_api::TurnLifecycleContributor;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use wiremock::ResponseTemplate;

#[derive(Default)]
struct RecordingTurnErrors {
    errors: Mutex<Vec<(CodexErrorInfo, String)>>,
}

impl RecordingTurnErrors {
    fn lock_errors(&self) -> std::sync::MutexGuard<'_, Vec<(CodexErrorInfo, String)>> {
        match self.errors.lock() {
            Ok(errors) => errors,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[async_trait::async_trait]
impl TurnLifecycleContributor for RecordingTurnErrors {
    async fn on_turn_error_with_fingerprint(
        &self,
        input: TurnErrorInput<'_>,
        error_fingerprint: &str,
    ) {
        self.lock_errors()
            .push((input.error, error_fingerprint.to_string()));
    }
}

fn disabled_user_turn(test: &TestCodex, items: Vec<UserInput>, model: String) -> Op {
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.config.cwd.as_path());
    Op::UserInput {
        items,
        environments: None,
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
            cwd: Some(test.config.cwd.clone()),
            approval_policy: Some(AskForApproval::Never),
            sandbox_policy: Some(sandbox_policy),
            permission_profile,
            collaboration_mode: Some(codex_protocol::config_types::CollaborationMode {
                mode: codex_protocol::config_types::ModeKind::Default,
                settings: codex_protocol::config_types::Settings {
                    model,
                    reasoning_effort: None,
                    developer_instructions: None,
                },
            }),
            ..Default::default()
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_image_run_turn_preserves_bad_request_and_stable_fingerprint() -> anyhow::Result<()>
{
    let server = start_mock_server().await;
    const INVALID_IMAGE_ERROR: &str =
        "The image data you provided does not represent a valid image";
    responses::mount_response_once(
        &server,
        ResponseTemplate::new(400)
            .insert_header("content-type", "text/plain")
            .set_body_string(INVALID_IMAGE_ERROR),
    )
    .await;

    let recorder = Arc::new(RecordingTurnErrors::default());
    let mut extensions = ExtensionRegistryBuilder::<codex_core::config::Config>::new();
    extensions.turn_lifecycle_contributor(recorder.clone());
    let mut builder = test_codex().with_extensions(Arc::new(extensions.build()));
    let test = builder.build(&server).await?;
    let session_model = test.session_configured.model.clone();

    test.codex
        .submit(disabled_user_turn(
            &test,
            vec![UserInput::Text {
                text: "trigger invalid image response mapping".into(),
                text_elements: Vec::new(),
            }],
            session_model,
        ))
        .await?;

    let event = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(error) = event else {
        unreachable!("wait predicate only accepts error events");
    };
    assert_eq!(Some(CodexErrorInfo::BadRequest), error.codex_error_info);
    assert_eq!(
        vec![(
            CodexErrorInfo::BadRequest,
            "codex_err:invalid_image_request".to_string(),
        )],
        *recorder.lock_errors()
    );

    Ok(())
}
