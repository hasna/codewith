use super::*;
use crate::runtime::goal_plans::insert_thread_goal_plan_in_tx;
use crate::runtime::goal_plans::snapshot_thread_goal_plan_in_tx;
use crate::runtime::workflows::workflow_state_json_string;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowGoalPlanProjectionParams {
    pub workflow_run_id: String,
    pub thread_id: ThreadId,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowGoalPlanProjectionOutcome {
    pub projection_id: String,
    pub run_id: String,
    pub thread_id: ThreadId,
    pub plan_id: String,
    pub idempotency_key: Option<String>,
    pub created: bool,
    pub snapshot: crate::ThreadGoalPlanSnapshot,
}

struct WorkflowGoalPlanProjectionCreateParams<'a> {
    run: &'a crate::WorkflowRunSnapshot,
    thread_id: ThreadId,
    idempotency_key: Option<&'a str>,
    now_ms: i64,
}

struct WorkflowGoalPlanProjectionRow {
    projection_id: String,
    run_id: String,
    thread_id: String,
    plan_id: String,
    idempotency_key: Option<String>,
}

struct InsertWorkflowGoalPlanProjectionParams<'a> {
    projection_id: &'a str,
    run: &'a crate::WorkflowRunSnapshot,
    thread_id: ThreadId,
    plan_id: &'a str,
    idempotency_key: Option<&'a str>,
    now_ms: i64,
}

impl StateRuntime {
    pub async fn project_workflow_run_to_goal_plan(
        &self,
        params: WorkflowGoalPlanProjectionParams,
    ) -> anyhow::Result<Option<WorkflowGoalPlanProjectionOutcome>> {
        validate_workflow_projection_params(&params)?;
        let Some(run) = self
            .workflows()
            .get_workflow_run_snapshot(params.workflow_run_id.as_str())
            .await?
        else {
            return Ok(None);
        };
        if !is_workflow_run_projectable(&run) {
            anyhow::bail!(
                "workflow run {} cannot be projected after reaching terminal status {}",
                run.run.run_id,
                run.run.status.as_str()
            );
        }
        if run.run.source_thread_id != Some(params.thread_id) {
            anyhow::bail!(
                "workflow run {} does not belong to thread {}",
                run.run.run_id,
                params.thread_id
            );
        }

        self.thread_goals()
            .create_workflow_goal_plan_projection(WorkflowGoalPlanProjectionCreateParams {
                run: &run,
                thread_id: params.thread_id,
                idempotency_key: params.idempotency_key.as_deref(),
                now_ms: datetime_to_epoch_millis(Utc::now()),
            })
            .await
            .map(Some)
    }
}

impl GoalStore {
    async fn create_workflow_goal_plan_projection(
        &self,
        params: WorkflowGoalPlanProjectionCreateParams<'_>,
    ) -> anyhow::Result<WorkflowGoalPlanProjectionOutcome> {
        let mut tx = self.pool.begin().await?;
        if let Some(row) =
            workflow_goal_plan_projection_by_run_in_tx(&mut tx, params.run.run.run_id.as_str())
                .await?
        {
            let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, row.plan_id.as_str()).await?;
            tx.commit().await?;
            return workflow_goal_plan_projection_from_row(row, /*created*/ false, snapshot);
        }

        let plan_params = thread_goal_plan_from_workflow_run(params.run, params.thread_id)?;
        let plan = insert_thread_goal_plan_in_tx(&mut tx, plan_params, params.now_ms).await?;
        let projection_id = Uuid::new_v4().to_string();
        insert_workflow_goal_plan_projection_in_tx(
            &mut tx,
            InsertWorkflowGoalPlanProjectionParams {
                projection_id: projection_id.as_str(),
                run: params.run,
                thread_id: params.thread_id,
                plan_id: plan.snapshot.plan.plan_id.as_str(),
                idempotency_key: params.idempotency_key,
                now_ms: params.now_ms,
            },
        )
        .await?;
        let snapshot =
            snapshot_thread_goal_plan_in_tx(&mut tx, plan.snapshot.plan.plan_id.as_str()).await?;
        let row =
            workflow_goal_plan_projection_by_run_in_tx(&mut tx, params.run.run.run_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("workflow goal-plan projection was not inserted"))?;
        tx.commit().await?;
        workflow_goal_plan_projection_from_row(row, /*created*/ true, snapshot)
    }

    pub(crate) async fn block_workflow_goal_plan_projection(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
        let Some(plan_id) =
            workflow_goal_plan_projection_plan_id_in_pool(&self.pool, run_id).await?
        else {
            return Ok(Vec::new());
        };
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status IN ('pending', 'active', 'paused', 'usage_limited')
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Blocked.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status = 'active'
            "#,
        )
        .bind(crate::ThreadGoalPlanStatus::Blocked.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, plan_id.as_str()).await?;
        tx.commit().await?;
        Ok(vec![snapshot])
    }

    pub(crate) async fn pause_workflow_goal_plan_projection(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
        let Some(plan_id) =
            workflow_goal_plan_projection_plan_id_in_pool(&self.pool, run_id).await?
        else {
            return Ok(Vec::new());
        };
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
UPDATE thread_goals
SET status = ?, updated_at_ms = ?
WHERE status = 'active'
  AND EXISTS (
      SELECT 1
      FROM thread_goal_plan_nodes node
      WHERE node.projected_goal_id = thread_goals.goal_id
        AND node.plan_id = ?
  )
            "#,
        )
        .bind(crate::ThreadGoalStatus::Paused.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status IN ('pending', 'active')
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Paused.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status = 'active'
            "#,
        )
        .bind(crate::ThreadGoalPlanStatus::Paused.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, plan_id.as_str()).await?;
        tx.commit().await?;
        Ok(vec![snapshot])
    }

    pub(crate) async fn resume_workflow_goal_plan_projection(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Vec<crate::ThreadGoalPlanSnapshot>> {
        let Some(plan_id) =
            workflow_goal_plan_projection_plan_id_in_pool(&self.pool, run_id).await?
        else {
            return Ok(Vec::new());
        };
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status = 'paused'
  AND EXISTS (
      SELECT 1
      FROM thread_goals current_goal
      WHERE current_goal.goal_id = thread_goal_plan_nodes.projected_goal_id
        AND current_goal.status = 'paused'
  )
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Active.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE thread_goals
SET status = ?, updated_at_ms = ?
WHERE status = 'paused'
  AND EXISTS (
      SELECT 1
      FROM thread_goal_plan_nodes node
      WHERE node.projected_goal_id = thread_goals.goal_id
        AND node.plan_id = ?
  )
            "#,
        )
        .bind(crate::ThreadGoalStatus::Active.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plan_nodes
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status = 'paused'
            "#,
        )
        .bind(crate::ThreadGoalPlanNodeStatus::Pending.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE thread_goal_plans
SET status = ?, updated_at_ms = ?
WHERE plan_id = ?
  AND status = 'paused'
            "#,
        )
        .bind(crate::ThreadGoalPlanStatus::Active.as_str())
        .bind(now_ms)
        .bind(plan_id.as_str())
        .execute(&mut *tx)
        .await?;
        let snapshot = snapshot_thread_goal_plan_in_tx(&mut tx, plan_id.as_str()).await?;
        tx.commit().await?;
        Ok(vec![snapshot])
    }

    pub(crate) async fn completed_workflow_projection_steps(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
SELECT projection.step_id
FROM workflow_goal_plan_node_projections projection
JOIN thread_goal_plan_nodes node
  ON node.node_id = projection.node_id
JOIN workflow_goal_plan_projections goal_projection
  ON goal_projection.projection_id = projection.projection_id
WHERE projection.run_id = ?
  AND goal_projection.run_id = ?
  AND node.status = 'complete'
ORDER BY node.sequence, projection.step_id
            "#,
        )
        .bind(run_id)
        .bind(run_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        rows.into_iter()
            .map(|row| row.try_get("step_id").map_err(anyhow::Error::from))
            .collect()
    }
}

fn workflow_goal_plan_projection_from_row(
    row: WorkflowGoalPlanProjectionRow,
    created: bool,
    snapshot: crate::ThreadGoalPlanSnapshot,
) -> anyhow::Result<WorkflowGoalPlanProjectionOutcome> {
    Ok(WorkflowGoalPlanProjectionOutcome {
        projection_id: row.projection_id,
        run_id: row.run_id,
        thread_id: ThreadId::from_string(row.thread_id.as_str())?,
        plan_id: row.plan_id,
        idempotency_key: row.idempotency_key,
        created,
        snapshot,
    })
}

async fn workflow_goal_plan_projection_by_run_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
) -> anyhow::Result<Option<WorkflowGoalPlanProjectionRow>> {
    let row = sqlx::query(
        r#"
SELECT projection_id, run_id, thread_id, plan_id, idempotency_key
FROM workflow_goal_plan_projections
WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(WorkflowGoalPlanProjectionRow {
            projection_id: row.try_get("projection_id")?,
            run_id: row.try_get("run_id")?,
            thread_id: row.try_get("thread_id")?,
            plan_id: row.try_get("plan_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
        })
    })
    .transpose()
}

async fn workflow_goal_plan_projection_plan_id_in_pool(
    pool: &SqlitePool,
    run_id: &str,
) -> anyhow::Result<Option<String>> {
    sqlx::query_scalar(
        r#"
SELECT plan_id
FROM workflow_goal_plan_projections
WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)
}

async fn insert_workflow_goal_plan_projection_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: InsertWorkflowGoalPlanProjectionParams<'_>,
) -> anyhow::Result<()> {
    let metadata_json = workflow_state_json_string(
        "workflow_goal_plan_projection_metadata",
        json!({
            "specWorkflowId": params.run.run.spec_workflow_id,
            "schemaVersion": params.run.run.schema_version,
            "sourceYamlSha256": params.run.run.source_yaml_sha256,
            "stepCount": params.run.steps.len(),
        }),
    )?;
    sqlx::query(
        r#"
INSERT INTO workflow_goal_plan_projections (
    projection_id,
    run_id,
    thread_id,
    plan_id,
    idempotency_key,
    status,
    metadata_json,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, 'projected', ?, ?, ?)
        "#,
    )
    .bind(params.projection_id)
    .bind(params.run.run.run_id.as_str())
    .bind(params.thread_id.to_string())
    .bind(params.plan_id)
    .bind(params.idempotency_key)
    .bind(metadata_json)
    .bind(params.now_ms)
    .bind(params.now_ms)
    .execute(&mut **tx)
    .await?;

    let nodes = snapshot_thread_goal_plan_in_tx(tx, params.plan_id)
        .await?
        .nodes;
    let nodes_by_key: HashMap<&str, &crate::ThreadGoalPlanNode> =
        nodes.iter().map(|node| (node.key.as_str(), node)).collect();
    let verifier_summaries = workflow_verifier_summaries_by_step(&params.run.verifiers)?;
    for step in &params.run.steps {
        let node = nodes_by_key.get(step.step_id.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "workflow projection missing goal-plan node for step {}",
                step.step_id
            )
        })?;
        let verifier_summary = verifier_summaries
            .get(step.step_id.as_str())
            .cloned()
            .unwrap_or_else(|| json!([]));
        sqlx::query(
            r#"
INSERT INTO workflow_goal_plan_node_projections (
    projection_id,
    run_id,
    step_id,
    step_run_id,
    node_id,
    agent_id,
    model_route_json,
    workspace_json,
    verifier_summary_json,
    time_budget_seconds,
    token_budget,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(params.projection_id)
        .bind(params.run.run.run_id.as_str())
        .bind(step.step_id.as_str())
        .bind(step.step_run_id.as_str())
        .bind(node.node_id.as_str())
        .bind(step.agent_id.as_str())
        .bind(optional_json_string(
            "workflow_goal_plan_node_model_route",
            &step.model_route_json,
        )?)
        .bind(optional_json_string(
            "workflow_goal_plan_node_workspace",
            &step.workspace_json,
        )?)
        .bind(workflow_state_json_string(
            "workflow_goal_plan_node_verifier_summary",
            verifier_summary,
        )?)
        .bind(workflow_step_time_budget_seconds(
            &params.run.run.limits_json,
        )?)
        .bind(node.token_budget)
        .bind(params.now_ms)
        .bind(params.now_ms)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn thread_goal_plan_from_workflow_run(
    run: &crate::WorkflowRunSnapshot,
    thread_id: ThreadId,
) -> anyhow::Result<ThreadGoalPlanCreateParams> {
    Ok(ThreadGoalPlanCreateParams {
        thread_id,
        auto_execute: crate::ThreadGoalPlanAutoExecute::Off,
        max_tokens: workflow_max_tokens(&run.run.limits_json)?,
        nodes: run
            .steps
            .iter()
            .map(|step| {
                Ok(ThreadGoalPlanNodeCreateParams {
                    key: step.step_id.clone(),
                    objective: workflow_step_objective(run, step),
                    assigned_thread_id: None,
                    priority: workflow_step_priority(step.sequence),
                    token_budget: workflow_step_token_budget(
                        &run.run.limits_json,
                        run.steps.len(),
                    )?,
                    depends_on: step.depends_on.clone(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    })
}

fn workflow_step_objective(
    run: &crate::WorkflowRunSnapshot,
    step: &crate::WorkflowRunStep,
) -> String {
    let verifier_count = run
        .verifiers
        .iter()
        .filter(|verifier| verifier.step_id == step.step_id)
        .count();
    let dependency_text = if step.depends_on.is_empty() {
        "no workflow step dependencies".to_string()
    } else {
        format!("depends on workflow steps: {}", step.depends_on.join(", "))
    };
    let approval_text = step
        .approval_gate
        .as_ref()
        .map(|gate| format!(" approval gate: {gate}."))
        .unwrap_or_default();
    let parallel_text = step
        .parallel_group
        .as_ref()
        .map(|group| format!(" parallel group: {group}."))
        .unwrap_or_default();
    let verifier_text = match verifier_count {
        0 => "no deterministic verifiers".to_string(),
        1 => "1 deterministic verifier".to_string(),
        count => format!("{count} deterministic verifiers"),
    };
    format!(
        "Workflow `{}` step `{}`: {}. Agent `{}`; {}; {};{}{} Produce the declared workflow outputs and stop with evidence for the verifier phase.",
        run.run.spec_workflow_id,
        step.step_id,
        step.title,
        step.agent_id,
        dependency_text,
        verifier_text,
        parallel_text,
        approval_text
    )
}

fn workflow_step_priority(sequence: i64) -> i64 {
    sequence.saturating_neg()
}

fn workflow_max_tokens(limits_json: &Value) -> anyhow::Result<Option<i64>> {
    workflow_limit_i64(limits_json, "max_tokens")
}

fn workflow_step_token_budget(
    limits_json: &Value,
    step_count: usize,
) -> anyhow::Result<Option<i64>> {
    let Some(max_tokens) = workflow_max_tokens(limits_json)? else {
        return Ok(None);
    };
    let step_count = i64::try_from(step_count)?.max(1);
    Ok(Some((max_tokens / step_count).max(1)))
}

fn workflow_step_time_budget_seconds(limits_json: &Value) -> anyhow::Result<Option<i64>> {
    workflow_limit_i64(limits_json, "max_step_runtime_seconds")
}

fn workflow_limit_i64(limits_json: &Value, key: &str) -> anyhow::Result<Option<i64>> {
    match workflow_state_data(limits_json).get(key) {
        Some(Value::Number(number)) => number
            .as_u64()
            .map(i64::try_from)
            .transpose()
            .map_err(anyhow::Error::from),
        Some(_) => anyhow::bail!("workflow limit `{key}` must be a non-negative integer"),
        None => Ok(None),
    }
}

fn workflow_state_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn workflow_verifier_summaries_by_step(
    verifiers: &[crate::WorkflowRunStepVerifier],
) -> anyhow::Result<HashMap<&str, Value>> {
    let mut summaries_by_step: HashMap<&str, Vec<Value>> = HashMap::new();
    for verifier in verifiers {
        let definition = &verifier.definition_json;
        let summary = json!({
            "id": verifier.verifier_id,
            "type": verifier.verifier_type,
            "status": verifier.status.as_str(),
            "maxAttempts": verifier.max_attempts,
            "cwd": definition.get("cwd").and_then(Value::as_str),
            "sandbox": definition.get("sandbox").and_then(Value::as_str),
            "network": definition.get("network").and_then(Value::as_str),
            "timeoutSeconds": definition.get("timeout_seconds").and_then(Value::as_u64),
            "outputLimitBytes": definition.get("output_limit_bytes").and_then(Value::as_u64),
            "expectedExitCode": definition.get("expected_exit_code").and_then(Value::as_i64),
        });
        summaries_by_step
            .entry(verifier.step_id.as_str())
            .or_default()
            .push(summary);
    }

    summaries_by_step
        .into_iter()
        .map(|(step_id, summaries)| Ok((step_id, workflow_sanitized_json_array(summaries)?)))
        .collect()
}

fn workflow_sanitized_json_array(values: Vec<Value>) -> anyhow::Result<Value> {
    serde_json::to_value(values).map_err(anyhow::Error::from)
}

fn optional_json_string(kind: &str, value: &Option<Value>) -> anyhow::Result<Option<String>> {
    value
        .as_ref()
        .map(|value| workflow_state_json_string(kind, value.clone()))
        .transpose()
}

fn validate_workflow_projection_params(
    params: &WorkflowGoalPlanProjectionParams,
) -> anyhow::Result<()> {
    if params.workflow_run_id.trim().is_empty() {
        anyhow::bail!("workflow_run_id must not be empty");
    }
    if params
        .idempotency_key
        .as_ref()
        .is_some_and(|key| key.trim().is_empty())
    {
        anyhow::bail!("idempotency_key must not be empty when set");
    }
    Ok(())
}

fn is_workflow_run_projectable(run: &crate::WorkflowRunSnapshot) -> bool {
    !run.run.status.is_terminal()
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

    fn projection_test_workflow_yaml(workflow_id: &str, provider: &str) -> String {
        format!(
            r#"
schema_version: "workflow.codex.codewith/v0"
workflow_id: "{workflow_id}"
display_name: "Projection Test"
source_prompt: "build a workflow that must not leak into goal objectives"
status: "draft"
execution_defaults:
  model_gateway: "malicious-gateway"
  provider: "{provider}"
  model: "malicious-model"
  reasoning: "xhigh"
  service_tier: "malicious-tier"
  approval_policy: "never-ask"
  permission_profile: "root-everything"
limits:
  max_parallel_steps: 2
  max_agents: 2
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 120
  max_tokens: 6000
  max_tool_calls: 50
approvals:
  required_before: []
agents:
  - id: "adversarial_scope"
    display_name: "Adversary-Hypatia"
    role: "Attack workflow scope and permission drift."
    model:
      model_gateway: "malicious-gateway"
      provider: "{provider}"
      model: "malicious-model"
      reasoning: "xhigh"
      service_tier: "malicious-tier"
      approval_policy: "never-ask"
      permission_profile: "root-everything"
  - id: "adversarial_review"
    display_name: "Adversary-Cicero"
    role: "Attack verifier and business readiness assumptions."
    model:
      model_gateway: "malicious-gateway"
      provider: "{provider}"
      model: "malicious-model"
      reasoning: "xhigh"
      service_tier: "malicious-tier"
      approval_policy: "never-ask"
      permission_profile: "root-everything"
steps:
  - id: "adversarial_scope"
    title: "Scope the implementation without leaking route strings"
    agent: "adversarial_scope"
    model:
      model_gateway: "malicious-gateway"
      provider: "{provider}"
      model: "malicious-model"
      reasoning: "xhigh"
      service_tier: "malicious-tier"
      approval_policy: "never-ask"
      permission_profile: "root-everything"
    workspace:
      mode: "shared"
    depends_on: []
    outputs:
      - "scope.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "scope_artifact"
          type: "artifact_contains"
          artifact: "scope.md"
          must_contain:
            - "scope"
  - id: "adversarial_review"
    title: "Review implementation and deterministic commands"
    agent: "adversarial_review"
    model:
      model_gateway: "malicious-gateway"
      provider: "{provider}"
      model: "malicious-model"
      reasoning: "xhigh"
      service_tier: "malicious-tier"
      approval_policy: "never-ask"
      permission_profile: "root-everything"
    parallel_group: "review"
    depends_on:
      - "adversarial_scope"
    outputs:
      - "review.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "review_commands"
          type: "run_commands"
          cwd: "."
          sandbox: "read-only"
          network: "disabled"
          timeout_seconds: 30
          output_limit_bytes: 2048
          commands:
            - "echo should-not-leak-command"
          expected_exit_code: 0
artifacts:
  retention: "until_workflow_complete"
  required:
    - "scope.md"
    - "review.md"
cleanup:
  on_cancel: []
  on_complete: []
"#
        )
    }

    async fn create_projection_test_run(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        workflow_id: &str,
        provider: &str,
    ) -> crate::WorkflowRunSnapshot {
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: projection_test_workflow_yaml(workflow_id, provider),
            })
            .await
            .expect("workflow spec should save");
        runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: Some(format!("{workflow_id}-run")),
            })
            .await
            .expect("workflow run should create")
    }

    #[tokio::test]
    async fn workflow_projection_creates_inert_idempotent_goal_plan() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let run =
            create_projection_test_run(&runtime, thread_id, "wf_projection_inert", "evil-provider")
                .await;

        let projected = runtime
            .project_workflow_run_to_goal_plan(WorkflowGoalPlanProjectionParams {
                workflow_run_id: run.run.run_id.clone(),
                thread_id,
                idempotency_key: Some("projection-key".to_string()),
            })
            .await
            .expect("projection should create")
            .expect("workflow run should exist");

        assert!(projected.created);
        assert_eq!(run.run.run_id, projected.run_id);
        assert_eq!(thread_id, projected.thread_id);
        assert_eq!(
            crate::ThreadGoalPlanAutoExecute::Off,
            projected.snapshot.plan.auto_execute
        );
        assert_eq!(Some(6000), projected.snapshot.plan.max_tokens);
        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            projected
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            vec![Vec::<String>::new(), vec!["adversarial_scope".to_string()]],
            projected
                .snapshot
                .nodes
                .iter()
                .map(|node| node.depends_on.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            None,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("thread goal should read")
        );
        for node in &projected.snapshot.nodes {
            assert_eq!(Some(3000), node.token_budget);
            assert!(!node.objective.contains("evil-provider"));
            assert!(!node.objective.contains("root-everything"));
            assert!(!node.objective.contains("should-not-leak-command"));
            assert!(
                !node
                    .objective
                    .contains("build a workflow that must not leak")
            );
        }

        let replayed = runtime
            .project_workflow_run_to_goal_plan(WorkflowGoalPlanProjectionParams {
                workflow_run_id: run.run.run_id.clone(),
                thread_id,
                idempotency_key: Some("projection-key".to_string()),
            })
            .await
            .expect("projection replay should read")
            .expect("workflow run should exist");
        assert!(!replayed.created);
        assert_eq!(projected.projection_id, replayed.projection_id);
        assert_eq!(projected.plan_id, replayed.plan_id);

        let plan_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_goal_plans WHERE thread_id = ?")
                .bind(thread_id.to_string())
                .fetch_one(runtime.thread_goals().pool.as_ref())
                .await
                .expect("goal plan count should read");
        assert_eq!(1, plan_count);

        let route_json: String = sqlx::query_scalar(
            r#"
SELECT model_route_json
FROM workflow_goal_plan_node_projections
WHERE run_id = ? AND step_id = 'adversarial_scope'
            "#,
        )
        .bind(run.run.run_id.as_str())
        .fetch_one(runtime.thread_goals().pool.as_ref())
        .await
        .expect("route metadata should read");
        assert!(route_json.contains("evil-provider"));
        assert!(route_json.contains("root-everything"));

        let verifier_summary_json: String = sqlx::query_scalar(
            r#"
SELECT verifier_summary_json
FROM workflow_goal_plan_node_projections
WHERE run_id = ? AND step_id = 'adversarial_review'
            "#,
        )
        .bind(run.run.run_id.as_str())
        .fetch_one(runtime.thread_goals().pool.as_ref())
        .await
        .expect("verifier summary should read");
        assert!(verifier_summary_json.contains("review_commands"));
        assert!(!verifier_summary_json.contains("should-not-leak-command"));
    }

    #[tokio::test]
    async fn workflow_projection_rejects_cross_thread_run() {
        let runtime = test_runtime().await;
        let thread_a = test_thread_id();
        let thread_b =
            ThreadId::from_string("00000000-0000-0000-0000-000000000789").expect("valid id");
        upsert_test_thread(&runtime, thread_a).await;
        upsert_test_thread(&runtime, thread_b).await;
        let run_b =
            create_projection_test_run(&runtime, thread_b, "wf_projection_scope", "provider-b")
                .await;

        let err = runtime
            .project_workflow_run_to_goal_plan(WorkflowGoalPlanProjectionParams {
                workflow_run_id: run_b.run.run_id,
                thread_id: thread_a,
                idempotency_key: None,
            })
            .await
            .expect_err("cross-thread projection should fail");

        assert!(err.to_string().contains("does not belong to thread"));
        let plan_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_goal_plans WHERE thread_id = ?")
                .bind(thread_a.to_string())
                .fetch_one(runtime.thread_goals().pool.as_ref())
                .await
                .expect("goal plan count should read");
        assert_eq!(0, plan_count);
    }

    #[tokio::test]
    async fn workflow_projection_does_not_overwrite_existing_goal() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id();
        upsert_test_thread(&runtime, thread_id).await;
        let existing = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "Manual active goal must stay current.",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("manual goal should create");
        let run =
            create_projection_test_run(&runtime, thread_id, "wf_projection_manual_goal", "openai")
                .await;

        let projected = runtime
            .project_workflow_run_to_goal_plan(WorkflowGoalPlanProjectionParams {
                workflow_run_id: run.run.run_id,
                thread_id,
                idempotency_key: None,
            })
            .await
            .expect("projection should create")
            .expect("workflow run should exist");

        assert_eq!(
            vec![
                crate::ThreadGoalPlanNodeStatus::Pending,
                crate::ThreadGoalPlanNodeStatus::Pending,
            ],
            projected
                .snapshot
                .nodes
                .iter()
                .map(|node| node.status)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            existing,
            runtime
                .thread_goals()
                .get_thread_goal(thread_id)
                .await
                .expect("thread goal should read")
                .expect("manual goal should remain")
        );
    }
}
