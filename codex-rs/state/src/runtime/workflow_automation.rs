use super::*;
use crate::runtime::workflows::WorkflowRunEventAppend;
use crate::runtime::workflows::append_workflow_run_event_in_tx;
use crate::runtime::workflows::workflow_state_json_string;
use chrono::Duration as ChronoDuration;
use serde_json::Value;
use serde_json::json;
use uuid::Uuid;

#[derive(Clone)]
pub struct WorkflowAutomationStore {
    pool: Arc<SqlitePool>,
}

impl WorkflowAutomationStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTimerClaimParams {
    pub owner_id: String,
    pub lease_id: String,
    pub lease_duration: Duration,
    pub now: DateTime<Utc>,
    pub max_claims: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTimerClaim {
    pub timer_id: String,
    pub timer_fire_id: String,
    pub run_id: String,
    pub workflow_loop_id: String,
    pub lease_id: String,
    pub generation: i64,
    pub scheduled_for: DateTime<Utc>,
    pub iteration: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTimerFireCompleteParams {
    pub timer_id: String,
    pub timer_fire_id: String,
    pub lease_id: String,
    pub owner_id: String,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTimerFireCompleteOutcome {
    pub run_id: String,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowMonitorObservationParams {
    pub run_id: String,
    pub owner_id: String,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowMonitorObservationOutcome {
    pub changed: bool,
    pub observed_event_count: i64,
}

impl WorkflowAutomationStore {
    pub async fn claim_due_workflow_timers(
        &self,
        params: WorkflowTimerClaimParams,
    ) -> anyhow::Result<Vec<WorkflowTimerClaim>> {
        if params.owner_id.trim().is_empty() {
            anyhow::bail!("workflow timer owner_id must not be empty");
        }
        if params.lease_id.trim().is_empty() {
            anyhow::bail!("workflow timer lease_id must not be empty");
        }
        if params.max_claims == 0 {
            return Ok(Vec::new());
        }
        let lease_duration = ChronoDuration::from_std(params.lease_duration)?;
        let now_ms = datetime_to_epoch_millis(params.now);
        let lease_expires_at_ms = datetime_to_epoch_millis(params.now + lease_duration);
        let mut tx = self.pool.begin().await?;
        let mut claims = Vec::new();
        for _ in 0..params.max_claims {
            let Some(timer) =
                claim_one_due_workflow_timer_in_tx(&mut tx, &params, now_ms, lease_expires_at_ms)
                    .await?
            else {
                break;
            };
            claims.push(timer);
        }
        tx.commit().await?;
        Ok(claims)
    }

    pub async fn complete_workflow_timer_fire(
        &self,
        params: WorkflowTimerFireCompleteParams,
    ) -> anyhow::Result<Option<WorkflowTimerFireCompleteOutcome>> {
        if params.owner_id.trim().is_empty() {
            anyhow::bail!("workflow timer owner_id must not be empty");
        }
        let now_ms = datetime_to_epoch_millis(params.now);
        let mut tx = self.pool.begin().await?;
        let Some(row) = sqlx::query(
            r#"
SELECT
    timer.run_id,
    timer.workflow_loop_id,
    timer.schedule_json,
    timer.timezone,
    timer.iteration_count,
    timer.max_iterations,
    run.generation,
    fire.scheduled_for_ms
FROM workflow_run_timer_fires fire
JOIN workflow_run_timers timer
  ON timer.timer_id = fire.timer_id
JOIN workflow_runs run
  ON run.run_id = timer.run_id
WHERE fire.timer_fire_id = ?
  AND fire.timer_id = ?
  AND fire.lease_id = ?
  AND fire.status = 'claimed'
  AND timer.lease_id = ?
  AND timer.status = 'active'
  AND run.owner_id = ?
  AND run.status NOT IN ('cancelled', 'failed', 'completed', 'paused')
            "#,
        )
        .bind(params.timer_fire_id.as_str())
        .bind(params.timer_id.as_str())
        .bind(params.lease_id.as_str())
        .bind(params.lease_id.as_str())
        .bind(params.owner_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };

        let run_id: String = row.try_get("run_id")?;
        let workflow_loop_id: String = row.try_get("workflow_loop_id")?;
        let schedule_json: String = row.try_get("schedule_json")?;
        let iteration_count: i64 = row.try_get("iteration_count")?;
        let max_iterations: i64 = row.try_get("max_iterations")?;
        let generation: i64 = row.try_get("generation")?;
        let scheduled_for_ms: i64 = row.try_get("scheduled_for_ms")?;
        let next_fire_at_ms = if iteration_count >= max_iterations {
            None
        } else {
            next_workflow_timer_fire_at(schedule_json.as_str(), now_ms)?
        };
        let timer_status = if next_fire_at_ms.is_some() {
            "active"
        } else {
            "expired"
        };
        let result_json = workflow_state_json_string(
            "workflow_timer_fire_result",
            json!({
                "reasonCode": "workflow_timer_tick_completed",
                "generation": generation,
            }),
        )?;
        sqlx::query(
            r#"
UPDATE workflow_run_timer_fires
SET status = 'completed', completed_at_ms = ?, result_json = ?
WHERE timer_fire_id = ?
  AND timer_id = ?
  AND lease_id = ?
  AND status = 'claimed'
            "#,
        )
        .bind(now_ms)
        .bind(result_json)
        .bind(params.timer_fire_id.as_str())
        .bind(params.timer_id.as_str())
        .bind(params.lease_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET
    status = ?,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    last_fire_at_ms = ?,
    next_fire_at_ms = ?,
    updated_at_ms = ?
WHERE timer_id = ?
  AND lease_id = ?
            "#,
        )
        .bind(timer_status)
        .bind(scheduled_for_ms)
        .bind(next_fire_at_ms)
        .bind(now_ms)
        .bind(params.timer_id.as_str())
        .bind(params.lease_id.as_str())
        .execute(&mut *tx)
        .await?;
        append_workflow_run_event_in_tx(
            &mut tx,
            run_id.as_str(),
            WorkflowRunEventAppend {
                event_type: "timer_fire_completed",
                actor_kind: "timer",
                actor_id: Some(params.owner_id),
                step_run_id: None,
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "timerId": params.timer_id,
                    "timerFireId": params.timer_fire_id,
                    "workflowLoopId": workflow_loop_id,
                    "iteration": iteration_count,
                    "generation": generation,
                    "reasonCode": "workflow_timer_tick_completed",
                }),
                now_ms,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(Some(WorkflowTimerFireCompleteOutcome {
            run_id,
            completed: true,
        }))
    }

    pub async fn observe_workflow_monitor_links(
        &self,
        params: WorkflowMonitorObservationParams,
    ) -> anyhow::Result<WorkflowMonitorObservationOutcome> {
        let now_ms = datetime_to_epoch_millis(params.now);
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"
SELECT
    link.link_id,
    link.workflow_monitor_id,
    link.monitor_ref,
    link.max_events_per_tick,
    link.last_seen_event_id,
    link.last_seen_created_at_ms
FROM workflow_run_monitor_links link
JOIN workflow_runs run
  ON run.run_id = link.run_id
WHERE link.run_id = ?
  AND link.status = 'active'
  AND run.owner_id = ?
  AND run.status NOT IN ('cancelled', 'failed', 'completed', 'paused')
  AND (
    link.trigger_step_id IS NULL
    OR EXISTS (
        SELECT 1
        FROM workflow_run_steps step
        WHERE step.run_id = link.run_id
          AND step.step_id = link.trigger_step_id
          AND step.status = 'succeeded'
    )
  )
  AND NOT (
    json_extract(link.stop_condition_json, '$.data.type') = 'step_succeeded'
    AND EXISTS (
        SELECT 1
        FROM workflow_run_steps step
        WHERE step.run_id = link.run_id
          AND step.step_id = json_extract(link.stop_condition_json, '$.data.step')
          AND step.status = 'succeeded'
    )
  )
ORDER BY link.workflow_monitor_id
            "#,
        )
        .bind(params.run_id.as_str())
        .bind(params.owner_id.as_str())
        .fetch_all(&mut *tx)
        .await?;
        let mut observed_event_count = 0_i64;
        let mut changed = false;
        for row in rows {
            let link_id: String = row.try_get("link_id")?;
            let workflow_monitor_id: String = row.try_get("workflow_monitor_id")?;
            let Some(monitor_ref) = row.try_get::<Option<String>, _>("monitor_ref")? else {
                continue;
            };
            let max_events_per_tick: i64 = row.try_get("max_events_per_tick")?;
            let last_seen_created_at_ms: Option<i64> = row.try_get("last_seen_created_at_ms")?;
            let last_seen_event_id: Option<String> = row.try_get("last_seen_event_id")?;
            let events = observed_monitor_events_in_tx(
                &mut tx,
                monitor_ref.as_str(),
                last_seen_created_at_ms,
                last_seen_event_id.as_deref(),
                max_events_per_tick,
            )
            .await?;
            let Some(last_event) = events.last() else {
                continue;
            };
            observed_event_count += i64::try_from(events.len())?;
            changed = true;
            sqlx::query(
                r#"
UPDATE workflow_run_monitor_links
SET last_seen_event_id = ?, last_seen_created_at_ms = ?, updated_at_ms = ?
WHERE link_id = ?
                "#,
            )
            .bind(last_event.event_id.as_str())
            .bind(last_event.created_at_ms)
            .bind(now_ms)
            .bind(link_id.as_str())
            .execute(&mut *tx)
            .await?;
            append_workflow_run_event_in_tx(
                &mut tx,
                params.run_id.as_str(),
                WorkflowRunEventAppend {
                    event_type: "monitor_events_observed",
                    actor_kind: "monitor",
                    actor_id: Some(params.owner_id.clone()),
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "internal",
                    payload: json!({
                        "workflowMonitorId": workflow_monitor_id,
                        "monitorRef": monitor_ref,
                        "eventCount": events.len(),
                        "lastEventId": last_event.event_id,
                        "lastEventAt": last_event.created_at_ms,
                    }),
                    now_ms,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(WorkflowMonitorObservationOutcome {
            changed,
            observed_event_count,
        })
    }
}

struct ClaimedTimerRow {
    timer_id: String,
    run_id: String,
    workflow_loop_id: String,
    generation: i64,
    scheduled_for_ms: i64,
    iteration_count: i64,
}

async fn claim_one_due_workflow_timer_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    params: &WorkflowTimerClaimParams,
    now_ms: i64,
    lease_expires_at_ms: i64,
) -> anyhow::Result<Option<WorkflowTimerClaim>> {
    let row = sqlx::query(
        r#"
UPDATE workflow_run_timers
SET
    lease_id = ?,
    lease_expires_at_ms = ?,
    iteration_count = iteration_count + 1,
    updated_at_ms = ?
WHERE timer_id = (
    SELECT timer.timer_id
    FROM workflow_run_timers timer
    JOIN workflow_runs run
      ON run.run_id = timer.run_id
    WHERE timer.status = 'active'
      AND timer.next_fire_at_ms IS NOT NULL
      AND timer.next_fire_at_ms <= ?
      AND (timer.expires_at_ms IS NULL OR timer.expires_at_ms > ?)
      AND timer.iteration_count < timer.max_iterations
      AND (timer.lease_id IS NULL OR timer.lease_expires_at_ms <= ?)
      AND run.status NOT IN ('cancelled', 'failed', 'completed', 'paused')
      AND NOT (
        json_extract(timer.stop_condition_json, '$.data.type') = 'step_succeeded'
        AND EXISTS (
            SELECT 1
            FROM workflow_run_steps step
            WHERE step.run_id = timer.run_id
              AND step.step_id = json_extract(timer.stop_condition_json, '$.data.step')
              AND step.status = 'succeeded'
        )
      )
    ORDER BY timer.next_fire_at_ms, timer.created_at_ms, timer.timer_id
    LIMIT 1
)
RETURNING
    timer_id,
    run_id,
    workflow_loop_id,
    next_fire_at_ms,
    iteration_count,
    (
        SELECT generation
        FROM workflow_runs
        WHERE workflow_runs.run_id = workflow_run_timers.run_id
    ) AS generation
        "#,
    )
    .bind(params.lease_id.as_str())
    .bind(lease_expires_at_ms)
    .bind(now_ms)
    .bind(now_ms)
    .bind(now_ms)
    .bind(now_ms)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let timer = ClaimedTimerRow {
        timer_id: row.try_get("timer_id")?,
        run_id: row.try_get("run_id")?,
        workflow_loop_id: row.try_get("workflow_loop_id")?,
        scheduled_for_ms: row.try_get("next_fire_at_ms")?,
        iteration_count: row.try_get("iteration_count")?,
        generation: row.try_get("generation")?,
    };
    let fire_key = format!("{}:{}", timer.workflow_loop_id, timer.scheduled_for_ms);
    let timer_fire_id = Uuid::new_v4().to_string();
    let inserted_fire = sqlx::query(
        r#"
INSERT INTO workflow_run_timer_fires (
    timer_fire_id,
    timer_id,
    run_id,
    workflow_loop_id,
    fire_key,
    status,
    lease_id,
    scheduled_for_ms,
    claimed_at_ms
) VALUES (?, ?, ?, ?, ?, 'claimed', ?, ?, ?)
ON CONFLICT(timer_id, fire_key) DO NOTHING
        "#,
    )
    .bind(timer_fire_id.as_str())
    .bind(timer.timer_id.as_str())
    .bind(timer.run_id.as_str())
    .bind(timer.workflow_loop_id.as_str())
    .bind(fire_key.as_str())
    .bind(params.lease_id.as_str())
    .bind(timer.scheduled_for_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    if inserted_fire.rows_affected() == 0 {
        let existing_fire_id: Option<String> = sqlx::query_scalar(
            r#"
UPDATE workflow_run_timer_fires
SET lease_id = ?, claimed_at_ms = ?
WHERE timer_id = ?
  AND fire_key = ?
  AND status = 'claimed'
RETURNING timer_fire_id
            "#,
        )
        .bind(params.lease_id.as_str())
        .bind(now_ms)
        .bind(timer.timer_id.as_str())
        .bind(fire_key.as_str())
        .fetch_optional(&mut **tx)
        .await?;
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET iteration_count = CASE
        WHEN iteration_count > 0 THEN iteration_count - 1
        ELSE 0
    END,
    updated_at_ms = ?
WHERE timer_id = ?
  AND lease_id = ?
            "#,
        )
        .bind(now_ms)
        .bind(timer.timer_id.as_str())
        .bind(params.lease_id.as_str())
        .execute(&mut **tx)
        .await?;
        if let Some(existing_fire_id) = existing_fire_id {
            append_workflow_run_event_in_tx(
                tx,
                timer.run_id.as_str(),
                WorkflowRunEventAppend {
                    event_type: "timer_fire_reclaimed",
                    actor_kind: "timer",
                    actor_id: Some(params.owner_id.clone()),
                    step_run_id: None,
                    verifier_run_id: None,
                    visibility: "internal",
                    payload: json!({
                        "timerId": timer.timer_id,
                        "timerFireId": existing_fire_id,
                        "workflowLoopId": timer.workflow_loop_id,
                        "iteration": timer.iteration_count.saturating_sub(1),
                        "generation": timer.generation,
                    }),
                    now_ms,
                },
            )
            .await?;
            return Ok(Some(WorkflowTimerClaim {
                timer_id: timer.timer_id,
                timer_fire_id: existing_fire_id,
                run_id: timer.run_id,
                workflow_loop_id: timer.workflow_loop_id,
                lease_id: params.lease_id.clone(),
                generation: timer.generation,
                scheduled_for: epoch_millis_to_datetime(timer.scheduled_for_ms)?,
                iteration: timer.iteration_count.saturating_sub(1),
            }));
        }
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET lease_id = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?
WHERE timer_id = ? AND lease_id = ?
            "#,
        )
        .bind(now_ms)
        .bind(timer.timer_id.as_str())
        .bind(params.lease_id.as_str())
        .execute(&mut **tx)
        .await?;
        return Ok(None);
    }
    append_workflow_run_event_in_tx(
        tx,
        timer.run_id.as_str(),
        WorkflowRunEventAppend {
            event_type: "timer_fire_claimed",
            actor_kind: "timer",
            actor_id: Some(params.owner_id.clone()),
            step_run_id: None,
            verifier_run_id: None,
            visibility: "internal",
            payload: json!({
                "timerId": timer.timer_id,
                "timerFireId": timer_fire_id,
                "workflowLoopId": timer.workflow_loop_id,
                "iteration": timer.iteration_count,
                "generation": timer.generation,
            }),
            now_ms,
        },
    )
    .await?;
    Ok(Some(WorkflowTimerClaim {
        timer_id: timer.timer_id,
        timer_fire_id,
        run_id: timer.run_id,
        workflow_loop_id: timer.workflow_loop_id,
        lease_id: params.lease_id.clone(),
        generation: timer.generation,
        scheduled_for: epoch_millis_to_datetime(timer.scheduled_for_ms)?,
        iteration: timer.iteration_count,
    }))
}

pub(crate) async fn arm_workflow_timers_for_succeeded_step_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    run_id: &str,
    step_id: &str,
    owner_id: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
        r#"
UPDATE workflow_run_timers
SET next_fire_at_ms = ?, updated_at_ms = ?
WHERE run_id = ?
  AND trigger_step_id = ?
  AND status = 'active'
  AND next_fire_at_ms IS NULL
RETURNING timer_id, workflow_loop_id
        "#,
    )
    .bind(now_ms)
    .bind(now_ms)
    .bind(run_id)
    .bind(step_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let timer_id: String = row.try_get("timer_id")?;
        let workflow_loop_id: String = row.try_get("workflow_loop_id")?;
        append_workflow_run_event_in_tx(
            tx,
            run_id,
            WorkflowRunEventAppend {
                event_type: "timer_armed",
                actor_kind: "orchestrator",
                actor_id: Some(owner_id.to_string()),
                step_run_id: None,
                verifier_run_id: None,
                visibility: "internal",
                payload: json!({
                    "timerId": timer_id,
                    "workflowLoopId": workflow_loop_id,
                    "triggerStepId": step_id,
                }),
                now_ms,
            },
        )
        .await?;
    }
    Ok(())
}

struct ObservedMonitorEvent {
    event_id: String,
    created_at_ms: i64,
}

async fn observed_monitor_events_in_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    monitor_id: &str,
    last_seen_created_at_ms: Option<i64>,
    last_seen_event_id: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<ObservedMonitorEvent>> {
    let rows = sqlx::query(
        r#"
SELECT event_id, created_at_ms
FROM thread_monitor_events
WHERE monitor_id = ?
  AND (
    ? IS NULL
    OR created_at_ms > ?
    OR (created_at_ms = ? AND event_id > ?)
  )
ORDER BY created_at_ms, event_id
LIMIT ?
        "#,
    )
    .bind(monitor_id)
    .bind(last_seen_created_at_ms)
    .bind(last_seen_created_at_ms)
    .bind(last_seen_created_at_ms)
    .bind(last_seen_event_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ObservedMonitorEvent {
                event_id: row.try_get("event_id")?,
                created_at_ms: row.try_get("created_at_ms")?,
            })
        })
        .collect()
}

fn next_workflow_timer_fire_at(schedule_json: &str, after_ms: i64) -> anyhow::Result<Option<i64>> {
    let payload: Value = serde_json::from_str(schedule_json)?;
    let data = payload
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("workflow timer schedule is missing data"))?;
    let schedule_type = data
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("workflow timer schedule is missing type"))?;
    let duration = match schedule_type {
        "dynamic" => Some(ChronoDuration::minutes(1)),
        "interval" => {
            let amount = data
                .get("amount")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow::anyhow!("workflow timer interval is missing amount"))?;
            let unit = data
                .get("unit")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("workflow timer interval is missing unit"))?;
            match unit {
                "minutes" => Some(ChronoDuration::minutes(amount)),
                "hours" => Some(ChronoDuration::hours(amount)),
                "days" => Some(ChronoDuration::days(amount)),
                other => anyhow::bail!("unknown workflow timer interval unit `{other}`"),
            }
        }
        "cron" => None,
        other => anyhow::bail!("unknown workflow timer schedule type `{other}`"),
    };
    duration
        .map(|duration| {
            epoch_millis_to_datetime(after_ms)
                .and_then(|after| {
                    after
                        .checked_add_signed(duration)
                        .ok_or_else(|| anyhow::anyhow!("workflow timer next fire overflowed"))
                })
                .map(datetime_to_epoch_millis)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ThreadMonitorCreateParams;
    use crate::ThreadMonitorEventCreateParams;
    use crate::WorkflowRunAdvanceParams;
    use crate::WorkflowRunCancelParams;
    use crate::WorkflowRunClaimParams;
    use crate::WorkflowRunCreateParams;
    use crate::WorkflowSpecCreateParams;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000045").expect("valid thread id")
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

    fn workflow_with_automation_yaml() -> String {
        r#"schema_version: "workflow.codex.codewith/v0"
workflow_id: "wf_timer_adapter"
display_name: "Timer Adapter"
source_prompt: "build and keep testing"
status: "draft"
execution_defaults:
  model_gateway: "hasna"
  provider: "openai"
  model: "gpt-5.4"
  reasoning: "high"
limits:
  max_parallel_steps: 2
  max_agents: 3
  max_worktrees: 1
  max_runtime_seconds: 3600
  max_step_runtime_seconds: 1200
  max_tokens: 100000
  max_tool_calls: 100
approvals:
  required_before: []
agents:
  - id: "builder"
    display_name: "Builder-Vitruvius"
    role: "Build the system."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "security_adversary"
    display_name: "Adversary-Hypatia"
    role: "Challenge security."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
  - id: "business_adversary"
    display_name: "Adversary-Cicero"
    role: "Challenge business assumptions."
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
steps:
  - id: "build"
    title: "Build the first slice"
    agent: "builder"
    model:
      model_gateway: "hasna"
      provider: "openai"
      model: "gpt-5.4"
      reasoning: "high"
    depends_on: []
    outputs:
      - "build.md"
    completion:
      model_marked_state: "candidate_succeeded"
      verifiers:
        - id: "build_doc"
          type: "artifact_contains"
          artifact: "build.md"
          must_contain:
            - "tests"
loops:
  - id: "quality_loop"
    title: "Keep testing until workflow complete"
    schedule:
      type: "interval"
      amount: 5
      unit: "minutes"
    timezone: "UTC"
    stop_condition:
      type: "workflow_complete"
    max_iterations: 2
    expires_after_seconds: 3600
monitors:
  - id: "ci_observer"
    title: "Observe CI monitor"
    source: "existing_thread_monitor"
    monitor_ref: "monitor-1"
    stop_condition:
      type: "workflow_complete"
    max_events_per_tick: 3
artifacts:
  retention: "preserve_evidence"
  required:
    - "build.md"
cleanup:
  on_cancel:
    - "cancel_timers"
  on_complete:
    - "archive_events"
"#
        .to_string()
    }

    async fn create_automation_run(runtime: &StateRuntime) -> crate::WorkflowRunSnapshot {
        let thread_id = test_thread_id();
        upsert_test_thread(runtime, thread_id).await;
        let spec = runtime
            .workflows()
            .save_workflow_spec_yaml(WorkflowSpecCreateParams {
                source_thread_id: Some(thread_id),
                source_yaml: workflow_with_automation_yaml(),
            })
            .await
            .expect("workflow spec should save");
        runtime
            .workflows()
            .create_workflow_run(WorkflowRunCreateParams {
                workflow_record_id: spec.workflow_record_id,
                source_thread_id: Some(thread_id),
                idempotency_key: Some("automation-run".to_string()),
            })
            .await
            .expect("workflow run should create")
    }

    #[tokio::test]
    async fn workflow_run_creates_owned_timers_and_monitor_links() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;

        assert_eq!(
            1,
            snapshot.run.loops_json.as_ref().unwrap()["data"]
                .as_array()
                .unwrap()
                .len()
        );
        assert_eq!(
            1,
            snapshot.run.monitor_links_json.as_ref().unwrap()["data"]
                .as_array()
                .unwrap()
                .len()
        );
        let timer_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_run_timers WHERE run_id = ?")
                .bind(snapshot.run.run_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("timer count should query");
        let monitor_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_run_monitor_links WHERE run_id = ?")
                .bind(snapshot.run.run_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("monitor count should query");
        assert_eq!(1, timer_count);
        assert_eq!(1, monitor_count);
    }

    #[tokio::test]
    async fn workflow_timer_claim_rearms_until_bounded_iteration_limit() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "timer-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed");
        let now = Utc::now() + ChronoDuration::seconds(1);
        let first_claim = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-1".to_string(),
                lease_duration: Duration::from_secs(60),
                now,
                max_claims: 1,
            })
            .await
            .expect("timer should claim");
        assert_eq!(1, first_claim.len());
        assert_eq!(1, first_claim[0].iteration);

        runtime
            .workflow_automation()
            .complete_workflow_timer_fire(WorkflowTimerFireCompleteParams {
                timer_id: first_claim[0].timer_id.clone(),
                timer_fire_id: first_claim[0].timer_fire_id.clone(),
                lease_id: first_claim[0].lease_id.clone(),
                owner_id: "timer-owner".to_string(),
                now,
            })
            .await
            .expect("timer completion should succeed")
            .expect("timer completion should update");
        let no_early_claim = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-early".to_string(),
                lease_duration: Duration::from_secs(60),
                now: now + ChronoDuration::minutes(4),
                max_claims: 1,
            })
            .await
            .expect("timer claim should query");
        assert_eq!(Vec::<WorkflowTimerClaim>::new(), no_early_claim);
        let second_claim = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-2".to_string(),
                lease_duration: Duration::from_secs(60),
                now: now + ChronoDuration::minutes(5),
                max_claims: 1,
            })
            .await
            .expect("second timer should claim");
        assert_eq!(1, second_claim.len());
        assert_eq!(2, second_claim[0].iteration);
        runtime
            .workflow_automation()
            .complete_workflow_timer_fire(WorkflowTimerFireCompleteParams {
                timer_id: second_claim[0].timer_id.clone(),
                timer_fire_id: second_claim[0].timer_fire_id.clone(),
                lease_id: second_claim[0].lease_id.clone(),
                owner_id: "timer-owner".to_string(),
                now: now + ChronoDuration::minutes(5),
            })
            .await
            .expect("timer completion should succeed")
            .expect("timer completion should update");
        let timer_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_run_timers WHERE timer_id = ?")
                .bind(second_claim[0].timer_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("timer status should query");
        assert_eq!("expired", timer_status);
    }

    #[tokio::test]
    async fn workflow_timer_stale_lease_reclaims_same_fire_without_extra_iteration() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "timer-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed");
        let now = Utc::now() + ChronoDuration::seconds(1);
        let first_claim = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-1".to_string(),
                lease_duration: Duration::from_secs(60),
                now,
                max_claims: 1,
            })
            .await
            .expect("timer should claim");
        sqlx::query("UPDATE workflow_run_timers SET lease_expires_at_ms = 0 WHERE timer_id = ?")
            .bind(first_claim[0].timer_id.as_str())
            .execute(runtime.pool.as_ref())
            .await
            .expect("lease should expire");
        let reclaimed = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-2".to_string(),
                lease_duration: Duration::from_secs(60),
                now: now + ChronoDuration::minutes(2),
                max_claims: 1,
            })
            .await
            .expect("timer should reclaim");
        assert_eq!(1, reclaimed.len());
        assert_eq!(first_claim[0].timer_fire_id, reclaimed[0].timer_fire_id);
        assert_eq!(1, reclaimed[0].iteration);
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT iteration_count, (SELECT COUNT(*) FROM workflow_run_timer_fires WHERE timer_id = workflow_run_timers.timer_id) FROM workflow_run_timers WHERE timer_id = ?",
        )
        .bind(reclaimed[0].timer_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("timer counts should query");
        assert_eq!((1, 1), counts);
    }

    #[tokio::test]
    async fn workflow_timer_claim_respects_trigger_stop_and_expiry_fences() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "timer-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed");
        let now = Utc::now() + ChronoDuration::seconds(1);
        let now_ms = datetime_to_epoch_millis(now);
        sqlx::query(
            "UPDATE workflow_run_timers SET trigger_step_id = 'build', next_fire_at_ms = NULL WHERE run_id = ?",
        )
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("timer should become triggered");
        let unarmed = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-unarmed".to_string(),
                lease_duration: Duration::from_secs(60),
                now,
                max_claims: 1,
            })
            .await
            .expect("unarmed timer query should succeed");
        assert_eq!(Vec::<WorkflowTimerClaim>::new(), unarmed);

        let stop_condition_json = workflow_state_json_string(
            "workflow_run_loop_stop_condition",
            json!({
                "type": "step_succeeded",
                "step": "build",
            }),
        )
        .expect("stop condition should serialize");
        sqlx::query(
            "UPDATE workflow_run_steps SET status = 'succeeded' WHERE run_id = ? AND step_id = 'build'",
        )
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("step should succeed");
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET trigger_step_id = NULL,
    stop_condition_json = ?,
    next_fire_at_ms = ?,
    expires_at_ms = NULL,
    lease_id = NULL,
    lease_expires_at_ms = NULL
WHERE run_id = ?
            "#,
        )
        .bind(stop_condition_json)
        .bind(now_ms)
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("timer should be due but stopped");
        let stopped = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-stopped".to_string(),
                lease_duration: Duration::from_secs(60),
                now,
                max_claims: 1,
            })
            .await
            .expect("stopped timer query should succeed");
        assert_eq!(Vec::<WorkflowTimerClaim>::new(), stopped);

        let workflow_stop_json = workflow_state_json_string(
            "workflow_run_loop_stop_condition",
            json!({ "type": "workflow_complete" }),
        )
        .expect("workflow stop condition should serialize");
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET stop_condition_json = ?,
    next_fire_at_ms = ?,
    expires_at_ms = ?,
    lease_id = NULL,
    lease_expires_at_ms = NULL
WHERE run_id = ?
            "#,
        )
        .bind(workflow_stop_json)
        .bind(now_ms)
        .bind(now_ms - 1)
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("timer should be expired");
        let expired = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-expired".to_string(),
                lease_duration: Duration::from_secs(60),
                now,
                max_claims: 1,
            })
            .await
            .expect("expired timer query should succeed");
        assert_eq!(Vec::<WorkflowTimerClaim>::new(), expired);
    }

    #[tokio::test]
    async fn workflow_timer_completion_handles_dynamic_and_cron_schedules() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "timer-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed");
        let now = Utc::now() + ChronoDuration::seconds(1);
        let now_ms = datetime_to_epoch_millis(now);
        let dynamic_schedule_json =
            workflow_state_json_string("workflow_run_loop_schedule", json!({ "type": "dynamic" }))
                .expect("dynamic schedule should serialize");
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET schedule_json = ?,
    stop_condition_json = ?,
    max_iterations = 2,
    iteration_count = 0,
    next_fire_at_ms = ?,
    expires_at_ms = NULL,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    status = 'active'
WHERE run_id = ?
            "#,
        )
        .bind(dynamic_schedule_json)
        .bind(
            workflow_state_json_string(
                "workflow_run_loop_stop_condition",
                json!({ "type": "workflow_complete" }),
            )
            .expect("stop condition should serialize"),
        )
        .bind(now_ms)
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("timer should use dynamic schedule");
        let dynamic_claim = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-dynamic".to_string(),
                lease_duration: Duration::from_secs(60),
                now,
                max_claims: 1,
            })
            .await
            .expect("dynamic timer should claim");
        assert_eq!(1, dynamic_claim.len());
        runtime
            .workflow_automation()
            .complete_workflow_timer_fire(WorkflowTimerFireCompleteParams {
                timer_id: dynamic_claim[0].timer_id.clone(),
                timer_fire_id: dynamic_claim[0].timer_fire_id.clone(),
                lease_id: dynamic_claim[0].lease_id.clone(),
                owner_id: "timer-owner".to_string(),
                now,
            })
            .await
            .expect("dynamic completion should succeed")
            .expect("dynamic completion should update");
        let dynamic_row: (String, Option<i64>) = sqlx::query_as(
            "SELECT status, next_fire_at_ms FROM workflow_run_timers WHERE timer_id = ?",
        )
        .bind(dynamic_claim[0].timer_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("dynamic timer should query");
        assert_eq!(("active".to_string(), Some(now_ms + 60_000)), dynamic_row);

        let cron_due = now + ChronoDuration::minutes(2);
        let cron_due_ms = datetime_to_epoch_millis(cron_due);
        let cron_schedule_json = workflow_state_json_string(
            "workflow_run_loop_schedule",
            json!({
                "type": "cron",
                "expression": "*/5 * * * *",
            }),
        )
        .expect("cron schedule should serialize");
        sqlx::query(
            r#"
UPDATE workflow_run_timers
SET schedule_json = ?,
    max_iterations = 2,
    iteration_count = 0,
    next_fire_at_ms = ?,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    status = 'active'
WHERE timer_id = ?
            "#,
        )
        .bind(cron_schedule_json)
        .bind(cron_due_ms)
        .bind(dynamic_claim[0].timer_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("timer should use cron schedule");
        let cron_claim = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "timer-owner".to_string(),
                lease_id: "lease-cron".to_string(),
                lease_duration: Duration::from_secs(60),
                now: cron_due,
                max_claims: 1,
            })
            .await
            .expect("cron timer should claim");
        assert_eq!(1, cron_claim.len());
        runtime
            .workflow_automation()
            .complete_workflow_timer_fire(WorkflowTimerFireCompleteParams {
                timer_id: cron_claim[0].timer_id.clone(),
                timer_fire_id: cron_claim[0].timer_fire_id.clone(),
                lease_id: cron_claim[0].lease_id.clone(),
                owner_id: "timer-owner".to_string(),
                now: cron_due,
            })
            .await
            .expect("cron completion should succeed")
            .expect("cron completion should update");
        let cron_row: (String, Option<i64>) = sqlx::query_as(
            "SELECT status, next_fire_at_ms FROM workflow_run_timers WHERE timer_id = ?",
        )
        .bind(cron_claim[0].timer_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("cron timer should query");
        assert_eq!(("expired".to_string(), None), cron_row);
    }

    #[tokio::test]
    async fn workflow_monitor_observation_records_sanitized_event_counts() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed");
        runtime
            .thread_monitors()
            .create_thread_monitor(ThreadMonitorCreateParams {
                thread_id: test_thread_id(),
                name: "CI".to_string(),
                prompt: "watch ci".to_string(),
                command: "echo ok".to_string(),
                cwd: None,
                routing: crate::ThreadMonitorRouting::Stream,
                output_file: None,
                status: crate::ThreadMonitorStatus::Running,
            })
            .await
            .expect("monitor should create");
        let monitor = runtime
            .thread_monitors()
            .list_thread_monitors(test_thread_id())
            .await
            .expect("monitors should list")
            .pop()
            .expect("monitor should exist");
        sqlx::query("UPDATE workflow_run_monitor_links SET monitor_ref = ? WHERE run_id = ?")
            .bind(monitor.monitor_id.as_str())
            .bind(snapshot.run.run_id.as_str())
            .execute(runtime.pool.as_ref())
            .await
            .expect("monitor link should bind");
        runtime
            .thread_monitors()
            .create_thread_monitor_event(ThreadMonitorEventCreateParams {
                thread_id: test_thread_id(),
                monitor_id: monitor.monitor_id.clone(),
                stream: crate::ThreadMonitorEventStream::Stdout,
                text: "SECRET_TOKEN=do-not-copy".to_string(),
            })
            .await
            .expect("monitor event should create");
        let observed = runtime
            .workflow_automation()
            .observe_workflow_monitor_links(WorkflowMonitorObservationParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                now: Utc::now(),
            })
            .await
            .expect("monitor observation should succeed");
        assert_eq!(
            WorkflowMonitorObservationOutcome {
                changed: true,
                observed_event_count: 1,
            },
            observed
        );
        let event_payloads_json: String = sqlx::query_scalar(
            "SELECT json_group_array(event_payload_json) FROM workflow_run_events WHERE run_id = ?",
        )
        .bind(snapshot.run.run_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("workflow events should query");
        let event_types_json: String = sqlx::query_scalar(
            "SELECT json_group_array(event_type) FROM workflow_run_events WHERE run_id = ?",
        )
        .bind(snapshot.run.run_id.as_str())
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("workflow event types should query");
        assert!(!event_payloads_json.contains("SECRET_TOKEN"));
        assert!(event_types_json.contains("monitor_events_observed"));
    }

    #[tokio::test]
    async fn workflow_monitor_observation_respects_trigger_cursor_and_stop_condition() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed");
        runtime
            .thread_monitors()
            .create_thread_monitor(ThreadMonitorCreateParams {
                thread_id: test_thread_id(),
                name: "CI".to_string(),
                prompt: "watch ci".to_string(),
                command: "echo ok".to_string(),
                cwd: None,
                routing: crate::ThreadMonitorRouting::Stream,
                output_file: None,
                status: crate::ThreadMonitorStatus::Running,
            })
            .await
            .expect("monitor should create");
        let monitor = runtime
            .thread_monitors()
            .list_thread_monitors(test_thread_id())
            .await
            .expect("monitors should list")
            .pop()
            .expect("monitor should exist");
        sqlx::query(
            "UPDATE workflow_run_monitor_links SET monitor_ref = ?, trigger_step_id = 'build', max_events_per_tick = 2 WHERE run_id = ?",
        )
        .bind(monitor.monitor_id.as_str())
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("monitor link should bind");
        for index in 1..=3 {
            runtime
                .thread_monitors()
                .create_thread_monitor_event(ThreadMonitorEventCreateParams {
                    thread_id: test_thread_id(),
                    monitor_id: monitor.monitor_id.clone(),
                    stream: crate::ThreadMonitorEventStream::Stdout,
                    text: format!("event-{index} SECRET_TOKEN=do-not-copy"),
                })
                .await
                .expect("monitor event should create");
        }

        let before_trigger = runtime
            .workflow_automation()
            .observe_workflow_monitor_links(WorkflowMonitorObservationParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                now: Utc::now(),
            })
            .await
            .expect("monitor observation before trigger should succeed");
        assert_eq!(
            WorkflowMonitorObservationOutcome {
                changed: false,
                observed_event_count: 0,
            },
            before_trigger
        );

        sqlx::query(
            "UPDATE workflow_run_steps SET status = 'succeeded' WHERE run_id = ? AND step_id = 'build'",
        )
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("trigger step should succeed");
        let first_page = runtime
            .workflow_automation()
            .observe_workflow_monitor_links(WorkflowMonitorObservationParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                now: Utc::now(),
            })
            .await
            .expect("first monitor observation page should succeed");
        assert_eq!(
            WorkflowMonitorObservationOutcome {
                changed: true,
                observed_event_count: 2,
            },
            first_page
        );
        let second_page = runtime
            .workflow_automation()
            .observe_workflow_monitor_links(WorkflowMonitorObservationParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                now: Utc::now(),
            })
            .await
            .expect("second monitor observation page should succeed");
        assert_eq!(
            WorkflowMonitorObservationOutcome {
                changed: true,
                observed_event_count: 1,
            },
            second_page
        );
        let no_duplicate = runtime
            .workflow_automation()
            .observe_workflow_monitor_links(WorkflowMonitorObservationParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "monitor-owner".to_string(),
                now: Utc::now(),
            })
            .await
            .expect("empty monitor observation should succeed");
        assert_eq!(
            WorkflowMonitorObservationOutcome {
                changed: false,
                observed_event_count: 0,
            },
            no_duplicate
        );

        let stop_condition_json = workflow_state_json_string(
            "workflow_run_monitor_stop_condition",
            json!({
                "type": "step_succeeded",
                "step": "build",
            }),
        )
        .expect("monitor stop condition should serialize");
        sqlx::query(
            "UPDATE workflow_run_monitor_links SET stop_condition_json = ?, last_seen_event_id = NULL, last_seen_created_at_ms = NULL WHERE run_id = ?",
        )
        .bind(stop_condition_json)
        .bind(snapshot.run.run_id.as_str())
        .execute(runtime.pool.as_ref())
        .await
        .expect("monitor link should stop after step");
        runtime
            .thread_monitors()
            .create_thread_monitor_event(ThreadMonitorEventCreateParams {
                thread_id: test_thread_id(),
                monitor_id: monitor.monitor_id,
                stream: crate::ThreadMonitorEventStream::Stdout,
                text: "event-after-stop SECRET_TOKEN=do-not-copy".to_string(),
            })
            .await
            .expect("post-stop monitor event should create");
        let stopped = runtime
            .workflow_automation()
            .observe_workflow_monitor_links(WorkflowMonitorObservationParams {
                run_id: snapshot.run.run_id,
                owner_id: "monitor-owner".to_string(),
                now: Utc::now(),
            })
            .await
            .expect("stopped monitor observation should succeed");
        assert_eq!(
            WorkflowMonitorObservationOutcome {
                changed: false,
                observed_event_count: 0,
            },
            stopped
        );
    }

    #[tokio::test]
    async fn workflow_cancel_terminalizes_owned_automation() {
        let runtime = test_runtime().await;
        let snapshot = create_automation_run(&runtime).await;
        let claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "cancel-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("claim should succeed")
            .expect("workflow should claim");
        let claimed_timer = runtime
            .workflow_automation()
            .claim_due_workflow_timers(WorkflowTimerClaimParams {
                owner_id: "cancel-owner".to_string(),
                lease_id: "lease-cancel".to_string(),
                lease_duration: Duration::from_secs(60),
                now: Utc::now() + ChronoDuration::seconds(1),
                max_claims: 1,
            })
            .await
            .expect("timer should claim");
        assert_eq!(1, claimed_timer.len());
        runtime
            .request_workflow_run_cancel(WorkflowRunCancelParams {
                run_id: snapshot.run.run_id.clone(),
                reason: "stop".to_string(),
            })
            .await
            .expect("cancel request should succeed");
        let cancel_claim = runtime
            .claim_workflow_run(WorkflowRunClaimParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "cancel-owner".to_string(),
                lease_duration_ms: None,
            })
            .await
            .expect("cancel claim should succeed")
            .expect("cancel should claim");
        runtime
            .advance_workflow_run(WorkflowRunAdvanceParams {
                run_id: snapshot.run.run_id.clone(),
                owner_id: "cancel-owner".to_string(),
                generation: cancel_claim.generation,
            })
            .await
            .expect("cancel advance should succeed");
        assert!(cancel_claim.generation > claim.generation);
        let timer_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_run_timers WHERE run_id = ?")
                .bind(snapshot.run.run_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("timer status should query");
        let fire_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_run_timer_fires WHERE timer_id = ?")
                .bind(claimed_timer[0].timer_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("fire status should query");
        let monitor_status: String =
            sqlx::query_scalar("SELECT status FROM workflow_run_monitor_links WHERE run_id = ?")
                .bind(snapshot.run.run_id.as_str())
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("monitor link status should query");
        assert_eq!("cancelled", timer_status);
        assert_eq!("skipped", fire_status);
        assert_eq!("cancelled", monitor_status);
    }
}
