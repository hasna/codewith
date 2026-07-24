use super::*;
use crate::model::ThreadScheduleRow;
use crate::model::ThreadScheduleRunRow;
use uuid::Uuid;

pub const MAX_THREAD_SCHEDULE_NESTING_DEPTH: i64 = 5;
const DYNAMIC_LOOP_CADENCE_SECONDS: i64 = 60;
const ONCE_SCHEDULE_KIND: &str = "once";
const PAUSED_SCHEDULE_RUN_ERROR: &str = "scheduled run cancelled because schedule was paused";
const EXPIRED_SCHEDULE_RUN_ERROR: &str = "scheduled run cancelled because schedule expired";

#[derive(Clone)]
pub struct ScheduleStore {
    pool: Arc<SqlitePool>,
    goals_pool: Arc<SqlitePool>,
}

impl ScheduleStore {
    pub(crate) fn new(pool: Arc<SqlitePool>, goals_pool: Arc<SqlitePool>) -> Self {
        Self { pool, goals_pool }
    }
}

pub struct ThreadScheduleCreateParams {
    pub thread_id: ThreadId,
    pub prompt: String,
    pub prompt_source: crate::ThreadSchedulePromptSource,
    pub schedule: crate::ThreadScheduleSpec,
    pub timezone: String,
    pub status: crate::ThreadScheduleStatus,
    pub next_run_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct ThreadScheduleUpdate {
    pub prompt: Option<String>,
    pub prompt_source: Option<crate::ThreadSchedulePromptSource>,
    pub schedule: Option<crate::ThreadScheduleSpec>,
    pub timezone: Option<String>,
    pub status: Option<crate::ThreadScheduleStatus>,
    pub next_run_at: Option<Option<DateTime<Utc>>>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Clone)]
pub struct ThreadScheduleClaim {
    pub schedule: crate::ThreadSchedule,
    pub run: crate::ThreadScheduleRun,
}

#[derive(Clone)]
pub struct ThreadScheduleDueClaimParams<'a> {
    pub now: DateTime<Utc>,
    pub lease_id: &'a str,
    pub lease_duration: Duration,
    pub local_active_owner_id: Option<&'a str>,
    pub local_active_fresh_after: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct ThreadScheduleNowClaimParams<'a> {
    pub schedule_id: &'a str,
    pub now: DateTime<Utc>,
    pub lease_id: &'a str,
    pub lease_duration: Duration,
    pub local_active_owner_id: Option<&'a str>,
    pub local_active_fresh_after: Option<DateTime<Utc>>,
}

pub struct ThreadScheduleRunForGoalFinishParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub completed_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub expected_goal_id: &'a str,
}

#[derive(Clone)]
pub struct ThreadScheduleRunStartParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub turn_id: &'a str,
    pub goal_id: Option<&'a str>,
    pub now: DateTime<Utc>,
    pub lease_duration: Duration,
}

#[derive(Clone)]
pub struct ThreadScheduleRunLeaseParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub now: DateTime<Utc>,
    pub lease_duration: Duration,
}

struct ScheduleNesting {
    parent_schedule_id: Option<String>,
    nesting_depth: i64,
}

#[derive(Clone, Copy)]
enum ThreadScheduleClaimTarget<'a> {
    Due,
    Now { schedule_id: &'a str },
}

#[derive(Clone)]
struct ClaimThreadScheduleParams<'a> {
    target: ThreadScheduleClaimTarget<'a>,
    now: DateTime<Utc>,
    lease_id: &'a str,
    lease_duration: Duration,
    local_active_owner_id: Option<&'a str>,
    local_active_fresh_after: Option<DateTime<Utc>>,
}

impl ScheduleStore {
    pub async fn create_thread_schedule(
        &self,
        params: ThreadScheduleCreateParams,
    ) -> anyhow::Result<crate::ThreadSchedule> {
        self.create_thread_schedule_with_recorded_auth_profile(
            params, /*parent_schedule_id*/ None, /*auth_profile*/ None,
        )
        .await
    }

    pub async fn create_thread_schedule_for_auth_profile(
        &self,
        params: ThreadScheduleCreateParams,
        auth_profile: Option<String>,
    ) -> anyhow::Result<crate::ThreadSchedule> {
        self.create_thread_schedule_with_recorded_auth_profile(
            params,
            /*parent_schedule_id*/ None,
            Some(auth_profile),
        )
        .await
    }

    pub async fn create_nested_thread_schedule(
        &self,
        params: ThreadScheduleCreateParams,
        parent_schedule_id: String,
    ) -> anyhow::Result<crate::ThreadSchedule> {
        self.create_thread_schedule_with_recorded_auth_profile(
            params,
            Some(parent_schedule_id),
            /*auth_profile*/ None,
        )
        .await
    }

    pub async fn create_nested_thread_schedule_for_auth_profile(
        &self,
        params: ThreadScheduleCreateParams,
        parent_schedule_id: String,
        auth_profile: Option<String>,
    ) -> anyhow::Result<crate::ThreadSchedule> {
        self.create_thread_schedule_with_recorded_auth_profile(
            params,
            Some(parent_schedule_id),
            Some(auth_profile),
        )
        .await
    }

    async fn create_thread_schedule_with_recorded_auth_profile(
        &self,
        params: ThreadScheduleCreateParams,
        parent_schedule_id: Option<String>,
        auth_profile: Option<Option<String>>,
    ) -> anyhow::Result<crate::ThreadSchedule> {
        let nesting = self
            .validate_schedule_create_nesting(&params, parent_schedule_id)
            .await?;
        let schedule_id = Uuid::new_v4().to_string();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let spec = schedule_bindings(&params.schedule);
        let cron_expression = spec.cron_expression.map(redact_state_string);
        let auth_profile_recorded = auth_profile.is_some();
        let auth_profile = auth_profile.flatten().map(redact_state_string);
        let prompt = redact_state_string(params.prompt);
        let timezone = redact_state_string(params.timezone);
        let sql = schedule_returning(
            r#"
INSERT INTO thread_schedules (
    schedule_id,
    thread_id,
    parent_schedule_id,
    nesting_depth,
    prompt_source,
    prompt,
    schedule_kind,
    interval_amount,
    interval_unit,
    cron_expression,
    timezone,
    auth_profile_recorded,
    auth_profile,
    status,
    next_run_at_ms,
    expires_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(schedule_id)
            .bind(params.thread_id.to_string())
            .bind(nesting.parent_schedule_id)
            .bind(nesting.nesting_depth)
            .bind(params.prompt_source.as_str())
            .bind(prompt)
            .bind(spec.kind)
            .bind(spec.interval_amount)
            .bind(spec.interval_unit)
            .bind(cron_expression)
            .bind(timezone)
            .bind(if auth_profile_recorded { 1_i64 } else { 0_i64 })
            .bind(auth_profile)
            .bind(params.status.as_str())
            .bind(params.next_run_at.map(datetime_to_epoch_millis))
            .bind(params.expires_at.map(datetime_to_epoch_millis))
            .bind(now_ms)
            .bind(now_ms)
            .fetch_one(self.pool.as_ref())
            .await?;
        thread_schedule_from_row(&row)
    }

    pub async fn get_thread_schedule(
        &self,
        schedule_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadSchedule>> {
        let sql = schedule_select_by_id(
            r#"
SELECT
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(schedule_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| thread_schedule_from_row(&row)).transpose()
    }

    pub async fn list_thread_schedules(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Vec<crate::ThreadSchedule>> {
        let rows = sqlx::query(
            r#"
SELECT
    schedule_id,
    thread_id,
    parent_schedule_id,
    nesting_depth,
    prompt_source,
    prompt,
    schedule_kind,
    interval_amount,
    interval_unit,
    cron_expression,
    timezone,
    auth_profile_recorded,
    auth_profile,
    status,
    next_run_at_ms,
    last_run_at_ms,
    expires_at_ms,
    failure_count,
    lease_id,
    lease_expires_at_ms,
    created_at_ms,
    updated_at_ms
FROM thread_schedules
WHERE thread_id = ?
ORDER BY status, next_run_at_ms IS NULL, next_run_at_ms, created_at_ms
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.iter().map(thread_schedule_from_row).collect()
    }

    pub async fn update_thread_schedule(
        &self,
        schedule_id: &str,
        update: ThreadScheduleUpdate,
    ) -> anyhow::Result<Option<crate::ThreadSchedule>> {
        let Some(existing) = self.get_thread_schedule(schedule_id).await? else {
            return Ok(None);
        };
        if let Some(schedule) = update
            .schedule
            .as_ref()
            .filter(|schedule| *schedule != &existing.schedule)
        {
            self.validate_schedule_update_nesting(&existing, schedule)
                .await?;
        }
        let prompt = update.prompt.unwrap_or(existing.prompt);
        let prompt_source = update.prompt_source.unwrap_or(existing.prompt_source);
        let schedule = update.schedule.unwrap_or(existing.schedule);
        let timezone = update.timezone.unwrap_or(existing.timezone);
        let reset_failure_count =
            matches!(update.status, Some(crate::ThreadScheduleStatus::Active));
        let status = update.status.unwrap_or(existing.status);
        let next_run_at = update.next_run_at.unwrap_or(existing.next_run_at);
        let expires_at = update.expires_at.unwrap_or(existing.expires_at);
        let spec = schedule_bindings(&schedule);
        let prompt = redact_state_string(prompt);
        let timezone = redact_state_string(timezone);
        let cron_expression = spec.cron_expression.map(redact_state_string);
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let sql = schedule_returning(
            r#"
UPDATE thread_schedules
SET
    prompt = ?,
    prompt_source = ?,
    schedule_kind = ?,
    interval_amount = ?,
    interval_unit = ?,
    cron_expression = ?,
    timezone = ?,
    status = ?,
    next_run_at_ms = CASE WHEN ? = 'expired' THEN NULL ELSE ? END,
    expires_at_ms = ?,
    failure_count = CASE WHEN ? THEN 0 ELSE failure_count END,
    lease_id = CASE WHEN ? = 'active' THEN lease_id ELSE NULL END,
    lease_expires_at_ms = CASE WHEN ? = 'active' THEN lease_expires_at_ms ELSE NULL END,
    updated_at_ms = ?
WHERE schedule_id = ?
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(prompt)
            .bind(prompt_source.as_str())
            .bind(spec.kind)
            .bind(spec.interval_amount)
            .bind(spec.interval_unit)
            .bind(cron_expression)
            .bind(timezone)
            .bind(status.as_str())
            .bind(status.as_str())
            .bind(next_run_at.map(datetime_to_epoch_millis))
            .bind(expires_at.map(datetime_to_epoch_millis))
            .bind(reset_failure_count)
            .bind(status.as_str())
            .bind(status.as_str())
            .bind(now_ms)
            .bind(schedule_id)
            .fetch_optional(&mut *tx)
            .await?;
        if row.is_some() {
            let terminal_error = match status {
                crate::ThreadScheduleStatus::Active => None,
                crate::ThreadScheduleStatus::Paused => Some(PAUSED_SCHEDULE_RUN_ERROR),
                crate::ThreadScheduleStatus::Expired => Some(EXPIRED_SCHEDULE_RUN_ERROR),
            };
            if let Some(terminal_error) = terminal_error {
                sqlx::query(
                    r#"
UPDATE thread_schedule_runs
SET status = 'failed',
    error = ?,
    completed_at_ms = COALESCE(completed_at_ms, ?)
WHERE schedule_id = ? AND status IN ('leased', 'running')
                    "#,
                )
                .bind(redact_state_string(terminal_error))
                .bind(now_ms)
                .bind(schedule_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        let schedule = row.map(|row| thread_schedule_from_row(&row)).transpose()?;
        tx.commit().await?;
        Ok(schedule)
    }

    pub async fn set_thread_schedule_status(
        &self,
        schedule_id: &str,
        status: crate::ThreadScheduleStatus,
    ) -> anyhow::Result<Option<crate::ThreadSchedule>> {
        self.update_thread_schedule(
            schedule_id,
            ThreadScheduleUpdate {
                prompt: None,
                prompt_source: None,
                schedule: None,
                timezone: None,
                status: Some(status),
                next_run_at: None,
                expires_at: None,
            },
        )
        .await
    }

    pub async fn resume_thread_schedule(
        &self,
        schedule_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadSchedule>> {
        self.resume_thread_schedule_with_next_run_at(schedule_id, /*next_run_at*/ None)
            .await
    }

    pub async fn resume_thread_schedule_at(
        &self,
        schedule_id: &str,
        next_run_at: DateTime<Utc>,
    ) -> anyhow::Result<Option<crate::ThreadSchedule>> {
        self.resume_thread_schedule_with_next_run_at(schedule_id, Some(next_run_at))
            .await
    }

    async fn resume_thread_schedule_with_next_run_at(
        &self,
        schedule_id: &str,
        next_run_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Option<crate::ThreadSchedule>> {
        let sql = schedule_returning(
            r#"
UPDATE thread_schedules
SET
    status = ?,
    next_run_at_ms = COALESCE(?, next_run_at_ms),
    failure_count = 0,
    updated_at_ms = ?
WHERE schedule_id = ?
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(crate::ThreadScheduleStatus::Active.as_str())
            .bind(next_run_at.map(datetime_to_epoch_millis))
            .bind(datetime_to_epoch_millis(Utc::now()))
            .bind(schedule_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| thread_schedule_from_row(&row)).transpose()
    }

    pub async fn delete_thread_schedule(&self, schedule_id: &str) -> anyhow::Result<bool> {
        Ok(!self
            .delete_thread_schedule_tree(schedule_id)
            .await?
            .is_empty())
    }

    pub async fn delete_thread_schedule_tree(
        &self,
        schedule_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;
        let deleted_schedule_ids = sqlx::query_scalar::<_, String>(
            r#"
WITH RECURSIVE subtree(schedule_id, nesting_depth) AS (
    SELECT schedule_id, nesting_depth
    FROM thread_schedules
    WHERE schedule_id = ?
    UNION ALL
    SELECT child.schedule_id, child.nesting_depth
    FROM thread_schedules child
    INNER JOIN subtree parent ON child.parent_schedule_id = parent.schedule_id
)
SELECT schedule_id
FROM subtree
ORDER BY nesting_depth DESC, schedule_id
            "#,
        )
        .bind(schedule_id)
        .fetch_all(&mut *tx)
        .await?;
        if deleted_schedule_ids.is_empty() {
            return Ok(Vec::new());
        }
        let result = sqlx::query(
            r#"
WITH RECURSIVE subtree(schedule_id) AS (
    SELECT schedule_id
    FROM thread_schedules
    WHERE schedule_id = ?
    UNION ALL
    SELECT child.schedule_id
    FROM thread_schedules child
    INNER JOIN subtree parent ON child.parent_schedule_id = parent.schedule_id
)
DELETE FROM thread_schedules
WHERE schedule_id IN (SELECT schedule_id FROM subtree)
            "#,
        )
        .bind(schedule_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(Vec::new());
        }
        tx.commit().await?;
        Ok(deleted_schedule_ids)
    }

    pub async fn list_thread_schedule_tree_ids(
        &self,
        schedule_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
WITH RECURSIVE schedule_tree(schedule_id, depth, created_at_ms) AS (
    SELECT schedule_id, 0, created_at_ms
    FROM thread_schedules
    WHERE schedule_id = ?
    UNION ALL
    SELECT child.schedule_id, parent.depth + 1, child.created_at_ms
    FROM thread_schedules AS child
    JOIN schedule_tree AS parent ON child.parent_schedule_id = parent.schedule_id
)
SELECT schedule_id
FROM schedule_tree
ORDER BY depth, created_at_ms, schedule_id
            "#,
        )
        .bind(schedule_id)
        .fetch_all(self.pool.as_ref())
        .await?;
        let ids = rows
            .iter()
            .map(|row| row.try_get::<String, _>("schedule_id"))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub async fn delete_thread_schedules_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<u64> {
        let result = sqlx::query("DELETE FROM thread_schedules WHERE thread_id = ?")
            .bind(thread_id.to_string())
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

    async fn validate_schedule_create_nesting(
        &self,
        params: &ThreadScheduleCreateParams,
        parent_schedule_id: Option<String>,
    ) -> anyhow::Result<ScheduleNesting> {
        let Some(parent_schedule_id) = parent_schedule_id else {
            return Ok(ScheduleNesting {
                parent_schedule_id: None,
                nesting_depth: 1,
            });
        };
        let parent_schedule_id = parent_schedule_id.trim();
        if parent_schedule_id.is_empty() {
            anyhow::bail!("invalid nested loop: parent schedule id cannot be empty");
        }
        if matches!(params.schedule, crate::ThreadScheduleSpec::Once) {
            anyhow::bail!("invalid nested loop: one-time schedules cannot be nested");
        }
        let parent = self
            .get_thread_schedule(parent_schedule_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid nested loop: parent schedule not found: {parent_schedule_id}"
                )
            })?;
        self.validate_parent_schedule(&parent, params.thread_id, &params.schedule)?;
        Ok(ScheduleNesting {
            parent_schedule_id: Some(parent.schedule_id),
            nesting_depth: parent.nesting_depth + 1,
        })
    }

    async fn validate_schedule_update_nesting(
        &self,
        existing: &crate::ThreadSchedule,
        schedule: &crate::ThreadScheduleSpec,
    ) -> anyhow::Result<()> {
        if self
            .has_child_thread_schedules(existing.schedule_id.as_str())
            .await?
        {
            anyhow::bail!(
                "invalid nested loop: cannot update loop cadence while it has nested child loops; update or clear child loops first"
            );
        }
        let Some(parent_schedule_id) = existing.parent_schedule_id.as_deref() else {
            return Ok(());
        };
        if matches!(schedule, crate::ThreadScheduleSpec::Once) {
            anyhow::bail!("invalid nested loop: one-time schedules cannot be nested");
        }
        let parent = self
            .get_thread_schedule(parent_schedule_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid nested loop: parent schedule not found: {parent_schedule_id}"
                )
            })?;
        self.validate_parent_schedule(&parent, existing.thread_id, schedule)
    }

    fn validate_parent_schedule(
        &self,
        parent: &crate::ThreadSchedule,
        thread_id: ThreadId,
        child_schedule: &crate::ThreadScheduleSpec,
    ) -> anyhow::Result<()> {
        if parent.thread_id != thread_id {
            anyhow::bail!("invalid nested loop: parent schedule must belong to the same thread");
        }
        if matches!(parent.schedule, crate::ThreadScheduleSpec::Once) {
            anyhow::bail!("invalid nested loop: parent schedule must be recurring");
        }
        if parent.nesting_depth >= MAX_THREAD_SCHEDULE_NESTING_DEPTH {
            anyhow::bail!(
                "invalid nested loop: maximum nesting depth is {MAX_THREAD_SCHEDULE_NESTING_DEPTH}"
            );
        }
        validate_nested_loop_cadence(&parent.schedule, child_schedule)
    }

    async fn has_child_thread_schedules(&self, schedule_id: &str) -> anyhow::Result<bool> {
        let count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM thread_schedules
WHERE parent_schedule_id = ?
            "#,
        )
        .bind(schedule_id)
        .fetch_one(self.pool.as_ref())
        .await?;
        Ok(count > 0)
    }

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

    pub async fn get_active_materialized_thread_schedule_run_for_turn(
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
  AND materialized_at_ms IS NOT NULL
  AND status IN ('leased', 'running')
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
    COUNT(*) AS total_runs,
    COALESCE(SUM(CASE WHEN status = 'leased' THEN 1 ELSE 0 END), 0) AS leased_runs,
    COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0) AS running_runs,
    COALESCE(SUM(CASE WHEN status = 'deferred' THEN 1 ELSE 0 END), 0) AS deferred_runs,
    COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) AS completed_runs,
    COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) AS failed_runs,
    MAX(started_at_ms) AS last_started_at_ms,
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
            ThreadScheduleClaimTarget::Due => query.bind(now_ms).bind(now_ms).bind(now_ms),
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
        let latest_run: Option<(String, String, Option<String>)> = sqlx::query_as(
            r#"
SELECT run_id, status, goal_id
FROM thread_schedule_runs
WHERE schedule_id = ?
ORDER BY started_at_ms DESC, rowid DESC
LIMIT 1
            "#,
        )
        .bind(selected_schedule.schedule_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let latest_active_run = latest_run
            .filter(|(_, status, _)| matches!(status.as_str(), "leased" | "running" | "deferred"));
        let retry_run_id = latest_active_run
            .as_ref()
            .map(|(run_id, _, _)| run_id.clone());
        let goal_ids = latest_active_run
            .iter()
            .filter_map(|(_, _, goal_id)| goal_id.clone())
            .collect::<Vec<_>>();
        let goal_hold_can_pause =
            !goal_ids.is_empty() && selected_schedule.schedule != crate::ThreadScheduleSpec::Once;
        let mut goal_tx = if goal_hold_can_pause {
            Some(self.goals_pool.begin_with("BEGIN IMMEDIATE").await?)
        } else {
            None
        };
        let mut pause_for_goal_hold = false;
        if let Some(goal_tx) = goal_tx.as_mut() {
            for goal_id in goal_ids {
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
                if pause_for_goal_hold {
                    break;
                }
            }
        }
        if pause_for_goal_hold && latest_active_run.is_some() {
            sqlx::query(
                r#"
UPDATE thread_schedule_runs
SET status = 'failed',
    error = ?,
    completed_at_ms = ?
WHERE schedule_id = ? AND status IN ('leased', 'running', 'deferred')
                "#,
            )
            .bind(redact_state_string(
                "scheduled run lease expired before terminal completion",
            ))
            .bind(now_ms)
            .bind(selected_schedule.schedule_id.as_str())
            .execute(&mut *tx)
            .await?;
        }
        if pause_for_goal_hold {
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
    last_run_at_ms = CASE WHEN ? THEN ? ELSE last_run_at_ms END,
    failure_count = CASE WHEN ? THEN failure_count + 1 ELSE failure_count END,
    updated_at_ms = ?
WHERE schedule_id = ? AND status = 'active'
RETURNING
"#,
        );
        let reaped_expired_run = false;
        let schedule_row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(lease_id)
            .bind(lease_expires_at_ms)
            .bind(reaped_expired_run)
            .bind(now_ms)
            .bind(reaped_expired_run)
            .bind(now_ms)
            .bind(selected_schedule.schedule_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
        let schedule = thread_schedule_from_row(&schedule_row)?;
        let run = if let Some(retry_run_id) = retry_run_id {
            Self::adopt_leased_run(&mut tx, &schedule, &retry_run_id, lease_id).await?
        } else {
            let scheduled_for_ms = match target {
                ThreadScheduleClaimTarget::Due => {
                    schedule.next_run_at.map(datetime_to_epoch_millis)
                }
                ThreadScheduleClaimTarget::Now { .. } => Some(now_ms),
            };
            Self::insert_leased_run(&mut tx, &schedule, lease_id, scheduled_for_ms, now_ms).await?
        };
        tx.commit().await?;
        if let Some(goal_tx) = goal_tx {
            let _ = goal_tx.rollback().await;
        }
        Ok(Some(ThreadScheduleClaim { schedule, run }))
    }

    async fn insert_leased_run(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        schedule: &crate::ThreadSchedule,
        lease_id: &str,
        scheduled_for_ms: Option<i64>,
        started_at_ms: i64,
    ) -> anyhow::Result<crate::ThreadScheduleRun> {
        let run_id = Uuid::new_v4().to_string();
        let turn_id = Uuid::now_v7().to_string();
        let run_row = sqlx::query(
            r#"
INSERT INTO thread_schedule_runs (
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    occurrence_id,
    turn_id,
    scheduled_for_ms,
    started_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING
    run_id,
    schedule_id,
    thread_id,
    occurrence_id,
    status,
    lease_id,
    turn_id,
    materialized_at_ms,
    turn_input,
    goal_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
            "#,
        )
        .bind(run_id)
        .bind(schedule.schedule_id.as_str())
        .bind(schedule.thread_id.to_string())
        .bind(crate::ThreadScheduleRunStatus::Leased.as_str())
        .bind(lease_id)
        .bind(run_id.clone())
        .bind(turn_id)
        .bind(scheduled_for_ms)
        .bind(started_at_ms)
        .fetch_one(&mut **tx)
        .await?;
        thread_schedule_run_from_row(&run_row)
    }

    async fn adopt_leased_run(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        schedule: &crate::ThreadSchedule,
        run_id: &str,
        lease_id: &str,
    ) -> anyhow::Result<crate::ThreadScheduleRun> {
        let turn_id = Uuid::now_v7().to_string();
        let row = sqlx::query(
            r#"
UPDATE thread_schedule_runs
SET status = 'leased',
    lease_id = ?,
    turn_id = COALESCE(turn_id, ?),
    error = NULL,
    completed_at_ms = NULL
WHERE schedule_id = ?
  AND run_id = ?
  AND status IN ('leased', 'running', 'deferred')
RETURNING
    run_id,
    schedule_id,
    thread_id,
    occurrence_id,
    status,
    lease_id,
    turn_id,
    materialized_at_ms,
    turn_input,
    goal_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
            "#,
        )
        .bind(lease_id)
        .bind(turn_id)
        .bind(schedule.schedule_id.as_str())
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
        thread_schedule_run_from_row(&row)
    }

    pub async fn mark_thread_schedule_run_started(
        &self,
        params: ThreadScheduleRunStartParams<'_>,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        self.materialize_thread_schedule_run(params, None).await
    }

    pub async fn attach_thread_schedule_run_goal(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        goal_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let now_ms = datetime_to_epoch_millis(now);
        crate::busy_retry::retry_on_busy("attach thread schedule run goal", || async {
            let result = sqlx::query(
                r#"
UPDATE thread_schedule_runs
SET goal_id = COALESCE(goal_id, ?)
WHERE schedule_id = ?
  AND run_id = ?
  AND lease_id = ?
  AND status = 'leased'
  AND (goal_id IS NULL OR goal_id = ?)
  AND EXISTS (
      SELECT 1
      FROM thread_schedules
      WHERE thread_schedules.schedule_id = thread_schedule_runs.schedule_id
        AND thread_schedules.lease_id = ?
        AND thread_schedules.lease_expires_at_ms > ?
  )
            "#,
            )
            .bind(goal_id)
            .bind(schedule_id)
            .bind(run_id)
            .bind(lease_id)
            .bind(goal_id)
            .bind(lease_id)
            .bind(now_ms)
            .execute(self.pool.as_ref())
            .await?;
            Ok(result.rows_affected() == 1)
        })
        .await
    }

    pub async fn materialize_thread_schedule_run(
        &self,
        params: ThreadScheduleRunStartParams<'_>,
        turn_input: Option<&str>,
    ) -> anyhow::Result<Option<crate::ThreadScheduleRun>> {
        crate::busy_retry::retry_on_busy("mark thread schedule run started", || {
            self.mark_thread_schedule_run_started_once(params.clone(), turn_input)
        })
        .await
    }

    async fn mark_thread_schedule_run_started_once(
        &self,
        params: ThreadScheduleRunStartParams<'_>,
        turn_input: Option<&str>,
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
      FROM thread_schedule_runs
      WHERE thread_schedule_runs.schedule_id = thread_schedules.schedule_id
        AND thread_schedule_runs.run_id = ?
        AND thread_schedule_runs.lease_id = ?
        AND thread_schedule_runs.status = 'leased'
  )
            "#,
        )
        .bind(lease_expires_at_ms)
        .bind(now_ms)
        .bind(schedule_id)
        .bind(lease_id)
        .bind(now_ms)
        .bind(run_id)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if schedule_result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }
        let sql = run_returning(
            r#"
UPDATE thread_schedule_runs
SET status = ?,
    turn_id = COALESCE(turn_id, ?),
    goal_id = COALESCE(goal_id, ?),
    materialized_at_ms = COALESCE(materialized_at_ms, ?),
    turn_input = COALESCE(turn_input, ?)
WHERE schedule_id = ?
  AND run_id = ?
  AND lease_id = ?
  AND status = 'leased'
  AND (turn_id IS NULL OR turn_id = ?)
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(crate::ThreadScheduleRunStatus::Running.as_str())
            .bind(turn_id)
            .bind(goal_id)
            .bind(now_ms)
            .bind(turn_input)
            .bind(schedule_id)
            .bind(run_id)
            .bind(lease_id)
            .bind(turn_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
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
      FROM thread_schedule_runs
      WHERE thread_schedule_runs.schedule_id = thread_schedules.schedule_id
        AND thread_schedule_runs.run_id = ?
        AND thread_schedule_runs.lease_id = ?
        AND thread_schedule_runs.status IN ('leased', 'running')
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
            finish: FinishScheduleRun::Completed {
                pause_schedule: false,
            },
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
            finish: FinishScheduleRun::Completed {
                pause_schedule: false,
            },
        })
        .await
    }

    pub async fn complete_thread_schedule_run_and_pause(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        self.finish_thread_schedule_run(FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at: None,
            expected_goal_id: None,
            finish: FinishScheduleRun::Completed {
                pause_schedule: true,
            },
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
            finish: FinishScheduleRun::Failed {
                error,
                pause_schedule: false,
            },
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
            finish: FinishScheduleRun::Failed {
                error,
                pause_schedule: false,
            },
        })
        .await
    }

    pub async fn fail_thread_schedule_run_and_pause(
        &self,
        schedule_id: &str,
        run_id: &str,
        lease_id: &str,
        completed_at: DateTime<Utc>,
        error: String,
    ) -> anyhow::Result<bool> {
        self.finish_thread_schedule_run(FinishThreadScheduleRunParams {
            schedule_id,
            run_id,
            lease_id,
            completed_at,
            next_run_at: None,
            expected_goal_id: None,
            finish: FinishScheduleRun::Failed {
                error,
                pause_schedule: true,
            },
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
        let mut tx = self.pool.begin().await?;
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
            tx.commit().await?;
            return Ok(false);
        }
        let run_result = sqlx::query(
            r#"
UPDATE thread_schedule_runs
SET status = ?, error = ?, completed_at_ms = ?
WHERE schedule_id = ? AND run_id = ? AND lease_id = ?
            "#,
        )
        .bind(crate::ThreadScheduleRunStatus::Deferred.as_str())
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
        let completed_at_ms = datetime_to_epoch_millis(completed_at);
        let next_run_at_ms = next_run_at.map(datetime_to_epoch_millis);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let schedule_context: Option<(String, String)> = sqlx::query_as(
            r#"
SELECT thread_id, schedule_kind
FROM thread_schedules
WHERE schedule_id = ? AND lease_id = ?
  AND EXISTS (
      SELECT 1
      FROM thread_schedule_runs
      WHERE thread_schedule_runs.schedule_id = thread_schedules.schedule_id
        AND thread_schedule_runs.run_id = ?
        AND thread_schedule_runs.lease_id = ?
        AND thread_schedule_runs.status IN ('leased', 'running')
        AND (? IS NULL OR thread_schedule_runs.goal_id IS NULL OR thread_schedule_runs.goal_id = ?)
  )
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
        let Some((thread_id, schedule_kind)) = schedule_context else {
            tx.commit().await?;
            return Ok(false);
        };
        let goal_hold_can_pause = expected_goal_id.is_some() && schedule_kind != ONCE_SCHEDULE_KIND;
        let mut goal_tx = if goal_hold_can_pause {
            Some(self.goals_pool.begin_with("BEGIN IMMEDIATE").await?)
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
            _ => unreachable!("goal transaction presence follows recurring goal schedule"),
        };
        let failed = matches!(finish, FinishScheduleRun::Failed { .. });
        let pause_schedule = match &finish {
            FinishScheduleRun::Completed { pause_schedule }
            | FinishScheduleRun::Failed { pause_schedule, .. } => {
                *pause_schedule || pause_for_goal_hold
            }
        };
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
        let (status, error) = match &finish {
            FinishScheduleRun::Completed { .. } => {
                (crate::ThreadScheduleRunStatus::Completed, None)
            }
            FinishScheduleRun::Failed { error, .. } => {
                (crate::ThreadScheduleRunStatus::Failed, Some(error.as_str()))
            }
        };
        let error = error.map(redact_state_string);
        let run_result = sqlx::query(
            r#"
UPDATE thread_schedule_runs
SET status = ?, error = ?, completed_at_ms = ?
WHERE schedule_id = ? AND run_id = ? AND lease_id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(error)
        .bind(completed_at_ms)
        .bind(schedule_id)
        .bind(run_id)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if run_result.rows_affected() == 0 {
            tx.rollback().await?;
            if let Some(goal_tx) = goal_tx {
                let _ = goal_tx.rollback().await;
            }
            return Ok(false);
        }
        tx.commit().await?;
        if let Some(goal_tx) = goal_tx {
            let _ = goal_tx.rollback().await;
        }
        Ok(true)
    }
}

#[derive(Clone)]
struct FinishThreadScheduleRunParams<'a> {
    schedule_id: &'a str,
    run_id: &'a str,
    lease_id: &'a str,
    completed_at: DateTime<Utc>,
    next_run_at: Option<DateTime<Utc>>,
    expected_goal_id: Option<&'a str>,
    finish: FinishScheduleRun,
}

#[derive(Clone)]
enum FinishScheduleRun {
    Completed { pause_schedule: bool },
    Failed { error: String, pause_schedule: bool },
}

struct ScheduleBindings<'a> {
    kind: &'static str,
    interval_amount: Option<i64>,
    interval_unit: Option<&'static str>,
    cron_expression: Option<&'a str>,
}

fn schedule_bindings(schedule: &crate::ThreadScheduleSpec) -> ScheduleBindings<'_> {
    match schedule {
        crate::ThreadScheduleSpec::Once => ScheduleBindings {
            kind: ONCE_SCHEDULE_KIND,
            interval_amount: None,
            interval_unit: None,
            cron_expression: None,
        },
        crate::ThreadScheduleSpec::Dynamic => ScheduleBindings {
            kind: "dynamic",
            interval_amount: None,
            interval_unit: None,
            cron_expression: None,
        },
        crate::ThreadScheduleSpec::Interval(interval) => ScheduleBindings {
            kind: "interval",
            interval_amount: Some(interval.amount),
            interval_unit: Some(interval.unit.as_str()),
            cron_expression: None,
        },
        crate::ThreadScheduleSpec::Cron { expression } => ScheduleBindings {
            kind: "cron",
            interval_amount: None,
            interval_unit: None,
            cron_expression: Some(expression.as_str()),
        },
    }
}

fn validate_nested_loop_cadence(
    parent_schedule: &crate::ThreadScheduleSpec,
    child_schedule: &crate::ThreadScheduleSpec,
) -> anyhow::Result<()> {
    let parent_seconds = recurring_loop_cadence_seconds(parent_schedule, "parent")?;
    let child_seconds = recurring_loop_cadence_seconds(child_schedule, "child")?;
    if child_seconds <= parent_seconds {
        anyhow::bail!(
            "invalid nested loop: child cadence must be slower than parent cadence (parent: {parent_seconds}s, child: {child_seconds}s)"
        );
    }
    Ok(())
}

fn recurring_loop_cadence_seconds(
    schedule: &crate::ThreadScheduleSpec,
    role: &str,
) -> anyhow::Result<i64> {
    match schedule {
        crate::ThreadScheduleSpec::Dynamic => Ok(DYNAMIC_LOOP_CADENCE_SECONDS),
        crate::ThreadScheduleSpec::Interval(interval) => {
            let unit_seconds = match interval.unit {
                crate::ThreadScheduleIntervalUnit::Minutes => 60,
                crate::ThreadScheduleIntervalUnit::Hours => 3_600,
                crate::ThreadScheduleIntervalUnit::Days => 86_400,
            };
            interval
                .amount
                .checked_mul(unit_seconds)
                .filter(|seconds| *seconds > 0)
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid nested loop: {role} interval cadence is invalid")
                })
        }
        crate::ThreadScheduleSpec::Cron { .. } => {
            anyhow::bail!(
                "invalid nested loop: {role} cron schedules cannot be nested; use dynamic or interval cadence"
            );
        }
        crate::ThreadScheduleSpec::Once => {
            anyhow::bail!("invalid nested loop: {role} schedule must be recurring");
        }
    }
}

const SCHEDULE_COLUMNS: &str = r#"
    schedule_id,
    thread_id,
    parent_schedule_id,
    nesting_depth,
    prompt_source,
    prompt,
    schedule_kind,
    interval_amount,
    interval_unit,
    cron_expression,
    timezone,
    auth_profile_recorded,
    auth_profile,
    status,
    next_run_at_ms,
    last_run_at_ms,
    expires_at_ms,
    failure_count,
    lease_id,
    lease_expires_at_ms,
    created_at_ms,
    updated_at_ms
"#;

fn schedule_returning(prefix: &str) -> String {
    format!("{prefix}{SCHEDULE_COLUMNS}")
}

fn schedule_select_by_id(prefix: &str) -> String {
    format!(
        r#"{prefix}{SCHEDULE_COLUMNS}
FROM thread_schedules
WHERE schedule_id = ?
"#
    )
}

const RUN_COLUMNS: &str = r#"
    run_id,
    schedule_id,
    thread_id,
    occurrence_id,
    status,
    lease_id,
    turn_id,
    materialized_at_ms,
    turn_input,
    goal_id,
    error,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
"#;

fn run_returning(prefix: &str) -> String {
    format!("{prefix}{RUN_COLUMNS}")
}

fn thread_schedule_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ThreadSchedule> {
    ThreadScheduleRow::try_from_row(row).and_then(crate::ThreadSchedule::try_from)
}

fn thread_schedule_run_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ThreadScheduleRun> {
    ThreadScheduleRunRow::try_from_row(row).and_then(crate::ThreadScheduleRun::try_from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id(id: u32) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{id:012}"))
            .expect("valid thread id")
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("valid timestamp")
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

    async fn create_interval_schedule(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        prompt: &str,
        next_run_at: Option<DateTime<Utc>>,
    ) -> crate::ThreadSchedule {
        create_interval_schedule_minutes(runtime, thread_id, prompt, 5, next_run_at).await
    }

    async fn create_interval_schedule_minutes(
        runtime: &StateRuntime,
        thread_id: ThreadId,
        prompt: &str,
        minutes: i64,
        next_run_at: Option<DateTime<Utc>>,
    ) -> crate::ThreadSchedule {
        runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: prompt.to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                    amount: minutes,
                    unit: crate::ThreadScheduleIntervalUnit::Minutes,
                }),
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at,
                expires_at: None,
            })
            .await
            .expect("schedule should be created")
    }

    #[tokio::test]
    async fn create_update_list_and_delete_thread_schedule() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;
        let next_run_at = at(/*seconds*/ 1_700_000_060);

        let created = create_interval_schedule(
            &runtime,
            thread_id,
            "summarize new alerts",
            Some(next_run_at),
        )
        .await;
        let expected_created = crate::ThreadSchedule {
            thread_id,
            schedule_id: created.schedule_id.clone(),
            parent_schedule_id: None,
            nesting_depth: 1,
            auth_profile: None,
            prompt: "summarize new alerts".to_string(),
            prompt_source: crate::ThreadSchedulePromptSource::Inline,
            schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                amount: 5,
                unit: crate::ThreadScheduleIntervalUnit::Minutes,
            }),
            timezone: "UTC".to_string(),
            status: crate::ThreadScheduleStatus::Active,
            next_run_at: Some(next_run_at),
            last_run_at: None,
            expires_at: None,
            failure_count: 0,
            lease_id: None,
            lease_expires_at: None,
            created_at: created.created_at,
            updated_at: created.updated_at,
        };
        assert_eq!(expected_created, created);
        assert_eq!(
            Some(created.clone()),
            runtime
                .thread_schedules()
                .get_thread_schedule(&created.schedule_id)
                .await
                .expect("schedule should load")
        );
        assert_eq!(
            vec![created.clone()],
            runtime
                .thread_schedules()
                .list_thread_schedules(thread_id)
                .await
                .expect("schedules should list")
        );

        let updated = runtime
            .thread_schedules()
            .update_thread_schedule(
                &created.schedule_id,
                ThreadScheduleUpdate {
                    prompt: Some("write the daily handoff".to_string()),
                    prompt_source: Some(crate::ThreadSchedulePromptSource::Default),
                    schedule: Some(crate::ThreadScheduleSpec::Cron {
                        expression: "0 9 * * 1-5".to_string(),
                    }),
                    timezone: Some("Europe/Bucharest".to_string()),
                    status: Some(crate::ThreadScheduleStatus::Paused),
                    next_run_at: Some(None),
                    expires_at: Some(Some(at(/*seconds*/ 1_700_086_400))),
                },
            )
            .await
            .expect("schedule should update")
            .expect("schedule should exist");
        let expected_updated = crate::ThreadSchedule {
            prompt: "write the daily handoff".to_string(),
            prompt_source: crate::ThreadSchedulePromptSource::Default,
            schedule: crate::ThreadScheduleSpec::Cron {
                expression: "0 9 * * 1-5".to_string(),
            },
            timezone: "Europe/Bucharest".to_string(),
            status: crate::ThreadScheduleStatus::Paused,
            next_run_at: None,
            expires_at: Some(at(/*seconds*/ 1_700_086_400)),
            updated_at: updated.updated_at,
            ..created.clone()
        };
        assert_eq!(expected_updated, updated);

        assert!(
            runtime
                .thread_schedules()
                .delete_thread_schedule(&created.schedule_id)
                .await
                .expect("schedule should delete")
        );
        assert!(
            !runtime
                .thread_schedules()
                .delete_thread_schedule(&created.schedule_id)
                .await
                .expect("missing schedule delete should be false")
        );
    }

    #[tokio::test]
    async fn create_once_thread_schedule() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 11);
        upsert_test_thread(&runtime, thread_id).await;
        let next_run_at = at(/*seconds*/ 1_700_000_060);

        let created = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "ask one question".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(next_run_at),
                expires_at: None,
            })
            .await
            .expect("one-time schedule should be created");

        assert_eq!(
            crate::ThreadSchedule {
                thread_id,
                schedule_id: created.schedule_id.clone(),
                parent_schedule_id: None,
                nesting_depth: 1,
                auth_profile: None,
                prompt: "ask one question".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(next_run_at),
                last_run_at: None,
                expires_at: None,
                failure_count: 0,
                lease_id: None,
                lease_expires_at: None,
                created_at: created.created_at,
                updated_at: created.updated_at,
            },
            created
        );
    }

    #[tokio::test]
    async fn create_nested_thread_schedule_derives_parent_and_depth() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 16);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let parent =
            create_interval_schedule_minutes(&runtime, thread_id, "parent loop", 1, Some(now))
                .await;

        let child = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "child loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 2,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(2)),
                    expires_at: None,
                },
                parent.schedule_id.clone(),
            )
            .await
            .expect("nested schedule should be created");
        assert_eq!(Some(parent.schedule_id.clone()), child.parent_schedule_id);
        assert_eq!(2, child.nesting_depth);

        let grandchild = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "grandchild loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 3,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(3)),
                    expires_at: None,
                },
                child.schedule_id.clone(),
            )
            .await
            .expect("third-level schedule should be created");
        assert_eq!(
            Some(child.schedule_id.clone()),
            grandchild.parent_schedule_id
        );
        assert_eq!(3, grandchild.nesting_depth);

        let level_4 = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "level 4 loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 4,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(4)),
                    expires_at: None,
                },
                grandchild.schedule_id.clone(),
            )
            .await
            .expect("fourth-level schedule should be created");
        assert_eq!(
            Some(grandchild.schedule_id.clone()),
            level_4.parent_schedule_id
        );
        assert_eq!(4, level_4.nesting_depth);

        let level_5 = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "level 5 loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 5,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(5)),
                    expires_at: None,
                },
                level_4.schedule_id.clone(),
            )
            .await
            .expect("fifth-level schedule should be created");
        assert_eq!(
            Some(level_4.schedule_id.clone()),
            level_5.parent_schedule_id
        );
        assert_eq!(5, level_5.nesting_depth);

        let err = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "too deep".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 6,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(6)),
                    expires_at: None,
                },
                level_5.schedule_id.clone(),
            )
            .await
            .expect_err("sixth-level nested schedule should be rejected");
        assert!(
            err.to_string().contains("maximum nesting depth is 5"),
            "unexpected error: {err}"
        );

        assert!(
            runtime
                .thread_schedules()
                .delete_thread_schedule(&parent.schedule_id)
                .await
                .expect("parent delete should succeed")
        );
        assert_eq!(
            None,
            runtime
                .thread_schedules()
                .get_thread_schedule(&child.schedule_id)
                .await
                .expect("child lookup should succeed")
        );
        assert_eq!(
            None,
            runtime
                .thread_schedules()
                .get_thread_schedule(&grandchild.schedule_id)
                .await
                .expect("grandchild lookup should succeed")
        );
    }

    #[tokio::test]
    async fn delete_thread_schedule_tree_cascades_descendants() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 24);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let root =
            create_interval_schedule_minutes(&runtime, thread_id, "root loop", 1, Some(now)).await;
        let child = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "child loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 2,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(2)),
                    expires_at: None,
                },
                root.schedule_id.clone(),
            )
            .await
            .expect("child schedule should be created");
        let grandchild = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "grandchild loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 3,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(3)),
                    expires_at: None,
                },
                child.schedule_id.clone(),
            )
            .await
            .expect("grandchild schedule should be created");

        assert_eq!(
            vec![
                grandchild.schedule_id.clone(),
                child.schedule_id.clone(),
                root.schedule_id.clone(),
            ],
            runtime
                .thread_schedules()
                .delete_thread_schedule_tree(&root.schedule_id)
                .await
                .expect("schedule tree should delete")
        );
        assert_eq!(
            Vec::<crate::ThreadSchedule>::new(),
            runtime
                .thread_schedules()
                .list_thread_schedules(thread_id)
                .await
                .expect("schedules should list")
        );
    }

    #[tokio::test]
    async fn create_nested_thread_schedule_rejects_impractical_cadence() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 17);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let parent =
            create_interval_schedule_minutes(&runtime, thread_id, "parent loop", 1, Some(now))
                .await;

        let err = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "same minute child".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Dynamic,
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(1)),
                    expires_at: None,
                },
                parent.schedule_id,
            )
            .await
            .expect_err("one-minute child under one-minute parent should be rejected");
        assert!(
            err.to_string()
                .contains("child cadence must be slower than parent cadence"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn create_nested_thread_schedule_rejects_cron_cadences() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 18);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let cron_parent = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "cron parent".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Cron {
                    expression: "*/5 * * * *".to_string(),
                },
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now + chrono::Duration::minutes(5)),
                expires_at: None,
            })
            .await
            .expect("cron parent should be created");

        let err = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "child".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 10,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(10)),
                    expires_at: None,
                },
                cron_parent.schedule_id,
            )
            .await
            .expect_err("cron parent should reject nested child loops");
        assert!(
            err.to_string()
                .contains("parent cron schedules cannot be nested"),
            "unexpected error: {err}"
        );

        let interval_parent =
            create_interval_schedule_minutes(&runtime, thread_id, "interval parent", 5, Some(now))
                .await;
        let err = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "cron child".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Cron {
                        expression: "*/10 * * * *".to_string(),
                    },
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(10)),
                    expires_at: None,
                },
                interval_parent.schedule_id,
            )
            .await
            .expect_err("cron child should be rejected");
        assert!(
            err.to_string()
                .contains("child cron schedules cannot be nested"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn update_thread_schedule_enforces_nested_loop_constraints() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 19);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let parent =
            create_interval_schedule_minutes(&runtime, thread_id, "parent loop", 1, Some(now))
                .await;
        let child = runtime
            .thread_schedules()
            .create_nested_thread_schedule(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "child loop".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Interval(crate::ThreadScheduleInterval {
                        amount: 2,
                        unit: crate::ThreadScheduleIntervalUnit::Minutes,
                    }),
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(now + chrono::Duration::minutes(2)),
                    expires_at: None,
                },
                parent.schedule_id.clone(),
            )
            .await
            .expect("nested child should be created");

        let renamed_parent = runtime
            .thread_schedules()
            .update_thread_schedule(
                &parent.schedule_id,
                ThreadScheduleUpdate {
                    prompt: Some("renamed parent loop".to_string()),
                    prompt_source: None,
                    schedule: Some(parent.schedule.clone()),
                    timezone: None,
                    status: None,
                    next_run_at: None,
                    expires_at: None,
                },
            )
            .await
            .expect("unchanged parent cadence with prompt update should succeed")
            .expect("parent schedule should exist");
        assert_eq!("renamed parent loop", renamed_parent.prompt);
        assert_eq!(parent.schedule, renamed_parent.schedule);

        let err = runtime
            .thread_schedules()
            .update_thread_schedule(
                &parent.schedule_id,
                ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: Some(crate::ThreadScheduleSpec::Interval(
                        crate::ThreadScheduleInterval {
                            amount: 10,
                            unit: crate::ThreadScheduleIntervalUnit::Minutes,
                        },
                    )),
                    timezone: None,
                    status: None,
                    next_run_at: None,
                    expires_at: None,
                },
            )
            .await
            .expect_err("parent cadence update should be rejected while children exist");
        assert!(
            err.to_string()
                .contains("cannot update loop cadence while it has nested child loops"),
            "unexpected error: {err}"
        );

        let err = runtime
            .thread_schedules()
            .update_thread_schedule(
                &child.schedule_id,
                ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: Some(crate::ThreadScheduleSpec::Dynamic),
                    timezone: None,
                    status: None,
                    next_run_at: None,
                    expires_at: None,
                },
            )
            .await
            .expect_err("child cadence update should be revalidated");
        assert!(
            err.to_string()
                .contains("child cadence must be slower than parent cadence"),
            "unexpected error: {err}"
        );

        let updated_child = runtime
            .thread_schedules()
            .update_thread_schedule(
                &child.schedule_id,
                ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: Some(crate::ThreadScheduleSpec::Interval(
                        crate::ThreadScheduleInterval {
                            amount: 3,
                            unit: crate::ThreadScheduleIntervalUnit::Minutes,
                        },
                    )),
                    timezone: None,
                    status: None,
                    next_run_at: None,
                    expires_at: None,
                },
            )
            .await
            .expect("valid child cadence update should succeed")
            .expect("child schedule should exist");
        assert_eq!(Some(parent.schedule_id), updated_child.parent_schedule_id);
        assert_eq!(2, updated_child.nesting_depth);
    }

    #[tokio::test]
    async fn create_thread_schedule_records_auth_profile() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 13);
        upsert_test_thread(&runtime, thread_id).await;

        let named = runtime
            .thread_schedules()
            .create_thread_schedule_for_auth_profile(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "named profile".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Once,
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(at(/*seconds*/ 1_700_000_060)),
                    expires_at: None,
                },
                Some("account002".to_string()),
            )
            .await
            .expect("schedule should be created");
        let root = runtime
            .thread_schedules()
            .create_thread_schedule_for_auth_profile(
                ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "root profile".to_string(),
                    prompt_source: crate::ThreadSchedulePromptSource::Inline,
                    schedule: crate::ThreadScheduleSpec::Once,
                    timezone: "UTC".to_string(),
                    status: crate::ThreadScheduleStatus::Active,
                    next_run_at: Some(at(/*seconds*/ 1_700_000_120)),
                    expires_at: None,
                },
                /*auth_profile*/ None,
            )
            .await
            .expect("schedule should be created");

        assert_eq!(Some(Some("account002".to_string())), named.auth_profile);
        assert_eq!(Some(None), root.auth_profile);
        assert_eq!(
            Some(Some("account002".to_string())),
            runtime
                .thread_schedules()
                .get_thread_schedule(named.schedule_id.as_str())
                .await
                .expect("schedule should load")
                .expect("schedule should exist")
                .auth_profile
        );
    }

    #[tokio::test]
    async fn completed_one_time_schedule_expires_and_cannot_run_again() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 12);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "ask one question".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: None,
            })
            .await
            .expect("one-time schedule should be created");
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-once", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("one-time schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-once",
                turn_id: claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(300),
            })
            .await
            .expect("run should update")
            .expect("run should exist");

        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-once",
                    now + chrono::Duration::seconds(5),
                    /*next_run_at*/ None,
                )
                .await
                .expect("run should complete")
        );

        let completed = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, completed.status);
        assert_eq!(None, completed.next_run_at);
        assert!(
            runtime
                .thread_schedules()
                .claim_thread_schedule_now(
                    &schedule.schedule_id,
                    now + chrono::Duration::seconds(10),
                    "lease-repeat",
                    Duration::from_secs(300),
                )
                .await
                .expect("manual claim should not fail")
                .is_none(),
            "completed one-time schedule should not be runnable again"
        );
    }

    #[tokio::test]
    async fn claim_due_thread_schedule_leases_one_due_active_schedule() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 2);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let first = create_interval_schedule(
            &runtime,
            thread_id,
            "first due task",
            Some(now - chrono::Duration::minutes(2)),
        )
        .await;
        let second = create_interval_schedule(
            &runtime,
            thread_id,
            "second due task",
            Some(now - chrono::Duration::minutes(1)),
        )
        .await;
        create_interval_schedule(
            &runtime,
            thread_id,
            "future task",
            Some(now + chrono::Duration::minutes(1)),
        )
        .await;

        let first_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-a", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("first due schedule should claim");
        assert_eq!(first.schedule_id, first_claim.schedule.schedule_id);
        assert_eq!(Some("lease-a".to_string()), first_claim.schedule.lease_id);
        assert_eq!(
            crate::ThreadScheduleRunStatus::Leased,
            first_claim.run.status
        );
        assert_eq!("lease-a", first_claim.run.lease_id);
        assert_eq!(
            Some(now - chrono::Duration::minutes(2)),
            first_claim.run.scheduled_for
        );

        let second_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-b", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("second due schedule should claim");
        assert_eq!(second.schedule_id, second_claim.schedule.schedule_id);

        assert!(
            runtime
                .thread_schedules()
                .claim_due_thread_schedule(now, "lease-c", Duration::from_secs(300))
                .await
                .expect("no more schedules should be claimable")
                .is_none()
        );
    }

    #[tokio::test]
    async fn claim_due_thread_schedule_adopts_expired_occurrence_once() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id = test_thread_id(/*id*/ 44);
        upsert_test_thread(runtime.as_ref(), thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(runtime.as_ref(), thread_id, "restart retry", Some(now)).await;
        let original_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-before-restart", Duration::from_secs(30))
            .await
            .expect("initial claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &original_claim.run.run_id,
                lease_id: "lease-before-restart",
                turn_id: original_claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("run should start")
            .expect("run should still exist");
        drop(runtime);

        let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("state db should reopen after process restart");
        let retry_at = now + chrono::Duration::seconds(31);
        let retry_claim = reopened
            .thread_schedules()
            .claim_due_thread_schedule(retry_at, "lease-after-restart", Duration::from_secs(30))
            .await
            .expect("expired run recovery should succeed")
            .expect("expired non-goal run should retry exactly once");

        let original_run = reopened
            .thread_schedules()
            .get_thread_schedule_run(&original_claim.run.run_id)
            .await
            .expect("original run should load")
            .expect("original run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Leased, original_run.status);
        assert_eq!(None, original_run.completed_at);
        assert_eq!(None, original_run.error);
        assert_eq!(
            crate::ThreadScheduleRunStatus::Leased,
            retry_claim.run.status
        );
        assert_eq!(
            original_claim.run.scheduled_for,
            retry_claim.run.scheduled_for
        );
        assert_eq!(original_claim.run.run_id, retry_claim.run.run_id);
        let stats = reopened
            .thread_schedules()
            .get_thread_schedule_stats(&schedule.schedule_id)
            .await
            .expect("schedule stats should load");
        assert_eq!(1, stats.total_runs);
        assert_eq!(1, stats.leased_runs);
        assert_eq!(0, stats.running_runs);
        assert_eq!(0, stats.failed_runs);
        assert!(
            reopened
                .thread_schedules()
                .claim_due_thread_schedule(
                    retry_at,
                    "lease-duplicate-retry",
                    Duration::from_secs(30),
                )
                .await
                .expect("duplicate claim check should succeed")
                .is_none(),
            "one expired lease may create at most one replacement claim"
        );
    }

    #[tokio::test]
    async fn claim_due_thread_schedule_pauses_expired_run_for_held_goal_after_restart() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id = test_thread_id(/*id*/ 45);
        upsert_test_thread(runtime.as_ref(), thread_id).await;
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "hold after restart",
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("blocked goal should persist");
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(runtime.as_ref(), thread_id, "hold after restart", Some(now))
                .await;
        let original_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-goal-restart", Duration::from_secs(30))
            .await
            .expect("initial claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &original_claim.run.run_id,
                lease_id: "lease-goal-restart",
                turn_id: original_claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: Some(&goal.goal_id),
                now,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("goal run should start")
            .expect("goal run should still exist");
        drop(runtime);

        let reopened = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("state db should reopen after process restart");
        let retry_at = now + chrono::Duration::seconds(31);
        assert!(
            reopened
                .thread_schedules()
                .claim_due_thread_schedule(
                    retry_at,
                    "lease-held-replacement",
                    Duration::from_secs(30),
                )
                .await
                .expect("expired goal run recovery should succeed")
                .is_none(),
            "a persisted held goal must pause instead of creating a replacement run"
        );

        let held_schedule = reopened
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Paused, held_schedule.status);
        assert_eq!(None, held_schedule.next_run_at);
        assert_eq!(None, held_schedule.lease_id);
        let original_run = reopened
            .thread_schedules()
            .get_thread_schedule_run(&original_claim.run.run_id)
            .await
            .expect("original run should load")
            .expect("original run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Failed, original_run.status);
        assert_eq!(Some(goal.goal_id), original_run.goal_id);
        assert_eq!(Some(retry_at), original_run.completed_at);
        let stats = reopened
            .thread_schedules()
            .get_thread_schedule_stats(&schedule.schedule_id)
            .await
            .expect("schedule stats should load");
        assert_eq!(1, stats.total_runs);
        assert_eq!(0, stats.leased_runs);
        assert_eq!(0, stats.running_runs);
        assert_eq!(1, stats.failed_runs);
    }

    #[tokio::test]
    async fn claim_due_thread_schedule_skips_fresh_foreign_active_owner() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 3);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "live owner task", Some(now)).await;
        runtime
            .local_active_sessions()
            .heartbeat_session(LocalActiveSessionHeartbeatParams {
                thread_id,
                owner_id: "owner-a".to_string(),
                session_id: "session-a".to_string(),
                pid: Some(100),
                now,
            })
            .await
            .expect("active session should heartbeat");

        assert!(
            runtime
                .thread_schedules()
                .claim_due_thread_schedule_with_params(ThreadScheduleDueClaimParams {
                    now,
                    lease_id: "lease-owner-b",
                    lease_duration: Duration::from_secs(300),
                    local_active_owner_id: Some("owner-b"),
                    local_active_fresh_after: Some(now - chrono::Duration::seconds(15)),
                })
                .await
                .expect("claim should not fail")
                .is_none(),
            "foreign processes should not claim loops owned by a fresh live session"
        );

        let owner_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule_with_params(ThreadScheduleDueClaimParams {
                now,
                lease_id: "lease-owner-a",
                lease_duration: Duration::from_secs(300),
                local_active_owner_id: Some("owner-a"),
                local_active_fresh_after: Some(now - chrono::Duration::seconds(15)),
            })
            .await
            .expect("owner claim should succeed")
            .expect("live owner should claim its due schedule");

        assert_eq!(schedule.schedule_id, owner_claim.schedule.schedule_id);
        assert_eq!(
            Some("owner:lease-owner-a".to_string()),
            owner_claim.schedule.lease_id
        );
        assert_eq!("owner:lease-owner-a", owner_claim.run.lease_id);
    }

    #[tokio::test]
    async fn claim_due_thread_schedule_ignores_legacy_claim_when_live_owner_is_fresh() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 5);
        upsert_test_thread(&runtime, thread_id).await;
        let now = Utc::now();
        let schedule =
            create_interval_schedule(&runtime, thread_id, "legacy live owner task", Some(now))
                .await;
        runtime
            .local_active_sessions()
            .heartbeat_session(LocalActiveSessionHeartbeatParams {
                thread_id,
                owner_id: "owner-a".to_string(),
                session_id: "session-a".to_string(),
                pid: Some(100),
                now,
            })
            .await
            .expect("active session should heartbeat");

        assert!(
            runtime
                .thread_schedules()
                .claim_due_thread_schedule(now, "legacy-lease", Duration::from_secs(300))
                .await
                .expect("legacy claim should be ignored without failing")
                .is_none(),
            "legacy schedulers should not steal loops from fresh live sessions"
        );

        let schedules = runtime
            .thread_schedules()
            .list_thread_schedules(thread_id)
            .await
            .expect("schedules should list");
        assert_eq!(None, schedules[0].lease_id);

        let owner_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule_with_params(ThreadScheduleDueClaimParams {
                now,
                lease_id: "owner-lease",
                lease_duration: Duration::from_secs(300),
                local_active_owner_id: Some("owner-a"),
                local_active_fresh_after: Some(now - chrono::Duration::seconds(15)),
            })
            .await
            .expect("owner claim should succeed")
            .expect("live owner should claim after legacy claim is ignored");

        assert_eq!(schedule.schedule_id, owner_claim.schedule.schedule_id);
        assert_eq!(
            Some("owner:owner-lease".to_string()),
            owner_claim.schedule.lease_id
        );
        assert_eq!("owner:owner-lease", owner_claim.run.lease_id);
    }

    #[tokio::test]
    async fn claim_due_thread_schedule_allows_stale_foreign_active_owner() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 4);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "stale owner task", Some(now)).await;
        runtime
            .local_active_sessions()
            .heartbeat_session(LocalActiveSessionHeartbeatParams {
                thread_id,
                owner_id: "owner-a".to_string(),
                session_id: "session-a".to_string(),
                pid: Some(100),
                now: now - chrono::Duration::seconds(30),
            })
            .await
            .expect("active session should heartbeat");

        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule_with_params(ThreadScheduleDueClaimParams {
                now,
                lease_id: "lease-owner-b",
                lease_duration: Duration::from_secs(300),
                local_active_owner_id: Some("owner-b"),
                local_active_fresh_after: Some(now - chrono::Duration::seconds(15)),
            })
            .await
            .expect("claim should succeed")
            .expect("stale foreign owner should not block recovery");

        assert_eq!(schedule.schedule_id, claim.schedule.schedule_id);
        assert_eq!(
            Some("owner:lease-owner-b".to_string()),
            claim.schedule.lease_id
        );
        assert_eq!("owner:lease-owner-b", claim.run.lease_id);
    }

    #[tokio::test]
    async fn claim_thread_schedule_now_leases_specific_active_schedule() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 5);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let future = create_interval_schedule(
            &runtime,
            thread_id,
            "future manual task",
            Some(now + chrono::Duration::hours(1)),
        )
        .await;
        let other = create_interval_schedule(&runtime, thread_id, "other task", Some(now)).await;

        let claim = runtime
            .thread_schedules()
            .claim_thread_schedule_now(
                &future.schedule_id,
                now,
                "lease-manual",
                Duration::from_secs(300),
            )
            .await
            .expect("manual claim should succeed")
            .expect("future schedule should claim");

        assert_eq!(future.schedule_id, claim.schedule.schedule_id);
        assert_eq!(Some("lease-manual".to_string()), claim.schedule.lease_id);
        assert_eq!(Some(now), claim.run.scheduled_for);
        assert_eq!(
            other,
            runtime
                .thread_schedules()
                .get_thread_schedule(&other.schedule_id)
                .await
                .expect("other schedule should load")
                .expect("other schedule should exist")
        );
        assert!(
            runtime
                .thread_schedules()
                .claim_thread_schedule_now(
                    &future.schedule_id,
                    now,
                    "lease-second",
                    Duration::from_secs(300),
                )
                .await
                .expect("second manual claim should not fail")
                .is_none()
        );
    }

    #[tokio::test]
    async fn claim_thread_schedule_now_ignores_legacy_claim_and_allows_live_owner() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 6);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(Utc::now().timestamp());
        let schedule = create_interval_schedule(
            &runtime,
            thread_id,
            "manual live owner task",
            Some(now + chrono::Duration::hours(1)),
        )
        .await;
        runtime
            .local_active_sessions()
            .heartbeat_session(LocalActiveSessionHeartbeatParams {
                thread_id,
                owner_id: "owner-a".to_string(),
                session_id: "session-a".to_string(),
                pid: Some(100),
                now,
            })
            .await
            .expect("active session should heartbeat");

        assert!(
            runtime
                .thread_schedules()
                .claim_thread_schedule_now(
                    &schedule.schedule_id,
                    now,
                    "legacy-manual-lease",
                    Duration::from_secs(300),
                )
                .await
                .expect("legacy manual claim should be ignored without failing")
                .is_none(),
            "legacy manual run-now should not steal loops from fresh live sessions"
        );

        assert!(
            runtime
                .thread_schedules()
                .claim_thread_schedule_now_with_params(ThreadScheduleNowClaimParams {
                    schedule_id: &schedule.schedule_id,
                    now,
                    lease_id: "manual-foreign-lease",
                    lease_duration: Duration::from_secs(300),
                    local_active_owner_id: Some("owner-b"),
                    local_active_fresh_after: Some(now - chrono::Duration::seconds(15)),
                })
                .await
                .expect("foreign manual claim should not fail")
                .is_none(),
            "new foreign manual run-now should not steal loops from fresh live sessions"
        );

        let owner_claim = runtime
            .thread_schedules()
            .claim_thread_schedule_now_with_params(ThreadScheduleNowClaimParams {
                schedule_id: &schedule.schedule_id,
                now,
                lease_id: "manual-owner-lease",
                lease_duration: Duration::from_secs(300),
                local_active_owner_id: Some("owner-a"),
                local_active_fresh_after: Some(now - chrono::Duration::seconds(15)),
            })
            .await
            .expect("owner manual claim should succeed")
            .expect("live owner should claim manual run-now");

        assert_eq!(schedule.schedule_id, owner_claim.schedule.schedule_id);
        assert_eq!(
            Some("owner:manual-owner-lease".to_string()),
            owner_claim.schedule.lease_id
        );
        assert_eq!("owner:manual-owner-lease", owner_claim.run.lease_id);
        assert_eq!(Some(now), owner_claim.run.scheduled_for);
    }

    #[tokio::test]
    async fn extend_thread_schedule_lease_refreshes_live_claim() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 6);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "long running task", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-long", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        assert_eq!(schedule.schedule_id, claim.schedule.schedule_id);

        assert!(
            runtime
                .thread_schedules()
                .extend_thread_schedule_lease(ThreadScheduleRunLeaseParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "lease-long",
                    now: now + chrono::Duration::seconds(120),
                    lease_duration: Duration::from_secs(300),
                })
                .await
                .expect("lease should extend")
        );
        let refreshed = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(
            Some(now + chrono::Duration::seconds(420)),
            refreshed.lease_expires_at
        );
        assert!(
            !runtime
                .thread_schedules()
                .extend_thread_schedule_lease(ThreadScheduleRunLeaseParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "wrong-lease",
                    now: now + chrono::Duration::seconds(180),
                    lease_duration: Duration::from_secs(300),
                })
                .await
                .expect("wrong lease should not fail")
        );
    }

    #[tokio::test]
    async fn expired_heartbeat_cannot_revive_schedule_lease() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 49);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "stale heartbeat", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-stale", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-stale",
                turn_id: claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("run start should persist")
            .expect("run should start");
        let expired_at = now + chrono::Duration::seconds(31);

        assert!(
            !runtime
                .thread_schedules()
                .extend_thread_schedule_lease(ThreadScheduleRunLeaseParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "lease-stale",
                    now: expired_at,
                    lease_duration: Duration::from_secs(30),
                })
                .await
                .expect("expired heartbeat should fail closed")
        );
        assert_eq!(
            Some(now + chrono::Duration::seconds(30)),
            runtime
                .thread_schedules()
                .get_thread_schedule(&schedule.schedule_id)
                .await
                .expect("schedule should load")
                .expect("schedule should exist")
                .lease_expires_at
        );
        let replacement = runtime
            .thread_schedules()
            .claim_due_thread_schedule(expired_at, "lease-new", Duration::from_secs(30))
            .await
            .expect("reaper should not error")
            .expect("expired run should be replaceable");
        assert_eq!(claim.run.run_id, replacement.run.run_id);
    }

    #[tokio::test]
    async fn completed_schedule_run_can_atomically_pause_without_rearming() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 31);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = create_interval_schedule(&runtime, thread_id, "held task", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-held", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");

        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run_and_pause(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-held",
                    now + chrono::Duration::seconds(5),
                )
                .await
                .expect("run should complete while pausing the schedule")
        );

        let held_schedule = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Paused, held_schedule.status);
        assert_eq!(None, held_schedule.next_run_at);
        assert_eq!(None, held_schedule.lease_id);
        assert_eq!(0, held_schedule.failure_count);
        let run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&claim.run.run_id)
            .await
            .expect("run should load")
            .expect("run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Completed, run.status);
        assert!(
            runtime
                .thread_schedules()
                .claim_due_thread_schedule(
                    now + chrono::Duration::days(1),
                    "lease-rearm",
                    Duration::from_secs(300),
                )
                .await
                .expect("paused schedule claim should not fail")
                .is_none(),
            "a held schedule must not become claimable again"
        );
    }

    #[tokio::test]
    async fn goal_correlated_completion_ignores_replacement_with_same_objective() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 41);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let next_run_at = now + chrono::Duration::minutes(5);
        let objective = "repeat the same objective";
        let original_goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                objective,
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("original goal should be created");
        let schedule = create_interval_schedule(&runtime, thread_id, objective, Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-original-goal", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        let replacement_goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                objective,
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("replacement goal should be created");
        assert_ne!(original_goal.goal_id, replacement_goal.goal_id);

        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run_for_goal(ThreadScheduleRunForGoalFinishParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "lease-original-goal",
                    completed_at: now + chrono::Duration::seconds(5),
                    next_run_at: Some(next_run_at),
                    expected_goal_id: original_goal.goal_id.as_str(),
                },)
                .await
                .expect("run should complete")
        );

        let completed = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Active, completed.status);
        assert_eq!(Some(next_run_at), completed.next_run_at);
        assert_eq!(None, completed.lease_id);
    }

    #[tokio::test]
    async fn goal_correlated_once_completion_expires_instead_of_pausing() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 43);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "finish once while held",
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("blocked goal should be created");
        let schedule = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "finish once while held".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: None,
            })
            .await
            .expect("one-time schedule should be created");
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-once", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");

        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run_for_goal(ThreadScheduleRunForGoalFinishParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "lease-once",
                    completed_at: now + chrono::Duration::seconds(5),
                    next_run_at: None,
                    expected_goal_id: goal.goal_id.as_str(),
                },)
                .await
                .expect("run should complete")
        );

        let completed = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, completed.status);
        assert_eq!(None, completed.next_run_at);
        assert_eq!(None, completed.lease_id);
    }

    #[tokio::test]
    async fn concurrent_goal_correlated_finalizers_complete_exactly_once() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id = test_thread_id(/*id*/ 42);
        upsert_test_thread(runtime.as_ref(), thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "pause once under contention",
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("blocked goal should be created");
        let schedule = create_interval_schedule(
            runtime.as_ref(),
            thread_id,
            "pause once under contention",
            Some(now),
        )
        .await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-contended", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");

        let contender_state_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(codex_home.join(crate::STATE_DB_FILENAME))
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_millis(1)),
            )
            .await
            .expect("contending state pool should open");
        let contender_goals_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(codex_home.join(crate::GOALS_DB_FILENAME))
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_millis(1)),
            )
            .await
            .expect("contending goals pool should open");
        let contender = ScheduleStore::new(
            Arc::new(contender_state_pool),
            Arc::new(contender_goals_pool),
        );
        let completed_at = now + chrono::Duration::seconds(5);
        let next_run_at = Some(now + chrono::Duration::minutes(5));
        let primary_completion = runtime
            .thread_schedules()
            .complete_thread_schedule_run_for_goal(ThreadScheduleRunForGoalFinishParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-contended",
                completed_at,
                next_run_at,
                expected_goal_id: goal.goal_id.as_str(),
            });
        let contender_completion =
            contender.complete_thread_schedule_run_for_goal(ThreadScheduleRunForGoalFinishParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-contended",
                completed_at,
                next_run_at,
                expected_goal_id: goal.goal_id.as_str(),
            });
        let (primary_result, contender_result) =
            tokio::join!(primary_completion, contender_completion);
        let completions = [
            primary_result.expect("primary finalizer should succeed"),
            contender_result.expect("contending finalizer should succeed"),
        ];
        assert_eq!(
            1,
            completions
                .into_iter()
                .filter(|completed| *completed)
                .count(),
            "the schedule lease must let exactly one finalizer commit"
        );

        let held_schedule = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Paused, held_schedule.status);
        assert_eq!(None, held_schedule.next_run_at);
        assert_eq!(None, held_schedule.lease_id);
        let run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&claim.run.run_id)
            .await
            .expect("run should load")
            .expect("run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Completed, run.status);
    }

    #[tokio::test]
    async fn late_terminal_and_expired_lease_reaper_settle_one_run_owner() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id = test_thread_id(/*id*/ 46);
        upsert_test_thread(runtime.as_ref(), thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(runtime.as_ref(), thread_id, "terminal race", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-terminal-race", Duration::from_secs(30))
            .await
            .expect("initial claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-terminal-race",
                turn_id: claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("run should start")
            .expect("run should still exist");

        let contender = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("contending state runtime should initialize");
        let retry_at = now + chrono::Duration::seconds(31);
        let completion = runtime.thread_schedules().complete_thread_schedule_run(
            &schedule.schedule_id,
            &claim.run.run_id,
            "lease-terminal-race",
            retry_at,
            Some(now + chrono::Duration::hours(1)),
        );
        let replacement = contender.thread_schedules().claim_due_thread_schedule(
            retry_at,
            "lease-reaper-race",
            Duration::from_secs(30),
        );
        let (completion, replacement) = tokio::join!(completion, replacement);
        let completion = completion.expect("late completion should not error");
        let replacement = replacement.expect("expired lease reaper should not error");
        assert_ne!(
            completion,
            replacement.is_some(),
            "either the terminal event or the reaper may own the old lease, never both"
        );

        let original_run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&claim.run.run_id)
            .await
            .expect("original run should load")
            .expect("original run should exist");
        assert_eq!(
            if completion {
                crate::ThreadScheduleRunStatus::Completed
            } else {
                crate::ThreadScheduleRunStatus::Leased
            },
            original_run.status,
            "the single occurrence must belong to either the terminal writer or replacement lease"
        );
        let stats = runtime
            .thread_schedules()
            .get_thread_schedule_stats(&schedule.schedule_id)
            .await
            .expect("schedule stats should load");
        assert_eq!(0, stats.running_runs);
        assert_eq!(i64::from(replacement.is_some()), stats.leased_runs);
        assert_eq!(1, stats.total_runs);
    }

    #[tokio::test]
    async fn expired_lease_reaper_prevents_delayed_start_resurrection() {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let thread_id = test_thread_id(/*id*/ 47);
        upsert_test_thread(runtime.as_ref(), thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(runtime.as_ref(), thread_id, "delayed start", Some(now)).await;
        let original_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-delayed-start", Duration::from_secs(30))
            .await
            .expect("initial claim should succeed")
            .expect("schedule should claim");
        let contender = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("contending state runtime should initialize");
        let retry_at = now + chrono::Duration::seconds(31);

        let delayed_start = runtime.thread_schedules().mark_thread_schedule_run_started(
            ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &original_claim.run.run_id,
                lease_id: "lease-delayed-start",
                turn_id: original_claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now: retry_at,
                lease_duration: Duration::from_secs(30),
            },
        );
        let replacement = contender.thread_schedules().claim_due_thread_schedule(
            retry_at,
            "lease-replacement",
            Duration::from_secs(30),
        );
        let (delayed_start, replacement) = tokio::join!(delayed_start, replacement);
        assert!(
            delayed_start
                .expect("delayed start should not error")
                .is_none(),
            "a reaped expired run must never become dispatchable"
        );
        let replacement = replacement
            .expect("expired lease reaper should not error")
            .expect("expired run should produce one replacement claim");

        let original_run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&original_claim.run.run_id)
            .await
            .expect("original run should load")
            .expect("original run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Leased, original_run.status);
        assert_eq!(None, original_run.completed_at);
        assert_eq!(None, original_run.error);
        assert_eq!(original_claim.run.run_id, replacement.run.run_id);
        let replacement_started_at = retry_at + chrono::Duration::seconds(1);
        let replacement_run = runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &replacement.run.run_id,
                lease_id: "lease-replacement",
                turn_id: replacement
                    .run
                    .turn_id
                    .as_deref()
                    .expect("replacement should preserve canonical turn id"),
                goal_id: None,
                now: replacement_started_at,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("replacement start should not error")
            .expect("replacement should remain the sole dispatchable run");
        assert_eq!(
            crate::ThreadScheduleRunStatus::Running,
            replacement_run.status
        );
        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &schedule.schedule_id,
                    &replacement.run.run_id,
                    "lease-replacement",
                    replacement_started_at + chrono::Duration::seconds(1),
                    Some(now + chrono::Duration::hours(1)),
                )
                .await
                .expect("replacement should remain finalizable")
        );

        let stats = runtime
            .thread_schedules()
            .get_thread_schedule_stats(&schedule.schedule_id)
            .await
            .expect("schedule stats should load");
        assert_eq!(1, stats.total_runs);
        assert_eq!(0, stats.leased_runs);
        assert_eq!(0, stats.running_runs);
        assert_eq!(1, stats.completed_runs);
        assert_eq!(0, stats.failed_runs);
    }

    #[tokio::test]
    async fn explicit_expiry_terminalizes_active_run_before_late_completion() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 32);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "expired while leased", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-expired", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");

        let expired = runtime
            .thread_schedules()
            .set_thread_schedule_status(&schedule.schedule_id, crate::ThreadScheduleStatus::Expired)
            .await
            .expect("schedule should expire")
            .expect("schedule should still exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, expired.status);
        assert_eq!(None, expired.lease_id);

        assert!(
            !runtime
                .thread_schedules()
                .complete_thread_schedule_run_and_pause(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-expired",
                    now + chrono::Duration::seconds(5),
                )
                .await
                .expect("late completion should fail closed")
        );

        let completed = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(
            crate::ThreadScheduleStatus::Expired,
            completed.status,
            "a late held completion must not downgrade expired to paused"
        );
        assert_eq!(None, completed.next_run_at);
        assert_eq!(None, completed.lease_id);
        let run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&claim.run.run_id)
            .await
            .expect("run should load")
            .expect("run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Failed, run.status);
        assert_eq!(Some(expired.updated_at), run.completed_at);
    }

    #[tokio::test]
    async fn terminal_schedule_status_rejects_late_finalizers() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 33);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);

        for (index, held_status) in [
            crate::ThreadScheduleStatus::Paused,
            crate::ThreadScheduleStatus::Expired,
        ]
        .into_iter()
        .enumerate()
        {
            let expected_next_run_at = match held_status {
                crate::ThreadScheduleStatus::Active => {
                    unreachable!("only terminal statuses tested")
                }
                crate::ThreadScheduleStatus::Paused => Some(now),
                crate::ThreadScheduleStatus::Expired => None,
            };
            let complete_lease = format!("lease-complete-{index}");
            let complete_schedule = create_interval_schedule(
                &runtime,
                thread_id,
                &format!("late complete {index}"),
                Some(now),
            )
            .await;
            let complete_claim = runtime
                .thread_schedules()
                .claim_due_thread_schedule(now, complete_lease.as_str(), Duration::from_secs(300))
                .await
                .expect("complete schedule should claim")
                .expect("complete schedule should be due");
            runtime
                .thread_schedules()
                .set_thread_schedule_status(&complete_schedule.schedule_id, held_status)
                .await
                .expect("complete schedule status should update")
                .expect("complete schedule should exist");
            assert!(
                !runtime
                    .thread_schedules()
                    .complete_thread_schedule_run(
                        &complete_schedule.schedule_id,
                        &complete_claim.run.run_id,
                        complete_lease.as_str(),
                        now + chrono::Duration::seconds(5),
                        Some(now + chrono::Duration::hours(1)),
                    )
                    .await
                    .expect("late completion should fail closed")
            );
            let after_complete = runtime
                .thread_schedules()
                .get_thread_schedule(&complete_schedule.schedule_id)
                .await
                .expect("complete schedule should load")
                .expect("complete schedule should exist");
            assert_eq!(held_status, after_complete.status);
            assert_eq!(expected_next_run_at, after_complete.next_run_at);
            assert_eq!(None, after_complete.lease_id);
            assert_eq!(
                crate::ThreadScheduleRunStatus::Failed,
                runtime
                    .thread_schedules()
                    .get_thread_schedule_run(&complete_claim.run.run_id)
                    .await
                    .expect("completed run should load")
                    .expect("completed run should exist")
                    .status
            );

            let defer_lease = format!("lease-defer-{index}");
            let defer_schedule = create_interval_schedule(
                &runtime,
                thread_id,
                &format!("late defer {index}"),
                Some(now),
            )
            .await;
            let defer_claim = runtime
                .thread_schedules()
                .claim_due_thread_schedule(now, defer_lease.as_str(), Duration::from_secs(300))
                .await
                .expect("deferred schedule should claim")
                .expect("deferred schedule should be due");
            runtime
                .thread_schedules()
                .set_thread_schedule_status(&defer_schedule.schedule_id, held_status)
                .await
                .expect("deferred schedule status should update")
                .expect("deferred schedule should exist");
            assert!(
                !runtime
                    .thread_schedules()
                    .defer_thread_schedule_run(
                        &defer_schedule.schedule_id,
                        &defer_claim.run.run_id,
                        defer_lease.as_str(),
                        now + chrono::Duration::seconds(5),
                        now + chrono::Duration::hours(1),
                        "held by goal status".to_string(),
                    )
                    .await
                    .expect("late deferral should fail closed")
            );
            let after_defer = runtime
                .thread_schedules()
                .get_thread_schedule(&defer_schedule.schedule_id)
                .await
                .expect("deferred schedule should load")
                .expect("deferred schedule should exist");
            assert_eq!(held_status, after_defer.status);
            assert_eq!(expected_next_run_at, after_defer.next_run_at);
            assert_eq!(None, after_defer.lease_id);
            let deferred_run = runtime
                .thread_schedules()
                .get_thread_schedule_run(&defer_claim.run.run_id)
                .await
                .expect("deferred run should load")
                .expect("deferred run should exist");
            assert_eq!(crate::ThreadScheduleRunStatus::Failed, deferred_run.status);
        }

        let failed_schedule =
            create_interval_schedule(&runtime, thread_id, "late failed hold", Some(now)).await;
        let failed_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-failed", Duration::from_secs(300))
            .await
            .expect("failed schedule should claim")
            .expect("failed schedule should be due");
        runtime
            .thread_schedules()
            .set_thread_schedule_status(
                &failed_schedule.schedule_id,
                crate::ThreadScheduleStatus::Expired,
            )
            .await
            .expect("failed schedule should expire")
            .expect("failed schedule should exist");
        assert!(
            !runtime
                .thread_schedules()
                .fail_thread_schedule_run_and_pause(
                    &failed_schedule.schedule_id,
                    &failed_claim.run.run_id,
                    "lease-failed",
                    now + chrono::Duration::seconds(5),
                    "goal held".to_string(),
                )
                .await
                .expect("late failure should fail closed")
        );
        let after_failure = runtime
            .thread_schedules()
            .get_thread_schedule(&failed_schedule.schedule_id)
            .await
            .expect("failed schedule should load")
            .expect("failed schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, after_failure.status);
        assert_eq!(None, after_failure.next_run_at);
        assert_eq!(None, after_failure.lease_id);
    }

    #[tokio::test]
    async fn complete_and_fail_thread_schedule_runs_release_the_lease() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 3);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let completed_schedule =
            create_interval_schedule(&runtime, thread_id, "completed task", Some(now)).await;
        let completed_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-complete", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");

        let running = runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &completed_schedule.schedule_id,
                run_id: &completed_claim.run.run_id,
                lease_id: "lease-complete",
                turn_id: completed_claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(300),
            })
            .await
            .expect("run should update")
            .expect("run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Running, running.status);
        assert_eq!(completed_claim.run.turn_id, running.turn_id);

        let next_run_at = now + chrono::Duration::minutes(5);
        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &completed_schedule.schedule_id,
                    &completed_claim.run.run_id,
                    "lease-complete",
                    now + chrono::Duration::seconds(5),
                    Some(next_run_at),
                )
                .await
                .expect("run should complete")
        );
        let after_complete = runtime
            .thread_schedules()
            .get_thread_schedule(&completed_schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(None, after_complete.lease_id);
        assert_eq!(Some(next_run_at), after_complete.next_run_at);
        assert_eq!(
            Some(now + chrono::Duration::seconds(5)),
            after_complete.last_run_at
        );
        assert_eq!(0, after_complete.failure_count);
        assert_eq!(
            crate::ThreadScheduleStats {
                total_runs: 1,
                completed_runs: 1,
                last_started_at: Some(now),
                last_completed_at: Some(now + chrono::Duration::seconds(5)),
                ..crate::ThreadScheduleStats::default()
            },
            runtime
                .thread_schedules()
                .get_thread_schedule_stats(&completed_schedule.schedule_id)
                .await
                .expect("completed run stats should load")
        );
        assert!(
            runtime
                .thread_schedules()
                .claim_due_thread_schedule(
                    next_run_at - chrono::Duration::seconds(1),
                    "lease-too-early",
                    Duration::from_secs(300),
                )
                .await
                .expect("claim should not fail")
                .is_none(),
            "completed schedule should not be claimed before its next_run_at"
        );
        let next_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(next_run_at, "lease-next", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim at next_run_at");
        assert_eq!(
            completed_schedule.schedule_id,
            next_claim.schedule.schedule_id
        );

        let failed_schedule =
            create_interval_schedule(&runtime, thread_id, "failed task", Some(now)).await;
        let failed_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-fail", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        assert!(
            runtime
                .thread_schedules()
                .fail_thread_schedule_run(
                    &failed_schedule.schedule_id,
                    &failed_claim.run.run_id,
                    "lease-fail",
                    now + chrono::Duration::seconds(10),
                    Some(now + chrono::Duration::minutes(10)),
                    "model unavailable".to_string(),
                )
                .await
                .expect("run should fail")
        );
        let after_failure = runtime
            .thread_schedules()
            .get_thread_schedule(&failed_schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(None, after_failure.lease_id);
        assert_eq!(1, after_failure.failure_count);
        assert_eq!(
            crate::ThreadScheduleStats {
                total_runs: 1,
                failed_runs: 1,
                last_started_at: Some(now),
                // A failed run did not complete, so last_completed_at stays null
                // to remain consistent with completed_runs == 0.
                last_completed_at: None,
                last_error: Some("model unavailable".to_string()),
                ..crate::ThreadScheduleStats::default()
            },
            runtime
                .thread_schedules()
                .get_thread_schedule_stats(&failed_schedule.schedule_id)
                .await
                .expect("failed run stats should load")
        );

        let failed_run_status: (String, String) =
            sqlx::query_as("SELECT status, error FROM thread_schedule_runs WHERE run_id = ?")
                .bind(&failed_claim.run.run_id)
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("failed run should be readable");
        assert_eq!(
            ("failed".to_string(), "model unavailable".to_string()),
            failed_run_status
        );
        assert_eq!(
            Some(crate::ThreadScheduleRun {
                status: crate::ThreadScheduleRunStatus::Failed,
                error: Some("model unavailable".to_string()),
                completed_at: Some(now + chrono::Duration::seconds(10)),
                ..failed_claim.run.clone()
            }),
            runtime
                .thread_schedules()
                .get_thread_schedule_run(&failed_claim.run.run_id)
                .await
                .expect("failed run should load through the schedule store")
        );
    }

    #[tokio::test]
    async fn resume_thread_schedule_resets_failure_count() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 14);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = create_interval_schedule(&runtime, thread_id, "retry me", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-fail", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .fail_thread_schedule_run(
                &schedule.schedule_id,
                &claim.run.run_id,
                "lease-fail",
                now + chrono::Duration::seconds(10),
                /*next_run_at*/ None,
                "model unavailable".to_string(),
            )
            .await
            .expect("run should fail");

        let after_failure = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, after_failure.status);
        assert_eq!(None, after_failure.next_run_at);
        assert_eq!(1, after_failure.failure_count);

        let resumed_at = now + chrono::Duration::minutes(5);
        let resumed = runtime
            .thread_schedules()
            .resume_thread_schedule_at(&schedule.schedule_id, resumed_at)
            .await
            .expect("schedule should resume")
            .expect("schedule should exist");
        assert_eq!(
            crate::ThreadSchedule {
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(resumed_at),
                failure_count: 0,
                updated_at: resumed.updated_at,
                ..after_failure
            },
            resumed
        );
    }

    #[tokio::test]
    async fn update_thread_schedule_to_active_resets_failure_count() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 15);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = create_interval_schedule(&runtime, thread_id, "retry me", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-fail", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .fail_thread_schedule_run(
                &schedule.schedule_id,
                &claim.run.run_id,
                "lease-fail",
                now + chrono::Duration::seconds(10),
                /*next_run_at*/ None,
                "model unavailable".to_string(),
            )
            .await
            .expect("run should fail");

        let after_failure = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, after_failure.status);
        assert_eq!(1, after_failure.failure_count);

        let resumed_at = now + chrono::Duration::minutes(5);
        let resumed = runtime
            .thread_schedules()
            .update_thread_schedule(
                &schedule.schedule_id,
                ThreadScheduleUpdate {
                    prompt: None,
                    prompt_source: None,
                    schedule: None,
                    timezone: None,
                    status: Some(crate::ThreadScheduleStatus::Active),
                    next_run_at: Some(Some(resumed_at)),
                    expires_at: None,
                },
            )
            .await
            .expect("schedule should update")
            .expect("schedule should exist");
        assert_eq!(
            crate::ThreadSchedule {
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(resumed_at),
                failure_count: 0,
                updated_at: resumed.updated_at,
                ..after_failure
            },
            resumed
        );
    }

    #[tokio::test]
    async fn defer_thread_schedule_run_rearms_without_incrementing_failure_count() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 16);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "wait for usage", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-wait", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        let completed_at = now + chrono::Duration::seconds(5);
        let retry_at = now + chrono::Duration::minutes(20);
        let error = "all eligible auth profiles are exhausted".to_string();
        let schedule_id = schedule.schedule_id.clone();

        assert!(
            runtime
                .thread_schedules()
                .defer_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-wait",
                    completed_at,
                    retry_at,
                    error.clone(),
                )
                .await
                .expect("run should defer")
        );

        let deferred = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(
            crate::ThreadSchedule {
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(retry_at),
                last_run_at: Some(completed_at),
                failure_count: 0,
                lease_id: None,
                lease_expires_at: None,
                updated_at: deferred.updated_at,
                ..schedule
            },
            deferred
        );
        let run_id = claim.run.run_id.clone();
        assert_eq!(
            Some(crate::ThreadScheduleRun {
                status: crate::ThreadScheduleRunStatus::Deferred,
                error: Some(error),
                completed_at: Some(completed_at),
                ..claim.run
            }),
            runtime
                .thread_schedules()
                .get_thread_schedule_run(&run_id)
                .await
                .expect("run should load")
        );
        assert_eq!(
            crate::ThreadScheduleStats {
                total_runs: 1,
                deferred_runs: 1,
                last_started_at: Some(now),
                // BUG-LOOP-001 regression: a deferred run re-arms the schedule
                // and does not complete, so last_completed_at must stay null
                // instead of reflecting the deferred run's finished-at timestamp.
                last_completed_at: None,
                last_error: None,
                ..crate::ThreadScheduleStats::default()
            },
            runtime
                .thread_schedules()
                .get_thread_schedule_stats(&schedule_id)
                .await
                .expect("deferred run stats should load")
        );
    }

    #[tokio::test]
    async fn schedule_stats_last_completed_at_tracks_only_completed_runs() {
        // BUG-LOOP-001 regression: with a mix of completed, deferred, and failed
        // runs on one schedule, last_completed_at must reflect the completed
        // run's finished-at timestamp only -- never a later deferred or failed
        // run -- so that last_completed_at is non-null iff completed_runs > 0.
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 17);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "mixed status runs", Some(now)).await;
        let schedule_id = schedule.schedule_id.clone();

        // Run 1: completes at now + 5s and re-arms 5 minutes later.
        let completed_at = now + chrono::Duration::seconds(5);
        let second_run_at = now + chrono::Duration::minutes(5);
        let claim_one = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-1", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &schedule_id,
                    &claim_one.run.run_id,
                    "lease-1",
                    completed_at,
                    Some(second_run_at),
                )
                .await
                .expect("run should complete")
        );

        // Run 2: defers at second_run_at + 5s (later than the completed run) and
        // re-arms 20 minutes later. A deferred finished-at must not leak in.
        let deferred_at = second_run_at + chrono::Duration::seconds(5);
        let third_run_at = second_run_at + chrono::Duration::minutes(20);
        let claim_two = runtime
            .thread_schedules()
            .claim_due_thread_schedule(second_run_at, "lease-2", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        assert!(
            runtime
                .thread_schedules()
                .defer_thread_schedule_run(
                    &schedule_id,
                    &claim_two.run.run_id,
                    "lease-2",
                    deferred_at,
                    third_run_at,
                    "waiting for usage window".to_string(),
                )
                .await
                .expect("run should defer")
        );

        // Run 3: fails at third_run_at + 5s (the latest finished-at overall). A
        // failed finished-at must not leak into last_completed_at either.
        let failed_at = third_run_at + chrono::Duration::seconds(5);
        let fourth_run_at = third_run_at + chrono::Duration::minutes(5);
        let claim_three = runtime
            .thread_schedules()
            .claim_due_thread_schedule(third_run_at, "lease-3", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        assert!(
            runtime
                .thread_schedules()
                .fail_thread_schedule_run(
                    &schedule_id,
                    &claim_three.run.run_id,
                    "lease-3",
                    failed_at,
                    Some(fourth_run_at),
                    "model unavailable".to_string(),
                )
                .await
                .expect("run should fail")
        );

        assert_eq!(
            crate::ThreadScheduleStats {
                total_runs: 3,
                completed_runs: 1,
                deferred_runs: 1,
                failed_runs: 1,
                // Last claim (run 3) started at third_run_at.
                last_started_at: Some(third_run_at),
                // Only the completed run counts, even though the deferred and
                // failed runs finished afterwards.
                last_completed_at: Some(completed_at),
                last_error: Some("model unavailable".to_string()),
                ..crate::ThreadScheduleStats::default()
            },
            runtime
                .thread_schedules()
                .get_thread_schedule_stats(&schedule_id)
                .await
                .expect("mixed status stats should load")
        );
    }

    #[tokio::test]
    async fn expire_schedules_and_delete_thread_cleanup_schedule_rows() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 4);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let expired = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "expire me".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Dynamic,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: Some(now),
            })
            .await
            .expect("expired schedule should be created");
        let paused = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "pause me".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Dynamic,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Paused,
                next_run_at: Some(now),
                expires_at: Some(now),
            })
            .await
            .expect("paused schedule should be created");

        assert_eq!(
            1,
            runtime
                .thread_schedules()
                .expire_thread_schedules(now)
                .await
                .expect("expiration should update due active schedules")
        );
        assert_eq!(
            crate::ThreadScheduleStatus::Expired,
            runtime
                .thread_schedules()
                .get_thread_schedule(&expired.schedule_id)
                .await
                .expect("expired schedule should load")
                .expect("expired schedule should exist")
                .status
        );
        assert_eq!(
            crate::ThreadScheduleStatus::Paused,
            runtime
                .thread_schedules()
                .get_thread_schedule(&paused.schedule_id)
                .await
                .expect("paused schedule should load")
                .expect("paused schedule should exist")
                .status
        );

        assert_eq!(
            1,
            runtime
                .delete_thread(thread_id)
                .await
                .expect("thread should delete")
        );
        assert_eq!(
            Vec::<crate::ThreadSchedule>::new(),
            runtime
                .thread_schedules()
                .list_thread_schedules(thread_id)
                .await
                .expect("thread schedules should be removed")
        );
    }

    #[tokio::test]
    async fn expire_thread_schedules_preserves_valid_lease_until_completion() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 13);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "finish despite expiry".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: Some(now + chrono::Duration::seconds(10)),
            })
            .await
            .expect("one-time schedule should be created");
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-live", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-live",
                turn_id: claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(300),
            })
            .await
            .expect("run should update")
            .expect("run should exist");

        let after_expiry = now + chrono::Duration::seconds(20);
        assert_eq!(
            0,
            runtime
                .thread_schedules()
                .expire_thread_schedules(after_expiry)
                .await
                .expect("valid lease should prevent expiry")
        );
        let still_leased = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Active, still_leased.status);
        assert_eq!(Some("lease-live".to_string()), still_leased.lease_id);

        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-live",
                    after_expiry,
                    /*next_run_at*/ None,
                )
                .await
                .expect("run should complete after schedule expiry")
        );
        let completed = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, completed.status);
        assert_eq!(None, completed.lease_id);
    }

    #[tokio::test]
    async fn expire_thread_schedules_clears_expired_lease() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 14);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "abandoned run".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: Some(now + chrono::Duration::seconds(10)),
            })
            .await
            .expect("one-time schedule should be created");
        runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-abandoned", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");

        assert_eq!(
            1,
            runtime
                .thread_schedules()
                .expire_thread_schedules(now + chrono::Duration::seconds(40))
                .await
                .expect("expired lease should not block expiry")
        );
        let expired = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Expired, expired.status);
        assert_eq!(None, expired.lease_id);
    }

    #[tokio::test]
    async fn pause_and_expiry_terminalize_active_schedule_runs() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 50);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);

        let paused_schedule =
            create_interval_schedule(&runtime, thread_id, "pause active run", Some(now)).await;
        let paused_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-paused", Duration::from_secs(300))
            .await
            .expect("paused schedule claim should succeed")
            .expect("paused schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &paused_schedule.schedule_id,
                run_id: &paused_claim.run.run_id,
                lease_id: "lease-paused",
                turn_id: paused_claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(300),
            })
            .await
            .expect("paused run start should persist")
            .expect("paused run should start");
        let paused = runtime
            .thread_schedules()
            .set_thread_schedule_status(
                &paused_schedule.schedule_id,
                crate::ThreadScheduleStatus::Paused,
            )
            .await
            .expect("pause should succeed")
            .expect("paused schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Paused, paused.status);
        assert_eq!(None, paused.lease_id);
        let paused_run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&paused_claim.run.run_id)
            .await
            .expect("paused run should load")
            .expect("paused run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Failed, paused_run.status);
        assert_eq!(Some(paused.updated_at), paused_run.completed_at);

        let expiring_schedule = runtime
            .thread_schedules()
            .create_thread_schedule(ThreadScheduleCreateParams {
                thread_id,
                prompt: "expire active run".to_string(),
                prompt_source: crate::ThreadSchedulePromptSource::Inline,
                schedule: crate::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: crate::ThreadScheduleStatus::Active,
                next_run_at: Some(now),
                expires_at: Some(now + chrono::Duration::seconds(10)),
            })
            .await
            .expect("expiring schedule should create");
        let expiring_claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-expiring", Duration::from_secs(30))
            .await
            .expect("expiring schedule claim should succeed")
            .expect("expiring schedule should claim");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &expiring_schedule.schedule_id,
                run_id: &expiring_claim.run.run_id,
                lease_id: "lease-expiring",
                turn_id: expiring_claim
                    .run
                    .turn_id
                    .as_deref()
                    .expect("claim should have canonical turn id"),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("expiring run start should persist")
            .expect("expiring run should start");
        let expired_at = now + chrono::Duration::seconds(31);
        assert_eq!(
            1,
            runtime
                .thread_schedules()
                .expire_thread_schedules(expired_at)
                .await
                .expect("expiry cleanup should succeed")
        );
        let expired_run = runtime
            .thread_schedules()
            .get_thread_schedule_run(&expiring_claim.run.run_id)
            .await
            .expect("expired run should load")
            .expect("expired run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Failed, expired_run.status);
        assert_eq!(Some(expired_at), expired_run.completed_at);

        for schedule_id in [
            paused_schedule.schedule_id.as_str(),
            expiring_schedule.schedule_id.as_str(),
        ] {
            let stats = runtime
                .thread_schedules()
                .get_thread_schedule_stats(schedule_id)
                .await
                .expect("schedule stats should load");
            assert_eq!(0, stats.leased_runs);
            assert_eq!(0, stats.running_runs);
        }
    }

    #[tokio::test]
    async fn expired_dispatch_is_adopted_as_one_logical_occurrence() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 61);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "adopt occurrence", Some(now)).await;
        let stale = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-stale", Duration::from_secs(10))
            .await
            .expect("stale claim should succeed")
            .expect("schedule should be due");
        let canonical_turn_id = stale
            .run
            .turn_id
            .clone()
            .expect("claim should persist a canonical turn id");
        let stale_running = runtime
            .thread_schedules()
            .materialize_thread_schedule_run(
                ThreadScheduleRunStartParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &stale.run.run_id,
                    lease_id: "lease-stale",
                    turn_id: canonical_turn_id.as_str(),
                    goal_id: None,
                    now,
                    lease_duration: Duration::from_secs(10),
                },
                Some("canonical scheduled input"),
            )
            .await
            .expect("stale dispatch should persist")
            .expect("stale dispatch should own its lease");
        assert_eq!(Some(canonical_turn_id.clone()), stale_running.turn_id);
        let materialized: (String, i64) = sqlx::query_as(
            "SELECT turn_input, materialized_at_ms FROM thread_schedule_runs WHERE run_id = ?",
        )
        .bind(&stale.run.run_id)
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("materialized input should load");
        assert_eq!(
            ("canonical scheduled input".to_string(), 1_700_000_000_000),
            materialized
        );

        let replacement_at = now + chrono::Duration::seconds(11);
        let replacement = runtime
            .thread_schedules()
            .claim_due_thread_schedule(replacement_at, "lease-replacement", Duration::from_secs(10))
            .await
            .expect("replacement claim should succeed")
            .expect("expired occurrence should be adopted");
        assert_eq!(stale.run.run_id, replacement.run.run_id);
        assert_eq!(Some(canonical_turn_id.clone()), replacement.run.turn_id);
        assert!(
            runtime
                .thread_schedules()
                .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &stale.run.run_id,
                    lease_id: "lease-stale",
                    turn_id: "turn-stale-duplicate",
                    goal_id: None,
                    now: replacement_at,
                    lease_duration: Duration::from_secs(10),
                })
                .await
                .expect("stale dispatch check should not fail")
                .is_none(),
            "the replaced lease must not submit"
        );
        let replacement_running = runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &replacement.run.run_id,
                lease_id: "lease-replacement",
                turn_id: canonical_turn_id.as_str(),
                goal_id: None,
                now: replacement_at,
                lease_duration: Duration::from_secs(10),
            })
            .await
            .expect("replacement dispatch should persist")
            .expect("replacement should own the occurrence");
        assert_eq!(Some(canonical_turn_id), replacement_running.turn_id);
        let run_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_schedule_runs WHERE schedule_id = ?")
                .bind(&schedule.schedule_id)
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("run count should load");
        assert_eq!(1, run_count);
    }

    #[tokio::test]
    async fn deferred_dispatch_reuses_occurrence_and_canonical_turn() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 62);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "defer occurrence", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-first", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should be due");
        let canonical_turn_id = claim
            .run
            .turn_id
            .clone()
            .expect("claim should persist a canonical turn id");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-first",
                turn_id: canonical_turn_id.as_str(),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(30),
            })
            .await
            .expect("dispatch should persist")
            .expect("dispatch should own lease");
        let retry_at = now + chrono::Duration::seconds(30);
        assert!(
            runtime
                .thread_schedules()
                .defer_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-first",
                    now + chrono::Duration::seconds(1),
                    retry_at,
                    "thread busy".to_string(),
                )
                .await
                .expect("deferral should succeed")
        );
        let retry = runtime
            .thread_schedules()
            .claim_due_thread_schedule(retry_at, "lease-retry", Duration::from_secs(30))
            .await
            .expect("retry claim should succeed")
            .expect("deferred occurrence should retry");
        assert_eq!(claim.run.run_id, retry.run.run_id);
        assert_eq!(Some(canonical_turn_id), retry.run.turn_id);
        let occurrence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT occurrence_id) FROM thread_schedule_runs WHERE schedule_id = ?",
        )
        .bind(&schedule.schedule_id)
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("occurrence count should load");
        assert_eq!(1, occurrence_count);
    }

    #[tokio::test]
    async fn completed_run_supersedes_older_deferred_history_during_claim() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 67);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "ignore stale history", Some(now)).await;
        for (run_id, occurrence_id, status, started_at_ms, completed_at_ms) in [
            (
                "historical-deferred",
                "historical-deferred",
                "deferred",
                1_699_999_980_000_i64,
                None,
            ),
            (
                "later-completed",
                "later-completed",
                "completed",
                1_699_999_990_000_i64,
                Some(1_699_999_995_000_i64),
            ),
        ] {
            sqlx::query(
                r#"
INSERT INTO thread_schedule_runs (
    run_id,
    schedule_id,
    thread_id,
    occurrence_id,
    status,
    lease_id,
    turn_id,
    scheduled_for_ms,
    started_at_ms,
    completed_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(run_id)
            .bind(&schedule.schedule_id)
            .bind(thread_id.to_string())
            .bind(occurrence_id)
            .bind(status)
            .bind(format!("lease-{run_id}"))
            .bind(format!("turn-{run_id}"))
            .bind(started_at_ms)
            .bind(started_at_ms)
            .bind(completed_at_ms)
            .execute(runtime.pool.as_ref())
            .await
            .expect("historical run should insert");
        }

        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-current", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should remain due");
        assert_ne!("historical-deferred", claim.run.run_id);
        assert_ne!("later-completed", claim.run.run_id);
        assert_eq!(crate::ThreadScheduleRunStatus::Leased, claim.run.status);
        assert_eq!(claim.run.run_id, claim.run.occurrence_id);
    }

    #[tokio::test]
    async fn replacement_pauses_materialized_occurrence_without_rearming() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 63);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "settle occurrence", Some(now)).await;
        let first = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-first", Duration::from_secs(10))
            .await
            .expect("claim should succeed")
            .expect("schedule should be due");
        let canonical_turn_id = first
            .run
            .turn_id
            .clone()
            .expect("claim should persist a canonical turn id");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &first.run.run_id,
                lease_id: "lease-first",
                turn_id: canonical_turn_id.as_str(),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(10),
            })
            .await
            .expect("reservation should persist")
            .expect("reservation should own lease");
        let replacement_at = now + chrono::Duration::seconds(11);
        let replacement = runtime
            .thread_schedules()
            .claim_due_thread_schedule(replacement_at, "lease-replacement", Duration::from_secs(30))
            .await
            .expect("replacement should claim")
            .expect("reserved occurrence should be recoverable");
        assert_eq!(first.run.run_id, replacement.run.run_id);
        assert!(
            runtime
                .thread_schedules()
                .fail_thread_schedule_run_and_pause(
                    &schedule.schedule_id,
                    &replacement.run.run_id,
                    "lease-replacement",
                    replacement_at,
                    "interrupted before finalization".to_string(),
                )
                .await
                .expect("replacement should settle")
        );
        assert!(
            !runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &schedule.schedule_id,
                    &first.run.run_id,
                    "lease-first",
                    replacement_at,
                    Some(replacement_at + chrono::Duration::minutes(10)),
                )
                .await
                .expect("stale terminal write should fail closed")
        );
        let settled = runtime
            .thread_schedules()
            .get_thread_schedule_run(&first.run.run_id)
            .await
            .expect("settled run should load")
            .expect("settled run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Failed, settled.status);
        assert!(settled.completed_at.is_some());
        let paused = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Paused, paused.status);
        assert_eq!(None, paused.next_run_at);
        assert_eq!(None, paused.lease_id);
    }

    #[tokio::test]
    async fn legacy_reaper_cannot_insert_second_dispatched_occurrence() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 64);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "legacy occurrence", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-new", Duration::from_secs(10))
            .await
            .expect("claim should succeed")
            .expect("schedule should be due");
        let canonical_turn_id = claim
            .run
            .turn_id
            .clone()
            .expect("claim should persist a canonical turn id");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: &schedule.schedule_id,
                run_id: &claim.run.run_id,
                lease_id: "lease-new",
                turn_id: canonical_turn_id.as_str(),
                goal_id: None,
                now,
                lease_duration: Duration::from_secs(10),
            })
            .await
            .expect("dispatch should persist")
            .expect("dispatch should own lease");

        let mut tx = runtime
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("legacy transaction should begin");
        sqlx::query("UPDATE thread_schedule_runs SET status = 'failed' WHERE run_id = ?")
            .bind(&claim.run.run_id)
            .execute(&mut *tx)
            .await
            .expect("legacy reaper update should stage");
        let legacy_insert = sqlx::query(
            r#"
INSERT INTO thread_schedule_runs (
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    scheduled_for_ms,
    started_at_ms
) VALUES (?, ?, ?, 'leased', ?, ?, ?)
            "#,
        )
        .bind("legacy-replacement")
        .bind(&schedule.schedule_id)
        .bind(thread_id.to_string())
        .bind("lease-legacy")
        .bind(claim.run.scheduled_for.map(datetime_to_epoch_millis))
        .bind(datetime_to_epoch_millis(
            now + chrono::Duration::seconds(11),
        ))
        .execute(&mut *tx)
        .await;
        assert!(
            legacy_insert.is_err(),
            "an older runtime must fail closed instead of minting a second dispatched occurrence"
        );
        tx.rollback()
            .await
            .expect("legacy transaction should roll back");
        let surviving = runtime
            .thread_schedules()
            .get_thread_schedule_run(&claim.run.run_id)
            .await
            .expect("surviving run should load")
            .expect("surviving run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Running, surviving.status);
        assert_eq!(Some(canonical_turn_id), surviving.turn_id);
    }

    #[tokio::test]
    async fn legacy_replacement_cannot_bypass_unmaterialized_occurrence() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 65);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "legacy sibling", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-original", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should be due");
        assert!(
            runtime
                .thread_schedules()
                .defer_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-original",
                    now + chrono::Duration::seconds(1),
                    now + chrono::Duration::seconds(30),
                    "legacy retry".to_string(),
                )
                .await
                .expect("deferral should succeed")
        );

        let replacement = sqlx::query(
            r#"
INSERT INTO thread_schedule_runs (
    run_id,
    schedule_id,
    thread_id,
    status,
    lease_id,
    scheduled_for_ms,
    started_at_ms
) VALUES (?, ?, ?, 'leased', ?, ?, ?)
            "#,
        )
        .bind("legacy-sibling")
        .bind(&schedule.schedule_id)
        .bind(thread_id.to_string())
        .bind("lease-legacy")
        .bind(claim.run.scheduled_for.map(datetime_to_epoch_millis))
        .bind(datetime_to_epoch_millis(
            now + chrono::Duration::seconds(30),
        ))
        .execute(runtime.pool.as_ref())
        .await;
        assert!(
            replacement.is_err(),
            "an older runtime must fail closed instead of replacing an unmaterialized occurrence"
        );
        let run_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_schedule_runs WHERE schedule_id = ?")
                .bind(&schedule.schedule_id)
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("run count should load");
        assert_eq!(1, run_count);
        let original_status: String =
            sqlx::query_scalar("SELECT status FROM thread_schedule_runs WHERE run_id = ?")
                .bind(&claim.run.run_id)
                .fetch_one(runtime.pool.as_ref())
                .await
                .expect("original occurrence should load");
        assert_eq!("deferred", original_status);
        let materialized_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM thread_schedule_runs WHERE occurrence_id = ? AND materialized_at_ms IS NOT NULL",
        )
        .bind(&claim.run.occurrence_id)
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("materialized occurrence count should load");
        assert_eq!(0, materialized_count);
    }

    #[tokio::test]
    async fn deferred_occurrence_reuses_attached_goal_without_recreation() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 66);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule =
            create_interval_schedule(&runtime, thread_id, "goal binding", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-goal", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should be due");
        assert!(
            runtime
                .thread_schedules()
                .attach_thread_schedule_run_goal(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-goal",
                    "goal-canonical",
                    now,
                )
                .await
                .expect("goal binding should persist")
        );
        let retry_at = now + chrono::Duration::seconds(30);
        assert!(
            runtime
                .thread_schedules()
                .defer_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-goal",
                    now + chrono::Duration::seconds(1),
                    retry_at,
                    "thread busy".to_string(),
                )
                .await
                .expect("goal occurrence should defer")
        );
        let retry = runtime
            .thread_schedules()
            .claim_due_thread_schedule(retry_at, "lease-retry", Duration::from_secs(30))
            .await
            .expect("retry should claim")
            .expect("deferred goal occurrence should retry");
        assert_eq!(claim.run.run_id, retry.run.run_id);
        assert_eq!(Some("goal-canonical".to_string()), retry.run.goal_id);
        assert!(
            !runtime
                .thread_schedules()
                .attach_thread_schedule_run_goal(
                    &schedule.schedule_id,
                    &retry.run.run_id,
                    "lease-retry",
                    "goal-conflict",
                    retry_at,
                )
                .await
                .expect("conflicting goal binding should fail closed")
        );
    }

    #[tokio::test]
    async fn held_attached_goal_pauses_deferred_occurrence_before_retry() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 67);
        upsert_test_thread(&runtime, thread_id).await;
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "hold deferred occurrence",
                crate::ThreadGoalStatus::Active,
                /*token_budget*/ None,
            )
            .await
            .expect("goal should persist");
        let now = at(/*seconds*/ 1_700_000_000);
        let schedule = create_interval_schedule(&runtime, thread_id, "goal hold", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-goal", Duration::from_secs(30))
            .await
            .expect("claim should succeed")
            .expect("schedule should be due");
        assert!(
            runtime
                .thread_schedules()
                .attach_thread_schedule_run_goal(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-goal",
                    &goal.goal_id,
                    now,
                )
                .await
                .expect("goal binding should persist")
        );
        let retry_at = now + chrono::Duration::seconds(30);
        assert!(
            runtime
                .thread_schedules()
                .defer_thread_schedule_run(
                    &schedule.schedule_id,
                    &claim.run.run_id,
                    "lease-goal",
                    now + chrono::Duration::seconds(1),
                    retry_at,
                    "thread busy".to_string(),
                )
                .await
                .expect("goal occurrence should defer")
        );
        runtime
            .thread_goals()
            .pause_active_thread_goal(thread_id)
            .await
            .expect("goal pause should persist")
            .expect("active goal should pause");
        assert!(
            runtime
                .thread_schedules()
                .claim_due_thread_schedule(retry_at, "lease-retry", Duration::from_secs(30))
                .await
                .expect("held goal claim should settle")
                .is_none(),
            "held goal must pause before a deferred occurrence retries"
        );
        let paused = runtime
            .thread_schedules()
            .get_thread_schedule(&schedule.schedule_id)
            .await
            .expect("schedule should load")
            .expect("schedule should exist");
        assert_eq!(crate::ThreadScheduleStatus::Paused, paused.status);
        let failed = runtime
            .thread_schedules()
            .get_thread_schedule_run(&claim.run.run_id)
            .await
            .expect("run should load")
            .expect("run should exist");
        assert_eq!(crate::ThreadScheduleRunStatus::Failed, failed.status);
    }
}
