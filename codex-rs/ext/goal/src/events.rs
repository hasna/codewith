use std::sync::Arc;

use codex_extension_api::ExtensionEventSink;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadGoal;
use codex_protocol::protocol::ThreadGoalPlan;
use codex_protocol::protocol::ThreadGoalPlanAutoExecute;
use codex_protocol::protocol::ThreadGoalPlanNode;
use codex_protocol::protocol::ThreadGoalPlanNodeStatus;
use codex_protocol::protocol::ThreadGoalPlanStatus;
use codex_protocol::protocol::ThreadGoalPlanUpdatedEvent;
use codex_protocol::protocol::ThreadGoalUpdatedEvent;

const MAX_GOAL_PLAN_EVENT_NODES: usize = 16;
const MAX_GOAL_PLAN_EVENT_OBJECTIVE_CHARS: usize = 512;

#[derive(Clone)]
pub(crate) struct GoalEventEmitter {
    sink: Arc<dyn ExtensionEventSink>,
}

impl GoalEventEmitter {
    pub(crate) fn new(sink: Arc<dyn ExtensionEventSink>) -> Self {
        Self { sink }
    }

    pub(crate) fn thread_goal_updated(
        &self,
        event_id: impl Into<String>,
        turn_id: Option<String>,
        goal: ThreadGoal,
    ) {
        self.sink.emit(Event {
            id: event_id.into(),
            msg: EventMsg::ThreadGoalUpdated(ThreadGoalUpdatedEvent {
                thread_id: goal.thread_id,
                turn_id,
                goal,
            }),
        });
    }

    pub(crate) fn thread_goal_plan_updated(
        &self,
        event_id: impl Into<String>,
        turn_id: Option<String>,
        snapshot: codex_state::ThreadGoalPlanSnapshot,
    ) {
        let thread_id = snapshot.plan.thread_id;
        self.sink.emit(Event {
            id: event_id.into(),
            msg: EventMsg::ThreadGoalPlanUpdated(ThreadGoalPlanUpdatedEvent {
                thread_id,
                turn_id,
                plan: protocol_goal_plan_from_state(snapshot),
            }),
        });
    }
}

fn protocol_goal_plan_from_state(snapshot: codex_state::ThreadGoalPlanSnapshot) -> ThreadGoalPlan {
    let summary = snapshot.usage_summary();
    let ready_node_ids = snapshot
        .ready_node_ids()
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    ThreadGoalPlan {
        plan_id: snapshot.plan.plan_id,
        thread_id: snapshot.plan.thread_id,
        status: protocol_goal_plan_status_from_state(snapshot.plan.status),
        auto_execute: protocol_goal_plan_auto_execute_from_state(snapshot.plan.auto_execute),
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
        deferred_node_count: summary.deferred_node_count,
        cancelled_node_count: summary.cancelled_node_count,
        created_at: snapshot.plan.created_at.timestamp(),
        updated_at: snapshot.plan.updated_at.timestamp(),
        nodes: snapshot
            .nodes
            .into_iter()
            .take(MAX_GOAL_PLAN_EVENT_NODES)
            .map(|node| {
                let ready = ready_node_ids.contains(&node.node_id);
                protocol_goal_plan_node_from_state(node, ready)
            })
            .collect(),
    }
}

fn protocol_goal_plan_node_from_state(
    node: codex_state::ThreadGoalPlanNode,
    ready: bool,
) -> ThreadGoalPlanNode {
    let objective = if node.objective.chars().count() <= MAX_GOAL_PLAN_EVENT_OBJECTIVE_CHARS {
        node.objective
    } else {
        let mut truncated = node
            .objective
            .chars()
            .take(MAX_GOAL_PLAN_EVENT_OBJECTIVE_CHARS.saturating_sub(3))
            .collect::<String>();
        truncated.push_str("...");
        truncated
    };

    ThreadGoalPlanNode {
        node_id: node.node_id,
        plan_id: node.plan_id,
        thread_id: node.thread_id,
        assigned_thread_id: Some(node.assigned_thread_id),
        key: node.key,
        sequence: node.sequence,
        priority: node.priority,
        objective,
        title: node.title,
        status: protocol_goal_plan_node_status_from_state(node.status),
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

fn protocol_goal_plan_status_from_state(
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

fn protocol_goal_plan_auto_execute_from_state(
    auto_execute: codex_state::ThreadGoalPlanAutoExecute,
) -> ThreadGoalPlanAutoExecute {
    match auto_execute {
        codex_state::ThreadGoalPlanAutoExecute::Off => ThreadGoalPlanAutoExecute::Off,
        codex_state::ThreadGoalPlanAutoExecute::ReadyOnly => ThreadGoalPlanAutoExecute::ReadyOnly,
        codex_state::ThreadGoalPlanAutoExecute::AiDirected => ThreadGoalPlanAutoExecute::AiDirected,
    }
}

fn protocol_goal_plan_node_status_from_state(
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
