use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_core::ThreadManager;
use codex_protocol::ThreadId;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ThreadGoal;

use crate::accounting::BudgetLimitedGoalDisposition;
use crate::accounting::GoalAccountingState;
use crate::events::GoalEventEmitter;
use crate::metrics::GoalMetrics;
use crate::steering::continuation_steering_item;
use crate::steering::objective_updated_steering_item;
use crate::tool::protocol_goal_from_state;
use tokio::sync::Semaphore;
use tokio::sync::SemaphorePermit;

#[derive(Clone)]
pub struct GoalRuntimeHandle {
    inner: Arc<GoalRuntimeInner>,
}

pub(crate) struct GoalRuntimeConfig {
    pub(crate) enabled: bool,
    pub(crate) tools_available_for_thread: bool,
    pub(crate) auto_execute: codex_state::ThreadGoalPlanAutoExecute,
    pub(crate) max_auto_goals_per_plan: usize,
    pub(crate) max_tokens_per_goal_plan: Option<i64>,
    pub(crate) max_goal_plan_node_objective_chars: usize,
    pub(crate) post_goal_context: codex_state::PostGoalContextAction,
    pub(crate) post_goal_plan_context: codex_state::PostGoalContextAction,
}

pub(crate) enum ActiveGoalStopReason {
    TurnError {
        error: CodexErrorInfo,
        fingerprint: String,
    },
    UsageLimit,
}

const REQUIRED_CONSECUTIVE_BLOCKER_TURNS: u8 = 3;

struct GoalRuntimeInner {
    thread_id: ThreadId,
    state_dbs: Arc<codex_state::StateRuntime>,
    event_emitter: GoalEventEmitter,
    metrics: GoalMetrics,
    thread_manager: Weak<ThreadManager>,
    accounting_state: Arc<GoalAccountingState>,
    enabled: AtomicBool,
    tools_available_for_thread: bool,
    plan_config: std::sync::RwLock<GoalPlanRuntimeConfig>,
    goal_state_lock: Semaphore,
    suppressed_idle_continuations: Mutex<HashSet<String>>,
    pending_context_compaction: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GoalPlanRuntimeConfig {
    pub(crate) auto_execute: codex_state::ThreadGoalPlanAutoExecute,
    pub(crate) max_auto_goals_per_plan: usize,
    pub(crate) max_tokens_per_goal_plan: Option<i64>,
    pub(crate) max_goal_plan_node_objective_chars: usize,
    pub(crate) post_goal_context: codex_state::PostGoalContextAction,
    pub(crate) post_goal_plan_context: codex_state::PostGoalContextAction,
}

#[derive(Clone)]
pub(crate) struct GoalPlanRuntimeConfigHandle {
    runtime: GoalRuntimeHandle,
}

impl GoalPlanRuntimeConfigHandle {
    pub(crate) fn current(&self) -> GoalPlanRuntimeConfig {
        self.runtime.plan_config()
    }

    pub(crate) async fn apply_post_completion_context_policy(
        &self,
        goal: &codex_state::ThreadGoal,
        plan_outcome: Option<&codex_state::ThreadGoalPlanAdvanceOutcome>,
    ) -> Result<Option<String>, String> {
        self.runtime
            .apply_post_completion_context_policy(goal, plan_outcome)
            .await
    }
}

pub(crate) struct AccountedGoalProgress {
    pub(crate) goal: ThreadGoal,
    pub(crate) goal_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousGoalSnapshot {
    pub goal_id: String,
    pub status: codex_state::ThreadGoalStatus,
    pub objective: String,
}

impl From<&codex_state::ThreadGoal> for PreviousGoalSnapshot {
    fn from(goal: &codex_state::ThreadGoal) -> Self {
        Self {
            goal_id: goal.goal_id.clone(),
            status: goal.status,
            objective: goal.objective.clone(),
        }
    }
}

impl std::fmt::Debug for GoalRuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalRuntimeHandle").finish_non_exhaustive()
    }
}

impl GoalRuntimeHandle {
    pub(crate) fn new(
        thread_id: ThreadId,
        state_dbs: Arc<codex_state::StateRuntime>,
        event_emitter: GoalEventEmitter,
        metrics: GoalMetrics,
        thread_manager: Weak<ThreadManager>,
        accounting_state: Arc<GoalAccountingState>,
        config: GoalRuntimeConfig,
    ) -> Self {
        Self {
            inner: Arc::new(GoalRuntimeInner {
                thread_id,
                state_dbs,
                event_emitter,
                metrics,
                thread_manager,
                accounting_state,
                enabled: AtomicBool::new(config.enabled),
                tools_available_for_thread: config.tools_available_for_thread,
                plan_config: std::sync::RwLock::new(GoalPlanRuntimeConfig {
                    auto_execute: config.auto_execute,
                    max_auto_goals_per_plan: config.max_auto_goals_per_plan,
                    max_tokens_per_goal_plan: config.max_tokens_per_goal_plan,
                    max_goal_plan_node_objective_chars: config.max_goal_plan_node_objective_chars,
                    post_goal_context: config.post_goal_context,
                    post_goal_plan_context: config.post_goal_plan_context,
                }),
                goal_state_lock: Semaphore::new(/*permits*/ 1),
                suppressed_idle_continuations: Mutex::new(HashSet::new()),
                pending_context_compaction: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn set_config(&self, config: GoalRuntimeConfig) {
        self.inner.enabled.store(config.enabled, Ordering::Relaxed);
        if let Ok(mut plan_config) = self.inner.plan_config.write() {
            *plan_config = GoalPlanRuntimeConfig {
                auto_execute: config.auto_execute,
                max_auto_goals_per_plan: config.max_auto_goals_per_plan,
                max_tokens_per_goal_plan: config.max_tokens_per_goal_plan,
                max_goal_plan_node_objective_chars: config.max_goal_plan_node_objective_chars,
                post_goal_context: config.post_goal_context,
                post_goal_plan_context: config.post_goal_plan_context,
            };
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn tools_visible(&self) -> bool {
        self.is_enabled() && self.inner.tools_available_for_thread
    }

    pub(crate) fn tools_available_for_thread(&self) -> bool {
        self.inner.tools_available_for_thread
    }

    pub(crate) fn thread_id(&self) -> ThreadId {
        self.inner.thread_id
    }

    pub(crate) fn accounting_state(&self) -> Arc<GoalAccountingState> {
        Arc::clone(&self.inner.accounting_state)
    }

    pub(crate) fn suppress_next_idle_continuation(&self, goal_id: impl Into<String>) {
        self.suppressed_idle_continuations().insert(goal_id.into());
    }

    pub(crate) fn plan_config(&self) -> GoalPlanRuntimeConfig {
        self.inner
            .plan_config
            .read()
            .map(|config| *config)
            .unwrap_or(GoalPlanRuntimeConfig {
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
                max_auto_goals_per_plan: 1,
                max_tokens_per_goal_plan: None,
                max_goal_plan_node_objective_chars:
                    codex_core::config::DEFAULT_GOAL_PLAN_NODE_OBJECTIVE_CHARS,
                post_goal_context: codex_state::PostGoalContextAction::Keep,
                post_goal_plan_context: codex_state::PostGoalContextAction::Keep,
            })
    }

    pub(crate) fn plan_config_handle(&self) -> GoalPlanRuntimeConfigHandle {
        GoalPlanRuntimeConfigHandle {
            runtime: self.clone(),
        }
    }

    pub(crate) async fn goal_state_permit(&self) -> Result<SemaphorePermit<'_>, String> {
        self.inner
            .goal_state_lock
            .acquire()
            .await
            .map_err(|err| err.to_string())
    }

    pub async fn prepare_external_goal_mutation(&self) -> Result<(), String> {
        if !self.is_enabled() {
            return Ok(());
        }

        if let Some(turn_id) = self.inner.accounting_state.current_turn_id() {
            self.account_active_goal_progress(
                turn_id.as_str(),
                &format!("{turn_id}:external-goal-mutation"),
                codex_state::GoalAccountingMode::ActiveOnly,
                BudgetLimitedGoalDisposition::ClearActive,
            )
            .await?;
            return Ok(());
        }

        self.account_idle_goal_progress(
            &format!("{}:external-goal-mutation", self.inner.thread_id),
            codex_state::GoalAccountingMode::ActiveOnly,
            BudgetLimitedGoalDisposition::ClearActive,
        )
        .await?;
        Ok(())
    }

    pub async fn apply_external_goal_set(
        &self,
        goal: codex_state::ThreadGoal,
        previous_goal: Option<PreviousGoalSnapshot>,
    ) -> Result<(), String> {
        if !self.is_enabled() {
            return Ok(());
        }

        let replaced_existing_goal = previous_goal
            .as_ref()
            .is_some_and(|previous_goal| previous_goal.goal_id != goal.goal_id);
        if previous_goal.is_none() || replaced_existing_goal {
            self.inner.metrics.record_created();
        }
        let previous_status = previous_goal
            .as_ref()
            .and_then(|previous_goal| (!replaced_existing_goal).then_some(previous_goal.status));
        self.inner
            .metrics
            .record_resumed_if_status_changed(previous_status, goal.status);
        self.inner
            .metrics
            .record_terminal_if_status_changed(previous_status, &goal);
        let plan_advance = match goal.status {
            codex_state::ThreadGoalStatus::Complete => self
                .inner
                .state_dbs
                .thread_goals()
                .complete_goal_plan_node_and_maybe_advance(
                    self.thread_id(),
                    &goal,
                    self.plan_config().auto_execute,
                )
                .await
                .map_err(|err| err.to_string())?,
            codex_state::ThreadGoalStatus::Deferred => self
                .inner
                .state_dbs
                .thread_goals()
                .defer_goal_plan_node_and_maybe_advance(
                    self.thread_id(),
                    &goal,
                    self.plan_config().auto_execute,
                )
                .await
                .map_err(|err| err.to_string())?,
            codex_state::ThreadGoalStatus::Active
            | codex_state::ThreadGoalStatus::Paused
            | codex_state::ThreadGoalStatus::Blocked
            | codex_state::ThreadGoalStatus::UsageLimited
            | codex_state::ThreadGoalStatus::BudgetLimited
            | codex_state::ThreadGoalStatus::Cancelled => self
                .inner
                .state_dbs
                .thread_goals()
                .sync_goal_plan_node_for_goal(self.thread_id(), &goal)
                .await
                .map_err(|err| err.to_string())?
                .map(|snapshot| codex_state::ThreadGoalPlanAdvanceOutcome {
                    snapshot,
                    activated_goal: None,
                }),
        };
        let objective_changed = previous_goal.as_ref().is_some_and(|previous_goal| {
            !replaced_existing_goal && previous_goal.objective != goal.objective
        });
        match goal.status {
            codex_state::ThreadGoalStatus::Active => {
                if matches!(
                    previous_status,
                    Some(
                        codex_state::ThreadGoalStatus::Blocked
                            | codex_state::ThreadGoalStatus::UsageLimited
                    )
                ) {
                    crate::pending_interaction::clear_goal_status_waits(
                        self.inner.state_dbs.as_ref(),
                        self.thread_id(),
                        goal.goal_id.as_str(),
                        "goal resumed",
                    )
                    .await?;
                }
                if self.inner.accounting_state.current_turn_id().is_some() {
                    let _ = self
                        .inner
                        .accounting_state
                        .mark_current_turn_goal_active(goal.goal_id.clone());
                } else {
                    self.inner
                        .accounting_state
                        .mark_idle_goal_active(goal.goal_id.clone());
                }
                if objective_changed {
                    let item =
                        objective_updated_steering_item(&protocol_goal_from_state(goal.clone()));
                    self.inject_active_turn_steering(item).await;
                }
                self.continue_if_idle().await?;
            }
            codex_state::ThreadGoalStatus::BudgetLimited => {
                if self.inner.accounting_state.current_turn_id().is_none() {
                    self.inner.accounting_state.clear_active_goal();
                }
            }
            codex_state::ThreadGoalStatus::Blocked
            | codex_state::ThreadGoalStatus::UsageLimited => {
                self.inner.accounting_state.clear_active_goal();
                if previous_status != Some(goal.status) {
                    let reason = match goal.status {
                        codex_state::ThreadGoalStatus::Blocked => "external-goal-blocked",
                        codex_state::ThreadGoalStatus::UsageLimited => "external-goal-usage-limit",
                        codex_state::ThreadGoalStatus::Active
                        | codex_state::ThreadGoalStatus::Paused
                        | codex_state::ThreadGoalStatus::BudgetLimited
                        | codex_state::ThreadGoalStatus::Deferred
                        | codex_state::ThreadGoalStatus::Complete
                        | codex_state::ThreadGoalStatus::Cancelled => {
                            unreachable!("status matched above")
                        }
                    };
                    crate::pending_interaction::record_goal_status_wait(
                        self.inner.state_dbs.as_ref(),
                        self.thread_id(),
                        &goal,
                        /*turn_id*/ None,
                        reason,
                    )
                    .await?;
                }
            }
            codex_state::ThreadGoalStatus::Deferred => {
                self.inner.accounting_state.clear_active_goal();
                if matches!(
                    previous_status,
                    Some(
                        codex_state::ThreadGoalStatus::Blocked
                            | codex_state::ThreadGoalStatus::UsageLimited
                    )
                ) {
                    crate::pending_interaction::clear_goal_status_waits(
                        self.inner.state_dbs.as_ref(),
                        self.thread_id(),
                        goal.goal_id.as_str(),
                        "goal deferred",
                    )
                    .await?;
                }
            }
            codex_state::ThreadGoalStatus::Paused
            | codex_state::ThreadGoalStatus::Complete
            | codex_state::ThreadGoalStatus::Cancelled => {
                self.inner.accounting_state.clear_active_goal();
            }
        }
        if let Some(outcome) = &plan_advance {
            self.inner.event_emitter.thread_goal_plan_updated(
                format!("{}:external-goal-plan", self.thread_id()),
                /*turn_id*/ None,
                outcome.snapshot.clone(),
            );
            if let Some(activated_goal) = outcome.activated_goal.clone() {
                self.inner.metrics.record_created();
                self.inner
                    .accounting_state
                    .mark_idle_goal_active(activated_goal.goal_id.clone());
                self.inner.event_emitter.thread_goal_updated(
                    format!("{}:external-goal-advance", self.thread_id()),
                    /*turn_id*/ None,
                    protocol_goal_from_state(activated_goal),
                );
                self.continue_if_idle().await?;
            }
        }
        if let Some(report) = self
            .apply_post_completion_context_policy(&goal, plan_advance.as_ref())
            .await?
        {
            tracing::info!("{report}");
        }
        Ok(())
    }

    pub async fn apply_external_goal_clear(&self) -> Result<(), String> {
        if !self.is_enabled() {
            return Ok(());
        }

        let snapshots = self
            .inner
            .state_dbs
            .thread_goals()
            .block_active_goal_plan_nodes_for_thread(self.thread_id())
            .await
            .map_err(|err| err.to_string())?;
        for (index, snapshot) in snapshots.into_iter().enumerate() {
            self.inner.event_emitter.thread_goal_plan_updated(
                format!("{}:external-goal-clear:goal-plan:{index}", self.thread_id()),
                /*turn_id*/ None,
                snapshot,
            );
        }
        self.inner.accounting_state.clear_active_goal();
        self.inner
            .state_dbs
            .thread_goals()
            .clear_thread_goal_blocker_audit(self.thread_id())
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    pub async fn usage_limit_active_goal_for_turn(&self, turn_id: &str) -> Result<(), String> {
        self.stop_active_goal_for_turn(turn_id, ActiveGoalStopReason::UsageLimit)
            .await
    }

    /// Accounts the ending turn and stops its active goal after a terminal error.
    pub(crate) async fn stop_active_goal_for_turn(
        &self,
        turn_id: &str,
        reason: ActiveGoalStopReason,
    ) -> Result<(), String> {
        if !self.is_enabled() {
            return Ok(());
        }

        // Hold this through accounting and the status update so external goal
        // mutations and idle continuation cannot interleave between them.
        let _goal_state_permit = self.goal_state_permit().await?;
        if !self
            .inner
            .accounting_state
            .turn_is_current_active_goal(turn_id)
        {
            return Ok(());
        }

        let event_name = match &reason {
            ActiveGoalStopReason::TurnError { .. } => "turn-error",
            ActiveGoalStopReason::UsageLimit => "usage-limit",
        };
        self.account_active_goal_progress(
            turn_id,
            &format!("{turn_id}:{event_name}-progress"),
            codex_state::GoalAccountingMode::ActiveOnly,
            BudgetLimitedGoalDisposition::ClearActive,
        )
        .await?;

        let Some(active_goal) = self
            .inner
            .state_dbs
            .thread_goals()
            .get_thread_goal(self.thread_id())
            .await
            .map_err(|err| err.to_string())?
        else {
            self.inner.accounting_state.clear_active_goal();
            return Ok(());
        };
        let previous_status = Some(active_goal.status);
        let goal = match &reason {
            ActiveGoalStopReason::TurnError { fingerprint, .. } => {
                match self
                    .inner
                    .state_dbs
                    .thread_goals()
                    .observe_active_thread_goal_blocker(
                        self.thread_id(),
                        active_goal.goal_id.as_str(),
                        turn_id,
                        fingerprint,
                        REQUIRED_CONSECUTIVE_BLOCKER_TURNS,
                    )
                    .await
                    .map_err(|err| err.to_string())?
                {
                    codex_state::GoalBlockerAuditOutcome::Updated { goal, .. } => goal,
                    codex_state::GoalBlockerAuditOutcome::Unchanged(goal) => {
                        self.inner.accounting_state.clear_active_goal();
                        if let Some(goal) = goal
                            && !matches!(
                                goal.status,
                                codex_state::ThreadGoalStatus::Active
                                    | codex_state::ThreadGoalStatus::Paused
                            )
                        {
                            self.inner
                                .state_dbs
                                .thread_goals()
                                .clear_thread_goal_blocker_audit_for_goal(
                                    self.thread_id(),
                                    goal.goal_id.as_str(),
                                )
                                .await
                                .map_err(|err| err.to_string())?;
                        }
                        return Ok(());
                    }
                }
            }
            ActiveGoalStopReason::UsageLimit => {
                let can_stop = active_goal.status == codex_state::ThreadGoalStatus::Active
                    || active_goal.status == codex_state::ThreadGoalStatus::BudgetLimited;
                if !can_stop {
                    self.inner.accounting_state.clear_active_goal();
                    return Ok(());
                }
                let Some(goal) = self
                    .inner
                    .state_dbs
                    .thread_goals()
                    .update_thread_goal(
                        self.thread_id(),
                        codex_state::GoalUpdate {
                            objective: None,
                            title: None,
                            status: Some(codex_state::ThreadGoalStatus::UsageLimited),
                            token_budget: None,
                            expected_goal_id: Some(active_goal.goal_id),
                        },
                    )
                    .await
                    .map_err(|err| err.to_string())?
                else {
                    return Ok(());
                };
                self.inner
                    .state_dbs
                    .thread_goals()
                    .clear_thread_goal_blocker_audit_for_goal(
                        self.thread_id(),
                        goal.goal_id.as_str(),
                    )
                    .await
                    .map_err(|err| err.to_string())?;
                goal
            }
        };
        match &reason {
            ActiveGoalStopReason::TurnError { error, .. }
                if goal.status == codex_state::ThreadGoalStatus::Blocked =>
            {
                crate::pending_interaction::record_goal_turn_error_status_wait(
                    self.inner.state_dbs.as_ref(),
                    self.thread_id(),
                    &goal,
                    turn_id,
                    error,
                )
                .await?;
            }
            ActiveGoalStopReason::TurnError { .. } => {}
            ActiveGoalStopReason::UsageLimit => {
                crate::pending_interaction::record_goal_status_wait(
                    self.inner.state_dbs.as_ref(),
                    self.thread_id(),
                    &goal,
                    Some(turn_id),
                    event_name,
                )
                .await?;
            }
        }
        self.inner
            .metrics
            .record_terminal_if_status_changed(previous_status, &goal);
        let plan_snapshot = self
            .inner
            .state_dbs
            .thread_goals()
            .sync_goal_plan_node_for_goal(self.thread_id(), &goal)
            .await
            .map_err(|err| err.to_string())?;
        if let Some(snapshot) = plan_snapshot {
            self.inner.event_emitter.thread_goal_plan_updated(
                format!("{turn_id}:{event_name}:goal-plan"),
                Some(turn_id.to_string()),
                snapshot,
            );
        }
        self.inner.accounting_state.clear_active_goal();
        let goal = protocol_goal_from_state(goal);
        self.inner.event_emitter.thread_goal_updated(
            format!("{turn_id}:{event_name}"),
            Some(turn_id.to_string()),
            goal,
        );
        Ok(())
    }

    pub async fn restore_after_resume(&self) -> Result<(), String> {
        if !self.is_enabled() {
            return Ok(());
        }

        let goal = self
            .inner
            .state_dbs
            .thread_goals()
            .get_thread_goal(self.thread_id())
            .await
            .map_err(|err| err.to_string())?;
        match goal {
            Some(goal) if goal.status == codex_state::ThreadGoalStatus::Active => {
                self.inner
                    .accounting_state
                    .mark_idle_goal_active(goal.goal_id);
                self.inner.metrics.record_resumed();
            }
            Some(_) | None => self.inner.accounting_state.clear_active_goal(),
        }
        Ok(())
    }

    pub(crate) async fn continue_if_idle(&self) -> Result<(), String> {
        if !self.tools_visible() {
            self.inner.accounting_state.clear_active_goal();
            return Ok(());
        }
        // Hold this through the read/start window so external set/clear cannot
        // change the goal after we read it but before the continuation launches.
        let _goal_state_permit = self.goal_state_permit().await?;

        let Some(thread_manager) = self.inner.thread_manager.upgrade() else {
            tracing::debug!("skipping goal continuation because thread manager is unavailable");
            return Ok(());
        };
        let Ok(thread) = thread_manager.get_thread(self.inner.thread_id).await else {
            tracing::debug!("skipping goal continuation because live thread is unavailable");
            return Ok(());
        };

        let Some(goal) = self
            .inner
            .state_dbs
            .thread_goals()
            .get_thread_goal(self.thread_id())
            .await
            .map_err(|err| err.to_string())?
        else {
            self.inner.accounting_state.clear_active_goal();
            return Ok(());
        };
        if goal.status != codex_state::ThreadGoalStatus::Active {
            self.inner.accounting_state.clear_active_goal();
            return Ok(());
        }
        if self
            .suppressed_idle_continuations()
            .remove(goal.goal_id.as_str())
        {
            self.inner.accounting_state.clear_active_goal();
            return Ok(());
        }
        let item = continuation_steering_item(&protocol_goal_from_state(goal));

        if let Err(err) = thread.try_start_turn_if_idle(vec![item]).await {
            let reason = err.reason();
            tracing::debug!(
                ?reason,
                "skipping goal continuation because automatic idle work was rejected"
            );
        }

        let current_turn_is_goal_active = self
            .inner
            .accounting_state
            .current_turn_id()
            .is_some_and(|turn_id| {
                self.inner
                    .accounting_state
                    .turn_is_current_active_goal(turn_id.as_str())
            });
        if !current_turn_is_goal_active {
            self.inner.accounting_state.clear_active_goal();
        }
        Ok(())
    }

    pub(crate) async fn apply_post_completion_context_policy(
        &self,
        goal: &codex_state::ThreadGoal,
        plan_outcome: Option<&codex_state::ThreadGoalPlanAdvanceOutcome>,
    ) -> Result<Option<String>, String> {
        if !self.is_enabled() || goal.status != codex_state::ThreadGoalStatus::Complete {
            return Ok(None);
        }

        let config = self.plan_config();
        let action = match plan_outcome {
            Some(outcome) if outcome.activated_goal.is_some() => {
                return Ok(None);
            }
            Some(outcome)
                if outcome.snapshot.plan.status == codex_state::ThreadGoalPlanStatus::Complete =>
            {
                self.inner
                    .state_dbs
                    .thread_goals()
                    .thread_goal_plan_completion_context_action(
                        self.thread_id(),
                        outcome.snapshot.plan.plan_id.as_str(),
                    )
                    .await
                    .map_err(|err| err.to_string())?
                    .unwrap_or(config.post_goal_plan_context)
            }
            Some(outcome) => self
                .inner
                .state_dbs
                .thread_goals()
                .thread_goal_plan_context_action(
                    self.thread_id(),
                    outcome.snapshot.plan.plan_id.as_str(),
                )
                .await
                .map_err(|err| err.to_string())?
                .unwrap_or(config.post_goal_context),
            None => self
                .inner
                .state_dbs
                .thread_goals()
                .thread_goal_context_action(self.thread_id(), goal.goal_id.as_str())
                .await
                .map_err(|err| err.to_string())?
                .unwrap_or(config.post_goal_context),
        };

        match action {
            codex_state::PostGoalContextAction::Keep => Ok(None),
            codex_state::PostGoalContextAction::Compact => {
                self.schedule_native_compaction_when_idle().await
            }
        }
    }

    async fn schedule_native_compaction_when_idle(&self) -> Result<Option<String>, String> {
        self.inner
            .pending_context_compaction
            .store(true, Ordering::Release);
        if let Some(thread_manager) = self.inner.thread_manager.upgrade()
            && let Ok(thread) = thread_manager.get_thread(self.thread_id()).await
        {
            thread.emit_thread_idle_lifecycle_if_idle().await;
        }
        Ok(Some(
            "Scheduled native context compaction after the thread becomes idle.".to_string(),
        ))
    }

    pub(crate) async fn drain_pending_context_compaction_if_idle(
        &self,
    ) -> Result<Option<String>, String> {
        if !self
            .inner
            .pending_context_compaction
            .swap(false, Ordering::AcqRel)
        {
            return Ok(None);
        }
        self.queue_native_compaction().await
    }

    async fn queue_native_compaction(&self) -> Result<Option<String>, String> {
        let Some(thread_manager) = self.inner.thread_manager.upgrade() else {
            return Ok(Some(
                "Skipped post-goal context compaction because the thread manager is unavailable."
                    .to_string(),
            ));
        };
        let thread = match thread_manager.get_thread(self.thread_id()).await {
            Ok(thread) => thread,
            Err(err) => {
                return Ok(Some(format!(
                    "Skipped post-goal context compaction because the live thread is unavailable: {err}"
                )));
            }
        };
        thread
            .submit(Op::Compact)
            .await
            .map_err(|err| err.to_string())?;
        Ok(Some(
            "Queued native context compaction after completed goal lifecycle.".to_string(),
        ))
    }

    pub(crate) async fn inject_active_turn_steering(&self, item: ResponseItem) {
        let Some(thread_manager) = self.inner.thread_manager.upgrade() else {
            tracing::debug!("skipping goal steering because thread manager is unavailable");
            return;
        };
        let Ok(thread) = thread_manager.get_thread(self.inner.thread_id).await else {
            tracing::debug!("skipping goal steering because live thread is unavailable");
            return;
        };
        if thread.inject_if_running(vec![item]).await.is_err() {
            tracing::debug!("skipping goal steering because no turn is active");
        }
    }

    pub(crate) async fn account_active_goal_progress(
        &self,
        turn_id: &str,
        event_id: &str,
        mode: codex_state::GoalAccountingMode,
        budget_limited_goal_disposition: BudgetLimitedGoalDisposition,
    ) -> Result<Option<AccountedGoalProgress>, String> {
        let accounting = self.accounting_state();
        let _accounting_permit = accounting
            .progress_accounting_permit()
            .await
            .map_err(|err| err.to_string())?;
        let Some(snapshot) = accounting.progress_snapshot(turn_id) else {
            return Ok(None);
        };
        let previous_status = self
            .current_goal_status_for_metrics(Some(snapshot.expected_goal_id.as_str()))
            .await?;
        let outcome = self
            .inner
            .state_dbs
            .thread_goals()
            .account_thread_goal_usage(
                self.thread_id(),
                snapshot.time_delta_seconds,
                snapshot.token_delta,
                mode,
                Some(snapshot.expected_goal_id.as_str()),
            )
            .await
            .map_err(|err| err.to_string())?;
        Ok(match outcome {
            codex_state::GoalAccountingOutcome::Updated(goal) => {
                let goal_id = goal.goal_id.clone();
                self.inner
                    .metrics
                    .record_terminal_if_status_changed(previous_status, &goal);
                let plan_snapshot = self
                    .inner
                    .state_dbs
                    .thread_goals()
                    .sync_goal_plan_node_for_goal(self.thread_id(), &goal)
                    .await
                    .map_err(|err| err.to_string())?;
                if let Some(snapshot) = plan_snapshot {
                    self.inner.event_emitter.thread_goal_plan_updated(
                        format!("{event_id}-goal-plan"),
                        Some(turn_id.to_string()),
                        snapshot,
                    );
                }
                accounting.mark_progress_accounted_for_status(
                    turn_id,
                    &snapshot,
                    goal.status,
                    budget_limited_goal_disposition,
                );
                let goal = protocol_goal_from_state(goal);
                self.inner.event_emitter.thread_goal_updated(
                    event_id.to_string(),
                    Some(turn_id.to_string()),
                    goal.clone(),
                );
                Some(AccountedGoalProgress { goal, goal_id })
            }
            codex_state::GoalAccountingOutcome::Unchanged(_) => None,
        })
    }

    async fn account_idle_goal_progress(
        &self,
        event_id: &str,
        mode: codex_state::GoalAccountingMode,
        budget_limited_goal_disposition: BudgetLimitedGoalDisposition,
    ) -> Result<Option<AccountedGoalProgress>, String> {
        let accounting = self.accounting_state();
        let _accounting_permit = accounting
            .progress_accounting_permit()
            .await
            .map_err(|err| err.to_string())?;
        let Some(snapshot) = accounting.idle_progress_snapshot() else {
            return Ok(None);
        };
        let previous_status = self
            .current_goal_status_for_metrics(Some(snapshot.expected_goal_id.as_str()))
            .await?;
        let outcome = self
            .inner
            .state_dbs
            .thread_goals()
            .account_thread_goal_usage(
                self.thread_id(),
                snapshot.time_delta_seconds,
                /*token_delta*/ 0,
                mode,
                Some(snapshot.expected_goal_id.as_str()),
            )
            .await
            .map_err(|err| err.to_string())?;
        Ok(match outcome {
            codex_state::GoalAccountingOutcome::Updated(goal) => {
                let goal_id = goal.goal_id.clone();
                self.inner
                    .metrics
                    .record_terminal_if_status_changed(previous_status, &goal);
                let plan_snapshot = self
                    .inner
                    .state_dbs
                    .thread_goals()
                    .sync_goal_plan_node_for_goal(self.thread_id(), &goal)
                    .await
                    .map_err(|err| err.to_string())?;
                if let Some(snapshot) = plan_snapshot {
                    self.inner.event_emitter.thread_goal_plan_updated(
                        format!("{event_id}-goal-plan"),
                        /*turn_id*/ None,
                        snapshot,
                    );
                }
                accounting.mark_idle_progress_accounted_for_status(
                    &snapshot,
                    goal.status,
                    budget_limited_goal_disposition,
                );
                let goal = protocol_goal_from_state(goal);
                self.inner.event_emitter.thread_goal_updated(
                    event_id.to_string(),
                    /*turn_id*/ None,
                    goal.clone(),
                );
                Some(AccountedGoalProgress { goal, goal_id })
            }
            codex_state::GoalAccountingOutcome::Unchanged(_) => {
                accounting.reset_idle_progress_baseline_and_clear_active_goal();
                None
            }
        })
    }

    async fn current_goal_status_for_metrics(
        &self,
        expected_goal_id: Option<&str>,
    ) -> Result<Option<codex_state::ThreadGoalStatus>, String> {
        let goal = self
            .inner
            .state_dbs
            .thread_goals()
            .get_thread_goal(self.thread_id())
            .await
            .map_err(|err| err.to_string())?;
        Ok(goal.and_then(|goal| {
            expected_goal_id
                .is_none_or(|expected_goal_id| goal.goal_id == expected_goal_id)
                .then_some(goal.status)
        }))
    }

    fn suppressed_idle_continuations(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.inner
            .suppressed_idle_continuations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
