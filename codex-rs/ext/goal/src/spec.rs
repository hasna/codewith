//! Responses API tool definitions for persisted thread goals.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const GET_GOAL_TOOL_NAME: &str = "get_goal";
pub const CREATE_GOAL_TOOL_NAME: &str = "create_goal";
pub const GET_GOAL_PLAN_TOOL_NAME: &str = "get_goal_plan";
pub const CREATE_GOAL_PLAN_TOOL_NAME: &str = "create_goal_plan";
pub const ACTIVATE_GOAL_PLAN_NODE_TOOL_NAME: &str = "activate_goal_plan_node";
pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";
pub const RESUME_GOAL_TOOL_NAME: &str = "resume_goal";

const ADVERSARIAL_GOAL_COMPLETION_REQUIREMENT: &str = "Adversarial verification is required before completing any goal: use at least one adversarial agent to verify and validate the work even if the user did not ask for one, reconcile the result before calling update_goal with status complete, and if no adversarial agent can be spawned, explicitly perform and report an adversarial self-review with the same standards.";

pub fn create_get_goal_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: GET_GOAL_TOOL_NAME.to_string(),
        description: "Get the current goal for this thread, including status, budgets, token and elapsed-time usage, and remaining token budget."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}

pub fn create_create_goal_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "objective".to_string(),
            JsonSchema::string(Some(
                "Required. The concrete objective to start pursuing. This starts a new active goal. If a goal already exists, this tool fails unless clear_existing_goal is true."
                    .to_string(),
            )),
        ),
        (
            "clear_existing_goal".to_string(),
            JsonSchema::boolean(Some(
                "Optional. Defaults to false. Set to true only when the user or system/developer instructions explicitly tell you to clear, replace, restart, or start a new goal while another goal exists."
                    .to_string(),
            )),
        ),
        (
            "token_budget".to_string(),
            JsonSchema::integer(Some(
                "Positive token budget for the new goal. Omit unless explicitly requested."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: CREATE_GOAL_TOOL_NAME.to_string(),
        description: format!(
            r#"Start a durable thread goal when explicitly requested by the user or system/developer instructions, or when the work is genuinely long-running, resumable, or high-effort enough that preserving progress across turns materially helps.
Do not use this as the default for ordinary coding, investigation, verification, or multi-step tasks; use update_plan/TODOs for short-horizon task tracking.
Skip it for simple requests such as greetings, direct factual answers, quick command outputs, brief clarifications, or other one-step work.
Set token_budget only when an explicit token budget is requested.
If a goal already exists, this fails by default. Set clear_existing_goal to true only when the user or system/developer instructions explicitly tell you to clear, replace, restart, or start a new goal. Use {UPDATE_GOAL_TOOL_NAME} only for terminal status.
{ADVERSARIAL_GOAL_COMPLETION_REQUIREMENT}"#
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["objective".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_get_goal_plan_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: GET_GOAL_PLAN_TOOL_NAME.to_string(),
        description: "Get the current capped page of durable goal plans for this thread, including each returned goal node's stable id, key, dependencies, status, usage, and budget. A missing token budget means unlimited."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}

pub fn create_create_goal_plan_tool() -> ToolSpec {
    let node_properties = BTreeMap::from([
        (
            "key".to_string(),
            JsonSchema::string(Some(
                "Required stable short key unique inside this plan, for example `investigate`, `implement`, or `verify`."
                    .to_string(),
            )),
        ),
        (
            "objective".to_string(),
            JsonSchema::string(Some(
                "Required concrete objective for this goal node. Each node should represent substantial work that can be pursued and completed independently."
                    .to_string(),
            )),
        ),
        (
            "depends_on".to_string(),
            JsonSchema::array(
                JsonSchema::string(Some(
                    "A goal key that must complete before this node is ready.".to_string(),
                )),
                Some(
                    "Optional list of goal keys this node depends on. Omit or use an empty array when the node can run independently."
                        .to_string(),
                ),
            ),
        ),
        (
            "priority".to_string(),
            JsonSchema::integer(Some(
                "Optional priority for choosing among independent ready goals. Higher runs first. Defaults to 0."
                    .to_string(),
            )),
        ),
        (
            "token_budget".to_string(),
            JsonSchema::integer(Some(
                "Optional positive token budget for this goal node. Omit for unlimited."
                    .to_string(),
            )),
        ),
    ]);
    let node_schema = JsonSchema::object(
        node_properties,
        Some(vec!["key".to_string(), "objective".to_string()]),
        Some(false.into()),
    );
    let properties = BTreeMap::from([
        (
            "goals".to_string(),
            JsonSchema::array(
                node_schema,
                Some(
                    "Required goal nodes for this plan. Use multiple nodes only for high-effort work that benefits from explicit sequencing or independent ready work."
                        .to_string(),
                ),
            ),
        ),
        (
            "clear_existing_goal".to_string(),
            JsonSchema::boolean(Some(
                "Optional. Defaults to false. Set to true only when the user or system/developer instructions explicitly tell you to replace the existing goal/plan."
                    .to_string(),
            )),
        ),
        (
            "max_tokens_per_goal_plan".to_string(),
            JsonSchema::integer(Some(
                "Optional positive token cap for the whole goal plan. Omit for unlimited. This is separate from each goal node's token_budget."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: CREATE_GOAL_PLAN_TOOL_NAME.to_string(),
        description: format!(
            r#"Create a durable plan made of multiple goals for high-effort work.
Use this when the task naturally splits into substantial goals, such as investigation, implementation, verification, release follow-up, or parallel independent work.
Dependencies are optional: use depends_on only when one goal truly requires another goal to finish first. If several goals are independent, leave them dependency-free and use priority to indicate the best next choice.
This is goal orchestration, not workflows. Workflows are higher-level reusable processes and should not be modeled here.
Automatic execution between ready goals is controlled by global config. When enabled, the next ready goal can be activated without asking the user again. When disabled, the plan is still saved but ready goals wait for explicit activation.
If update_plan is available, maintain TODOs for the current goal's concrete tasks and tool-prep steps; the goal plan is the durable high-level execution graph, not the short-horizon checklist.
Omit token_budget for unlimited per-goal tokens. Omit max_tokens_per_goal_plan for an unlimited plan-level budget.
{ADVERSARIAL_GOAL_COMPLETION_REQUIREMENT}"#
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["goals".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_activate_goal_plan_node_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "node_id".to_string(),
        JsonSchema::string(Some(
            "Required stable node id from get_goal_plan for a pending node whose dependencies are complete."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: ACTIVATE_GOAL_PLAN_NODE_TOOL_NAME.to_string(),
        description: format!(
            r#"Activate one ready goal plan node as the current goal.
Use this only when a goal plan exists and you need to choose among ready independent goals under the configured automatic execution policy.
Do not activate a node whose dependencies are incomplete, and do not use this to bypass blocked, budget-limited, or paused goals.
{ADVERSARIAL_GOAL_COMPLETION_REQUIREMENT}"#
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["node_id".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_update_goal_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "status".to_string(),
        JsonSchema::string_enum(
            vec![json!("complete"), json!("blocked"), json!("cancelled")],
            Some(
                "Required. Set to `complete` only when the objective is achieved and no required work remains. Set to `blocked` only after the same blocking condition has recurred for at least three consecutive goal turns and the agent is at an impasse. Set to `cancelled` only when the user explicitly cancels the goal or the current goal is intentionally abandoned before completion."
                    .to_string(),
            ),
        ),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: UPDATE_GOAL_TOOL_NAME.to_string(),
        description: format!(
            r#"Update the existing goal.
Use this tool only to mark the goal achieved or genuinely blocked.
Set status to `complete` only when the objective has actually been achieved and no required work remains.
Adversarial verification is required before status `complete`: use at least one adversarial agent to verify and validate the work even if the user did not ask for one, reconcile the result before calling this tool, and if no adversarial agent can be spawned, explicitly perform and report an adversarial self-review with the same standards.
Set status to `blocked` only when the same blocking condition has repeated for at least three consecutive goal turns, counting the original/user-triggered turn and any automatic continuations, and the agent cannot make meaningful progress without user input or an external-state change.
Set status to `cancelled` only when the user explicitly cancels the goal, asks you to stop pursuing it, or the current goal is intentionally abandoned before completion.
If the user resumes a goal that was previously marked `blocked`, treat the resumed run as a fresh blocked audit. If the same blocking condition then repeats for at least three consecutive resumed goal turns, set status to `blocked` again.
Once the blocked threshold is satisfied, do not keep reporting that you are still blocked while leaving the goal active; set status to `blocked`.
Do not use `blocked` merely because the work is hard, slow, uncertain, incomplete, or would benefit from clarification.
Do not mark a goal complete merely because its budget is nearly exhausted or because you are stopping work.
You cannot use this tool to pause, resume, budget-limit, or usage-limit a goal; those status changes are controlled by the user or system.
When marking a budgeted goal achieved with status `complete`, report the final token usage from the tool result to the user.
{ADVERSARIAL_GOAL_COMPLETION_REQUIREMENT}"#
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["status".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_resume_goal_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: RESUME_GOAL_TOOL_NAME.to_string(),
        description: format!(
            r#"Resume an existing stopped goal by setting it back to active.
Use this tool only when the user explicitly asks to resume a paused, blocked, or usage-limited goal.
Do not use this tool for budget-limited goals because they cannot resume without changing the budget.
Do not use this tool for completed or cancelled goals; create a new goal only when explicitly requested.
After resuming a previously blocked goal, treat the resumed run as a fresh blocked audit before any later blocked update.
{ADVERSARIAL_GOAL_COMPLETION_REQUIREMENT}"#
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}
