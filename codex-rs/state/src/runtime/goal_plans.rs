use super::*;
use crate::model::ThreadGoalPlanNodeRow;
use crate::model::ThreadGoalPlanRow;
use codex_protocol::protocol::normalize_thread_goal_title;
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
    pub assigned_thread_id: Option<ThreadId>,
    pub title: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanAppendParams {
    pub thread_id: ThreadId,
    pub plan_id: String,
    pub max_total_nodes: Option<usize>,
    pub nodes: Vec<ThreadGoalPlanNodeCreateParams>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanAddParams {
    pub thread_id: ThreadId,
    pub objective: String,
    pub title: Option<String>,
    pub token_budget: Option<i64>,
    pub auto_execute: crate::ThreadGoalPlanAutoExecute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanAdvanceOutcome {
    pub snapshot: crate::ThreadGoalPlanSnapshot,
    pub activated_goal: Option<crate::ThreadGoal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanAddOutcome {
    pub snapshot: crate::ThreadGoalPlanSnapshot,
    pub added_node: crate::ThreadGoalPlanNode,
    pub goal: Option<crate::ThreadGoal>,
    pub activated_goal: Option<crate::ThreadGoal>,
    pub created_plan: bool,
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

    pub async fn append_thread_goal_plan_nodes(
        &self,
        params: ThreadGoalPlanAppendParams,
    ) -> anyhow::Result<crate::ThreadGoalPlanSnapshot> {
        validate_plan_append_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let snapshot = append_thread_goal_plan_nodes_in_tx(&mut tx, params, now_ms).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    pub async fn add_thread_goal_to_plan(
        &self,
        params: ThreadGoalPlanAddParams,
    ) -> anyhow::Result<ThreadGoalPlanAddOutcome> {
        validate_plan_add_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let goal = get_thread_goal_in_tx(&mut tx, params.thread_id).await?;

        if let Some(goal) = goal.as_ref()
            && let Some(plan_id) =
                appendable_goal_plan_id_for_goal_in_tx(&mut tx, params.thread_id, goal).await?
        {
            let (snapshot, added_node) =
                append_goal_plan_node_in_tx(&mut tx, &plan_id, &params, now_ms).await?;
            tx.commit().await?;
            return Ok(ThreadGoalPlanAddOutcome {
                snapshot,
                added_node,
                goal: Some(goal.clone()),
                activated_goal: None,
                created_plan: false,
            });
        }

        if let Some(goal) = goal.as_ref()
            && !matches!(
                goal.status,
                crate::ThreadGoalStatus::Complete | crate::ThreadGoalStatus::Cancelled
            )
        {
            let (snapshot, added_node) =
                create_goal_plan_from_goal_in_tx(&mut tx, &params, goal, now_ms).await?;
            tx.commit().await?;
            return Ok(ThreadGoalPlanAddOutcome {
                snapshot,
                added_node,
                goal: Some(goal.clone()),
                activated_goal: None,
                created_plan: true,
            });
        }

        if let Some(plan_id) =
            newest_appendable_goal_plan_id_for_thread_in_tx(&mut tx, params.thread_id).await?
        {
            let (snapshot, added_node) =
                append_goal_plan_node_in_tx(&mut tx, &plan_id, &params, now_ms).await?;
            tx.commit().await?;
            return Ok(ThreadGoalPlanAddOutcome {
                snapshot,
                added_node,
                goal,
                activated_goal: None,
                created_plan: false,
            });
        }

        let (snapshot, added_node, activated_goal) =
            create_goal_plan_from_added_goal_in_tx(&mut tx, &params, now_ms).await?;
        tx.commit().await?;
        Ok(ThreadGoalPlanAddOutcome {
            snapshot,
            added_node,
            goal: Some(activated_goal.clone()),
            activated_goal: Some(activated_goal),
            created_plan: true,
        })
    }
}

impl GoalStore {
    pub async fn get_thread_goal_plan_for_thread(
        &self,
        thread_id: ThreadId,
        plan_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadGoalPlanSnapshot>> {
        let mut tx = self.pool.begin().await?;
        let visible = sqlx::query_scalar::<_, i64>(
            r#"
SELECT 1
FROM thread_goal_plans plan
WHERE plan.plan_id = ?
  AND (
      plan.thread_id = ?
      OR EXISTS (
          SELECT 1
          FROM thread_goal_plan_nodes node
          WHERE node.plan_id = plan.plan_id
            AND node.assigned_thread_id = ?
      )
  )
LIMIT 1
            "#,
        )
        .bind(plan_id)
        .bind(thread_id.to_string())
        .bind(thread_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        if !visible {
            tx.commit().await?;
            return Ok(None);
        }

        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, plan_id).await?;
        tx.commit().await?;
        Ok(Some(snapshot))
    }

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
	FROM thread_goal_plans plan
WHERE plan.thread_id = ?
   OR EXISTS (
       SELECT 1
       FROM thread_goal_plan_nodes node
       WHERE node.plan_id = plan.plan_id
         AND node.assigned_thread_id = ?
   )
ORDER BY created_at_ms DESC, plan_id DESC
LIMIT ?
OFFSET ?
            "#,
        )
        .bind(thread_id.to_string())
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
            .list_thread_goal_plan_nodes_for_plans(&plan_ids)
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
	    assigned_thread_id,
	    key,
	    sequence,
	    priority,
    objective,
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
	    updated_at_ms
	FROM thread_goal_plan_nodes
WHERE plan_id IN (
            "#,
        );
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
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
DELETE FROM thread_goals
WHERE EXISTS (
    SELECT 1
    FROM thread_goal_plan_nodes node
    JOIN thread_goal_plans plan
      ON plan.plan_id = node.plan_id
    WHERE plan.thread_id = ?
      AND node.assigned_thread_id = thread_goals.thread_id
      AND node.projected_goal_id = thread_goals.goal_id
)
            "#,
        )
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"
DELETE FROM thread_goal_plans
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn cancel_pending_goal_plan_nodes_assigned_to_thread(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE assigned_thread_id = ?
  AND thread_id != ?
  AND status = 'pending'
RETURNING plan_id
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Cancelled.as_str())
        .bind(now_ms)
        .bind(thread_id.to_string())
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

        let mut snapshots = Vec::with_capacity(plan_ids.len());
        for plan_id in plan_ids {
            recalculate_goal_plan_status_in_tx(&mut tx, &plan_id, now_ms).await?;
            snapshots.push(snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?);
        }
        tx.commit().await?;
        Ok(snapshots)
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
WHERE assigned_thread_id = ?
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
            recalculate_goal_plan_status_in_tx(&mut tx, plan_id, now_ms).await?;
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
WHERE assigned_thread_id = ?
  AND projected_goal_id = ?
  AND status IN ('active', 'paused', 'blocked', 'usage_limited', 'budget_limited', 'deferred')
  AND EXISTS (
      SELECT 1
      FROM thread_goal_plans plan
      WHERE plan.plan_id = thread_goal_plan_nodes.plan_id
        AND plan.status IN ('active', 'paused', 'blocked', 'budget_limited')
  )
  AND EXISTS (
	      SELECT 1
	      FROM thread_goals goal
	      WHERE goal.thread_id = thread_goal_plan_nodes.assigned_thread_id
	        AND goal.goal_id = thread_goal_plan_nodes.projected_goal_id
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
        mark_pending_nodes_budget_limited_if_plan_spent_in_tx(&mut tx, &plan_id, now_ms).await?;
        recalculate_goal_plan_status_in_tx(&mut tx, &plan_id, now_ms).await?;
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
WHERE assigned_thread_id = ?
  AND projected_goal_id = ?
  AND status IN ('active', 'paused', 'blocked', 'usage_limited', 'budget_limited', 'deferred')
  AND EXISTS (
      SELECT 1
      FROM thread_goal_plans plan
      WHERE plan.plan_id = thread_goal_plan_nodes.plan_id
        AND plan.status IN ('active', 'paused', 'blocked', 'budget_limited')
  )
  AND EXISTS (
	      SELECT 1
	      FROM thread_goals goal
	      WHERE goal.thread_id = thread_goal_plan_nodes.assigned_thread_id
	        AND goal.goal_id = thread_goal_plan_nodes.projected_goal_id
	        AND goal.status = 'complete'
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
        let hit_plan_budget =
            mark_pending_nodes_budget_limited_if_plan_spent_in_tx(&mut tx, &plan_id, now_ms)
                .await?;
        if hit_plan_budget {
            recalculate_goal_plan_status_in_tx(&mut tx, &plan_id, now_ms).await?;
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
        recalculate_goal_plan_status_in_tx(&mut tx, &plan_id, now_ms).await?;
        let activated_goal = if incomplete_count == 0 {
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
        recalculate_goal_plan_status_in_tx(&mut tx, &plan_id, now_ms).await?;
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, &plan_id).await?;
        tx.commit().await?;
        Ok(Some(ThreadGoalPlanAdvanceOutcome {
            snapshot,
            activated_goal,
        }))
    }

    pub async fn defer_goal_plan_node_and_maybe_advance(
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
  AND status IN ('active', 'paused', 'blocked', 'usage_limited', 'budget_limited', 'deferred')
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
        AND current_goal.status = 'deferred'
  )
RETURNING plan_id
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Deferred.as_str())
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
        if plan.auto_execute != effective_auto_execute
            || plan.status != crate::ThreadGoalPlanStatus::Active
        {
            sqlx::query(
                r#"
UPDATE thread_goal_plans
SET status = ?, auto_execute = ?, updated_at_ms = ?
WHERE plan_id = ?
                "#,
            )
            .bind(crate::ThreadGoalPlanStatus::Active.as_str())
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

        let activated_goal = activate_next_ready_node_in_tx(
            &mut tx,
            thread_id,
            &plan_id,
            effective_auto_execute,
            now_ms,
        )
        .await?;
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

    /// Resume a goal-plan node that was previously deferred, re-activating it as
    /// the thread's current goal without disturbing the rest of the plan.
    ///
    /// This is the escape hatch for a plan that has stalled after a deferral: an
    /// independent node can complete while an earlier node stays deferred, which
    /// leaves the plan with no active or ready node even though a resumable
    /// deferred node (and its downstream dependents) remain. When `node_id` is
    /// provided the matching deferred node is resumed; otherwise the
    /// highest-priority deferred node in the thread's active plan is selected.
    /// Returns `Ok(None)` when no matching resumable deferred node exists.
    pub async fn resume_deferred_goal_plan_node(
        &self,
        thread_id: ThreadId,
        node_id: Option<&str>,
    ) -> anyhow::Result<Option<ThreadGoalPlanAdvanceOutcome>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let Some((plan_id, target_node_id)) =
            resumable_deferred_node_in_tx(&mut tx, thread_id, node_id).await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        let activated_goal =
            activate_node_in_tx(&mut tx, thread_id, target_node_id.as_str(), now_ms).await?;
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
    let ready_nodes = ready_plan_nodes_in_tx(tx, plan_id).await?;
    let ready_nodes_for_thread = ready_nodes
        .iter()
        .filter(|node| node.assigned_thread_id == thread_id)
        .collect::<Vec<_>>();
    let node_id = match auto_execute {
        crate::ThreadGoalPlanAutoExecute::Off => return Ok(None),
        crate::ThreadGoalPlanAutoExecute::ReadyOnly if ready_nodes_for_thread.len() != 1 => {
            return Ok(None);
        }
        crate::ThreadGoalPlanAutoExecute::ReadyOnly
        | crate::ThreadGoalPlanAutoExecute::AiDirected => ready_nodes_for_thread.first(),
    };
    let Some(node) = node_id else {
        return Ok(None);
    };
    activate_node_in_tx(tx, thread_id, node.node_id.as_str(), now_ms)
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
        let title =
            normalize_thread_goal_title(node.title.as_deref()).map_err(anyhow::Error::msg)?;
        node_ids_by_key.insert(node.key.clone(), node_id.clone());
        sqlx::query(
            r#"
	INSERT INTO thread_goal_plan_nodes (
	    node_id,
	    plan_id,
	    thread_id,
	    assigned_thread_id,
	    key,
	    sequence,
	    priority,
    objective,
    title,
    status,
    token_budget,
    created_at_ms,
    updated_at_ms
	) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&node_id)
        .bind(&plan_id)
        .bind(params.thread_id.to_string())
        .bind(
            node.assigned_thread_id
                .unwrap_or(params.thread_id)
                .to_string(),
        )
        .bind(&node.key)
        .bind(i64::try_from(sequence)?)
        .bind(node.priority)
        .bind(&node.objective)
        .bind(title)
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

async fn append_thread_goal_plan_nodes_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: ThreadGoalPlanAppendParams,
    now_ms: i64,
) -> anyhow::Result<crate::ThreadGoalPlanSnapshot> {
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, params.plan_id.as_str()).await?;
    if snapshot.plan.thread_id != params.thread_id {
        anyhow::bail!(
            "goal plan {} does not belong to thread {}",
            params.plan_id,
            params.thread_id
        );
    }
    if snapshot.plan.status != crate::ThreadGoalPlanStatus::Active {
        anyhow::bail!("can only append goals to an active goal plan");
    }

    let max_total_nodes = params.max_total_nodes.unwrap_or(MAX_GOAL_PLAN_NODES);
    let max_total_nodes = max_total_nodes.min(MAX_GOAL_PLAN_NODES);
    let total_nodes = snapshot.nodes.len().saturating_add(params.nodes.len());
    if total_nodes > max_total_nodes {
        anyhow::bail!(
            "goal plan would contain {total_nodes} goals but the maximum is {max_total_nodes}"
        );
    }

    let combined_nodes = snapshot
        .nodes
        .iter()
        .map(|node| ThreadGoalPlanNodeCreateParams {
            key: node.key.clone(),
            objective: node.objective.clone(),
            assigned_thread_id: None,
            title: None,
            priority: node.priority,
            token_budget: node.token_budget,
            depends_on: node.depends_on.clone(),
        })
        .chain(params.nodes.iter().cloned())
        .collect::<Vec<_>>();
    validate_plan_create_params(&ThreadGoalPlanCreateParams {
        thread_id: params.thread_id,
        auto_execute: snapshot.plan.auto_execute,
        max_tokens: snapshot.plan.max_tokens,
        nodes: combined_nodes,
    })?;

    let mut node_ids_by_key = snapshot
        .nodes
        .iter()
        .map(|node| (node.key.clone(), node.node_id.clone()))
        .collect::<HashMap<_, _>>();
    let next_sequence = snapshot
        .nodes
        .iter()
        .map(|node| node.sequence)
        .max()
        .unwrap_or(-1)
        + 1;

    for (index, node) in params.nodes.iter().enumerate() {
        let node_id = Uuid::new_v4().to_string();
        node_ids_by_key.insert(node.key.clone(), node_id.clone());
        sqlx::query(
            r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    assigned_thread_id,
    key,
    sequence,
    priority,
    objective,
    title,
    status,
    token_budget,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&node_id)
        .bind(&params.plan_id)
        .bind(params.thread_id.to_string())
        .bind(
            node.assigned_thread_id
                .unwrap_or(params.thread_id)
                .to_string(),
        )
        .bind(&node.key)
        .bind(next_sequence + i64::try_from(index)?)
        .bind(node.priority)
        .bind(&node.objective)
        .bind(normalize_thread_goal_title(node.title.as_deref()).map_err(anyhow::Error::msg)?)
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
            .ok_or_else(|| anyhow::anyhow!("missing appended goal node key {}", node.key))?;
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

    sqlx::query(
        r#"
UPDATE thread_goal_plans
SET updated_at_ms = ?
WHERE plan_id = ?
        "#,
    )
    .bind(now_ms)
    .bind(&params.plan_id)
    .execute(&mut **tx)
    .await?;

    snapshot_thread_goal_plan_in_tx(tx, params.plan_id.as_str()).await
}

async fn create_goal_plan_from_goal_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &ThreadGoalPlanAddParams,
    goal: &crate::ThreadGoal,
    now_ms: i64,
) -> anyhow::Result<(crate::ThreadGoalPlanSnapshot, crate::ThreadGoalPlanNode)> {
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
) VALUES (?, ?, ?, ?, NULL, ?, ?)
        "#,
    )
    .bind(&plan_id)
    .bind(params.thread_id.to_string())
    .bind(crate::ThreadGoalPlanStatus::Active.as_str())
    .bind(params.auto_execute.as_str())
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    assigned_thread_id,
    key,
    sequence,
    priority,
    objective,
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&plan_id)
    .bind(params.thread_id.to_string())
    .bind(params.thread_id.to_string())
    .bind("current")
    .bind(&goal.objective)
    .bind(&goal.title)
    .bind(crate::ThreadGoalPlanNodeStatus::from(goal.status).as_str())
    .bind(goal.token_budget)
    .bind(goal.tokens_used)
    .bind(goal.time_used_seconds)
    .bind(&goal.goal_id)
    .bind(datetime_to_epoch_millis(goal.created_at))
    .bind(datetime_to_epoch_millis(goal.updated_at))
    .execute(&mut **tx)
    .await?;

    append_goal_plan_node_in_tx(tx, &plan_id, params, now_ms).await
}

async fn create_goal_plan_from_added_goal_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &ThreadGoalPlanAddParams,
    now_ms: i64,
) -> anyhow::Result<(
    crate::ThreadGoalPlanSnapshot,
    crate::ThreadGoalPlanNode,
    crate::ThreadGoal,
)> {
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
) VALUES (?, ?, ?, ?, NULL, ?, ?)
        "#,
    )
    .bind(&plan_id)
    .bind(params.thread_id.to_string())
    .bind(crate::ThreadGoalPlanStatus::Active.as_str())
    .bind(params.auto_execute.as_str())
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;

    let node_id = Uuid::new_v4().to_string();
    let title = normalize_thread_goal_title(params.title.as_deref()).map_err(anyhow::Error::msg)?;
    sqlx::query(
        r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    assigned_thread_id,
    key,
    sequence,
    priority,
    objective,
    title,
    status,
    token_budget,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&node_id)
    .bind(&plan_id)
    .bind(params.thread_id.to_string())
    .bind(params.thread_id.to_string())
    .bind("goal_1")
    .bind(&params.objective)
    .bind(title)
    .bind(crate::ThreadGoalPlanNodeStatus::Pending.as_str())
    .bind(params.token_budget)
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;

    let activated_goal = activate_node_in_tx(tx, params.thread_id, &node_id, now_ms).await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, &plan_id).await?;
    let added_node = snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("added goal plan node was not persisted"))?;
    Ok((snapshot, added_node, activated_goal))
}

async fn append_goal_plan_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
    params: &ThreadGoalPlanAddParams,
    now_ms: i64,
) -> anyhow::Result<(crate::ThreadGoalPlanSnapshot, crate::ThreadGoalPlanNode)> {
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, plan_id).await?;
    if snapshot.nodes.len() >= MAX_GOAL_PLAN_NODES {
        anyhow::bail!(
            "goal plan contains {} goals but the maximum is {MAX_GOAL_PLAN_NODES}",
            snapshot.nodes.len()
        );
    }

    let existing_keys = snapshot
        .nodes
        .iter()
        .map(|node| node.key.as_str())
        .collect::<HashSet<_>>();
    let node_key = next_goal_plan_node_key(&existing_keys);
    let dependency_keys = append_dependency_keys(&snapshot);
    let sequence = snapshot
        .nodes
        .iter()
        .map(|node| node.sequence)
        .max()
        .unwrap_or(-1)
        + 1;
    let node_id = Uuid::new_v4().to_string();
    let title = normalize_thread_goal_title(params.title.as_deref()).map_err(anyhow::Error::msg)?;
    sqlx::query(
        r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    assigned_thread_id,
    key,
    sequence,
    priority,
    objective,
    title,
    status,
    token_budget,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&node_id)
    .bind(plan_id)
    .bind(snapshot.plan.thread_id.to_string())
    .bind(params.thread_id.to_string())
    .bind(&node_key)
    .bind(sequence)
    .bind(&params.objective)
    .bind(title)
    .bind(crate::ThreadGoalPlanNodeStatus::Pending.as_str())
    .bind(params.token_budget)
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;

    for dependency_key in dependency_keys {
        let dependency_id = snapshot
            .nodes
            .iter()
            .find(|node| node.key == dependency_key)
            .map(|node| node.node_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing dependency node `{dependency_key}`"))?;
        sqlx::query(
            r#"
INSERT INTO thread_goal_plan_dependencies (node_id, depends_on_node_id)
VALUES (?, ?)
            "#,
        )
        .bind(&node_id)
        .bind(dependency_id)
        .execute(&mut **tx)
        .await?;
    }

    recalculate_goal_plan_status_in_tx(tx, plan_id, now_ms).await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, plan_id).await?;
    let added_node = snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("appended goal plan node was not persisted"))?;
    Ok((snapshot, added_node))
}

async fn appendable_goal_plan_id_for_goal_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    goal: &crate::ThreadGoal,
) -> anyhow::Result<Option<String>> {
    let plan_id = sqlx::query_scalar(
        r#"
SELECT plan.plan_id
FROM thread_goal_plan_nodes node
JOIN thread_goal_plans plan
  ON plan.plan_id = node.plan_id
WHERE node.assigned_thread_id = ?
  AND node.projected_goal_id = ?
  AND plan.status IN ('active', 'paused', 'blocked', 'budget_limited')
ORDER BY plan.created_at_ms DESC, plan.plan_id DESC
LIMIT 1
        "#,
    )
    .bind(thread_id.to_string())
    .bind(&goal.goal_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(plan_id)
}

async fn newest_appendable_goal_plan_id_for_thread_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
) -> anyhow::Result<Option<String>> {
    let plan_id = sqlx::query_scalar(
        r#"
SELECT plan.plan_id
FROM thread_goal_plans plan
WHERE plan.status IN ('active', 'paused', 'blocked', 'budget_limited')
  AND (
      plan.thread_id = ?
      OR EXISTS (
          SELECT 1
          FROM thread_goal_plan_nodes node
          WHERE node.plan_id = plan.plan_id
            AND node.assigned_thread_id = ?
      )
  )
ORDER BY
    CASE WHEN plan.thread_id = ? THEN 0 ELSE 1 END,
    plan.created_at_ms DESC,
    plan.plan_id DESC
LIMIT 1
        "#,
    )
    .bind(thread_id.to_string())
    .bind(thread_id.to_string())
    .bind(thread_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    Ok(plan_id)
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
WHERE assigned_thread_id = ?
  AND status = 'active'
	        "#,
    )
    .bind(thread_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if active_count > 0 {
        anyhow::bail!("cannot activate goal plan node while another plan node is active");
    }
    let goal_status: Option<String> = sqlx::query_scalar(
        r#"
SELECT status
FROM thread_goals
WHERE thread_id = ?
        "#,
    )
    .bind(thread_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(status) = goal_status.as_deref() {
        let status = crate::ThreadGoalStatus::try_from(status)?;
        if !matches!(
            status,
            crate::ThreadGoalStatus::BudgetLimited
                | crate::ThreadGoalStatus::Deferred
                | crate::ThreadGoalStatus::Complete
                | crate::ThreadGoalStatus::Cancelled
        ) {
            anyhow::bail!("cannot activate goal plan node while thread has a non-terminal goal");
        }
    }
    let row = sqlx::query(
        r#"
SELECT
	    node_id,
	    plan_id,
	    thread_id,
	    assigned_thread_id,
	    key,
	    sequence,
	    priority,
    objective,
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    projected_goal_id,
    created_at_ms,
    updated_at_ms
	FROM thread_goal_plan_nodes
WHERE node_id = ?
	  AND assigned_thread_id = ?
	  AND status IN ('pending', 'deferred')
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
        let reserved_tokens = reserved_plan_tokens_in_tx(tx, &node.plan_id).await?;
        Some(max_tokens.saturating_sub(reserved_tokens).max(0))
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
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?, ?)
ON CONFLICT(thread_id) DO UPDATE SET
    goal_id = excluded.goal_id,
    objective = excluded.objective,
    title = excluded.title,
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
    title,
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
    .bind(&node.title)
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
  AND status IN ('pending', 'deferred')
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

struct ReadyPlanNode {
    node_id: String,
    assigned_thread_id: ThreadId,
}

async fn ready_plan_nodes_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
) -> anyhow::Result<Vec<ReadyPlanNode>> {
    let rows = sqlx::query(
        r#"
SELECT candidate.node_id, candidate.assigned_thread_id
FROM thread_goal_plan_nodes candidate
JOIN thread_goal_plans plan
  ON plan.plan_id = candidate.plan_id
WHERE candidate.plan_id = ?
	  AND candidate.status = 'pending'
  AND plan.status = 'active'
  AND (
      plan.max_tokens IS NULL
      OR (
          SELECT COALESCE(SUM(
              CASE
                  WHEN plan_node.status IN ('active', 'paused', 'blocked', 'usage_limited')
                    AND plan_node.token_budget IS NOT NULL
                    AND plan_node.token_budget > plan_node.tokens_used
                  THEN plan_node.token_budget
                  ELSE plan_node.tokens_used
              END
          ), 0)
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
    .bind(plan_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let node_id: String = row.try_get("node_id")?;
            let assigned_thread_id: String = row.try_get("assigned_thread_id")?;
            Ok(ReadyPlanNode {
                node_id,
                assigned_thread_id: ThreadId::try_from(assigned_thread_id)?,
            })
        })
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
WHERE candidate.assigned_thread_id = ?
  AND candidate.node_id = ?
  AND candidate.status IN ('pending', 'deferred')
  AND plan.status = 'active'
  AND (
      plan.max_tokens IS NULL
      OR (
          SELECT COALESCE(SUM(
              CASE
                  WHEN plan_node.status IN ('active', 'paused', 'blocked', 'usage_limited')
                    AND plan_node.token_budget IS NOT NULL
                    AND plan_node.token_budget > plan_node.tokens_used
                  THEN plan_node.token_budget
                  ELSE plan_node.tokens_used
              END
          ), 0)
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

/// Locate a deferred goal-plan node that is eligible to be resumed for the
/// thread. When `node_id` is supplied only that node is considered; otherwise
/// the highest-priority (then earliest) deferred node in the thread's active
/// plan is selected. Returns `(plan_id, node_id)` when a match is found.
async fn resumable_deferred_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    node_id: Option<&str>,
) -> anyhow::Result<Option<(String, String)>> {
    let row = if let Some(node_id) = node_id {
        sqlx::query(
            r#"
SELECT candidate.plan_id, candidate.node_id
FROM thread_goal_plan_nodes candidate
JOIN thread_goal_plans plan
  ON plan.plan_id = candidate.plan_id
WHERE candidate.assigned_thread_id = ?
  AND candidate.node_id = ?
  AND candidate.status = 'deferred'
  AND plan.status = 'active'
LIMIT 1
            "#,
        )
        .bind(thread_id.to_string())
        .bind(node_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query(
            r#"
SELECT candidate.plan_id, candidate.node_id
FROM thread_goal_plan_nodes candidate
JOIN thread_goal_plans plan
  ON plan.plan_id = candidate.plan_id
WHERE candidate.assigned_thread_id = ?
  AND candidate.status = 'deferred'
  AND plan.status = 'active'
ORDER BY candidate.priority DESC, candidate.sequence ASC, candidate.node_id ASC
LIMIT 1
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_optional(&mut **tx)
        .await?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let plan_id: String = row.try_get("plan_id")?;
    let node_id: String = row.try_get("node_id")?;
    Ok(Some((plan_id, node_id)))
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

async fn reserved_plan_tokens_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
) -> anyhow::Result<i64> {
    let total = sqlx::query_scalar(
        r#"
SELECT COALESCE(SUM(
    CASE
        WHEN status IN ('active', 'paused', 'blocked', 'usage_limited')
          AND token_budget IS NOT NULL
          AND token_budget > tokens_used
        THEN token_budget
        ELSE tokens_used
    END
), 0)
FROM thread_goal_plan_nodes
WHERE plan_id = ?
        "#,
    )
    .bind(plan_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(total)
}

async fn mark_pending_nodes_budget_limited_if_plan_spent_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
    now_ms: i64,
) -> anyhow::Result<bool> {
    let plan = get_thread_goal_plan_in_tx(tx, plan_id).await?;
    let Some(max_tokens) = plan.max_tokens else {
        return Ok(false);
    };
    let total_tokens = total_plan_tokens_in_tx(tx, plan_id).await?;
    if total_tokens < max_tokens {
        return Ok(false);
    }

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
    .bind(plan_id)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

pub(super) async fn recalculate_goal_plan_status_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
    now_ms: i64,
) -> anyhow::Result<crate::ThreadGoalPlanStatus> {
    let counts: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
SELECT
    COUNT(*),
    SUM(CASE WHEN status = 'complete' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'paused' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'blocked' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'usage_limited' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'budget_limited' THEN 1 ELSE 0 END),
    SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END),
    COALESCE(SUM(tokens_used), 0)
FROM thread_goal_plan_nodes
WHERE plan_id = ?
        "#,
    )
    .bind(plan_id)
    .fetch_one(&mut **tx)
    .await?;
    let (
        node_count,
        complete_count,
        active_count,
        pending_count,
        paused_count,
        blocked_count,
        usage_limited_count,
        budget_limited_count,
        cancelled_count,
        total_tokens,
    ) = counts;

    let ready_count = ready_node_count_ignoring_plan_status_in_tx(tx, plan_id).await?;
    let plan = get_thread_goal_plan_in_tx(tx, plan_id).await?;
    let status = if node_count > 0 && complete_count == node_count {
        crate::ThreadGoalPlanStatus::Complete
    } else if node_count > 0
        && cancelled_count > 0
        && complete_count + cancelled_count == node_count
    {
        crate::ThreadGoalPlanStatus::Cancelled
    } else if plan
        .max_tokens
        .is_some_and(|max_tokens| total_tokens >= max_tokens)
    {
        crate::ThreadGoalPlanStatus::BudgetLimited
    } else if active_count > 0 || ready_count > 0 {
        crate::ThreadGoalPlanStatus::Active
    } else if pending_count > 0 && paused_count > 0 {
        crate::ThreadGoalPlanStatus::Paused
    } else if pending_count > 0
        && (blocked_count > 0 || usage_limited_count > 0 || cancelled_count > 0)
    {
        crate::ThreadGoalPlanStatus::Blocked
    } else if pending_count > 0 && budget_limited_count > 0 {
        crate::ThreadGoalPlanStatus::BudgetLimited
    } else if pending_count > 0 {
        crate::ThreadGoalPlanStatus::Active
    } else if paused_count > 0 {
        crate::ThreadGoalPlanStatus::Paused
    } else if blocked_count > 0 || usage_limited_count > 0 || cancelled_count > 0 {
        crate::ThreadGoalPlanStatus::Blocked
    } else if budget_limited_count > 0 {
        crate::ThreadGoalPlanStatus::BudgetLimited
    } else {
        crate::ThreadGoalPlanStatus::Active
    };

    sqlx::query(
        r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
        "#,
    )
    .bind(status.as_str())
    .bind(now_ms)
    .bind(plan_id)
    .execute(&mut **tx)
    .await?;
    Ok(status)
}

async fn ready_node_count_ignoring_plan_status_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    plan_id: &str,
) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM thread_goal_plan_nodes candidate
JOIN thread_goal_plans plan
  ON plan.plan_id = candidate.plan_id
WHERE candidate.plan_id = ?
  AND candidate.status = 'pending'
  AND (
      plan.max_tokens IS NULL
      OR (
          SELECT COALESCE(SUM(
              CASE
                  WHEN plan_node.status IN ('active', 'paused', 'blocked', 'usage_limited')
                    AND plan_node.token_budget IS NOT NULL
                    AND plan_node.token_budget > plan_node.tokens_used
                  THEN plan_node.token_budget
                  ELSE plan_node.tokens_used
              END
          ), 0)
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
    .bind(plan_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(count)
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

async fn get_thread_goal_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
) -> anyhow::Result<Option<crate::ThreadGoal>> {
    let row = sqlx::query(
        r#"
SELECT
    thread_id,
    goal_id,
    objective,
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms
FROM thread_goals
WHERE thread_id = ?
        "#,
    )
    .bind(thread_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;

    row.map(|row| thread_goal_from_row(&row)).transpose()
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
	    assigned_thread_id,
	    key,
	    sequence,
    priority,
    objective,
    title,
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
        if let Some(title) = node.title.as_deref() {
            normalize_thread_goal_title(Some(title)).map_err(anyhow::Error::msg)?;
        }
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

fn validate_plan_append_params(params: &ThreadGoalPlanAppendParams) -> anyhow::Result<()> {
    if params.plan_id.trim().is_empty() {
        anyhow::bail!("goal plan id must not be empty");
    }
    if params.nodes.is_empty() {
        anyhow::bail!("goal plan append must contain at least one goal");
    }
    if params.max_total_nodes == Some(0) {
        anyhow::bail!("goal plan max_total_nodes must be positive when set");
    }
    Ok(())
}

fn validate_plan_add_params(params: &ThreadGoalPlanAddParams) -> anyhow::Result<()> {
    validate_thread_goal_objective(params.objective.trim()).map_err(anyhow::Error::msg)?;
    if let Some(title) = params.title.as_deref() {
        normalize_thread_goal_title(Some(title)).map_err(anyhow::Error::msg)?;
    }
    if params.token_budget.is_some_and(|budget| budget <= 0) {
        anyhow::bail!("goal plan node token_budget must be positive when set");
    }
    Ok(())
}

fn is_valid_goal_plan_node_key(key: &str) -> bool {
    key.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn next_goal_plan_node_key(existing_keys: &HashSet<&str>) -> String {
    for idx in 1..=MAX_GOAL_PLAN_NODES + 1 {
        let candidate = format!("goal_{idx}");
        if !existing_keys.contains(candidate.as_str()) {
            return candidate;
        }
    }
    "goal".to_string()
}

fn append_dependency_keys(snapshot: &crate::ThreadGoalPlanSnapshot) -> Vec<String> {
    let depended_on = snapshot
        .nodes
        .iter()
        .flat_map(|node| node.depends_on.iter().map(String::as_str))
        .collect::<HashSet<_>>();
    snapshot
        .nodes
        .iter()
        .filter(|node| !depended_on.contains(node.key.as_str()))
        .map(|node| node.key.clone())
        .collect()
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

    fn test_delegate_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000457").expect("valid thread id")
    }

    fn test_second_delegate_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000458").expect("valid thread id")
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

    fn goal_node(
        key: &str,
        objective: &str,
        depends_on: &[&str],
    ) -> ThreadGoalPlanNodeCreateParams {
        ThreadGoalPlanNodeCreateParams {
            key: key.to_string(),
            objective: objective.to_string(),
            assigned_thread_id: None,
            title: None,
            priority: 0,
            token_budget: None,
            depends_on: depends_on
                .iter()
                .map(|dependency| String::from(*dependency))
                .collect(),
        }
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
                        assigned_thread_id: None,
                        title: Some("Investigate goal plans".to_string()),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "implement".to_string(),
                        objective: "Implement goal plans.".to_string(),
                        assigned_thread_id: None,
                        title: None,
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
        assert_eq!(Some("Investigate goal plans".to_string()), first_goal.title);
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
                    title: None,
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
    async fn append_goal_plan_nodes_queues_followup_without_replacing_active_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "first".to_string(),
                    objective: "Run the first goal.".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("goal plan should be created");
        let first_goal = created
            .activated_goal
            .expect("first ready goal should activate");

        let appended = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id,
                max_total_nodes: Some(4),
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "second".to_string(),
                    objective: "Run the appended follow-up goal.".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: Some(2_000),
                    depends_on: vec!["first".to_string()],
                }],
            })
            .await
            .expect("goal plan append should succeed");

        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Active,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            appended
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            Some(first_goal.clone()),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("goal should read")
        );

        let completed = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
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
            .expect("appended follow-up should activate");
        assert_eq!("Run the appended follow-up goal.", second_goal.objective);
        assert_eq!(Some(2_000), second_goal.token_budget);
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
    async fn append_goal_plan_nodes_enforces_total_goal_cap() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "one".to_string(),
                        objective: "Do one goal.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "two".to_string(),
                        objective: "Do two goal.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                ],
            })
            .await
            .expect("goal plan should be created");

        let err = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id,
                max_total_nodes: Some(2),
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "three".to_string(),
                    objective: "Do three goal.".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect_err("append beyond cap should fail");

        assert!(
            err.to_string()
                .contains("goal plan would contain 3 goals but the maximum is 2")
        );
    }

    #[tokio::test]
    async fn append_goal_plan_nodes_rejects_invalid_combined_graph() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![goal_node("first", "Run the first goal.", &[])],
            })
            .await
            .expect("goal plan should be created");

        let duplicate = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id.clone(),
                max_total_nodes: Some(8),
                nodes: vec![goal_node("first", "Duplicate the existing goal key.", &[])],
            })
            .await
            .expect_err("duplicate key should fail");
        assert!(
            duplicate
                .to_string()
                .contains("goal plan node key `first` is duplicated")
        );

        let unknown_dependency = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id.clone(),
                max_total_nodes: Some(8),
                nodes: vec![goal_node(
                    "second",
                    "Depend on a missing goal key.",
                    &["missing"],
                )],
            })
            .await
            .expect_err("unknown dependency should fail");
        assert!(
            unknown_dependency
                .to_string()
                .contains("goal plan node `second` depends on unknown node `missing`")
        );

        let cycle = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id,
                max_total_nodes: Some(8),
                nodes: vec![
                    goal_node("second", "Depend on the third goal.", &["third"]),
                    goal_node("third", "Depend on the second goal.", &["second"]),
                ],
            })
            .await
            .expect_err("cycle should fail");
        assert!(
            cycle
                .to_string()
                .contains("goal plan dependencies contain a cycle")
        );
    }

    #[tokio::test]
    async fn append_goal_plan_nodes_rejects_wrong_thread_and_inactive_plan() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let other_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000457").expect("valid thread id");
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![goal_node("first", "Run the first goal.", &[])],
            })
            .await
            .expect("goal plan should be created");

        let wrong_thread = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id: other_thread_id,
                plan_id: created.snapshot.plan.plan_id.clone(),
                max_total_nodes: Some(8),
                nodes: vec![goal_node("other", "Append from the wrong thread.", &[])],
            })
            .await
            .expect_err("wrong thread should fail");
        assert!(
            wrong_thread
                .to_string()
                .contains("does not belong to thread")
        );

        runtime
            .thread_goals()
            .block_active_goal_plan_nodes_for_thread(thread_id)
            .await
            .expect("active plan should block");
        let inactive = runtime
            .thread_goals()
            .append_thread_goal_plan_nodes(ThreadGoalPlanAppendParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id,
                max_total_nodes: Some(8),
                nodes: vec![goal_node("blocked", "Append to a blocked plan.", &[])],
            })
            .await
            .expect_err("inactive plan should fail");
        assert!(
            inactive
                .to_string()
                .contains("can only append goals to an active goal plan")
        );
    }

    #[tokio::test]
    async fn deferred_goal_plan_node_advances_only_to_independent_ready_node() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "investigate".to_string(),
                        objective: "Investigate the goal.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 10,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "implement".to_string(),
                        objective: "Implement the dependent goal.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 5,
                        token_budget: None,
                        depends_on: vec!["investigate".to_string()],
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "cleanup".to_string(),
                        objective: "Run independent cleanup.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                ],
            })
            .await
            .expect("goal plan should be created");

        let first_goal = created
            .activated_goal
            .expect("highest priority ready goal should activate");
        assert_eq!("Investigate the goal.", first_goal.objective);

        let deferred = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Deferred),
                    token_budget: None,
                    expected_goal_id: Some(first_goal.goal_id),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        let advanced = runtime
            .thread_goals()
            .defer_goal_plan_node_and_maybe_advance(
                thread_id,
                &deferred,
                crate::ThreadGoalPlanAutoExecute::AiDirected,
            )
            .await
            .expect("goal plan should advance")
            .expect("goal plan outcome should exist");

        let activated_goal = advanced
            .activated_goal
            .expect("independent ready goal should activate");
        assert_eq!("Run independent cleanup.", activated_goal.objective);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Deferred,
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            advanced
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
        let summary = advanced.snapshot.usage_summary();
        assert_eq!(1, summary.deferred_node_count);
        assert_eq!(0, summary.ready_node_count);
        assert_eq!(
            crate::ThreadGoalPlanStatus::Active,
            advanced.snapshot.plan.status
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
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "two".to_string(),
                        objective: "Do another independent goal.".to_string(),
                        assigned_thread_id: None,
                        title: None,
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
    async fn add_goal_creates_plan_and_starts_first_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let added = runtime
            .thread_goals()
            .add_thread_goal_to_plan(ThreadGoalPlanAddParams {
                thread_id,
                objective: "Start with a plan-backed goal.".to_string(),
                title: Some("Plan backed goal".to_string()),
                token_budget: Some(250),
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
            })
            .await
            .expect("first goal should create and start a plan");

        let active_goal = added
            .activated_goal
            .clone()
            .expect("first added goal should activate");
        assert!(added.created_plan);
        assert_eq!(Some(active_goal.clone()), added.goal);
        assert_eq!(
            Some(active_goal.clone()),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("goal lookup should succeed")
        );
        assert_eq!(
            crate::ThreadGoalPlanAutoExecute::Off,
            added.snapshot.plan.auto_execute
        );
        assert_eq!(1, added.snapshot.nodes.len());
        assert_eq!("goal_1", added.added_node.key);
        assert_eq!("Start with a plan-backed goal.", added.added_node.objective);
        assert_eq!(Some("Plan backed goal".to_string()), added.added_node.title);
        assert_eq!(Some(250), added.added_node.token_budget);
        assert_eq!(
            crate::ThreadGoalPlanNodeStatus::Active,
            added.added_node.status
        );
        assert_eq!(
            Some(active_goal.goal_id.as_str()),
            added.added_node.projected_goal_id.as_deref()
        );
        assert_eq!(Vec::<String>::new(), added.added_node.depends_on);
        assert_eq!(
            Vec::<String>::new(),
            added.snapshot.ready_node_ids_for_thread(thread_id)
        );
    }

    #[tokio::test]
    async fn add_goal_after_terminal_goal_creates_fresh_plan_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let terminal_goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Already finished.",
                crate::ThreadGoalStatus::Complete,
                None,
            )
            .await
            .expect("terminal goal should be created");

        let added = runtime
            .thread_goals()
            .add_thread_goal_to_plan(ThreadGoalPlanAddParams {
                thread_id,
                objective: "Start fresh after terminal goal.".to_string(),
                title: None,
                token_budget: None,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            })
            .await
            .expect("terminal current goal should not be edited");

        let active_goal = added
            .activated_goal
            .clone()
            .expect("fresh plan goal should activate");
        assert_ne!(terminal_goal.goal_id, active_goal.goal_id);
        assert_eq!(Some(active_goal.clone()), added.goal);
        assert_eq!("Start fresh after terminal goal.", active_goal.objective);
        assert_eq!(crate::ThreadGoalStatus::Active, active_goal.status);
        assert_eq!(1, added.snapshot.nodes.len());
        assert_eq!("goal_1", added.added_node.key);
        assert_eq!(
            crate::ThreadGoalPlanNodeStatus::Active,
            added.added_node.status
        );
        assert_eq!(
            Some(active_goal.goal_id.as_str()),
            added.added_node.projected_goal_id.as_deref()
        );
    }

    #[tokio::test]
    async fn add_goal_wraps_goal_without_replacing_it() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let active_goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Finish the active goal.",
                crate::ThreadGoalStatus::Active,
                Some(100),
            )
            .await
            .expect("active goal should be created");
        runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 11,
                /*token_delta*/ 7,
                crate::GoalAccountingMode::ActiveOnly,
                Some(active_goal.goal_id.as_str()),
            )
            .await
            .expect("active goal usage should account");
        let active_goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("goal lookup should succeed")
            .expect("goal should still exist");

        let appended = runtime
            .thread_goals()
            .add_thread_goal_to_plan(ThreadGoalPlanAddParams {
                thread_id,
                objective: "Queue the follow-up goal.".to_string(),
                title: None,
                token_budget: None,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            })
            .await
            .expect("goal should append to a new plan");

        assert!(appended.created_plan);
        assert_eq!(Some(active_goal.clone()), appended.goal);
        assert_eq!(
            Some(active_goal.clone()),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("goal lookup should succeed")
        );
        assert_eq!(2, appended.snapshot.nodes.len());
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Active,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            appended
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let current_node = &appended.snapshot.nodes[0];
        assert_eq!("current", current_node.key);
        assert_eq!(active_goal.objective, current_node.objective);
        assert_eq!(
            Some(active_goal.goal_id.as_str()),
            current_node.projected_goal_id.as_deref()
        );
        assert_eq!(Some(100), current_node.token_budget);
        assert_eq!(7, current_node.tokens_used);
        assert_eq!(11, current_node.time_used_seconds);

        assert_eq!("Queue the follow-up goal.", appended.added_node.objective);
        assert_eq!(vec!["current".to_string()], appended.added_node.depends_on);
        assert_eq!(
            Vec::<String>::new(),
            appended.snapshot.ready_node_ids_for_thread(thread_id)
        );
    }

    #[tokio::test]
    async fn add_goal_extends_existing_plan_after_current_leaves() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "investigate".to_string(),
                        objective: "Investigate first.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "implement".to_string(),
                        objective: "Implement second.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["investigate".to_string()],
                    },
                ],
            })
            .await
            .expect("goal plan should be created");

        let appended = runtime
            .thread_goals()
            .add_thread_goal_to_plan(ThreadGoalPlanAddParams {
                thread_id,
                objective: "Validate third.".to_string(),
                title: None,
                token_budget: Some(123),
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            })
            .await
            .expect("goal should append to existing plan");

        assert!(!appended.created_plan);
        assert_eq!(None, appended.goal);
        assert_eq!(
            created.snapshot.plan.plan_id,
            appended.snapshot.plan.plan_id
        );
        assert_eq!(3, appended.snapshot.nodes.len());
        assert_eq!("goal_1", appended.added_node.key);
        assert_eq!("Validate third.", appended.added_node.objective);
        assert_eq!(Some(123), appended.added_node.token_budget);
        assert_eq!(
            vec!["implement".to_string()],
            appended.added_node.depends_on
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            appended
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn add_goal_generates_unique_goal_keys() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "goal_1".to_string(),
                        objective: "Existing generated key one.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "goal_2".to_string(),
                        objective: "Existing generated key two.".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["goal_1".to_string()],
                    },
                ],
            })
            .await
            .expect("goal plan should be created");

        let added = runtime
            .thread_goals()
            .add_thread_goal_to_plan(ThreadGoalPlanAddParams {
                thread_id,
                objective: "Use the next generated key.".to_string(),
                title: None,
                token_budget: None,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            })
            .await
            .expect("goal should append with a collision-free key");

        assert_eq!(created.snapshot.plan.plan_id, added.snapshot.plan.plan_id);
        assert_eq!("goal_3", added.added_node.key);
        assert_eq!(vec!["goal_2".to_string()], added.added_node.depends_on);
    }

    #[tokio::test]
    async fn add_goal_wraps_standalone_goal_before_unrelated_plan() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();

        let active_goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Finish the standalone current goal.",
                crate::ThreadGoalStatus::Active,
                None,
            )
            .await
            .expect("active goal should be created");
        let unrelated_plan = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "old".to_string(),
                    objective: "Existing unrelated plan work.".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("unrelated plan should be created");

        let appended = runtime
            .thread_goals()
            .add_thread_goal_to_plan(ThreadGoalPlanAddParams {
                thread_id,
                objective: "Queue the new follow-up.".to_string(),
                title: None,
                token_budget: None,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            })
            .await
            .expect("standalone current goal should be wrapped first");

        assert!(appended.created_plan);
        assert_ne!(
            unrelated_plan.snapshot.plan.plan_id,
            appended.snapshot.plan.plan_id
        );
        assert_eq!(Some(active_goal.clone()), appended.goal);
        assert_eq!(vec!["current".to_string()], appended.added_node.depends_on);
        assert_eq!(
            Some(active_goal.goal_id.as_str()),
            appended.snapshot.nodes[0].projected_goal_id.as_deref()
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
                        assigned_thread_id: None,
                        title: None,
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
    async fn delegated_goal_plan_node_activates_in_assigned_thread() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let delegate_thread_id = test_delegate_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "delegate".to_string(),
                    objective: "Complete the delegated goal node.".to_string(),
                    assigned_thread_id: Some(delegate_thread_id),
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("delegated goal plan should be created");
        let node_id = created.snapshot.nodes[0].node_id.clone();
        assert_eq!(primary_thread_id, created.snapshot.plan.thread_id);
        assert_eq!(primary_thread_id, created.snapshot.nodes[0].thread_id);
        assert_eq!(
            delegate_thread_id,
            created.snapshot.nodes[0].assigned_thread_id
        );

        let primary_plans = runtime
            .thread_goals()
            .list_thread_goal_plans(primary_thread_id)
            .await
            .expect("primary should list delegated plan");
        assert_eq!(created.snapshot, primary_plans[0]);
        let delegate_plans = runtime
            .thread_goals()
            .list_thread_goal_plans(delegate_thread_id)
            .await
            .expect("delegate should list assigned plan");
        assert_eq!(created.snapshot, delegate_plans[0]);
        assert_eq!(
            None,
            runtime
                .thread_goals()
                .activate_thread_goal_plan_node(primary_thread_id, node_id.as_str())
                .await
                .expect("primary should not activate a delegated node")
        );

        let activated = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(delegate_thread_id, node_id.as_str())
            .await
            .expect("delegate activation should not fail")
            .expect("delegate should activate assigned node");
        let delegated_goal = activated
            .activated_goal
            .clone()
            .expect("delegate activation should create goal");
        assert_eq!(delegate_thread_id, delegated_goal.thread_id);
        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(primary_thread_id)
                .await
                .expect("primary goal lookup should not fail")
        );
        assert_eq!(
            Some(delegated_goal.clone()),
            runtime
                .thread_goals()
                .get_thread_goal(delegate_thread_id)
                .await
                .expect("delegate goal lookup should not fail")
        );
        assert_eq!(primary_thread_id, activated.snapshot.plan.thread_id);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Active],
            activated
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let completed = runtime
            .thread_goals()
            .update_thread_goal(
                delegate_thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(delegated_goal.goal_id),
                },
            )
            .await
            .expect("delegate goal should update")
            .expect("delegate goal should exist");
        let advanced = runtime
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(
                delegate_thread_id,
                &completed,
                crate::ThreadGoalPlanAutoExecute::Off,
            )
            .await
            .expect("delegate completion should sync plan")
            .expect("completion should produce plan outcome");

        assert_eq!(
            crate::ThreadGoalPlanStatus::Complete,
            advanced.snapshot.plan.status
        );
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Complete],
            advanced
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn delegated_goal_plan_allows_parallel_assignees() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let first_delegate_thread_id = test_delegate_thread_id();
        let second_delegate_thread_id = test_second_delegate_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "first_delegate_a".to_string(),
                        objective: "Run the first delegate's first goal.".to_string(),
                        assigned_thread_id: Some(first_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "first_delegate_b".to_string(),
                        objective: "Run the first delegate's second goal.".to_string(),
                        assigned_thread_id: Some(first_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second_delegate".to_string(),
                        objective: "Run the second delegate's goal.".to_string(),
                        assigned_thread_id: Some(second_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                ],
            })
            .await
            .expect("delegated goal plan should be created");
        let first_delegate_node_id = created.snapshot.nodes[0].node_id.clone();
        let first_delegate_second_node_id = created.snapshot.nodes[1].node_id.clone();
        let second_delegate_node_id = created.snapshot.nodes[2].node_id.clone();

        runtime
            .thread_goals()
            .activate_thread_goal_plan_node(
                first_delegate_thread_id,
                first_delegate_node_id.as_str(),
            )
            .await
            .expect("first delegate activation should not fail")
            .expect("first delegate node should activate");
        let second_activation = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(
                second_delegate_thread_id,
                second_delegate_node_id.as_str(),
            )
            .await
            .expect("second delegate activation should not fail")
            .expect("second delegate node should activate");
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Active,
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            second_activation
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let err = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(
                first_delegate_thread_id,
                first_delegate_second_node_id.as_str(),
            )
            .await
            .expect_err("same delegate should not activate a second node");
        assert!(
            err.to_string()
                .contains("cannot activate goal plan node while another plan node is active")
        );
    }

    #[tokio::test]
    async fn ready_only_goal_plan_does_not_auto_start_other_assignee() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let delegate_thread_id = test_delegate_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "delegate".to_string(),
                    objective: "Run the delegated ready-only node.".to_string(),
                    assigned_thread_id: Some(delegate_thread_id),
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("delegated goal plan should be created");

        assert_eq!(None, created.activated_goal);
        assert_eq!(
            Vec::<String>::new(),
            created
                .snapshot
                .ready_node_ids_for_thread(primary_thread_id)
        );
        assert_eq!(
            vec![created.snapshot.nodes[0].node_id.clone()],
            created
                .snapshot
                .ready_node_ids_for_thread(delegate_thread_id)
        );
    }

    #[tokio::test]
    async fn delegated_goal_plan_reserves_active_node_budgets() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let first_delegate_thread_id = test_delegate_thread_id();
        let second_delegate_thread_id = test_second_delegate_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: Some(100),
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "first_delegate".to_string(),
                        objective: "Run the first delegated budgeted goal.".to_string(),
                        assigned_thread_id: Some(first_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: Some(40),
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second_delegate".to_string(),
                        objective: "Run the second delegated budgeted goal.".to_string(),
                        assigned_thread_id: Some(second_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: Some(80),
                        depends_on: Vec::new(),
                    },
                ],
            })
            .await
            .expect("delegated goal plan should be created");
        let first_node_id = created.snapshot.nodes[0].node_id.clone();
        let second_node_id = created.snapshot.nodes[1].node_id.clone();

        let first_activation = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(first_delegate_thread_id, first_node_id.as_str())
            .await
            .expect("first delegate activation should not fail")
            .expect("first delegate node should activate");
        assert_eq!(
            Some(40),
            first_activation
                .activated_goal
                .as_ref()
                .and_then(|goal| goal.token_budget)
        );

        let second_activation = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(second_delegate_thread_id, second_node_id.as_str())
            .await
            .expect("second delegate activation should not fail")
            .expect("second delegate node should activate");
        assert_eq!(
            Some(60),
            second_activation
                .activated_goal
                .as_ref()
                .and_then(|goal| goal.token_budget)
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Active,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            second_activation
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn delegated_goal_plan_keeps_active_plan_when_one_assignee_cancels() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let first_delegate_thread_id = test_delegate_thread_id();
        let second_delegate_thread_id = test_second_delegate_thread_id();

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![
                    ThreadGoalPlanNodeCreateParams {
                        key: "first_delegate".to_string(),
                        objective: "Run the first cancellable delegated goal.".to_string(),
                        assigned_thread_id: Some(first_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second_delegate".to_string(),
                        objective: "Run the second delegated goal after cancellation.".to_string(),
                        assigned_thread_id: Some(second_delegate_thread_id),
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                ],
            })
            .await
            .expect("delegated goal plan should be created");
        let first_node_id = created.snapshot.nodes[0].node_id.clone();
        let second_node_id = created.snapshot.nodes[1].node_id.clone();

        let first_goal = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(first_delegate_thread_id, first_node_id.as_str())
            .await
            .expect("first delegate activation should not fail")
            .expect("first delegate node should activate")
            .activated_goal
            .expect("first delegate activation should create goal");
        let second_goal = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(second_delegate_thread_id, second_node_id.as_str())
            .await
            .expect("second delegate activation should not fail")
            .expect("second delegate node should activate")
            .activated_goal
            .expect("second delegate activation should create goal");

        let cancelled_goal = runtime
            .thread_goals()
            .update_thread_goal(
                first_delegate_thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Cancelled),
                    token_budget: None,
                    expected_goal_id: Some(first_goal.goal_id),
                },
            )
            .await
            .expect("first delegate goal should update")
            .expect("first delegate goal should exist");
        let cancelled_snapshot = runtime
            .thread_goals()
            .sync_goal_plan_node_for_goal(first_delegate_thread_id, &cancelled_goal)
            .await
            .expect("cancelled delegate node should sync")
            .expect("cancelled delegate node should update the plan");
        assert_eq!(
            crate::ThreadGoalPlanStatus::Active,
            cancelled_snapshot.plan.status
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Cancelled,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            cancelled_snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        let completed_goal = runtime
            .thread_goals()
            .update_thread_goal(
                second_delegate_thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(second_goal.goal_id),
                },
            )
            .await
            .expect("second delegate goal should update")
            .expect("second delegate goal should exist");
        let completed_snapshot = runtime
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(
                second_delegate_thread_id,
                &completed_goal,
                crate::ThreadGoalPlanAutoExecute::Off,
            )
            .await
            .expect("second delegate completion should sync")
            .expect("completion should produce plan outcome")
            .snapshot;
        assert_eq!(
            crate::ThreadGoalPlanStatus::Cancelled,
            completed_snapshot.plan.status
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Cancelled,
                crate::ThreadGoalPlanNodeStatus::Complete,
            ],
            completed_snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
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
                    assigned_thread_id: None,
                    title: None,
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
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Should not run after the plan budget is exhausted.".to_string(),
                        assigned_thread_id: None,
                        title: None,
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
                    title: None,
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
                    assigned_thread_id: None,
                    title: None,
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
                    assigned_thread_id: None,
                    title: None,
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
                    title: None,
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
                    title: None,
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

        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Manual replacement after cancellation.",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("manual replacement should not rewrite cancelled node");
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
    async fn replacing_paused_projected_goal_blocks_old_plan_node() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "pause".to_string(),
                    objective: "Pause and replace the projected goal.".to_string(),
                    assigned_thread_id: None,
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("goal plan should be created");
        let active_goal = created.activated_goal.expect("goal should activate");

        let paused_goal = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: None,
                    expected_goal_id: Some(active_goal.goal_id),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        let paused_snapshot = runtime
            .thread_goals()
            .sync_goal_plan_node_for_goal(thread_id, &paused_goal)
            .await
            .expect("goal plan should sync")
            .expect("paused goal should update the plan");
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Paused],
            paused_snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );

        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Manual replacement after pause.",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("manual replacement should block old projected node");
        let snapshots = runtime
            .thread_goals()
            .list_thread_goal_plans(thread_id)
            .await
            .expect("plan should list");
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
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["b".to_string()],
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "b".to_string(),
                        objective: "Do goal B.".to_string(),
                        assigned_thread_id: None,
                        title: None,
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
                    assigned_thread_id: None,
                    title: None,
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
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Run the dependent projected goal.".to_string(),
                        assigned_thread_id: None,
                        title: None,
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
                /*token_budget*/ None,
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
    async fn activation_rolls_back_when_goal_is_non_terminal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Manual active goal.",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
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
                    assigned_thread_id: None,
                    title: None,
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
                assigned_thread_id: None,
                title: None,
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
                    assigned_thread_id: None,
                    title: None,
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

    #[tokio::test]
    async fn deleting_primary_thread_deletes_delegated_projected_goals() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let delegate_thread_id = ThreadId::new();
        upsert_test_thread(&runtime, primary_thread_id).await;
        upsert_test_thread(&runtime, delegate_thread_id).await;
        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "delegate".to_string(),
                    objective: "Clean up the delegated projected goal.".to_string(),
                    assigned_thread_id: Some(delegate_thread_id),
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("delegated goal plan should be created");

        runtime
            .thread_goals()
            .activate_thread_goal_plan_node(
                delegate_thread_id,
                created.snapshot.nodes[0].node_id.as_str(),
            )
            .await
            .expect("delegate activation should not fail")
            .expect("delegate node should activate");
        assert!(
            runtime
                .thread_goals()
                .get_thread_goal(delegate_thread_id)
                .await
                .expect("delegate goal should load")
                .is_some()
        );

        runtime
            .delete_thread(primary_thread_id)
            .await
            .expect("primary thread deletion should succeed");

        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(delegate_thread_id)
                .await
                .expect("delegate goal should load")
        );
        assert_eq!(
            Vec::<crate::ThreadGoalPlanSnapshot>::new(),
            runtime
                .thread_goals()
                .list_thread_goal_plans(delegate_thread_id)
                .await
                .expect("delegated plan should be deleted")
        );
    }

    #[tokio::test]
    async fn deleting_delegate_thread_cancels_pending_delegated_nodes() {
        let runtime = test_runtime().await;
        let primary_thread_id = test_thread_id();
        let delegate_thread_id = ThreadId::new();
        upsert_test_thread(&runtime, primary_thread_id).await;
        upsert_test_thread(&runtime, delegate_thread_id).await;
        runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id: primary_thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![ThreadGoalPlanNodeCreateParams {
                    key: "delegate".to_string(),
                    objective: "Cancel this pending delegated node when its thread is deleted."
                        .to_string(),
                    assigned_thread_id: Some(delegate_thread_id),
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("delegated goal plan should be created");

        runtime
            .delete_thread(delegate_thread_id)
            .await
            .expect("delegate thread deletion should succeed");

        let plans = runtime
            .thread_goals()
            .list_thread_goal_plans(primary_thread_id)
            .await
            .expect("owner should still list plan");
        assert_eq!(1, plans.len());
        assert_eq!(crate::ThreadGoalPlanStatus::Cancelled, plans[0].plan.status);
        assert_eq!(
            vec![crate::ThreadGoalPlanNodeStatus::Cancelled],
            plans[0]
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }

    async fn defer_current_goal_and_advance(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        auto_execute: crate::ThreadGoalPlanAutoExecute,
    ) -> ThreadGoalPlanAdvanceOutcome {
        let current = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("current goal should read")
            .expect("current goal should exist");
        let deferred = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Deferred),
                    token_budget: None,
                    expected_goal_id: Some(current.goal_id.clone()),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        runtime
            .thread_goals()
            .defer_goal_plan_node_and_maybe_advance(thread_id, &deferred, auto_execute)
            .await
            .expect("defer should advance")
            .expect("defer outcome should exist")
    }

    async fn complete_current_goal_and_advance(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        auto_execute: crate::ThreadGoalPlanAutoExecute,
    ) -> ThreadGoalPlanAdvanceOutcome {
        let current = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("current goal should read")
            .expect("current goal should exist");
        let completed = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(current.goal_id.clone()),
                },
            )
            .await
            .expect("goal should update")
            .expect("goal should exist");
        runtime
            .thread_goals()
            .complete_goal_plan_node_and_maybe_advance(thread_id, &completed, auto_execute)
            .await
            .expect("complete should advance")
            .expect("complete outcome should exist")
    }

    fn node_statuses(
        outcome: &ThreadGoalPlanAdvanceOutcome,
    ) -> Vec<crate::ThreadGoalPlanNodeStatus> {
        outcome
            .snapshot
            .nodes
            .iter()
            .map(|node| node.status)
            .collect::<Vec<_>>()
    }

    #[tokio::test]
    async fn resume_deferred_goal_plan_node_reactivates_after_independent_node_completes() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![
                    goal_node("preservation", "Preserve prior work.", &[]),
                    goal_node("consolidate", "Consolidate independent results.", &[]),
                    goal_node("downstream", "Finish downstream work.", &["preservation"]),
                ],
            })
            .await
            .expect("goal plan should be created");

        let preservation = created
            .activated_goal
            .expect("first ready node should activate");
        assert_eq!("Preserve prior work.", preservation.objective);

        // Defer preservation; only the independent consolidate node is ready.
        let advanced = defer_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(
            "Consolidate independent results.",
            advanced
                .activated_goal
                .expect("independent node should activate")
                .objective
        );

        // Complete consolidate; downstream still depends on the deferred node so
        // nothing new activates and the plan stalls.
        let stalled = complete_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(None, stalled.activated_goal);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Deferred,
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            node_statuses(&stalled)
        );

        // Resume the deferred node with no explicit id; it is auto-selected.
        let resumed = runtime
            .thread_goals()
            .resume_deferred_goal_plan_node(thread_id, None)
            .await
            .expect("resume should succeed")
            .expect("deferred node should resume");
        assert_eq!(
            "Preserve prior work.",
            resumed
                .activated_goal
                .expect("deferred node should reactivate")
                .objective
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Active,
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            node_statuses(&resumed)
        );
        let current = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("goal should read")
            .expect("goal should exist");
        assert_eq!(crate::ThreadGoalStatus::Active, current.status);
        assert_eq!("Preserve prior work.", current.objective);

        // Completing the resumed node satisfies the downstream dependency.
        let after = complete_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(
            "Finish downstream work.",
            after
                .activated_goal
                .expect("downstream node should activate")
                .objective
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            node_statuses(&after)
        );
    }

    #[tokio::test]
    async fn resume_deferred_goal_plan_node_selects_the_requested_node() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![
                    goal_node("first", "Work the first goal.", &[]),
                    goal_node("second", "Work the second goal.", &[]),
                ],
            })
            .await
            .expect("goal plan should be created");
        let second_node_id = created.snapshot.nodes[1].node_id.clone();
        assert_eq!(
            "Work the first goal.",
            created
                .activated_goal
                .expect("first node should activate")
                .objective
        );

        // Defer both nodes so the plan stalls with two resumable deferred nodes.
        let advanced = defer_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(
            "Work the second goal.",
            advanced
                .activated_goal
                .expect("second node should activate")
                .objective
        );
        let stalled = defer_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(None, stalled.activated_goal);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Deferred,
                crate::ThreadGoalPlanNodeStatus::Deferred,
            ],
            node_statuses(&stalled)
        );

        // Explicitly resume the second deferred node; the first stays deferred.
        let resumed = runtime
            .thread_goals()
            .resume_deferred_goal_plan_node(thread_id, Some(second_node_id.as_str()))
            .await
            .expect("resume should succeed")
            .expect("requested node should resume");
        assert_eq!(
            "Work the second goal.",
            resumed
                .activated_goal
                .expect("requested node should reactivate")
                .objective
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Deferred,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            node_statuses(&resumed)
        );
    }

    #[tokio::test]
    async fn resume_deferred_goal_plan_node_returns_none_without_deferred_node() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![goal_node("only", "Do the only goal.", &[])],
            })
            .await
            .expect("goal plan should be created");
        let active_node_id = created.snapshot.nodes[0].node_id.clone();
        assert!(created.activated_goal.is_some());

        // No deferred node exists, so an auto-selected resume is a no-op.
        assert!(
            runtime
                .thread_goals()
                .resume_deferred_goal_plan_node(thread_id, None)
                .await
                .expect("resume call should succeed")
                .is_none()
        );
        // An explicit request for a non-deferred node is also a no-op.
        assert!(
            runtime
                .thread_goals()
                .resume_deferred_goal_plan_node(thread_id, Some(active_node_id.as_str()))
                .await
                .expect("resume call should succeed")
                .is_none()
        );
    }

    #[tokio::test]
    async fn activate_thread_goal_plan_node_can_reactivate_deferred_node() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes: vec![
                    goal_node("preservation", "Preserve prior work.", &[]),
                    goal_node("consolidate", "Consolidate independent results.", &[]),
                    goal_node("downstream", "Finish downstream work.", &["preservation"]),
                ],
            })
            .await
            .expect("goal plan should be created");
        assert!(created.activated_goal.is_some());

        defer_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        let stalled = complete_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        let deferred_node_id = stalled.snapshot.nodes[0].node_id.clone();

        // Explicit node activation now revives the deferred node directly.
        let outcome = runtime
            .thread_goals()
            .activate_thread_goal_plan_node(thread_id, deferred_node_id.as_str())
            .await
            .expect("activation should succeed")
            .expect("deferred node should be activatable");
        assert_eq!(
            "Preserve prior work.",
            outcome
                .activated_goal
                .expect("deferred node should activate")
                .objective
        );
        assert_eq!(
            crate::ThreadGoalPlanNodeStatus::Active,
            outcome.snapshot.nodes[0].status
        );
    }

    #[tokio::test]
    async fn max_node_goal_plan_continues_after_deferred_resume() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        // A full 12-node plan: an independently deferrable root, one independent
        // node, and a ten-node chain that all ultimately depends on the root.
        let mut nodes = vec![
            goal_node("root", "Root node work.", &[]),
            goal_node("independent", "Independent node work.", &[]),
        ];
        let mut previous = String::from("root");
        for index in 2..12 {
            let key = format!("chain_{index}");
            nodes.push(ThreadGoalPlanNodeCreateParams {
                key: key.clone(),
                objective: format!("Chain node {index} work."),
                assigned_thread_id: None,
                title: None,
                priority: 0,
                token_budget: None,
                depends_on: vec![previous.clone()],
            });
            previous = key;
        }
        assert_eq!(12, nodes.len());

        let created = runtime
            .thread_goals()
            .create_thread_goal_plan(ThreadGoalPlanCreateParams {
                thread_id,
                auto_execute: crate::ThreadGoalPlanAutoExecute::AiDirected,
                max_tokens: None,
                nodes,
            })
            .await
            .expect("goal plan should be created");
        assert_eq!(
            "Root node work.",
            created
                .activated_goal
                .expect("root node should activate")
                .objective
        );

        // Defer the root, complete the independent node, then stall.
        let advanced = defer_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(
            "Independent node work.",
            advanced
                .activated_goal
                .expect("independent node should activate")
                .objective
        );
        let stalled = complete_current_goal_and_advance(
            runtime.as_ref(),
            thread_id,
            crate::ThreadGoalPlanAutoExecute::AiDirected,
        )
        .await;
        assert_eq!(None, stalled.activated_goal);

        // Resume the deferred root without appending nodes to the maxed-out plan.
        let resumed = runtime
            .thread_goals()
            .resume_deferred_goal_plan_node(thread_id, None)
            .await
            .expect("resume should succeed")
            .expect("root node should resume");
        assert_eq!(
            "Root node work.",
            resumed
                .activated_goal
                .expect("root node should reactivate")
                .objective
        );

        // Drain the remaining chain; each completion activates the next node.
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(guard < 32, "plan should drain without looping forever");
            let outcome = complete_current_goal_and_advance(
                runtime.as_ref(),
                thread_id,
                crate::ThreadGoalPlanAutoExecute::AiDirected,
            )
            .await;
            if outcome.activated_goal.is_none() {
                assert_eq!(
                    crate::ThreadGoalPlanStatus::Complete,
                    outcome.snapshot.plan.status
                );
                assert!(
                    outcome
                        .snapshot
                        .nodes
                        .iter()
                        .all(|node| node.status == crate::ThreadGoalPlanNodeStatus::Complete),
                    "every node should be complete once the plan drains"
                );
                break;
            }
        }
    }
}
