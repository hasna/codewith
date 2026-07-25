use super::*;
use crate::runtime::goal_plans::activate_next_ready_node_in_tx;
use crate::runtime::goal_plans::recalculate_goal_plan_status_in_tx;
use crate::runtime::goal_plans::snapshot_thread_goal_plan_in_tx;
use crate::runtime::goal_plans::thread_goal_from_row;
use crate::runtime::goal_plans::validate_plan_create_params;
use codex_protocol::protocol::normalize_thread_goal_title;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadGoalPlanNodeTitleUpdate {
    Keep,
    Set(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadGoalPlanNodeTokenBudgetUpdate {
    Keep,
    Set(Option<i64>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanNodeUpdateParams {
    pub thread_id: ThreadId,
    pub node_id: String,
    pub key: Option<String>,
    pub objective: Option<String>,
    pub title: ThreadGoalPlanNodeTitleUpdate,
    pub priority: Option<i64>,
    pub token_budget: ThreadGoalPlanNodeTokenBudgetUpdate,
    pub depends_on: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadGoalPlanNodeInsertPosition {
    Before(String),
    After(String),
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanNodeInsertParams {
    pub thread_id: ThreadId,
    pub plan_id: String,
    pub position: ThreadGoalPlanNodeInsertPosition,
    pub key: String,
    pub objective: String,
    pub title: Option<String>,
    pub priority: i64,
    pub token_budget: Option<i64>,
    pub depends_on: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadGoalPlanNodeCompletionStatus {
    Complete,
    Pending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadGoalPlanNodeStatusUpdateParams {
    pub thread_id: ThreadId,
    pub node_id: String,
    pub status: ThreadGoalPlanNodeCompletionStatus,
    pub auto_execute: crate::ThreadGoalPlanAutoExecute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanNodeMutationOutcome {
    pub snapshot: crate::ThreadGoalPlanSnapshot,
    pub node: crate::ThreadGoalPlanNode,
    pub goal: Option<crate::ThreadGoal>,
    pub activated_goal: Option<crate::ThreadGoal>,
    pub cleared_goal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalPlanNodeInsertOutcome {
    pub snapshot: crate::ThreadGoalPlanSnapshot,
    pub inserted_node: crate::ThreadGoalPlanNode,
}

impl GoalStore {
    pub async fn update_thread_goal_plan_node(
        &self,
        params: ThreadGoalPlanNodeUpdateParams,
    ) -> anyhow::Result<ThreadGoalPlanNodeMutationOutcome> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let outcome = update_thread_goal_plan_node_in_tx(&mut tx, params, now_ms).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn insert_thread_goal_plan_node(
        &self,
        params: ThreadGoalPlanNodeInsertParams,
    ) -> anyhow::Result<ThreadGoalPlanNodeInsertOutcome> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let outcome = insert_thread_goal_plan_node_in_tx(&mut tx, params, now_ms).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn set_thread_goal_plan_node_status(
        &self,
        params: ThreadGoalPlanNodeStatusUpdateParams,
    ) -> anyhow::Result<ThreadGoalPlanNodeMutationOutcome> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        let outcome = set_thread_goal_plan_node_status_in_tx(&mut tx, params, now_ms).await?;
        tx.commit().await?;
        Ok(outcome)
    }
}

async fn update_thread_goal_plan_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: ThreadGoalPlanNodeUpdateParams,
    now_ms: i64,
) -> anyhow::Result<ThreadGoalPlanNodeMutationOutcome> {
    let node_id = require_non_empty_id(params.node_id.trim(), "goal plan node id")?;
    let plan_id = plan_id_for_node_in_tx(tx, node_id).await?;
    let snapshot = owned_plan_snapshot_in_tx(tx, params.thread_id, &plan_id).await?;
    if snapshot.plan.status == crate::ThreadGoalPlanStatus::Cancelled {
        anyhow::bail!("cannot edit a node in a cancelled goal plan");
    }
    let node_index = snapshot_node_index(&snapshot, node_id)?;
    let existing_node = snapshot.nodes[node_index].clone();
    if existing_node.status == crate::ThreadGoalPlanNodeStatus::Complete {
        anyhow::bail!("cannot edit a completed goal-plan node; mark it undone first");
    }
    if existing_node.status == crate::ThreadGoalPlanNodeStatus::Cancelled {
        anyhow::bail!("cannot edit a cancelled goal-plan node");
    }

    let mut updated_node = existing_node.clone();
    if let Some(key) = params.key {
        updated_node.key = key.trim().to_string();
    }
    if let Some(objective) = params.objective {
        updated_node.objective = objective.trim().to_string();
    }
    if let ThreadGoalPlanNodeTitleUpdate::Set(title) = params.title {
        updated_node.title =
            normalize_thread_goal_title(title.as_deref()).map_err(anyhow::Error::msg)?;
    }
    if let Some(priority) = params.priority {
        updated_node.priority = priority;
    }
    if let ThreadGoalPlanNodeTokenBudgetUpdate::Set(token_budget) = params.token_budget {
        updated_node.token_budget = token_budget;
    }
    let updating_dependencies = params.depends_on.is_some();
    if let Some(depends_on) = params.depends_on {
        updated_node.depends_on = normalize_dependency_keys(depends_on);
    }

    let validation_nodes =
        validation_nodes_with_replacement(&snapshot, node_index, &existing_node.key, &updated_node);
    validate_plan_create_params(&ThreadGoalPlanCreateParams {
        thread_id: snapshot.plan.thread_id,
        auto_execute: snapshot.plan.auto_execute,
        max_tokens: snapshot.plan.max_tokens,
        nodes: validation_nodes,
    })?;

    sqlx::query(
        r#"
UPDATE thread_goal_plan_nodes
SET
    key = ?,
    objective = ?,
    title = ?,
    priority = ?,
    token_budget = ?,
    updated_at_ms = ?
WHERE node_id = ?
        "#,
    )
    .bind(&updated_node.key)
    .bind(&updated_node.objective)
    .bind(&updated_node.title)
    .bind(updated_node.priority)
    .bind(updated_node.token_budget)
    .bind(now_ms)
    .bind(&updated_node.node_id)
    .execute(&mut **tx)
    .await?;

    if updating_dependencies {
        replace_node_dependencies_in_tx(tx, &updated_node, &snapshot.nodes).await?;
    }

    let goal = update_projected_goal_metadata_in_tx(tx, &updated_node, now_ms).await?;
    recalculate_goal_plan_status_in_tx(tx, &snapshot.plan.plan_id, now_ms).await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, &snapshot.plan.plan_id).await?;
    let node = snapshot_node(&snapshot, node_id)?;
    Ok(ThreadGoalPlanNodeMutationOutcome {
        snapshot,
        node,
        goal,
        activated_goal: None,
        cleared_goal: false,
    })
}

async fn insert_thread_goal_plan_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: ThreadGoalPlanNodeInsertParams,
    now_ms: i64,
) -> anyhow::Result<ThreadGoalPlanNodeInsertOutcome> {
    let plan_id = require_non_empty_id(params.plan_id.trim(), "goal plan id")?;
    let snapshot = owned_plan_snapshot_in_tx(tx, params.thread_id, plan_id).await?;
    if snapshot.plan.status == crate::ThreadGoalPlanStatus::Cancelled {
        anyhow::bail!("cannot insert a node into a cancelled goal plan");
    }
    if snapshot.nodes.len() >= super::goal_plans::MAX_GOAL_PLAN_NODES {
        anyhow::bail!(
            "goal plan contains {} goals but the maximum is {}",
            snapshot.nodes.len(),
            super::goal_plans::MAX_GOAL_PLAN_NODES
        );
    }

    // An inserted node becomes a sibling of the node it is positioned against,
    // so it inherits that node's place in the hierarchy introduced by nested
    // goal plans. Appending at the end always produces a top-level node.
    let (insert_sequence, parent_node_id, nesting_depth) = match &params.position {
        ThreadGoalPlanNodeInsertPosition::Before(reference_node_id) => {
            let reference = reference_node(&snapshot, reference_node_id.trim())?;
            (
                reference.sequence,
                reference.parent_node_id,
                reference.nesting_depth,
            )
        }
        ThreadGoalPlanNodeInsertPosition::After(reference_node_id) => {
            let reference = reference_node(&snapshot, reference_node_id.trim())?;
            (
                reference.sequence + 1,
                reference.parent_node_id,
                reference.nesting_depth,
            )
        }
        ThreadGoalPlanNodeInsertPosition::End => (
            snapshot
                .nodes
                .iter()
                .map(|node| node.sequence)
                .max()
                .unwrap_or(-1)
                + 1,
            None,
            1,
        ),
    };

    let title = normalize_thread_goal_title(params.title.as_deref()).map_err(anyhow::Error::msg)?;
    let inserted_node_id = Uuid::new_v4().to_string();
    let inserted_node = ThreadGoalPlanNodeCreateParams {
        key: params.key.trim().to_string(),
        objective: params.objective.trim().to_string(),
        assigned_thread_id: Some(params.thread_id),
        title,
        priority: params.priority,
        token_budget: params.token_budget,
        depends_on: normalize_dependency_keys(params.depends_on),
    };
    let mut validation_nodes = snapshot_nodes_as_create_params(&snapshot.nodes);
    validation_nodes.push(inserted_node.clone());
    validate_plan_create_params(&ThreadGoalPlanCreateParams {
        thread_id: snapshot.plan.thread_id,
        auto_execute: snapshot.plan.auto_execute,
        max_tokens: snapshot.plan.max_tokens,
        nodes: validation_nodes,
    })?;

    sqlx::query(
        r#"
UPDATE thread_goal_plan_nodes
SET sequence = sequence + 1, updated_at_ms = ?
WHERE plan_id = ?
  AND sequence >= ?
        "#,
    )
    .bind(now_ms)
    .bind(plan_id)
    .bind(insert_sequence)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
INSERT INTO thread_goal_plan_nodes (
    node_id,
    plan_id,
    thread_id,
    assigned_thread_id,
    parent_node_id,
    nesting_depth,
    key,
    sequence,
    priority,
    objective,
    title,
    status,
    token_budget,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&inserted_node_id)
    .bind(plan_id)
    .bind(snapshot.plan.thread_id.to_string())
    .bind(params.thread_id.to_string())
    .bind(parent_node_id.clone())
    .bind(nesting_depth)
    .bind(&inserted_node.key)
    .bind(insert_sequence)
    .bind(inserted_node.priority)
    .bind(&inserted_node.objective)
    .bind(&inserted_node.title)
    .bind(crate::ThreadGoalPlanNodeStatus::Pending.as_str())
    .bind(inserted_node.token_budget)
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;

    let inserted_state_node = crate::ThreadGoalPlanNode {
        node_id: inserted_node_id.clone(),
        plan_id: plan_id.to_string(),
        thread_id: snapshot.plan.thread_id,
        assigned_thread_id: params.thread_id,
        parent_node_id,
        nesting_depth,
        key: inserted_node.key,
        sequence: insert_sequence,
        priority: inserted_node.priority,
        objective: inserted_node.objective,
        title: inserted_node.title,
        status: crate::ThreadGoalPlanNodeStatus::Pending,
        token_budget: inserted_node.token_budget,
        tokens_used: 0,
        time_used_seconds: 0,
        lines_added: 0,
        lines_deleted: 0,
        projected_goal_id: None,
        depends_on: inserted_node.depends_on,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    replace_node_dependencies_in_tx(tx, &inserted_state_node, &snapshot.nodes).await?;
    recalculate_goal_plan_status_in_tx(tx, plan_id, now_ms).await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, plan_id).await?;
    let inserted_node = snapshot_node(&snapshot, &inserted_node_id)?;
    Ok(ThreadGoalPlanNodeInsertOutcome {
        snapshot,
        inserted_node,
    })
}

async fn set_thread_goal_plan_node_status_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: ThreadGoalPlanNodeStatusUpdateParams,
    now_ms: i64,
) -> anyhow::Result<ThreadGoalPlanNodeMutationOutcome> {
    let node_id = require_non_empty_id(params.node_id.trim(), "goal plan node id")?;
    let plan_id = plan_id_for_node_in_tx(tx, node_id).await?;
    let snapshot = owned_plan_snapshot_in_tx(tx, params.thread_id, &plan_id).await?;
    if snapshot.plan.status == crate::ThreadGoalPlanStatus::Cancelled {
        anyhow::bail!("cannot update a node in a cancelled goal plan");
    }
    let node = snapshot_node(&snapshot, node_id)?;
    match params.status {
        ThreadGoalPlanNodeCompletionStatus::Complete => {
            mark_goal_plan_node_complete_in_tx(tx, params, &snapshot, node, now_ms).await
        }
        ThreadGoalPlanNodeCompletionStatus::Pending => {
            mark_goal_plan_node_pending_in_tx(tx, &snapshot, node, now_ms).await
        }
    }
}

async fn mark_goal_plan_node_complete_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: ThreadGoalPlanNodeStatusUpdateParams,
    snapshot: &crate::ThreadGoalPlanSnapshot,
    node: crate::ThreadGoalPlanNode,
    now_ms: i64,
) -> anyhow::Result<ThreadGoalPlanNodeMutationOutcome> {
    if node.status == crate::ThreadGoalPlanNodeStatus::Complete {
        return Ok(ThreadGoalPlanNodeMutationOutcome {
            snapshot: snapshot.clone(),
            node,
            goal: None,
            activated_goal: None,
            cleared_goal: false,
        });
    }
    if node.status == crate::ThreadGoalPlanNodeStatus::Cancelled {
        anyhow::bail!("cannot mark a cancelled goal-plan node complete");
    }

    let goal = mark_projected_goal_complete_in_tx(tx, &node, now_ms).await?;
    let tokens_used = goal
        .as_ref()
        .map_or(node.tokens_used, |goal| goal.tokens_used);
    let time_used_seconds = goal
        .as_ref()
        .map_or(node.time_used_seconds, |goal| goal.time_used_seconds);
    let lines_added = goal
        .as_ref()
        .map_or(node.lines_added, |goal| goal.lines_added);
    let lines_deleted = goal
        .as_ref()
        .map_or(node.lines_deleted, |goal| goal.lines_deleted);
    sqlx::query(
        r#"
UPDATE thread_goal_plan_nodes
SET
    status = ?,
    tokens_used = ?,
    time_used_seconds = ?,
    lines_added = ?,
    lines_deleted = ?,
    updated_at_ms = ?
WHERE node_id = ?
        "#,
    )
    .bind(crate::ThreadGoalPlanNodeStatus::Complete.as_str())
    .bind(tokens_used)
    .bind(time_used_seconds)
    .bind(lines_added)
    .bind(lines_deleted)
    .bind(now_ms)
    .bind(&node.node_id)
    .execute(&mut **tx)
    .await?;

    recalculate_goal_plan_status_in_tx(tx, &node.plan_id, now_ms).await?;
    let should_auto_advance = matches!(
        node.status,
        crate::ThreadGoalPlanNodeStatus::Active
            | crate::ThreadGoalPlanNodeStatus::Paused
            | crate::ThreadGoalPlanNodeStatus::Blocked
            | crate::ThreadGoalPlanNodeStatus::UsageLimited
            | crate::ThreadGoalPlanNodeStatus::BudgetLimited
            | crate::ThreadGoalPlanNodeStatus::Deferred
    );
    let activated_goal = if should_auto_advance {
        activate_next_ready_node_in_tx(
            tx,
            params.thread_id,
            &node.plan_id,
            params.auto_execute,
            now_ms,
        )
        .await?
    } else {
        None
    };
    recalculate_goal_plan_status_in_tx(tx, &node.plan_id, now_ms).await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, &node.plan_id).await?;
    let node = snapshot_node(&snapshot, &node.node_id)?;
    Ok(ThreadGoalPlanNodeMutationOutcome {
        snapshot,
        node,
        goal,
        activated_goal,
        cleared_goal: false,
    })
}

async fn mark_goal_plan_node_pending_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    snapshot: &crate::ThreadGoalPlanSnapshot,
    node: crate::ThreadGoalPlanNode,
    now_ms: i64,
) -> anyhow::Result<ThreadGoalPlanNodeMutationOutcome> {
    if node.status == crate::ThreadGoalPlanNodeStatus::Pending {
        return Ok(ThreadGoalPlanNodeMutationOutcome {
            snapshot: snapshot.clone(),
            node,
            goal: None,
            activated_goal: None,
            cleared_goal: false,
        });
    }
    if node.status != crate::ThreadGoalPlanNodeStatus::Complete {
        anyhow::bail!("can only mark completed goal-plan nodes undone");
    }

    let cleared_goal = if let Some(projected_goal_id) = node.projected_goal_id.as_deref() {
        sqlx::query(
            r#"
DELETE FROM thread_goals
WHERE thread_id = ?
  AND goal_id = ?
            "#,
        )
        .bind(node.assigned_thread_id.to_string())
        .bind(projected_goal_id)
        .execute(&mut **tx)
        .await?
        .rows_affected()
            > 0
    } else {
        false
    };

    sqlx::query(
        r#"
UPDATE thread_goal_plan_nodes
SET
    status = ?,
    projected_goal_id = NULL,
    tokens_used = 0,
    time_used_seconds = 0,
    lines_added = 0,
    lines_deleted = 0,
    updated_at_ms = ?
WHERE node_id = ?
        "#,
    )
    .bind(crate::ThreadGoalPlanNodeStatus::Pending.as_str())
    .bind(now_ms)
    .bind(&node.node_id)
    .execute(&mut **tx)
    .await?;

    recalculate_goal_plan_status_in_tx(tx, &node.plan_id, now_ms).await?;
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, &node.plan_id).await?;
    let node = snapshot_node(&snapshot, &node.node_id)?;
    Ok(ThreadGoalPlanNodeMutationOutcome {
        snapshot,
        node,
        goal: None,
        activated_goal: None,
        cleared_goal,
    })
}

async fn owned_plan_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    thread_id: ThreadId,
    plan_id: &str,
) -> anyhow::Result<crate::ThreadGoalPlanSnapshot> {
    let snapshot = snapshot_thread_goal_plan_in_tx(tx, plan_id).await?;
    if snapshot.plan.thread_id != thread_id {
        anyhow::bail!("goal plan {plan_id} does not belong to thread {thread_id}");
    }
    Ok(snapshot)
}

async fn plan_id_for_node_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    node_id: &str,
) -> anyhow::Result<String> {
    sqlx::query_scalar(
        r#"
SELECT plan_id
FROM thread_goal_plan_nodes
WHERE node_id = ?
        "#,
    )
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("goal plan node `{node_id}` does not exist"))
}

fn snapshot_node_index(
    snapshot: &crate::ThreadGoalPlanSnapshot,
    node_id: &str,
) -> anyhow::Result<usize> {
    snapshot
        .nodes
        .iter()
        .position(|node| node.node_id == node_id)
        .ok_or_else(|| anyhow::anyhow!("goal plan node `{node_id}` does not exist"))
}

fn snapshot_node(
    snapshot: &crate::ThreadGoalPlanSnapshot,
    node_id: &str,
) -> anyhow::Result<crate::ThreadGoalPlanNode> {
    snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("goal plan node `{node_id}` does not exist"))
}

fn reference_node(
    snapshot: &crate::ThreadGoalPlanSnapshot,
    reference_node_id: &str,
) -> anyhow::Result<crate::ThreadGoalPlanNode> {
    require_non_empty_id(reference_node_id, "reference goal plan node id")?;
    snapshot_node(snapshot, reference_node_id)
}

fn require_non_empty_id<'a>(value: &'a str, label: &str) -> anyhow::Result<&'a str> {
    if value.is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    Ok(value)
}

fn normalize_dependency_keys(depends_on: Vec<String>) -> Vec<String> {
    depends_on
        .into_iter()
        .map(|dependency| dependency.trim().to_string())
        .collect()
}

fn validation_nodes_with_replacement(
    snapshot: &crate::ThreadGoalPlanSnapshot,
    node_index: usize,
    old_key: &str,
    updated_node: &crate::ThreadGoalPlanNode,
) -> Vec<ThreadGoalPlanNodeCreateParams> {
    snapshot
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let source = if index == node_index {
                updated_node
            } else {
                node
            };
            let depends_on = source
                .depends_on
                .iter()
                .map(|dependency| {
                    if dependency == old_key {
                        updated_node.key.clone()
                    } else {
                        dependency.clone()
                    }
                })
                .collect();
            ThreadGoalPlanNodeCreateParams {
                key: source.key.clone(),
                objective: source.objective.clone(),
                assigned_thread_id: Some(source.assigned_thread_id),
                title: source.title.clone(),
                priority: source.priority,
                token_budget: source.token_budget,
                depends_on,
            }
        })
        .collect()
}

fn snapshot_nodes_as_create_params(
    nodes: &[crate::ThreadGoalPlanNode],
) -> Vec<ThreadGoalPlanNodeCreateParams> {
    nodes
        .iter()
        .map(|node| ThreadGoalPlanNodeCreateParams {
            key: node.key.clone(),
            objective: node.objective.clone(),
            assigned_thread_id: Some(node.assigned_thread_id),
            title: node.title.clone(),
            priority: node.priority,
            token_budget: node.token_budget,
            depends_on: node.depends_on.clone(),
        })
        .collect()
}

async fn replace_node_dependencies_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    node: &crate::ThreadGoalPlanNode,
    existing_nodes: &[crate::ThreadGoalPlanNode],
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
DELETE FROM thread_goal_plan_dependencies
WHERE node_id = ?
        "#,
    )
    .bind(&node.node_id)
    .execute(&mut **tx)
    .await?;

    let node_ids_by_key = existing_nodes
        .iter()
        .map(|node| (node.key.as_str(), node.node_id.as_str()))
        .chain(std::iter::once((node.key.as_str(), node.node_id.as_str())))
        .collect::<HashMap<_, _>>();
    for dependency_key in &node.depends_on {
        let dependency_id = node_ids_by_key
            .get(dependency_key.as_str())
            .ok_or_else(|| {
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
        .bind(&node.node_id)
        .bind(dependency_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn update_projected_goal_metadata_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    node: &crate::ThreadGoalPlanNode,
    now_ms: i64,
) -> anyhow::Result<Option<crate::ThreadGoal>> {
    let Some(projected_goal_id) = node.projected_goal_id.as_deref() else {
        return Ok(None);
    };
    let row = sqlx::query(
        r#"
UPDATE thread_goals
SET
    objective = ?,
    title = ?,
    token_budget = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND goal_id = ?
RETURNING
    thread_id,
    goal_id,
    objective,
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    lines_added,
    lines_deleted,
    created_at_ms,
    updated_at_ms
        "#,
    )
    .bind(&node.objective)
    .bind(&node.title)
    .bind(node.token_budget)
    .bind(now_ms)
    .bind(node.assigned_thread_id.to_string())
    .bind(projected_goal_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| thread_goal_from_row(&row)).transpose()
}

async fn mark_projected_goal_complete_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    node: &crate::ThreadGoalPlanNode,
    now_ms: i64,
) -> anyhow::Result<Option<crate::ThreadGoal>> {
    let Some(projected_goal_id) = node.projected_goal_id.as_deref() else {
        return Ok(None);
    };
    let row = sqlx::query(
        r#"
UPDATE thread_goals
SET status = ?, updated_at_ms = ?
WHERE thread_id = ?
  AND goal_id = ?
RETURNING
    thread_id,
    goal_id,
    objective,
    title,
    status,
    token_budget,
    tokens_used,
    time_used_seconds,
    lines_added,
    lines_deleted,
    created_at_ms,
    updated_at_ms
        "#,
    )
    .bind(crate::ThreadGoalStatus::Complete.as_str())
    .bind(now_ms)
    .bind(node.assigned_thread_id.to_string())
    .bind(projected_goal_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| thread_goal_from_row(&row)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn completed_goal_plan_node_must_be_marked_pending_before_editing() {
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
                        objective: "First objective".to_string(),
                        assigned_thread_id: None,
                        title: Some("First".to_string()),
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Second objective".to_string(),
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
        let first_node_id = created.snapshot.nodes[0].node_id.clone();
        let active_goal = created.activated_goal.expect("first node should activate");
        let accounted = runtime
            .thread_goals()
            .account_thread_goal_usage(
                thread_id,
                /*time_delta_seconds*/ 7,
                /*token_delta*/ 11,
                Some(crate::ThreadGoalLineChangeStats {
                    lines_added: 23,
                    lines_deleted: 4,
                }),
                crate::GoalAccountingMode::ActiveOnly,
                Some(active_goal.goal_id.as_str()),
            )
            .await
            .expect("goal usage should update");
        let crate::GoalAccountingOutcome::Updated(accounted_goal) = accounted else {
            panic!("goal usage should be updated");
        };

        let completed = runtime
            .thread_goals()
            .set_thread_goal_plan_node_status(ThreadGoalPlanNodeStatusUpdateParams {
                thread_id,
                node_id: first_node_id.clone(),
                status: ThreadGoalPlanNodeCompletionStatus::Complete,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
            })
            .await
            .expect("node should be marked complete");
        assert_eq!(
            (11, 7, 23, 4),
            (
                completed.snapshot.nodes[0].tokens_used,
                completed.snapshot.nodes[0].time_used_seconds,
                completed.snapshot.nodes[0].lines_added,
                completed.snapshot.nodes[0].lines_deleted,
            )
        );
        assert_eq!(
            Some((11, 7, 23, 4)),
            completed.goal.as_ref().map(|goal| (
                goal.tokens_used,
                goal.time_used_seconds,
                goal.lines_added,
                goal.lines_deleted,
            ))
        );
        assert_eq!(accounted_goal.goal_id, active_goal.goal_id);

        let edit_complete = runtime
            .thread_goals()
            .update_thread_goal_plan_node(ThreadGoalPlanNodeUpdateParams {
                thread_id,
                node_id: first_node_id.clone(),
                key: Some("renamed".to_string()),
                objective: None,
                title: ThreadGoalPlanNodeTitleUpdate::Keep,
                priority: None,
                token_budget: ThreadGoalPlanNodeTokenBudgetUpdate::Keep,
                depends_on: None,
            })
            .await;
        assert!(
            edit_complete
                .expect_err("editing a complete node should fail")
                .to_string()
                .contains("mark it undone first")
        );

        let pending = runtime
            .thread_goals()
            .set_thread_goal_plan_node_status(ThreadGoalPlanNodeStatusUpdateParams {
                thread_id,
                node_id: first_node_id.clone(),
                status: ThreadGoalPlanNodeCompletionStatus::Pending,
                auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
            })
            .await
            .expect("complete node should be marked pending");
        assert_eq!(
            (0, 0, 0, 0),
            (
                pending.snapshot.nodes[0].tokens_used,
                pending.snapshot.nodes[0].time_used_seconds,
                pending.snapshot.nodes[0].lines_added,
                pending.snapshot.nodes[0].lines_deleted,
            )
        );
        let pending_summary = pending.snapshot.usage_summary();
        assert_eq!(
            (0, 0, 0, 0),
            (
                pending_summary.total_tokens_used,
                pending_summary.total_time_used_seconds,
                pending_summary.total_lines_added,
                pending_summary.total_lines_deleted,
            )
        );
        let edited = runtime
            .thread_goals()
            .update_thread_goal_plan_node(ThreadGoalPlanNodeUpdateParams {
                thread_id,
                node_id: first_node_id.clone(),
                key: Some("renamed".to_string()),
                objective: Some("Edited first objective".to_string()),
                title: ThreadGoalPlanNodeTitleUpdate::Set(Some("Edited".to_string())),
                priority: Some(5),
                token_budget: ThreadGoalPlanNodeTokenBudgetUpdate::Set(Some(123)),
                depends_on: None,
            })
            .await
            .expect("pending node should be editable");

        assert_eq!("renamed", edited.node.key);
        assert_eq!("Edited first objective", edited.node.objective);
        assert_eq!(Some("Edited".to_string()), edited.node.title);
        assert_eq!(5, edited.node.priority);
        assert_eq!(Some(123), edited.node.token_budget);
        assert_eq!(
            vec!["renamed".to_string()],
            edited.snapshot.nodes[1].depends_on
        );
    }

    #[tokio::test]
    async fn insert_goal_plan_node_between_existing_nodes_updates_sequence() {
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
                        key: "first".to_string(),
                        objective: "First objective".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Second objective".to_string(),
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
        let second_node_id = created.snapshot.nodes[1].node_id.clone();

        let inserted = runtime
            .thread_goals()
            .insert_thread_goal_plan_node(ThreadGoalPlanNodeInsertParams {
                thread_id,
                plan_id: created.snapshot.plan.plan_id,
                position: ThreadGoalPlanNodeInsertPosition::Before(second_node_id.clone()),
                key: "middle".to_string(),
                objective: "Middle objective".to_string(),
                title: Some("Middle".to_string()),
                priority: 2,
                token_budget: Some(50),
                depends_on: vec!["first".to_string()],
            })
            .await
            .expect("node should insert before existing node");

        assert_eq!(
            vec!["first", "middle", "second"],
            inserted
                .snapshot
                .nodes
                .iter()
                .map(|node| node.key.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(1, inserted.inserted_node.sequence);
        assert_eq!(2, inserted.snapshot.nodes[2].sequence);
        assert_eq!(vec!["first".to_string()], inserted.inserted_node.depends_on);
        assert_eq!(None, inserted.inserted_node.parent_node_id);
        assert_eq!(1, inserted.inserted_node.nesting_depth);
    }

    #[tokio::test]
    async fn insert_goal_plan_node_inherits_reference_node_hierarchy() {
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
                        key: "parent".to_string(),
                        objective: "Parent objective".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "child".to_string(),
                        objective: "Child objective".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: vec!["parent".to_string()],
                    },
                ],
            })
            .await
            .expect("goal plan should be created");
        let plan_id = created.snapshot.plan.plan_id.clone();
        let snapshot = runtime
            .thread_goals()
            .update_thread_goal_plan_node_hierarchy(
                thread_id,
                plan_id.as_str(),
                vec![
                    crate::ThreadGoalPlanNodeHierarchyParams {
                        key: "parent".to_string(),
                        parent_key: None,
                        nesting_depth: 1,
                    },
                    crate::ThreadGoalPlanNodeHierarchyParams {
                        key: "child".to_string(),
                        parent_key: Some("parent".to_string()),
                        nesting_depth: 2,
                    },
                ],
            )
            .await
            .expect("hierarchy should apply");
        let parent_node_id = snapshot.nodes[0].node_id.clone();
        let child_node_id = snapshot.nodes[1].node_id.clone();

        let inserted = runtime
            .thread_goals()
            .insert_thread_goal_plan_node(ThreadGoalPlanNodeInsertParams {
                thread_id,
                plan_id: plan_id.clone(),
                position: ThreadGoalPlanNodeInsertPosition::After(child_node_id),
                key: "sibling".to_string(),
                objective: "Sibling objective".to_string(),
                title: None,
                priority: 0,
                token_budget: None,
                depends_on: vec!["parent".to_string()],
            })
            .await
            .expect("node should insert after the nested node");

        assert_eq!(
            Some(parent_node_id),
            inserted.inserted_node.parent_node_id,
            "an inserted sibling inherits the reference node's parent"
        );
        assert_eq!(2, inserted.inserted_node.nesting_depth);

        let appended = runtime
            .thread_goals()
            .insert_thread_goal_plan_node(ThreadGoalPlanNodeInsertParams {
                thread_id,
                plan_id,
                position: ThreadGoalPlanNodeInsertPosition::End,
                key: "tail".to_string(),
                objective: "Tail objective".to_string(),
                title: None,
                priority: 0,
                token_budget: None,
                depends_on: Vec::new(),
            })
            .await
            .expect("node should append at the end");

        assert_eq!(None, appended.inserted_node.parent_node_id);
        assert_eq!(1, appended.inserted_node.nesting_depth);
    }

    #[tokio::test]
    async fn marking_active_goal_plan_node_complete_advances_ready_node() {
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
                        objective: "First objective".to_string(),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    },
                    ThreadGoalPlanNodeCreateParams {
                        key: "second".to_string(),
                        objective: "Second objective".to_string(),
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
        let first_node_id = created.snapshot.nodes[0].node_id.clone();

        let completed = runtime
            .thread_goals()
            .set_thread_goal_plan_node_status(ThreadGoalPlanNodeStatusUpdateParams {
                thread_id,
                node_id: first_node_id,
                status: ThreadGoalPlanNodeCompletionStatus::Complete,
                auto_execute: crate::ThreadGoalPlanAutoExecute::ReadyOnly,
            })
            .await
            .expect("active node should complete");

        assert_eq!(
            Some(crate::ThreadGoalStatus::Complete),
            completed.goal.as_ref().map(|goal| goal.status)
        );
        assert_eq!(
            Some("Second objective"),
            completed
                .activated_goal
                .as_ref()
                .map(|goal| goal.objective.as_str())
        );
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Complete,
                crate::ThreadGoalPlanNodeStatus::Active,
            ],
            completed
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
    }
}
