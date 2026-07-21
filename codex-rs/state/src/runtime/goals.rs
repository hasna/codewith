use super::goal_plans::recalculate_goal_plan_status_in_tx;
use super::*;
use crate::model::ThreadGoalRow;
use codex_protocol::protocol::normalize_thread_goal_title;
use uuid::Uuid;

#[derive(Clone)]
pub struct GoalStore {
    pub(crate) pool: Arc<SqlitePool>,
}

impl GoalStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

pub struct GoalUpdate {
    pub objective: Option<String>,
    pub title: Option<Option<String>>,
    pub status: Option<crate::ThreadGoalStatus>,
    pub token_budget: Option<Option<i64>>,
    pub expected_goal_id: Option<String>,
}

pub enum GoalAccountingOutcome {
    Unchanged(Option<crate::ThreadGoal>),
    Updated(crate::ThreadGoal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalDeleteOutcome {
    pub deleted: bool,
    pub plan_updates: Vec<crate::ThreadGoalPlanSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalAccountingMode {
    ActiveStatusOnly,
    ActiveOnly,
    ActiveOrComplete,
    ActiveOrStopped,
}

const CONTEXT_TARGET_GOAL: &str = "goal";
const CONTEXT_TARGET_GOAL_PLAN: &str = "goal_plan";

enum ContextLifecycleColumn {
    PostGoal,
    PostGoalPlan,
}

impl GoalStore {
    pub async fn get_thread_goal(
        &self,
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
        .fetch_optional(self.pool.as_ref())
        .await?;

        row.map(|row| thread_goal_from_row(&row)).transpose()
    }

    pub async fn replace_thread_goal(
        &self,
        thread_id: ThreadId,
        objective: &str,
        status: crate::ThreadGoalStatus,
        token_budget: Option<i64>,
    ) -> anyhow::Result<crate::ThreadGoal> {
        self.replace_thread_goal_with_title(
            thread_id,
            objective,
            /*title*/ None,
            status,
            token_budget,
        )
        .await
    }

    pub async fn replace_thread_goal_with_title(
        &self,
        thread_id: ThreadId,
        objective: &str,
        title: Option<&str>,
        status: crate::ThreadGoalStatus,
        token_budget: Option<i64>,
    ) -> anyhow::Result<crate::ThreadGoal> {
        let objective = redact_state_string(objective);
        let title = title.map(redact_state_string);
        let title = normalize_thread_goal_title(title.as_deref()).map_err(anyhow::Error::msg)?;
        let goal_id = Uuid::new_v4().to_string();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let status = status_after_budget_limit(status, /*tokens_used*/ 0, token_budget);
        let mut tx = self.pool.begin().await?;
        let previous_goal_id: Option<String> = sqlx::query_scalar(
            r#"
SELECT goal_id
FROM thread_goals
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(previous_goal_id) = previous_goal_id.as_deref() {
            block_projected_goal_plan_nodes_in_tx(&mut tx, thread_id, previous_goal_id, now_ms)
                .await?;
        }
        let row = sqlx::query(
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
        .bind(goal_id)
        .bind(objective)
        .bind(title)
        .bind(status.as_str())
        .bind(token_budget)
        .bind(now_ms)
        .bind(now_ms)
        .fetch_one(&mut *tx)
        .await?;

        let goal = thread_goal_from_row(&row)?;
        tx.commit().await?;
        Ok(goal)
    }

    pub async fn insert_thread_goal(
        &self,
        thread_id: ThreadId,
        objective: &str,
        status: crate::ThreadGoalStatus,
        token_budget: Option<i64>,
    ) -> anyhow::Result<Option<crate::ThreadGoal>> {
        self.insert_thread_goal_with_title(
            thread_id,
            objective,
            /*title*/ None,
            status,
            token_budget,
        )
        .await
    }

    pub async fn insert_thread_goal_with_title(
        &self,
        thread_id: ThreadId,
        objective: &str,
        title: Option<&str>,
        status: crate::ThreadGoalStatus,
        token_budget: Option<i64>,
    ) -> anyhow::Result<Option<crate::ThreadGoal>> {
        let objective = redact_state_string(objective);
        let title = title.map(redact_state_string);
        let title = normalize_thread_goal_title(title.as_deref()).map_err(anyhow::Error::msg)?;
        let goal_id = Uuid::new_v4().to_string();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let status = status_after_budget_limit(status, /*tokens_used*/ 0, token_budget);
        let row = sqlx::query(
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
ON CONFLICT(thread_id) DO NOTHING
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
        .bind(goal_id)
        .bind(objective)
        .bind(title)
        .bind(status.as_str())
        .bind(token_budget)
        .bind(now_ms)
        .bind(now_ms)
        .fetch_optional(self.pool.as_ref())
        .await?;

        row.map(|row| thread_goal_from_row(&row)).transpose()
    }

    pub async fn set_thread_goal_context_action(
        &self,
        thread_id: ThreadId,
        goal_id: &str,
        action: crate::PostGoalContextAction,
    ) -> anyhow::Result<()> {
        self.upsert_context_lifecycle_policy(
            thread_id,
            CONTEXT_TARGET_GOAL,
            goal_id,
            action,
            /*post_goal_plan_action*/ None,
        )
        .await
    }

    pub async fn set_thread_goal_plan_context_actions(
        &self,
        thread_id: ThreadId,
        plan_id: &str,
        post_goal_action: crate::PostGoalContextAction,
        post_goal_plan_action: crate::PostGoalContextAction,
    ) -> anyhow::Result<()> {
        self.upsert_context_lifecycle_policy(
            thread_id,
            CONTEXT_TARGET_GOAL_PLAN,
            plan_id,
            post_goal_action,
            Some(post_goal_plan_action),
        )
        .await
    }

    pub async fn thread_goal_context_action(
        &self,
        thread_id: ThreadId,
        goal_id: &str,
    ) -> anyhow::Result<Option<crate::PostGoalContextAction>> {
        self.context_lifecycle_action(
            thread_id,
            CONTEXT_TARGET_GOAL,
            goal_id,
            ContextLifecycleColumn::PostGoal,
        )
        .await
    }

    pub async fn thread_goal_plan_context_action(
        &self,
        thread_id: ThreadId,
        plan_id: &str,
    ) -> anyhow::Result<Option<crate::PostGoalContextAction>> {
        self.context_lifecycle_action(
            thread_id,
            CONTEXT_TARGET_GOAL_PLAN,
            plan_id,
            ContextLifecycleColumn::PostGoal,
        )
        .await
    }

    pub async fn thread_goal_plan_completion_context_action(
        &self,
        thread_id: ThreadId,
        plan_id: &str,
    ) -> anyhow::Result<Option<crate::PostGoalContextAction>> {
        self.context_lifecycle_action(
            thread_id,
            CONTEXT_TARGET_GOAL_PLAN,
            plan_id,
            ContextLifecycleColumn::PostGoalPlan,
        )
        .await
    }

    async fn upsert_context_lifecycle_policy(
        &self,
        thread_id: ThreadId,
        target_kind: &str,
        target_id: &str,
        post_goal_action: crate::PostGoalContextAction,
        post_goal_plan_action: Option<crate::PostGoalContextAction>,
    ) -> anyhow::Result<()> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        sqlx::query(
            r#"
INSERT INTO thread_goal_context_lifecycle (
    thread_id,
    target_kind,
    target_id,
    post_goal_action,
    post_goal_plan_action,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(thread_id, target_kind, target_id) DO UPDATE SET
    post_goal_action = excluded.post_goal_action,
    post_goal_plan_action = excluded.post_goal_plan_action,
    updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(thread_id.to_string())
        .bind(target_kind)
        .bind(target_id)
        .bind(post_goal_action.as_str())
        .bind(post_goal_plan_action.map(crate::PostGoalContextAction::as_str))
        .bind(now_ms)
        .bind(now_ms)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    async fn context_lifecycle_action(
        &self,
        thread_id: ThreadId,
        target_kind: &str,
        target_id: &str,
        column: ContextLifecycleColumn,
    ) -> anyhow::Result<Option<crate::PostGoalContextAction>> {
        let action: Option<String> = match column {
            ContextLifecycleColumn::PostGoal => {
                sqlx::query_scalar(
                    r#"
SELECT post_goal_action
FROM thread_goal_context_lifecycle
WHERE thread_id = ?
  AND target_kind = ?
  AND target_id = ?
                    "#,
                )
                .bind(thread_id.to_string())
                .bind(target_kind)
                .bind(target_id)
                .fetch_optional(self.pool.as_ref())
                .await?
            }
            ContextLifecycleColumn::PostGoalPlan => sqlx::query_scalar(
                r#"
SELECT post_goal_plan_action
FROM thread_goal_context_lifecycle
WHERE thread_id = ?
  AND target_kind = ?
  AND target_id = ?
                    "#,
            )
            .bind(thread_id.to_string())
            .bind(target_kind)
            .bind(target_id)
            .fetch_optional(self.pool.as_ref())
            .await?
            .flatten(),
        };
        action
            .as_deref()
            .map(crate::PostGoalContextAction::try_from)
            .transpose()
    }

    pub async fn update_thread_goal(
        &self,
        thread_id: ThreadId,
        update: GoalUpdate,
    ) -> anyhow::Result<Option<crate::ThreadGoal>> {
        let GoalUpdate {
            objective,
            title,
            status,
            token_budget,
            expected_goal_id,
        } = update;
        let objective = objective.as_deref().map(redact_state_string);
        let update_title = title.is_some();
        let title = title
            .as_ref()
            .and_then(|title| title.as_deref())
            .map(redact_state_string);
        let title = normalize_thread_goal_title(title.as_deref()).map_err(anyhow::Error::msg)?;
        let title = title.as_deref();
        let expected_goal_id = expected_goal_id.as_deref();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let result = match (status, token_budget) {
            (Some(status), Some(token_budget)) => {
                sqlx::query(
                    r#"
UPDATE thread_goals
SET
    objective = COALESCE(?, objective),
    title = CASE WHEN ? THEN ? ELSE title END,
    status = CASE
        WHEN status IN (?, ?) AND ? != status THEN status
        WHEN status = ? AND ? IN (?, ?) THEN status
        WHEN ? = 'active' AND ? IS NOT NULL AND tokens_used >= ? THEN ?
        ELSE ?
    END,
    token_budget = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND (? IS NULL OR goal_id = ?)
            "#,
                )
                .bind(objective.as_deref())
                .bind(update_title)
                .bind(title)
                .bind(crate::ThreadGoalStatus::Complete.as_str())
                .bind(crate::ThreadGoalStatus::Cancelled.as_str())
                .bind(status.as_str())
                .bind(crate::ThreadGoalStatus::BudgetLimited.as_str())
                .bind(status.as_str())
                .bind(crate::ThreadGoalStatus::Paused.as_str())
                .bind(crate::ThreadGoalStatus::Blocked.as_str())
                .bind(status.as_str())
                .bind(token_budget)
                .bind(token_budget)
                .bind(crate::ThreadGoalStatus::BudgetLimited.as_str())
                .bind(status.as_str())
                .bind(token_budget)
                .bind(now_ms)
                .bind(thread_id.to_string())
                .bind(expected_goal_id)
                .bind(expected_goal_id)
                .execute(self.pool.as_ref())
                .await?
            }
            (Some(status), None) => {
                sqlx::query(
                    r#"
UPDATE thread_goals
SET
    objective = COALESCE(?, objective),
    title = CASE WHEN ? THEN ? ELSE title END,
    status = CASE
        WHEN status IN (?, ?) AND ? != status THEN status
        WHEN status = ? AND ? IN (?, ?) THEN status
        WHEN ? = 'active' AND token_budget IS NOT NULL AND tokens_used >= token_budget THEN ?
        ELSE ?
    END,
    updated_at_ms = ?
WHERE thread_id = ?
  AND (? IS NULL OR goal_id = ?)
            "#,
                )
                .bind(objective.as_deref())
                .bind(update_title)
                .bind(title)
                .bind(crate::ThreadGoalStatus::Complete.as_str())
                .bind(crate::ThreadGoalStatus::Cancelled.as_str())
                .bind(status.as_str())
                .bind(crate::ThreadGoalStatus::BudgetLimited.as_str())
                .bind(status.as_str())
                .bind(crate::ThreadGoalStatus::Paused.as_str())
                .bind(crate::ThreadGoalStatus::Blocked.as_str())
                .bind(status.as_str())
                .bind(crate::ThreadGoalStatus::BudgetLimited.as_str())
                .bind(status.as_str())
                .bind(now_ms)
                .bind(thread_id.to_string())
                .bind(expected_goal_id)
                .bind(expected_goal_id)
                .execute(self.pool.as_ref())
                .await?
            }
            (None, Some(token_budget)) => {
                sqlx::query(
                    r#"
UPDATE thread_goals
SET
    objective = COALESCE(?, objective),
    title = CASE WHEN ? THEN ? ELSE title END,
    token_budget = ?,
    status = CASE
        WHEN status = 'active' AND ? IS NOT NULL AND tokens_used >= ? THEN ?
        ELSE status
    END,
    updated_at_ms = ?
WHERE thread_id = ?
  AND (? IS NULL OR goal_id = ?)
            "#,
                )
                .bind(objective.as_deref())
                .bind(update_title)
                .bind(title)
                .bind(token_budget)
                .bind(token_budget)
                .bind(token_budget)
                .bind(crate::ThreadGoalStatus::BudgetLimited.as_str())
                .bind(now_ms)
                .bind(thread_id.to_string())
                .bind(expected_goal_id)
                .bind(expected_goal_id)
                .execute(self.pool.as_ref())
                .await?
            }
            (None, None) => {
                if objective.is_some() || update_title {
                    sqlx::query(
                        r#"
UPDATE thread_goals
SET
    objective = COALESCE(?, objective),
    title = CASE WHEN ? THEN ? ELSE title END,
    updated_at_ms = ?
WHERE thread_id = ?
  AND (? IS NULL OR goal_id = ?)
            "#,
                    )
                    .bind(objective.as_deref())
                    .bind(update_title)
                    .bind(title)
                    .bind(now_ms)
                    .bind(thread_id.to_string())
                    .bind(expected_goal_id)
                    .bind(expected_goal_id)
                    .execute(self.pool.as_ref())
                    .await?
                } else {
                    let goal = self.get_thread_goal(thread_id).await?;
                    return Ok(match (goal, expected_goal_id) {
                        (Some(goal), Some(expected_goal_id))
                            if goal.goal_id != expected_goal_id =>
                        {
                            None
                        }
                        (goal, _) => goal,
                    });
                }
            }
        };

        if result.rows_affected() == 0 {
            return Ok(None);
        }

        self.get_thread_goal(thread_id).await
    }

    pub async fn pause_active_thread_goal(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<crate::ThreadGoal>> {
        self.update_active_thread_goal_status(thread_id, crate::ThreadGoalStatus::Paused)
            .await
    }

    pub async fn usage_limit_active_thread_goal(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<crate::ThreadGoal>> {
        self.update_active_thread_goal_status(thread_id, crate::ThreadGoalStatus::UsageLimited)
            .await
    }

    async fn update_active_thread_goal_status(
        &self,
        thread_id: ThreadId,
        status: crate::ThreadGoalStatus,
    ) -> anyhow::Result<Option<crate::ThreadGoal>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let result = sqlx::query(
            r#"
UPDATE thread_goals
SET
    status = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND (
      status = 'active'
      OR (
          ? = 'usage_limited'
          AND status = 'budget_limited'
      )
  )
            "#,
        )
        .bind(status.as_str())
        .bind(now_ms)
        .bind(thread_id.to_string())
        .bind(status.as_str())
        .execute(self.pool.as_ref())
        .await?;

        if result.rows_affected() == 0 {
            return Ok(None);
        }

        self.get_thread_goal(thread_id).await
    }

    pub async fn delete_thread_goal_with_plan_updates(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<GoalDeleteOutcome> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let previous_goal_id: Option<String> = sqlx::query_scalar(
            r#"
SELECT goal_id
FROM thread_goals
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let plan_updates = if let Some(previous_goal_id) = previous_goal_id.as_deref() {
            block_projected_goal_plan_nodes_in_tx(&mut tx, thread_id, previous_goal_id, now_ms)
                .await?
        } else {
            Vec::new()
        };
        let result = sqlx::query(
            r#"
DELETE FROM thread_goals
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(GoalDeleteOutcome {
            deleted: result.rows_affected() > 0,
            plan_updates,
        })
    }

    pub async fn delete_thread_goal(&self, thread_id: ThreadId) -> anyhow::Result<bool> {
        self.delete_thread_goal_with_plan_updates(thread_id)
            .await
            .map(|outcome| outcome.deleted)
    }

    pub async fn account_thread_goal_usage(
        &self,
        thread_id: ThreadId,
        time_delta_seconds: i64,
        token_delta: i64,
        mode: GoalAccountingMode,
        expected_goal_id: Option<&str>,
    ) -> anyhow::Result<GoalAccountingOutcome> {
        let time_delta_seconds = time_delta_seconds.max(0);
        let token_delta = token_delta.max(0);
        if time_delta_seconds == 0 && token_delta == 0 {
            return Ok(GoalAccountingOutcome::Unchanged(
                self.get_thread_goal(thread_id).await?,
            ));
        }

        let now_ms = datetime_to_epoch_millis(Utc::now());
        let active_or_stopped_status_filter = "status IN ('active', 'paused', 'blocked', 'usage_limited', 'budget_limited', 'deferred')";
        let status_filter = match mode {
            GoalAccountingMode::ActiveStatusOnly => "status = 'active'",
            GoalAccountingMode::ActiveOnly => "status IN ('active', 'budget_limited')",
            GoalAccountingMode::ActiveOrComplete => {
                "status IN ('active', 'budget_limited', 'complete')"
            }
            GoalAccountingMode::ActiveOrStopped => active_or_stopped_status_filter,
        };
        let budget_limit_status_filter = match mode {
            GoalAccountingMode::ActiveStatusOnly
            | GoalAccountingMode::ActiveOnly
            | GoalAccountingMode::ActiveOrComplete => "status = 'active'",
            GoalAccountingMode::ActiveOrStopped => active_or_stopped_status_filter,
        };
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
UPDATE thread_goals
SET
    time_used_seconds = time_used_seconds +
            "#,
        );
        builder.push_bind(time_delta_seconds);
        builder.push(
            r#",
    tokens_used = tokens_used +
            "#,
        );
        builder.push_bind(token_delta);
        builder.push(
            r#",
    status = CASE
        WHEN
            "#,
        );
        builder.push(budget_limit_status_filter);
        builder.push(
            r#"
            AND token_budget IS NOT NULL
            AND tokens_used +
            "#,
        );
        builder.push_bind(token_delta);
        builder.push(
            r#"
                >= token_budget
            THEN
            "#,
        );
        builder.push_bind(crate::ThreadGoalStatus::BudgetLimited.as_str());
        builder.push(
            r#"
        ELSE status
    END,
    updated_at_ms =
            "#,
        );
        builder.push_bind(now_ms);
        builder.push(
            r#"
WHERE thread_id =
            "#,
        );
        builder.push_bind(thread_id.to_string());
        builder.push(" AND ");
        builder.push(status_filter);
        if let Some(expected_goal_id) = expected_goal_id {
            builder.push(" AND goal_id = ").push_bind(expected_goal_id);
        }
        builder.push(
            r#"
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
        );

        let row = builder.build().fetch_optional(self.pool.as_ref()).await?;

        let Some(row) = row else {
            return Ok(GoalAccountingOutcome::Unchanged(
                self.get_thread_goal(thread_id).await?,
            ));
        };

        let updated = thread_goal_from_row(&row)?;
        Ok(GoalAccountingOutcome::Updated(updated))
    }
}

fn thread_goal_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<crate::ThreadGoal> {
    ThreadGoalRow::try_from_row(row).and_then(crate::ThreadGoal::try_from)
}

async fn block_projected_goal_plan_nodes_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    projected_goal_id: &str,
    now_ms: i64,
) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
    let rows = sqlx::query(
        r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE assigned_thread_id = ?
  AND projected_goal_id = ?
  AND status IN ('active', 'paused', 'blocked', 'usage_limited')
	RETURNING plan_id
	        "#,
    )
    .bind(crate::ThreadGoalPlanNodeStatus::Blocked.as_str())
    .bind(now_ms)
    .bind(thread_id.to_string())
    .bind(projected_goal_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut plan_ids = Vec::new();
    for row in rows {
        let plan_id: String = row.try_get("plan_id")?;
        if !plan_ids.contains(&plan_id) {
            plan_ids.push(plan_id);
        }
    }
    let mut snapshots = Vec::with_capacity(plan_ids.len());
    for plan_id in plan_ids {
        recalculate_goal_plan_status_in_tx(tx, &plan_id, now_ms).await?;
        snapshots.push(super::goal_plans::snapshot_thread_goal_plan_in_tx(tx, &plan_id).await?);
    }
    Ok(snapshots)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;

    async fn test_runtime() -> std::sync::Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000123").expect("valid thread id")
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
    async fn goal_context_lifecycle_policy_round_trips() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        let other_thread_id =
            ThreadId::from_string("22222222-2222-4222-8222-222222222222").expect("valid thread id");
        upsert_test_thread(&runtime, thread_id).await;
        upsert_test_thread(&runtime, other_thread_id).await;

        runtime
            .thread_goals()
            .set_thread_goal_context_action(
                thread_id,
                "goal-1",
                crate::PostGoalContextAction::Compact,
            )
            .await
            .expect("goal policy should persist");
        runtime
            .thread_goals()
            .set_thread_goal_plan_context_actions(
                thread_id,
                "plan-1",
                crate::PostGoalContextAction::Keep,
                crate::PostGoalContextAction::Compact,
            )
            .await
            .expect("plan policy should persist");
        runtime
            .thread_goals()
            .set_thread_goal_context_action(
                other_thread_id,
                "goal-1",
                crate::PostGoalContextAction::Keep,
            )
            .await
            .expect("same goal id in another thread should persist independently");

        assert_eq!(
            Some(crate::PostGoalContextAction::Compact),
            runtime
                .thread_goals()
                .thread_goal_context_action(thread_id, "goal-1")
                .await
                .expect("goal policy should load")
        );
        assert_eq!(
            Some(crate::PostGoalContextAction::Keep),
            runtime
                .thread_goals()
                .thread_goal_context_action(other_thread_id, "goal-1")
                .await
                .expect("other thread goal policy should load")
        );
        assert_eq!(
            Some(crate::PostGoalContextAction::Keep),
            runtime
                .thread_goals()
                .thread_goal_plan_context_action(thread_id, "plan-1")
                .await
                .expect("plan post-goal policy should load")
        );
        assert_eq!(
            Some(crate::PostGoalContextAction::Compact),
            runtime
                .thread_goals()
                .thread_goal_plan_completion_context_action(thread_id, "plan-1")
                .await
                .expect("plan completion policy should load")
        );
    }

    #[tokio::test]
    async fn replace_update_and_get_thread_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "optimize the benchmark",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100_000),
            )
            .await
            .expect("goal replacement should succeed");
        assert_eq!(
            Some(goal.clone()),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .unwrap()
        );
        let metadata = runtime
            .get_thread(thread_id)
            .await
            .expect("thread metadata should load")
            .expect("thread should exist");
        assert_eq!(metadata.preview.as_deref(), Some("hello"));

        let updated = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: Some(Some(200_000)),
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");
        let expected = crate::ThreadGoal {
            status: crate::ThreadGoalStatus::Paused,
            token_budget: Some(200_000),
            updated_at: updated.updated_at,
            ..goal.clone()
        };
        assert_eq!(expected, updated);

        let replaced = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "ship the new result",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("goal replacement should succeed");
        assert_eq!("ship the new result", replaced.objective);
        assert_eq!(crate::ThreadGoalStatus::Active, replaced.status);
        assert_eq!(None, replaced.token_budget);
        assert_eq!(0, replaced.tokens_used);
        assert_eq!(0, replaced.time_used_seconds);

        assert!(
            runtime
                .thread_goals()
                .delete_thread_goal(thread_id)
                .await
                .unwrap()
        );
        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .unwrap()
        );
        assert!(
            !runtime
                .thread_goals()
                .delete_thread_goal(thread_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn replace_thread_goal_applies_budget_limit_immediately() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let replaced = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(0),
            )
            .await
            .expect("goal replacement should succeed");

        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, replaced.status);
        assert_eq!(Some(0), replaced.token_budget);
        assert_eq!(0, replaced.tokens_used);
        assert_eq!(0, replaced.time_used_seconds);
    }

    #[tokio::test]
    async fn thread_goal_title_can_be_set_preserved_and_cleared() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let created = runtime
            .thread_goals()
            .replace_thread_goal_with_title(
                thread_id,
                "ship the status line title work",
                Some("Ship statusline titles"),
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("goal replacement should succeed");
        assert_eq!(Some("Ship statusline titles".to_string()), created.title);

        let preserved = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: None,
                    expected_goal_id: Some(created.goal_id.clone()),
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");
        assert_eq!(Some("Ship statusline titles".to_string()), preserved.title);

        let renamed = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: Some(Some("Polish goal title".to_string())),
                    status: None,
                    token_budget: None,
                    expected_goal_id: Some(created.goal_id.clone()),
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");
        assert_eq!(Some("Polish goal title".to_string()), renamed.title);

        let cleared = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: Some(None),
                    status: None,
                    token_budget: None,
                    expected_goal_id: Some(created.goal_id),
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");
        assert_eq!(None, cleared.title);
    }

    #[tokio::test]
    async fn insert_thread_goal_does_not_replace_existing_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let inserted = runtime
            .thread_goals()
            .insert_thread_goal(
                thread_id,
                "optimize the benchmark",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100_000),
            )
            .await
            .expect("goal insertion should succeed")
            .expect("goal should be inserted");

        let duplicate = runtime
            .thread_goals()
            .insert_thread_goal(
                thread_id,
                "replace the benchmark",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(200_000),
            )
            .await
            .expect("duplicate insert should not fail");

        assert_eq!(None, duplicate);
        assert_eq!(
            Some(inserted),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn insert_thread_goal_applies_budget_limit_immediately() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let inserted = runtime
            .thread_goals()
            .insert_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(0),
            )
            .await
            .expect("goal insertion should succeed")
            .expect("goal should be inserted");

        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, inserted.status);
        assert_eq!(Some(0), inserted.token_budget);
        assert_eq!(0, inserted.tokens_used);
        assert_eq!(0, inserted.time_used_seconds);
    }

    #[tokio::test]
    async fn update_thread_goal_ignores_replaced_goal_version() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let original = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "old objective",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100),
            )
            .await
            .expect("goal replacement should succeed");
        let replacement = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "new objective",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(10),
            )
            .await
            .expect("goal replacement should succeed");

        let stale_update = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(original.goal_id),
                },
            )
            .await
            .expect("goal update should succeed");

        assert_eq!(None, stale_update);
        assert_eq!(
            Some(replacement.clone()),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("goal read should succeed")
        );

        let fresh_update = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: Some(replacement.goal_id),
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("fresh update should match the replacement goal");
        assert_eq!(crate::ThreadGoalStatus::Complete, fresh_update.status);
    }

    #[tokio::test]
    async fn usage_accounting_ignores_replaced_goal_version() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        let original = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "old objective",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100),
            )
            .await
            .expect("goal replacement should succeed");
        let replacement = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "new objective",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(10),
            )
            .await
            .expect("goal replacement should succeed");

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 5,
                /*token_delta*/ 5,
                GoalAccountingMode::ActiveOnly,
                Some(original.goal_id.as_str()),
            )
            .await
            .expect("usage accounting should succeed");

        let GoalAccountingOutcome::Unchanged(Some(goal)) = outcome else {
            panic!("stale goal version should not be updated");
        };
        assert_ne!(replacement.goal_id, original.goal_id);
        assert_eq!(replacement.created_at, goal.created_at);
        assert_eq!("new objective", goal.objective);
        assert_eq!(0, goal.tokens_used);
        assert_eq!(0, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn update_thread_goal_objective_preserves_usage_and_created_at() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;

        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "draft the report",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100),
            )
            .await
            .expect("goal replacement should succeed");
        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 12,
                /*token_delta*/ 30,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(accounted) = outcome else {
            panic!("active goal should account usage");
        };

        let updated = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: Some("draft the report clearly".to_string()),
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: Some(Some(200)),
                    expected_goal_id: Some(accounted.goal_id.clone()),
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");
        let expected = crate::ThreadGoal {
            objective: "draft the report clearly".to_string(),
            status: crate::ThreadGoalStatus::Paused,
            token_budget: Some(200),
            updated_at: updated.updated_at,
            ..accounted
        };
        assert_eq!(expected, updated);
    }

    #[tokio::test]
    async fn concurrent_partial_updates_preserve_independent_fields() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "optimize the benchmark",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100_000),
            )
            .await
            .expect("goal replacement should succeed");

        let status_update = runtime.thread_goals().update_thread_goal(
            thread_id,
            GoalUpdate {
                objective: None,
                title: None,
                status: Some(crate::ThreadGoalStatus::Paused),
                token_budget: None,
                expected_goal_id: None,
            },
        );
        let budget_update = runtime.thread_goals().update_thread_goal(
            thread_id,
            GoalUpdate {
                objective: None,
                title: None,
                status: None,
                token_budget: Some(Some(200_000)),
                expected_goal_id: None,
            },
        );
        let (status_update, budget_update) = tokio::join!(status_update, budget_update);
        status_update.expect("status update should succeed");
        budget_update.expect("budget update should succeed");

        let goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("goal read should succeed")
            .expect("goal should exist");
        assert_eq!(crate::ThreadGoalStatus::Paused, goal.status);
        assert_eq!(Some(200_000), goal.token_budget);
    }

    #[tokio::test]
    async fn pause_active_thread_goal_does_not_clobber_terminal_status() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "optimize the benchmark",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100_000),
            )
            .await
            .expect("goal replacement should succeed");

        let paused = runtime
            .thread_goals()
            .pause_active_thread_goal(thread_id)
            .await
            .expect("active pause should succeed")
            .expect("active goal should be paused");
        let expected = crate::ThreadGoal {
            status: crate::ThreadGoalStatus::Paused,
            updated_at: paused.updated_at,
            ..goal
        };
        assert_eq!(expected, paused);

        let complete = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Complete),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");
        let pause_result = runtime
            .thread_goals()
            .pause_active_thread_goal(thread_id)
            .await
            .expect("terminal pause attempt should succeed");
        assert_eq!(None, pause_result);
        assert_eq!(
            Some(complete),
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("goal read should succeed")
        );
    }

    #[tokio::test]
    async fn usage_limit_active_thread_goal_updates_active_or_budget_limited_goals() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "optimize the benchmark",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("goal replacement should succeed");

        let usage_limited = runtime
            .thread_goals()
            .usage_limit_active_thread_goal(thread_id)
            .await
            .expect("usage limiting should succeed")
            .expect("active goal should become usage limited");
        let expected = crate::ThreadGoal {
            status: crate::ThreadGoalStatus::UsageLimited,
            updated_at: usage_limited.updated_at,
            ..goal
        };
        assert_eq!(expected, usage_limited);

        let second_update = runtime
            .thread_goals()
            .usage_limit_active_thread_goal(thread_id)
            .await
            .expect("repeated usage limiting should succeed");
        assert_eq!(None, second_update);

        let budget_limited = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "keep the usage failure visible",
                crate::ThreadGoalStatus::BudgetLimited,
                /*token_budget*/ Some(1),
            )
            .await
            .expect("goal replacement should succeed");
        let usage_limited = runtime
            .thread_goals()
            .usage_limit_active_thread_goal(thread_id)
            .await
            .expect("usage limiting should succeed")
            .expect("budget-limited goal should become usage limited");
        let expected = crate::ThreadGoal {
            status: crate::ThreadGoalStatus::UsageLimited,
            updated_at: usage_limited.updated_at,
            ..budget_limited
        };
        assert_eq!(expected, usage_limited);
    }

    #[tokio::test]
    async fn usage_accounting_updates_active_goals_and_accounts_budget_limited_in_flight_usage() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(20),
            )
            .await
            .expect("goal replacement should succeed");

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 7,
                /*token_delta*/ 5,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(goal) = outcome else {
            panic!("active goal should be updated");
        };
        assert_eq!(crate::ThreadGoalStatus::Active, goal.status);
        assert_eq!(5, goal.tokens_used);
        assert_eq!(7, goal.time_used_seconds);

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 3,
                /*token_delta*/ 15,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(goal) = outcome else {
            panic!("budget crossing should update the goal");
        };
        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, goal.status);
        assert_eq!(20, goal.tokens_used);
        assert_eq!(10, goal.time_used_seconds);

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 5,
                /*token_delta*/ 5,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(goal) = outcome else {
            panic!("budget-limited goal should still account in-flight active usage");
        };
        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, goal.status);
        assert_eq!(25, goal.tokens_used);
        assert_eq!(15, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn active_status_only_usage_accounting_does_not_update_budget_limited_goals() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay stopped",
                crate::ThreadGoalStatus::BudgetLimited,
                /*token_budget*/ Some(20),
            )
            .await
            .expect("goal replacement should succeed");

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 5,
                /*token_delta*/ 5,
                GoalAccountingMode::ActiveStatusOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Unchanged(Some(goal)) = outcome else {
            panic!("budget-limited goal should not be updated");
        };
        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, goal.status);
        assert_eq!(0, goal.tokens_used);
        assert_eq!(0, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn stopped_usage_accounting_promotes_paused_goal_over_budget() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stop before overrun",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(20),
            )
            .await
            .expect("goal replacement should succeed");
        runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                crate::GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed");

        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 3,
                /*token_delta*/ 25,
                GoalAccountingMode::ActiveOrStopped,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(goal) = outcome else {
            panic!("stopped goal should account final usage");
        };
        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, goal.status);
        assert_eq!(25, goal.tokens_used);
        assert_eq!(3, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn budget_updates_immediately_stop_active_goals_already_over_budget() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(100),
            )
            .await
            .expect("goal replacement should succeed");
        runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 1,
                /*token_delta*/ 50,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");

        let lowered = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: None,
                    token_budget: Some(Some(40)),
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");

        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, lowered.status);
        assert_eq!(Some(40), lowered.token_budget);
        assert_eq!(50, lowered.tokens_used);
    }

    #[tokio::test]
    async fn activating_goal_already_over_budget_keeps_it_budget_limited() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(40),
            )
            .await
            .expect("goal replacement should succeed");
        runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 1,
                /*token_delta*/ 50,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");

        let reactivated = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: Some("stay within budget, with clearer wording".to_string()),
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Active),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");

        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, reactivated.status);
        assert_eq!(
            "stay within budget, with clearer wording",
            reactivated.objective
        );
        assert_eq!(Some(40), reactivated.token_budget);
        assert_eq!(50, reactivated.tokens_used);
    }

    #[tokio::test]
    async fn pausing_budget_limited_goal_preserves_terminal_status() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(40),
            )
            .await
            .expect("goal replacement should succeed");
        runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 1,
                /*token_delta*/ 50,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");

        let paused = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");

        assert_eq!(crate::ThreadGoalStatus::BudgetLimited, paused.status);
        assert_eq!(Some(40), paused.token_budget);
        assert_eq!(50, paused.tokens_used);
    }

    #[tokio::test]
    async fn blocking_budget_limited_goal_preserves_terminal_status() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "stay within budget",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(40),
            )
            .await
            .expect("goal replacement should succeed");
        let outcome = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 1,
                /*token_delta*/ 50,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(budget_limited) = outcome else {
            panic!("budget crossing should update the goal");
        };

        let blocked = runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Blocked),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");

        let expected = crate::ThreadGoal {
            updated_at: blocked.updated_at,
            ..budget_limited
        };
        assert_eq!(expected, blocked);
    }

    #[tokio::test]
    async fn usage_accounting_can_finalize_completed_goal_for_completing_turn() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "finish the report",
                crate::ThreadGoalStatus::Complete,
                /*token_budget*/ Some(1_000),
            )
            .await
            .expect("goal replacement should succeed");

        let active_only = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 30,
                /*token_delta*/ 200,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Unchanged(Some(goal)) = active_only else {
            panic!("completed goal should not be updated by active-only accounting");
        };
        assert_eq!(crate::ThreadGoalStatus::Complete, goal.status);
        assert_eq!(0, goal.tokens_used);
        assert_eq!(0, goal.time_used_seconds);

        let completing_turn = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 30,
                /*token_delta*/ 200,
                GoalAccountingMode::ActiveOrComplete,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(goal) = completing_turn else {
            panic!("completed goal should be updated for final accounting");
        };
        assert_eq!(crate::ThreadGoalStatus::Complete, goal.status);
        assert_eq!(200, goal.tokens_used);
        assert_eq!(30, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn usage_accounting_can_finalize_stopped_goal_for_in_flight_turn() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "finish the report",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(1_000),
            )
            .await
            .expect("goal replacement should succeed");
        runtime
            .thread_goals()
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    title: None,
                    status: Some(crate::ThreadGoalStatus::Paused),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await
            .expect("goal update should succeed")
            .expect("goal should exist");

        let active_only = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 30,
                /*token_delta*/ 200,
                GoalAccountingMode::ActiveOnly,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Unchanged(Some(goal)) = active_only else {
            panic!("paused goal should not be updated by active-only accounting");
        };
        assert_eq!(crate::ThreadGoalStatus::Paused, goal.status);
        assert_eq!(0, goal.tokens_used);
        assert_eq!(0, goal.time_used_seconds);

        let in_flight_turn = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 30,
                /*token_delta*/ 200,
                GoalAccountingMode::ActiveOrStopped,
                /*expected_goal_id*/ None,
            )
            .await
            .expect("usage accounting should succeed");
        let GoalAccountingOutcome::Updated(goal) = in_flight_turn else {
            panic!("stopped goal should be updated for in-flight accounting");
        };
        assert_eq!(crate::ThreadGoalStatus::Paused, goal.status);
        assert_eq!(200, goal.tokens_used);
        assert_eq!(30, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn usage_accounting_adds_concurrent_token_deltas() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "count every token",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ Some(1_000),
            )
            .await
            .expect("goal replacement should succeed");

        let first = runtime.thread_goals().account_thread_goal_usage(
            thread_id,
            /*time_delta_seconds*/ 4,
            /*token_delta*/ 40,
            GoalAccountingMode::ActiveOnly,
            /*expected_goal_id*/ None,
        );
        let second = runtime.thread_goals().account_thread_goal_usage(
            thread_id,
            /*time_delta_seconds*/ 6,
            /*token_delta*/ 60,
            GoalAccountingMode::ActiveOnly,
            /*expected_goal_id*/ None,
        );
        let (first, second) = tokio::join!(first, second);
        first.expect("first usage accounting should succeed");
        second.expect("second usage accounting should succeed");

        let goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await
            .expect("goal read should succeed")
            .expect("goal should exist");
        assert_eq!(100, goal.tokens_used);
        assert_eq!(10, goal.time_used_seconds);
    }

    #[tokio::test]
    async fn deleting_thread_deletes_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "clean up with the thread",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("goal replacement should succeed");

        runtime
            .delete_thread(thread_id)
            .await
            .expect("thread deletion should succeed");

        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("goal read should succeed")
        );
    }
}
