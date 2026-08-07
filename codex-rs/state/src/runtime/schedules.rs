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

mod occurrence;
pub use occurrence::ThreadScheduleClaim;
pub use occurrence::ThreadScheduleDueClaimParams;
pub use occurrence::ThreadScheduleNowClaimParams;
pub use occurrence::ThreadScheduleOccurrenceState;
pub use occurrence::ThreadScheduleRunEnqueueParams;
pub use occurrence::ThreadScheduleRunForGoalFinishParams;
pub use occurrence::ThreadScheduleRunLeaseParams;
pub use occurrence::ThreadScheduleRunStartParams;

struct ScheduleNesting {
    parent_schedule_id: Option<String>,
    nesting_depth: i64,
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
        // Only non-active transitions release the lease. `lease_id` must stay
        // out of the SET list otherwise: `UPDATE OF lease_id` triggers such as
        // `thread_schedules_ignore_legacy_live_owner_claim` fire on the mere
        // mention of the column and would silently drop the whole update.
        let release_lease_clause = if matches!(status, crate::ThreadScheduleStatus::Active) {
            ""
        } else {
            r#"
    lease_id = NULL,
    lease_expires_at_ms = NULL,"#
        };
        let sql = schedule_returning(&format!(
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
    failure_count = CASE WHEN ? THEN 0 ELSE failure_count END,{release_lease_clause}
    updated_at_ms = ?
WHERE schedule_id = ?
RETURNING
"#
        ));
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
                sqlx::query("DELETE FROM thread_schedule_occurrences WHERE schedule_id = ?")
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
    status,
    lease_id,
    turn_id,
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
        create_interval_schedule_minutes(
            runtime,
            thread_id,
            prompt,
            /*minutes*/ 5,
            next_run_at,
        )
        .await
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

    async fn enqueue_and_start_claim(
        runtime: &StateRuntime,
        claim: &ThreadScheduleClaim,
        goal_id: Option<&str>,
        turn_input: &str,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> crate::ThreadScheduleRun {
        let turn_id = claim
            .run
            .turn_id
            .as_deref()
            .expect("claimed occurrence should have a stable turn id");
        runtime
            .thread_schedules()
            .enqueue_thread_schedule_run(ThreadScheduleRunEnqueueParams {
                schedule_id: claim.schedule.schedule_id.as_str(),
                run_id: claim.run.run_id.as_str(),
                lease_id: claim.run.lease_id.as_str(),
                goal_id,
                auth_profile_recorded: false,
                auth_profile: None,
                turn_input,
                now,
            })
            .await
            .expect("claimed occurrence should enqueue")
            .expect("claimed occurrence should retain its lease");
        runtime
            .thread_schedules()
            .mark_thread_schedule_run_started(ThreadScheduleRunStartParams {
                schedule_id: claim.schedule.schedule_id.as_str(),
                run_id: claim.run.run_id.as_str(),
                lease_id: claim.run.lease_id.as_str(),
                turn_id,
                goal_id,
                now,
                lease_duration,
            })
            .await
            .expect("enqueued occurrence should start")
            .expect("enqueued occurrence should retain its lease")
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
        let parent = create_interval_schedule_minutes(
            &runtime,
            thread_id,
            "parent loop",
            /*minutes*/ 1,
            Some(now),
        )
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
        let root = create_interval_schedule_minutes(
            &runtime,
            thread_id,
            "root loop",
            /*minutes*/ 1,
            Some(now),
        )
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
        let parent = create_interval_schedule_minutes(
            &runtime,
            thread_id,
            "parent loop",
            /*minutes*/ 1,
            Some(now),
        )
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

        let interval_parent = create_interval_schedule_minutes(
            &runtime,
            thread_id,
            "interval parent",
            /*minutes*/ 5,
            Some(now),
        )
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
        let parent = create_interval_schedule_minutes(
            &runtime,
            thread_id,
            "parent loop",
            /*minutes*/ 1,
            Some(now),
        )
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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "once input",
            now,
            Duration::from_secs(300),
        )
        .await;

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

    #[path = "occurrence_tests.rs"]
    mod occurrence_tests;

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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "long running input",
            now,
            Duration::from_secs(300),
        )
        .await;

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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "stale input",
            now,
            Duration::from_secs(30),
        )
        .await;
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
        let recovered = runtime
            .thread_schedules()
            .claim_due_thread_schedule(expired_at, "lease-new", Duration::from_secs(30))
            .await
            .expect("reaper should not error")
            .expect("expired run should be recoverable");
        assert_eq!(claim.run.run_id, recovered.run.run_id);
        assert_eq!(claim.run.turn_id, recovered.run.turn_id);
        assert_eq!(
            crate::ThreadScheduleRunStatus::Running,
            recovered.run.status
        );
        let stats = runtime
            .thread_schedules()
            .get_thread_schedule_stats(&schedule.schedule_id)
            .await
            .expect("schedule stats should load");
        assert_eq!(1, stats.total_runs);
        assert_eq!(1, stats.running_runs);
    }

    #[tokio::test]
    async fn completed_schedule_run_for_held_goal_pauses_without_rearming() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 31);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(/*seconds*/ 1_700_000_000);
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "held task",
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("blocked goal should persist");
        let schedule = create_interval_schedule(&runtime, thread_id, "held task", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-held", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        enqueue_and_start_claim(
            &runtime,
            &claim,
            Some(goal.goal_id.as_str()),
            "held task input",
            now,
            Duration::from_secs(300),
        )
        .await;

        // The caller still asks for a rearm; the goal hold must win.
        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run_for_goal(ThreadScheduleRunForGoalFinishParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "lease-held",
                    completed_at: now + chrono::Duration::seconds(5),
                    next_run_at: Some(now + chrono::Duration::minutes(5)),
                    expected_goal_id: &goal.goal_id,
                })
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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            Some(original_goal.goal_id.as_str()),
            "original goal input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            Some(goal.goal_id.as_str()),
            "once goal input",
            now,
            Duration::from_secs(300),
        )
        .await;

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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            Some(goal.goal_id.as_str()),
            "contended goal input",
            now,
            Duration::from_secs(300),
        )
        .await;

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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "terminal race input",
            now,
            Duration::from_secs(30),
        )
        .await;

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
                crate::ThreadScheduleRunStatus::Running
            },
            original_run.status,
            "the same run is either terminalized or reclaimed without replacement"
        );
        if let Some(replacement) = replacement.as_ref() {
            assert_eq!(claim.run.run_id, replacement.run.run_id);
            assert_eq!(claim.run.turn_id, replacement.run.turn_id);
        }
        let stats = runtime
            .thread_schedules()
            .get_thread_schedule_stats(&schedule.schedule_id)
            .await
            .expect("schedule stats should load");
        assert_eq!(i64::from(replacement.is_some()), stats.running_runs);
        assert_eq!(0, stats.leased_runs);
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
                turn_id: "turn-delayed-start",
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
        let recovered = replacement
            .expect("expired lease reaper should not error")
            .expect("expired occurrence should be reclaimed");
        assert_eq!(original_claim.run.run_id, recovered.run.run_id);
        assert_eq!(original_claim.run.turn_id, recovered.run.turn_id);
        assert!(
            runtime
                .thread_schedules()
                .get_thread_schedule_run(&original_claim.run.run_id)
                .await
                .expect("run lookup should succeed")
                .is_none(),
            "waiting occurrence must not create a run row"
        );
        let replacement_started_at = retry_at + chrono::Duration::seconds(1);
        let replacement_run = enqueue_and_start_claim(
            &runtime,
            &recovered,
            None,
            "recovered waiting input",
            replacement_started_at,
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(
            crate::ThreadScheduleRunStatus::Running,
            replacement_run.status
        );
        assert!(
            runtime
                .thread_schedules()
                .complete_thread_schedule_run(
                    &schedule.schedule_id,
                    &recovered.run.run_id,
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
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "expired while leased",
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("blocked goal should persist");
        let schedule =
            create_interval_schedule(&runtime, thread_id, "expired while leased", Some(now)).await;
        let claim = runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "lease-expired", Duration::from_secs(300))
            .await
            .expect("claim should succeed")
            .expect("schedule should claim");
        enqueue_and_start_claim(
            &runtime,
            &claim,
            Some(goal.goal_id.as_str()),
            "expired active input",
            now,
            Duration::from_secs(300),
        )
        .await;

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
                .complete_thread_schedule_run_for_goal(ThreadScheduleRunForGoalFinishParams {
                    schedule_id: &schedule.schedule_id,
                    run_id: &claim.run.run_id,
                    lease_id: "lease-expired",
                    completed_at: now + chrono::Duration::seconds(5),
                    next_run_at: None,
                    expected_goal_id: &goal.goal_id,
                })
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
            enqueue_and_start_claim(
                &runtime,
                &complete_claim,
                None,
                "late complete input",
                now,
                Duration::from_secs(300),
            )
            .await;
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
            enqueue_and_start_claim(
                &runtime,
                &defer_claim,
                None,
                "late defer input",
                now,
                Duration::from_secs(300),
            )
            .await;
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

        let held_goal = runtime
            .thread_goals()
            .replace_thread_goal(
                thread_id,
                "late failed hold",
                crate::ThreadGoalStatus::Blocked,
                /*token_budget*/ None,
            )
            .await
            .expect("blocked goal should persist");
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
                .fail_thread_schedule_run_for_goal(
                    ThreadScheduleRunForGoalFinishParams {
                        schedule_id: &failed_schedule.schedule_id,
                        run_id: &failed_claim.run.run_id,
                        lease_id: "lease-failed",
                        completed_at: now + chrono::Duration::seconds(5),
                        next_run_at: None,
                        expected_goal_id: &held_goal.goal_id,
                    },
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

        let running = enqueue_and_start_claim(
            &runtime,
            &completed_claim,
            None,
            "completed input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &failed_claim,
            None,
            "failed input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "resume failure input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "update failure input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
    async fn update_active_thread_schedule_applies_while_legacy_lease_and_live_owner_exist() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 15);
        upsert_test_thread(&runtime, thread_id).await;
        let now = at(Utc::now().timestamp());
        let schedule =
            create_interval_schedule(&runtime, thread_id, "editable task", Some(now)).await;
        runtime
            .thread_schedules()
            .claim_due_thread_schedule(now, "legacy-lease", Duration::from_secs(300))
            .await
            .expect("legacy claim should succeed before a session is live")
            .expect("schedule should claim");
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

        // `thread_schedules_ignore_legacy_live_owner_claim` fires for any
        // update that mentions `lease_id`, so an active-status edit must leave
        // the lease columns untouched or the whole update is silently dropped.
        let updated = runtime
            .thread_schedules()
            .update_thread_schedule(
                &schedule.schedule_id,
                ThreadScheduleUpdate {
                    prompt: Some("edited task".to_string()),
                    prompt_source: None,
                    schedule: None,
                    timezone: None,
                    status: Some(crate::ThreadScheduleStatus::Active),
                    next_run_at: None,
                    expires_at: None,
                },
            )
            .await
            .expect("active update should not fail")
            .expect("active update should apply while a legacy lease is held");

        assert_eq!("edited task", updated.prompt);
        assert_eq!(crate::ThreadScheduleStatus::Active, updated.status);
        assert_eq!(Some("legacy-lease".to_string()), updated.lease_id);
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
                turn_id: None,
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
        enqueue_and_start_claim(
            &runtime,
            &claim_one,
            None,
            "completed stats input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &claim_three,
            None,
            "failed stats input",
            third_run_at,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &claim,
            None,
            "live input",
            now,
            Duration::from_secs(300),
        )
        .await;

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
        enqueue_and_start_claim(
            &runtime,
            &paused_claim,
            None,
            "paused input",
            now,
            Duration::from_secs(300),
        )
        .await;
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
        enqueue_and_start_claim(
            &runtime,
            &expiring_claim,
            None,
            "expiring input",
            now,
            Duration::from_secs(30),
        )
        .await;
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
}
