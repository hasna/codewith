//! Enqueue and durable start transitions for scheduled occurrences.

use super::*;

impl ScheduleStore {
    pub async fn enqueue_thread_schedule_run(
        &self,
        params: ThreadScheduleRunEnqueueParams<'_>,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        let ThreadScheduleRunEnqueueParams {
            schedule_id,
            run_id,
            lease_id,
            goal_id,
            auth_profile_recorded,
            auth_profile,
            turn_input,
            now,
        } = params;
        let now_ms = datetime_to_epoch_millis(now);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            r#"
UPDATE thread_schedule_occurrences
SET state = 'enqueued',
    goal_id = COALESCE(goal_id, ?),
    auth_profile_recorded = CASE WHEN ? THEN 1 ELSE auth_profile_recorded END,
    auth_profile = CASE WHEN ? THEN ? ELSE auth_profile END,
    turn_input = ?,
    retry_at_ms = NULL,
    updated_at_ms = ?
WHERE occurrence_id = ?
  AND schedule_id = ?
  AND state IN ('waiting_idle', 'enqueued')
  AND EXISTS (
      SELECT 1
      FROM thread_schedules
      WHERE thread_schedules.schedule_id = thread_schedule_occurrences.schedule_id
        AND thread_schedules.lease_id = ?
        AND thread_schedules.lease_expires_at_ms > ?
  )
            "#,
        )
        .bind(goal_id)
        .bind(auth_profile_recorded)
        .bind(auth_profile_recorded)
        .bind(auth_profile)
        .bind(redact_state_string(turn_input))
        .bind(now_ms)
        .bind(run_id)
        .bind(schedule_id)
        .bind(lease_id)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }
        let occurrence = sqlx::query_as::<_, ThreadScheduleOccurrenceRow>(
            r#"
SELECT occurrence_id, schedule_id, thread_id, state, turn_id, goal_id,
       auth_profile_recorded, auth_profile, scheduled_for_ms, turn_input, created_at_ms
FROM thread_schedule_occurrences
WHERE occurrence_id = ?
            "#,
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
        let run = Self::occurrence_run(&mut tx, &occurrence, lease_id).await?;
        tx.commit().await?;
        Ok(Some(run))
    }

    pub async fn mark_thread_schedule_run_started(
        &self,
        params: ThreadScheduleRunStartParams<'_>,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        crate::busy_retry::retry_on_busy("mark thread schedule run started", || {
            self.mark_thread_schedule_run_started_once(params.clone())
        })
        .await
    }

    async fn mark_thread_schedule_run_started_once(
        &self,
        params: ThreadScheduleRunStartParams<'_>,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        let ThreadScheduleRunStartParams {
            schedule_id,
            run_id,
            lease_id,
            turn_id,
            goal_id,
            now,
            lease_duration,
        } = params;
        let now_ms = datetime_to_epoch_millis(now);
        let lease_expires_at_ms =
            datetime_to_epoch_millis(now + chrono::Duration::from_std(lease_duration)?);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let schedule_result = sqlx::query(
            r#"
UPDATE thread_schedules
SET lease_expires_at_ms = MAX(lease_expires_at_ms, ?),
    updated_at_ms = ?
WHERE schedule_id = ?
  AND lease_id = ?
  AND lease_expires_at_ms > ?
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_occurrences
      WHERE thread_schedule_occurrences.schedule_id = thread_schedules.schedule_id
        AND thread_schedule_occurrences.occurrence_id = ?
        AND thread_schedule_occurrences.turn_id = ?
        AND thread_schedule_occurrences.state IN ('enqueued', 'started')
  )
            "#,
        )
        .bind(lease_expires_at_ms)
        .bind(now_ms)
        .bind(schedule_id)
        .bind(lease_id)
        .bind(now_ms)
        .bind(run_id)
        .bind(turn_id)
        .execute(&mut *tx)
        .await?;
        if schedule_result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }
        let occurrence_result = sqlx::query(
            r#"
UPDATE thread_schedule_occurrences
SET state = 'started', goal_id = COALESCE(goal_id, ?), updated_at_ms = ?
WHERE schedule_id = ?
  AND occurrence_id = ?
  AND turn_id = ?
  AND state IN ('enqueued', 'started')
"#,
        )
        .bind(goal_id)
        .bind(now_ms)
        .bind(schedule_id)
        .bind(run_id)
        .bind(turn_id)
        .execute(&mut *tx)
        .await?;
        if occurrence_result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
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
    scheduled_for_ms,
    started_at_ms
)
SELECT
    occurrence_id,
    schedule_id,
    thread_id,
    'running',
    ?,
    turn_id,
    goal_id,
    scheduled_for_ms,
    ?
FROM thread_schedule_occurrences
WHERE occurrence_id = ? AND state = 'started'
ON CONFLICT(run_id) DO UPDATE SET
    lease_id = excluded.lease_id,
    goal_id = COALESCE(thread_schedule_runs.goal_id, excluded.goal_id)
WHERE thread_schedule_runs.status = 'running'
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(lease_id)
            .bind(now_ms)
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
        let run = thread_schedule_run_from_row(&row)?;
        tx.commit().await?;
        Ok(Some(run))
    }

    pub async fn extend_thread_schedule_lease(
        &self,
        params: ThreadScheduleRunLeaseParams<'_>,
    ) -> anyhow::Result<bool> {
        let ThreadScheduleRunLeaseParams {
            schedule_id,
            run_id,
            lease_id,
            now,
            lease_duration,
        } = params;
        let now_ms = datetime_to_epoch_millis(now);
        let lease_expires_at = now + chrono::Duration::from_std(lease_duration)?;
        let result = sqlx::query(
            r#"
UPDATE thread_schedules
SET lease_expires_at_ms = ?, updated_at_ms = ?
WHERE schedule_id = ?
  AND status = 'active'
  AND lease_id = ?
  AND lease_expires_at_ms > ?
  AND (expires_at_ms IS NULL OR expires_at_ms > ?)
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_occurrences
      WHERE thread_schedule_occurrences.schedule_id = thread_schedules.schedule_id
        AND thread_schedule_occurrences.occurrence_id = ?
        AND thread_schedule_occurrences.state IN ('enqueued', 'started')
        AND (
            thread_schedule_occurrences.state = 'enqueued'
            OR EXISTS (
                SELECT 1
                FROM thread_schedule_runs
                WHERE thread_schedule_runs.run_id = thread_schedule_occurrences.occurrence_id
                  AND thread_schedule_runs.lease_id = ?
                  AND thread_schedule_runs.status = 'running'
            )
        )
  )
            "#,
        )
        .bind(datetime_to_epoch_millis(lease_expires_at))
        .bind(now_ms)
        .bind(schedule_id)
        .bind(lease_id)
        .bind(now_ms)
        .bind(now_ms)
        .bind(run_id)
        .bind(lease_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }
}
