use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::Weak;

use codex_exec_server::LOCAL_ENVIRONMENT_ID;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolCallOutcome;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolLifecycleFuture;
use codex_extension_api::ToolWorktreeMutationSignal;
use codex_goal_extension::GoalExtensionConfig;
use codex_goal_extension::GoalService;
use codex_goal_extension::install_with_backend;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TurnEnvironmentSelection;
use core_test_support::responses::ev_apply_patch_custom_tool_call;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::ev_shell_command_call;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;

#[derive(Default)]
struct RecordingToolSignals {
    signals: Mutex<Vec<(String, ToolCallOutcome, ToolWorktreeMutationSignal)>>,
}

impl RecordingToolSignals {
    fn signals(&self) -> Vec<(String, ToolCallOutcome, ToolWorktreeMutationSignal)> {
        self.signals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ToolLifecycleContributor for RecordingToolSignals {
    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            self.signals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    input.tool_name.name.clone(),
                    input.outcome,
                    input.worktree_mutation_signal,
                ));
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_turn_dispatches_mutation_signals_and_persists_final_line_changes()
-> anyhow::Result<()> {
    let server = start_mock_server().await;
    let home = Arc::new(tempfile::tempdir()?);
    let state_runtime =
        codex_state::StateRuntime::init(home.path().to_path_buf(), "test-provider".to_string())
            .await?;
    let recorder = Arc::new(RecordingToolSignals::default());
    let mut extensions =
        ExtensionRegistryBuilder::<codex_core::config::Config>::new();
    extensions.tool_lifecycle_contributor(recorder.clone());
    install_with_backend(
        &mut extensions,
        state_runtime.clone(),
        /*metrics_client*/ None,
        Weak::new(),
        Arc::new(GoalService::new()),
        |_config: &codex_core::config::Config| GoalExtensionConfig {
            enabled: true,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
            max_auto_goals_per_plan: 48,
            max_tokens_per_goal_plan: None,
            max_goal_plan_node_objective_chars: 4_000,
            post_goal_context: codex_state::PostGoalContextAction::Keep,
            post_goal_plan_context: codex_state::PostGoalContextAction::Keep,
        },
    );
    let mut builder = test_codex()
        .with_home(home)
        .with_extensions(Arc::new(extensions.build()));
    let test = builder.build(&server).await?;

    run_git(test.cwd_path(), &["init"]).await?;
    std::fs::write(test.workspace_path("tracked.txt"), "baseline\n")?;
    run_git(test.cwd_path(), &["add", "."]).await?;
    run_git(
        test.cwd_path(),
        &[
            "-c",
            "user.name=Codewith Test",
            "-c",
            "user.email=codewith@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-m",
            "initial",
        ],
    )
    .await?;

    let thread_id = test.session_configured.thread_id;
    let rollout_path = test
        .codex
        .rollout_path()
        .unwrap_or_else(|| {
            test.config
                .codex_home
                .join(format!("rollout-{thread_id}.jsonl"))
                .to_path_buf()
        });
    let mut metadata = codex_state::ThreadMetadataBuilder::new(
        thread_id,
        rollout_path,
        chrono::Utc::now(),
        SessionSource::Exec,
    );
    metadata.cwd = test.cwd_path().to_path_buf();
    state_runtime
        .upsert_thread(&metadata.build("test-provider"))
        .await?;
    state_runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "account real turn line changes",
            codex_state::ThreadGoalStatus::Active,
            /*token_budget*/ None,
        )
        .await?;

    let patch =
        "*** Begin Patch\n*** Add File: accounted.txt\n+accounted\n*** End Patch";
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-read"),
                ev_shell_command_call("call-read", "cat tracked.txt"),
                ev_completed("resp-read"),
            ]),
            sse(vec![
                ev_response_created("resp-patch"),
                ev_apply_patch_custom_tool_call("call-patch", patch),
                ev_completed("resp-patch"),
            ]),
            sse(vec![
                ev_response_created("resp-done"),
                ev_assistant_message("msg-done", "done"),
                ev_completed("resp-done"),
            ]),
        ],
    )
    .await;

    test.submit_turn_with_environments(
        "read the tracked file, then add the accounted file",
        Some(vec![TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: test.config.cwd.clone(),
        }]),
    )
    .await?;

    assert_eq!(
        vec![
            (
                "shell_command".to_string(),
                ToolCallOutcome::Completed { success: true },
                ToolWorktreeMutationSignal::MaybeMutatesWorktree,
            ),
            (
                "apply_patch".to_string(),
                ToolCallOutcome::Completed { success: true },
                ToolWorktreeMutationSignal::ConfirmedWorktreeMutation,
            ),
        ],
        recorder.signals()
    );
    assert_eq!("accounted\n", std::fs::read_to_string(test.workspace_path("accounted.txt"))?);
    let goal = state_runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(1, goal.lines_added);
    assert_eq!(0, goal.lines_deleted);
    Ok(())
}

async fn run_git(cwd: &std::path::Path, args: &[&str]) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("git")
        .current_dir(cwd)
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(args)
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
