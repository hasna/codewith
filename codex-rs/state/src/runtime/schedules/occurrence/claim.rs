//! Claim and restart recovery for pending scheduled occurrences.

use super::*;

impl ScheduleStore {
    pub async fn get_thread_schedule_run(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        let sql = run_returning(
            r#"
SELECT
            "#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{sql}FROM thread_schedule_runs WHERE run_id = ?"
        )))
        .bind(run_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(|row| thread_schedule_run_from_row(&row))
            .transpose()
    }

    pub async fn get_running_thread_schedule_run_for_turn(
        &self,
        thread_id: ThreadId,
        turn_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        let sql = run_returning(
            r#"
SELECT
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            r#"{sql}
FROM thread_schedule_runs
WHERE thread_id = ?
  AND turn_id = ?
  AND status = 'running'
ORDER BY started_at_ms DESC
LIMIT 1
"#
        )))
        .bind(thread_id.to_string())
        .bind(turn_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(|row| thread_schedule_run_from_row(&row))
            .transpose()
    }

    pub async fn get_thread_schedule_stats(
        &self,
        schedule_id: &str,
    ) -> anyhow::Result<crate::ThreadScheduleStats> {
        let row = sqlx::query(
            r#"
SELECT
    COALESCE(SUM(CASE WHEN deferral_kind IS NULL OR deferral_kind != 'idle' THEN 1 ELSE 0 END), 0) AS total_runs,
    COALESCE(SUM(CASE WHEN status = 'leased' THEN 1 ELSE 0 END), 0) AS leased_runs,
    COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0) AS running_runs,
    COALESCE(SUM(CASE WHEN status = 'deferred' AND deferral_kind = 'capacity' THEN 1 ELSE 0 END), 0) AS deferred_runs,
    COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) AS completed_runs,
    COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) AS failed_runs,
    MAX(CASE WHEN deferral_kind IS NULL OR deferral_kind != 'idle' THEN started_at_ms END) AS last_started_at_ms,
    -- Only successfully completed runs contribute to last_completed_at. The
    -- completed_at_ms column is also written for deferred and failed runs (it is
    -- really a "finished at" timestamp), so deriving last_completed_at from the
    -- raw MAX would populate it even when completed_runs is 0. Keeping this
    -- filtered ensures last_completed_at is non-null iff completed_runs > 0.
    MAX(CASE WHEN status = 'completed' THEN completed_at_ms END) AS last_completed_at_ms
FROM thread_schedule_runs
WHERE schedule_id = ?
            "#,
        )
        .bind(schedule_id)
        .fetch_one(self.pool.as_ref())
        .await?;
        let last_error = sqlx::query_scalar(
            r#"
SELECT error
FROM thread_schedule_runs
WHERE schedule_id = ?
  AND status = 'failed'
  AND error IS NOT NULL
  AND TRIM(error) != ''
ORDER BY completed_at_ms DESC, started_at_ms DESC
LIMIT 1
            "#,
        )
        .bind(schedule_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(crate::ThreadScheduleStats {
            total_runs: row.try_get("total_runs")?,
            leased_runs: row.try_get("leased_runs")?,
            running_runs: row.try_get("running_runs")?,
            deferred_runs: row.try_get("deferred_runs")?,
            completed_runs: row.try_get("completed_runs")?,
            failed_runs: row.try_get("failed_runs")?,
            last_started_at: row
                .try_get::<Option<i64>, _>("last_started_at_ms")?
                .map(epoch_millis_to_datetime)
                .transpose()?,
            last_completed_at: row
                .try_get::<Option<i64>, _>("last_completed_at_ms")?
                .map(epoch_millis_to_datetime)
                .transpose()?,
            last_error,
        })
    }

    pub async fn claim_due_thread_schedule(
        &self,
        now: DateTime<Utc>,
        lease_id: &str,
        lease_duration: Duration,
    ) -> anyhow::Result<Option<ThreadScheduleClaim>> {
        self.claim_due_thread_schedule_with_params(ThreadScheduleDueClaimParams {
            now,
            lease_id,
            lease_duration,
            local_active_owner_id: None,
            local_active_fresh_after: None,
        })
        .await
    }

    pub async fn claim_due_thread_schedule_with_params(
        &self,
        params: ThreadScheduleDueClaimParams<'_>,
    ) -> anyhow::Result<Option<ThreadScheduleClaim>> {
        let ThreadScheduleDueClaimParams {
            now,
            lease_id,
            lease_duration,
            local_active_owner_id,
            local_active_fresh_after,
        } = params;
        let params = ClaimThreadScheduleParams {
            target: ThreadScheduleClaimTarget::Due,
            now,
            lease_id,
            lease_duration,
            local_active_owner_id,
            local_active_fresh_after,
        };
        crate::busy_retry::retry_on_busy("claim due thread schedule", || {
            self.claim_thread_schedule_once(params.clone())
        })
        .await
    }

    pub async fn claim_thread_schedule_now(
        &self,
        schedule_id: &str,
        now: DateTime<Utc>,
        lease_id: &str,
        lease_duration: Duration,
    ) -> anyhow::Result<Option<ThreadScheduleClaim>> {
        self.claim_thread_schedule_now_with_params(ThreadScheduleNowClaimParams {
            schedule_id,
            now,
            lease_id,
            lease_duration,
            local_active_owner_id: None,
            local_active_fresh_after: None,
        })
        .await
    }

    pub async fn claim_thread_schedule_now_with_params(
        &self,
        params: ThreadScheduleNowClaimParams<'_>,
    ) -> anyhow::Result<Option<ThreadScheduleClaim>> {
        let ThreadScheduleNowClaimParams {
            schedule_id,
            now,
            lease_id,
            lease_duration,
            local_active_owner_id,
            local_active_fresh_after,
        } = params;
        let params = ClaimThreadScheduleParams {
            target: ThreadScheduleClaimTarget::Now { schedule_id },
            now,
            lease_id,
            lease_duration,
            local_active_owner_id,
            local_active_fresh_after,
        };
        crate::busy_retry::retry_on_busy("claim thread schedule now", || {
            self.claim_thread_schedule_once(params.clone())
        })
        .await
    }

    async fn claim_thread_schedule_once(
        &self,
        params: ClaimThreadScheduleParams<'_>,
    ) -> anyhow::Result<Option<ThreadScheduleClaim>> {
        let ClaimThreadScheduleParams {
            target,
            now,
            lease_id,
            lease_duration,
            local_active_owner_id,
            local_active_fresh_after,
        } = params;
        let now_ms = datetime_to_epoch_millis(now);
        let lease_expires_at = now + chrono::Duration::from_std(lease_duration)?;
        let lease_expires_at_ms = datetime_to_epoch_millis(lease_expires_at);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let owner_filter = match (local_active_owner_id, local_active_fresh_after) {
            (Some(owner_id), Some(fresh_after)) => {
                Some((owner_id, datetime_to_epoch_millis(fresh_after)))
            }
            _ => None,
        };
        let owner_scoped_lease_id = owner_filter.as_ref().map(|_| format!("owner:{lease_id}"));
        let lease_id = owner_scoped_lease_id.as_deref().unwrap_or(lease_id);
        let active_owner_filter = if owner_filter.is_some() {
            r#"
  AND NOT EXISTS (
    SELECT 1
    FROM local_active_sessions
    WHERE local_active_sessions.thread_id = thread_schedules.thread_id
      AND local_active_sessions.last_seen_at_ms >= ?
      AND local_active_sessions.owner_id != ?
  )
"#
        } else {
            ""
        };
        let sql = match target {
            ThreadScheduleClaimTarget::Due => format!(
                r#"
SELECT {SCHEDULE_COLUMNS}
FROM thread_schedules
WHERE status = 'active'
  AND next_run_at_ms IS NOT NULL
  AND next_run_at_ms <= ?
  AND (expires_at_ms IS NULL OR expires_at_ms > ?)
  AND (lease_id IS NULL OR lease_expires_at_ms <= ?)
  AND NOT EXISTS (
      SELECT 1
      FROM thread_schedule_occurrences
      WHERE thread_schedule_occurrences.schedule_id = thread_schedules.schedule_id
        AND thread_schedule_occurrences.state IN ('waiting_idle', 'enqueued', 'started')
        AND thread_schedule_occurrences.retry_at_ms > ?
  )
{active_owner_filter}
ORDER BY next_run_at_ms, created_at_ms
LIMIT 1
"#
            ),
            ThreadScheduleClaimTarget::Now { .. } => format!(
                r#"
SELECT {SCHEDULE_COLUMNS}
FROM thread_schedules
WHERE schedule_id = ?
  AND status = 'active'
  AND (expires_at_ms IS NULL OR expires_at_ms > ?)
  AND (lease_id IS NULL OR lease_expires_at_ms <= ?)
{active_owner_filter}
"#
            ),
        };
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        query = match target {
            ThreadScheduleClaimTarget::Due => {
                query.bind(now_ms).bind(now_ms).bind(now_ms).bind(now_ms)
            }
            ThreadScheduleClaimTarget::Now { schedule_id } => {
                query.bind(schedule_id).bind(now_ms).bind(now_ms)
            }
        };
        if let Some((owner_id, fresh_after_ms)) = owner_filter {
            query = query.bind(fresh_after_ms).bind(owner_id);
        }
        let schedule_row = query.fetch_optional(&mut *tx).await?;
        let Some(schedule_row) = schedule_row else {
            tx.commit().await?;
            return Ok(None);
        };
        let selected_schedule = thread_schedule_from_row(&schedule_row)?;
        let existing_occurrence = sqlx::query_as::<_, ThreadScheduleOccurrenceRow>(
            r#"
SELECT
    occurrence_id,
    schedule_id,
    thread_id,
    state,
    turn_id,
    goal_id,
    auth_profile_recorded,
    auth_profile,
    scheduled_for_ms,
    turn_input,
    created_at_ms
FROM thread_schedule_occurrences
WHERE schedule_id = ?
            "#,
        )
        .bind(selected_schedule.schedule_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let goal_id = existing_occurrence
            .as_ref()
            .and_then(|occurrence| occurrence.goal_id.as_deref());
        let goal_hold_can_pause =
            goal_id.is_some() && selected_schedule.schedule != crate::ThreadScheduleSpec::Once;
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
        let mut pause_for_goal_hold = false;
        if let (Some(goal_tx), Some(goal_id)) = (goal_tx.as_mut(), goal_id) {
            pause_for_goal_hold = sqlx::query_scalar::<_, bool>(
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
            .bind(selected_schedule.thread_id.to_string())
            .bind(goal_id)
            .fetch_one(&mut **goal_tx)
            .await?;
        }
        if pause_for_goal_hold
            && existing_occurrence.as_ref().is_none_or(|occurrence| {
                occurrence.state != OCCURRENCE_STARTED && occurrence.state != OCCURRENCE_TERMINAL
            })
        {
            if let Some(occurrence) = existing_occurrence.as_ref() {
                let error = redact_state_string("scheduled run stopped because its goal is held");
                if occurrence.state == OCCURRENCE_ENQUEUED {
                    sqlx::query(
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
    goal_id,
    ?,
    scheduled_for_ms,
    created_at_ms,
    ?
FROM thread_schedule_occurrences
WHERE occurrence_id = ?
                        "#,
                    )
                    .bind(lease_id)
                    .bind(error)
                    .bind(now_ms)
                    .bind(occurrence.occurrence_id.as_str())
                    .execute(&mut *tx)
                    .await?;
                }
            }
            sqlx::query("DELETE FROM thread_schedule_occurrences WHERE schedule_id = ?")
                .bind(selected_schedule.schedule_id.as_str())
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                r#"
UPDATE thread_schedules
SET status = 'paused',
    next_run_at_ms = NULL,
    last_run_at_ms = ?,
    failure_count = failure_count + 1,
    lease_id = NULL,
    lease_expires_at_ms = NULL,
    updated_at_ms = ?
WHERE schedule_id = ? AND status = 'active'
                "#,
            )
            .bind(now_ms)
            .bind(now_ms)
            .bind(selected_schedule.schedule_id.as_str())
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            if let Some(goal_tx) = goal_tx {
                let _ = goal_tx.rollback().await;
            }
            return Ok(None);
        }
        let sql = schedule_returning(
            r#"
        UPDATE thread_schedules
SET lease_id = ?,
    lease_expires_at_ms = ?,
    updated_at_ms = ?
WHERE schedule_id = ? AND status = 'active'
RETURNING
"#,
        );
        let schedule_row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(lease_id)
            .bind(lease_expires_at_ms)
            .bind(now_ms)
            .bind(selected_schedule.schedule_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
        let Some(schedule_row) = schedule_row else {
            // `thread_schedules_ignore_legacy_live_owner_claim` silently drops
            // the lease update (RAISE(IGNORE)) when a legacy, non-owner-scoped
            // lease is claimed while a local session is live. Treat that as an
            // unclaimed schedule and discard the speculative reap above so the
            // live owner keeps ownership of its runs.
            tx.rollback().await?;
            if let Some(goal_tx) = goal_tx {
                let _ = goal_tx.rollback().await;
            }
            return Ok(None);
        };
        let schedule = thread_schedule_from_row(&schedule_row)?;
        let occurrence = match existing_occurrence {
            Some(occurrence) => {
                sqlx::query(
                    r#"
UPDATE thread_schedule_occurrences
SET state = ?, updated_at_ms = ?
WHERE occurrence_id = ?
                    "#,
                )
                .bind(occurrence.state.as_str())
                .bind(now_ms)
                .bind(occurrence.occurrence_id.as_str())
                .execute(&mut *tx)
                .await?;
                occurrence
            }
            None => {
                let occurrence_id = Uuid::new_v4().to_string();
                let turn_id = Uuid::now_v7().to_string();
                let scheduled_for_ms = match target {
                    ThreadScheduleClaimTarget::Due => {
                        selected_schedule.next_run_at.map(datetime_to_epoch_millis)
                    }
                    ThreadScheduleClaimTarget::Now { .. } => Some(now_ms),
                };
                sqlx::query(
                    r#"
INSERT INTO thread_schedule_occurrences (
    occurrence_id,
    schedule_id,
    thread_id,
    state,
    turn_id,
    scheduled_for_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, 'waiting_idle', ?, ?, ?, ?)
                    "#,
                )
                .bind(occurrence_id.as_str())
                .bind(schedule.schedule_id.as_str())
                .bind(schedule.thread_id.to_string())
                .bind(turn_id.as_str())
                .bind(scheduled_for_ms)
                .bind(now_ms)
                .bind(now_ms)
                .execute(&mut *tx)
                .await?;
                ThreadScheduleOccurrenceRow {
                    occurrence_id,
                    schedule_id: schedule.schedule_id.clone(),
                    thread_id: schedule.thread_id.to_string(),
                    state: OCCURRENCE_WAITING_IDLE.to_string(),
                    turn_id,
                    goal_id: None,
                    auth_profile_recorded: false,
                    auth_profile: None,
                    scheduled_for_ms,
                    turn_input: None,
                    created_at_ms: now_ms,
                }
            }
        };
        if occurrence.state == OCCURRENCE_STARTED || occurrence.state == OCCURRENCE_TERMINAL {
            sqlx::query("UPDATE thread_schedule_runs SET lease_id = ? WHERE run_id = ?")
                .bind(lease_id)
                .bind(occurrence.occurrence_id.as_str())
                .execute(&mut *tx)
                .await?;
        }
        let run = Self::occurrence_run(&mut tx, &occurrence, lease_id).await?;
        tx.commit().await?;
        if let Some(goal_tx) = goal_tx {
            let _ = goal_tx.rollback().await;
        }
        let occurrence_state = ThreadScheduleOccurrenceState::from_str(&occurrence.state)?;
        let occurrence_auth_profile = occurrence
            .auth_profile_recorded
            .then(|| occurrence.auth_profile.clone());
        Ok(Some(ThreadScheduleClaim {
            schedule,
            run,
            occurrence_state,
            turn_input: occurrence.turn_input,
            occurrence_auth_profile,
        }))
    }

    pub(super) async fn occurrence_run(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        occurrence: &ThreadScheduleOccurrenceRow,
        lease_id: &str,
    ) -> anyhow::Result<crate::ThreadScheduleRun> {
        if occurrence.state == OCCURRENCE_STARTED || occurrence.state == OCCURRENCE_TERMINAL {
            let sql = run_returning("SELECT");
            let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "{sql} FROM thread_schedule_runs WHERE run_id = ?"
            )))
            .bind(occurrence.occurrence_id.as_str())
            .fetch_one(&mut **tx)
            .await?;
            return thread_schedule_run_from_row(&row);
        }
        Ok(crate::ThreadScheduleRun {
            thread_id: ThreadId::try_from(occurrence.thread_id.clone())?,
            schedule_id: occurrence.schedule_id.clone(),
            run_id: occurrence.occurrence_id.clone(),
            status: crate::ThreadScheduleRunStatus::Leased,
            lease_id: lease_id.to_string(),
            turn_id: Some(occurrence.turn_id.clone()),
            goal_id: occurrence.goal_id.clone(),
            error: None,
            scheduled_for: occurrence
                .scheduled_for_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
            started_at: epoch_millis_to_datetime(occurrence.created_at_ms)?,
            completed_at: None,
        })
    }
}
