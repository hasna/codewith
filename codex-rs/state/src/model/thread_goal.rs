use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use std::collections::HashSet;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
    Cancelled,
}

impl ThreadGoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        self == Self::Active
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::BudgetLimited | Self::Complete | Self::Cancelled)
    }
}

impl TryFrom<&str> for ThreadGoalStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "blocked" => Ok(Self::Blocked),
            "usage_limited" => Ok(Self::UsageLimited),
            "budget_limited" => Ok(Self::BudgetLimited),
            "complete" => Ok(Self::Complete),
            "cancelled" | "canceled" => Ok(Self::Cancelled),
            other => Err(anyhow!("unknown thread goal status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoal {
    pub thread_id: ThreadId,
    pub goal_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalPlanStatus {
    Active,
    Paused,
    Blocked,
    BudgetLimited,
    Complete,
    Cancelled,
}

impl ThreadGoalPlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
        }
    }
}

impl TryFrom<&str> for ThreadGoalPlanStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "blocked" => Ok(Self::Blocked),
            "budget_limited" => Ok(Self::BudgetLimited),
            "complete" => Ok(Self::Complete),
            "cancelled" | "canceled" => Ok(Self::Cancelled),
            other => Err(anyhow!("unknown thread goal plan status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalPlanAutoExecute {
    Off,
    ReadyOnly,
    /// Activate the highest-priority ready node without asking the model to choose.
    ///
    /// This continues to serialize as `ai_directed` for persisted config and DB
    /// compatibility with earlier goal-plan experiments.
    AiDirected,
}

impl ThreadGoalPlanAutoExecute {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ReadyOnly => "ready_only",
            Self::AiDirected => "ai_directed",
        }
    }
}

impl TryFrom<&str> for ThreadGoalPlanAutoExecute {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "off" => Ok(Self::Off),
            "ready_only" => Ok(Self::ReadyOnly),
            "ai_directed" | "priority_first" => Ok(Self::AiDirected),
            other => Err(anyhow!(
                "unknown thread goal plan auto-execute mode `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalPlanNodeStatus {
    Pending,
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
    Cancelled,
}

impl ThreadGoalPlanNodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
        }
    }
}

impl TryFrom<&str> for ThreadGoalPlanNodeStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "blocked" => Ok(Self::Blocked),
            "usage_limited" => Ok(Self::UsageLimited),
            "budget_limited" => Ok(Self::BudgetLimited),
            "complete" => Ok(Self::Complete),
            "cancelled" | "canceled" => Ok(Self::Cancelled),
            other => Err(anyhow!("unknown thread goal plan node status `{other}`")),
        }
    }
}

impl From<ThreadGoalStatus> for ThreadGoalPlanNodeStatus {
    fn from(status: ThreadGoalStatus) -> Self {
        match status {
            ThreadGoalStatus::Active => Self::Active,
            ThreadGoalStatus::Paused => Self::Paused,
            ThreadGoalStatus::Blocked => Self::Blocked,
            ThreadGoalStatus::UsageLimited => Self::UsageLimited,
            ThreadGoalStatus::BudgetLimited => Self::BudgetLimited,
            ThreadGoalStatus::Complete => Self::Complete,
            ThreadGoalStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlan {
    pub plan_id: String,
    pub thread_id: ThreadId,
    pub status: ThreadGoalPlanStatus,
    pub auto_execute: ThreadGoalPlanAutoExecute,
    pub max_tokens: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanNode {
    pub node_id: String,
    pub plan_id: String,
    pub thread_id: ThreadId,
    pub key: String,
    pub sequence: i64,
    pub priority: i64,
    pub objective: String,
    pub status: ThreadGoalPlanNodeStatus,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub projected_goal_id: Option<String>,
    pub depends_on: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanSnapshot {
    pub plan: ThreadGoalPlan,
    pub nodes: Vec<ThreadGoalPlanNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadGoalPlanUsageSummary {
    pub node_count: i64,
    pub completed_node_count: i64,
    pub ready_node_count: i64,
    pub active_node_count: i64,
    pub pending_node_count: i64,
    pub paused_node_count: i64,
    pub blocked_node_count: i64,
    pub usage_limited_node_count: i64,
    pub budget_limited_node_count: i64,
    pub cancelled_node_count: i64,
    pub total_tokens_used: i64,
    pub total_time_used_seconds: i64,
    pub remaining_tokens: Option<i64>,
}

impl ThreadGoalPlanSnapshot {
    pub fn usage_summary(&self) -> ThreadGoalPlanUsageSummary {
        let mut summary = self.usage_summary_without_ready_nodes();
        summary.ready_node_count = i64::try_from(self.ready_node_ids().len()).unwrap_or(i64::MAX);
        summary
    }

    pub fn ready_node_ids(&self) -> Vec<String> {
        if self.plan.status != ThreadGoalPlanStatus::Active {
            return Vec::new();
        }

        let summary = self.usage_summary_without_ready_nodes();
        if self
            .plan
            .max_tokens
            .is_some_and(|max_tokens| summary.total_tokens_used >= max_tokens)
        {
            return Vec::new();
        }

        let completed_keys: HashSet<&str> = self
            .nodes
            .iter()
            .filter(|node| node.status == ThreadGoalPlanNodeStatus::Complete)
            .map(|node| node.key.as_str())
            .collect();

        self.nodes
            .iter()
            .filter(|node| {
                node.status == ThreadGoalPlanNodeStatus::Pending
                    && node
                        .depends_on
                        .iter()
                        .all(|dependency| completed_keys.contains(dependency.as_str()))
            })
            .map(|node| node.node_id.clone())
            .collect()
    }

    fn usage_summary_without_ready_nodes(&self) -> ThreadGoalPlanUsageSummary {
        let mut summary = ThreadGoalPlanUsageSummary {
            node_count: 0,
            completed_node_count: 0,
            ready_node_count: 0,
            active_node_count: 0,
            pending_node_count: 0,
            paused_node_count: 0,
            blocked_node_count: 0,
            usage_limited_node_count: 0,
            budget_limited_node_count: 0,
            cancelled_node_count: 0,
            total_tokens_used: 0,
            total_time_used_seconds: 0,
            remaining_tokens: None,
        };

        for node in &self.nodes {
            summary.node_count += 1;
            summary.total_tokens_used += node.tokens_used.max(0);
            summary.total_time_used_seconds += node.time_used_seconds.max(0);
            match node.status {
                ThreadGoalPlanNodeStatus::Pending => summary.pending_node_count += 1,
                ThreadGoalPlanNodeStatus::Active => summary.active_node_count += 1,
                ThreadGoalPlanNodeStatus::Paused => summary.paused_node_count += 1,
                ThreadGoalPlanNodeStatus::Blocked => summary.blocked_node_count += 1,
                ThreadGoalPlanNodeStatus::UsageLimited => summary.usage_limited_node_count += 1,
                ThreadGoalPlanNodeStatus::BudgetLimited => summary.budget_limited_node_count += 1,
                ThreadGoalPlanNodeStatus::Complete => summary.completed_node_count += 1,
                ThreadGoalPlanNodeStatus::Cancelled => summary.cancelled_node_count += 1,
            }
        }

        summary.remaining_tokens = self
            .plan
            .max_tokens
            .map(|max_tokens| max_tokens.saturating_sub(summary.total_tokens_used).max(0));
        summary
    }
}

pub(crate) struct ThreadGoalRow {
    pub thread_id: String,
    pub goal_id: String,
    pub objective: String,
    pub status: String,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub(crate) struct ThreadGoalPlanRow {
    pub plan_id: String,
    pub thread_id: String,
    pub status: String,
    pub auto_execute: String,
    pub max_tokens: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub(crate) struct ThreadGoalPlanNodeRow {
    pub node_id: String,
    pub plan_id: String,
    pub thread_id: String,
    pub key: String,
    pub sequence: i64,
    pub priority: i64,
    pub objective: String,
    pub status: String,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub projected_goal_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ThreadGoalRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            goal_id: row.try_get("goal_id")?,
            objective: row.try_get("objective")?,
            status: row.try_get("status")?,
            token_budget: row.try_get("token_budget")?,
            tokens_used: row.try_get("tokens_used")?,
            time_used_seconds: row.try_get("time_used_seconds")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl ThreadGoalPlanRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            plan_id: row.try_get("plan_id")?,
            thread_id: row.try_get("thread_id")?,
            status: row.try_get("status")?,
            auto_execute: row.try_get("auto_execute")?,
            max_tokens: row.try_get("max_tokens")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl ThreadGoalPlanNodeRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            node_id: row.try_get("node_id")?,
            plan_id: row.try_get("plan_id")?,
            thread_id: row.try_get("thread_id")?,
            key: row.try_get("key")?,
            sequence: row.try_get("sequence")?,
            priority: row.try_get("priority")?,
            objective: row.try_get("objective")?,
            status: row.try_get("status")?,
            token_budget: row.try_get("token_budget")?,
            tokens_used: row.try_get("tokens_used")?,
            time_used_seconds: row.try_get("time_used_seconds")?,
            projected_goal_id: row.try_get("projected_goal_id")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<ThreadGoalRow> for ThreadGoal {
    type Error = anyhow::Error;

    fn try_from(row: ThreadGoalRow) -> Result<Self> {
        Ok(Self {
            thread_id: ThreadId::try_from(row.thread_id)?,
            goal_id: row.goal_id,
            objective: row.objective,
            status: ThreadGoalStatus::try_from(row.status.as_str())?,
            token_budget: row.token_budget,
            tokens_used: row.tokens_used,
            time_used_seconds: row.time_used_seconds,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

impl TryFrom<ThreadGoalPlanRow> for ThreadGoalPlan {
    type Error = anyhow::Error;

    fn try_from(row: ThreadGoalPlanRow) -> Result<Self> {
        Ok(Self {
            plan_id: row.plan_id,
            thread_id: ThreadId::try_from(row.thread_id)?,
            status: ThreadGoalPlanStatus::try_from(row.status.as_str())?,
            auto_execute: ThreadGoalPlanAutoExecute::try_from(row.auto_execute.as_str())?,
            max_tokens: row.max_tokens,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

impl ThreadGoalPlanNode {
    pub(crate) fn from_row_with_dependencies(
        row: ThreadGoalPlanNodeRow,
        depends_on: Vec<String>,
    ) -> Result<Self> {
        Ok(Self {
            node_id: row.node_id,
            plan_id: row.plan_id,
            thread_id: ThreadId::try_from(row.thread_id)?,
            key: row.key,
            sequence: row.sequence,
            priority: row.priority,
            objective: row.objective,
            status: ThreadGoalPlanNodeStatus::try_from(row.status.as_str())?,
            token_budget: row.token_budget,
            tokens_used: row.tokens_used,
            time_used_seconds: row.time_used_seconds,
            projected_goal_id: row.projected_goal_id,
            depends_on,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}
