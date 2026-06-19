use super::*;
use crate::model::ThreadGoalPlanNodeRow;
use crate::model::ThreadGoalPlanRow;
use codex_protocol::protocol::validate_thread_goal_objective;
use sqlx::QueryBuilder;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

pub const DEFAULT_THREAD_GOAL_PLAN_LIST_LIMIT: u32 = 20;
pub const MAX_THREAD_GOAL_PLAN_LIST_LIMIT: u32 = 50;

const MAX_GOAL_PLAN_NODES: usize = 128;
const MAX_GOAL_PLAN_NODE_KEY_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanNodeCreateParams {
    pub key: String,
    pub objective: String,
    pub priority: i64,
    pub token_budget: Option<i64>,
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanCreateParams {
    pub thread_id: ThreadId,
    pub auto_execute: crate::ThreadGoalPlanAutoExecute,
    pub max_tokens: Option<i64>,
    pub nodes: Vec<ThreadGoalPlanNodeCreateParams>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanAdvanceOutcome {
    pub snapshot: crate::ThreadGoalPlanSnapshot,
    pub activated_goal: Option<crate::ThreadGoal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanListPage {
    pub data: Vec<crate::ThreadGoalPlanSnapshot>,
    pub next_cursor: Option<String>,
}

impl GoalStore {
    pub async fn create_thread_goal_plan(
        &self,
        params: ThreadGoalPlanCreateParams,
    ) -> anyhow::Result<ThreadGoalPlanAdvanceOutcome> {
        validate_plan_create_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let outcome = insert_thread_goal_plan_in_tx(&mut tx, params, now_ms).await?;
        tx.commit().await?;
        Ok(outcome)
    }
}

impl GoalStore {
    pub async fn list_thread_goal_plans(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
        self.list_thread_goal_plans_page(
            thread_id,
            /*cursor*/ None,
            DEFAULT_THREAD_GOAL_PLAN_LIST_LIMIT,
        )
        .await
        .map(|page| page.data)
    }

    pub async fn list_thread_goal_plans_page(
        &self,
        thread_id: ThreadId,
        cursor: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<ThreadGoalPlanListPage> {
        let offset = parse_goal_plan_list_cursor(cursor)?;
        let limit = limit.clamp(1, MAX_THREAD_GOAL_PLAN_LIST_LIMIT);
        let plan_rows = sqlx::query(
            r#"
SELECT
    plan_id,
    thread_id,
    status,
    auto_execute,
    max_tokens,
    created_at_ms,
    updated_at_ms
FROM thread_goal_plans
WHERE thread_id = ?
ORDER BY created_at_ms DESC, plan_id DESC
LIMIT ?
OFFSET ?
            "#,
        )
        .bind(thread_id.to_string())
        .bind(i64::from(limit) + 1)
        .bind(i64::from(offset))
        .fetch_all(self.pool.as_ref())
        .await?;
        let has_more = plan_rows.len() > limit as usize;
        let plans = plan_rows
            .into_iter()
            .take(limit as usize)
            .map(|row| thread_goal_plan_from_row(&row))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let plan_ids = plans
            .iter()
            .map(|plan| plan.plan_id.clone())
            .collect::<Vec<_>>();
        let mut nodes_by_plan_id = self
            .list_thread_goal_plan_nodes_for_plans(thread_id, &plan_ids)
            .await?;
        let mut snapshots = Vec::with_capacity(plans.len());
        for plan in plans {
            let nodes = nodes_by_plan_id.remove(&plan.plan_id).unwrap_or_default();
            snapshots.push(crate::ThreadGoalPlanSnapshot { plan, nodes });
        }
        let next_cursor = has_more.then(|| offset.saturating_add(limit).to_string());
        Ok(ThreadGoalPlanListPage {
            data: snapshots,
            next_cursor,
        })
    }

    async fn list_thread_goal_plan_nodes_for_plans(
        &self,
        thread_id: ThreadId,
        plan_ids: &[String],
    ) -> anyhow::Result<HashMap<String, Vec<crate::ThreadGoalPlanNode>>> {
        if plan_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut node_query = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms
FROM thread_goal_plan_nodes
WHERE thread_id =
            "#,
        );
        node_query.push_bind(thread_id.to_string());
        node_query.push(" AND plan_id IN (");
        let mut separated = node_query.separated(", ");
        for plan_id in plan_ids {
            separated.push_bind(plan_id);
        }
        separated.push_unseparated(") ORDER BY plan_id, sequence, node_id");
        let rows = node_query.build().fetch_all(self.pool.as_ref()).await?;

        let node_rows = rows
            .iter()
            .map(ThreadGoalPlanNodeRow::try_from_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let node_ids = node_rows
            .iter()
            .map(|row| row.node_id.clone())
            .collect::<Vec<_>>();
        let mut dependency_keys_by_node_id =
            node_dependency_keys_for_nodes(&self.pool, &node_ids).await?;
        let mut nodes_by_plan_id: HashMap<String, Vec<crate::ThreadGoalPlanNode>> = HashMap::new();
        for row in node_rows {
            let depends_on = dependency_keys_by_node_id
                .remove(row.node_id.as_str())
                .unwrap_or_default();
            let plan_id = row.plan_id.clone();
            let node = crate::ThreadGoalPlanNode::from_row_with_dependencies(row, depends_on)?;
            nodes_by_plan_id.entry(plan_id).or_default().push(node);
        }
        Ok(nodes_by_plan_id)
    }

    pub async fn delete_thread_goal_plans_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<u64> {
        let result = sqlx::query(
            r#"
DELETE FROM thread_goal_plans
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn block_active_goal_plan_nodes_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET
    status = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND status = 'active'
RETURNING plan_id
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Blocked.as_str())
        .bind(now_ms)
        .bind(thread_id.to_string())
        .fetch_all(&mut *tx)
        .await?;

        let mut plan_ids = Vec::new();
        let mut seen_plan_ids = HashSet::new();
        for row in rows {
            let plan_id: String = row.try_get("plan_id")?;
            if seen_plan_ids.insert(plan_id.clone()) {
                plan_ids.push(plan_id);
            }
        }

        for plan_id in &plan_ids {
            sqlx::query(
                r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
                "#,
            )
            .bind(crate::ThreadGoalPlanStatus::Blocked.as_str())
            .bind(now_ms)
            .bind(plan_id)
            .execute(&mut *tx)
            .await?;
        }

        let mut snapshots = Vec::with_capacity(plan_ids.len());
        for plan_id in plan_ids {
            snapshots.push(snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?);
        }
        tx.commit().await?;
        Ok(snapshots)
    }

    pub async fn sync_goal_plan_node_for_goal(
        &self,
        thread_id: ThreadId,
        goal: &crate::ThreadGoal,
    ) -> anyhow::Result<Option<crate::ThreadGoalPlanSnapshot>> {
        let node_status = crate::ThreadGoalPlanNodeStatus::from(goal.status);
        let plan_status = plan_status_after_goal_status(goal.status);
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET
    status = ?,
    tokens_used = ?,
    time_used_seconds = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND projected_goal_id = ?
  AND status IN ('active', 'paused', 'blocked', 'usage_limited', 'budget_limited')
  AND EXISTS (
      SELECT 1
      FROM thread_goal_plans plan
      WHERE plan.plan_id = thread_goal_plan_nodes.plan_id
        AND plan.status IN ('active', 'paused', 'blocked', 'budget_limited')
  )
  AND EXISTS (
      SELECT 1
      FROM thread_goals current_goal
      WHERE current_goal.thread_id = thread_goal_plan_nodes.thread_id
        AND current_goal.goal_id = thread_goal_plan_nodes.projected_goal_id
  )
RETURNING plan_id
            "#,
        )
        .bind(node_status.as_str())
        .bind(goal.tokens_used)
        .bind(goal.time_used_seconds)
        .bind(now_ms)
        .bind(thread_id.to_string())
        .bind(&goal.goal_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let plan_id: String = row.try_get("plan_id")?;
        if let Some(plan_status) = plan_status {
            sqlx::query(
                r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
                "#,
            )
            .bind(plan_status.as_str())
            .bind(now_ms)
            .bind(&plan_id)
            .execute(&mut *tx)
            .await?;
        }
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?;
        tx.commit().await?;
        Ok(Some(snapshot))
    }

    pub async fn complete_goal_plan_node_and_maybe_advance(
        &self,
        thread_id: ThreadId,
        goal: &crate::ThreadGoal,
        effective_auto_execute: crate::ThreadGoalPlanAutoExecute,
    ) -> anyhow::Result<Option<ThreadGoalPlanAdvanceOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET
    status = ?,
    tokens_used = ?,
    time_used_seconds = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND projected_goal_id = ?
  AND status IN ('active', 'paused', 'blocked', 'usage_limited', 'budget_limited')
  AND EXISTS (
      SELECT 1
      FROM thread_goal_plans plan
      WHERE plan.plan_id = thread_goal_plan_nodes.plan_id
        AND plan.status IN ('active', 'paused', 'blocked', 'budget_limited')
  )
  AND EXISTS (
      SELECT 1
      FROM thread_goals current_goal
      WHERE current_goal.thread_id = thread_goal_plan_nodes.thread_id
        AND current_goal.goal_id = thread_goal_plan_nodes.projected_goal_id
        AND current_goal.status = 'complete'
  )
RETURNING plan_id
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Complete.as_str())
        .bind(goal.tokens_used)
        .bind(goal.time_used_seconds)
        .bind(now_ms)
        .bind(thread_id.to_string())
        .bind(&goal.goal_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let plan_id: String = row.try_get("plan_id")?;
        let plan = get_thread_goal_plan_in_tx(&mut tx, &plan_id).await?;
        if plan.auto_execute != effective_auto_execute {
            sqlx::query(
                r#"
UPDATE thread_goal_plans
SET auto_execute = ?, updated_at_ms = ?
WHERE plan_id = ?
                "#,
            )
            .bind(effective_auto_execute.as_str())
            .bind(now_ms)
            .bind(&plan_id)
            .execute(&mut *tx)
            .await?;
        }
        let total_tokens = total_plan_tokens_in_tx(&mut tx, &plan_id).await?;
        let hit_plan_budget = plan
            .max_tokens
            .is_some_and(|max_tokens| total_tokens >= max_tokens);
        if hit_plan_budget {
            sqlx::query(
                r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status = 'pending'
                "#,
            )
            .bind(crate::ThreadGoalPlanNodeStatus::BudgetLimited.as_str())
            .bind(now_ms)
            .bind(&plan_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
                "#,
            )
            .bind(crate::ThreadGoalPlanStatus::BudgetLimited.as_str())
            .bind(now_ms)
            .bind(&plan_id)
            .execute(&mut *tx)
            .await?;
            let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?;
            tx.commit().await?;
            return Ok(Some(ThreadGoalPlanAdvanceOutcome {
                snapshot,
                activated_goal: None,
            }));
        }

        let incomplete_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM thread_goal_plan_nodes
WHERE plan_id = ?
  AND status != 'complete'
            "#,
        )
        .bind(&plan_id)
        .fetch_one(&mut *tx)
        .await?;
        let activated_goal = if incomplete_count == 0 {
            sqlx::query(
                r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
                "#,
            )
            .bind(crate::ThreadGoalPlanStatus::Complete.as_str())
            .bind(now_ms)
            .bind(&plan_id)
            .execute(&mut *tx)
            .await?;
            None
        } else {
            activate_next_ready_node_in_tx(
                &mut tx,
                thread_id,
                &plan_id,
                effective_auto_execute,
                now_ms,
            )
            .await?
        };
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?;
        tx.commit().await?;
        Ok(Some(ThreadGoalPlanAdvanceOutcome {
            snapshot,
            activated_goal,
        }))
    }

    pub async fn activate_thread_goal_plan_node(
        &self,
        thread_id: ThreadId,
        node_id: &str,
    ) -> anyhow::Result<Option<ThreadGoalPlanAdvanceOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some(plan_id) = ready_node_plan_id_in_tx(&mut tx, thread_id, node_id).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let activated_goal = activate_node_in_tx(&mut tx, thread_id, node_id, now_ms).await?;
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?;
        tx.commit().await?;
        Ok(Some(ThreadGoalPlanAdvanceOutcome {
            snapshot,
            activated_goal: Some(activated_goal),
        }))
    }
}

async fn activate_next_ready_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    plan_id: &str,
    auto_execute: crate::ThreadGoalPlanAutoExecute,
    now_ms: i64,
) -> anyhow::Result<Option<crate::ThreadGoal>> {
    if auto_execute == crate::ThreadGoalPlanAutoExecute::Off {
        return Ok(None);
    }
    let ready_nodes = ready_node_ids_in_tx(tx, thread_id, plan_id).await?;
    let node_id = match auto_execute {
        crate::ThreadGoalPlanAutoExecute::Off => return Ok(None),
        crate::ThreadGoalPlanAutoExecute::ReadyOnly if ready_nodes.len() != 1 => return Ok(None),
        crate::ThreadGoalPlanAutoExecute::ReadyOnly
        | crate::ThreadGoalPlanAutoExecute::AiDirected => ready_nodes.first(),
    };
    let Some(node_id) = node_id else {
        return Ok(None);
    };
    activate_node_in_tx(tx, thread_id, node_id, now_ms)
        .await
        .map(Some)
}

pub(super) async fn insert_thread_goal_plan_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: ThreadGoalPlanCreateParams,
    now_ms: i64,
) -> anyhow::Result<ThreadGoalPlanAdvanceOutcome> {
    validate_plan_create_params(&params)?;
    let plan_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
INSERT INTO thread_goal_plans (
    plan_id,
    thread_id,
    status,
    auto_execute,
    max_tokens,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&plan_id)
    .bind(params.thread_id.to_string())
    .bind(crate::ThreadGoalPlanStatus::Active.as_str())
    .bind(params.auto_execute.as_str())
    .bind(params.max_tokens)
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;

    let mut node_ids_by_key = HashMap::new();
    for (sequence, node) in params.nodes.iter().enumerate() {
        let node_id = Uuid::new_v4().to_string();
        node_ids_by_key.insert(node.key.clone(), node_id.clone());
        sqlx::query(
            r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    token_budget,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&node_id)
        .bind(&plan_id)
        .bind(params.thread_id.to_string())
        .bind(&node.key)
        .bind(i64::try_from(sequence)?)
        .bind(node.priority)
        .bind(&node.objective)
        .bind(crate::ThreadGoalPlanNodeStatus::Pending.as_str())
        .bind(node.token_budget)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut **tx)
        .await?;
    }

    for node in &params.nodes {
        let node_id = node_ids_by_key
            .get(&node.key)
            .ok_or_else(|| anyhow::anyhow!("missing inserted goal node key {}", node.key))?;
        for dependency_key in &node.depends_on {
            let dependency_id = node_ids_by_key.get(dependency_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "goal node {} depends on unknown goal node {dependency_key}",
                    node.key
                )
            })?;
            sqlx::query(
                r#"
INSERT INTO thread_goal_plan_dependencies (node_id, depends_on_node_id)
VALUES (?, ?)
                "#,
            )
            .bind(node_id)
            .bind(dependency_id)
            .execute(&mut **tx)
            .await?;
        }
    }

    let activated_goal =
        activate_next_ready_node_in_tx(tx, params.thread_id, &plan_id, params.auto_execute, now_ms)
            .await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, &plan_id).await?;
    Ok(ThreadGoalPlanAdvanceOutcome {
        snapshot,
        activated_goal,
    })
}

async fn activate_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    node_id: &str,
    now_ms: i64,
) -> anyhow::Result<crate::ThreadGoal> {
    let active_count: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM thread_goal_plan_nodes
WHERE thread_id = ?
  AND status = 'active'
        "#,
    )
    .bind(thread_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if active_count > 0 {
        anyhow::bail!("cannot activate goal plan node while another plan node is active");
    }
    let current_goal_status: Option<String> = sqlx::query_scalar(
        r#"
SELECT status
FROM thread_goals
WHERE thread_id = ?
        "#,
    )
    .bind(thread_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(status) = current_goal_status.as_deref() {
        let status = crate::ThreadGoalStatus::try_from(status)?;
        if !status.is_terminal() {
            anyhow::bail!("cannot activate goal plan node while thread has a non-terminal goal");
        }
    }
    let row = sqlx::query(
        r#"
SELECT
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms
FROM thread_goal_plan_nodes
WHERE node_id = ?
  AND thread_id = ?
  AND status = 'pending'
        "#,
    )
    .bind(node_id)
    .bind(thread_id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("goal plan node is not ready to activate"))?;
    let node = ThreadGoalPlanNodeRow::try_from_row(&row)?;
    let projected_goal_id = Uuid::new_v4().to_string();
    let plan = get_thread_goal_plan_in_tx(tx, &node.plan_id).await?;
    let remaining_plan_tokens = if let Some(max_tokens) = plan.max_tokens {
        let total_tokens = total_plan_tokens_in_tx(tx, &node.plan_id).await?;
        Some(max_tokens.saturating_sub(total_tokens).max(0))
    } else {
        None
    };
    let effective_token_budget = match (node.token_budget, remaining_plan_tokens) {
        (Some(node_budget), Some(remaining_plan_tokens)) => {
            Some(node_budget.min(remaining_plan_tokens))
        }
        (Some(node_budget), None) => Some(node_budget),
        (None, Some(remaining_plan_tokens)) => Some(remaining_plan_tokens),
        (None, None) => None,
    };
    let status = status_after_budget_limit(
        crate::ThreadGoalStatus::Active,
        /*tokens_used*/ 0,
        effective_token_budget,
    );
    let goal_row = sqlx::query(
        r#"
INSERT INTO thread_goals (
    thread_id,
    goal_id,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?)
ON CONFLICT(thread_id) DO UPDATE SET
    goal_id = excluded.goal_id,
    objective = excluded.objective,
    status = excluded.status,
    token_budget = excluded.token_budget,
    tokens_used = 0,
    time_used_seconds = 0,
    created_at_ms = excluded.created_at_ms,
    updated_at_ms = excluded.updated_at_ms
RETURNING
    thread_id,
    goal_id,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms
        "#,
    )
    .bind(thread_id.to_string())
    .bind(&projected_goal_id)
    .bind(&node.objective)
    .bind(status.as_str())
    .bind(effective_token_budget)
    .bind(now_ms)
    .bind(now_ms)
    .fetch_one(&mut **tx)
    .await?;
    let updated_node_count = sqlx::query(
        r#"
UPDATE thread_goal_plan_nodes
SET
    status = ?,
    token_budget = ?,
    projected_goal_id = ?,
    tokens_used = 0,
    time_used_seconds = 0,
    updated_at_ms = ?
WHERE node_id = ?
  AND status = 'pending'
        "#,
    )
    .bind(crate::ThreadGoalPlanNodeStatus::from(status).as_str())
    .bind(effective_token_budget)
    .bind(&projected_goal_id)
    .bind(now_ms)
    .bind(node_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated_node_count != 1 {
        anyhow::bail!("goal plan node changed before activation could be recorded");
    }
    thread_goal_from_row(&goal_row)
}

async fn ready_node_ids_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    plan_id: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
SELECT candidate.node_id
FROM thread_goal_plan_nodes candidate
JOIN thread_goal_plans plan
  ON plan.plan_id = candidate.plan_id
WHERE candidate.thread_id = ?
  AND candidate.plan_id = ?
  AND candidate.status = 'pending'
  AND plan.status = 'active'
  AND (
      plan.max_tokens IS NULL
      OR (
          SELECT COALESCE(SUM(plan_node.tokens_used), 0)
          FROM thread_goal_plan_nodes plan_node
          WHERE plan_node.plan_id = plan.plan_id
      ) < plan.max_tokens
  )
  AND NOT EXISTS (
      SELECT 1
      FROM thread_goal_plan_dependencies dependency
      JOIN thread_goal_plan_nodes dependency_node
        ON dependency_node.node_id = dependency.depends_on_node_id
      WHERE dependency.node_id = candidate.node_id
        AND dependency_node.status != 'complete'
  )
ORDER BY candidate.priority DESC, candidate.sequence ASC, candidate.node_id ASC
        "#,
    )
    .bind(thread_id.to_string())
    .bind(plan_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| row.try_get("node_id").map_err(anyhow::Error::from))
        .collect()
}

async fn ready_node_plan_id_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    node_id: &str,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        r#"
SELECT candidate.plan_id
FROM thread_goal_plan_nodes candidate
JOIN thread_goal_plans plan
  ON plan.plan_id = candidate.plan_id
WHERE candidate.thread_id = ?
  AND candidate.node_id = ?
  AND candidate.status = 'pending'
  AND plan.status = 'active'
  AND (
      plan.max_tokens IS NULL
      OR (
          SELECT COALESCE(SUM(plan_node.tokens_used), 0)
          FROM thread_goal_plan_nodes plan_node
          WHERE plan_node.plan_id = plan.plan_id
      ) < plan.max_tokens
  )
  AND NOT EXISTS (
      SELECT 1
      FROM thread_goal_plan_dependencies dependency
      JOIN thread_goal_plan_nodes dependency_node
        ON dependency_node.node_id = dependency.depends_on_node_id
      WHERE dependency.node_id = candidate.node_id
        AND dependency_node.status != 'complete'
  )
        "#,
    )
    .bind(thread_id.to_string())
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| row.try_get("plan_id").map_err(anyhow::Error::from))
        .transpose()
}

async fn total_plan_tokens_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
) -> anyhow::Result<i64> {
    let total = sqlx::query_scalar(
        r#"
SELECT COALESCE(SUM(tokens_used), 0)
FROM thread_goal_plan_nodes
WHERE plan_id = ?
        "#,
    )
    .bind(plan_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(total)
}

async fn get_thread_goal_plan_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
) -> anyhow::Result<crate::ThreadGoalPlan> {
    let row = sqlx::query(
        r#"
SELECT
    plan_id,
    thread_id,
    status,
    auto_execute,
    max_tokens,
    created_at_ms,
    updated_at_ms
FROM thread_goal_plans
WHERE plan_id = ?
        "#,
    )
    .bind(plan_id)
    .fetch_one(&mut **tx)
    .await?;
    thread_goal_plan_from_row(&row)
}

pub(super) async fn snapshot_thread_goal_plan_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
) -> anyhow::Result<crate::ThreadGoalPlanSnapshot> {
    let plan = get_thread_goal_plan_in_tx(tx, plan_id).await?;
    let rows = sqlx::query(
        r#"
SELECT
    node_id,
    plan_id,
    thread_id,
    key,
    sequence,
    priority,
    objective,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms
FROM thread_goal_plan_nodes
WHERE plan_id = ?
ORDER BY sequence, node_id
        "#,
    )
    .bind(plan_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut nodes = Vec::with_capacity(rows.len());
    for row in rows {
        let row = ThreadGoalPlanNodeRow::try_from_row(&row)?;
        let depends_on = node_dependency_keys_in_tx(tx, row.node_id.as_str()).await?;
        nodes.push(crate::ThreadGoalPlanNode::from_row_with_dependencies(
            row, depends_on,
        )?);
    }
    Ok(crate::ThreadGoalPlanSnapshot { plan, nodes })
}

async fn node_dependency_keys_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    node_id: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
SELECT dependency_node.key
FROM thread_goal_plan_dependencies dependency
JOIN thread_goal_plan_nodes dependency_node
  ON dependency_node.node_id = dependency.depends_on_node_id
WHERE dependency.node_id = ?
ORDER BY dependency_node.sequence, dependency_node.key
        "#,
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| row.try_get("key").map_err(anyhow::Error::from))
        .collect()
}

async fn node_dependency_keys_for_nodes(
    executor: &SqlitePool,
    node_ids: &[String],
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
SELECT
    dependency.node_id,
    dependency_node.key
FROM thread_goal_plan_dependencies dependency
JOIN thread_goal_plan_nodes dependency_node
  ON dependency_node.node_id = dependency.depends_on_node_id
WHERE dependency.node_id IN (
        "#,
    );
    let mut separated = query.separated(", ");
    for node_id in node_ids {
        separated.push_bind(node_id);
    }
    separated.push_unseparated(
        ") ORDER BY dependency.node_id, dependency_node.sequence, dependency_node.key",
    );

    let rows = query.build().fetch_all(executor).await?;
    let mut dependencies_by_node_id: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let node_id: String = row.try_get("node_id")?;
        let key: String = row.try_get("key")?;
        dependencies_by_node_id
            .entry(node_id)
            .or_default()
            .push(key);
    }
    Ok(dependencies_by_node_id)
}

fn thread_goal_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<crate::ThreadGoal> {
    crate::model::ThreadGoalRow::try_from_row(row).and_then(crate::ThreadGoal::try_from)
}

fn thread_goal_plan_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ThreadGoalPlan> {
    ThreadGoalPlanRow::try_from_row(row).and_then(crate::ThreadGoalPlan::try_from)
}

fn status_after_budget_limit(
    status: crate::ThreadGoalStatus,
    tokens_used: i64,
    token_budget: Option<i64>,
) -> crate::ThreadGoalStatus {
    if status == crate::ThreadGoalStatus::Active
        && token_budget.is_some_and(|budget| tokens_used >= budget)
    {
        crate::ThreadGoalStatus::BudgetLimited
    } else {
        status
    }
}

fn plan_status_after_goal_status(
    status: crate::ThreadGoalStatus,
) -> Option<crate::ThreadGoalPlanStatus> {
    match status {
        crate::ThreadGoalStatus::Active => Some(crate::ThreadGoalPlanStatus::Active),
        crate::ThreadGoalStatus::Complete => None,
        crate::ThreadGoalStatus::Paused => Some(crate::ThreadGoalPlanStatus::Paused),
        crate::ThreadGoalStatus::Blocked | crate::ThreadGoalStatus::UsageLimited => {
            Some(crate::ThreadGoalPlanStatus::Blocked)
        }
        crate::ThreadGoalStatus::BudgetLimited => Some(crate::ThreadGoalPlanStatus::BudgetLimited),
        crate::ThreadGoalStatus::Cancelled => Some(crate::ThreadGoalPlanStatus::Cancelled),
    }
}

fn validate_plan_create_params(params: &ThreadGoalPlanCreateParams) -> anyhow::Result<()> {
    if params.nodes.is_empty() {
        anyhow::bail!("goal plan must contain at least one goal");
    }
    if params.nodes.len() > MAX_GOAL_PLAN_NODES {
        anyhow::bail!(
            "goal plan contains {} goals but the maximum is {MAX_GOAL_PLAN_NODES}",
            params.nodes.len()
        );
    }
    if params.max_tokens.is_some_and(|max_tokens| max_tokens <= 0) {
        anyhow::bail!("goal plan max_tokens must be positive when set");
    }
    let mut keys = HashSet::new();
    for node in &params.nodes {
        if node.key.trim().is_empty() {
            anyhow::bail!("goal plan node key must not be empty");
        }
        if node.key.len() > MAX_GOAL_PLAN_NODE_KEY_LEN {
            anyhow::bail!(
                "goal plan node key `{}` is too long; maximum is {MAX_GOAL_PLAN_NODE_KEY_LEN} bytes",
                node.key
            );
        }
        if !is_valid_goal_plan_node_key(&node.key) {
            anyhow::bail!(
                "goal plan node key `{}` must contain only ASCII letters, numbers, underscores, or hyphens",
                node.key
            );
        }
        if !keys.insert(node.key.clone()) {
            anyhow::bail!("goal plan node key `{}` is duplicated", node.key);
        }
        validate_thread_goal_objective(node.objective.trim()).map_err(anyhow::Error::msg)?;
        if node.token_budget.is_some_and(|budget| budget <= 0) {
            anyhow::bail!("goal plan node token_budget must be positive when set");
        }
    }
    for node in &params.nodes {
        for dependency in &node.depends_on {
            if dependency == &node.key {
                anyhow::bail!("goal plan node `{}` cannot depend on itself", node.key);
            }
            if !keys.contains(dependency) {
                anyhow::bail!(
                    "goal plan node `{}` depends on unknown node `{dependency}`",
                    node.key
                );
            }
        }
    }
    validate_acyclic_dependencies(&params.nodes)
}

fn is_valid_goal_plan_node_key(key: &str) -> bool {
    key.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn parse_goal_plan_list_cursor(cursor: Option<&str>) -> anyhow::Result<u32> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let cursor = cursor.trim();
    if cursor.is_empty() {
        return Ok(0);
    }
    cursor
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("invalid goal plan list cursor `{cursor}`"))
}

fn validate_acyclic_dependencies(nodes: &[ThreadGoalPlanNodeCreateParams]) -> anyhow::Result<()> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum VisitState {
        Visiting,
        Visited,
    }

    fn visit(
        key: &str,
        graph: &HashMap<&str, Vec<&str>>,
        states: &mut HashMap<String, VisitState>,
    ) -> anyhow::Result<()> {
        match states.get(key) {
            Some(VisitState::Visited) => return Ok(()),
            Some(VisitState::Visiting) => {
                anyhow::bail!("goal plan dependencies contain a cycle involving `{key}`")
            }
            None => {}
        }
        states.insert(key.to_string(), VisitState::Visiting);
        if let Some(dependencies) = graph.get(key) {
            for dependency in dependencies {
                visit(dependency, graph, states)?;
            }
        }
        states.insert(key.to_string(), VisitState::Visited);
        Ok(())
    }

    let graph: HashMap<&str, Vec<&str>> = nodes
        .iter()
        .map(|node| {
            (
                node.key.as_str(),
                node.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut states = HashMap::new();
    for node in nodes {
        visit(node.key.as_str(), &graph, &mut states)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000456").expect("valid thread id")
    }

    async fn upsert_test_thread(runtime: &StateRuntime, thread_id: ThreadId) {
        let metadata = test_thread_metadata(
            runtime.codex_home(),
            thread_id,
            runtime.codex_home().join("workspace"),
        );
        runtime
            .upsert_thread(&metadata)
            .await
            .expect("test thread should be upserted");
    }

    #[tokio::test]
    async fn ready_only_goal_plan_advances_through_dependencies() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "investigate".to_string(),
                        objective: "Investigate goal plans.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "implement".to_string(),
                        objective: "Implement goal plans.".to_string(),
                        priority: 0,
                        token_budget: Some(10_000),
                        depends_on: vec!["investigate".to_string()],
                    },
                ],
            })
            .await
            .expect("goal plan should be created");

        let first_goal = created
            .activated_goal
            .expect("first ready goal should activate");
        assert_eq!("Investigate goal plans.", first_goal.objective);
        assert_eq!(None, first_goal.token_budget);
        assert_eq!(
            crate::ThreadGoalPlanNodeStatus::Active,
            created.snapshot.nodes[0].status
        );
        assert_eq!(
            crate::ThreadGoalPlanNodeStatus::Pending,
            created.snapshot.nodes[1].status
        );

        let completed = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(first_goal.goal_id),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        let advanced = runtime
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(
                thread_id,
                &completed,
                crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            )
            .await
            .expect("goal plan should advance")
            .expect("goal plan outcome should exist");
        let second_goal = advanced
            .activated_goal
            .expect("dependent goal should activate");
        assert_eq!("Implement goal plans.", second_goal.objective);
        assert_eq!(Some(10_000), second_goal.token_budget);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            advanced
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn ready_only_goal_plan_waits_when_multiple_goals_are_ready() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "one".to_string(),
                        objective: "Do one independent goal.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "two".to_string(),
                        objective: "Do another independent goal.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                ],
            })
            .await
            .expect("goal plan should be created");

        assert_eq!(None, created.activated_goal);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            created
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn list_thread_goal_plans_paginates() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        for idx in 0..3 {
            runtime
                .thread_goals()
                .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                    thread_id,
                    auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                    max_tokens: None,
                    nodes: vec![ThreadGoalPlanNodeCreateParams {
                        key: format!("goal_{idx}"),
                        objective: format!("Do paged goal {idx}."),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    }],
                })
                .await
                .expect("goal plan should be created");
        }

        let first_page = runtime
            .thread_goals()
            .list_thread_goal_plans_page(thread_id, /*cursor*/ None, /*limit*/ 2)
            .await
            .expect("first page should list");
        assert_eq!(2, first_page.data.len());
        assert_eq!(Some("2".to_string()), first_page.next_cursor);

        let second_page = runtime
            .thread_goals()
            .list_thread_goal_plans_page(
                thread_id,
                first_page.next_cursor.as_deref(),
                /*limit*/ 2,
            )
            .await
            .expect("second page should list");
        assert_eq!(1, second_page.data.len());
        assert_eq!(None, second_page.next_cursor);
    }

    #[tokio::test]
    async fn blocking_active_plan_node_marks_plan_blocked() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "active".to_string(),
                    objective: "Run the active projected goal.".to_string(),
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("goal plan should be created");
        assert_eq!(
            Some(crate::ThreadGoalStatus::Active),
            created.activated_goal.map(|goal| goal.status)
        );

        let snapshots = runtime
            .thread_goals()
            .block_active_goal_plan_nodes_for_thread(thread_id)
            .await
            .expect("active plan nodes should be blocked");

        assert_eq!(1, snapshots.len());
        assert_eq!(
            crate::ThreadGoalPlanStatus::Blocked,
            snapshots[0].plan.status
        );
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Blocked],
            snapshots[0]
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn plan_budget_limit_stops_pending_nodes() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: Some(1),
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "first".to_string(),
                        objective: "Spend the whole plan budget.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Should not run after the plan budget is exhausted.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["first".to_string()],
                    },
                ],
            })
            .await
            .expect("goal plan should be created");
        let first_goal = created.activated_goal.expect("first goal should activate");

        runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 0,
                /*token_delta*/ 1,
                GoalAccountingMode::ActiveOnly,
                Some(first_goal.goal_id.as_str()),
            )
            .await
            .expect("usage should be accounted");
        let completed = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(first_goal.goal_id),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        let outcome = runtime
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(
                thread_id,
                &completed,
                crate::ThreadGoalPlanAutoExecute::AiDirected,
            )
            .await
            .expect("goal plan should update")
            .expect("goal plan outcome should exist");

        assert_eq!(
            crate::ThreadGoalPlanStatus::BudgetLimited,
            outcome.snapshot.plan.status
        );
        let summary = outcome.snapshot.usage_summary();
        assert_eq!(1, summary.total_tokens_used);
        assert_eq!(Some(0), summary.remaining_tokens);
        assert_eq!(2, summary.node_count);
        assert_eq!(1, summary.completed_node_count);
        assert_eq!(1, summary.budget_limited_node_count);
        assert_eq!(None, outcome.activated_goal);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::BudgetLimited,
            ],
            outcome
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn plan_budget_limit_applies_to_active_projected_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: Some(10),
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "active".to_string(),
                    objective: "Spend more than the remaining plan budget.".to_string(),
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("goal plan should be created");
        let active_goal = created.activated_goal.expect("goal should activate");
        assert_eq!(Some(10), active_goal.token_budget);

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 7,
                /*token_delta*/ 12,
                GoalAccountingMode::ActiveOnly,
                Some(active_goal.goal_id.as_str()),
            )
            .await
            .expect("usage should be accounted");
        let GoalAccountingOutcome::Updated(goal) = outcome else {
            panic!("goal usage should update");
        };
        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, goal.status);

        let snapshot = runtime
            .thread_goals()
            .sync_goal_plan_node_for_goal(thread_id, &goal)
            .await
            .expect("goal plan should sync")
            .expect("goal plan snapshot should exist");
        assert_eq!(
            crate::ThreadGoalPlanStatus::BudgetLimited,
            snapshot.plan.status
        );
        assert_eq!(
            crate::ThreadGoalPlanNodeStatus::BudgetLimited,
            snapshot.nodes[0].status
        );
        let summary = snapshot.usage_summary();
        assert_eq!(12, summary.total_tokens_used);
        assert_eq!(7, summary.total_time_used_seconds);
        assert_eq!(Some(0), summary.remaining_tokens);
    }

    #[tokio::test]
    async fn cancelled_active_projected_goal_marks_plan_cancelled() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "active".to_string(),
                    objective: "Cancel the active projected goal.".to_string(),
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("goal plan should be created");
        let active_goal = created.activated_goal.expect("goal should activate");
        let active_goal_id = active_goal.goal_id.clone();

        let cancelled_goal = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(crate::ThreadGoalStatus::Cancelled),
                    token_budget: None,
                    expected_goal_id: Some(active_goal_id.clone()),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        let snapshot = runtime
            .thread_goals()
            .sync_goal_plan_node_for_goal(thread_id, &cancelled_goal)
            .await
            .expect("goal plan should sync")
            .expect("goal plan snapshot should exist");

        assert_eq!(crate::ThreadGoalPlanStatus::Cancelled, snapshot.plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Cancelled],
            snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
        assert_eq!(1, snapshot.usage_summary().cancelled_node_count);

        let still_cancelled_goal = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(crate::ThreadGoalStatus::Active),
                    token_budget: None,
                    expected_goal_id: Some(active_goal_id),
                },
            )
            .await
            .expect("goal update should not fail")
            .expect("goal should exist");
        assert_eq!(
            crate::ThreadGoalStatus::Cancelled,
            still_cancelled_goal.status
        );
        assert!(
            runtime
                .thread_goals()
                .sync_goal_plan_node_for_goal(thread_id, &still_cancelled_goal)
                .await
                .expect("sync should not fail")
                .is_none()
        );
        let snapshots = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("plan should list");
        assert_eq!(
            crate::ThreadGoalPlanStatus::Cancelled,
            snapshots[0].plan.status
        );
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Cancelled],
            snapshots[0]
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn goal_plan_rejects_cycles() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let err = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "a".to_string(),
                        objective: "Do goal A.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["b".to_string()],
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "b".to_string(),
                        objective: "Do goal B.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["a".to_string()],
                    },
                ],
            })
            .await
            .expect_err("cyclic plan should be rejected");

        assert!(
            err.to_string()
                .contains("goal plan dependencies contain a cycle")
        );
    }

    #[tokio::test]
    async fn goal_plan_rejects_invalid_node_keys() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let err = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "bad key".to_string(),
                    objective: "Do a goal with a bad key.".to_string(),
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect_err("invalid key should be rejected");

        assert!(
            err.to_string()
                .contains("must contain only ASCII letters, numbers, underscores, or hyphens")
        );
    }

    #[tokio::test]
    async fn stale_projected_completion_does_not_advance_blocked_plan() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "first".to_string(),
                        objective: "Run the first projected goal.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Run the dependent projected goal.".to_string(),
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["first".to_string()],
                    },
                ],
            })
            .await
            .expect("goal plan should be created");
        let first_goal = created.activated_goal.expect("first goal should activate");

        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Manual replacement goal.",
                crate::ThreadGoalStatus::Active,
                None,
            )
            .await
            .expect("manual replacement should block projected node");
        let stale_completed = crate::ThreadGoal {
            status: crate::ThreadGoalStatus::Complete,
            ..first_goal
        };
        let outcome = runtime
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(
                thread_id,
                &stale_completed,
                crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            )
            .await
            .expect("stale completion should be a no-op");

        assert_eq!(None, outcome);
        let plans = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("goal plans should list");
        assert_eq!(1, plans.len());
        assert_eq!(crate::ThreadGoalPlanStatus::Blocked, plans[0].plan.status);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Blocked,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            plans[0]
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn activation_rolls_back_when_current_goal_is_non_terminal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Manual active goal.",
                crate::ThreadGoalStatus::Active,
                None,
            )
            .await
            .expect("manual goal should create");

        let err = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "ready".to_string(),
                    objective: "This goal must not overwrite the manual goal.".to_string(),
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect_err("activation should refuse to overwrite manual goal");

        assert!(
            err.to_string()
                .contains("cannot activate goal plan node while thread has a non-terminal goal")
        );
        assert_eq!(
            Vec::<crate::ThreadGoalPlanSnapshot>::new(),
            runtime
                .thread_goals()
                .list_thread_goal_plans(thread_id)
                .await
                .expect("rolled back plan should not list")
        );
    }

    #[tokio::test]
    async fn goal_plan_rejects_too_many_nodes() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let nodes = (0..=MAX_GOAL_PLAN_NODES)
            .map(|idx| ThreadGoalPlanNodeCreateParams {
                key: format!("goal_{idx}"),
                objective: format!("Do goal {idx}."),
                priority: 0,
                token_budget: None,
                depends_on: Vec::new(),
            })
            .collect();

        let err = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes,
            })
            .await
            .expect_err("oversized goal plan should be rejected");

        assert!(err.to_string().contains(&format!(
            "goal plan contains {} goals but the maximum is {MAX_GOAL_PLAN_NODES}",
            MAX_GOAL_PLAN_NODES + 1
        )));
    }

    #[tokio::test]
    async fn deleting_thread_deletes_goal_plans() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "cleanup".to_string(),
                    objective: "Clean up the plan with the thread.".to_string(),
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("goal plan should be created");

        runtime
            .delete_thread(thread_id)
            .await
            .expect("thread deletion should succeed");

        assert_eq!(
            Vec::<crate::ThreadGoalPlanSnapshot>::new(),
            runtime
                .thread_goals()
                .list_thread_goal_plans(thread_id)
                .await
                .expect("goal plans should list")
        );
    }
}
