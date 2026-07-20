use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::Weak;
use std::time::Duration;

use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::FunctionCallError;
use codex_extension_api::NoopTurnItemEmitter;
use codex_extension_api::ThreadResumeInput;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ThreadStopInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolCallOutcome;
use codex_extension_api::ToolCallSource;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolPayload;
use codex_extension_api::ToolSpec;
use codex_extension_api::TurnErrorInput;
use codex_extension_api::TurnStartInput;
use codex_extension_api::TurnStopInput;
use codex_goal_extension::GoalExtensionConfig;
use codex_goal_extension::GoalObjectiveUpdate;
use codex_goal_extension::GoalRuntimeHandle;
use codex_goal_extension::GoalService;
use codex_goal_extension::GoalSetRequest;
use codex_goal_extension::GoalTitleUpdate;
use codex_goal_extension::GoalTokenBudgetUpdate;
use codex_goal_extension::install_with_backend;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadGoalStatus;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageInfo;
use codex_protocol::protocol::TruncationPolicy;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn installed_goal_tools_create_goal_and_fill_empty_preview() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime.clone(), thread_id).await;

    let create_tool = tool_by_name(&tools, "create_goal");
    let invocation = tool_call(
        "create_goal",
        "call-create-goal",
        json!({
            "objective": "ship goal extension backend",
            "token_budget": 123,
        }),
    );
    let output = create_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(
        result,
        json!({
            "goal": {
                "goalId": result["goal"]["goalId"],
                "threadId": thread_id,
                "objective": "ship goal extension backend",
                "title": "ship goal extension backend",
                "status": "active",
                "tokenBudget": 123,
                "tokensUsed": 0,
                "timeUsedSeconds": 0,
                "createdAt": result["goal"]["createdAt"],
                "updatedAt": result["goal"]["updatedAt"],
            },
            "remainingTokens": 123,
            "completionBudgetReport": serde_json::Value::Null,
            "goalPlanCompletionReport": serde_json::Value::Null,
        })
    );

    let metadata = runtime
        .get_thread(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("seeded thread metadata should exist"))?;
    assert_eq!(
        metadata.preview.as_deref(),
        Some("ship goal extension backend")
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_rejects_blank_explicit_title() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime.clone(), thread_id).await;

    let create_tool = tool_by_name(&tools, "create_goal");
    let err = match create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "ship goal extension backend",
                "title": "   ",
            }),
        ))
        .await
    {
        Ok(_) => panic!("blank goal title should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        FunctionCallError::RespondToModel("goal title must not be empty".to_string())
    );
    assert_eq!(
        None,
        runtime.thread_goals().get_thread_goal(thread_id).await?
    );
    Ok(())
}

#[tokio::test]
async fn installed_goal_tools_include_resume_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime, thread_id).await;

    assert_eq!(
        vec![
            "get_goal".to_string(),
            "get_goal_plan".to_string(),
            "create_goal".to_string(),
            "create_goal_plan".to_string(),
            "activate_goal_plan_node".to_string(),
            "update_goal".to_string(),
            "resume_goal".to_string(),
        ],
        tool_names(&tools)
    );
    Ok(())
}

#[tokio::test]
async fn resume_goal_reactivates_deferred_node_after_independent_node_completes()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    // Plan: preservation (independent) is worked first, consolidate is an
    // independent node, and downstream depends on preservation.
    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-create-goal-plan",
            json!({
                "goals": [
                    { "key": "preservation", "objective": "Preserve prior work" },
                    { "key": "consolidate", "objective": "Consolidate independent results" },
                    {
                        "key": "downstream",
                        "objective": "Finish downstream work",
                        "depends_on": ["preservation"]
                    }
                ]
            }),
        ))
        .await?;

    // Defer the active preservation node; only the independent consolidate node
    // is ready, so it activates automatically.
    let update_tool = tool_by_name(&tools, "update_goal");
    let defer = tool_call(
        "update_goal",
        "call-defer-preservation",
        json!({ "status": "deferred" }),
    );
    let output = update_tool.handle(defer.clone()).await?;
    let result = output.code_mode_result(&defer.payload);
    assert_eq!(result["goal"]["status"], "deferred");
    assert_eq!(
        result["activatedGoal"]["objective"],
        "Consolidate independent results"
    );

    // Complete the independent consolidate node. Downstream still depends on the
    // deferred preservation node, so nothing new activates and the plan stalls.
    let complete_consolidate = tool_call(
        "update_goal",
        "call-complete-consolidate",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(complete_consolidate.clone()).await?;
    let result = output.code_mode_result(&complete_consolidate.payload);
    assert_eq!(
        result["goal"]["objective"],
        "Consolidate independent results"
    );
    assert_eq!(result["goal"]["status"], "complete");
    assert_eq!(result["activatedGoal"], serde_json::Value::Null);

    // Explicit user resume: with no directly resumable current goal, resume_goal
    // revives the deferred preservation node instead of failing.
    let resume_tool = tool_by_name(&tools, "resume_goal");
    let resume = tool_call("resume_goal", "call-resume-goal", json!({}));
    let output = resume_tool.handle(resume.clone()).await?;
    let result = output.code_mode_result(&resume.payload);
    assert_eq!(result["goal"]["objective"], "Preserve prior work");
    assert_eq!(result["goal"]["status"], "active");
    assert_eq!(result["activatedGoal"]["objective"], "Preserve prior work");
    let node_statuses = result["goalPlans"][0]["nodes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("goal plan nodes should be an array"))?
        .iter()
        .map(|node| node["status"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(vec!["active", "complete", "pending"], node_statuses);

    // Completing the resumed node now satisfies the downstream dependency, which
    // activates without replacing plan history.
    let complete_preservation = tool_call(
        "update_goal",
        "call-complete-preservation",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(complete_preservation.clone()).await?;
    let result = output.code_mode_result(&complete_preservation.payload);
    assert_eq!(result["goal"]["objective"], "Preserve prior work");
    assert_eq!(
        result["activatedGoal"]["objective"],
        "Finish downstream work"
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_activates_first_goal_and_returns_plan() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime, thread_id).await;

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "goals": [
                {
                    "key": "investigate",
                    "objective": "Investigate chained goals"
                },
                {
                    "key": "implement",
                    "objective": "Implement chained goals",
                    "depends_on": ["investigate"],
                    "token_budget": 50000
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(result["goal"]["objective"], "Investigate chained goals");
    assert_eq!(
        result["activatedGoal"]["objective"],
        "Investigate chained goals"
    );
    assert_eq!(result["goal"]["tokenBudget"], serde_json::Value::Null);
    assert_eq!(result["goalPlans"][0]["autoExecute"], "ai_directed");
    assert_eq!(result["goalPlans"][0]["nodes"][0]["status"], "active");
    assert_eq!(
        result["goalPlans"][0]["nodes"][0]["tokenBudget"],
        serde_json::Value::Null
    );
    assert_eq!(
        result["goalPlans"][0]["nodes"][1]["dependsOn"][0],
        "investigate"
    );
    assert_eq!(result["remainingTokens"], serde_json::Value::Null);
    Ok(())
}

#[tokio::test]
async fn create_goal_tools_persist_context_lifecycle_actions() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "compact after standalone goal",
                "post_goal_context": "compact",
            }),
        ))
        .await?;
    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("created goal should exist"))?;
    assert_eq!(
        Some(codex_state::PostGoalContextAction::Compact),
        runtime
            .thread_goals()
            .thread_goal_context_action(thread_id, goal.goal_id.as_str())
            .await?
    );

    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-complete-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(
        result["contextLifecycleReport"],
        "Scheduled native context compaction after the thread becomes idle."
    );

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "clear_existing_goal": true,
            "post_goal_context": "compact",
            "post_goal_plan_context": "compact",
            "goals": [
                {
                    "key": "planned",
                    "objective": "compact after planned goal"
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    let plan_id = result["goalPlans"][0]["planId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("created plan id should be returned"))?;
    assert_eq!(
        Some(codex_state::PostGoalContextAction::Compact),
        runtime
            .thread_goals()
            .thread_goal_plan_context_action(thread_id, plan_id)
            .await?
    );
    assert_eq!(
        Some(codex_state::PostGoalContextAction::Compact),
        runtime
            .thread_goals()
            .thread_goal_plan_completion_context_action(thread_id, plan_id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_appends_followup_nodes_to_active_plan() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime, thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "goals": [
                {
                    "key": "first",
                    "objective": "Run the first goal"
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    let plan_id = result["goalPlans"][0]["planId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("plan id should be returned"))?
        .to_string();

    let invocation = tool_call(
        "create_goal_plan",
        "call-append-goal-plan",
        json!({
            "append_to_plan_id": plan_id,
            "goals": [
                {
                    "key": "second",
                    "objective": "Run the appended follow-up goal",
                    "depends_on": ["first"],
                    "token_budget": 1000
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(result["goal"]["objective"], "Run the first goal");
    assert_eq!(result["activatedGoal"], serde_json::Value::Null);
    assert_eq!(result["goalPlans"][0]["nodeCount"], 2);
    assert_eq!(result["goalPlans"][0]["nodes"][0]["status"], "active");
    assert_eq!(result["goalPlans"][0]["nodes"][1]["status"], "pending");
    assert_eq!(result["goalPlans"][0]["nodes"][1]["dependsOn"][0], "first");
    assert_eq!(result["goalPlans"][0]["nodes"][1]["tokenBudget"], 1000);

    let plan_events = harness.sink.goal_plan_events();
    assert_eq!(2, plan_events.len());
    assert_eq!(2, plan_events[1].node_count);

    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-complete-first-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(
        result["activatedGoal"]["objective"],
        "Run the appended follow-up goal"
    );
    assert_eq!(result["activatedGoal"]["tokenBudget"], 1000);
    assert_eq!(result["goalPlans"][0]["nodes"][0]["status"], "complete");
    assert_eq!(result["goalPlans"][0]["nodes"][1]["status"], "active");
    Ok(())
}

#[tokio::test]
async fn update_goal_does_not_schedule_context_lifecycle_for_blocked_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime, thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "do not compact blocked goal",
                "post_goal_context": "compact",
            }),
        ))
        .await?;

    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-block-goal",
        json!({ "status": "blocked" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(serde_json::Value::Null, result["contextLifecycleReport"]);
    Ok(())
}

#[tokio::test]
async fn goal_plan_context_lifecycle_skips_auto_advance_and_schedules_completion()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new_with_config(
        runtime,
        thread_id,
        GoalExtensionConfig {
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            ..test_goal_extension_config()
        },
    )
    .await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-create-goal-plan",
            json!({
                "post_goal_context": "compact",
                "post_goal_plan_context": "compact",
                "goals": [
                    {
                        "key": "first",
                        "objective": "complete first without compacting"
                    },
                    {
                        "key": "second",
                        "objective": "compact after final completion",
                        "depends_on": ["first"]
                    }
                ]
            }),
        ))
        .await?;

    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-complete-first-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(
        result["activatedGoal"]["objective"],
        "compact after final completion"
    );
    assert_eq!(serde_json::Value::Null, result["contextLifecycleReport"]);

    let invocation = tool_call(
        "update_goal",
        "call-complete-second-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(result["goalPlans"][0]["status"], "complete");
    assert_eq!(
        result["contextLifecycleReport"],
        "Scheduled native context compaction after the thread becomes idle."
    );
    Ok(())
}

#[tokio::test]
async fn goal_plan_context_lifecycle_schedules_when_no_next_goal_activates() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new_with_config(
        runtime,
        thread_id,
        GoalExtensionConfig {
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
            ..test_goal_extension_config()
        },
    )
    .await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "post_goal_context": "compact",
            "post_goal_plan_context": "keep",
            "goals": [
                {
                    "key": "manual",
                    "objective": "compact after manual node completion"
                },
                {
                    "key": "later",
                    "objective": "remain ready after manual node completion",
                    "depends_on": ["manual"]
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    let first_node_id = result["goalPlans"][0]["nodes"][0]["nodeId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("first node id should be returned"))?;

    let activate_tool = tool_by_name(&tools, "activate_goal_plan_node");
    activate_tool
        .handle(tool_call(
            "activate_goal_plan_node",
            "call-activate-first-node",
            json!({ "node_id": first_node_id }),
        ))
        .await?;

    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-complete-manual-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(serde_json::Value::Null, result["activatedGoal"]);
    assert_eq!(result["goalPlans"][0]["status"], "active");
    assert_eq!(
        result["contextLifecycleReport"],
        "Scheduled native context compaction after the thread becomes idle."
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_tool_response_caps_model_visible_plan_details() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new_with_config(
        runtime,
        thread_id,
        GoalExtensionConfig {
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
            max_auto_goals_per_plan: 24,
            ..test_goal_extension_config()
        },
    )
    .await?;
    let tools = harness.tools();
    let long_objective = "audit model-visible output ".repeat(80);
    let goals = (0..20)
        .map(|index| {
            json!({
                "key": format!("node-{index}"),
                "objective": format!("{long_objective}{index}")
            })
        })
        .collect::<Vec<_>>();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-large-goal-plan",
        json!({
            "goals": goals,
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    let nodes = result["goalPlans"][0]["nodes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("goal plan nodes should be an array"))?;

    assert_eq!(nodes.len(), 16);
    assert_eq!(result["goalPlans"][0]["nodeCount"], 20);
    assert_eq!(result["goalPlans"][0]["nodesOmittedCount"], 4);
    assert!(nodes[0]["objective"].as_str().unwrap_or_default().len() < long_objective.len());
    assert_eq!(nodes[0]["objectiveTruncated"], true);

    let plan_events = harness.sink.goal_plan_events();
    assert_eq!(1, plan_events.len());
    assert_eq!(
        "call-create-large-goal-plan-goal-plan",
        plan_events[0].event_id
    );
    assert_eq!(Some("turn-1".to_string()), plan_events[0].turn_id);
    assert_eq!(20, plan_events[0].node_count);
    assert_eq!(16, plan_events[0].node_objectives.len());
    assert!(
        plan_events[0]
            .node_objectives
            .iter()
            .all(|objective| objective.len() < long_objective.len())
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_rejects_model_supplied_assigned_thread_id() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime, thread_id).await;

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let err = match create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-create-delegated-goal-plan",
            json!({
                "goals": [
                    {
                        "key": "delegate",
                        "objective": "Try to delegate through the model-facing tool",
                        "assigned_thread_id": "22222222-2222-4222-8222-222222222222"
                    }
                ]
            }),
        ))
        .await
    {
        Ok(_) => panic!("model-supplied assigned_thread_id should fail"),
        Err(err) => err,
    };

    let FunctionCallError::RespondToModel(message) = err else {
        panic!("expected model-visible validation error");
    };
    assert!(message.contains("assigned_thread_id"));
    assert!(message.contains("unknown field"));
    Ok(())
}

#[tokio::test]
async fn delegated_goal_plan_response_hides_unassigned_nodes_and_includes_ready_node()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let owner_thread_id = test_thread_id()?;
    let delegate_thread_id = ThreadId::from_string("22222222-2222-4222-8222-222222222222")
        .map_err(anyhow::Error::msg)?;
    seed_thread_metadata(runtime.as_ref(), owner_thread_id).await?;
    seed_thread_metadata(runtime.as_ref(), delegate_thread_id).await?;
    let mut nodes = (0..20)
        .map(|idx| codex_state::ThreadGoalPlanNodeCreateParams {
            key: format!("owner-{idx}"),
            objective: format!("Owner-only secret objective {idx}."),
            assigned_thread_id: None,
            title: None,
            priority: 0,
            token_budget: None,
            depends_on: Vec::new(),
        })
        .collect::<Vec<_>>();
    nodes.push(codex_state::ThreadGoalPlanNodeCreateParams {
        key: "delegate".to_string(),
        objective: "Visible delegated objective.".to_string(),
        assigned_thread_id: Some(delegate_thread_id),
        title: None,
        priority: 0,
        token_budget: None,
        depends_on: Vec::new(),
    });
    let created = runtime
        .thread_goals()
        .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
            thread_id: owner_thread_id,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
            max_tokens: None,
            nodes,
        })
        .await?;
    let delegated_node_id = created
        .snapshot
        .nodes
        .iter()
        .find(|node| node.assigned_thread_id == delegate_thread_id)
        .map(|node| node.node_id.clone())
        .ok_or_else(|| anyhow::anyhow!("delegated node should exist"))?;

    let delegate_tools = installed_tools(runtime.clone(), delegate_thread_id).await;
    let get_plan_tool = tool_by_name(&delegate_tools, "get_goal_plan");
    let get_plan = tool_call("get_goal_plan", "call-get-delegate-plan", json!({}));
    let output = get_plan_tool.handle(get_plan.clone()).await?;
    let result = output.code_mode_result(&get_plan.payload);
    let plan = &result["goalPlans"][0];
    let visible_nodes = plan["nodes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("goal plan nodes should be an array"))?;
    assert_eq!(1, plan["readyNodeCount"]);
    assert_eq!(1, visible_nodes.len());
    assert_eq!(delegated_node_id, visible_nodes[0]["nodeId"]);
    assert_eq!(
        "Visible delegated objective.",
        visible_nodes[0]["objective"]
    );
    assert_eq!(
        delegate_thread_id.to_string(),
        visible_nodes[0]["assignedThreadId"]
    );
    assert!(!result.to_string().contains("Owner-only secret objective"));

    let activate_tool = tool_by_name(&delegate_tools, "activate_goal_plan_node");
    let activate = tool_call(
        "activate_goal_plan_node",
        "call-activate-delegated-plan",
        json!({ "node_id": delegated_node_id }),
    );
    let output = activate_tool.handle(activate.clone()).await?;
    let result = output.code_mode_result(&activate.payload);
    assert_eq!(result["goal"]["threadId"], delegate_thread_id.to_string());
    assert_eq!(result["goal"]["objective"], "Visible delegated objective.");
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_with_auto_off_clears_replaced_goal_without_activation()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new_with_config(
        runtime.clone(),
        thread_id,
        GoalExtensionConfig {
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
            ..test_goal_extension_config()
        },
    )
    .await?;
    let tools = harness.tools();

    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "existing active goal" }),
        ))
        .await?;

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "clear_existing_goal": true,
            "goals": [
                {
                    "key": "followup",
                    "objective": "Run later when automatic goal plans are disabled"
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(result["goal"], serde_json::Value::Null);
    assert_eq!(result["goalPlans"][0]["autoExecute"], "off");
    assert_eq!(result["goalPlans"][0]["nodes"][0]["status"], "pending");
    assert_eq!(
        None,
        runtime.thread_goals().get_thread_goal(thread_id).await?
    );
    Ok(())
}

#[tokio::test]
async fn activate_goal_plan_node_allows_explicit_activation_when_auto_off() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new_with_config(
        runtime,
        thread_id,
        GoalExtensionConfig {
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
            ..test_goal_extension_config()
        },
    )
    .await?;
    let tools = harness.tools();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "goals": [
                {
                    "key": "manual",
                    "objective": "Activate this goal explicitly"
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    let node_id = result["goalPlans"][0]["nodes"][0]["nodeId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("node id should be returned"))?;

    let activate_tool = tool_by_name(&tools, "activate_goal_plan_node");
    let invocation = tool_call(
        "activate_goal_plan_node",
        "call-activate-goal-plan-node",
        json!({ "node_id": node_id }),
    );
    let output = activate_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(result["goal"]["objective"], "Activate this goal explicitly");
    assert_eq!(
        result["activatedGoal"]["objective"],
        "Activate this goal explicitly"
    );
    assert_eq!(result["goalPlans"][0]["nodes"][0]["status"], "active");
    Ok(())
}

#[tokio::test]
async fn update_goal_uses_current_auto_execute_config_after_mid_turn_change() -> anyhow::Result<()>
{
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let initial_config = GoalExtensionConfig {
        auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
        ..test_goal_extension_config()
    };
    let harness =
        GoalExtensionHarness::new_with_config(runtime.clone(), thread_id, initial_config.clone())
            .await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let invocation = tool_call(
        "create_goal_plan",
        "call-create-goal-plan",
        json!({
            "goals": [
                {
                    "key": "first",
                    "objective": "Run first while automation is disabled"
                },
                {
                    "key": "second",
                    "objective": "Run second after config is enabled",
                    "depends_on": ["first"]
                }
            ]
        }),
    );
    let output = create_plan_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);
    assert_eq!(result["goal"], serde_json::Value::Null);
    assert_eq!(result["goalPlans"][0]["autoExecute"], "off");
    let first_node_id = result["goalPlans"][0]["nodes"][0]["nodeId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("first node id should be returned"))?;

    let activate_tool = tool_by_name(&tools, "activate_goal_plan_node");
    activate_tool
        .handle(tool_call(
            "activate_goal_plan_node",
            "call-activate-first-node",
            json!({ "node_id": first_node_id }),
        ))
        .await?;

    let enabled_config = GoalExtensionConfig {
        auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
        ..initial_config.clone()
    };
    harness.change_config(&initial_config, &enabled_config);

    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-complete-first-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(
        result["activatedGoal"]["objective"],
        "Run second after config is enabled"
    );
    assert_eq!(result["goalPlans"][0]["autoExecute"], "ready_only");
    assert_eq!(result["goalPlans"][0]["nodes"][0]["status"], "complete");
    assert_eq!(result["goalPlans"][0]["nodes"][1]["status"], "active");
    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("second goal should be active"))?;
    assert_eq!("Run second after config is enabled", goal.objective);
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    Ok(())
}

#[tokio::test]
async fn update_goal_can_complete_auto_activated_next_goal_in_same_turn() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new_with_config(
        runtime.clone(),
        thread_id,
        GoalExtensionConfig {
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            ..test_goal_extension_config()
        },
    )
    .await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-create-goal-plan",
            json!({
                "goals": [
                    {
                        "key": "first",
                        "objective": "Complete first chained goal"
                    },
                    {
                        "key": "second",
                        "objective": "Complete second chained goal",
                        "depends_on": ["first"]
                    }
                ]
            }),
        ))
        .await?;

    let update_tool = tool_by_name(&tools, "update_goal");
    update_tool
        .handle(tool_call(
            "update_goal",
            "call-complete-first-goal",
            json!({ "status": "complete" }),
        ))
        .await?;
    let second_complete = tool_call(
        "update_goal",
        "call-complete-second-goal",
        json!({ "status": "complete" }),
    );
    let output = update_tool.handle(second_complete.clone()).await?;
    let result = output.code_mode_result(&second_complete.payload);

    assert_eq!(result["goal"]["objective"], "Complete second chained goal");
    assert_eq!(result["goal"]["status"], "complete");
    assert_eq!(result["goalPlans"][0]["status"], "complete");
    assert_eq!(
        vec!["complete", "complete"],
        result["goalPlans"][0]["nodes"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("goal plan nodes should be an array"))?
            .iter()
            .map(|node| node["status"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
    );
    let plan = runtime
        .thread_goals()
        .list_thread_goal_plans(thread_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("goal plan should exist"))?;
    assert_eq!(
        codex_state::ThreadGoalPlanStatus::Complete,
        plan.plan.status
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_rejects_invalid_node_keys() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime.clone(), thread_id).await;

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let err = match create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-create-goal-plan-invalid-key",
            json!({
                "goals": [
                    {
                        "key": "invalid key",
                        "objective": "Try to create a plan with an invalid key"
                    }
                ]
            }),
        ))
        .await
    {
        Ok(_) => panic!("invalid goal plan key should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "goal plan node key `invalid key` must contain only ASCII letters, numbers, underscores, or hyphens"
                .to_string()
        )
    );
    assert_eq!(
        Vec::<codex_state::ThreadGoalPlanSnapshot>::new(),
        runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_rejects_invalid_title_before_clearing_existing_goal() -> anyhow::Result<()>
{
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime.clone(), thread_id).await;

    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "existing active goal",
                "title": "Existing active goal",
            }),
        ))
        .await?;
    let original_goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("existing goal should be present"))?;

    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");
    let err = match create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-create-goal-plan-invalid-title",
            json!({
                "clear_existing_goal": true,
                "goals": [
                    {
                        "key": "followup",
                        "objective": "Replace existing goal only after validation",
                        "title": "one two three four five six"
                    }
                ]
            }),
        ))
        .await
    {
        Ok(_) => panic!("invalid goal plan title should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        FunctionCallError::RespondToModel("goal title must be at most 5 words".to_string())
    );
    assert_eq!(
        Some(original_goal),
        runtime.thread_goals().get_thread_goal(thread_id).await?
    );
    assert_eq!(
        Vec::<codex_state::ThreadGoalPlanSnapshot>::new(),
        runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_plan_append_rejects_replacement_and_budget_options() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime.clone(), thread_id).await;
    let create_plan_tool = tool_by_name(&tools, "create_goal_plan");

    let clear_err = match create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-append-with-clear",
            json!({
                "append_to_plan_id": "plan-123",
                "clear_existing_goal": true,
                "goals": [
                    {
                        "key": "followup",
                        "objective": "Append with an invalid clear request"
                    }
                ]
            }),
        ))
        .await
    {
        Ok(_) => panic!("append with clear_existing_goal should fail"),
        Err(err) => err,
    };
    assert_eq!(
        clear_err,
        FunctionCallError::RespondToModel(
            "append_to_plan_id cannot be combined with clear_existing_goal".to_string()
        )
    );

    let budget_err = match create_plan_tool
        .handle(tool_call(
            "create_goal_plan",
            "call-append-with-plan-budget",
            json!({
                "append_to_plan_id": "plan-123",
                "max_tokens_per_goal_plan": 1000,
                "goals": [
                    {
                        "key": "followup",
                        "objective": "Append with an invalid plan budget request"
                    }
                ]
            }),
        ))
        .await
    {
        Ok(_) => panic!("append with max_tokens_per_goal_plan should fail"),
        Err(err) => err,
    };
    assert_eq!(
        budget_err,
        FunctionCallError::RespondToModel(
            "append_to_plan_id cannot be combined with max_tokens_per_goal_plan; appending does not change an existing plan budget"
                .to_string()
        )
    );
    assert_eq!(
        Vec::<codex_state::ThreadGoalPlanSnapshot>::new(),
        runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn installed_create_goal_tool_describes_default_task_use() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime, thread_id).await;

    let ToolSpec::Function(create_goal_tool) = tool_by_name(&tools, "create_goal").spec() else {
        panic!("create_goal should be a function tool");
    };

    assert!(
        create_goal_tool
            .description
            .contains("Start a durable thread goal when explicitly requested")
    );
    assert!(
        create_goal_tool
            .description
            .contains("Do not use this as the default for ordinary coding")
    );
    assert!(
        create_goal_tool
            .description
            .contains("Skip it for simple requests")
    );
    Ok(())
}

#[tokio::test]
async fn installed_goal_tools_require_adversarial_verification_before_completion()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let tools = installed_tools(runtime, thread_id).await;

    for tool_name in [
        "create_goal",
        "create_goal_plan",
        "activate_goal_plan_node",
        "update_goal",
        "resume_goal",
    ] {
        let ToolSpec::Function(tool) = tool_by_name(&tools, tool_name).spec() else {
            panic!("{tool_name} should be a function tool");
        };

        assert!(
            tool.description
                .contains("Adversarial verification is required")
        );
        assert!(
            tool.description
                .contains("use at least one adversarial agent")
        );
        assert!(tool.description.contains("even if the user did not ask"));
        assert!(tool.description.contains("adversarial self-review"));
    }
    Ok(())
}

#[tokio::test]
async fn goal_tools_hidden_for_ephemeral_threads() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    let tools = installed_tools_with_start(
        runtime,
        thread_id,
        SessionSource::Cli,
        /*persistent_thread_state_available*/ false,
    )
    .await;

    assert_eq!(Vec::<String>::new(), tool_names(&tools));
    Ok(())
}

#[tokio::test]
async fn goal_tools_hidden_for_review_subagents() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    let tools = installed_tools_with_start(
        runtime,
        thread_id,
        SessionSource::SubAgent(SubAgentSource::Review),
        /*persistent_thread_state_available*/ true,
    )
    .await;

    assert_eq!(Vec::<String>::new(), tool_names(&tools));
    Ok(())
}

#[tokio::test]
async fn installed_goal_tools_reject_duplicate_goal_creation() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime, thread_id).await?;
    let tools = harness.tools();

    let create_tool = tool_by_name(&tools, "create_goal");
    let first = tool_call(
        "create_goal",
        "call-create-goal-1",
        json!({ "objective": "first goal" }),
    );
    create_tool.handle(first).await?;

    let second = tool_call(
        "create_goal",
        "call-create-goal-2",
        json!({ "objective": "second goal" }),
    );
    let err = match create_tool.handle(second).await {
        Ok(_) => panic!("duplicate create should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "cannot create a new goal because this thread already has a goal; set clear_existing_goal to true only when explicitly instructed to replace or start a new goal"
                .to_string()
        )
    );
    Ok(())
}

#[tokio::test]
async fn installed_goal_tools_replace_existing_goal_when_explicit() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    let tools = harness.tools();

    let create_tool = tool_by_name(&tools, "create_goal");
    let first = tool_call(
        "create_goal",
        "call-create-goal-1",
        json!({
            "objective": "first goal",
            "token_budget": 123,
        }),
    );
    create_tool.handle(first).await?;
    let first_goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("first goal should exist"))?;

    let second = tool_call(
        "create_goal",
        "call-create-goal-2",
        json!({
            "objective": "second goal",
            "token_budget": 456,
            "clear_existing_goal": true,
        }),
    );
    create_tool.handle(second).await?;
    let replaced_goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("replacement goal should exist"))?;

    assert_ne!(first_goal.goal_id, replaced_goal.goal_id);
    assert_eq!("second goal", replaced_goal.objective);
    assert_eq!(codex_state::ThreadGoalStatus::Active, replaced_goal.status);
    assert_eq!(Some(456), replaced_goal.token_budget);
    assert_eq!(0, replaced_goal.tokens_used);
    assert_eq!(0, replaced_goal.time_used_seconds);
    Ok(())
}

#[tokio::test]
async fn create_goal_resets_baseline_before_turn_stop_accounting() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness
        .start_turn(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 100, /*cached_input_tokens*/ 10,
                /*output_tokens*/ 30, /*reasoning_output_tokens*/ 5,
                /*total_tokens*/ 135,
            ),
        )
        .await;
    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 120, /*cached_input_tokens*/ 14,
                /*output_tokens*/ 42, /*reasoning_output_tokens*/ 8,
                /*total_tokens*/ 162,
            ),
        )
        .await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 127, /*cached_input_tokens*/ 16,
                /*output_tokens*/ 52, /*reasoning_output_tokens*/ 10,
                /*total_tokens*/ 189,
            ),
        )
        .await;
    harness.stop_turn("turn-1").await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(15, goal.tokens_used);
    assert_eq!(ThreadGoalStatus::Active, protocol_status(goal.status));
    Ok(())
}

#[tokio::test]
async fn tool_finish_accounts_active_goal_progress_and_emits_event() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(23, goal.tokens_used);

    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "call-shell".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 23,
        }],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn parallel_tool_finish_accounts_active_goal_progress_once() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness
        .start_turn(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 100, /*cached_input_tokens*/ 0,
                /*output_tokens*/ 0, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 100,
            ),
        )
        .await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 130, /*cached_input_tokens*/ 0,
                /*output_tokens*/ 0, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 130,
            ),
        )
        .await;

    tokio::join!(
        harness.notify_tool_finish("turn-1", "call-shell-1", "shell"),
        harness.notify_tool_finish("turn-1", "call-shell-2", "shell"),
    );

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(30, goal.tokens_used);

    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "call-shell-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 30,
        }],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn budget_limited_goal_keeps_accruing_until_turn_stop() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "ship goal extension backend",
                "token_budget": 25,
            }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 10, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;
    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 24, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 16, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 40,
            ),
        )
        .await;
    harness.stop_turn("turn-1").await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(35, goal.tokens_used);
    assert_eq!(codex_state::ThreadGoalStatus::BudgetLimited, goal.status);

    assert_eq!(
        vec![
            CapturedGoalEvent {
                event_id: "call-shell".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::BudgetLimited,
                tokens_used: 25,
            },
            CapturedGoalEvent {
                event_id: "turn-1:turn-stop".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::BudgetLimited,
                tokens_used: 35,
            },
        ],
        harness.sink.goal_events()
    );

    Ok(())
}

#[tokio::test]
async fn budget_limited_goal_keeps_accounting_after_later_tool_finish() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "ship goal extension backend",
                "token_budget": 25,
            }),
        ))
        .await?;

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 10, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell-1", "shell")
        .await;
    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 24, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 16, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 40,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell-2", "shell")
        .await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(35, goal.tokens_used);
    assert_eq!(codex_state::ThreadGoalStatus::BudgetLimited, goal.status);
    Ok(())
}

#[tokio::test]
async fn turn_error_usage_limit_accounts_progress_and_clears_accounting() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_turn_error("turn-1", CodexErrorInfo::UsageLimitExceeded)
        .await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(23, goal.tokens_used);
    assert_eq!(codex_state::ThreadGoalStatus::UsageLimited, goal.status);
    let pending = pending_interactions_for_kind(
        runtime.as_ref(),
        thread_id,
        codex_state::PendingInteractionKind::UsageLimit,
    )
    .await?;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].source_kind,
        codex_state::PendingInteractionSourceKind::Goal
    );
    assert_eq!(pending[0].source_id.as_deref(), Some(goal.goal_id.as_str()));
    assert_eq!(pending[0].turn_id.as_deref(), Some("turn-1"));
    assert_eq!(pending[0].request_payload_json["reason"], "usage-limit");
    assert_eq!(
        vec![
            CapturedGoalEvent {
                event_id: "turn-1:usage-limit-progress".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::Active,
                tokens_used: 23,
            },
            CapturedGoalEvent {
                event_id: "turn-1:usage-limit".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::UsageLimited,
                tokens_used: 23,
            },
        ],
        harness.sink.goal_events()
    );

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 50, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 20, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 70,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell-after-usage-limit", "shell")
        .await;
    harness.stop_turn("turn-1").await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(23, goal.tokens_used);
    assert_eq!(codex_state::ThreadGoalStatus::UsageLimited, goal.status);
    Ok(())
}

#[tokio::test]
async fn turn_error_blocks_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    tool_by_name(&tools, "create_goal")
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;

    harness
        .notify_turn_error("turn-1", CodexErrorInfo::Other)
        .await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Blocked, goal.status);
    Ok(())
}

#[tokio::test]
async fn turn_error_blocks_goal_plan_node_with_actionable_wait_payload() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    tool_by_name(&tools, "create_goal_plan")
        .handle(tool_call(
            "create_goal_plan",
            "call-create-loop-goal-plan",
            json!({
                "goals": [
                    {
                        "key": "init",
                        "objective": "Initialize a headless loop run"
                    },
                    {
                        "key": "finish",
                        "objective": "Finish the headless loop run",
                        "depends_on": ["init"]
                    }
                ]
            }),
        ))
        .await?;

    harness
        .notify_turn_error("turn-1", CodexErrorInfo::ContextWindowExceeded)
        .await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Blocked, goal.status);

    let plan = runtime
        .thread_goals()
        .list_thread_goal_plans(thread_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("goal plan should exist"))?;
    assert_eq!(codex_state::ThreadGoalPlanStatus::Blocked, plan.plan.status);
    assert_eq!(
        codex_state::ThreadGoalPlanNodeStatus::Blocked,
        plan.nodes[0].status
    );
    assert_eq!(
        codex_state::ThreadGoalPlanNodeStatus::Pending,
        plan.nodes[1].status
    );

    let pending = pending_interactions_for_kind(
        runtime.as_ref(),
        thread_id,
        codex_state::PendingInteractionKind::Blocked,
    )
    .await?;
    assert_eq!(1, pending.len());
    assert_eq!(Some(goal.goal_id.as_str()), pending[0].source_id.as_deref());
    assert_eq!(Some("turn-1"), pending[0].turn_id.as_deref());
    assert_eq!(pending[0].request_payload_json["reason"], "turn-error");
    assert_eq!(
        pending[0].request_payload_json["terminalError"],
        json!({
            "codexErrorInfo": "context_window_exceeded",
            "code": "context_window_exceeded",
            "action": "The turn exceeded the model context window. Reduce prompt/history size or run a cleanup/compaction before retrying the loop.",
        })
    );
    Ok(())
}

#[tokio::test]
async fn usage_limit_budget_limited_goal_accounts_remaining_progress() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({
                "objective": "ship goal extension backend",
                "token_budget": 25,
            }),
        ))
        .await?;

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 10, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 24, /*cached_input_tokens*/ 5,
                /*output_tokens*/ 16, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 40,
            ),
        )
        .await;
    harness
        .runtime_handle()
        .usage_limit_active_goal_for_turn("turn-1")
        .await
        .map_err(anyhow::Error::msg)?;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(35, goal.tokens_used);
    assert_eq!(codex_state::ThreadGoalStatus::UsageLimited, goal.status);
    assert_eq!(
        vec![
            CapturedGoalEvent {
                event_id: "turn-1:usage-limit-progress".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::BudgetLimited,
                tokens_used: 35,
            },
            CapturedGoalEvent {
                event_id: "turn-1:usage-limit".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::UsageLimited,
                tokens_used: 35,
            },
        ],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn usage_limit_plan_turn_does_not_stop_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;

    harness
        .start_turn_with_mode("turn-plan", ModeKind::Plan, &TokenUsage::default())
        .await;
    harness.sink.clear();
    harness
        .runtime_handle()
        .usage_limit_active_goal_for_turn("turn-plan")
        .await
        .map_err(anyhow::Error::msg)?;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());
    Ok(())
}

#[tokio::test]
async fn usage_limit_stale_turn_does_not_stop_current_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.stop_turn("turn-1").await;
    harness.start_turn("turn-2", &TokenUsage::default()).await;
    harness.sink.clear();

    harness
        .runtime_handle()
        .usage_limit_active_goal_for_turn("turn-1")
        .await
        .map_err(anyhow::Error::msg)?;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());
    Ok(())
}

#[tokio::test]
async fn update_goal_stale_turn_does_not_complete_current_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.stop_turn("turn-1").await;
    harness.start_turn("turn-2", &TokenUsage::default()).await;

    let update_tool = tool_by_name(&tools, "update_goal");
    let err = match update_tool
        .handle(tool_call(
            "update_goal",
            "call-update-stale-goal",
            json!({ "status": "complete" }),
        ))
        .await
    {
        Ok(_) => panic!("stale update_goal should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "cannot update goal because this tool call is no longer associated with the active goal turn"
                .to_string()
        )
    );
    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    Ok(())
}

#[tokio::test]
async fn update_goal_can_block_and_accounts_final_progress() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    let update_tool = tool_by_name(&tools, "update_goal");
    let invocation = tool_call(
        "update_goal",
        "call-update-goal",
        json!({ "status": "blocked" }),
    );
    let output = update_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(
        result,
        json!({
            "goal": {
                "goalId": result["goal"]["goalId"],
                "threadId": thread_id,
                "objective": "ship goal extension backend",
                "title": "ship goal extension backend",
                "status": "blocked",
                "tokensUsed": 23,
                "timeUsedSeconds": 0,
                "createdAt": result["goal"]["createdAt"],
                "updatedAt": result["goal"]["updatedAt"],
            },
            "remainingTokens": serde_json::Value::Null,
            "completionBudgetReport": serde_json::Value::Null,
            "goalPlanCompletionReport": serde_json::Value::Null,
        })
    );

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(23, goal.tokens_used);
    assert_eq!(codex_state::ThreadGoalStatus::Blocked, goal.status);
    let pending = pending_interactions_for_kind(
        runtime.as_ref(),
        thread_id,
        codex_state::PendingInteractionKind::Blocked,
    )
    .await?;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].source_kind,
        codex_state::PendingInteractionSourceKind::Goal
    );
    assert_eq!(pending[0].source_id.as_deref(), Some(goal.goal_id.as_str()));
    assert_eq!(pending[0].turn_id.as_deref(), Some("turn-1"));
    assert_eq!(
        pending[0].request_payload_json["reason"],
        "update-goal-blocked"
    );

    assert_eq!(
        vec![
            CapturedGoalEvent {
                event_id: "call-update-goal".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::Active,
                tokens_used: 23,
            },
            CapturedGoalEvent {
                event_id: "call-update-goal".to_string(),
                turn_id: Some("turn-1".to_string()),
                status: ThreadGoalStatus::Blocked,
                tokens_used: 23,
            },
        ],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn resume_goal_reactivates_blocked_goal_and_accounts_future_progress() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "ship goal extension backend",
            codex_state::ThreadGoalStatus::Blocked,
            /*token_budget*/ Some(100),
        )
        .await?;
    let outcome = runtime
        .thread_goals()
        .account_thread_goal_usage(
            thread_id,
            /*time_delta_seconds*/ 42,
            /*token_delta*/ 17,
            codex_state::GoalAccountingMode::ActiveOrStopped,
            /*expected_goal_id*/ None,
        )
        .await?;
    let codex_state::GoalAccountingOutcome::Updated(blocked_goal) = outcome else {
        panic!("blocked goal should preserve accounted usage before resume");
    };
    assert_eq!(codex_state::ThreadGoalStatus::Blocked, blocked_goal.status);
    assert_eq!(17, blocked_goal.tokens_used);
    assert_eq!(42, blocked_goal.time_used_seconds);
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let resume_tool = tool_by_name(&tools, "resume_goal");
    let invocation = tool_call("resume_goal", "call-resume-goal", json!({}));
    let output = resume_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(
        result,
        json!({
            "goal": {
                "goalId": result["goal"]["goalId"],
                "threadId": thread_id,
                "objective": "ship goal extension backend",
                "status": "active",
                "tokenBudget": 100,
                "tokensUsed": 17,
                "timeUsedSeconds": 42,
                "createdAt": result["goal"]["createdAt"],
                "updatedAt": result["goal"]["updatedAt"],
            },
            "remainingTokens": 83,
            "completionBudgetReport": serde_json::Value::Null,
            "goalPlanCompletionReport": serde_json::Value::Null,
        })
    );
    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "call-resume-goal".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 17,
        }],
        harness.sink.goal_events()
    );

    harness.sink.clear();
    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-resume-goal", "resume_goal")
        .await;
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());

    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;
    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    assert_eq!(40, goal.tokens_used);
    assert!(
        goal.time_used_seconds >= 42,
        "resumed goal should not reset previously recorded elapsed time"
    );
    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "call-shell".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 40,
        }],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn resume_goal_preserves_usage_limited_goal_usage() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "ship goal extension backend",
            codex_state::ThreadGoalStatus::Active,
            /*token_budget*/ Some(80),
        )
        .await?;
    let outcome = runtime
        .thread_goals()
        .account_thread_goal_usage(
            thread_id,
            /*time_delta_seconds*/ 31,
            /*token_delta*/ 19,
            codex_state::GoalAccountingMode::ActiveOnly,
            /*expected_goal_id*/ None,
        )
        .await?;
    let codex_state::GoalAccountingOutcome::Updated(accounted_goal) = outcome else {
        panic!("active goal should account usage before usage limiting");
    };
    assert_eq!(codex_state::ThreadGoalStatus::Active, accounted_goal.status);
    let usage_limited_goal = runtime
        .thread_goals()
        .usage_limit_active_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("active goal should become usage limited"))?;
    assert_eq!(
        codex_state::ThreadGoalStatus::UsageLimited,
        usage_limited_goal.status
    );
    assert_eq!(19, usage_limited_goal.tokens_used);
    assert_eq!(31, usage_limited_goal.time_used_seconds);
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let resume_tool = tool_by_name(&tools, "resume_goal");
    let invocation = tool_call("resume_goal", "call-resume-goal", json!({}));
    let output = resume_tool.handle(invocation.clone()).await?;
    let result = output.code_mode_result(&invocation.payload);

    assert_eq!(
        result,
        json!({
            "goal": {
                "goalId": result["goal"]["goalId"],
                "threadId": thread_id,
                "objective": "ship goal extension backend",
                "status": "active",
                "tokenBudget": 80,
                "tokensUsed": 19,
                "timeUsedSeconds": 31,
                "createdAt": result["goal"]["createdAt"],
                "updatedAt": result["goal"]["updatedAt"],
            },
            "remainingTokens": 61,
            "completionBudgetReport": serde_json::Value::Null,
            "goalPlanCompletionReport": serde_json::Value::Null,
        })
    );
    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "call-resume-goal".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 19,
        }],
        harness.sink.goal_events()
    );

    harness.sink.clear();
    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-resume-goal", "resume_goal")
        .await;
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());

    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;
    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    assert_eq!(42, goal.tokens_used);
    assert!(
        goal.time_used_seconds >= 31,
        "usage-limited resume should preserve elapsed time before future accounting"
    );
    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "call-shell".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 42,
        }],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn resume_goal_rejects_active_and_complete_goals() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();
    let resume_tool = tool_by_name(&tools, "resume_goal");

    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "ship goal extension backend",
            codex_state::ThreadGoalStatus::Active,
            /*token_budget*/ None,
        )
        .await?;
    let err = match resume_tool
        .handle(tool_call("resume_goal", "call-resume-active", json!({})))
        .await
    {
        Ok(_) => panic!("active goal resume should fail"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "cannot resume goal because it is already active".to_string()
        )
    );

    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "ship goal extension backend",
            codex_state::ThreadGoalStatus::Complete,
            /*token_budget*/ None,
        )
        .await?;
    let err = match resume_tool
        .handle(tool_call("resume_goal", "call-resume-complete", json!({})))
        .await
    {
        Ok(_) => panic!("complete goal resume should fail"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "cannot resume a completed goal; create a new goal only when explicitly requested"
                .to_string()
        )
    );
    Ok(())
}

#[tokio::test]
async fn resume_goal_rejects_budget_limited_goal_without_accounting() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "ship goal extension backend",
            codex_state::ThreadGoalStatus::Active,
            /*token_budget*/ Some(0),
        )
        .await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;
    let tools = harness.tools();
    let resume_tool = tool_by_name(&tools, "resume_goal");

    let err = match resume_tool
        .handle(tool_call(
            "resume_goal",
            "call-resume-budget-limited",
            json!({}),
        ))
        .await
    {
        Ok(_) => panic!("budget-limited goal resume should fail"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "cannot resume a budget-limited goal without changing its token budget".to_string()
        )
    );
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;
    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(codex_state::ThreadGoalStatus::BudgetLimited, goal.status);
    assert_eq!(0, goal.tokens_used);
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());
    Ok(())
}

#[tokio::test]
async fn external_goal_mutation_start_accounts_active_goal_progress() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 20, /*cached_input_tokens*/ 5, /*output_tokens*/ 8,
                /*reasoning_output_tokens*/ 2, /*total_tokens*/ 30,
            ),
        )
        .await;
    harness
        .runtime_handle()
        .prepare_external_goal_mutation()
        .await
        .map_err(anyhow::Error::msg)?;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(23, goal.tokens_used);
    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: "turn-1:external-goal-mutation".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: ThreadGoalStatus::Active,
            tokens_used: 23,
        }],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn goal_service_external_set_active_resets_baseline_without_live_thread() -> anyhow::Result<()>
{
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness
        .start_turn(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 100, /*cached_input_tokens*/ 0,
                /*output_tokens*/ 0, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 100,
            ),
        )
        .await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "old objective" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 120, /*cached_input_tokens*/ 0,
                /*output_tokens*/ 0, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 120,
            ),
        )
        .await;
    let outcome = harness
        .goal_service
        .set_thread_goal(
            runtime.as_ref(),
            GoalSetRequest {
                thread_id,
                objective: GoalObjectiveUpdate::Set("new objective"),
                title: GoalTitleUpdate::Keep,
                status: Some(ThreadGoalStatus::Active),
                token_budget: GoalTokenBudgetUpdate::Keep,
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            },
        )
        .await?;
    outcome.apply_runtime_effects(&harness.goal_service).await;

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 130, /*cached_input_tokens*/ 0,
                /*output_tokens*/ 0, /*reasoning_output_tokens*/ 0,
                /*total_tokens*/ 130,
            ),
        )
        .await;
    harness
        .notify_tool_finish("turn-1", "call-shell", "shell")
        .await;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(30, goal.tokens_used);
    Ok(())
}

#[tokio::test]
async fn goal_service_external_complete_advances_ready_plan_node_without_live_thread()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let api = GoalService::new();

    let created = runtime
        .thread_goals()
        .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
            thread_id,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            max_tokens: None,
            nodes: vec![
                codex_state::ThreadGoalPlanNodeCreateParams {
                    key: "first".to_string(),
                    objective: "Finish first external goal".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                },
                codex_state::ThreadGoalPlanNodeCreateParams {
                    key: "second".to_string(),
                    objective: "Continue with second external goal".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: vec!["first".to_string()],
                },
            ],
        })
        .await?;
    assert_eq!(
        Some("Finish first external goal"),
        created
            .activated_goal
            .as_ref()
            .map(|goal| goal.objective.as_str())
    );

    let outcome = api
        .set_thread_goal(
            runtime.as_ref(),
            GoalSetRequest {
                thread_id,
                objective: GoalObjectiveUpdate::Keep,
                title: GoalTitleUpdate::Keep,
                status: Some(ThreadGoalStatus::Complete),
                token_budget: GoalTokenBudgetUpdate::Keep,
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            },
        )
        .await?;
    assert_eq!(
        Some("Continue with second external goal"),
        outcome
            .plan_update
            .as_ref()
            .and_then(|update| update.activated_goal.as_ref())
            .map(|goal| goal.objective.as_str())
    );

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("second goal should be active"))?;
    assert_eq!("Continue with second external goal", goal.objective);
    assert_eq!(codex_state::ThreadGoalStatus::Active, goal.status);
    let plan = runtime
        .thread_goals()
        .list_thread_goal_plans(thread_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("goal plan should exist"))?;
    assert_eq!(
        vec![
            codex_state::ThreadGoalPlanNodeStatus::Complete,
            codex_state::ThreadGoalPlanNodeStatus::Active,
        ],
        plan.nodes
            .iter()
            .map(|node| node.status)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test]
async fn goal_service_external_resume_reactivates_blocked_plan_without_live_thread()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let api = GoalService::new();

    let created = runtime
        .thread_goals()
        .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
            thread_id,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            max_tokens: None,
            nodes: vec![codex_state::ThreadGoalPlanNodeCreateParams {
                key: "blocked".to_string(),
                objective: "Wait for coordinator input".to_string(),
                assigned_thread_id: None,
                title: None,
                priority: 0,
                token_budget: None,
                depends_on: Vec::new(),
            }],
        })
        .await?;
    let active_goal = created
        .activated_goal
        .ok_or_else(|| anyhow::anyhow!("goal should activate"))?;

    api.set_thread_goal(
        runtime.as_ref(),
        GoalSetRequest {
            thread_id,
            objective: GoalObjectiveUpdate::Keep,
            title: GoalTitleUpdate::Keep,
            status: Some(ThreadGoalStatus::Blocked),
            token_budget: GoalTokenBudgetUpdate::Keep,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
        },
    )
    .await?;
    let blocked_plan = runtime
        .thread_goals()
        .list_thread_goal_plans(thread_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("goal plan should exist"))?;
    assert_eq!(
        codex_state::ThreadGoalPlanStatus::Blocked,
        blocked_plan.plan.status
    );
    assert_eq!(
        codex_state::ThreadGoalPlanNodeStatus::Blocked,
        blocked_plan.nodes[0].status
    );
    let pending = pending_interactions_for_kind(
        runtime.as_ref(),
        thread_id,
        codex_state::PendingInteractionKind::Blocked,
    )
    .await?;
    assert_eq!(1, pending.len());
    assert_eq!(
        Some(active_goal.goal_id.as_str()),
        pending[0].source_id.as_deref()
    );

    let outcome = api
        .set_thread_goal(
            runtime.as_ref(),
            GoalSetRequest {
                thread_id,
                objective: GoalObjectiveUpdate::Keep,
                title: GoalTitleUpdate::Keep,
                status: Some(ThreadGoalStatus::Active),
                token_budget: GoalTokenBudgetUpdate::Keep,
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            },
        )
        .await?;
    assert_eq!(
        Some(codex_state::ThreadGoalPlanStatus::Active),
        outcome
            .plan_update
            .as_ref()
            .map(|update| update.snapshot.plan.status)
    );
    assert_eq!(
        Some(codex_state::ThreadGoalPlanNodeStatus::Active),
        outcome
            .plan_update
            .as_ref()
            .and_then(|update| update.snapshot.nodes.first())
            .map(|node| node.status)
    );
    assert_eq!(
        Vec::<codex_state::PendingInteraction>::new(),
        pending_interactions_for_kind(
            runtime.as_ref(),
            thread_id,
            codex_state::PendingInteractionKind::Blocked,
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn goal_service_external_wait_statuses_record_and_clear_pending_interactions()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness
        .goal_service
        .set_thread_goal(
            runtime.as_ref(),
            GoalSetRequest {
                thread_id,
                objective: GoalObjectiveUpdate::Set("wait for coordinator"),
                title: GoalTitleUpdate::Keep,
                status: None,
                token_budget: GoalTokenBudgetUpdate::Keep,
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            },
        )
        .await?
        .apply_runtime_effects(&harness.goal_service)
        .await;

    for (status, kind, reason) in [
        (
            ThreadGoalStatus::Blocked,
            codex_state::PendingInteractionKind::Blocked,
            "external-goal-blocked",
        ),
        (
            ThreadGoalStatus::UsageLimited,
            codex_state::PendingInteractionKind::UsageLimit,
            "external-goal-usage-limit",
        ),
    ] {
        harness
            .goal_service
            .set_thread_goal(
                runtime.as_ref(),
                GoalSetRequest {
                    thread_id,
                    objective: GoalObjectiveUpdate::Keep,
                    title: GoalTitleUpdate::Keep,
                    status: Some(status),
                    token_budget: GoalTokenBudgetUpdate::Keep,
                    auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
                },
            )
            .await?
            .apply_runtime_effects(&harness.goal_service)
            .await;

        let goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
        let pending = pending_interactions_for_kind(runtime.as_ref(), thread_id, kind).await?;
        assert_eq!(1, pending.len());
        assert_eq!(Some(goal.goal_id.as_str()), pending[0].source_id.as_deref());
        assert_eq!(
            serde_json::Value::String(reason.to_string()),
            pending[0].request_payload_json["reason"]
        );

        harness
            .goal_service
            .set_thread_goal(
                runtime.as_ref(),
                GoalSetRequest {
                    thread_id,
                    objective: GoalObjectiveUpdate::Keep,
                    title: GoalTitleUpdate::Keep,
                    status: Some(ThreadGoalStatus::Active),
                    token_budget: GoalTokenBudgetUpdate::Keep,
                    auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
                },
            )
            .await?
            .apply_runtime_effects(&harness.goal_service)
            .await;
        assert_eq!(
            Vec::<codex_state::PendingInteraction>::new(),
            pending_interactions_for_kind(runtime.as_ref(), thread_id, kind).await?
        );
    }
    Ok(())
}

#[tokio::test]
async fn goal_service_clear_thread_goal_clears_pending_interactions_without_live_thread()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let api = GoalService::new();

    api.set_thread_goal(
        runtime.as_ref(),
        GoalSetRequest {
            thread_id,
            objective: GoalObjectiveUpdate::Set("wait for external unblock"),
            title: GoalTitleUpdate::Keep,
            status: Some(ThreadGoalStatus::Blocked),
            token_budget: GoalTokenBudgetUpdate::Keep,
            auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
        },
    )
    .await?;
    assert_eq!(
        1,
        pending_interactions_for_kind(
            runtime.as_ref(),
            thread_id,
            codex_state::PendingInteractionKind::Blocked,
        )
        .await?
        .len()
    );

    assert!(
        api.clear_thread_goal(runtime.as_ref(), thread_id)
            .await?
            .cleared
    );
    assert_eq!(
        Vec::<codex_state::PendingInteraction>::new(),
        pending_interactions_for_kind(
            runtime.as_ref(),
            thread_id,
            codex_state::PendingInteractionKind::Blocked,
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn create_goal_clear_existing_goal_clears_pending_interactions() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness
        .goal_service
        .set_thread_goal(
            runtime.as_ref(),
            GoalSetRequest {
                thread_id,
                objective: GoalObjectiveUpdate::Set("blocked goal to replace"),
                title: GoalTitleUpdate::Keep,
                status: Some(ThreadGoalStatus::Blocked),
                token_budget: GoalTokenBudgetUpdate::Keep,
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            },
        )
        .await?
        .apply_runtime_effects(&harness.goal_service)
        .await;
    assert_eq!(
        1,
        pending_interactions_for_kind(
            runtime.as_ref(),
            thread_id,
            codex_state::PendingInteractionKind::Blocked,
        )
        .await?
        .len()
    );

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-replace-goal",
            json!({
                "objective": "replacement goal",
                "clear_existing_goal": true,
            }),
        ))
        .await?;

    assert_eq!(
        Vec::<codex_state::PendingInteraction>::new(),
        pending_interactions_for_kind(
            runtime.as_ref(),
            thread_id,
            codex_state::PendingInteractionKind::Blocked,
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn thread_stop_unregisters_goal_runtime_from_service() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness.start_turn("turn-1", &TokenUsage::default()).await;

    let tools = harness.tools();
    let create_tool = tool_by_name(&tools, "create_goal");
    create_tool
        .handle(tool_call(
            "create_goal",
            "call-create-goal",
            json!({ "objective": "ship goal extension backend" }),
        ))
        .await?;
    harness.sink.clear();

    harness
        .record_token_usage(
            "turn-1",
            &token_usage(
                /*input_tokens*/ 10, /*cached_input_tokens*/ 0, /*output_tokens*/ 0,
                /*reasoning_output_tokens*/ 0, /*total_tokens*/ 10,
            ),
        )
        .await;
    harness.stop_thread().await;

    assert!(
        harness
            .goal_service
            .clear_thread_goal(runtime.as_ref(), thread_id)
            .await?
            .cleared
    );
    assert_eq!(Vec::<CapturedGoalEvent>::new(), harness.sink.goal_events());
    Ok(())
}

#[tokio::test]
async fn thread_resume_rehydrates_active_goal_idle_accounting() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "ship goal extension backend",
            codex_state::ThreadGoalStatus::Active,
            /*token_budget*/ None,
        )
        .await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;

    harness.resume_thread().await;
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    harness
        .runtime_handle()
        .prepare_external_goal_mutation()
        .await
        .map_err(anyhow::Error::msg)?;

    let goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    assert_eq!(ThreadGoalStatus::Active, protocol_status(goal.status));
    assert!(
        goal.time_used_seconds >= 1,
        "resumed idle accounting should add elapsed wall-clock time"
    );
    assert_eq!(
        vec![CapturedGoalEvent {
            event_id: format!("{thread_id}:external-goal-mutation"),
            turn_id: None,
            status: ThreadGoalStatus::Active,
            tokens_used: 0,
        }],
        harness.sink.goal_events()
    );
    Ok(())
}

#[tokio::test]
async fn goal_service_sets_gets_and_clears_thread_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let api = GoalService::new();

    let set = api
        .set_thread_goal(
            runtime.as_ref(),
            GoalSetRequest {
                thread_id,
                objective: GoalObjectiveUpdate::Set(" ship goal API ownership "),
                title: GoalTitleUpdate::Keep,
                status: None,
                token_budget: GoalTokenBudgetUpdate::Set(Some(123)),
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::ReadyOnly,
            },
        )
        .await?;
    let get = api
        .get_thread_goal(runtime.as_ref(), thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("goal should exist"))?;
    let metadata = runtime
        .get_thread(thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("seeded thread metadata should exist"))?;

    assert_eq!(set.goal, get);
    assert_eq!("ship goal API ownership", get.objective);
    assert_eq!(ThreadGoalStatus::Active, get.status);
    assert_eq!(Some(123), get.token_budget);
    assert_eq!(Some("ship goal API ownership"), metadata.preview.as_deref());

    assert!(
        api.clear_thread_goal(runtime.as_ref(), thread_id)
            .await?
            .cleared
    );
    assert_eq!(
        None,
        api.get_thread_goal(runtime.as_ref(), thread_id).await?
    );
    assert!(
        !api.clear_thread_goal(runtime.as_ref(), thread_id)
            .await?
            .cleared
    );
    Ok(())
}

async fn installed_tools(
    runtime: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    installed_tools_with_start(
        runtime,
        thread_id,
        SessionSource::Cli,
        /*persistent_thread_state_available*/ true,
    )
    .await
}

async fn installed_tools_with_start(
    runtime: Arc<codex_state::StateRuntime>,
    thread_id: ThreadId,
    session_source: SessionSource,
    persistent_thread_state_available: bool,
) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    let mut builder = ExtensionRegistryBuilder::<()>::new();
    let goal_service = Arc::new(GoalService::new());
    install_with_backend(
        &mut builder,
        runtime,
        /*metrics_client*/ None,
        Weak::new(),
        goal_service,
        |_| test_goal_extension_config(),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-1");
    let thread_store = ExtensionData::new(thread_id.to_string());
    for contributor in registry.thread_lifecycle_contributors() {
        contributor
            .on_thread_start(ThreadStartInput {
                config: &(),
                session_source: &session_source,
                persistent_thread_state_available,
                session_store: &session_store,
                thread_store: &thread_store,
            })
            .await;
    }

    registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
        .collect()
}

fn tool_names(tools: &[Arc<dyn ToolExecutor<ToolCall>>]) -> Vec<String> {
    tools.iter().map(|tool| tool.tool_name().name).collect()
}

fn test_goal_extension_config() -> GoalExtensionConfig {
    GoalExtensionConfig {
        enabled: true,
        auto_execute: codex_state::ThreadGoalPlanAutoExecute::AiDirected,
        max_auto_goals_per_plan: 12,
        max_tokens_per_goal_plan: None,
        post_goal_context: codex_state::PostGoalContextAction::Keep,
        post_goal_plan_context: codex_state::PostGoalContextAction::Keep,
    }
}

struct GoalExtensionHarness {
    registry: codex_extension_api::ExtensionRegistry<GoalExtensionConfig>,
    session_store: ExtensionData,
    thread_store: ExtensionData,
    goal_service: Arc<GoalService>,
    sink: Arc<RecordingEventSink>,
}

impl GoalExtensionHarness {
    async fn new(
        runtime: Arc<codex_state::StateRuntime>,
        thread_id: ThreadId,
    ) -> anyhow::Result<Self> {
        Self::new_with_config(runtime, thread_id, test_goal_extension_config()).await
    }

    async fn new_with_config(
        runtime: Arc<codex_state::StateRuntime>,
        thread_id: ThreadId,
        config: GoalExtensionConfig,
    ) -> anyhow::Result<Self> {
        let sink = Arc::new(RecordingEventSink::default());
        let mut builder =
            ExtensionRegistryBuilder::<GoalExtensionConfig>::with_event_sink(sink.clone());
        let goal_service = Arc::new(GoalService::new());
        install_with_backend(
            &mut builder,
            runtime,
            /*metrics_client*/ None,
            Weak::new(),
            Arc::clone(&goal_service),
            |config: &GoalExtensionConfig| config.clone(),
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session-1");
        let thread_store = ExtensionData::new(thread_id.to_string());
        let session_source = SessionSource::Cli;
        for contributor in registry.thread_lifecycle_contributors() {
            contributor
                .on_thread_start(ThreadStartInput {
                    config: &config,
                    session_source: &session_source,
                    persistent_thread_state_available: true,
                    session_store: &session_store,
                    thread_store: &thread_store,
                })
                .await;
        }
        Ok(Self {
            registry,
            session_store,
            thread_store,
            goal_service,
            sink,
        })
    }

    fn tools(&self) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        self.registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&self.session_store, &self.thread_store))
            .collect()
    }

    fn change_config(
        &self,
        previous_config: &GoalExtensionConfig,
        new_config: &GoalExtensionConfig,
    ) {
        for contributor in self.registry.config_contributors() {
            contributor.on_config_changed(
                &self.session_store,
                &self.thread_store,
                previous_config,
                new_config,
            );
        }
    }

    async fn start_turn(&self, turn_id: &str, usage: &TokenUsage) {
        self.start_turn_with_mode(turn_id, ModeKind::Default, usage)
            .await;
    }

    async fn start_turn_with_mode(&self, turn_id: &str, mode: ModeKind, usage: &TokenUsage) {
        let turn_store = ExtensionData::new(turn_id);
        let mut collaboration_mode = default_collaboration_mode();
        collaboration_mode.mode = mode;
        for contributor in self.registry.turn_lifecycle_contributors() {
            contributor
                .on_turn_start(TurnStartInput {
                    turn_id,
                    collaboration_mode: &collaboration_mode,
                    token_usage_at_turn_start: usage,
                    session_store: &self.session_store,
                    thread_store: &self.thread_store,
                    turn_store: &turn_store,
                })
                .await;
        }
    }

    async fn stop_turn(&self, turn_id: &str) {
        let turn_store = ExtensionData::new(turn_id);
        for contributor in self.registry.turn_lifecycle_contributors() {
            contributor
                .on_turn_stop(TurnStopInput {
                    session_store: &self.session_store,
                    thread_store: &self.thread_store,
                    turn_store: &turn_store,
                })
                .await;
        }
    }

    async fn record_token_usage(&self, turn_id: &str, usage: &TokenUsage) {
        let turn_store = ExtensionData::new(turn_id);
        let token_usage = TokenUsageInfo {
            total_token_usage: usage.clone(),
            last_token_usage: TokenUsage::default(),
            model_context_window: None,
        };
        for contributor in self.registry.token_usage_contributors() {
            contributor
                .on_token_usage(
                    &self.session_store,
                    &self.thread_store,
                    &turn_store,
                    &token_usage,
                )
                .await;
        }
    }

    async fn resume_thread(&self) {
        for contributor in self.registry.thread_lifecycle_contributors() {
            contributor
                .on_thread_resume(ThreadResumeInput {
                    session_store: &self.session_store,
                    thread_store: &self.thread_store,
                })
                .await;
        }
    }

    async fn stop_thread(&self) {
        for contributor in self.registry.thread_lifecycle_contributors() {
            contributor
                .on_thread_stop(ThreadStopInput {
                    session_store: &self.session_store,
                    thread_store: &self.thread_store,
                })
                .await;
        }
    }

    async fn notify_tool_finish(&self, turn_id: &str, call_id: &str, tool_name: &str) {
        let turn_store = ExtensionData::new(turn_id);
        let tool_name = codex_extension_api::ToolName::plain(tool_name);
        for contributor in self.registry.tool_lifecycle_contributors() {
            contributor
                .on_tool_finish(ToolFinishInput {
                    session_store: &self.session_store,
                    thread_store: &self.thread_store,
                    turn_store: &turn_store,
                    turn_id,
                    call_id,
                    tool_name: &tool_name,
                    source: ToolCallSource::Direct,
                    outcome: ToolCallOutcome::Completed { success: true },
                })
                .await;
        }
    }

    async fn notify_turn_error(&self, turn_id: &str, error: CodexErrorInfo) {
        let turn_store = ExtensionData::new(turn_id);
        for contributor in self.registry.turn_lifecycle_contributors() {
            contributor
                .on_turn_error(TurnErrorInput {
                    turn_id,
                    error: error.clone(),
                    session_store: &self.session_store,
                    thread_store: &self.thread_store,
                    turn_store: &turn_store,
                })
                .await;
        }
    }

    fn runtime_handle(&self) -> Arc<GoalRuntimeHandle> {
        self.thread_store
            .get::<GoalRuntimeHandle>()
            .unwrap_or_else(|| panic!("goal runtime handle should exist"))
    }
}

fn tool_by_name<'a>(
    tools: &'a [Arc<dyn ToolExecutor<ToolCall>>],
    name: &str,
) -> &'a Arc<dyn ToolExecutor<ToolCall>> {
    tools
        .iter()
        .find(|tool| tool.tool_name().namespace.is_none() && tool.tool_name().name == name)
        .unwrap_or_else(|| panic!("missing tool {name}"))
}

fn tool_call(tool_name: &str, call_id: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        turn_id: "turn-1".to_string(),
        call_id: call_id.to_string(),
        tool_name: codex_extension_api::ToolName::plain(tool_name),
        model: "gpt-test".to_string(),
        truncation_policy: TruncationPolicy::Bytes(1024),
        conversation_history: codex_extension_api::ConversationHistory::default(),
        turn_item_emitter: Arc::new(NoopTurnItemEmitter),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

async fn test_runtime() -> anyhow::Result<Arc<codex_state::StateRuntime>> {
    let tempdir = TempDir::new()?;
    codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string()).await
}

async fn pending_interactions_for_kind(
    runtime: &codex_state::StateRuntime,
    thread_id: ThreadId,
    kind: codex_state::PendingInteractionKind,
) -> anyhow::Result<Vec<codex_state::PendingInteraction>> {
    Ok(runtime
        .list_thread_pending_interactions(codex_state::PendingInteractionListParams {
            thread_id: Some(thread_id),
            statuses: vec![codex_state::PendingInteractionStatus::Pending],
            kinds: vec![kind],
            cursor: None,
            limit: 10,
        })
        .await?
        .data)
}

fn test_thread_id() -> anyhow::Result<ThreadId> {
    ThreadId::from_string("11111111-1111-4111-8111-111111111111").map_err(anyhow::Error::msg)
}

async fn seed_thread_metadata(
    runtime: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> anyhow::Result<()> {
    let builder = codex_state::ThreadMetadataBuilder::new(
        thread_id,
        runtime
            .codex_home()
            .join(format!("rollout-{thread_id}.jsonl")),
        chrono::Utc::now(),
        SessionSource::Cli,
    );
    runtime.upsert_thread(&builder.build("test-provider")).await
}

#[derive(Debug, Default)]
struct RecordingEventSink {
    events: Mutex<Vec<Event>>,
}

impl RecordingEventSink {
    fn goal_events(&self) -> Vec<CapturedGoalEvent> {
        self.events()
            .iter()
            .filter_map(|event| match &event.msg {
                EventMsg::ThreadGoalUpdated(updated) => Some(CapturedGoalEvent {
                    event_id: event.id.clone(),
                    turn_id: updated.turn_id.clone(),
                    status: updated.goal.status,
                    tokens_used: updated.goal.tokens_used,
                }),
                _ => None,
            })
            .collect()
    }

    fn goal_plan_events(&self) -> Vec<CapturedGoalPlanEvent> {
        self.events()
            .iter()
            .filter_map(|event| match &event.msg {
                EventMsg::ThreadGoalPlanUpdated(updated) => Some(CapturedGoalPlanEvent {
                    event_id: event.id.clone(),
                    turn_id: updated.turn_id.clone(),
                    node_count: updated.plan.node_count,
                    node_objectives: updated
                        .plan
                        .nodes
                        .iter()
                        .map(|node| node.objective.clone())
                        .collect(),
                }),
                _ => None,
            })
            .collect()
    }

    fn clear(&self) {
        self.events().clear();
    }

    fn events(&self) -> std::sync::MutexGuard<'_, Vec<Event>> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ExtensionEventSink for RecordingEventSink {
    fn emit(&self, event: Event) {
        self.events().push(event);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CapturedGoalEvent {
    event_id: String,
    turn_id: Option<String>,
    status: ThreadGoalStatus,
    tokens_used: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct CapturedGoalPlanEvent {
    event_id: String,
    turn_id: Option<String>,
    node_count: i64,
    node_objectives: Vec<String>,
}

fn default_collaboration_mode() -> CollaborationMode {
    CollaborationMode {
        mode: ModeKind::Default,
        settings: Settings {
            model: "gpt-5".to_string(),
            reasoning_effort: None,
            developer_instructions: None,
        },
    }
}

fn token_usage(
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
) -> TokenUsage {
    TokenUsage {
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens: 0,
        output_tokens,
        reasoning_output_tokens,
        total_tokens,
    }
}

fn protocol_status(status: codex_state::ThreadGoalStatus) -> ThreadGoalStatus {
    match status {
        codex_state::ThreadGoalStatus::Active => ThreadGoalStatus::Active,
        codex_state::ThreadGoalStatus::Paused => ThreadGoalStatus::Paused,
        codex_state::ThreadGoalStatus::Blocked => ThreadGoalStatus::Blocked,
        codex_state::ThreadGoalStatus::UsageLimited => ThreadGoalStatus::UsageLimited,
        codex_state::ThreadGoalStatus::BudgetLimited => ThreadGoalStatus::BudgetLimited,
        codex_state::ThreadGoalStatus::Deferred => ThreadGoalStatus::Deferred,
        codex_state::ThreadGoalStatus::Complete => ThreadGoalStatus::Complete,
        codex_state::ThreadGoalStatus::Cancelled => ThreadGoalStatus::Cancelled,
    }
}
