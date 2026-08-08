//! Terminal recording, cadence finalization, and explicit deferrals.

use super::*;

impl ScheduleStore {
    pub async fn record_thread_schedule_run_terminal(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        expected_goal_id: Option<&str>,
        error: Option<String>,
    ) -> anyhow::Result<bool> {
        let completed_at_ms = datetime_to_epoch_millis(completed_at);
        let status = if error.is_some() {
            crate::ThreadScheduleRunStatus::Failed
        } else {
            crate::ThreadScheduleRunStatus::Completed
        };
        let error = error.map(redact_state_string);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let run_result = sqlx::query(
            r#"
UPDATE thread_schedule_runs
SET status = ?, error = ?, completed_at_ms = ?
WHERE schedule_id = ?
  AND run_id = ?
  AND lease_id = ?
  AND status = 'running'
  AND (? IS NULL OR goal_id IS NULL OR goal_id = ?)
  AND EXISTS (
      SELECT 1
      FROM thread_schedules
      WHERE thread_schedules.schedule_id = thread_schedule_runs.schedule_id
        AND thread_schedules.lease_id = ?
        AND thread_schedules.lease_expires_at_ms > ?
  )
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_occurrences
      WHERE thread_schedule_occurrences.occurrence_id = thread_schedule_runs.run_id
        AND thread_schedule_occurrences.state = 'started'
  )
            "#,
        )
        .bind(status.as_str())
        .bind(error)
        .bind(completed_at_ms)
        .bind(schedule_id)
        .bind(run_id)
        .bind(lease_id)
        .bind(expected_goal_id)
        .bind(expected_goal_id)
        .bind(lease_id)
        .bind(completed_at_ms)
        .execute(&mut *tx)
        .await?;
        if run_result.rows_affected() == 0 {
            let already_terminal: bool = sqlx::query_scalar(
                r#"
SELECT EXISTS(
    SELECT 1
    FROM thread_schedule_runs
    JOIN thread_schedule_occurrences
      ON thread_schedule_occurrences.occurrence_id = thread_schedule_runs.run_id
    JOIN thread_schedules
      ON thread_schedules.schedule_id = thread_schedule_runs.schedule_id
    WHERE thread_schedule_runs.schedule_id = ?
      AND thread_schedule_runs.run_id = ?
      AND thread_schedule_runs.lease_id = ?
      AND thread_schedule_runs.status IN ('completed', 'failed')
      AND thread_schedule_occurrences.state = 'terminal'
      AND thread_schedules.lease_id = ?
      AND thread_schedules.lease_expires_at_ms > ?
      AND (? IS NULL OR thread_schedule_runs.goal_id IS NULL OR thread_schedule_runs.goal_id = ?)
)
                "#,
            )
            .bind(schedule_id)
            .bind(run_id)
            .bind(lease_id)
            .bind(lease_id)
            .bind(completed_at_ms)
            .bind(expected_goal_id)
            .bind(expected_goal_id)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(already_terminal);
        }
        let occurrence_result = sqlx::query(
            r#"
UPDATE thread_schedule_occurrences
SET state = 'terminal', updated_at_ms = ?
WHERE occurrence_id = ? AND schedule_id = ? AND state = 'started'
            "#,
        )
        .bind(completed_at_ms)
        .bind(run_id)
        .bind(schedule_id)
        .execute(&mut *tx)
        .await?;
        if occurrence_result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn fail_thread_schedule_occurrence_before_start(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        goal_id: Option<&str>,
        error: String,
    ) -> anyhow::Result<bool> {
        let completed_at_ms = datetime_to_epoch_millis(completed_at);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let sql = run_returning(
            r#"
INSERT INTO thread_schedule_runs (
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    turn_id,
    goal_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
)
SELECT
    occurrence_id,
    schedule_id,
    thread_id,
    'failed',
    ?,
    turn_id,
    COALESCE(goal_id, ?),
    ?,
    scheduled_for_ms,
    created_at_ms,
    ?
FROM thread_schedule_occurrences
WHERE occurrence_id = ?
  AND schedule_id = ?
  AND state IN ('waiting_idle', 'enqueued')
  AND EXISTS (
      SELECT 1
      FROM thread_schedules
      WHERE thread_schedules.schedule_id = thread_schedule_occurrences.schedule_id
        AND thread_schedules.lease_id = ?
  )
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(lease_id)
            .bind(goal_id)
            .bind(redact_state_string(error))
            .bind(completed_at_ms)
            .bind(run_id)
            .bind(schedule_id)
            .bind(lease_id)
            .fetch_optional(&mut *tx)
            .await?;
        if row.is_none() {
            tx.commit().await?;
            return Ok(false);
        }
        let occurrence_result = sqlx::query(
            r#"
UPDATE thread_schedule_occurrences
SET state = 'terminal',
    goal_id = COALESCE(goal_id, ?),
    updated_at_ms = ?
WHERE occurrence_id = ? AND state IN ('waiting_idle', 'enqueued')
            "#,
        )
        .bind(goal_id)
        .bind(completed_at_ms)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        if occurrence_result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn finalize_terminal_thread_schedule_run(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
        expected_goal_id: Option<&str>,
    ) -> anyhow::Result<bool> {
        self.finalize_terminal_thread_schedule_run_once(FinalizeThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id,
        })
        .await
    }

    pub async fn complete_thread_schedule_run(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<bool> {
        self.finish_thread_schedule_run(FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id: None,
            finish: FinishScheduleRun::Completed,
        })
        .await
    }

    pub async fn complete_thread_schedule_run_for_goal(
        &self,
        params: ThreadScheduleRunForGoalFinishParams<'_>,
    ) -> anyhow::Result<bool> {
        let ThreadScheduleRunForGoalFinishParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id,
        } = params;
        self.finish_thread_schedule_run(FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id: Some(expected_goal_id),
            finish: FinishScheduleRun::Completed,
        })
        .await
    }

    pub async fn fail_thread_schedule_run(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
        error: String,
    ) -> anyhow::Result<bool> {
        self.finish_thread_schedule_run(FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id: None,
            finish: FinishScheduleRun::Failed { error },
        })
        .await
    }

    pub async fn fail_thread_schedule_run_for_goal(
        &self,
        params: ThreadScheduleRunForGoalFinishParams<'_>,
        error: String,
    ) -> anyhow::Result<bool> {
        let ThreadScheduleRunForGoalFinishParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id,
        } = params;
        self.finish_thread_schedule_run(FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id: Some(expected_goal_id),
            finish: FinishScheduleRun::Failed { error },
        })
        .await
    }

    pub async fn defer_thread_schedule_run(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        next_run_at: DateTime<Utc>,
        error: String,
    ) -> anyhow::Result<bool> {
        let completed_at_ms = datetime_to_epoch_millis(completed_at);
        let requested_next_run_at_ms = datetime_to_epoch_millis(next_run_at);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let run_result = sqlx::query(
            r#"
INSERT INTO thread_schedule_runs (
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    turn_id,
    goal_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms,
    deferral_kind
)
SELECT
    occurrence_id,
    schedule_id,
    thread_id,
    'deferred',
    ?,
    NULL,
    goal_id,
    ?,
    scheduled_for_ms,
    created_at_ms,
    ?,
    'capacity'
FROM thread_schedule_occurrences
WHERE schedule_id = ?
  AND occurrence_id = ?
  AND EXISTS (
      SELECT 1
      FROM thread_schedules
      WHERE thread_schedules.schedule_id = thread_schedule_occurrences.schedule_id
        AND thread_schedules.lease_id = ?
  )
ON CONFLICT(run_id) DO UPDATE SET
    status = 'deferred',
    error = excluded.error,
    completed_at_ms = excluded.completed_at_ms,
    deferral_kind = 'capacity'
            "#,
        )
        .bind(lease_id)
        .bind(redact_state_string(error))
        .bind(completed_at_ms)
        .bind(schedule_id)
        .bind(run_id)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if run_result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query("DELETE FROM thread_schedule_occurrences WHERE occurrence_id = ?")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        let schedule_result = sqlx::query(
            r#"
UPDATE thread_schedules
SET
    status = CASE
        WHEN status = 'expired' THEN 'expired'
        WHEN expires_at_ms IS NOT NULL AND ? >= expires_at_ms THEN 'expired'
        WHEN status = 'paused' THEN 'paused'
        ELSE status
    END,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    last_run_at_ms = ?,
    next_run_at_ms = CASE
        WHEN status IN ('expired', 'paused') THEN NULL
        WHEN expires_at_ms IS NOT NULL AND ? >= expires_at_ms THEN NULL
        ELSE ?
    END,
    updated_at_ms = ?
WHERE schedule_id = ? AND lease_id = ?
            "#,
        )
        .bind(requested_next_run_at_ms)
        .bind(completed_at_ms)
        .bind(requested_next_run_at_ms)
        .bind(requested_next_run_at_ms)
        .bind(completed_at_ms)
        .bind(schedule_id)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if schedule_result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn wait_thread_schedule_run_for_idle(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        retry_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let now_ms = datetime_to_epoch_millis(now);
        let retry_at_ms = datetime_to_epoch_millis(retry_at);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let occurrence_result = sqlx::query(
            r#"
UPDATE thread_schedule_occurrences
SET retry_at_ms = ?,
    updated_at_ms = ?
WHERE schedule_id = ?
  AND occurrence_id = ?
  AND state IN ('waiting_idle', 'enqueued', 'started')
            "#,
        )
        .bind(retry_at_ms)
        .bind(now_ms)
        .bind(schedule_id)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        if occurrence_result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        let schedule_result = sqlx::query(
            r#"
UPDATE thread_schedules
SET lease_id = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?
WHERE schedule_id = ? AND lease_id = ?
            "#,
        )
        .bind(now_ms)
        .bind(schedule_id)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if schedule_result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn expire_thread_schedules(&self, now: DateTime<Utc>) -> anyhow::Result<u64> {
        let now_ms = datetime_to_epoch_millis(now);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            r#"
UPDATE thread_schedule_runs
SET status = 'failed',
    error = ?,
    completed_at_ms = COALESCE(completed_at_ms, ?)
WHERE status IN ('leased', 'running')
  AND EXISTS (
      SELECT 1
      FROM thread_schedules
      WHERE thread_schedules.schedule_id = thread_schedule_runs.schedule_id
        AND thread_schedules.status = 'active'
        AND thread_schedules.expires_at_ms IS NOT NULL
        AND thread_schedules.expires_at_ms <= ?
        AND (
            thread_schedules.lease_id IS NULL
            OR thread_schedules.lease_expires_at_ms <= ?
        )
  )
            "#,
        )
        .bind(redact_state_string(EXPIRED_SCHEDULE_RUN_ERROR))
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"
UPDATE thread_schedules
SET
    status = 'expired',
    next_run_at_ms = NULL,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    updated_at_ms = ?
WHERE status = 'active'
  AND expires_at_ms IS NOT NULL
  AND expires_at_ms <= ?
  AND (lease_id IS NULL OR lease_expires_at_ms <= ?)
            "#,
        )
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
DELETE FROM thread_schedule_occurrences
WHERE EXISTS (
    SELECT 1
    FROM thread_schedules
    WHERE thread_schedules.schedule_id = thread_schedule_occurrences.schedule_id
      AND thread_schedules.status = 'expired'
)
            "#,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    async fn finish_thread_schedule_run(
        &self,
        params: FinishThreadScheduleRunParams<'_>,
    ) -> anyhow::Result<bool> {
        crate::busy_retry::retry_on_busy("finish thread schedule run", || {
            self.finish_thread_schedule_run_once(params.clone())
        })
        .await
    }

    async fn finish_thread_schedule_run_once(
        &self,
        params: FinishThreadScheduleRunParams<'_>,
    ) -> anyhow::Result<bool> {
        let FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id,
            finish,
        } = params;
        let error = match finish {
            FinishScheduleRun::Completed => None,
            FinishScheduleRun::Failed { error } => Some(error),
        };
        if !self
            .record_thread_schedule_run_terminal(
                schedule_id,
                run_id,
                lease_id,
                completed_at,
                expected_goal_id,
                error,
            )
            .await?
        {
            return Ok(false);
        }
        self.finalize_terminal_thread_schedule_run_once(FinalizeThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id,
        })
        .await
    }

    async fn finalize_terminal_thread_schedule_run_once(
        &self,
        params: FinalizeThreadScheduleRunParams<'_>,
    ) -> anyhow::Result<bool> {
        let FinalizeThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at,
            expected_goal_id,
        } = params;
        let completed_at_ms = datetime_to_epoch_millis(completed_at);
        let next_run_at_ms = next_run_at.map(datetime_to_epoch_millis);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let schedule_context: Option<(String, String, String)> = sqlx::query_as(
            r#"
SELECT thread_schedules.thread_id, thread_schedules.schedule_kind, thread_schedule_runs.status
FROM thread_schedules
JOIN thread_schedule_runs ON thread_schedule_runs.schedule_id = thread_schedules.schedule_id
JOIN thread_schedule_occurrences ON thread_schedule_occurrences.occurrence_id = thread_schedule_runs.run_id
WHERE thread_schedules.schedule_id = ? AND thread_schedules.lease_id = ?
  AND thread_schedule_runs.run_id = ?
  AND thread_schedule_runs.lease_id = ?
  AND thread_schedule_runs.status IN ('completed', 'failed')
  AND thread_schedule_occurrences.state = 'terminal'
  AND (? IS NULL OR thread_schedule_runs.goal_id IS NULL OR thread_schedule_runs.goal_id = ?)
            "#,
        )
        .bind(schedule_id)
        .bind(lease_id)
        .bind(run_id)
        .bind(lease_id)
        .bind(expected_goal_id)
        .bind(expected_goal_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((thread_id, schedule_kind, run_status)) = schedule_context else {
            tx.commit().await?;
            return Ok(false);
        };
        let goal_hold_can_pause = expected_goal_id.is_some() && schedule_kind != ONCE_SCHEDULE_KIND;
        // Read-only probe: this transaction only runs `SELECT EXISTS` against
        // goals.db and is always rolled back, so a deferred `BEGIN` is enough. A
        // `BEGIN IMMEDIATE` would take a goals.db write lock and hold it across the
        // state.db commit for no benefit. Lock order is consistently state -> goals
        // at every site that touches both, so there is no inversion to guard against.
        let mut goal_tx = if goal_hold_can_pause {
            Some(self.goals_pool.begin().await?)
        } else {
            None
        };
        let pause_for_goal_hold = match (expected_goal_id, goal_hold_can_pause, goal_tx.as_mut()) {
            (Some(expected_goal_id), true, Some(goal_tx)) => {
                sqlx::query_scalar::<_, bool>(
                    r#"
SELECT EXISTS(
    SELECT 1
    FROM thread_goals
    WHERE thread_id = ?
      AND goal_id = ?
      AND status IN ('paused', 'blocked', 'usage_limited', 'budget_limited')
)
                    "#,
                )
                .bind(thread_id)
                .bind(expected_goal_id)
                .fetch_one(&mut **goal_tx)
                .await?
            }
            (Some(_), false, None) | (None, false, None) => false,
            // `goal_hold_can_pause` is what decides whether `goal_tx` was opened, so
            // the arms above are exhaustive in practice. Fail the write instead of
            // panicking out of a state-store transaction if that ever drifts.
            (expected_goal_id, goal_hold_can_pause, goal_tx) => {
                anyhow::bail!(
                    "goal transaction presence does not match the recurring goal schedule invariant (expected_goal_id={}, goal_hold_can_pause={goal_hold_can_pause}, goal_tx={})",
                    expected_goal_id.is_some(),
                    goal_tx.is_some(),
                );
            }
        };
        let failed = run_status == crate::ThreadScheduleRunStatus::Failed.as_str();
        // The only thing that pauses a schedule at finish time is a goal hold; there
        // is deliberately no caller-supplied pause flag.
        let pause_schedule = pause_for_goal_hold;
        let schedule_result = sqlx::query(
            r#"
UPDATE thread_schedules
SET
    status = CASE
        WHEN status = 'expired' THEN 'expired'
        WHEN expires_at_ms IS NOT NULL AND ? >= expires_at_ms THEN 'expired'
        WHEN status = 'paused' THEN 'paused'
        WHEN ? THEN 'paused'
        WHEN ? IS NULL THEN 'expired'
        ELSE status
    END,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    last_run_at_ms = ?,
    next_run_at_ms = CASE
        WHEN status IN ('expired', 'paused') THEN NULL
        WHEN expires_at_ms IS NOT NULL AND ? >= expires_at_ms THEN NULL
        WHEN ? THEN NULL
        WHEN ? IS NULL THEN NULL
        ELSE ?
    END,
    failure_count = CASE WHEN ? THEN failure_count + 1 ELSE 0 END,
    updated_at_ms = ?
WHERE schedule_id = ? AND lease_id = ?
            "#,
        )
        .bind(completed_at_ms)
        .bind(pause_schedule)
        .bind(next_run_at_ms)
        .bind(completed_at_ms)
        .bind(completed_at_ms)
        .bind(pause_schedule)
        .bind(next_run_at_ms)
        .bind(next_run_at_ms)
        .bind(failed)
        .bind(completed_at_ms)
        .bind(schedule_id)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if schedule_result.rows_affected() == 0 {
            tx.commit().await?;
            if let Some(goal_tx) = goal_tx {
                let _ = goal_tx.rollback().await;
            }
            return Ok(false);
        }
        sqlx::query("DELETE FROM thread_schedule_occurrences WHERE occurrence_id = ?")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        if let Some(goal_tx) = goal_tx {
            let _ = goal_tx.rollback().await;
        }
        Ok(true)
    }
}
