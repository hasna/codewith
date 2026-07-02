use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::Weak;

use codex_protocol::ThreadId;
use codex_protocol::protocol::ThreadGoal;
use codex_protocol::protocol::ThreadGoalStatus;
use codex_protocol::protocol::normalize_thread_goal_title;
use codex_protocol::protocol::validate_thread_goal_objective;

use crate::runtime::GoalRuntimeHandle;
use crate::runtime::PreviousGoalSnapshot;
use crate::tool::fill_empty_thread_preview_if_possible;
use crate::tool::protocol_goal_from_state;
use crate::tool::state_status_from_protocol;
use crate::tool::validate_goal_budget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalServiceError {
    InvalidRequest(String),
    Internal(String),
}

impl fmt::Display for GoalServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) | Self::Internal(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for GoalServiceError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalObjectiveUpdate<'a> {
    Keep,
    Set(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalTitleUpdate<'a> {
    Keep,
    Set(Option<&'a str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalTokenBudgetUpdate {
    Keep,
    Set(Option<i64>),
}

#[derive(Clone, Copy, Debug)]
pub struct GoalSetRequest<'a> {
    pub thread_id: ThreadId,
    pub objective: GoalObjectiveUpdate<'a>,
    pub title: GoalTitleUpdate<'a>,
    pub status: Option<ThreadGoalStatus>,
    pub token_budget: GoalTokenBudgetUpdate,
    pub auto_execute: codex_state::ThreadGoalPlanAutoExecute,
}

#[derive(Clone, Debug)]
pub struct GoalSetOutcome {
    pub goal: ThreadGoal,
    pub plan_update: Option<codex_state::ThreadGoalPlanAdvanceOutcome>,
    state_goal: codex_state::ThreadGoal,
    previous_goal: Option<PreviousGoalSnapshot>,
}

impl GoalSetOutcome {
    pub async fn apply_runtime_effects(&self, goal_service: &GoalService) {
        if let Some(runtime) = goal_service.runtime_for_thread(self.goal.thread_id)
            && let Err(err) = runtime
                .apply_external_goal_set(self.state_goal.clone(), self.previous_goal.clone())
                .await
        {
            tracing::warn!("failed to apply external goal status runtime effects: {err}");
        }
    }
}

#[derive(Clone, Debug)]
pub struct GoalPlanActivateOutcome {
    pub goal: ThreadGoal,
    pub plan: codex_state::ThreadGoalPlanSnapshot,
    state_goal: codex_state::ThreadGoal,
    previous_goal: Option<PreviousGoalSnapshot>,
}

impl GoalPlanActivateOutcome {
    pub async fn apply_runtime_effects(&self, goal_service: &GoalService) {
        if let Some(runtime) = goal_service.runtime_for_thread(self.goal.thread_id)
            && let Err(err) = runtime
                .apply_external_goal_set(self.state_goal.clone(), self.previous_goal.clone())
                .await
        {
            tracing::warn!("failed to apply external goal plan activation runtime effects: {err}");
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GoalPlanAddRequest<'a> {
    pub thread_id: ThreadId,
    pub objective: &'a str,
    pub title: Option<&'a str>,
    pub token_budget: Option<i64>,
    pub auto_execute: codex_state::ThreadGoalPlanAutoExecute,
}

#[derive(Clone, Debug)]
pub struct GoalPlanAddOutcome {
    pub goal: Option<ThreadGoal>,
    pub activated_goal: Option<ThreadGoal>,
    pub plan: codex_state::ThreadGoalPlanSnapshot,
    pub added_node: codex_state::ThreadGoalPlanNode,
    pub created_plan: bool,
    state_goal: Option<codex_state::ThreadGoal>,
    previous_goal: Option<PreviousGoalSnapshot>,
}

impl GoalPlanAddOutcome {
    pub async fn apply_runtime_effects(&self, goal_service: &GoalService) {
        let Some(state_goal) = self.state_goal.clone() else {
            return;
        };
        if let Some(runtime) = goal_service.runtime_for_thread(state_goal.thread_id)
            && let Err(err) = runtime
                .apply_external_goal_set(state_goal, self.previous_goal.clone())
                .await
        {
            tracing::warn!("failed to apply external goal plan add runtime effects: {err}");
        }
    }
}

#[derive(Clone, Debug)]
pub struct GoalClearOutcome {
    pub cleared: bool,
    pub plan_updates: Vec<codex_state::ThreadGoalPlanSnapshot>,
}

#[derive(Debug, Default)]
pub struct GoalService {
    runtimes: Mutex<HashMap<String, Weak<GoalRuntimeHandle>>>,
}

impl GoalService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_thread_goal(
        &self,
        state_db: &codex_state::StateRuntime,
        thread_id: ThreadId,
    ) -> Result<Option<ThreadGoal>, GoalServiceError> {
        state_db
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .map(|goal| goal.map(protocol_goal_from_state))
            .map_err(|err| GoalServiceError::Internal(format!("failed to read thread goal: {err}")))
    }

    pub async fn set_thread_goal(
        &self,
        state_db: &codex_state::StateRuntime,
        request: GoalSetRequest<'_>,
    ) -> Result<GoalSetOutcome, GoalServiceError> {
        let GoalSetRequest {
            thread_id,
            objective,
            title,
            status,
            token_budget,
            auto_execute,
        } = request;
        let status = status.map(state_status_from_protocol);
        let objective = match objective {
            GoalObjectiveUpdate::Keep => None,
            GoalObjectiveUpdate::Set(objective) => Some(objective.trim()),
        };
        let token_budget = match token_budget {
            GoalTokenBudgetUpdate::Keep => None,
            GoalTokenBudgetUpdate::Set(token_budget) => Some(token_budget),
        };
        let title = match title {
            GoalTitleUpdate::Keep => None,
            GoalTitleUpdate::Set(title) => {
                Some(normalize_thread_goal_title(title).map_err(GoalServiceError::InvalidRequest)?)
            }
        };

        if let Some(objective) = objective {
            validate_thread_goal_objective(objective).map_err(GoalServiceError::InvalidRequest)?;
        }
        if objective.is_some() || token_budget.is_some() {
            validate_goal_budget(token_budget.flatten())
                .map_err(GoalServiceError::InvalidRequest)?;
        }

        let runtime = self.runtime_for_thread(thread_id);
        // Hold this through the prepare/write window so idle continuation cannot
        // launch from goal state that this external mutation is about to change.
        let _goal_state_permit = match runtime.as_ref() {
            Some(runtime) => Some(
                runtime
                    .goal_state_permit()
                    .await
                    .map_err(GoalServiceError::Internal)?,
            ),
            None => None,
        };
        if let Some(runtime) = runtime.as_ref()
            && let Err(err) = runtime.prepare_external_goal_mutation().await
        {
            tracing::warn!("failed to prepare external goal mutation: {err}");
        }

        let (goal, previous_goal) = if let Some(objective) = objective {
            let existing_goal = state_db
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .map_err(|err| {
                    GoalServiceError::Internal(format!("failed to read thread goal: {err}"))
                })?;
            if let Some(existing_goal) = existing_goal.as_ref() {
                let previous_goal = PreviousGoalSnapshot::from(existing_goal);
                state_db
                    .thread_goals()
                    .update_thread_goal(
                        thread_id,
                        codex_state::GoalUpdate {
                            objective: Some(objective.to_string()),
                            title: title.clone(),
                            status,
                            token_budget,
                            expected_goal_id: Some(existing_goal.goal_id.clone()),
                        },
                    )
                    .await
                    .map_err(|err| {
                        GoalServiceError::Internal(format!("failed to update thread goal: {err}"))
                    })?
                    .ok_or_else(|| {
                        GoalServiceError::InvalidRequest(format!(
                            "cannot update goal for thread {thread_id}: no goal exists"
                        ))
                    })
                    .map(|goal| (goal, Some(previous_goal)))?
            } else {
                state_db
                    .thread_goals()
                    .replace_thread_goal_with_title(
                        thread_id,
                        objective,
                        title.as_ref().and_then(|title| title.as_deref()),
                        status.unwrap_or(codex_state::ThreadGoalStatus::Active),
                        token_budget.flatten(),
                    )
                    .await
                    .map_err(|err| {
                        GoalServiceError::Internal(format!("failed to replace thread goal: {err}"))
                    })
                    .map(|goal| (goal, None))?
            }
        } else {
            let existing_goal = state_db
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .map_err(|err| {
                    GoalServiceError::Internal(format!("failed to read thread goal: {err}"))
                })?
                .ok_or_else(|| {
                    GoalServiceError::InvalidRequest(format!(
                        "cannot update goal for thread {thread_id}: no goal exists"
                    ))
                })?;
            let previous_goal = PreviousGoalSnapshot::from(&existing_goal);
            let expected_goal_id = existing_goal.goal_id.clone();
            state_db
                .thread_goals()
                .update_thread_goal(
                    thread_id,
                    codex_state::GoalUpdate {
                        objective: None,
                        title,
                        status,
                        token_budget,
                        expected_goal_id: Some(expected_goal_id),
                    },
                )
                .await
                .map_err(|err| {
                    GoalServiceError::Internal(format!("failed to update thread goal: {err}"))
                })?
                .ok_or_else(|| {
                    GoalServiceError::InvalidRequest(format!(
                        "cannot update goal for thread {thread_id}: no goal exists"
                    ))
                })
                .map(|goal| (goal, Some(previous_goal)))?
        };

        if objective.is_some() {
            fill_empty_thread_preview_if_possible(state_db, thread_id, &goal).await;
        }
        let plan_update = if runtime.is_none() {
            sync_external_goal_without_runtime(
                state_db,
                thread_id,
                &goal,
                previous_goal.as_ref(),
                auto_execute,
            )
            .await?
        } else {
            None
        };
        Ok(GoalSetOutcome {
            goal: protocol_goal_from_state(goal.clone()),
            plan_update,
            state_goal: goal,
            previous_goal,
        })
    }

    pub async fn clear_thread_goal(
        &self,
        state_db: &codex_state::StateRuntime,
        thread_id: ThreadId,
    ) -> Result<GoalClearOutcome, GoalServiceError> {
        let runtime = self.runtime_for_thread(thread_id);
        // Hold this through the prepare/write window so idle continuation cannot
        // launch from goal state that this external mutation is about to change.
        let goal_state_permit = match runtime.as_ref() {
            Some(runtime) => Some(
                runtime
                    .goal_state_permit()
                    .await
                    .map_err(GoalServiceError::Internal)?,
            ),
            None => None,
        };
        if let Some(runtime) = runtime.as_ref()
            && let Err(err) = runtime.prepare_external_goal_mutation().await
        {
            tracing::warn!("failed to prepare external goal mutation: {err}");
        }

        let existing_goal = state_db
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to read thread goal: {err}"))
            })?;
        let delete_outcome = state_db
            .thread_goals()
            .delete_thread_goal_with_plan_updates(thread_id)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to clear thread goal: {err}"))
            })?;
        drop(goal_state_permit);
        drop(runtime);

        if delete_outcome.deleted
            && let Some(existing_goal) = existing_goal.as_ref()
            && let Err(err) = crate::pending_interaction::clear_goal_status_waits(
                state_db,
                thread_id,
                existing_goal.goal_id.as_str(),
                "goal cleared",
            )
            .await
        {
            tracing::warn!("failed to clear pending goal interactions after clear: {err}");
        }

        if delete_outcome.deleted
            && let Some(runtime) = self.runtime_for_thread(thread_id)
            && let Err(err) = runtime.apply_external_goal_clear().await
        {
            tracing::warn!("failed to apply external goal clear runtime effects: {err}");
        }

        Ok(GoalClearOutcome {
            cleared: delete_outcome.deleted,
            plan_updates: delete_outcome.plan_updates,
        })
    }

    pub async fn add_thread_goal_to_plan(
        &self,
        state_db: &codex_state::StateRuntime,
        request: GoalPlanAddRequest<'_>,
    ) -> Result<GoalPlanAddOutcome, GoalServiceError> {
        let GoalPlanAddRequest {
            thread_id,
            objective,
            title,
            token_budget,
            auto_execute,
        } = request;
        let objective = objective.trim();
        validate_thread_goal_objective(objective).map_err(GoalServiceError::InvalidRequest)?;
        let title = normalize_thread_goal_title(title).map_err(GoalServiceError::InvalidRequest)?;
        validate_goal_budget(token_budget).map_err(GoalServiceError::InvalidRequest)?;

        let runtime = self.runtime_for_thread(thread_id);
        let _goal_state_permit = match runtime.as_ref() {
            Some(runtime) => Some(
                runtime
                    .goal_state_permit()
                    .await
                    .map_err(GoalServiceError::Internal)?,
            ),
            None => None,
        };
        if let Some(runtime) = runtime.as_ref()
            && let Err(err) = runtime.prepare_external_goal_mutation().await
        {
            tracing::warn!("failed to prepare external goal plan add: {err}");
        }

        let existing_goal = state_db
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to read thread goal: {err}"))
            })?;
        let previous_goal = existing_goal.as_ref().map(PreviousGoalSnapshot::from);
        let outcome = state_db
            .thread_goals()
            .add_thread_goal_to_plan(codex_state::ThreadGoalPlanAddParams {
                thread_id,
                objective: objective.to_string(),
                title,
                token_budget,
                auto_execute,
            })
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to add thread goal to plan: {err}"))
            })?;

        if let Some(activated_goal) = outcome.activated_goal.as_ref() {
            fill_empty_thread_preview_if_possible(state_db, thread_id, activated_goal).await;
        }

        Ok(GoalPlanAddOutcome {
            goal: outcome.goal.map(protocol_goal_from_state),
            activated_goal: outcome.activated_goal.clone().map(protocol_goal_from_state),
            plan: outcome.snapshot,
            added_node: outcome.added_node,
            created_plan: outcome.created_plan,
            state_goal: outcome.activated_goal,
            previous_goal,
        })
    }

    pub fn suppress_next_idle_continuation(&self, thread_id: ThreadId, goal_id: &str) {
        if let Some(runtime) = self.runtime_for_thread(thread_id) {
            runtime.suppress_next_idle_continuation(goal_id);
        }
    }

    pub async fn activate_thread_goal_plan_node(
        &self,
        state_db: &codex_state::StateRuntime,
        thread_id: ThreadId,
        node_id: &str,
    ) -> Result<GoalPlanActivateOutcome, GoalServiceError> {
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Err(GoalServiceError::InvalidRequest(
                "goal plan node id must not be empty".to_string(),
            ));
        }

        let runtime = self.runtime_for_thread(thread_id);
        let _goal_state_permit = match runtime.as_ref() {
            Some(runtime) => Some(
                runtime
                    .goal_state_permit()
                    .await
                    .map_err(GoalServiceError::Internal)?,
            ),
            None => None,
        };
        if let Some(runtime) = runtime.as_ref()
            && let Err(err) = runtime.prepare_external_goal_mutation().await
        {
            tracing::warn!("failed to prepare external goal plan activation: {err}");
        }

        let existing_goal = state_db
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to read thread goal: {err}"))
            })?;
        if existing_goal.as_ref().is_some_and(|goal| {
            !matches!(
                goal.status,
                codex_state::ThreadGoalStatus::Complete
                    | codex_state::ThreadGoalStatus::BudgetLimited
                    | codex_state::ThreadGoalStatus::Deferred
                    | codex_state::ThreadGoalStatus::Cancelled
            )
        }) {
            return Err(GoalServiceError::InvalidRequest(
                "cannot activate a goal plan node while the current goal is still active or stopped resumably"
                    .to_string(),
            ));
        }

        let previous_goal = existing_goal.as_ref().map(PreviousGoalSnapshot::from);
        let outcome = state_db
            .thread_goals()
            .activate_thread_goal_plan_node(thread_id, node_id)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to activate goal plan node: {err}"))
            })?
            .ok_or_else(|| {
                GoalServiceError::InvalidRequest(
                    "cannot activate goal plan node because it is not ready".to_string(),
                )
            })?;
        let state_goal = outcome.activated_goal.ok_or_else(|| {
            GoalServiceError::Internal(
                "goal plan activation completed without an activated goal".to_string(),
            )
        })?;
        fill_empty_thread_preview_if_possible(state_db, thread_id, &state_goal).await;
        Ok(GoalPlanActivateOutcome {
            goal: protocol_goal_from_state(state_goal.clone()),
            plan: outcome.snapshot,
            state_goal,
            previous_goal,
        })
    }

    pub(crate) fn register_runtime(&self, runtime: &Arc<GoalRuntimeHandle>) {
        self.runtimes()
            .insert(runtime.thread_id().to_string(), Arc::downgrade(runtime));
    }

    pub(crate) fn unregister_runtime(&self, runtime: &Arc<GoalRuntimeHandle>) {
        let key = runtime.thread_id().to_string();
        let runtime = Arc::downgrade(runtime);
        let mut runtimes = self.runtimes();
        if runtimes
            .get(&key)
            .is_some_and(|registered| registered.ptr_eq(&runtime))
        {
            runtimes.remove(&key);
        }
    }

    fn runtime_for_thread(&self, thread_id: ThreadId) -> Option<Arc<GoalRuntimeHandle>> {
        let key = thread_id.to_string();
        let mut runtimes = self.runtimes();
        let runtime = runtimes.get(&key).and_then(Weak::upgrade);
        if runtime.is_none() {
            runtimes.remove(&key);
        }
        runtime
    }

    fn runtimes(&self) -> std::sync::MutexGuard<'_, HashMap<String, Weak<GoalRuntimeHandle>>> {
        self.runtimes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

async fn sync_external_goal_without_runtime(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
    goal: &codex_state::ThreadGoal,
    previous_goal: Option<&PreviousGoalSnapshot>,
    auto_execute: codex_state::ThreadGoalPlanAutoExecute,
) -> Result<Option<codex_state::ThreadGoalPlanAdvanceOutcome>, GoalServiceError> {
    if matches!(
        goal.status,
        codex_state::ThreadGoalStatus::Active
            | codex_state::ThreadGoalStatus::Deferred
            | codex_state::ThreadGoalStatus::Complete
    ) && let Some(previous_goal) = previous_goal
        && matches!(
            previous_goal.status,
            codex_state::ThreadGoalStatus::Blocked | codex_state::ThreadGoalStatus::UsageLimited
        )
        && let Err(err) = crate::pending_interaction::clear_goal_status_waits(
            state_db,
            thread_id,
            previous_goal.goal_id.as_str(),
            "goal resumed",
        )
        .await
    {
        tracing::warn!("failed to clear pending goal interactions without runtime: {err}");
    }

    if matches!(
        goal.status,
        codex_state::ThreadGoalStatus::Blocked | codex_state::ThreadGoalStatus::UsageLimited
    ) && previous_goal.is_none_or(|previous_goal| previous_goal.status != goal.status)
    {
        let reason = match goal.status {
            codex_state::ThreadGoalStatus::Blocked => "external-goal-blocked",
            codex_state::ThreadGoalStatus::UsageLimited => "external-goal-usage-limit",
            codex_state::ThreadGoalStatus::Active
            | codex_state::ThreadGoalStatus::Paused
            | codex_state::ThreadGoalStatus::BudgetLimited
            | codex_state::ThreadGoalStatus::Deferred
            | codex_state::ThreadGoalStatus::Complete
            | codex_state::ThreadGoalStatus::Cancelled => unreachable!("status matched above"),
        };
        if let Err(err) = crate::pending_interaction::record_goal_status_wait(
            state_db, thread_id, goal, /*turn_id*/ None, reason,
        )
        .await
        {
            tracing::warn!("failed to record pending goal interaction without runtime: {err}");
        }
    }

    let plan_update = match goal.status {
        codex_state::ThreadGoalStatus::Complete => state_db
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(thread_id, goal, auto_execute)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to advance goal plan: {err}"))
            })?,
        codex_state::ThreadGoalStatus::Deferred => state_db
            .thread_goals()
            .defer_goal_plan_node_and_maybe_advance(thread_id, goal, auto_execute)
            .await
            .map_err(|err| {
                GoalServiceError::Internal(format!("failed to advance deferred goal plan: {err}"))
            })?,
        codex_state::ThreadGoalStatus::Active
        | codex_state::ThreadGoalStatus::Paused
        | codex_state::ThreadGoalStatus::Blocked
        | codex_state::ThreadGoalStatus::UsageLimited
        | codex_state::ThreadGoalStatus::BudgetLimited
        | codex_state::ThreadGoalStatus::Cancelled => state_db
            .thread_goals()
            .sync_goal_plan_node_for_goal(thread_id, goal)
            .await
            .map_err(|err| GoalServiceError::Internal(format!("failed to sync goal plan: {err}")))?
            .map(|snapshot| codex_state::ThreadGoalPlanAdvanceOutcome {
                snapshot,
                activated_goal: None,
            }),
    };
    Ok(plan_update)
}
