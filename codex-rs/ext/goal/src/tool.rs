use std::sync::Arc;

use async_trait::async_trait;
use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_protocol::ThreadId;
use codex_protocol::protocol::ThreadGoal;
use codex_protocol::protocol::ThreadGoalStatus;
use codex_protocol::protocol::derive_thread_goal_title_from_objective;
use codex_protocol::protocol::normalize_thread_goal_title;
use codex_protocol::protocol::validate_thread_goal_objective;
use serde::Deserialize;
use serde::Serialize;

use crate::accounting::BudgetLimitedGoalDisposition;
use crate::accounting::GoalAccountingState;
use crate::events::GoalEventEmitter;
use crate::metrics::GoalMetrics;
use crate::runtime::GoalPlanRuntimeConfigHandle;
use crate::spec::ACTIVATE_GOAL_PLAN_NODE_TOOL_NAME;
use crate::spec::CREATE_GOAL_PLAN_TOOL_NAME;
use crate::spec::CREATE_GOAL_TOOL_NAME;
use crate::spec::GET_GOAL_PLAN_TOOL_NAME;
use crate::spec::GET_GOAL_TOOL_NAME;
use crate::spec::RESUME_GOAL_TOOL_NAME;
use crate::spec::UPDATE_GOAL_TOOL_NAME;
use crate::spec::create_activate_goal_plan_node_tool;
use crate::spec::create_create_goal_plan_tool;
use crate::spec::create_create_goal_tool;
use crate::spec::create_get_goal_plan_tool;
use crate::spec::create_get_goal_tool;
use crate::spec::create_resume_goal_tool;
use crate::spec::create_update_goal_tool;
use crate::tool_plan::GoalPlanCompletionReport;
use crate::tool_plan::GoalPlanResponse;

const MAX_GOAL_TOOL_RESPONSE_PLANS: usize = 4;
const MAX_GOAL_TOOL_OBJECTIVE_CHARS: usize = 512;

#[derive(Clone)]
pub(crate) struct GoalToolExecutor {
    kind: GoalToolKind,
    pub(crate) thread_id: ThreadId,
    pub(crate) state_db: Arc<codex_state::StateRuntime>,
    pub(crate) accounting_state: Arc<GoalAccountingState>,
    pub(crate) event_emitter: GoalEventEmitter,
    pub(crate) metrics: GoalMetrics,
    pub(crate) plan_config: Option<GoalPlanRuntimeConfigHandle>,
}

#[derive(Clone, Copy)]
enum GoalToolKind {
    Get,
    Create,
    GetPlan,
    CreatePlan,
    ActivatePlanNode,
    Update,
    Resume,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateGoalRequest {
    pub objective: String,
    pub title: Option<String>,
    pub token_budget: Option<i64>,
    pub post_goal_context: Option<PostGoalContextActionArg>,
    #[serde(default)]
    pub clear_existing_goal: bool,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum PostGoalContextActionArg {
    Keep,
    Compact,
}

impl From<PostGoalContextActionArg> for codex_state::PostGoalContextAction {
    fn from(value: PostGoalContextActionArg) -> Self {
        match value {
            PostGoalContextActionArg::Keep => Self::Keep,
            PostGoalContextActionArg::Compact => Self::Compact,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct UpdateGoalArgs {
    status: ThreadGoalStatus,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalToolResponse {
    goal: Option<ThreadGoal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activated_goal: Option<ThreadGoal>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    goal_plans: Vec<GoalPlanResponse>,
    #[serde(skip_serializing_if = "is_zero")]
    goal_plans_omitted_count: i64,
    remaining_tokens: Option<i64>,
    completion_budget_report: Option<String>,
    goal_plan_completion_report: Option<GoalPlanCompletionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_lifecycle_report: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum CompletionBudgetReport {
    Include,
    Omit,
}

impl GoalToolExecutor {
    pub(crate) fn get(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
    ) -> Self {
        Self {
            kind: GoalToolKind::Get,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: None,
        }
    }

    pub(crate) fn create(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
    ) -> Self {
        Self {
            kind: GoalToolKind::Create,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: None,
        }
    }

    pub(crate) fn get_plan(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
        plan_config: GoalPlanRuntimeConfigHandle,
    ) -> Self {
        Self {
            kind: GoalToolKind::GetPlan,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: Some(plan_config),
        }
    }

    pub(crate) fn create_plan(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
        plan_config: GoalPlanRuntimeConfigHandle,
    ) -> Self {
        Self {
            kind: GoalToolKind::CreatePlan,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: Some(plan_config),
        }
    }

    pub(crate) fn activate_plan_node(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
        plan_config: GoalPlanRuntimeConfigHandle,
    ) -> Self {
        Self {
            kind: GoalToolKind::ActivatePlanNode,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: Some(plan_config),
        }
    }

    pub(crate) fn update(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
        plan_config: GoalPlanRuntimeConfigHandle,
    ) -> Self {
        Self {
            kind: GoalToolKind::Update,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: Some(plan_config),
        }
    }

    pub(crate) fn resume(
        thread_id: ThreadId,
        state_db: Arc<codex_state::StateRuntime>,
        accounting_state: Arc<GoalAccountingState>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
    ) -> Self {
        Self {
            kind: GoalToolKind::Resume,
            thread_id,
            state_db,
            accounting_state,
            event_emitter,
            metrics,
            plan_config: None,
        }
    }
}

#[async_trait]
impl ToolExecutor<ToolCall> for GoalToolExecutor {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(match self.kind {
            GoalToolKind::Get => GET_GOAL_TOOL_NAME,
            GoalToolKind::Create => CREATE_GOAL_TOOL_NAME,
            GoalToolKind::GetPlan => GET_GOAL_PLAN_TOOL_NAME,
            GoalToolKind::CreatePlan => CREATE_GOAL_PLAN_TOOL_NAME,
            GoalToolKind::ActivatePlanNode => ACTIVATE_GOAL_PLAN_NODE_TOOL_NAME,
            GoalToolKind::Update => UPDATE_GOAL_TOOL_NAME,
            GoalToolKind::Resume => RESUME_GOAL_TOOL_NAME,
        })
    }

    fn spec(&self) -> ToolSpec {
        match self.kind {
            GoalToolKind::Get => create_get_goal_tool(),
            GoalToolKind::Create => create_create_goal_tool(),
            GoalToolKind::GetPlan => create_get_goal_plan_tool(),
            GoalToolKind::CreatePlan => create_create_goal_plan_tool(),
            GoalToolKind::ActivatePlanNode => create_activate_goal_plan_node_tool(),
            GoalToolKind::Update => create_update_goal_tool(),
            GoalToolKind::Resume => create_resume_goal_tool(),
        }
    }

    async fn handle(&self, invocation: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        match self.kind {
            GoalToolKind::Get => self.handle_get(invocation).await,
            GoalToolKind::Create => self.handle_create(invocation).await,
            GoalToolKind::GetPlan => self.handle_get_plan(invocation).await,
            GoalToolKind::CreatePlan => self.handle_create_plan(invocation).await,
            GoalToolKind::ActivatePlanNode => self.handle_activate_plan_node(invocation).await,
            GoalToolKind::Update => self.handle_update(invocation).await,
            GoalToolKind::Resume => self.handle_resume(invocation).await,
        }
    }
}

impl GoalToolExecutor {
    async fn handle_get(
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
        goal_response(goal, CompletionBudgetReport::Omit)
    }

    async fn handle_create(
        &self,
        invocation: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let mut request: CreateGoalRequest = parse_arguments(invocation.function_arguments()?)?;
        request.objective = request.objective.trim().to_string();
        validate_thread_goal_objective(&request.objective)
            .map_err(FunctionCallError::RespondToModel)?;
        let title = normalize_thread_goal_title(request.title.as_deref())
            .map_err(FunctionCallError::RespondToModel)?
            .unwrap_or_else(|| derive_thread_goal_title_from_objective(&request.objective));
        validate_goal_budget(request.token_budget).map_err(FunctionCallError::RespondToModel)?;

        let existing_goal = if request.clear_existing_goal {
            self.state_db
                .thread_goals()
                .get_thread_goal(self.thread_id)
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to read goal: {err}"))
                })?
        } else {
            None
        };
        if request.clear_existing_goal {
            self.account_active_goal_progress(
                codex_state::GoalAccountingMode::ActiveOnly,
                invocation.call_id.as_str(),
                BudgetLimitedGoalDisposition::ClearActive,
            )
            .await?;
            self.mark_existing_plan_goal_replaced(existing_goal).await?;
        }
        let goal = if request.clear_existing_goal {
            self.state_db
                .thread_goals()
                .replace_thread_goal_with_title(
                    self.thread_id,
                    request.objective.as_str(),
                    Some(title.as_str()),
                    codex_state::ThreadGoalStatus::Active,
                    request.token_budget,
                )
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to create goal: {err}"))
                })?
        } else {
            self.state_db
                .thread_goals()
                .insert_thread_goal_with_title(
                    self.thread_id,
                    request.objective.as_str(),
                    Some(title.as_str()),
                    codex_state::ThreadGoalStatus::Active,
                    request.token_budget,
                )
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to create goal: {err}"))
                })?
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "cannot create a new goal because this thread already has a goal; set clear_existing_goal to true only when explicitly instructed to replace or start a new goal"
                            .to_string(),
                    )
                })?
        };
        fill_empty_thread_preview_if_possible(self.state_db.as_ref(), self.thread_id, &goal).await;
        if let Some(action) = request.post_goal_context {
            self.state_db
                .thread_goals()
                .set_thread_goal_context_action(self.thread_id, &goal.goal_id, action.into())
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to set goal context lifecycle policy: {err}"
                    ))
                })?;
        }
        let turn_id = self
            .accounting_state
            .mark_current_turn_goal_active(goal.goal_id.clone());
        self.metrics.record_created();
        let goal = protocol_goal_from_state(goal);
        self.emit_goal_updated_from_tool_call(&invocation, turn_id, goal.clone());
        goal_response(Some(goal), CompletionBudgetReport::Omit)
    }

    async fn handle_update(
        &self,
        invocation: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let args: UpdateGoalArgs = parse_arguments(invocation.function_arguments()?)?;
        if !matches!(
            args.status,
            ThreadGoalStatus::Complete
                | ThreadGoalStatus::Blocked
                | ThreadGoalStatus::Deferred
                | ThreadGoalStatus::Cancelled
        ) {
            return Err(FunctionCallError::RespondToModel(
                "update_goal can only mark the existing goal complete, blocked, deferred, or cancelled; pause, resume, budget-limited, and usage-limited status changes are controlled by the user or system"
                    .to_string(),
            ));
        }

        let expected_goal_id = self
            .accounting_state
            .active_goal_id_for_current_turn(invocation.turn_id.as_str())
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "cannot update goal because this tool call is no longer associated with the active goal turn"
                        .to_string(),
                )
            })?;

        self.account_active_goal_progress_for_turn(
            invocation.turn_id.as_str(),
            match args.status {
                ThreadGoalStatus::Complete => codex_state::GoalAccountingMode::ActiveOrComplete,
                ThreadGoalStatus::Blocked
                | ThreadGoalStatus::Deferred
                | ThreadGoalStatus::Cancelled => codex_state::GoalAccountingMode::ActiveOrStopped,
                ThreadGoalStatus::Active
                | ThreadGoalStatus::Paused
                | ThreadGoalStatus::UsageLimited
                | ThreadGoalStatus::BudgetLimited => unreachable!("status validated above"),
            },
            invocation.call_id.as_str(),
            BudgetLimitedGoalDisposition::ClearActive,
        )
        .await?;
        let previous_status = self
            .current_goal_status_for_metrics(Some(expected_goal_id.as_str()))
            .await?;
        let goal = self
            .state_db
            .thread_goals()
            .update_thread_goal(
                self.thread_id,
                codex_state::GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(state_status_from_protocol(args.status)),
                    token_budget: None,
                    expected_goal_id: Some(expected_goal_id),
                },
            )
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to update goal: {err}"))
            })?
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "cannot update goal because this thread has no goal".to_string(),
                )
            })?;
        if goal.status == codex_state::ThreadGoalStatus::Blocked {
            crate::pending_interaction::record_goal_status_wait(
                self.state_db.as_ref(),
                self.thread_id,
                &goal,
                Some(invocation.turn_id.as_str()),
                "update-goal-blocked",
            )
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to record blocked goal pending interaction: {err}"
                ))
            })?;
        }
        self.metrics
            .record_terminal_if_status_changed(previous_status, &goal);
        let plan_outcome = match args.status {
            ThreadGoalStatus::Complete => {
                let plan_config = self.plan_config.as_ref().ok_or_else(|| {
                    FunctionCallError::Fatal("goal update tool missing runtime config".to_string())
                })?;
                self.state_db
                    .thread_goals()
                    .complete_goal_plan_node_and_maybe_advance(
                        self.thread_id,
                        &goal,
                        plan_config.current().auto_execute,
                    )
                    .await
                    .map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "failed to advance goal plan: {err}"
                        ))
                    })?
            }
            ThreadGoalStatus::Deferred => {
                let plan_config = self.plan_config.as_ref().ok_or_else(|| {
                    FunctionCallError::Fatal("goal update tool missing runtime config".to_string())
                })?;
                self.state_db
                    .thread_goals()
                    .defer_goal_plan_node_and_maybe_advance(
                        self.thread_id,
                        &goal,
                        plan_config.current().auto_execute,
                    )
                    .await
                    .map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "failed to advance deferred goal plan: {err}"
                        ))
                    })?
            }
            _ => self
                .state_db
                .thread_goals()
                .sync_goal_plan_node_for_goal(self.thread_id, &goal)
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to sync goal plan: {err}"))
                })?
                .map(|snapshot| codex_state::ThreadGoalPlanAdvanceOutcome {
                    snapshot,
                    activated_goal: None,
                }),
        };
        let context_lifecycle_report = if args.status == ThreadGoalStatus::Complete {
            let plan_config = self.plan_config.as_ref().ok_or_else(|| {
                FunctionCallError::Fatal("goal update tool missing runtime config".to_string())
            })?;
            plan_config
                .apply_post_completion_context_policy(&goal, plan_outcome.as_ref())
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to apply post-goal context lifecycle policy: {err}"
                    ))
                })?
        } else {
            None
        };
        let goal = protocol_goal_from_state(goal);
        let turn_id = self.accounting_state.clear_current_turn_goal();
        self.emit_goal_updated_from_tool_call(&invocation, turn_id, goal.clone());
        let (goal_plans, activated_goal, goal_plan_completion_report) = if let Some(outcome) =
            plan_outcome
        {
            self.event_emitter.thread_goal_plan_updated(
                format!("{}-goal-plan", invocation.call_id),
                Some(invocation.turn_id.clone()),
                outcome.snapshot.clone(),
            );
            let goal_plan_completion_report = GoalPlanCompletionReport::from_snapshot_if_terminal(
                &outcome.snapshot,
                self.thread_id,
            );
            let activated_goal = self
                .apply_activated_goal_from_plan(&invocation, outcome.activated_goal)
                .await?;
            (
                vec![GoalPlanResponse::from_snapshot_for_thread(
                    outcome.snapshot,
                    self.thread_id,
                )],
                activated_goal,
                goal_plan_completion_report,
            )
        } else {
            (Vec::new(), None, None)
        };
        goal_response_with_plan_report_and_context(
            Some(goal),
            activated_goal,
            goal_plans,
            if args.status == ThreadGoalStatus::Complete {
                CompletionBudgetReport::Include
            } else {
                CompletionBudgetReport::Omit
            },
            goal_plan_completion_report,
            context_lifecycle_report,
        )
    }

    /// Attempt to resume a deferred goal-plan node for this thread when the
    /// current goal is no longer resumable (for example it completed or was
    /// cancelled) but the active plan still holds a deferred node. This lets an
    /// explicit user resume revive a node that was set aside earlier, so the
    /// plan can continue to its downstream dependents without discarding plan
    /// history. Returns `Ok(None)` when there is no resumable deferred node.
    async fn try_resume_deferred_plan_node(
        &self,
        invocation: &ToolCall,
    ) -> Result<Option<Box<dyn ToolOutput>>, FunctionCallError> {
        let Some(outcome) = self
            .state_db
            .thread_goals()
            .resume_deferred_goal_plan_node(self.thread_id, None)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to resume deferred goal plan node: {err}"
                ))
            })?
        else {
            return Ok(None);
        };
        self.event_emitter.thread_goal_plan_updated(
            format!("{}-goal-plan", invocation.call_id),
            Some(invocation.turn_id.clone()),
            outcome.snapshot.clone(),
        );
        let activated_goal = self
            .apply_activated_goal_from_plan(invocation, outcome.activated_goal)
            .await?;
        let goal_plans = vec![GoalPlanResponse::from_snapshot_for_thread(
            outcome.snapshot,
            self.thread_id,
        )];
        let response = goal_response_with_plan(
            activated_goal.clone(),
            activated_goal,
            goal_plans,
            CompletionBudgetReport::Omit,
        )?;
        Ok(Some(response))
    }

    async fn handle_resume(
        &self,
        invocation: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let _ = invocation.function_arguments()?;

        self.account_active_goal_progress(
            codex_state::GoalAccountingMode::ActiveOnly,
            invocation.call_id.as_str(),
            BudgetLimitedGoalDisposition::ClearActive,
        )
        .await?;
        let existing_goal = self
            .state_db
            .thread_goals()
            .get_thread_goal(self.thread_id)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read goal: {err}"))
            })?
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "cannot resume goal because this thread has no goal".to_string(),
                )
            })?;
        match existing_goal.status {
            codex_state::ThreadGoalStatus::Paused
            | codex_state::ThreadGoalStatus::Blocked
            | codex_state::ThreadGoalStatus::Deferred
            | codex_state::ThreadGoalStatus::UsageLimited => {}
            codex_state::ThreadGoalStatus::BudgetLimited => {
                self.accounting_state.clear_active_goal();
                return Err(FunctionCallError::RespondToModel(
                    "cannot resume a budget-limited goal without changing its token budget"
                        .to_string(),
                ));
            }
            codex_state::ThreadGoalStatus::Active => {
                return Err(FunctionCallError::RespondToModel(
                    "cannot resume goal because it is already active".to_string(),
                ));
            }
            codex_state::ThreadGoalStatus::Complete => {
                if let Some(response) = self.try_resume_deferred_plan_node(&invocation).await? {
                    return Ok(response);
                }
                return Err(FunctionCallError::RespondToModel(
                    "cannot resume a completed goal; create a new goal only when explicitly requested"
                        .to_string(),
                ));
            }
            codex_state::ThreadGoalStatus::Cancelled => {
                if let Some(response) = self.try_resume_deferred_plan_node(&invocation).await? {
                    return Ok(response);
                }
                return Err(FunctionCallError::RespondToModel(
                    "cannot resume a cancelled goal; create a new goal only when explicitly requested"
                        .to_string(),
                ));
            }
        }

        let previous_status = existing_goal.status;
        let goal = self
            .state_db
            .thread_goals()
            .update_thread_goal(
                self.thread_id,
                codex_state::GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(codex_state::ThreadGoalStatus::Active),
                    token_budget: None,
                    expected_goal_id: Some(existing_goal.goal_id.clone()),
                },
            )
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to resume goal: {err}"))
            })?
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "cannot resume goal because this thread has no goal".to_string(),
                )
            })?;
        self.metrics
            .record_resumed_if_status_changed(Some(previous_status), goal.status);
        crate::pending_interaction::clear_goal_status_waits(
            self.state_db.as_ref(),
            self.thread_id,
            existing_goal.goal_id.as_str(),
            "goal resumed",
        )
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to clear goal pending interactions: {err}"
            ))
        })?;
        self.state_db
            .thread_goals()
            .sync_goal_plan_node_for_goal(self.thread_id, &goal)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to sync resumed goal plan: {err}"
                ))
            })?;
        let turn_id = self
            .accounting_state
            .mark_current_turn_goal_active(goal.goal_id.clone());
        if turn_id.is_none() {
            self.accounting_state
                .mark_idle_goal_active(goal.goal_id.clone());
        }
        let goal = protocol_goal_from_state(goal);
        self.emit_goal_updated_from_tool_call(&invocation, turn_id, goal.clone());
        goal_response(Some(goal), CompletionBudgetReport::Omit)
    }

    fn emit_goal_updated_from_tool_call(
        &self,
        invocation: &ToolCall,
        turn_id: Option<String>,
        goal: ThreadGoal,
    ) {
        self.event_emitter
            .thread_goal_updated(invocation.call_id.clone(), turn_id, goal);
    }

    pub(crate) async fn account_active_goal_progress(
        &self,
        mode: codex_state::GoalAccountingMode,
        event_id: &str,
        budget_limited_goal_disposition: BudgetLimitedGoalDisposition,
    ) -> Result<Option<ThreadGoal>, FunctionCallError> {
        let Some(turn_id) = self.accounting_state.current_turn_id() else {
            return Ok(None);
        };
        self.account_active_goal_progress_for_turn(
            turn_id.as_str(),
            mode,
            event_id,
            budget_limited_goal_disposition,
        )
        .await
    }

    pub(crate) async fn account_active_goal_progress_for_turn(
        &self,
        turn_id: &str,
        mode: codex_state::GoalAccountingMode,
        event_id: &str,
        budget_limited_goal_disposition: BudgetLimitedGoalDisposition,
    ) -> Result<Option<ThreadGoal>, FunctionCallError> {
        let _accounting_permit = self
            .accounting_state
            .progress_accounting_permit()
            .await
            .map_err(|err| {
                FunctionCallError::Fatal(format!(
                    "goal progress accounting semaphore closed: {err}"
                ))
            })?;
        let Some(snapshot) = self.accounting_state.progress_snapshot(turn_id) else {
            return Ok(None);
        };
        let previous_status = self
            .current_goal_status_for_metrics(Some(snapshot.expected_goal_id.as_str()))
            .await?;
        let outcome = self
            .state_db
            .thread_goals()
            .account_thread_goal_usage(
                self.thread_id,
                snapshot.time_delta_seconds,
                snapshot.token_delta,
                mode,
                Some(snapshot.expected_goal_id.as_str()),
            )
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to account goal progress: {err}"))
            })?;
        Ok(match outcome {
            codex_state::GoalAccountingOutcome::Updated(goal) => {
                self.metrics
                    .record_terminal_if_status_changed(previous_status, &goal);
                let plan_snapshot = self
                    .state_db
                    .thread_goals()
                    .sync_goal_plan_node_for_goal(self.thread_id, &goal)
                    .await
                    .map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "failed to sync goal plan progress: {err}"
                        ))
                    })?;
                if let Some(snapshot) = plan_snapshot {
                    self.event_emitter.thread_goal_plan_updated(
                        format!("{event_id}-goal-plan"),
                        Some(turn_id.to_string()),
                        snapshot,
                    );
                }
                self.accounting_state.mark_progress_accounted_for_status(
                    turn_id,
                    &snapshot,
                    goal.status,
                    budget_limited_goal_disposition,
                );
                let goal = protocol_goal_from_state(goal);
                self.event_emitter.thread_goal_updated(
                    event_id.to_string(),
                    Some(turn_id.to_string()),
                    goal.clone(),
                );
                Some(goal)
            }
            codex_state::GoalAccountingOutcome::Unchanged(_) => None,
        })
    }

    async fn current_goal_status_for_metrics(
        &self,
        expected_goal_id: Option<&str>,
    ) -> Result<Option<codex_state::ThreadGoalStatus>, FunctionCallError> {
        let goal = self
            .state_db
            .thread_goals()
            .get_thread_goal(self.thread_id)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to read goal metrics status: {err}"
                ))
            })?;
        Ok(goal.and_then(|goal| {
            expected_goal_id
                .is_none_or(|expected_goal_id| goal.goal_id == expected_goal_id)
                .then_some(goal.status)
        }))
    }
}

pub(crate) fn parse_arguments<T>(arguments: &str) -> Result<T, FunctionCallError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(arguments)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

pub(crate) fn validate_goal_budget(value: Option<i64>) -> Result<(), String> {
    if let Some(value) = value
        && value <= 0
    {
        return Err("goal budgets must be positive when provided".to_string());
    }
    Ok(())
}

fn goal_response(
    goal: Option<ThreadGoal>,
    completion_budget_report: CompletionBudgetReport,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    goal_response_with_plan(
        goal,
        /*activated_goal*/ None,
        Vec::new(),
        completion_budget_report,
    )
}

pub(crate) fn goal_response_with_plan(
    goal: Option<ThreadGoal>,
    activated_goal: Option<ThreadGoal>,
    goal_plans: Vec<GoalPlanResponse>,
    completion_budget_report: CompletionBudgetReport,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    goal_response_with_plan_and_report(
        goal,
        activated_goal,
        goal_plans,
        completion_budget_report,
        /*goal_plan_completion_report*/ None,
    )
}

pub(crate) fn goal_response_with_plan_and_report(
    goal: Option<ThreadGoal>,
    activated_goal: Option<ThreadGoal>,
    goal_plans: Vec<GoalPlanResponse>,
    completion_budget_report: CompletionBudgetReport,
    goal_plan_completion_report: Option<GoalPlanCompletionReport>,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    goal_response_with_plan_report_and_context(
        goal,
        activated_goal,
        goal_plans,
        completion_budget_report,
        goal_plan_completion_report,
        /*context_lifecycle_report*/ None,
    )
}

pub(crate) fn goal_response_with_plan_report_and_context(
    goal: Option<ThreadGoal>,
    activated_goal: Option<ThreadGoal>,
    goal_plans: Vec<GoalPlanResponse>,
    completion_budget_report: CompletionBudgetReport,
    goal_plan_completion_report: Option<GoalPlanCompletionReport>,
    context_lifecycle_report: Option<String>,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let value = serde_json::to_value(GoalToolResponse::new(
        goal,
        activated_goal,
        goal_plans,
        completion_budget_report,
        goal_plan_completion_report,
        context_lifecycle_report,
    ))
    .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
    Ok(Box::new(JsonToolOutput::new(value)))
}

impl GoalToolResponse {
    fn new(
        goal: Option<ThreadGoal>,
        activated_goal: Option<ThreadGoal>,
        goal_plans: Vec<GoalPlanResponse>,
        report_mode: CompletionBudgetReport,
        goal_plan_completion_report: Option<GoalPlanCompletionReport>,
        context_lifecycle_report: Option<String>,
    ) -> Self {
        let remaining_tokens = goal.as_ref().and_then(|goal| {
            goal.token_budget
                .map(|budget| (budget - goal.tokens_used).max(0))
        });
        let completion_budget_report = match report_mode {
            CompletionBudgetReport::Include => goal
                .as_ref()
                .filter(|goal| goal.status == ThreadGoalStatus::Complete)
                .and_then(completion_budget_report),
            CompletionBudgetReport::Omit => None,
        };
        let goal_plans_omitted_count = goal_plans
            .len()
            .saturating_sub(MAX_GOAL_TOOL_RESPONSE_PLANS)
            as i64;
        Self {
            goal: goal.map(bounded_goal_for_tool_response),
            activated_goal: activated_goal.map(bounded_goal_for_tool_response),
            goal_plans: goal_plans
                .into_iter()
                .take(MAX_GOAL_TOOL_RESPONSE_PLANS)
                .collect(),
            goal_plans_omitted_count,
            remaining_tokens,
            completion_budget_report,
            goal_plan_completion_report,
            context_lifecycle_report,
        }
    }
}

fn bounded_goal_for_tool_response(goal: ThreadGoal) -> ThreadGoal {
    ThreadGoal {
        objective: truncate_chars(goal.objective, MAX_GOAL_TOOL_OBJECTIVE_CHARS),
        ..goal
    }
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &i64) -> bool {
    *value == 0
}

pub(crate) async fn fill_empty_thread_preview_if_possible(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal: &codex_state::ThreadGoal,
) {
    if let Err(err) = state_db
        .set_thread_preview_if_empty(thread_id, goal.objective.as_str())
        .await
    {
        tracing::warn!(
            "failed to set empty thread preview from goal objective for {thread_id}: {err}"
        );
    }
}

pub(crate) fn protocol_goal_from_state(goal: codex_state::ThreadGoal) -> ThreadGoal {
    ThreadGoal {
        thread_id: goal.thread_id,
        goal_id: goal.goal_id,
        objective: goal.objective,
        title: goal.title,
        status: protocol_status_from_state(goal.status),
        token_budget: goal.token_budget,
        tokens_used: goal.tokens_used,
        time_used_seconds: goal.time_used_seconds,
        created_at: goal.created_at.timestamp(),
        updated_at: goal.updated_at.timestamp(),
    }
}

fn protocol_status_from_state(status: codex_state::ThreadGoalStatus) -> ThreadGoalStatus {
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

pub(crate) fn state_status_from_protocol(
    status: ThreadGoalStatus,
) -> codex_state::ThreadGoalStatus {
    match status {
        ThreadGoalStatus::Active => codex_state::ThreadGoalStatus::Active,
        ThreadGoalStatus::Paused => codex_state::ThreadGoalStatus::Paused,
        ThreadGoalStatus::Blocked => codex_state::ThreadGoalStatus::Blocked,
        ThreadGoalStatus::UsageLimited => codex_state::ThreadGoalStatus::UsageLimited,
        ThreadGoalStatus::BudgetLimited => codex_state::ThreadGoalStatus::BudgetLimited,
        ThreadGoalStatus::Deferred => codex_state::ThreadGoalStatus::Deferred,
        ThreadGoalStatus::Complete => codex_state::ThreadGoalStatus::Complete,
        ThreadGoalStatus::Cancelled => codex_state::ThreadGoalStatus::Cancelled,
    }
}

fn completion_budget_report(goal: &ThreadGoal) -> Option<String> {
    if goal.token_budget.is_none() && goal.time_used_seconds <= 0 {
        None
    } else {
        Some(
            "Goal achieved. Report final usage from this tool result's structured goal fields. If `goal.tokenBudget` is present, include token usage from `goal.tokensUsed` and `goal.tokenBudget`. If `goal.timeUsedSeconds` is greater than 0, summarize elapsed time in a concise, human-friendly form appropriate to the response language."
                .to_string(),
        )
    }
}
