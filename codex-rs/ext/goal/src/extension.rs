use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use codex_core::ThreadManager;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadIdleInput;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadResumeInput;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ThreadStopInput;
use codex_extension_api::TokenUsageContributor;
use codex_extension_api::ToolCallOutcome;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolLifecycleFuture;
use codex_extension_api::ToolStartInput;
use codex_extension_api::ToolWorktreeMutationSignal;
use codex_extension_api::TurnAbortInput;
use codex_extension_api::TurnCompletionDecision;
use codex_extension_api::TurnCompletionInput;
use codex_extension_api::TurnErrorInput;
use codex_extension_api::TurnLifecycleContributor;
use codex_extension_api::TurnStartInput;
use codex_extension_api::TurnStopInput;
use codex_otel::MetricsClient;
use codex_protocol::ThreadId;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadGoalStatus;
use codex_protocol::protocol::TokenUsageInfo;

use crate::accounting::BudgetLimitedGoalDisposition;
use crate::accounting::GoalAccountingState;
use crate::accounting::LineChangeAccounting;
use crate::api::GoalService;
use crate::events::GoalEventEmitter;
use crate::metrics::GoalMetrics;
use crate::runtime::ActiveGoalStopReason;
use crate::runtime::GoalRuntimeConfig;
use crate::runtime::GoalRuntimeHandle;
use crate::spec::ACTIVATE_GOAL_PLAN_NODE_TOOL_NAME;
use crate::spec::CREATE_GOAL_PLAN_TOOL_NAME;
use crate::spec::GET_GOAL_PLAN_TOOL_NAME;
use crate::spec::INSERT_GOAL_PLAN_NODE_TOOL_NAME;
use crate::spec::PAUSE_GOAL_TOOL_NAME;
use crate::spec::RESUME_GOAL_TOOL_NAME;
use crate::spec::SET_GOAL_PLAN_NODE_STATUS_TOOL_NAME;
use crate::spec::UPDATE_GOAL_PLAN_NODE_TOOL_NAME;
use crate::spec::UPDATE_GOAL_TOOL_NAME;
use crate::steering::budget_limit_steering_item;
use crate::steering::plan_completion_guard_steering_item;
use crate::tool::GoalToolExecutor;

#[derive(Default)]
struct GoalPlanTerminationGuardState {
    continuation_requested: AtomicBool,
}

#[derive(Clone, Debug)]
pub struct GoalExtensionConfig {
    pub enabled: bool,
    pub auto_execute: codex_state::ThreadGoalPlanAutoExecute,
    pub max_auto_goals_per_plan: usize,
    pub max_tokens_per_goal_plan: Option<i64>,
    pub max_goal_plan_node_objective_chars: usize,
    pub post_goal_context: codex_state::PostGoalContextAction,
    pub post_goal_plan_context: codex_state::PostGoalContextAction,
}

#[derive(Clone)]
pub struct GoalExtension<C> {
    state_dbs: Arc<codex_state::StateRuntime>,
    event_emitter: GoalEventEmitter,
    metrics: GoalMetrics,
    thread_manager: Weak<ThreadManager>,
    goal_service: Arc<GoalService>,
    goals_config: Arc<dyn Fn(&C) -> GoalExtensionConfig + Send + Sync>,
}

impl<C> std::fmt::Debug for GoalExtension<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalExtension").finish_non_exhaustive()
    }
}

impl<C> GoalExtension<C> {
    pub(crate) fn new_with_host_capabilities(
        state_dbs: Arc<codex_state::StateRuntime>,
        event_sink: Arc<dyn ExtensionEventSink>,
        metrics_client: Option<MetricsClient>,
        thread_manager: Weak<ThreadManager>,
        goal_service: Arc<GoalService>,
        goals_config: impl Fn(&C) -> GoalExtensionConfig + Send + Sync + 'static,
    ) -> Self {
        Self {
            state_dbs,
            event_emitter: GoalEventEmitter::new(event_sink),
            metrics: GoalMetrics::new(metrics_client),
            thread_manager,
            goal_service,
            goals_config: Arc::new(goals_config),
        }
    }

    async fn handle_turn_error(&self, input: TurnErrorInput<'_>, error_fingerprint: &str) {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return;
        };
        // Core emits turn-stop after terminal turn errors. Record the error
        // before any fallible persistence so that later success-only cleanup
        // cannot erase this or an earlier durable blocker observation.
        runtime
            .accounting_state()
            .mark_turn_error_observed(input.turn_id);

        let reason = match &input.error {
            CodexErrorInfo::UsageLimitExceeded => ActiveGoalStopReason::UsageLimit,
            // The turn has ended because the error was non-retryable or its
            // retries were exhausted. Hold the goal to prevent automatic
            // continuation from looping and consuming tokens. The runtime
            // promotes the same blocker to Blocked only after the required
            // number of consecutive goal turns.
            _ => ActiveGoalStopReason::TurnError {
                error: input.error.clone(),
                fingerprint: error_fingerprint.to_string(),
            },
        };
        if let Err(err) = runtime
            .stop_active_goal_for_turn(input.turn_id, reason)
            .await
        {
            tracing::warn!(
                error = ?input.error,
                "failed to stop active goal after turn error: {err}"
            );
        }
    }
}

#[async_trait]
impl<C> ThreadLifecycleContributor<C> for GoalExtension<C>
where
    C: Send + Sync + 'static,
{
    async fn on_thread_start(&self, input: ThreadStartInput<'_, C>) {
        let config = (self.goals_config)(input.config);
        let tools_available_for_thread = input.persistent_thread_state_available
            && !matches!(
                input.session_source,
                SessionSource::SubAgent(SubAgentSource::Review)
            );
        input.thread_store.insert(config.clone());
        let accounting_state = input
            .thread_store
            .get_or_init::<GoalAccountingState>(GoalAccountingState::default);
        let Ok(thread_id) = ThreadId::from_string(input.thread_store.level_id()) else {
            return;
        };
        let runtime = input.thread_store.get_or_init::<GoalRuntimeHandle>(|| {
            GoalRuntimeHandle::new(
                thread_id,
                Arc::clone(&self.state_dbs),
                self.event_emitter.clone(),
                self.metrics.clone(),
                self.thread_manager.clone(),
                accounting_state,
                GoalRuntimeConfig {
                    enabled: config.enabled,
                    tools_available_for_thread,
                    auto_execute: config.auto_execute,
                    max_auto_goals_per_plan: config.max_auto_goals_per_plan,
                    max_tokens_per_goal_plan: config.max_tokens_per_goal_plan,
                    max_goal_plan_node_objective_chars: config.max_goal_plan_node_objective_chars,
                    post_goal_context: config.post_goal_context,
                    post_goal_plan_context: config.post_goal_plan_context,
                },
            )
        });
        runtime.set_config(GoalRuntimeConfig {
            enabled: config.enabled,
            tools_available_for_thread: runtime.tools_available_for_thread(),
            auto_execute: config.auto_execute,
            max_auto_goals_per_plan: config.max_auto_goals_per_plan,
            max_tokens_per_goal_plan: config.max_tokens_per_goal_plan,
            max_goal_plan_node_objective_chars: config.max_goal_plan_node_objective_chars,
            post_goal_context: config.post_goal_context,
            post_goal_plan_context: config.post_goal_plan_context,
        });
        self.goal_service.register_runtime(&runtime);
    }

    async fn on_thread_resume(&self, input: ThreadResumeInput<'_>) {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return;
        };

        if let Err(err) = runtime.restore_after_resume().await {
            tracing::warn!(
                "failed to restore goal runtime after thread resume for {}: {err}",
                runtime.thread_id()
            );
        }
    }

    async fn on_thread_idle(&self, input: ThreadIdleInput<'_>) {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return;
        };

        match runtime.drain_pending_context_compaction_if_idle().await {
            Ok(Some(report)) => {
                tracing::info!("{report}");
                return;
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    "failed to run pending post-goal context compaction for idle thread {}: {err}",
                    runtime.thread_id()
                );
                return;
            }
        }

        if let Err(err) = runtime.continue_if_idle().await {
            tracing::warn!(
                "failed to continue active goal for idle thread {}: {err}",
                runtime.thread_id()
            );
        }
    }

    async fn on_thread_stop(&self, input: ThreadStopInput<'_>) {
        if let Some(runtime) = goal_runtime_handle(input.thread_store) {
            self.goal_service.unregister_runtime(&runtime);
        }
    }
}

impl<C> ConfigContributor<C> for GoalExtension<C>
where
    C: Send + Sync + 'static,
{
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &C,
        new_config: &C,
    ) {
        let config = (self.goals_config)(new_config);
        thread_store.insert(config.clone());
        if let Some(runtime) = goal_runtime_handle(thread_store) {
            runtime.set_config(GoalRuntimeConfig {
                enabled: config.enabled,
                tools_available_for_thread: runtime.tools_available_for_thread(),
                auto_execute: config.auto_execute,
                max_auto_goals_per_plan: config.max_auto_goals_per_plan,
                max_tokens_per_goal_plan: config.max_tokens_per_goal_plan,
                max_goal_plan_node_objective_chars: config.max_goal_plan_node_objective_chars,
                post_goal_context: config.post_goal_context,
                post_goal_plan_context: config.post_goal_plan_context,
            });
        }
    }
}

#[async_trait]
impl<C> TurnLifecycleContributor for GoalExtension<C>
where
    C: Send + Sync + 'static,
{
    async fn on_turn_completion(&self, input: TurnCompletionInput<'_>) -> TurnCompletionDecision {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return TurnCompletionDecision::Allow;
        };
        if !runtime.tools_visible() {
            return TurnCompletionDecision::Allow;
        }

        match self
            .state_dbs
            .thread_goals()
            .get_thread_goal(runtime.thread_id())
            .await
        {
            Ok(Some(goal))
                if matches!(
                    goal.status,
                    codex_state::ThreadGoalStatus::Active
                        | codex_state::ThreadGoalStatus::Paused
                        | codex_state::ThreadGoalStatus::Blocked
                        | codex_state::ThreadGoalStatus::UsageLimited
                        | codex_state::ThreadGoalStatus::BudgetLimited
                        | codex_state::ThreadGoalStatus::Cancelled
                ) =>
            {
                return TurnCompletionDecision::Allow;
            }
            Ok(Some(_)) | Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    "failed to inspect current goal at turn completion for {}: {err}",
                    runtime.thread_id()
                );
                return TurnCompletionDecision::Allow;
            }
        }

        match crate::pending_interaction::has_active_thread_wait(
            self.state_dbs.as_ref(),
            runtime.thread_id(),
        )
        .await
        {
            Ok(true) => return TurnCompletionDecision::Allow,
            Ok(false) => {}
            Err(err) => tracing::warn!(
                "failed to inspect pending interactions at turn completion for {}: {err}",
                runtime.thread_id()
            ),
        }

        let plans = match self
            .state_dbs
            .thread_goals()
            .list_thread_goal_plans(runtime.thread_id())
            .await
        {
            Ok(plans) => plans,
            Err(err) => {
                tracing::warn!(
                    "failed to inspect goal plans at turn completion for {}: {err}",
                    runtime.thread_id()
                );
                return TurnCompletionDecision::Allow;
            }
        };
        let Some(plan) = plans
            .into_iter()
            .find(|plan| active_plan_requires_guard(plan, runtime.thread_id()))
        else {
            return TurnCompletionDecision::Allow;
        };

        let guard_state = input
            .turn_store
            .get_or_init::<GoalPlanTerminationGuardState>(GoalPlanTerminationGuardState::default);
        if !guard_state
            .continuation_requested
            .swap(true, Ordering::AcqRel)
        {
            return TurnCompletionDecision::Continue(vec![plan_completion_guard_steering_item(
                &plan,
            )]);
        }

        if let Err(err) = crate::pending_interaction::record_goal_plan_termination_wait(
            self.state_dbs.as_ref(),
            runtime.thread_id(),
            input.turn_id,
            &plan,
        )
        .await
        {
            tracing::warn!(
                "failed to record goal-plan termination wait for {}: {err}",
                runtime.thread_id()
            );
        }
        TurnCompletionDecision::Allow
    }

    async fn on_turn_start(&self, input: TurnStartInput<'_>) {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return;
        };
        if !runtime.is_enabled() {
            return;
        }

        let accounting = runtime.accounting_state();
        accounting.start_turn(
            input.turn_id,
            input.collaboration_mode.mode,
            input.token_usage_at_turn_start,
            input.local_cwd.map(std::path::Path::to_path_buf),
        );
        if matches!(
            input.collaboration_mode.mode,
            codex_protocol::config_types::ModeKind::Plan
        ) {
            accounting.clear_current_turn_goal();
            return;
        }
        let Ok(goal) = self
            .state_dbs
            .thread_goals()
            .get_thread_goal(runtime.thread_id())
            .await
        else {
            return;
        };
        if let Some(goal) = goal
            && matches!(
                goal.status,
                codex_state::ThreadGoalStatus::Active
                    | codex_state::ThreadGoalStatus::BudgetLimited
            )
        {
            accounting.mark_turn_goal_active(input.turn_id, goal.goal_id.clone());
            crate::line_changes::establish_current_turn_baseline(accounting.as_ref(), &goal).await;
        }
    }

    async fn on_turn_stop(&self, input: TurnStopInput<'_>) {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return;
        };
        if !runtime.is_enabled() {
            return;
        }

        let turn_id = input.turn_store.level_id();
        let accounting_state = runtime.accounting_state();
        let goal_id = (!accounting_state.turn_error_observed(turn_id))
            .then(|| accounting_state.active_goal_id_for_current_turn(turn_id))
            .flatten();
        if let Some(goal_id) = goal_id.as_deref() {
            let clear_result = codex_state::busy_retry::retry_on_busy(
                "clear blocker audit after successful turn",
                || {
                    self.state_dbs
                        .thread_goals()
                        .clear_thread_goal_blocker_audit_for_goal(runtime.thread_id(), goal_id)
                },
            )
            .await;
            if let Err(err) = clear_result {
                tracing::warn!(
                    "failed to clear blocker audit after successful turn {turn_id}: {err}"
                );
            }
        }
        let accounting_result = runtime
            .account_active_goal_progress(
                turn_id,
                &format!("{turn_id}:turn-stop"),
                LineChangeAccounting::Capture,
                codex_state::GoalAccountingMode::ActiveOnly,
                BudgetLimitedGoalDisposition::ClearActive,
            )
            .await;
        runtime.accounting_state().finish_turn(turn_id);
        if let Err(err) = accounting_result {
            tracing::warn!(
                "failed to account active goal progress at turn stop for {turn_id}: {err}"
            );
        }
    }

    async fn on_turn_abort(&self, input: TurnAbortInput<'_>) {
        let Some(runtime) = goal_runtime_handle(input.thread_store) else {
            return;
        };
        if !runtime.is_enabled() {
            return;
        }

        let turn_id = input.turn_store.level_id();
        let accounting_state = runtime.accounting_state();
        let goal_id = (!accounting_state.turn_error_observed(turn_id))
            .then(|| accounting_state.active_goal_id_for_current_turn(turn_id))
            .flatten();
        if let Some(goal_id) = goal_id.as_deref() {
            let clear_result = codex_state::busy_retry::retry_on_busy(
                "clear blocker audit after turn abort",
                || {
                    self.state_dbs
                        .thread_goals()
                        .clear_thread_goal_blocker_audit_for_goal(runtime.thread_id(), goal_id)
                },
            )
            .await;
            if let Err(err) = clear_result {
                tracing::warn!("failed to clear blocker audit after turn abort {turn_id}: {err}");
            }
        }
        let accounting_result = runtime
            .account_active_goal_progress(
                turn_id,
                &format!("{turn_id}:turn-abort"),
                LineChangeAccounting::Capture,
                codex_state::GoalAccountingMode::ActiveOnly,
                BudgetLimitedGoalDisposition::ClearActive,
            )
            .await;
        runtime.accounting_state().finish_turn(turn_id);
        if let Err(err) = accounting_result {
            tracing::warn!(
                "failed to account active goal progress after turn abort for {turn_id}: {err}"
            );
        }
    }

    async fn on_turn_error(&self, input: TurnErrorInput<'_>) {
        let error_fingerprint = protocol_error_fingerprint(&input.error);
        self.handle_turn_error(input, error_fingerprint.as_str())
            .await;
    }

    async fn on_turn_error_with_fingerprint(
        &self,
        input: TurnErrorInput<'_>,
        error_fingerprint: &str,
    ) {
        self.handle_turn_error(input, error_fingerprint).await;
    }
}

#[async_trait]
impl<C> TokenUsageContributor for GoalExtension<C>
where
    C: Send + Sync + 'static,
{
    async fn on_token_usage(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        turn_store: &ExtensionData,
        token_usage: &TokenUsageInfo,
    ) {
        let Some(runtime) = goal_runtime_handle(thread_store) else {
            return;
        };
        if !runtime.is_enabled() {
            return;
        }

        let Some(_recorded) = runtime
            .accounting_state()
            .record_token_usage(turn_store.level_id(), &token_usage.total_token_usage)
        else {
            return;
        };
    }
}

impl<C> ToolLifecycleContributor for GoalExtension<C>
where
    C: Send + Sync + 'static,
{
    fn on_tool_start<'a>(&'a self, input: ToolStartInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            let Some(runtime) = goal_runtime_handle(input.thread_store) else {
                return;
            };
            if !runtime.is_enabled() {
                return;
            }
            let accounting = runtime.accounting_state();
            let Some(goal_id) = accounting.current_turn_line_change_retry_goal_id(input.turn_id)
            else {
                return;
            };
            let Ok(Some(goal)) = self
                .state_dbs
                .thread_goals()
                .get_thread_goal(runtime.thread_id())
                .await
            else {
                return;
            };
            if goal.goal_id == goal_id {
                crate::line_changes::establish_current_turn_baseline(accounting.as_ref(), &goal)
                    .await;
            }
        })
    }

    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            let Some(runtime) = goal_runtime_handle(input.thread_store) else {
                return;
            };
            let should_count_for_goal_progress = runtime.is_enabled()
                && tool_attempt_counts_for_goal_progress(input.outcome)
                && !(input.tool_name.namespace.is_none()
                    && matches!(
                        input.tool_name.name.as_str(),
                        UPDATE_GOAL_TOOL_NAME
                            | PAUSE_GOAL_TOOL_NAME
                            | RESUME_GOAL_TOOL_NAME
                            | GET_GOAL_PLAN_TOOL_NAME
                            | CREATE_GOAL_PLAN_TOOL_NAME
                            | ACTIVATE_GOAL_PLAN_NODE_TOOL_NAME
                            | UPDATE_GOAL_PLAN_NODE_TOOL_NAME
                            | INSERT_GOAL_PLAN_NODE_TOOL_NAME
                            | SET_GOAL_PLAN_NODE_STATUS_TOOL_NAME
                    ));
            if !should_count_for_goal_progress {
                return;
            }
            let turn_id = input.turn_id;
            let progress = match runtime
                .account_active_goal_progress(
                    turn_id,
                    input.call_id,
                    line_change_accounting_for_tool(input.worktree_mutation_signal),
                    codex_state::GoalAccountingMode::ActiveOnly,
                    BudgetLimitedGoalDisposition::KeepActive,
                )
                .await
            {
                Ok(Some(progress)) => progress,
                Ok(None) => return,
                Err(err) => {
                    tracing::warn!(
                        "failed to account active goal progress after tool finish for {turn_id}: {err}"
                    );
                    return;
                }
            };
            let goal = progress.goal;
            if goal.status != ThreadGoalStatus::BudgetLimited {
                return;
            }
            if !runtime
                .accounting_state()
                .mark_budget_limit_reported_if_new(progress.goal_id.as_str())
            {
                return;
            }
            let item = budget_limit_steering_item(&goal);
            runtime.inject_active_turn_steering(item).await;
        })
    }
}

impl<C> ToolContributor for GoalExtension<C>
where
    C: Send + Sync + 'static,
{
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>> {
        let Some(runtime) = goal_runtime_handle(thread_store) else {
            return Vec::new();
        };
        if !runtime.tools_visible() {
            return Vec::new();
        }

        vec![
            Arc::new(GoalToolExecutor::get(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
            )),
            Arc::new(GoalToolExecutor::get_plan(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::create(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
            )),
            Arc::new(GoalToolExecutor::create_plan(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::activate_plan_node(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::update_plan_node(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::insert_plan_node(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::set_plan_node_status(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::update(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::pause(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
                runtime.plan_config_handle(),
            )),
            Arc::new(GoalToolExecutor::resume(
                runtime.thread_id(),
                Arc::clone(&self.state_dbs),
                runtime.accounting_state(),
                self.event_emitter.clone(),
                self.metrics.clone(),
            )),
        ]
    }
}

pub fn install_with_backend<C>(
    registry: &mut ExtensionRegistryBuilder<C>,
    state_dbs: Arc<codex_state::StateRuntime>,
    metrics_client: Option<MetricsClient>,
    thread_manager: Weak<ThreadManager>,
    goal_service: Arc<GoalService>,
    goals_config: impl Fn(&C) -> GoalExtensionConfig + Send + Sync + 'static,
) where
    C: Send + Sync + 'static,
{
    let extension = Arc::new(GoalExtension::new_with_host_capabilities(
        state_dbs,
        registry.event_sink(),
        metrics_client,
        thread_manager,
        Arc::clone(&goal_service),
        goals_config,
    ));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.turn_lifecycle_contributor(extension.clone());
    registry.token_usage_contributor(extension.clone());
    registry.tool_lifecycle_contributor(extension.clone());
    registry.tool_contributor(extension);
}

fn active_plan_requires_guard(
    plan: &codex_state::ThreadGoalPlanSnapshot,
    thread_id: ThreadId,
) -> bool {
    if plan.plan.thread_id != thread_id
        || plan.plan.status != codex_state::ThreadGoalPlanStatus::Active
    {
        return false;
    }
    let summary = plan.usage_summary();
    summary.active_node_count == 0
        && summary.paused_node_count == 0
        && summary.blocked_node_count == 0
        && summary.usage_limited_node_count == 0
        && summary.budget_limited_node_count == 0
        && summary.pending_node_count + summary.deferred_node_count > 0
}

fn goal_runtime_handle(thread_store: &ExtensionData) -> Option<Arc<GoalRuntimeHandle>> {
    thread_store.get::<GoalRuntimeHandle>()
}

fn protocol_error_fingerprint(error: &CodexErrorInfo) -> String {
    let fingerprint = match error {
        CodexErrorInfo::ContextWindowExceeded => "codex_err:protocol:context_window_exceeded",
        CodexErrorInfo::UsageLimitExceeded => "codex_err:protocol:usage_limit_exceeded",
        CodexErrorInfo::ServerOverloaded => "codex_err:protocol:server_overloaded",
        CodexErrorInfo::CyberPolicy => "codex_err:protocol:cyber_policy",
        CodexErrorInfo::HttpConnectionFailed { http_status_code } => {
            return protocol_http_error_fingerprint("http_connection_failed", *http_status_code);
        }
        CodexErrorInfo::ResponseStreamConnectionFailed { http_status_code } => {
            return protocol_http_error_fingerprint(
                "response_stream_connection_failed",
                *http_status_code,
            );
        }
        CodexErrorInfo::InternalServerError => "codex_err:protocol:internal_server_error",
        CodexErrorInfo::Unauthorized => "codex_err:protocol:unauthorized",
        CodexErrorInfo::BadRequest => "codex_err:protocol:bad_request",
        CodexErrorInfo::SandboxError => "codex_err:protocol:sandbox_error",
        CodexErrorInfo::ResponseStreamDisconnected { http_status_code } => {
            return protocol_http_error_fingerprint(
                "response_stream_disconnected",
                *http_status_code,
            );
        }
        CodexErrorInfo::ResponseTooManyFailedAttempts { http_status_code } => {
            return protocol_http_error_fingerprint(
                "response_too_many_failed_attempts",
                *http_status_code,
            );
        }
        CodexErrorInfo::ActiveTurnNotSteerable { turn_kind } => match turn_kind {
            codex_protocol::protocol::NonSteerableTurnKind::Review => {
                "codex_err:protocol:active_turn_not_steerable:review"
            }
            codex_protocol::protocol::NonSteerableTurnKind::Compact => {
                "codex_err:protocol:active_turn_not_steerable:compact"
            }
        },
        CodexErrorInfo::ThreadRollbackFailed => "codex_err:protocol:thread_rollback_failed",
        CodexErrorInfo::Other => "codex_err:protocol:other",
    };
    fingerprint.to_string()
}

fn protocol_http_error_fingerprint(kind: &str, status: Option<u16>) -> String {
    match status {
        Some(status) => format!("codex_err:protocol:{kind}:http_{status}"),
        None => format!("codex_err:protocol:{kind}:http_unknown"),
    }
}

fn tool_attempt_counts_for_goal_progress(outcome: ToolCallOutcome) -> bool {
    match outcome {
        ToolCallOutcome::Completed { .. } => true,
        ToolCallOutcome::Failed {
            handler_executed: true,
        } => true,
        ToolCallOutcome::Blocked
        | ToolCallOutcome::Failed {
            handler_executed: false,
        }
        | ToolCallOutcome::Aborted => false,
    }
}

fn line_change_accounting_for_tool(
    worktree_mutation_signal: ToolWorktreeMutationSignal,
) -> LineChangeAccounting {
    match worktree_mutation_signal {
        ToolWorktreeMutationSignal::NoWorktreeMutation => LineChangeAccounting::Skip,
        ToolWorktreeMutationSignal::ConfirmedWorktreeMutation => LineChangeAccounting::Capture,
        ToolWorktreeMutationSignal::MaybeMutatesWorktree => LineChangeAccounting::Skip,
    }
}
