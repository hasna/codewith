use super::*;
use codex_goal_extension::GoalObjectiveUpdate;
use codex_goal_extension::GoalPlanAddRequest;
use codex_goal_extension::GoalService;
use codex_goal_extension::GoalServiceError;
use codex_goal_extension::GoalSetRequest;
use codex_goal_extension::GoalTitleUpdate;
use codex_goal_extension::GoalTokenBudgetUpdate;

#[derive(Clone)]
pub(crate) struct ThreadGoalRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    thread_state_manager: ThreadStateManager,
    state_db: Option<StateDbHandle>,
    goal_service: Arc<GoalService>,
}

impl ThreadGoalRequestProcessor {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        thread_state_manager: ThreadStateManager,
        state_db: Option<StateDbHandle>,
        goal_service: Arc<GoalService>,
    ) -> Self {
        Self {
            thread_manager,
            outgoing,
            config,
            thread_state_manager,
            state_db,
            goal_service,
        }
    }

    pub(crate) async fn thread_goal_set(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalSetParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_goal_set_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_goal_get(
        &self,
        params: ThreadGoalGetParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_goal_get_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_goal_list(
        &self,
        params: ThreadGoalListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_goal_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_goal_plan_activate_node(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalPlanActivateNodeParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_goal_plan_activate_node_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_goal_plan_add_goal(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalPlanAddGoalParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_goal_plan_add_goal_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn thread_goal_clear(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalClearParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_goal_clear_inner(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn emit_resume_goal_snapshot_and_continue(
        &self,
        thread_id: ThreadId,
        thread: &CodexThread,
    ) {
        if !self.config.features.enabled(Feature::Goals) {
            return;
        }
        self.emit_thread_goal_snapshot(thread_id).await;
        // App-server owns resume response and snapshot ordering, so wait until
        // those are sent before letting extensions react to the idle thread.
        thread.emit_thread_idle_lifecycle_if_idle().await;
    }

    pub(crate) async fn pending_resume_goal_state(
        &self,
        thread: &CodexThread,
    ) -> (bool, Option<StateDbHandle>) {
        let emit_thread_goal_update = self.config.features.enabled(Feature::Goals);
        let thread_goal_state_db = if emit_thread_goal_update {
            if let Some(state_db) = thread.state_db() {
                Some(state_db)
            } else {
                self.state_db.clone()
            }
        } else {
            None
        };
        (emit_thread_goal_update, thread_goal_state_db)
    }

    async fn thread_goal_set_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalSetParams,
    ) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Goals) {
            return Err(invalid_request("goals feature is disabled"));
        }

        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        self.reconcile_thread_goal_rollout(thread_id, &state_db)
            .await?;

        let listener_command_tx = {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        let status = params.status.map(ThreadGoalStatus::to_core);
        let objective = params.objective.as_deref();
        let title = params.title.as_ref().map(|title| title.as_deref());

        let outcome = self
            .goal_service
            .set_thread_goal(
                &state_db,
                GoalSetRequest {
                    thread_id,
                    objective: objective
                        .map(GoalObjectiveUpdate::Set)
                        .unwrap_or(GoalObjectiveUpdate::Keep),
                    title: title
                        .map(GoalTitleUpdate::Set)
                        .unwrap_or(GoalTitleUpdate::Keep),
                    status,
                    token_budget: match params.token_budget {
                        Some(token_budget) => GoalTokenBudgetUpdate::Set(token_budget),
                        None => GoalTokenBudgetUpdate::Keep,
                    },
                    auto_execute: goal_auto_execute_from_config(&self.config),
                },
            )
            .await
            .map_err(goal_service_error)?;
        let goal = ThreadGoal::from(outcome.goal.clone());
        self.outgoing
            .send_response(
                request_id.clone(),
                ThreadGoalSetResponse { goal: goal.clone() },
            )
            .await;
        self.emit_thread_goal_updated_ordered(thread_id, goal, listener_command_tx)
            .await;
        if let Some(plan_update) = outcome.plan_update.clone() {
            self.emit_thread_goal_plan_snapshot_updated_ordered(
                thread_id,
                plan_update.snapshot,
                /*listener_command_tx*/ None,
            )
            .await;
            if let Some(activated_goal) = plan_update.activated_goal {
                self.emit_thread_goal_updated_ordered(
                    thread_id,
                    api_thread_goal_from_state(activated_goal),
                    /*listener_command_tx*/ None,
                )
                .await;
            }
        }
        outcome.apply_runtime_effects(&self.goal_service).await;
        Ok(())
    }

    async fn thread_goal_get_inner(
        &self,
        params: ThreadGoalGetParams,
    ) -> Result<ThreadGoalGetResponse, JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Goals) {
            return Err(invalid_request("goals feature is disabled"));
        }

        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let goal = self
            .goal_service
            .get_thread_goal(&state_db, thread_id)
            .await
            .map_err(goal_service_error)?
            .map(ThreadGoal::from);
        Ok(ThreadGoalGetResponse { goal })
    }

    async fn thread_goal_list_inner(
        &self,
        params: ThreadGoalListParams,
    ) -> Result<ThreadGoalListResponse, JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Goals) {
            return Err(invalid_request("goals feature is disabled"));
        }

        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        let goal = self
            .goal_service
            .get_thread_goal(&state_db, thread_id)
            .await
            .map_err(goal_service_error)?
            .map(ThreadGoal::from);
        let goal_plan_page = state_db
            .thread_goals()
            .list_thread_goal_plans_page(
                thread_id,
                params.cursor.as_deref(),
                params
                    .limit
                    .unwrap_or(codex_state::DEFAULT_THREAD_GOAL_PLAN_LIST_LIMIT),
            )
            .await
            .map_err(|err| invalid_request(format!("failed to read thread goal plans: {err}")))?;
        let goal_plans = goal_plan_page
            .data
            .into_iter()
            .map(|snapshot| api_thread_goal_plan_from_state_for_thread(snapshot, thread_id))
            .collect();
        Ok(ThreadGoalListResponse {
            goal,
            goal_plans,
            next_cursor: goal_plan_page.next_cursor,
        })
    }

    async fn thread_goal_clear_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalClearParams,
    ) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Goals) {
            return Err(invalid_request("goals feature is disabled"));
        }

        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        self.reconcile_thread_goal_rollout(thread_id, &state_db)
            .await?;

        let listener_command_tx = {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        let outcome = self
            .goal_service
            .clear_thread_goal(&state_db, thread_id)
            .await
            .map_err(goal_service_error)?;

        self.outgoing
            .send_response(
                request_id,
                ThreadGoalClearResponse {
                    cleared: outcome.cleared,
                },
            )
            .await;
        if outcome.cleared {
            self.emit_thread_goal_cleared_ordered(thread_id, listener_command_tx.clone())
                .await;
            for plan_update in outcome.plan_updates {
                self.emit_thread_goal_plan_snapshot_updated_ordered(
                    thread_id,
                    plan_update,
                    listener_command_tx.clone(),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn thread_goal_plan_activate_node_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalPlanActivateNodeParams,
    ) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Goals) {
            return Err(invalid_request("goals feature is disabled"));
        }

        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        self.reconcile_thread_goal_rollout(thread_id, &state_db)
            .await?;

        let listener_command_tx = {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        let outcome = self
            .goal_service
            .activate_thread_goal_plan_node(&state_db, thread_id, params.node_id.as_str())
            .await
            .map_err(goal_service_error)?;
        let goal = ThreadGoal::from(outcome.goal.clone());
        let plan = api_thread_goal_plan_from_state_for_thread(outcome.plan.clone(), thread_id);
        self.outgoing
            .send_response(
                request_id,
                ThreadGoalPlanActivateNodeResponse {
                    goal: goal.clone(),
                    plan: plan.clone(),
                },
            )
            .await;
        self.emit_thread_goal_updated_ordered(thread_id, goal, listener_command_tx.clone())
            .await;
        self.emit_thread_goal_plan_snapshot_updated_ordered(
            thread_id,
            outcome.plan.clone(),
            listener_command_tx,
        )
        .await;
        outcome.apply_runtime_effects(&self.goal_service).await;
        Ok(())
    }

    async fn thread_goal_plan_add_goal_inner(
        &self,
        request_id: ConnectionRequestId,
        params: ThreadGoalPlanAddGoalParams,
    ) -> Result<(), JSONRPCErrorError> {
        if !self.config.features.enabled(Feature::Goals) {
            return Err(invalid_request("goals feature is disabled"));
        }

        let thread_id = parse_thread_id_for_request(params.thread_id.as_str())?;
        let state_db = self.state_db_for_materialized_thread(thread_id).await?;
        self.reconcile_thread_goal_rollout(thread_id, &state_db)
            .await?;

        let listener_command_tx = {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        let outcome = self
            .goal_service
            .add_thread_goal_to_plan(
                &state_db,
                GoalPlanAddRequest {
                    thread_id,
                    objective: params.objective.as_str(),
                    title: None,
                    token_budget: None,
                    auto_execute: goal_auto_execute_from_config(&self.config),
                },
            )
            .await
            .map_err(goal_service_error)?;
        let goal = outcome.goal.clone().map(ThreadGoal::from);
        let plan = api_thread_goal_plan_from_state_for_thread(outcome.plan.clone(), thread_id);
        let added_node = plan
            .nodes
            .iter()
            .find(|node| node.node_id == outcome.added_node.node_id)
            .cloned()
            .ok_or_else(|| internal_error("added goal-plan node missing from snapshot"))?;
        self.outgoing
            .send_response(
                request_id,
                ThreadGoalPlanAddGoalResponse {
                    goal,
                    plan: plan.clone(),
                    added_node,
                    created_plan: outcome.created_plan,
                },
            )
            .await;
        if let Some(activated_goal) = outcome.activated_goal.clone() {
            self.emit_thread_goal_updated_ordered(
                thread_id,
                ThreadGoal::from(activated_goal),
                listener_command_tx.clone(),
            )
            .await;
        }
        self.emit_thread_goal_plan_snapshot_updated_ordered(
            thread_id,
            outcome.plan.clone(),
            listener_command_tx,
        )
        .await;
        outcome.apply_runtime_effects(&self.goal_service).await;
        Ok(())
    }

    async fn state_db_for_materialized_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<StateDbHandle, JSONRPCErrorError> {
        if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
            if thread.rollout_path().is_none() {
                return Err(invalid_request(format!(
                    "ephemeral thread does not support goals: {thread_id}"
                )));
            }
            if let Some(state_db) = thread.state_db() {
                return Ok(state_db);
            }
        } else {
            codex_rollout::find_thread_path_by_id_str(
                &self.config.codex_home,
                &thread_id.to_string(),
                self.state_db.as_deref(),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to locate thread id {thread_id}: {err}"))
            })?
            .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?;
        }

        self.state_db
            .clone()
            .ok_or_else(|| internal_error("sqlite state db unavailable for thread goals"))
    }

    async fn reconcile_thread_goal_rollout(
        &self,
        thread_id: ThreadId,
        state_db: &StateDbHandle,
    ) -> Result<(), JSONRPCErrorError> {
        let running_thread = self.thread_manager.get_thread(thread_id).await.ok();
        let rollout_path = match running_thread.as_ref() {
            Some(thread) => thread.rollout_path().ok_or_else(|| {
                invalid_request(format!(
                    "ephemeral thread does not support goals: {thread_id}"
                ))
            })?,
            None => codex_rollout::find_thread_path_by_id_str(
                &self.config.codex_home,
                &thread_id.to_string(),
                self.state_db.as_deref(),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to locate thread id {thread_id}: {err}"))
            })?
            .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?,
        };
        reconcile_rollout(
            Some(state_db),
            rollout_path.as_path(),
            self.config.model_provider_id.as_str(),
            /*builder*/ None,
            &[],
            /*archived_only*/ None,
            /*new_thread_memory_mode*/ None,
        )
        .await;
        Ok(())
    }

    async fn emit_thread_goal_snapshot(&self, thread_id: ThreadId) {
        let state_db = match self.state_db_for_materialized_thread(thread_id).await {
            Ok(state_db) => state_db,
            Err(err) => {
                warn!(
                    "failed to open state db before emitting thread goal resume snapshot for {thread_id}: {}",
                    err.message
                );
                return;
            }
        };
        let listener_command_tx = {
            let thread_state = self.thread_state_manager.thread_state(thread_id).await;
            let thread_state = thread_state.lock().await;
            thread_state.listener_command_tx()
        };
        if let Some(listener_command_tx) = listener_command_tx {
            let command = crate::thread_state::ThreadListenerCommand::EmitThreadGoalSnapshot {
                state_db: state_db.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue thread goal snapshot for {thread_id}: listener command channel is closed"
            );
        }
        send_thread_goal_snapshot_notification(&self.outgoing, thread_id, &state_db).await;
    }

    async fn emit_thread_goal_updated_ordered(
        &self,
        thread_id: ThreadId,
        goal: ThreadGoal,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = crate::thread_state::ThreadListenerCommand::EmitThreadGoalUpdated {
                turn_id: None,
                goal: goal.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue thread goal update for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadGoalUpdated(
                ThreadGoalUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    turn_id: None,
                    goal,
                },
            ))
            .await;
    }

    async fn emit_thread_goal_cleared_ordered(
        &self,
        thread_id: ThreadId,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = crate::thread_state::ThreadListenerCommand::EmitThreadGoalCleared;
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue thread goal clear for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadGoalCleared(
                ThreadGoalClearedNotification {
                    thread_id: thread_id.to_string(),
                },
            ))
            .await;
    }

    async fn emit_thread_goal_plan_snapshot_updated_ordered(
        &self,
        request_thread_id: ThreadId,
        snapshot: codex_state::ThreadGoalPlanSnapshot,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        for target_thread_id in snapshot.participant_thread_ids() {
            let plan =
                api_thread_goal_plan_from_state_for_thread(snapshot.clone(), target_thread_id);
            let listener_command_tx = if target_thread_id == request_thread_id {
                listener_command_tx.clone()
            } else {
                self.thread_state_manager
                    .current_listener_command_tx(target_thread_id)
            };
            self.emit_thread_goal_plan_updated_ordered(target_thread_id, plan, listener_command_tx)
                .await;
        }
    }

    async fn emit_thread_goal_plan_updated_ordered(
        &self,
        thread_id: ThreadId,
        plan: ThreadGoalPlan,
        listener_command_tx: Option<tokio::sync::mpsc::UnboundedSender<ThreadListenerCommand>>,
    ) {
        if let Some(listener_command_tx) = listener_command_tx {
            let command = crate::thread_state::ThreadListenerCommand::EmitThreadGoalPlanUpdated {
                turn_id: None,
                plan: plan.clone(),
            };
            if listener_command_tx.send(command).is_ok() {
                return;
            }
            warn!(
                "failed to enqueue thread goal plan update for {thread_id}: listener command channel is closed"
            );
        }
        self.outgoing
            .send_server_notification(ServerNotification::ThreadGoalPlanUpdated(
                ThreadGoalPlanUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    turn_id: None,
                    plan,
                },
            ))
            .await;
    }
}

pub(super) fn api_thread_goal_from_state(goal: codex_state::ThreadGoal) -> ThreadGoal {
    ThreadGoal {
        thread_id: goal.thread_id.to_string(),
        goal_id: goal.goal_id,
        objective: goal.objective,
        title: goal.title,
        status: api_thread_goal_status_from_state(goal.status),
        token_budget: goal.token_budget,
        tokens_used: goal.tokens_used,
        time_used_seconds: goal.time_used_seconds,
        created_at: goal.created_at.timestamp(),
        updated_at: goal.updated_at.timestamp(),
    }
}

pub(super) fn goal_auto_execute_from_config(
    config: &Config,
) -> codex_state::ThreadGoalPlanAutoExecute {
    match config.goals.auto_execute {
        codex_core::config::GoalAutoExecuteMode::Off => codex_state::ThreadGoalPlanAutoExecute::Off,
        codex_core::config::GoalAutoExecuteMode::ReadyOnly => {
            codex_state::ThreadGoalPlanAutoExecute::ReadyOnly
        }
        codex_core::config::GoalAutoExecuteMode::AiDirected => {
            codex_state::ThreadGoalPlanAutoExecute::AiDirected
        }
    }
}

pub(super) fn api_thread_goal_plan_from_state(
    snapshot: codex_state::ThreadGoalPlanSnapshot,
) -> ThreadGoalPlan {
    let thread_id = snapshot.plan.thread_id;
    api_thread_goal_plan_from_state_for_thread(snapshot, thread_id)
}

pub(crate) fn api_thread_goal_plan_from_state_for_thread(
    snapshot: codex_state::ThreadGoalPlanSnapshot,
    thread_id: ThreadId,
) -> ThreadGoalPlan {
    let summary = snapshot.usage_summary();
    let ready_node_ids = snapshot
        .ready_node_ids_for_thread(thread_id)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let ready_node_count = i64::try_from(ready_node_ids.len()).unwrap_or(i64::MAX);
    ThreadGoalPlan {
        plan_id: snapshot.plan.plan_id.clone(),
        thread_id: snapshot.plan.thread_id.to_string(),
        status: api_thread_goal_plan_status_from_state(snapshot.plan.status),
        auto_execute: api_thread_goal_plan_auto_execute_from_state(snapshot.plan.auto_execute),
        max_tokens: snapshot.plan.max_tokens,
        total_tokens_used: summary.total_tokens_used,
        total_time_used_seconds: summary.total_time_used_seconds,
        remaining_tokens: summary.remaining_tokens,
        node_count: summary.node_count,
        completed_node_count: summary.completed_node_count,
        ready_node_count,
        active_node_count: summary.active_node_count,
        pending_node_count: summary.pending_node_count,
        paused_node_count: summary.paused_node_count,
        blocked_node_count: summary.blocked_node_count,
        usage_limited_node_count: summary.usage_limited_node_count,
        budget_limited_node_count: summary.budget_limited_node_count,
        deferred_node_count: summary.deferred_node_count,
        cancelled_node_count: summary.cancelled_node_count,
        created_at: snapshot.plan.created_at.timestamp(),
        updated_at: snapshot.plan.updated_at.timestamp(),
        nodes: snapshot
            .visible_nodes_for_thread(thread_id, usize::MAX)
            .into_iter()
            .map(|node| {
                let ready = ready_node_ids.contains(&node.node_id);
                api_thread_goal_plan_node_from_state(node.clone(), ready)
            })
            .collect(),
    }
}

fn api_thread_goal_plan_node_from_state(
    node: codex_state::ThreadGoalPlanNode,
    ready: bool,
) -> ThreadGoalPlanNode {
    ThreadGoalPlanNode {
        node_id: node.node_id,
        plan_id: node.plan_id,
        thread_id: node.thread_id.to_string(),
        assigned_thread_id: node.assigned_thread_id.to_string(),
        key: node.key,
        sequence: node.sequence,
        priority: node.priority,
        objective: node.objective,
        title: node.title,
        status: api_thread_goal_plan_node_status_from_state(node.status),
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

fn api_thread_goal_status_from_state(status: codex_state::ThreadGoalStatus) -> ThreadGoalStatus {
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

fn api_thread_goal_plan_status_from_state(
    status: codex_state::ThreadGoalPlanStatus,
) -> ThreadGoalPlanStatus {
    match status {
        codex_state::ThreadGoalPlanStatus::Active => ThreadGoalPlanStatus::Active,
        codex_state::ThreadGoalPlanStatus::Paused => ThreadGoalPlanStatus::Paused,
        codex_state::ThreadGoalPlanStatus::Blocked => ThreadGoalPlanStatus::Blocked,
        codex_state::ThreadGoalPlanStatus::BudgetLimited => ThreadGoalPlanStatus::BudgetLimited,
        codex_state::ThreadGoalPlanStatus::Complete => ThreadGoalPlanStatus::Complete,
        codex_state::ThreadGoalPlanStatus::Cancelled => ThreadGoalPlanStatus::Cancelled,
    }
}

fn api_thread_goal_plan_auto_execute_from_state(
    auto_execute: codex_state::ThreadGoalPlanAutoExecute,
) -> ThreadGoalPlanAutoExecute {
    match auto_execute {
        codex_state::ThreadGoalPlanAutoExecute::Off => ThreadGoalPlanAutoExecute::Off,
        codex_state::ThreadGoalPlanAutoExecute::ReadyOnly => ThreadGoalPlanAutoExecute::ReadyOnly,
        codex_state::ThreadGoalPlanAutoExecute::AiDirected => ThreadGoalPlanAutoExecute::AiDirected,
    }
}

fn api_thread_goal_plan_node_status_from_state(
    status: codex_state::ThreadGoalPlanNodeStatus,
) -> ThreadGoalPlanNodeStatus {
    match status {
        codex_state::ThreadGoalPlanNodeStatus::Pending => ThreadGoalPlanNodeStatus::Pending,
        codex_state::ThreadGoalPlanNodeStatus::Active => ThreadGoalPlanNodeStatus::Active,
        codex_state::ThreadGoalPlanNodeStatus::Paused => ThreadGoalPlanNodeStatus::Paused,
        codex_state::ThreadGoalPlanNodeStatus::Blocked => ThreadGoalPlanNodeStatus::Blocked,
        codex_state::ThreadGoalPlanNodeStatus::UsageLimited => {
            ThreadGoalPlanNodeStatus::UsageLimited
        }
        codex_state::ThreadGoalPlanNodeStatus::BudgetLimited => {
            ThreadGoalPlanNodeStatus::BudgetLimited
        }
        codex_state::ThreadGoalPlanNodeStatus::Deferred => ThreadGoalPlanNodeStatus::Deferred,
        codex_state::ThreadGoalPlanNodeStatus::Complete => ThreadGoalPlanNodeStatus::Complete,
        codex_state::ThreadGoalPlanNodeStatus::Cancelled => ThreadGoalPlanNodeStatus::Cancelled,
    }
}

fn goal_service_error(err: GoalServiceError) -> JSONRPCErrorError {
    match err {
        GoalServiceError::InvalidRequest(message) => invalid_request(message),
        GoalServiceError::Internal(message) => internal_error(message),
    }
}

fn parse_thread_id_for_request(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}
