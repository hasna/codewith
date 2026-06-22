use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolOutput;
use codex_protocol::protocol::ThreadGoal;
use codex_protocol::protocol::validate_thread_goal_objective;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

use crate::accounting::BudgetLimitedGoalDisposition;
use crate::runtime::GoalPlanRuntimeConfig;
use crate::tool::CompletionBudgetReport;
use crate::tool::GoalToolExecutor;
use crate::tool::PostGoalContextActionArg;
use crate::tool::fill_empty_thread_preview_if_possible;
use crate::tool::goal_response_with_plan;
use crate::tool::parse_arguments;
use crate::tool::protocol_goal_from_state;
use crate::tool::validate_goal_budget;

const MAX_GOAL_PLAN_NODES: usize = 64;
const MAX_GOAL_PLAN_NODE_KEY_LEN: usize = 64;
const MAX_GOAL_PLAN_RESPONSE_NODES: usize = 16;
const MAX_GOAL_PLAN_RESPONSE_OBJECTIVE_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateGoalPlanRequest {
    goals: Vec<CreateGoalPlanNodeRequest>,
    #[serde(default)]
    clear_existing_goal: bool,
    max_tokens_per_goal_plan: Option<i64>,
    post_goal_context: Option<PostGoalContextActionArg>,
    post_goal_plan_context: Option<PostGoalContextActionArg>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateGoalPlanNodeRequest {
    key: String,
    objective: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    priority: Option<i64>,
    token_budget: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ActivateGoalPlanNodeRequest {
    node_id: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalPlanResponse {
    plan_id: String,
    thread_id: String,
    status: String,
    auto_execute: String,
    max_tokens: Option<i64>,
    total_tokens_used: i64,
    total_time_used_seconds: i64,
    remaining_tokens: Option<i64>,
    node_count: i64,
    completed_node_count: i64,
    ready_node_count: i64,
    active_node_count: i64,
    pending_node_count: i64,
    paused_node_count: i64,
    blocked_node_count: i64,
    usage_limited_node_count: i64,
    budget_limited_node_count: i64,
    cancelled_node_count: i64,
    created_at: i64,
    updated_at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    nodes_omitted_count: i64,
    nodes: Vec<GoalPlanNodeResponse>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalPlanCompletionReport {
    plan_id: String,
    status: String,
    max_tokens: Option<i64>,
    total_tokens_used: i64,
    total_time_used_seconds: i64,
    remaining_tokens: Option<i64>,
    node_count: i64,
    completed_node_count: i64,
    ready_node_count: i64,
    active_node_count: i64,
    pending_node_count: i64,
    paused_node_count: i64,
    blocked_node_count: i64,
    usage_limited_node_count: i64,
    budget_limited_node_count: i64,
    cancelled_node_count: i64,
    #[serde(skip_serializing_if = "is_zero")]
    nodes_omitted_count: i64,
    nodes: Vec<GoalPlanCompletionNodeReport>,
    summary_instruction: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GoalPlanCompletionNodeReport {
    key: String,
    objective: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    objective_truncated: bool,
    status: String,
    ready: bool,
    token_budget: Option<i64>,
    tokens_used: i64,
    time_used_seconds: i64,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GoalPlanNodeResponse {
    node_id: String,
    plan_id: String,
    thread_id: String,
    key: String,
    sequence: i64,
    priority: i64,
    objective: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    objective_truncated: bool,
    status: String,
    ready: bool,
    token_budget: Option<i64>,
    tokens_used: i64,
    time_used_seconds: i64,
    projected_goal_id: Option<String>,
    depends_on: Vec<String>,
    created_at: i64,
    updated_at: i64,
}

impl GoalToolExecutor {
    pub(crate) async fn handle_get_plan(
        &self,
        invocation: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let _ = invocation.function_arguments()?;
        let goal = self
            .state_db
            .thread_goals()
            .get_thread_goal(self.thread_id)
            .await
            .map(|goal| goal.map(protocol_goal_from_state))
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read goal: {err}"))
            })?;
        let goal_plans = self.goal_plan_responses().await?;
        goal_response_with_plan(
            goal,
            /*activated_goal*/ None,
            goal_plans,
            CompletionBudgetReport::Omit,
        )
    }

    pub(crate) async fn handle_create_plan(
        &self,
        invocation: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let mut request: CreateGoalPlanRequest = parse_arguments(invocation.function_arguments()?)?;
        let plan_config = self
            .plan_config
            .as_ref()
            .ok_or_else(|| {
                FunctionCallError::Fatal("goal plan tool missing runtime config".to_string())
            })?
            .current();
        validate_goal_plan_request(&mut request, plan_config)?;
        let existing_goal = self
            .state_db
            .thread_goals()
            .get_thread_goal(self.thread_id)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read goal: {err}"))
            })?;
        if existing_goal.is_some() && !request.clear_existing_goal {
            return Err(FunctionCallError::RespondToModel(
                "cannot create a goal plan because this thread already has a goal; set clear_existing_goal to true only when explicitly instructed to replace or start a new goal plan"
                    .to_string(),
            ));
        }
        if request.clear_existing_goal {
            self.account_active_goal_progress(
                codex_state::GoalAccountingMode::ActiveOnly,
                invocation.call_id.as_str(),
                BudgetLimitedGoalDisposition::ClearActive,
            )
            .await?;
            self.mark_existing_plan_goal_replaced(existing_goal).await?;
            self.state_db
                .thread_goals()
                .delete_thread_goal(self.thread_id)
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to clear replaced goal before creating goal plan: {err}"
                    ))
                })?;
        }

        let nodes = request
            .goals
            .into_iter()
            .map(|node| codex_state::ThreadGoalPlanNodeCreateParams {
                key: node.key,
                objective: node.objective,
                priority: node.priority.unwrap_or(0),
                token_budget: node.token_budget,
                depends_on: node.depends_on,
            })
            .collect();
        let max_tokens = match (
            request.max_tokens_per_goal_plan,
            plan_config.max_tokens_per_goal_plan,
        ) {
            (Some(requested), Some(configured)) => Some(requested.min(configured)),
            (Some(requested), None) => Some(requested),
            (None, configured) => configured,
        };
        let outcome = self
            .state_db
            .thread_goals()
            .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
                thread_id: self.thread_id,
                auto_execute: plan_config.auto_execute,
                max_tokens,
                nodes,
            })
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to create goal plan: {err}"))
            })?;
        if request.post_goal_context.is_some() || request.post_goal_plan_context.is_some() {
            let post_goal_action = request
                .post_goal_context
                .map(codex_state::PostGoalContextAction::from)
                .unwrap_or(plan_config.post_goal_context);
            let post_goal_plan_action = request
                .post_goal_plan_context
                .map(codex_state::PostGoalContextAction::from)
                .unwrap_or(plan_config.post_goal_plan_context);
            self.state_db
                .thread_goals()
                .set_thread_goal_plan_context_actions(
                    self.thread_id,
                    outcome.snapshot.plan.plan_id.as_str(),
                    post_goal_action,
                    post_goal_plan_action,
                )
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to set goal plan context lifecycle policy: {err}"
                    ))
                })?;
        }
        let activated_goal = self
            .apply_activated_goal_from_plan(&invocation, outcome.activated_goal)
            .await?;
        self.event_emitter.thread_goal_plan_updated(
            format!("{}-goal-plan", invocation.call_id),
            Some(invocation.turn_id.clone()),
            outcome.snapshot.clone(),
        );
        let goal_plans = vec![GoalPlanResponse::from(outcome.snapshot)];
        goal_response_with_plan(
            activated_goal.clone(),
            activated_goal,
            goal_plans,
            CompletionBudgetReport::Omit,
        )
    }

    pub(crate) async fn handle_activate_plan_node(
        &self,
        invocation: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let request: ActivateGoalPlanNodeRequest =
            parse_arguments(invocation.function_arguments()?)?;
        let _plan_config = self
            .plan_config
            .as_ref()
            .ok_or_else(|| {
                FunctionCallError::Fatal("goal plan tool missing runtime config".to_string())
            })?
            .current();
        let existing_goal = self
            .state_db
            .thread_goals()
            .get_thread_goal(self.thread_id)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read goal: {err}"))
            })?;
        if existing_goal.is_some_and(|goal| {
            !matches!(
                goal.status,
                codex_state::ThreadGoalStatus::Complete
                    | codex_state::ThreadGoalStatus::BudgetLimited
                    | codex_state::ThreadGoalStatus::Cancelled
            )
        }) {
            return Err(FunctionCallError::RespondToModel(
                "cannot activate a goal plan node while the current goal is still active or stopped resumably"
                    .to_string(),
            ));
        }
        let outcome = self
            .state_db
            .thread_goals()
            .activate_thread_goal_plan_node(self.thread_id, request.node_id.trim())
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to activate goal plan node: {err}"
                ))
            })?
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "cannot activate goal plan node because it is not ready".to_string(),
                )
            })?;
        let activated_goal = self
            .apply_activated_goal_from_plan(&invocation, outcome.activated_goal)
            .await?;
        self.event_emitter.thread_goal_plan_updated(
            format!("{}-goal-plan", invocation.call_id),
            Some(invocation.turn_id.clone()),
            outcome.snapshot.clone(),
        );
        let goal_plans = vec![GoalPlanResponse::from(outcome.snapshot)];
        goal_response_with_plan(
            activated_goal.clone(),
            activated_goal,
            goal_plans,
            CompletionBudgetReport::Omit,
        )
    }

    pub(crate) async fn apply_activated_goal_from_plan(
        &self,
        invocation: &ToolCall,
        goal: Option<codex_state::ThreadGoal>,
    ) -> Result<Option<ThreadGoal>, FunctionCallError> {
        let Some(goal) = goal else {
            return Ok(None);
        };
        fill_empty_thread_preview_if_possible(self.state_db.as_ref(), self.thread_id, &goal).await;
        self.metrics.record_created();
        let turn_id = self
            .accounting_state
            .mark_current_turn_goal_active(goal.goal_id.clone());
        if turn_id.is_none() {
            self.accounting_state
                .mark_idle_goal_active(goal.goal_id.clone());
        }
        let goal = protocol_goal_from_state(goal);
        self.event_emitter.thread_goal_updated(
            format!("{}:activated-goal", invocation.call_id),
            turn_id,
            goal.clone(),
        );
        Ok(Some(goal))
    }

    pub(crate) async fn mark_existing_plan_goal_replaced(
        &self,
        existing_goal: Option<codex_state::ThreadGoal>,
    ) -> Result<(), FunctionCallError> {
        let Some(mut existing_goal) = existing_goal else {
            return Ok(());
        };
        if matches!(
            existing_goal.status,
            codex_state::ThreadGoalStatus::Complete | codex_state::ThreadGoalStatus::Cancelled
        ) {
            return Ok(());
        }
        crate::pending_interaction::clear_goal_status_waits(
            self.state_db.as_ref(),
            self.thread_id,
            existing_goal.goal_id.as_str(),
            "goal replaced",
        )
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to clear replaced goal pending interactions: {err}"
            ))
        })?;
        existing_goal.status = codex_state::ThreadGoalStatus::Blocked;
        self.state_db
            .thread_goals()
            .sync_goal_plan_node_for_goal(self.thread_id, &existing_goal)
            .await
            .map(|_| ())
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to sync replaced goal plan node: {err}"
                ))
            })
    }

    pub(crate) async fn goal_plan_responses(
        &self,
    ) -> Result<Vec<GoalPlanResponse>, FunctionCallError> {
        self.state_db
            .thread_goals()
            .list_thread_goal_plans(self.thread_id)
            .await
            .map(|plans| plans.into_iter().map(GoalPlanResponse::from).collect())
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read goal plans: {err}"))
            })
    }
}

fn validate_goal_plan_request(
    request: &mut CreateGoalPlanRequest,
    plan_config: GoalPlanRuntimeConfig,
) -> Result<(), FunctionCallError> {
    if request.goals.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "goal plan must contain at least one goal".to_string(),
        ));
    }
    let max_goals = plan_config.max_auto_goals_per_plan.min(MAX_GOAL_PLAN_NODES);
    if request.goals.len() > max_goals {
        return Err(FunctionCallError::RespondToModel(format!(
            "goal plan contains {} goals but max_auto_goals_per_plan is {}",
            request.goals.len(),
            max_goals
        )));
    }
    validate_goal_budget(request.max_tokens_per_goal_plan)
        .map_err(FunctionCallError::RespondToModel)?;
    let mut keys = HashSet::new();
    for node in &mut request.goals {
        node.key = node.key.trim().to_string();
        node.objective = node.objective.trim().to_string();
        node.depends_on = node
            .depends_on
            .iter()
            .map(|dependency| dependency.trim().to_string())
            .collect();
        if node.key.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "goal plan node keys must not be empty".to_string(),
            ));
        }
        if node.key.len() > MAX_GOAL_PLAN_NODE_KEY_LEN {
            return Err(FunctionCallError::RespondToModel(format!(
                "goal plan node key `{}` is too long; maximum is {MAX_GOAL_PLAN_NODE_KEY_LEN} bytes",
                node.key
            )));
        }
        if !is_valid_goal_plan_node_key(&node.key) {
            return Err(FunctionCallError::RespondToModel(format!(
                "goal plan node key `{}` must contain only ASCII letters, numbers, underscores, or hyphens",
                node.key
            )));
        }
        if !keys.insert(node.key.clone()) {
            return Err(FunctionCallError::RespondToModel(format!(
                "goal plan node key `{}` is duplicated",
                node.key
            )));
        }
        validate_thread_goal_objective(&node.objective)
            .map_err(FunctionCallError::RespondToModel)?;
        validate_goal_budget(node.token_budget).map_err(FunctionCallError::RespondToModel)?;
    }
    for node in &request.goals {
        for dependency in &node.depends_on {
            if dependency.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "goal plan node `{}` has an empty dependency key",
                    node.key
                )));
            }
            if dependency == &node.key {
                return Err(FunctionCallError::RespondToModel(format!(
                    "goal plan node `{}` cannot depend on itself",
                    node.key
                )));
            }
            if !keys.contains(dependency) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "goal plan node `{}` depends on unknown node `{dependency}`",
                    node.key
                )));
            }
        }
    }
    Ok(())
}

fn is_valid_goal_plan_node_key(key: &str) -> bool {
    key.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

impl From<codex_state::ThreadGoalPlanSnapshot> for GoalPlanResponse {
    fn from(snapshot: codex_state::ThreadGoalPlanSnapshot) -> Self {
        let summary = snapshot.usage_summary();
        let ready_node_ids = snapshot
            .ready_node_ids()
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let nodes_omitted_count = snapshot
            .nodes
            .len()
            .saturating_sub(MAX_GOAL_PLAN_RESPONSE_NODES);
        Self {
            plan_id: snapshot.plan.plan_id,
            thread_id: snapshot.plan.thread_id.to_string(),
            status: snapshot.plan.status.as_str().to_string(),
            auto_execute: snapshot.plan.auto_execute.as_str().to_string(),
            max_tokens: snapshot.plan.max_tokens,
            total_tokens_used: summary.total_tokens_used,
            total_time_used_seconds: summary.total_time_used_seconds,
            remaining_tokens: summary.remaining_tokens,
            node_count: summary.node_count,
            completed_node_count: summary.completed_node_count,
            ready_node_count: summary.ready_node_count,
            active_node_count: summary.active_node_count,
            pending_node_count: summary.pending_node_count,
            paused_node_count: summary.paused_node_count,
            blocked_node_count: summary.blocked_node_count,
            usage_limited_node_count: summary.usage_limited_node_count,
            budget_limited_node_count: summary.budget_limited_node_count,
            cancelled_node_count: summary.cancelled_node_count,
            created_at: snapshot.plan.created_at.timestamp(),
            updated_at: snapshot.plan.updated_at.timestamp(),
            nodes_omitted_count: nodes_omitted_count as i64,
            nodes: snapshot
                .nodes
                .into_iter()
                .take(MAX_GOAL_PLAN_RESPONSE_NODES)
                .map(|node| {
                    let ready = ready_node_ids.contains(&node.node_id);
                    GoalPlanNodeResponse::from_node(node, ready)
                })
                .collect(),
        }
    }
}

impl GoalPlanCompletionReport {
    pub(crate) fn from_snapshot_if_terminal(
        snapshot: &codex_state::ThreadGoalPlanSnapshot,
    ) -> Option<Self> {
        if !matches!(
            snapshot.plan.status,
            codex_state::ThreadGoalPlanStatus::BudgetLimited
                | codex_state::ThreadGoalPlanStatus::Complete
                | codex_state::ThreadGoalPlanStatus::Cancelled
        ) {
            return None;
        }

        let summary = snapshot.usage_summary();
        let ready_node_ids = snapshot
            .ready_node_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        let nodes_omitted_count = snapshot
            .nodes
            .len()
            .saturating_sub(MAX_GOAL_PLAN_RESPONSE_NODES);
        Some(Self {
            plan_id: snapshot.plan.plan_id.clone(),
            status: snapshot.plan.status.as_str().to_string(),
            max_tokens: snapshot.plan.max_tokens,
            total_tokens_used: summary.total_tokens_used,
            total_time_used_seconds: summary.total_time_used_seconds,
            remaining_tokens: summary.remaining_tokens,
            node_count: summary.node_count,
            completed_node_count: summary.completed_node_count,
            ready_node_count: summary.ready_node_count,
            active_node_count: summary.active_node_count,
            pending_node_count: summary.pending_node_count,
            paused_node_count: summary.paused_node_count,
            blocked_node_count: summary.blocked_node_count,
            usage_limited_node_count: summary.usage_limited_node_count,
            budget_limited_node_count: summary.budget_limited_node_count,
            cancelled_node_count: summary.cancelled_node_count,
            nodes_omitted_count: nodes_omitted_count as i64,
            nodes: snapshot
                .nodes
                .iter()
                .take(MAX_GOAL_PLAN_RESPONSE_NODES)
                .map(|node| {
                    let (objective, objective_truncated) = truncated_objective(&node.objective);
                    GoalPlanCompletionNodeReport {
                        key: node.key.clone(),
                        objective,
                        objective_truncated,
                        status: node.status.as_str().to_string(),
                        ready: ready_node_ids.contains(&node.node_id),
                        token_budget: node.token_budget,
                        tokens_used: node.tokens_used,
                        time_used_seconds: node.time_used_seconds,
                    }
                })
                .collect(),
            summary_instruction:
                "Summarize the goal chain for the user. Include each goal's outcome, tokens, and elapsed time, then include total tokens and total elapsed time."
                    .to_string(),
        })
    }
}

impl GoalPlanNodeResponse {
    fn from_node(node: codex_state::ThreadGoalPlanNode, ready: bool) -> Self {
        let (objective, objective_truncated) = truncated_objective(&node.objective);
        Self {
            node_id: node.node_id,
            plan_id: node.plan_id,
            thread_id: node.thread_id.to_string(),
            key: node.key,
            sequence: node.sequence,
            priority: node.priority,
            objective,
            objective_truncated,
            status: node.status.as_str().to_string(),
            ready,
            token_budget: node.token_budget,
            tokens_used: node.tokens_used,
            time_used_seconds: node.time_used_seconds,
            projected_goal_id: node.projected_goal_id,
            depends_on: node.depends_on,
            created_at: node.created_at.timestamp(),
            updated_at: node.updated_at.timestamp(),
        }
    }
}

fn truncated_objective(value: &str) -> (String, bool) {
    truncate_chars(value, MAX_GOAL_PLAN_RESPONSE_OBJECTIVE_CHARS)
}

fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_string(), false);
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    (truncated, true)
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &i64) -> bool {
    *value == 0
}
